//! Stand-in tray apps, for trying the tray without installing any.
//!
//! Puts two icons in the tray of whatever desktop is running on the
//! session bus: one drawn from pixels with a menu, one named from the
//! icon theme. Prints what it is asked to do.
//!
//!     cargo run -p scuttle-tray --example fake-tray-item

use std::collections::HashMap;

use zbus::blocking::{connection, Proxy};
use zbus::interface;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Structure, Value};

type Pixmap = (i32, i32, Vec<u8>);

/// A filled circle on a clear background, in ARGB.
fn disc(size: i32, colour: [u8; 3]) -> Pixmap {
    let mut data = Vec::with_capacity((size * size * 4) as usize);
    let centre = (size as f32 - 1.0) / 2.0;
    let radius = size as f32 / 2.0 - 1.0;
    for y in 0..size {
        for x in 0..size {
            let distance = ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2)).sqrt();
            // Soften the edge over one pixel.
            let alpha = ((radius - distance + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            data.extend_from_slice(&[alpha, colour[0], colour[1], colour[2]]);
        }
    }
    (size, size, data)
}

struct Item {
    id: &'static str,
    title: &'static str,
    icon_name: &'static str,
    pixmaps: Vec<Pixmap>,
    menu: &'static str,
}

#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn id(&self) -> String {
        self.id.into()
    }
    #[zbus(property)]
    fn title(&self) -> String {
        self.title.into()
    }
    #[zbus(property)]
    fn status(&self) -> String {
        "Active".into()
    }
    #[zbus(property)]
    fn icon_name(&self) -> String {
        self.icon_name.into()
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> String {
        String::new()
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<Pixmap> {
        self.pixmaps.clone()
    }
    #[zbus(property)]
    fn tool_tip(&self) -> (String, Vec<Pixmap>, String, String) {
        (String::new(), Vec::new(), self.title.into(), String::new())
    }
    #[zbus(property)]
    fn menu(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(self.menu).unwrap()
    }
    #[zbus(property)]
    fn item_is_menu(&self) -> bool {
        false
    }

    fn activate(&self, x: i32, y: i32) {
        println!("{}: activate {x} {y}", self.id);
    }
    fn secondary_activate(&self, x: i32, y: i32) {
        println!("{}: secondary {x} {y}", self.id);
    }
    fn context_menu(&self, x: i32, y: i32) {
        println!("{}: context menu {x} {y}", self.id);
    }
    fn scroll(&self, delta: i32, orientation: &str) {
        println!("{}: scroll {delta} {orientation}", self.id);
    }
}

struct Menu;

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
impl Menu {
    fn about_to_show(&self, _id: i32) -> bool {
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
                    ("label", Value::from("Mute notifications")),
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
                    node(
                        41,
                        &[
                            ("label", Value::from("Online")),
                            ("toggle-type", Value::from("radio")),
                            ("toggle-state", Value::from(1i32)),
                        ],
                        vec![],
                    ),
                    node(
                        42,
                        &[
                            ("label", Value::from("Away")),
                            ("toggle-type", Value::from("radio")),
                            ("toggle-state", Value::from(0i32)),
                        ],
                        vec![],
                    ),
                ],
            ),
            node(
                5,
                &[
                    ("label", Value::from("Check for updates")),
                    ("enabled", Value::from(false)),
                ],
                vec![],
            ),
            node(6, &[("type", Value::from("separator"))], vec![]),
            node(7, &[("label", Value::from("_Quit"))], vec![]),
        ];
        (1, (0, HashMap::new(), children))
    }

    fn event(&self, id: i32, event_id: &str, _data: Value<'_>, _timestamp: u32) {
        println!("menu: {event_id} {id}");
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let chat = connection::Builder::session()?
        .name("org.example.FakeChat")?
        .serve_at(
            "/StatusNotifierItem",
            Item {
                id: "fake-chat",
                title: "Chat",
                icon_name: "",
                pixmaps: vec![disc(24, [0x7c, 0x9c, 0xff]), disc(64, [0x7c, 0x9c, 0xff])],
                menu: "/Menu",
            },
        )?
        .serve_at("/Menu", Menu)?
        .build()?;
    let network = connection::Builder::session()?
        .name("org.example.FakeNetwork")?
        .serve_at(
            "/StatusNotifierItem",
            Item {
                id: "fake-network",
                title: "Wired connection",
                icon_name: "network-wired-symbolic",
                pixmaps: Vec::new(),
                menu: "/NO_DBUSMENU",
            },
        )?
        .build()?;

    for (connection, name) in [
        (&chat, "org.example.FakeChat"),
        (&network, "org.example.FakeNetwork"),
    ] {
        Proxy::new(
            connection,
            "org.kde.StatusNotifierWatcher",
            "/StatusNotifierWatcher",
            "org.kde.StatusNotifierWatcher",
        )?
        .call_method("RegisterStatusNotifierItem", &(name,))?;
    }
    println!("Two icons are in the tray. Press Ctrl+C to remove them.");
    loop {
        std::thread::park();
    }
}
