//! A stand-in media player for the headless demo and for trying the
//! bar's media widget without a real player.
//!
//! Usage: fake-player [title] [artist]
//!
//! It appears on the session bus as an MPRIS player, starts "playing",
//! and flips between playing and paused when told to play or pause.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zbus::blocking::connection;
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::Value;

const PATH: &str = "/org/mpris/MediaPlayer2";

struct Player {
    playing: Arc<Mutex<bool>>,
    title: String,
    artist: String,
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    #[zbus(property)]
    fn playback_status(&self) -> String {
        if *self.playing.lock().unwrap() {
            "Playing"
        } else {
            "Paused"
        }
        .to_string()
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, Value<'static>> {
        let mut map = HashMap::new();
        map.insert("xesam:title".to_string(), Value::from(self.title.clone()));
        map.insert(
            "xesam:artist".to_string(),
            Value::from(vec![self.artist.clone()]),
        );
        map
    }

    async fn play_pause(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        {
            let mut playing = self.playing.lock().unwrap();
            *playing = !*playing;
        }
        let _ = self.playback_status_changed(&emitter).await;
    }

    fn next(&self) {}

    fn previous(&self) {}
}

struct Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    #[zbus(property)]
    fn identity(&self) -> String {
        "Fake Player".to_string()
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let title = args.next().unwrap_or_else(|| "Lo-fi mix".to_string());
    let artist = args.next().unwrap_or_else(|| "Various".to_string());
    let player = Player {
        playing: Arc::new(Mutex::new(true)),
        title,
        artist,
    };

    let connection = connection::Builder::session()
        .and_then(|b| b.name("org.mpris.MediaPlayer2.scuttle_fake"))
        .and_then(|b| b.serve_at(PATH, player))
        .and_then(|b| b.serve_at(PATH, Root))
        .and_then(|b| b.build());
    let _connection = match connection {
        Ok(connection) => connection,
        Err(e) => {
            eprintln!("Could not start the fake player: {e}");
            std::process::exit(1);
        }
    };
    println!("Fake player running. Press Ctrl+C to stop.");
    loop {
        std::thread::park();
    }
}
