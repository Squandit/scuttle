//! The D-Bus side of the tray: the watcher that apps register their
//! icons with, and the host that reads each icon and talks back.
//!
//! When nothing else on the bus is a watcher, Scuttle becomes one. If
//! another program already is (a second bar, say), Scuttle leaves it
//! in charge and reads the list of items from it instead.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, Thread};
use std::time::Duration;

use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{connection, Connection, MessageIterator, Proxy};
use zbus::fdo::{self, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::names::BusName;
use zbus::zvariant::{OwnedValue, Structure, Value};
use zbus::{interface, MatchRule};

use crate::menu::{parse_layout, MenuEntry};
use crate::model::{choose_icon, plain, registration, split_key, Item, Status};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const MENU: &str = "com.canonical.dbusmenu";

/// The icon size asked for when an app offers several, in pixels.
/// Bigger than the bar draws, so it stays sharp on a dense screen.
const WANTED_ICON: i32 = 48;

/// How long an app gets to answer before it is given up on. A hung
/// app must not hang the tray.
const PATIENCE: Duration = Duration::from_secs(3);

/// State shared between the watcher service, the signal listeners and
/// the worker that re-reads items.
#[derive(Default)]
struct Shared {
    /// Item keys in the order they registered. Only used when Scuttle
    /// is the watcher.
    registered: Mutex<Vec<String>>,
    /// Bus names whose items have changed and need reading again.
    stale: Mutex<HashSet<String>>,
    /// Something changed: the worker has work to do.
    dirty: AtomicBool,
    worker: Mutex<Option<Thread>>,
}

impl Shared {
    fn wake(&self) {
        self.dirty.store(true, Ordering::Release);
        if let Ok(worker) = self.worker.lock() {
            if let Some(worker) = worker.as_ref() {
                worker.unpark();
            }
        }
    }
}

/// The `org.kde.StatusNotifierWatcher` service.
struct Watcher {
    shared: Arc<Shared>,
}

#[interface(name = "org.kde.StatusNotifierWatcher")]
impl Watcher {
    /// An app announcing its tray icon.
    fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let (service, path) = registration(&sender, service)
            .ok_or_else(|| fdo::Error::InvalidArgs("That is not a tray item.".into()))?;
        let key = format!("{service}{path}");
        if let Ok(mut registered) = self.shared.registered.lock() {
            if !registered.contains(&key) {
                registered.push(key);
            }
        }
        self.shared.wake();
        Ok(())
    }

    /// Another program offering to show the tray. Scuttle already is
    /// one, so there is nothing to record.
    fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.shared
            .registered
            .lock()
            .map(|items| items.clone())
            .unwrap_or_default()
    }

    /// Apps check this before bothering to register.
    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }
}

fn string(value: Option<OwnedValue>) -> String {
    value
        .and_then(|value| match &*value {
            Value::Str(text) => Some(text.to_string()),
            Value::ObjectPath(path) => Some(path.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Reads a list of images, `a(iiay)`.
fn pixmaps(value: Option<OwnedValue>) -> Vec<(i32, i32, Vec<u8>)> {
    let Some(value) = value else {
        return Vec::new();
    };
    let Value::Array(images) = &*value else {
        return Vec::new();
    };
    images
        .inner()
        .iter()
        .filter_map(|image| {
            let Value::Structure(image) = image else {
                return None;
            };
            let [Value::I32(width), Value::I32(height), Value::Array(data)] = image.fields() else {
                return None;
            };
            let bytes = data
                .inner()
                .iter()
                .filter_map(|byte| match byte {
                    Value::U8(byte) => Some(*byte),
                    _ => None,
                })
                .collect();
            Some((*width, *height, bytes))
        })
        .collect()
}

/// Reads the words out of a tooltip, `(sa(iiay)ss)`: a title and a
/// longer description.
fn tooltip(value: Option<OwnedValue>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let Value::Structure(tip) = &*value else {
        return String::new();
    };
    let [_, _, Value::Str(title), Value::Str(description)] = tip.fields() else {
        return String::new();
    };
    let title = plain(title.as_str());
    let description = plain(description.as_str());
    match (title.is_empty(), description.is_empty()) {
        (_, true) => title,
        (true, false) => description,
        (false, false) => format!("{title}: {description}"),
    }
}

/// Reads one item from the app that owns it. `None` when the app does
/// not answer, which means it has gone.
fn read_item(connection: &Connection, key: &str) -> Option<Item> {
    let (service, path) = split_key(key)?;
    let proxy = Proxy::new(connection, service, path, ITEM).ok()?;
    let get = |name: &str| proxy.get_property::<OwnedValue>(name).ok();

    // The id is the one property every item has. No answer here means
    // there is nobody home.
    let id = get("Id")?;
    let status = Status::parse(&string(get("Status")));
    // While an app wants attention it may offer a different icon.
    let attention = (status == Status::NeedsAttention)
        .then(|| string(get("AttentionIconName")))
        .filter(|name| !name.is_empty());
    let icon_name = attention.unwrap_or_else(|| string(get("IconName")));
    // Images are only fetched when there is no named icon. They can be
    // large, and most apps that name an icon send none.
    let images = if icon_name.trim().is_empty() {
        pixmaps(get("IconPixmap"))
    } else {
        Vec::new()
    };
    let menu = string(get("Menu"));
    Some(Item {
        key: key.to_string(),
        id: string(Some(id)),
        title: string(get("Title")),
        status,
        icon: choose_icon(
            &icon_name,
            &string(get("IconThemePath")),
            &images,
            WANTED_ICON,
        ),
        tooltip: tooltip(get("ToolTip")),
        menu: Some(menu).filter(|path| path.starts_with('/') && path != "/NO_DBUSMENU"),
        is_menu: get("ItemIsMenu")
            .and_then(|value| bool::try_from(value).ok())
            .unwrap_or(false),
    })
}

/// A handle for acting on tray items. Cheap to clone.
///
/// Every method here waits for the app to answer, for up to a few
/// seconds if the app is slow, so call them off the UI thread.
#[derive(Clone)]
pub struct Tray {
    connection: Connection,
    owns_watcher: bool,
}

impl Tray {
    /// Whether Scuttle is the watcher apps register with, as opposed
    /// to reading the list from another program's.
    pub fn owns_watcher(&self) -> bool {
        self.owns_watcher
    }

    fn item(&self, key: &str) -> Result<Proxy<'static>, String> {
        let (service, path) = split_key(key).ok_or("That tray icon has gone.")?;
        Proxy::new(
            &self.connection,
            service.to_string(),
            path.to_string(),
            ITEM,
        )
        .map_err(|e| e.to_string())
    }

    fn call(&self, key: &str, method: &str, x: i32, y: i32) -> Result<(), String> {
        self.item(key)?
            .call_method(method, &(x, y))
            .map(|_| ())
            .map_err(|e| format!("The app did not respond: {e}"))
    }

    /// The main action, as on a left click. `x` and `y` say where on
    /// the screen the click was, for apps that open a window there.
    pub fn activate(&self, key: &str, x: i32, y: i32) -> Result<(), String> {
        self.call(key, "Activate", x, y)
    }

    /// The second action, as on a middle click.
    pub fn secondary_activate(&self, key: &str, x: i32, y: i32) -> Result<(), String> {
        self.call(key, "SecondaryActivate", x, y)
    }

    /// Asks the app to show its own menu, for apps that draw theirs
    /// themselves instead of describing it.
    pub fn context_menu(&self, key: &str, x: i32, y: i32) -> Result<(), String> {
        self.call(key, "ContextMenu", x, y)
    }

    /// Scrolling over the icon. Apps use it for things like volume.
    pub fn scroll(&self, key: &str, delta: i32, horizontal: bool) -> Result<(), String> {
        let direction = if horizontal { "horizontal" } else { "vertical" };
        self.item(key)?
            .call_method("Scroll", &(delta, direction))
            .map(|_| ())
            .map_err(|e| format!("The app did not respond: {e}"))
    }

    fn menu_proxy(&self, key: &str, menu_path: &str) -> Result<Proxy<'static>, String> {
        let (service, _) = split_key(key).ok_or("That tray icon has gone.")?;
        Proxy::new(
            &self.connection,
            service.to_string(),
            menu_path.to_string(),
            MENU,
        )
        .map_err(|e| e.to_string())
    }

    /// Fetches an item's menu.
    pub fn menu(&self, key: &str, menu_path: &str) -> Result<Vec<MenuEntry>, String> {
        let proxy = self.menu_proxy(key, menu_path)?;
        // Some apps only fill their menu in when told it is about to
        // be shown. Those that do not care may not implement this.
        let _ = proxy.call_method("AboutToShow", &(0i32,));
        let reply = proxy
            .call_method("GetLayout", &(0i32, -1i32, Vec::<&str>::new()))
            .map_err(|e| format!("The app did not send its menu: {e}"))?;
        let body = reply.body();
        // The reply is a revision number and the root of the menu:
        // an id, its properties, and its children.
        type Root = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);
        let (_revision, root): (u32, Root) = body
            .deserialize()
            .map_err(|e| format!("The app sent a menu Scuttle could not read: {e}"))?;
        Ok(parse_layout(&Value::Structure(Structure::from(root))))
    }

    /// Tells the app a menu entry was chosen.
    pub fn click_menu(&self, key: &str, menu_path: &str, id: i32) -> Result<(), String> {
        self.menu_proxy(key, menu_path)?
            .call_method("Event", &(id, "clicked", Value::from(0i32), 0u32))
            .map(|_| ())
            .map_err(|e| format!("The app did not respond: {e}"))
    }
}

/// Starts the tray on the session bus. `on_change` is called from a
/// background thread with every item, in the order they arrived, once
/// at the start and again whenever anything about them changes.
pub fn watch(on_change: impl Fn(Vec<Item>) + Send + 'static) -> Result<Tray, String> {
    watch_on(None, on_change)
}

/// Like [`watch`], on the bus at `address` instead of the session bus.
pub fn watch_on(
    address: Option<&str>,
    on_change: impl Fn(Vec<Item>) + Send + 'static,
) -> Result<Tray, String> {
    let builder = match address {
        Some(address) => connection::Builder::address(address),
        None => connection::Builder::session(),
    };
    let shared = Arc::new(Shared::default());
    let connection = builder
        .and_then(|builder| {
            builder
                .method_timeout(PATIENCE)
                .serve_at(
                    WATCHER_PATH,
                    Watcher {
                        shared: Arc::clone(&shared),
                    },
                )?
                .build()
        })
        .map_err(|e| format!("The tray is off: no D-Bus session bus ({e})."))?;

    // Become the watcher unless something else already is.
    let owns_watcher = matches!(
        connection.request_name_with_flags(WATCHER, RequestNameFlags::DoNotQueue.into()),
        Ok(RequestNameReply::PrimaryOwner)
    );
    if !owns_watcher {
        // Tell the watcher in charge that there is a tray to show
        // icons in. Apps wait for that before registering.
        let host = format!("org.kde.StatusNotifierHost-{}", std::process::id());
        let _ = connection.request_name(host.as_str());
        if let Ok(watcher) = Proxy::new(&connection, WATCHER, WATCHER_PATH, WATCHER) {
            let _ = watcher.call_method("RegisterStatusNotifierHost", &(host.as_str(),));
        }
    }

    let signal = || MatchRule::builder().msg_type(zbus::message::Type::Signal);
    // An item saying something about it changed.
    let item_changed = signal()
        .interface(ITEM)
        .map(|b| b.build())
        .map_err(|e| e.to_string())?;
    // A program joining or leaving the bus.
    let came_or_went = signal()
        .sender("org.freedesktop.DBus")
        .and_then(|b| b.interface("org.freedesktop.DBus"))
        .and_then(|b| b.member("NameOwnerChanged"))
        .map(|b| b.build())
        .map_err(|e| e.to_string())?;
    // Another watcher's list changing.
    let list_changed = signal()
        .interface(WATCHER)
        .map(|b| b.build())
        .map_err(|e| e.to_string())?;

    let worker = thread::spawn({
        let connection = connection.clone();
        let shared = Arc::clone(&shared);
        move || run(&connection, &shared, owns_watcher, on_change)
    });
    if let Ok(mut slot) = shared.worker.lock() {
        *slot = Some(worker.thread().clone());
    }
    shared.wake();

    for (rule, everything) in [
        (item_changed, false),
        (came_or_went, true),
        (list_changed, true),
    ] {
        let signals = MessageIterator::for_match_rule(rule, &connection, Some(64))
            .map_err(|e| e.to_string())?;
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            for message in signals.flatten() {
                if !everything {
                    // Only the app that spoke needs reading again.
                    if let Some(sender) = message.header().sender() {
                        if let Ok(mut stale) = shared.stale.lock() {
                            stale.insert(sender.to_string());
                        }
                    }
                }
                shared.wake();
            }
        });
    }

    Ok(Tray {
        connection,
        owns_watcher,
    })
}

/// The unique bus name behind `service`, which is what signals are
/// stamped with.
fn owner_of(bus: &DBusProxy<'_>, service: &str) -> Option<String> {
    if service.starts_with(':') {
        return Some(service.to_string());
    }
    let name = BusName::try_from(service).ok()?;
    bus.get_name_owner(name).ok().map(|owner| owner.to_string())
}

fn is_running(bus: &DBusProxy<'_>, service: &str) -> bool {
    BusName::try_from(service)
        .ok()
        .and_then(|name| bus.name_has_owner(name).ok())
        .unwrap_or(false)
}

fn emit(connection: &Connection, signal: &str, key: &str) {
    let _ = connection.emit_signal(None::<BusName<'_>>, WATCHER_PATH, WATCHER, signal, &(key,));
}

/// The worker: turns bursts of signals into one pass over the items.
fn run(
    connection: &Connection,
    shared: &Shared,
    owns_watcher: bool,
    on_change: impl Fn(Vec<Item>),
) {
    let Ok(bus) = DBusProxy::new(connection) else {
        return;
    };
    // What is known about each item, and the unique bus name it
    // belongs to.
    let mut known: HashMap<String, (Item, String)> = HashMap::new();
    let mut last: Option<Vec<Item>> = None;
    loop {
        if !shared.dirty.swap(false, Ordering::AcqRel) {
            thread::park();
            continue;
        }
        // Let a burst finish: an app changing its icon often sends
        // several signals in a row.
        thread::sleep(Duration::from_millis(40));
        shared.dirty.store(false, Ordering::Release);
        let stale: HashSet<String> = shared
            .stale
            .lock()
            .map(|mut stale| std::mem::take(&mut *stale))
            .unwrap_or_default();

        let keys: Vec<String> = if owns_watcher {
            shared
                .registered
                .lock()
                .map(|registered| registered.clone())
                .unwrap_or_default()
        } else {
            Proxy::new(connection, WATCHER, WATCHER_PATH, WATCHER)
                .and_then(|watcher| {
                    watcher.get_property::<Vec<String>>("RegisteredStatusNotifierItems")
                })
                .unwrap_or_default()
                .into_iter()
                // Other watchers name items their own way. Bring them
                // to the one form used here.
                .filter_map(|name| registration("", &name))
                .map(|(service, path)| format!("{service}{path}"))
                .collect()
        };

        let mut items = Vec::with_capacity(keys.len());
        let mut gone = Vec::new();
        for key in &keys {
            let Some((service, _)) = split_key(key) else {
                gone.push(key.clone());
                continue;
            };
            if !is_running(&bus, service) {
                gone.push(key.clone());
                continue;
            }
            let fresh = match known.get(key) {
                Some((_, owner)) => stale.contains(owner),
                None => true,
            };
            if fresh {
                match read_item(connection, key) {
                    Some(item) => {
                        let owner = owner_of(&bus, service).unwrap_or_default();
                        if owns_watcher && !known.contains_key(key) {
                            emit(connection, "StatusNotifierItemRegistered", key);
                        }
                        known.insert(key.clone(), (item, owner));
                    }
                    None => {
                        gone.push(key.clone());
                        continue;
                    }
                }
            }
            if let Some((item, _)) = known.get(key) {
                items.push(item.clone());
            }
        }

        for key in &gone {
            known.remove(key);
            if owns_watcher {
                emit(connection, "StatusNotifierItemUnregistered", key);
            }
        }
        if owns_watcher && !gone.is_empty() {
            if let Ok(mut registered) = shared.registered.lock() {
                registered.retain(|key| !gone.contains(key));
            }
        }
        known.retain(|key, _| keys.contains(key));

        if last.as_ref() != Some(&items) {
            last = Some(items.clone());
            on_change(items);
        }
    }
}
