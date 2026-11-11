//! A small, dependency-light client for Hyprland's two IPC sockets.
//!
//! * The command socket answers one request per connection (`workspaces`,
//!   `clients`, `dispatch ...`).
//! * The event socket streams one line per compositor event.
//!
//! [`mock`] contains a stand-in server that speaks the same protocol, so
//! everything built on this crate can be tested without Hyprland running.

pub mod event;
pub mod mock;
pub mod types;

use std::env;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use event::{parse_event, Event};
pub use types::{Client, Monitor, Workspace, WorkspaceRef};

/// A handle to one running Hyprland instance.
#[derive(Debug, Clone)]
pub struct Hyprland {
    command_socket: PathBuf,
    event_socket: PathBuf,
}

impl Hyprland {
    /// Finds the instance this process was started under, using
    /// `HYPRLAND_INSTANCE_SIGNATURE`. Returns `None` when not running
    /// under Hyprland.
    pub fn from_env() -> Option<Hyprland> {
        let signature = env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
        if signature.is_empty() {
            return None;
        }
        let mut candidates = Vec::new();
        if let Some(runtime) = env::var_os("XDG_RUNTIME_DIR") {
            candidates.push(PathBuf::from(runtime).join("hypr").join(&signature));
        }
        // Older Hyprland releases kept their sockets in /tmp.
        candidates.push(PathBuf::from("/tmp/hypr").join(&signature));
        candidates
            .into_iter()
            .map(|dir| Hyprland::at(&dir))
            .find(|h| h.command_socket.exists())
    }

    /// Uses the sockets inside `dir` directly.
    pub fn at(dir: &Path) -> Hyprland {
        Hyprland {
            command_socket: dir.join(".socket.sock"),
            event_socket: dir.join(".socket2.sock"),
        }
    }

    /// Sends a raw request and returns the raw reply.
    pub fn request(&self, command: &str) -> io::Result<String> {
        let mut stream = UnixStream::connect(&self.command_socket)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        stream.write_all(command.as_bytes())?;
        stream.flush()?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply)?;
        Ok(reply)
    }

    fn json<T: serde::de::DeserializeOwned>(&self, what: &str) -> io::Result<T> {
        let reply = self.request(&format!("j/{what}"))?;
        serde_json::from_str(&reply).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Hyprland sent an unexpected reply to \"{what}\": {e}"),
            )
        })
    }

    /// All workspaces that currently exist, sorted by id.
    pub fn workspaces(&self) -> io::Result<Vec<Workspace>> {
        let mut list: Vec<Workspace> = self.json("workspaces")?;
        list.sort_by_key(|w| w.id);
        Ok(list)
    }

    /// The workspace that has focus.
    pub fn active_workspace(&self) -> io::Result<Workspace> {
        self.json("activeworkspace")
    }

    /// All mapped windows.
    pub fn clients(&self) -> io::Result<Vec<Client>> {
        let list: Vec<Client> = self.json("clients")?;
        Ok(list.into_iter().filter(|c| c.mapped && !c.title.is_empty()).collect())
    }

    /// The focused window, if any.
    pub fn active_window(&self) -> io::Result<Option<Client>> {
        let client: Client = self.json("activewindow")?;
        Ok(if client.address.is_empty() { None } else { Some(client) })
    }

    /// All connected monitors.
    pub fn monitors(&self) -> io::Result<Vec<Monitor>> {
        self.json("monitors")
    }

    /// Runs a dispatcher, e.g. `dispatch("workspace 3")`.
    pub fn dispatch(&self, args: &str) -> io::Result<()> {
        let reply = self.request(&format!("dispatch {args}"))?;
        if reply.trim() == "ok" {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "Hyprland refused \"dispatch {args}\": {}",
                reply.trim()
            )))
        }
    }

    /// Opens the event stream. Iterating blocks until the next event.
    pub fn events(&self) -> io::Result<EventStream> {
        let stream = UnixStream::connect(&self.event_socket)?;
        Ok(EventStream { reader: BufReader::new(stream) })
    }
}

/// A blocking iterator over compositor events.
pub struct EventStream {
    reader: BufReader<UnixStream>,
}

impl Iterator for EventStream {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) | Err(_) => return None,
                Ok(_) => {
                    if let Some(event) = parse_event(&line) {
                        return Some(event);
                    }
                }
            }
        }
    }
}
