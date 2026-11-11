//! Runs the notification service on a private D-Bus daemon and talks to
//! it the way an app would.
//!
//! These tests need the `dbus-daemon` program. Where it is missing they
//! print a note and pass without checking anything.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use scuttle_notify::{serve_on, Change, CloseReason, Handle, ServeError, Store, Urgency};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// A private bus that is shut down when the test ends.
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

    fn client(&self) -> Connection {
        zbus::blocking::connection::Builder::address(self.address.as_str())
            .unwrap()
            .build()
            .unwrap()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

struct Fixture {
    bus: Bus,
    handle: Handle,
    changes: Receiver<Change>,
}

fn fixture() -> Option<Fixture> {
    let Some(bus) = Bus::start() else {
        eprintln!("dbus-daemon is not installed; skipping the notification service test");
        return None;
    };
    let (tx, changes) = channel();
    let tx = Mutex::new(tx);
    let handle = serve_on(
        Some(&bus.address),
        Arc::new(Mutex::new(Store::default())),
        move |c| {
            let _ = tx.lock().unwrap().send(c);
        },
    )
    .expect("the service should start on a fresh bus");
    Some(Fixture {
        bus,
        handle,
        changes,
    })
}

#[allow(clippy::too_many_arguments)]
fn notify(
    client: &Connection,
    app: &str,
    replaces: u32,
    summary: &str,
    body: &str,
    actions: &[&str],
    hints: HashMap<&str, Value<'_>>,
    timeout: i32,
) -> u32 {
    client
        .call_method(
            Some(NAME),
            PATH,
            Some(NAME),
            "Notify",
            &(
                app,
                replaces,
                "dialog-information",
                summary,
                body,
                actions,
                hints,
                timeout,
            ),
        )
        .unwrap()
        .body()
        .deserialize::<u32>()
        .unwrap()
}

fn next_change(fixture: &Fixture) -> Change {
    fixture
        .changes
        .recv_timeout(Duration::from_secs(5))
        .expect("the UI should be told")
}

#[test]
fn an_app_can_send_a_notification() {
    let Some(f) = fixture() else { return };
    let client = f.bus.client();
    let mut hints = HashMap::new();
    hints.insert("urgency", Value::U8(2));
    let id = notify(
        &client,
        "Test App",
        0,
        "Build <b>finished</b>",
        "It took 41 seconds &amp; passed",
        &["default", "Open", "logs", "Show logs"],
        hints,
        -1,
    );
    assert_eq!(id, 1);

    let Change::Show(n) = next_change(&f) else {
        panic!("expected a new toast")
    };
    assert_eq!(n.id, 1);
    assert_eq!(n.app_name, "Test App");
    assert_eq!(n.summary, "Build finished");
    assert_eq!(n.body, "It took 41 seconds & passed");
    assert_eq!(n.urgency, Urgency::Critical);
    assert_eq!(n.icon, "dialog-information");
    assert_eq!(n.default_action().unwrap().label, "Open");
    assert_eq!(n.buttons().count(), 1);
}

#[test]
fn replacing_keeps_the_id_and_updates_the_toast() {
    let Some(f) = fixture() else { return };
    let client = f.bus.client();
    let id = notify(&client, "dl", 0, "10%", "", &[], HashMap::new(), -1);
    assert!(matches!(next_change(&f), Change::Show(_)));
    let again = notify(&client, "dl", id, "90%", "", &[], HashMap::new(), -1);
    assert_eq!(again, id);
    assert!(matches!(next_change(&f), Change::Replace(n) if n.summary == "90%"));
}

#[test]
fn an_image_path_hint_replaces_the_icon() {
    let Some(f) = fixture() else { return };
    let client = f.bus.client();
    let mut hints = HashMap::new();
    hints.insert("image-path", Value::from("file:///tmp/shot.png"));
    hints.insert("transient", Value::Bool(true));
    notify(&client, "shot", 0, "Saved", "", &[], hints, 3000);
    let Change::Show(n) = next_change(&f) else {
        panic!("expected a new toast")
    };
    assert_eq!(n.icon, "/tmp/shot.png");
    assert!(n.transient);
}

#[test]
fn the_app_hears_when_its_notification_is_dismissed_or_acted_on() {
    let Some(f) = fixture() else { return };
    let client = f.bus.client();
    let proxy = Proxy::new(&client, NAME, PATH, NAME).unwrap();
    let mut closed = proxy.receive_signal("NotificationClosed").unwrap();
    let mut invoked = proxy.receive_signal("ActionInvoked").unwrap();

    let id = notify(
        &client,
        "chat",
        0,
        "New message",
        "",
        &["reply", "Reply"],
        HashMap::new(),
        -1,
    );
    next_change(&f);

    f.handle.invoke(id, "reply");
    let (got_id, key): (u32, String) = invoked.next().unwrap().body().deserialize().unwrap();
    assert_eq!((got_id, key.as_str()), (id, "reply"));

    let gone = f.handle.close(id, CloseReason::Dismissed).unwrap();
    assert_eq!(gone.summary, "New message");
    let (got_id, reason): (u32, u32) = closed.next().unwrap().body().deserialize().unwrap();
    assert_eq!((got_id, reason), (id, 2));

    // Closing it a second time is a no-op.
    assert!(f.handle.close(id, CloseReason::Dismissed).is_none());
}

#[test]
fn an_app_can_withdraw_its_own_notification() {
    let Some(f) = fixture() else { return };
    let client = f.bus.client();
    let proxy = Proxy::new(&client, NAME, PATH, NAME).unwrap();
    let mut closed = proxy.receive_signal("NotificationClosed").unwrap();

    let id = notify(&client, "app", 0, "Syncing", "", &[], HashMap::new(), 0);
    next_change(&f);
    client
        .call_method(Some(NAME), PATH, Some(NAME), "CloseNotification", &(id,))
        .unwrap();

    assert_eq!(next_change(&f), Change::Hide(id));
    let (got_id, reason): (u32, u32) = closed.next().unwrap().body().deserialize().unwrap();
    assert_eq!((got_id, reason), (id, 3));
    assert!(f.handle.store().lock().unwrap().active().is_empty());
}

#[test]
fn reports_its_capabilities_and_identity() {
    let Some(f) = fixture() else { return };
    let client = f.bus.client();
    let capabilities: Vec<String> = client
        .call_method(Some(NAME), PATH, Some(NAME), "GetCapabilities", &())
        .unwrap()
        .body()
        .deserialize()
        .unwrap();
    assert!(capabilities.contains(&"body".to_string()));
    assert!(capabilities.contains(&"actions".to_string()));
    // Bodies are shown as plain text, so markup must not be advertised.
    assert!(!capabilities.contains(&"body-markup".to_string()));

    let (name, _vendor, _version, spec): (String, String, String, String) = client
        .call_method(Some(NAME), PATH, Some(NAME), "GetServerInformation", &())
        .unwrap()
        .body()
        .deserialize()
        .unwrap();
    assert_eq!(name, "Scuttle");
    assert_eq!(spec, "1.2");
}

#[test]
fn a_second_daemon_on_the_same_bus_is_refused_with_a_clear_reason() {
    let Some(f) = fixture() else { return };
    let second = serve_on(
        Some(&f.bus.address),
        Arc::new(Mutex::new(Store::default())),
        |_| {},
    );
    let Err(error) = second else {
        panic!("two daemons must not both own the name")
    };
    assert!(matches!(error, ServeError::AlreadyRunning), "{error}");
    assert!(error.to_string().contains("mako"));
}

#[test]
fn a_missing_bus_is_an_error_not_a_hang() {
    let result = serve_on(
        Some("unix:path=/nonexistent/scuttle-test-bus"),
        Arc::new(Mutex::new(Store::default())),
        |_| {},
    );
    assert!(matches!(result, Err(ServeError::NoSessionBus(_))));
}
