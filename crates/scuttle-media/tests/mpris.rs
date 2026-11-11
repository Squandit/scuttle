//! Runs a stand-in media player on a private D-Bus daemon and checks
//! that it is found, followed and controlled.
//!
//! These tests need the `dbus-daemon` program. Where it is missing they
//! print a note and pass without checking anything.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use scuttle_media::{watch_on, Control, Track};
use zbus::blocking::{connection, Connection};
use zbus::interface;
use zbus::zvariant::Value;

const PATH: &str = "/org/mpris/MediaPlayer2";

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

/// The part of a player that reports and controls playback.
struct FakePlayer {
    status: Arc<Mutex<String>>,
    title: String,
    artists: Vec<String>,
    calls: Mutex<Sender<&'static str>>,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl FakePlayer {
    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.status.lock().unwrap().clone()
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, Value<'static>> {
        let mut map = HashMap::new();
        map.insert("xesam:title".to_string(), Value::from(self.title.clone()));
        map.insert(
            "xesam:artist".to_string(),
            Value::from(self.artists.clone()),
        );
        map
    }

    fn play_pause(&self) {
        let _ = self.calls.lock().unwrap().send("PlayPause");
    }

    fn next(&self) {
        let _ = self.calls.lock().unwrap().send("Next");
    }

    fn previous(&self) {
        let _ = self.calls.lock().unwrap().send("Previous");
    }
}

/// The part of a player that says what it is called.
struct FakeRoot;

#[interface(name = "org.mpris.MediaPlayer2")]
impl FakeRoot {
    #[zbus(property)]
    fn identity(&self) -> String {
        "Fake Player".to_string()
    }
}

struct Player {
    connection: Connection,
    status: Arc<Mutex<String>>,
    calls: Receiver<&'static str>,
}

impl Player {
    fn start(bus: &Bus, name: &str, status: &str, title: &str, artists: &[&str]) -> Player {
        let status = Arc::new(Mutex::new(status.to_string()));
        let (tx, calls) = channel();
        let player = FakePlayer {
            status: Arc::clone(&status),
            title: title.to_string(),
            artists: artists.iter().map(|s| s.to_string()).collect(),
            calls: Mutex::new(tx),
        };
        let connection = connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name(format!("org.mpris.MediaPlayer2.{name}"))
            .unwrap()
            .serve_at(PATH, player)
            .unwrap()
            .serve_at(PATH, FakeRoot)
            .unwrap()
            .build()
            .unwrap();
        Player {
            connection,
            status,
            calls,
        }
    }

    /// Changes the playback status and announces it, as a real player
    /// does.
    fn set_status(&self, status: &str) {
        *self.status.lock().unwrap() = status.to_string();
        let mut changed = HashMap::new();
        changed.insert("PlaybackStatus", Value::from(status));
        self.connection
            .emit_signal(
                None::<()>,
                PATH,
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                &("org.mpris.MediaPlayer2.Player", changed, Vec::<&str>::new()),
            )
            .unwrap();
    }
}

fn next(updates: &Receiver<Option<Track>>) -> Option<Track> {
    updates
        .recv_timeout(Duration::from_secs(5))
        .expect("the watcher should report a change")
}

fn watcher(bus: &Bus) -> (scuttle_media::Controller, Receiver<Option<Track>>) {
    let (tx, updates) = channel();
    let tx = Mutex::new(tx);
    let controller = watch_on(Some(&bus.address), move |track| {
        let _ = tx.lock().unwrap().send(track);
    })
    .expect("watching a fresh bus should work");
    (controller, updates)
}

#[test]
fn follows_a_player_from_start_to_exit() {
    let Some(bus) = Bus::start() else {
        eprintln!("dbus-daemon is not installed; skipping the media player test");
        return;
    };
    let (controller, updates) = watcher(&bus);

    // Nothing is playing yet.
    assert_eq!(next(&updates), None);
    assert!(controller.send(Control::PlayPause).is_err());

    // A player starts, paused, with a track loaded.
    let player = Player::start(
        &bus,
        "fake",
        "Paused",
        "Weightless",
        &["Marconi Union", "Guest"],
    );
    let track = next(&updates).expect("the new player should be shown");
    assert_eq!(track.player, "org.mpris.MediaPlayer2.fake");
    assert_eq!(track.identity, "Fake Player");
    assert_eq!(track.title, "Weightless");
    assert_eq!(track.artist, "Marconi Union, Guest");
    assert!(!track.playing);

    // It starts playing.
    player.set_status("Playing");
    assert!(next(&updates).unwrap().playing);

    // Controls reach it.
    controller.send(Control::PlayPause).unwrap();
    controller.send(Control::Next).unwrap();
    controller.send(Control::Previous).unwrap();
    let got: Vec<&str> = (0..3)
        .map(|_| player.calls.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect();
    assert_eq!(got, vec!["PlayPause", "Next", "Previous"]);

    // It quits.
    drop(player);
    assert_eq!(next(&updates), None);
    assert!(controller.send(Control::PlayPause).is_err());
}

#[test]
fn the_playing_player_is_shown_over_a_paused_one() {
    let Some(bus) = Bus::start() else {
        return;
    };
    let (controller, updates) = watcher(&bus);
    assert_eq!(next(&updates), None);

    let video = Player::start(&bus, "browser", "Paused", "A video", &[]);
    assert_eq!(next(&updates).unwrap().title, "A video");

    let music = Player::start(&bus, "music", "Playing", "A song", &["Someone"]);
    let shown = next(&updates).unwrap();
    assert_eq!(shown.title, "A song");
    assert!(shown.playing);

    // Controls go to the player that is shown, not the other one.
    controller.send(Control::PlayPause).unwrap();
    assert_eq!(
        music.calls.recv_timeout(Duration::from_secs(5)).unwrap(),
        "PlayPause"
    );
    assert!(video.calls.try_recv().is_err());

    // When the music stops and the video plays, the video takes over.
    music.set_status("Paused");
    video.set_status("Playing");
    let mut shown = next(&updates).unwrap();
    // The two changes may arrive as one update or two.
    if shown.title != "A video" || !shown.playing {
        shown = next(&updates).unwrap();
    }
    assert_eq!(shown.title, "A video");
    assert!(shown.playing);
}

#[test]
fn a_missing_bus_is_an_error_not_a_hang() {
    let result = watch_on(Some("unix:path=/nonexistent/scuttle-media-test"), |_| {});
    assert!(result.is_err());
}
