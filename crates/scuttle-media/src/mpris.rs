//! Talks to MPRIS players over D-Bus.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use zbus::blocking::{fdo::DBusProxy, Connection, MessageIterator, Proxy};
use zbus::zvariant::{Array, OwnedValue};
use zbus::MatchRule;

use crate::model::{pick, Control, Track};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const ROOT: &str = "org.mpris.MediaPlayer2";

fn text(value: &OwnedValue) -> Option<String> {
    value.downcast_ref::<String>().ok()
}

/// Artists arrive as a list of strings. Some players send one string.
fn artists(value: &OwnedValue) -> String {
    if let Some(single) = text(value) {
        return single;
    }
    let Ok(list) = value.downcast_ref::<Array>() else {
        return String::new();
    };
    list.iter()
        .filter_map(|item| item.downcast_ref::<String>().ok())
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reads what one player is doing. `None` when it does not answer.
fn read_player(connection: &Connection, name: &str) -> Option<Track> {
    let player = Proxy::new(connection, name, PATH, PLAYER).ok()?;
    let status: String = player.get_property("PlaybackStatus").ok()?;
    let metadata: HashMap<String, OwnedValue> = player.get_property("Metadata").unwrap_or_default();
    let identity = Proxy::new(connection, name, PATH, ROOT)
        .ok()
        .and_then(|root| root.get_property::<String>("Identity").ok())
        .filter(|s| !s.is_empty())
        // Fall back to the last part of the bus name.
        .unwrap_or_else(|| name.trim_start_matches(PREFIX).to_string());
    Some(Track {
        player: name.to_string(),
        identity,
        title: metadata
            .get("xesam:title")
            .and_then(text)
            .unwrap_or_default(),
        artist: metadata
            .get("xesam:artist")
            .map(artists)
            .unwrap_or_default(),
        playing: status == "Playing",
    })
}

/// Reads every running player.
fn read_all(connection: &Connection) -> Vec<Track> {
    let Ok(bus) = DBusProxy::new(connection) else {
        return Vec::new();
    };
    let Ok(names) = bus.list_names() else {
        return Vec::new();
    };
    let mut tracks: Vec<Track> = names
        .iter()
        .map(|n| n.as_str())
        .filter(|n| n.starts_with(PREFIX))
        .filter_map(|n| read_player(connection, n))
        .collect();
    // A stable order, so the choice between equals does not depend on
    // the order the bus happens to list names in.
    tracks.sort_by(|a, b| a.player.cmp(&b.player));
    tracks
}

/// Sends playback commands to the player currently being shown.
#[derive(Clone)]
pub struct Controller {
    connection: Connection,
    current: Arc<Mutex<Option<String>>>,
}

impl Controller {
    /// Sends `control` to the player being shown.
    pub fn send(&self, control: Control) -> Result<(), String> {
        let name = self
            .current
            .lock()
            .ok()
            .and_then(|current| current.clone())
            .ok_or_else(|| "No media player is running.".to_string())?;
        let player =
            Proxy::new(&self.connection, name.as_str(), PATH, PLAYER).map_err(|e| e.to_string())?;
        player
            .call_method(control.method(), &())
            .map(|_| ())
            .map_err(|e| format!("The player did not accept that: {e}"))
    }
}

/// Watches the session bus for media players. `on_change` is called
/// from a background thread with the track to show, or `None` when no
/// player is running, once at the start and then on every change.
pub fn watch(on_change: impl Fn(Option<Track>) + Send + 'static) -> Result<Controller, String> {
    watch_on(None, on_change)
}

/// Like [`watch`], on the bus at `address` instead of the session bus.
pub fn watch_on(
    address: Option<&str>,
    on_change: impl Fn(Option<Track>) + Send + 'static,
) -> Result<Controller, String> {
    let connection = match address {
        Some(address) => zbus::blocking::connection::Builder::address(address)
            .and_then(|builder| builder.build()),
        None => Connection::session(),
    }
    .map_err(|e| format!("Media controls are off: no D-Bus session bus ({e})."))?;

    // Two kinds of signal matter: a player's properties changing, and a
    // player appearing on or leaving the bus.
    let changed = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.DBus.Properties")
        .and_then(|b| b.member("PropertiesChanged"))
        .and_then(|b| b.path(PATH))
        .map(|b| b.build())
        .map_err(|e| e.to_string())?;
    let came_or_went = MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")
        .and_then(|b| b.interface("org.freedesktop.DBus"))
        .and_then(|b| b.member("NameOwnerChanged"))
        .and_then(|b| b.arg0ns(ROOT))
        .map(|b| b.build())
        .map_err(|e| e.to_string())?;

    let current: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let dirty = Arc::new(AtomicBool::new(true));

    // The worker turns bursts of signals into one re-read. A track
    // change fires several property signals in a row.
    let worker = thread::spawn({
        let connection = connection.clone();
        let current = Arc::clone(&current);
        let dirty = Arc::clone(&dirty);
        move || {
            let mut last: Option<Option<Track>> = None;
            loop {
                if !dirty.swap(false, Ordering::AcqRel) {
                    thread::park();
                    continue;
                }
                thread::sleep(Duration::from_millis(40));
                dirty.store(false, Ordering::Release);

                let tracks = read_all(&connection);
                let previous = current.lock().ok().and_then(|c| c.clone());
                let chosen = pick(previous.as_deref(), &tracks).cloned();
                if let Ok(mut slot) = current.lock() {
                    *slot = chosen.as_ref().map(|t| t.player.clone());
                }
                if last.as_ref() != Some(&chosen) {
                    last = Some(chosen.clone());
                    on_change(chosen);
                }
            }
        }
    });

    for rule in [changed, came_or_went] {
        let signals = MessageIterator::for_match_rule(rule, &connection, Some(32))
            .map_err(|e| e.to_string())?;
        let dirty = Arc::clone(&dirty);
        let worker = worker.thread().clone();
        thread::spawn(move || {
            for _ in signals {
                dirty.store(true, Ordering::Release);
                worker.unpark();
            }
        });
    }

    Ok(Controller {
        connection,
        current,
    })
}
