//! The `org.freedesktop.Notifications` D-Bus service.
//!
//! Apps call `Notify` here. The server records the notification in the
//! shared [`Store`] and tells the UI what changed. When the UI closes a
//! notification or the user presses one of its buttons, the [`Handle`]
//! sends the matching signal back to the app.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use zbus::blocking::{connection, Connection};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;
use zbus::{fdo, interface};

use crate::model::{
    pair_actions, plain_text, Change, CloseReason, Notification, Store, Timeout, Urgency,
};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// Called on a D-Bus thread whenever the UI needs to update.
type OnChange = Box<dyn Fn(Change) + Send + Sync>;

struct Server {
    store: Arc<Mutex<Store>>,
    on_change: OnChange,
}

fn hint_string(hints: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    hints
        .get(key)
        .and_then(|v| v.downcast_ref::<String>().ok())
        .filter(|s| !s.is_empty())
}

#[interface(name = "org.freedesktop.Notifications")]
impl Server {
    /// What this server supports, so apps know what to send.
    fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["body", "actions", "icon-static", "persistence"]
    }

    /// Name, vendor, version and the spec version implemented.
    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("Scuttle", "Scuttle", env!("CARGO_PKG_VERSION"), "1.2")
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let urgency = hints
            .get("urgency")
            .and_then(|v| v.downcast_ref::<u8>().ok())
            .map(Urgency::from_hint)
            .unwrap_or_default();
        let transient = hints
            .get("transient")
            .and_then(|v| v.downcast_ref::<bool>().ok())
            .unwrap_or(false);
        // An explicit image beats the app's icon.
        let icon = hint_string(&hints, "image-path")
            .or_else(|| hint_string(&hints, "image_path"))
            .unwrap_or(app_icon);
        let icon = icon
            .strip_prefix("file://")
            .map(str::to_string)
            .unwrap_or(icon);

        // Volume and brightness popups from scripts carry a level and a
        // tag that says "replace my previous one".
        let progress = hints.get("value").and_then(|v| {
            v.downcast_ref::<i32>()
                .ok()
                .map(i64::from)
                .or_else(|| v.downcast_ref::<u32>().ok().map(i64::from))
                .or_else(|| v.downcast_ref::<i64>().ok())
                .map(|level| level.clamp(0, 100) as u8)
        });
        let sync_tag = [
            "x-canonical-private-synchronous",
            "synchronous",
            "x-dunst-stack-tag",
        ]
        .iter()
        .find_map(|key| hint_string(&hints, key));
        let transient = transient || sync_tag.is_some();

        let notification = Notification {
            id: 0,
            app_name,
            icon,
            summary: plain_text(&summary),
            body: plain_text(&body),
            actions: pair_actions(&actions),
            urgency,
            timeout: Timeout::from_spec(expire_timeout),
            transient,
            progress,
            sync_tag,
            received_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };

        let (id, change) = match self.store.lock() {
            Ok(mut store) => store.add(notification, replaces_id),
            Err(_) => return 0,
        };
        if let Some(change) = change {
            (self.on_change)(change);
        }
        id
    }

    /// An app withdrawing its own notification.
    async fn close_notification(
        &self,
        id: u32,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let closed = self.store.lock().ok().and_then(|mut store| store.close(id));
        if closed.is_some() {
            (self.on_change)(Change::Hide(id));
            Self::notification_closed(&emitter, id, CloseReason::ClosedByApp as u32)
                .await
                .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        }
        Ok(())
    }

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

/// Why the notification service could not start.
#[derive(Debug)]
pub enum ServeError {
    /// There is no D-Bus session to attach to.
    NoSessionBus(String),
    /// Another notification daemon already owns the name.
    AlreadyRunning,
    Other(String),
}

impl fmt::Display for ServeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServeError::NoSessionBus(e) => {
                write!(
                    f,
                    "Notifications are off: no D-Bus session bus was found ({e})."
                )
            }
            ServeError::AlreadyRunning => f.write_str(
                "Notifications are off: another notification daemon (such as mako, dunst or \
                 swaync) is already running. Stop it and restart Scuttle to let Scuttle show them.",
            ),
            ServeError::Other(e) => write!(f, "Notifications are off: {e}"),
        }
    }
}

impl std::error::Error for ServeError {}

/// The UI's side of the service: reports back to apps.
#[derive(Clone)]
pub struct Handle {
    connection: Connection,
    store: Arc<Mutex<Store>>,
}

impl Handle {
    /// The store shared with the D-Bus service.
    pub fn store(&self) -> &Arc<Mutex<Store>> {
        &self.store
    }

    fn emit(&self, send: impl FnOnce(&SignalEmitter<'_>) -> zbus::Result<()>) {
        if let Ok(interface) = self.connection.object_server().interface::<_, Server>(PATH) {
            let _ = send(interface.signal_emitter());
        }
    }

    /// Removes a notification from the screen and tells its app why.
    /// Returns the notification if it was showing.
    pub fn close(&self, id: u32, reason: CloseReason) -> Option<Notification> {
        let closed = self.store.lock().ok()?.close(id)?;
        self.emit(|emitter| {
            zbus::block_on(Server::notification_closed(emitter, id, reason as u32))
        });
        Some(closed)
    }

    /// Tells the app that one of its notification's actions was chosen.
    pub fn invoke(&self, id: u32, action_key: &str) {
        self.emit(|emitter| zbus::block_on(Server::action_invoked(emitter, id, action_key)));
    }
}

/// Claims the notification service name on the session bus and starts
/// answering. `on_change` is called from a D-Bus thread.
pub fn serve(
    store: Arc<Mutex<Store>>,
    on_change: impl Fn(Change) + Send + Sync + 'static,
) -> Result<Handle, ServeError> {
    serve_on(None, store, on_change)
}

/// Like [`serve`], on the bus at `address` instead of the session bus.
pub fn serve_on(
    address: Option<&str>,
    store: Arc<Mutex<Store>>,
    on_change: impl Fn(Change) + Send + Sync + 'static,
) -> Result<Handle, ServeError> {
    let server = Server {
        store: Arc::clone(&store),
        on_change: Box::new(on_change),
    };
    let builder = match address {
        Some(address) => connection::Builder::address(address),
        None => connection::Builder::session(),
    }
    .map_err(|e| ServeError::NoSessionBus(e.to_string()))?;
    let connection = builder
        .serve_at(PATH, server)
        .map_err(|e| ServeError::Other(e.to_string()))?
        .build()
        .map_err(|e| ServeError::NoSessionBus(e.to_string()))?;

    // Ask for the name without queueing and without pushing out a
    // daemon that already has it: two daemons fighting over
    // notifications helps nobody, so the one already there keeps them.
    match connection.request_name_with_flags(NAME, RequestNameFlags::DoNotQueue.into()) {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {}
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) => {
            return Err(ServeError::AlreadyRunning)
        }
        Err(zbus::Error::NameTaken) => return Err(ServeError::AlreadyRunning),
        Err(e) => return Err(ServeError::Other(e.to_string())),
    }
    Ok(Handle { connection, store })
}
