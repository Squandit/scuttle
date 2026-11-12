//! Runs stand-in tray apps on a private D-Bus daemon and checks that
//! the tray finds them, follows them and passes clicks back.
//!
//! These tests need the `dbus-daemon` program. Where it is missing
//! they print a note and pass without checking anything.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use scuttle_tray::menu::Toggle;
use scuttle_tray::{watch_on, Icon, Item, Status, Tray};
use zbus::blocking::{connection, Connection, Proxy};
use zbus::interface;
use zbus::names::BusName;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Structure, Value};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM_PATH: &str = "/StatusNotifierItem";
const ITEM: &str = "org.kde.StatusNotifierItem";

struct Bus {
    daemon: Child,
    address: String,
}

impl Bus {
    fn start() -> Option<Bus> {
        let mut daemon = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut address = String::new();
        BufReader::new(daemon.stdout.take()?)
            .read_line(&mut address)
            .ok()?;
        let address = address.trim().to_string();
        if address.is_empty() {
            let _ = daemon.kill();
            return None;
        }
        Some(Bus { daemon, address })
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

macro_rules! bus_or_skip {
    () => {
        match Bus::start() {
            Some(bus) => bus,
            None => {
                eprintln!("dbus-daemon is not installed; skipping.");
                return;
            }
        }
    };
}

type Pixmap = (i32, i32, Vec<u8>);

/// What a stand-in app says about its tray icon.
#[derive(Clone)]
struct Facts {
    id: String,
    title: String,
    status: String,
    icon_name: String,
    theme_path: String,
    pixmaps: Vec<Pixmap>,
    tooltip: (String, String),
    menu: String,
    is_menu: bool,
}

impl Default for Facts {
    fn default() -> Self {
        Facts {
            id: "chat".into(),
            title: "Chat".into(),
            status: "Active".into(),
            icon_name: "chat-symbolic".into(),
            theme_path: String::new(),
            pixmaps: Vec::new(),
            tooltip: (String::new(), String::new()),
            menu: "/Menu".into(),
            is_menu: false,
        }
    }
}

struct FakeItem {
    facts: Arc<Mutex<Facts>>,
    calls: Mutex<Sender<String>>,
}

impl FakeItem {
    fn facts(&self) -> Facts {
        self.facts.lock().unwrap().clone()
    }

    fn record(&self, call: String) {
        let _ = self.calls.lock().unwrap().send(call);
    }
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl FakeItem {
    #[zbus(property)]
    fn id(&self) -> String {
        self.facts().id
    }
    #[zbus(property)]
    fn title(&self) -> String {
        self.facts().title
    }
    #[zbus(property)]
    fn status(&self) -> String {
        self.facts().status
    }
    #[zbus(property)]
    fn icon_name(&self) -> String {
        self.facts().icon_name
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> String {
        "chat-unread".into()
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> String {
        self.facts().theme_path
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        self.facts().pixmaps
    }
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Vec<Pixmap>, String, String) {
        let (title, description) = self.facts().tooltip;
        (String::new(), Vec::new(), title, description)
    }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(self.facts().menu).unwrap()
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        self.facts().is_menu
    }

    fn activate(&self, x: i32, y: i32) {
        self.record(format!("activate {x} {y}"));
    }
    fn secondary_activate(&self, x: i32, y: i32) {
        self.record(format!("secondary {x} {y}"));
    }
    fn context_menu(&self, x: i32, y: i32) {
        self.record(format!("context {x} {y}"));
    }
    fn scroll(&self, delta: i32, orientation: &str) {
        self.record(format!("scroll {delta} {orientation}"));
    }
}

struct FakeMenu {
    calls: Mutex<Sender<String>>,
}

fn node(id: i32, properties: &[(&str, Value<'static>)], children: Vec<OwnedValue>) -> OwnedValue {
    let properties: HashMap<String, OwnedValue> = properties
        .iter()
        .map(|(key, value)| {
            (
                key.to_string(),
                OwnedValue::try_from(value.clone()).unwrap(),
            )
        })
        .collect();
    OwnedValue::try_from(Value::Structure(Structure::from((
        id, properties, children,
    ))))
    .unwrap()
}

#[interface(name = "com.canonical.dbusmenu")]
impl FakeMenu {
    fn about_to_show(&self, id: i32) -> bool {
        let _ = self
            .calls
            .lock()
            .unwrap()
            .send(format!("about-to-show {id}"));
        false
    }

    #[allow(clippy::type_complexity)]
    fn get_layout(
        &self,
        _parent: i32,
        _depth: i32,
        _properties: Vec<String>,
    ) -> (u32, (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>)) {
        let children = vec![
            node(1, &[("label", Value::from("_Open Chat"))], vec![]),
            node(
                2,
                &[
                    ("label", Value::from("Mute")),
                    ("toggle-type", Value::from("checkmark")),
                    ("toggle-state", Value::from(1i32)),
                ],
                vec![],
            ),
            node(3, &[("type", Value::from("separator"))], vec![]),
            node(
                4,
                &[
                    ("label", Value::from("Status")),
                    ("children-display", Value::from("submenu")),
                ],
                vec![
                    node(41, &[("label", Value::from("Online"))], vec![]),
                    node(42, &[("label", Value::from("Away"))], vec![]),
                ],
            ),
            node(
                5,
                &[
                    ("label", Value::from("Secret")),
                    ("visible", Value::from(false)),
                ],
                vec![],
            ),
            node(6, &[("label", Value::from("_Quit"))], vec![]),
        ];
        (7, (0, HashMap::new(), children))
    }

    fn event(&self, id: i32, event_id: &str, _data: Value<'_>, _timestamp: u32) {
        let _ = self
            .calls
            .lock()
            .unwrap()
            .send(format!("event {id} {event_id}"));
    }
}

/// A stand-in app: its bus connection, what it says, and what it has
/// been asked to do.
struct App {
    connection: Connection,
    facts: Arc<Mutex<Facts>>,
    calls: Receiver<String>,
}

impl App {
    /// Connects an app that keeps its icon at `path`. With a `name` it
    /// also takes that well-known bus name.
    fn start(bus: &Bus, name: Option<&str>, path: &str, facts: Facts) -> App {
        let facts = Arc::new(Mutex::new(facts));
        let (tx, calls) = channel();
        let mut builder = connection::Builder::address(bus.address.as_str())
            .unwrap()
            .serve_at(
                path,
                FakeItem {
                    facts: Arc::clone(&facts),
                    calls: Mutex::new(tx.clone()),
                },
            )
            .unwrap()
            .serve_at(
                "/Menu",
                FakeMenu {
                    calls: Mutex::new(tx),
                },
            )
            .unwrap();
        if let Some(name) = name {
            builder = builder.name(name).unwrap();
        }
        App {
            connection: builder.build().unwrap(),
            facts,
            calls,
        }
    }

    fn unique_name(&self) -> String {
        self.connection.unique_name().unwrap().to_string()
    }

    /// Registers with the watcher, passing `argument` as apps do: a
    /// bus name or an object path.
    fn register(&self, argument: &str) {
        Proxy::new(&self.connection, WATCHER, WATCHER_PATH, WATCHER)
            .unwrap()
            .call_method("RegisterStatusNotifierItem", &(argument,))
            .unwrap();
    }

    fn signal(&self, path: &str, name: &str) {
        self.connection
            .emit_signal(None::<BusName<'_>>, path, ITEM, name, &())
            .unwrap();
    }

    fn next_call(&self) -> String {
        self.calls
            .recv_timeout(Duration::from_secs(5))
            .expect("the app was not called")
    }
}

/// A tray under test, and the item lists it has reported.
struct Watched {
    tray: Tray,
    updates: Receiver<Vec<Item>>,
}

fn start_tray(bus: &Bus) -> Watched {
    let (tx, updates) = channel();
    let tx = Mutex::new(tx);
    let tray = watch_on(Some(bus.address.as_str()), move |items| {
        let _ = tx.lock().unwrap().send(items);
    })
    .expect("the tray should start");
    Watched { tray, updates }
}

impl Watched {
    /// Waits for an update that satisfies `wanted`.
    fn until(&self, what: &str, wanted: impl Fn(&[Item]) -> bool) -> Vec<Item> {
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut last = None;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match self.updates.recv_timeout(left) {
                Ok(items) if wanted(&items) => return items,
                Ok(items) => last = Some(items),
                Err(_) => break,
            }
        }
        panic!("never saw: {what}. Last update: {last:?}");
    }
}

#[test]
fn an_app_that_registers_by_name_appears_with_what_it_says() {
    let bus = bus_or_skip!();
    let watched = start_tray(&bus);
    assert!(watched.tray.owns_watcher());
    watched.until("an empty tray to start with", |items| items.is_empty());

    let app = App::start(
        &bus,
        Some("org.example.Chat"),
        ITEM_PATH,
        Facts {
            tooltip: ("Chat".into(), "<b>3</b> unread &amp; 1 call".into()),
            ..Facts::default()
        },
    );
    app.register("org.example.Chat");

    let items = watched.until("the chat icon", |items| items.len() == 1);
    let item = &items[0];
    assert_eq!(item.key, "org.example.Chat/StatusNotifierItem");
    assert_eq!(item.id, "chat");
    assert_eq!(item.title, "Chat");
    assert_eq!(item.status, Status::Active);
    assert_eq!(
        item.icon,
        Icon::Named {
            name: "chat-symbolic".into(),
            theme_path: None
        }
    );
    assert_eq!(item.tooltip, "Chat: 3 unread & 1 call");
    assert_eq!(item.menu.as_deref(), Some("/Menu"));
    assert!(!item.is_menu);
}

#[test]
fn an_app_that_registers_by_path_is_found_under_its_own_name() {
    let bus = bus_or_skip!();
    let watched = start_tray(&bus);
    let path = "/org/ayatana/NotificationItem/network";
    let app = App::start(
        &bus,
        None,
        path,
        Facts {
            id: "network".into(),
            menu: "/NO_DBUSMENU".into(),
            is_menu: true,
            ..Facts::default()
        },
    );
    app.register(path);

    let items = watched.until("the network icon", |items| items.len() == 1);
    assert_eq!(items[0].key, format!("{}{path}", app.unique_name()));
    assert_eq!(items[0].id, "network");
    // "No menu" is spelled as a path by some apps.
    assert_eq!(items[0].menu, None);
    assert!(items[0].is_menu);
}

#[test]
fn the_watcher_lists_its_items_for_other_programs() {
    let bus = bus_or_skip!();
    let watched = start_tray(&bus);
    let app = App::start(&bus, Some("org.example.Chat"), ITEM_PATH, Facts::default());
    app.register("org.example.Chat");
    watched.until("the chat icon", |items| items.len() == 1);

    let watcher = Proxy::new(&app.connection, WATCHER, WATCHER_PATH, WATCHER).unwrap();
    let listed: Vec<String> = watcher
        .get_property("RegisteredStatusNotifierItems")
        .unwrap();
    assert_eq!(listed, ["org.example.Chat/StatusNotifierItem"]);
    let has_host: bool = watcher
        .get_property("IsStatusNotifierHostRegistered")
        .unwrap();
    assert!(has_host, "apps check this before registering");
    // Registering twice does not list the item twice.
    app.register("org.example.Chat");
    let listed: Vec<String> = watcher
        .get_property("RegisteredStatusNotifierItems")
        .unwrap();
    assert_eq!(listed.len(), 1);
}

#[test]
fn a_change_the_app_announces_is_picked_up() {
    let bus = bus_or_skip!();
    let watched = start_tray(&bus);
    let app = App::start(&bus, Some("org.example.Chat"), ITEM_PATH, Facts::default());
    app.register("org.example.Chat");
    watched.until("the chat icon", |items| items.len() == 1);

    {
        let mut facts = app.facts.lock().unwrap();
        facts.title = "Chat (2)".into();
        facts.status = "NeedsAttention".into();
    }
    app.signal(ITEM_PATH, "NewStatus");

    let items = watched.until("the new title", |items| {
        items.first().is_some_and(|item| item.title == "Chat (2)")
    });
    assert_eq!(items[0].status, Status::NeedsAttention);
    // While it wants attention, its attention icon is the one shown.
    assert_eq!(
        items[0].icon,
        Icon::Named {
            name: "chat-unread".into(),
            theme_path: None
        }
    );
}

#[test]
fn only_the_app_that_spoke_is_read_again() {
    let bus = bus_or_skip!();
    let watched = start_tray(&bus);
    let chat = App::start(&bus, Some("org.example.Chat"), ITEM_PATH, Facts::default());
    let music = App::start(
        &bus,
        Some("org.example.Music"),
        ITEM_PATH,
        Facts {
            id: "music".into(),
            title: "Music".into(),
            ..Facts::default()
        },
    );
    chat.register("org.example.Chat");
    music.register("org.example.Music");
    watched.until("both icons", |items| items.len() == 2);

    // Both change, but only the music app says so.
    chat.facts.lock().unwrap().title = "Chat changed quietly".into();
    music.facts.lock().unwrap().title = "Music changed aloud".into();
    music.signal(ITEM_PATH, "NewTitle");

    let items = watched.until("the music title", |items| {
        items.iter().any(|item| item.title == "Music changed aloud")
    });
    assert_eq!(
        items[0].title, "Chat",
        "items keep the order they arrived in"
    );
    assert_eq!(items[1].title, "Music changed aloud");
}

#[test]
fn an_icon_sent_as_pixels_is_converted() {
    let bus = bus_or_skip!();
    let watched = start_tray(&bus);
    // Two sizes of one opaque orange square, in ARGB as apps send it.
    let image = |size: i32| -> Pixmap {
        let mut data = Vec::new();
        for _ in 0..size * size {
            data.extend_from_slice(&[0xff, 0xf7, 0x4c, 0x00]);
        }
        (size, size, data)
    };
    let app = App::start(
        &bus,
        Some("org.example.Pixels"),
        ITEM_PATH,
        Facts {
            icon_name: String::new(),
            pixmaps: vec![image(16), image(64)],
            ..Facts::default()
        },
    );
    app.register("org.example.Pixels");

    let items = watched.until("the pixel icon", |items| items.len() == 1);
    let Icon::Pixels(pixels) = &items[0].icon else {
        panic!("expected pixels, got {:?}", items[0].icon);
    };
    assert_eq!((pixels.width, pixels.height), (64, 64));
    assert_eq!(pixels.rgba.len(), 64 * 64 * 4);
    assert_eq!(&pixels.rgba[..4], [0xf7, 0x4c, 0x00, 0xff]);
}

#[test]
fn an_app_that_quits_leaves_the_tray() {
    let bus = bus_or_skip!();
    let watched = start_tray(&bus);
    let stays = App::start(&bus, Some("org.example.Stays"), ITEM_PATH, Facts::default());
    let quits = App::start(&bus, Some("org.example.Quits"), ITEM_PATH, Facts::default());
    stays.register("org.example.Stays");
    quits.register("org.example.Quits");
    watched.until("both icons", |items| items.len() == 2);

    drop(quits);
    let items = watched.until("one icon left", |items| items.len() == 1);
    assert_eq!(items[0].key, "org.example.Stays/StatusNotifierItem");

    let watcher = Proxy::new(&stays.connection, WATCHER, WATCHER_PATH, WATCHER).unwrap();
    let listed: Vec<String> = watcher
        .get_property("RegisteredStatusNotifierItems")
        .unwrap();
    assert_eq!(listed, ["org.example.Stays/StatusNotifierItem"]);
}

#[test]
fn clicks_and_scrolling_reach_the_app() {
    let bus = bus_or_skip!();
    let watched = start_tray(&bus);
    let app = App::start(&bus, Some("org.example.Chat"), ITEM_PATH, Facts::default());
    app.register("org.example.Chat");
    let key = watched.until("the chat icon", |items| items.len() == 1)[0]
        .key
        .clone();

    watched.tray.activate(&key, 1200, 20).unwrap();
    assert_eq!(app.next_call(), "activate 1200 20");
    watched.tray.secondary_activate(&key, 5, 6).unwrap();
    assert_eq!(app.next_call(), "secondary 5 6");
    watched.tray.context_menu(&key, 7, 8).unwrap();
    assert_eq!(app.next_call(), "context 7 8");
    watched.tray.scroll(&key, -120, false).unwrap();
    assert_eq!(app.next_call(), "scroll -120 vertical");
    watched.tray.scroll(&key, 15, true).unwrap();
    assert_eq!(app.next_call(), "scroll 15 horizontal");
}

#[test]
fn the_menu_is_fetched_and_a_choice_is_sent_back() {
    let bus = bus_or_skip!();
    let watched = start_tray(&bus);
    let app = App::start(&bus, Some("org.example.Chat"), ITEM_PATH, Facts::default());
    app.register("org.example.Chat");
    let item = watched.until("the chat icon", |items| items.len() == 1)[0].clone();
    let menu_path = item.menu.clone().unwrap();

    let menu = watched.tray.menu(&item.key, &menu_path).unwrap();
    assert_eq!(app.next_call(), "about-to-show 0");
    let shape: Vec<String> = menu
        .iter()
        .map(|entry| {
            if entry.separator {
                "-".to_string()
            } else {
                entry.label.clone()
            }
        })
        .collect();
    // The hidden entry is gone and mnemonic underscores are removed.
    assert_eq!(shape, ["Open Chat", "Mute", "-", "Status", "Quit"]);
    assert_eq!(
        menu[1].toggle,
        Some(Toggle {
            radio: false,
            on: true
        })
    );
    let status: Vec<&str> = menu[3].children.iter().map(|e| e.label.as_str()).collect();
    assert_eq!(status, ["Online", "Away"]);

    watched
        .tray
        .click_menu(&item.key, &menu_path, menu[4].id)
        .unwrap();
    assert_eq!(app.next_call(), "event 6 clicked");
}

#[test]
fn an_app_that_has_gone_gives_an_error_not_a_hang() {
    let bus = bus_or_skip!();
    let watched = start_tray(&bus);
    let started = Instant::now();
    assert!(watched
        .tray
        .activate("org.example.Nobody/StatusNotifierItem", 0, 0)
        .is_err());
    assert!(watched
        .tray
        .menu("org.example.Nobody/StatusNotifierItem", "/Menu")
        .is_err());
    assert!(watched.tray.activate("not a key", 0, 0).is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_second_tray_reads_the_list_from_the_first() {
    let bus = bus_or_skip!();
    let first = start_tray(&bus);
    let app = App::start(&bus, Some("org.example.Chat"), ITEM_PATH, Facts::default());
    app.register("org.example.Chat");
    first.until("the chat icon in the first tray", |items| items.len() == 1);

    // Another bar on the same desktop. The first stays in charge.
    let second = start_tray(&bus);
    assert!(!second.tray.owns_watcher());
    let items = second.until("the chat icon in the second tray", |items| items.len() == 1);
    assert_eq!(items[0].key, "org.example.Chat/StatusNotifierItem");
    assert_eq!(items[0].id, "chat");

    // It follows the first one's list as apps come and go.
    let music = App::start(
        &bus,
        Some("org.example.Music"),
        ITEM_PATH,
        Facts {
            id: "music".into(),
            ..Facts::default()
        },
    );
    music.register("org.example.Music");
    second.until("both icons in the second tray", |items| items.len() == 2);
    drop(music);
    second.until("one icon again in the second tray", |items| {
        items.len() == 1
    });
}
