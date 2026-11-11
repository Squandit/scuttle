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
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub use event::{parse_event, Event};
pub use types::{Client, Monitor, Workspace, WorkspaceRef};

/// How a compositor expects actions to be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// Hyprland 0.55 and newer with a Lua config: `hl.dsp.focus({...})`.
    Lua = 1,
    /// Older Hyprland, or a hyprlang config: `workspace 3`.
    Legacy = 2,
}

const LEGACY: u8 = Dialect::Legacy as u8;

/// An action for the compositor to carry out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dispatch<'a> {
    /// Switch to a workspace by number.
    Workspace(i32),
    /// Focus a window by its address, e.g. `0x55d0c0a0`.
    FocusWindow(&'a str),
    /// Start a shell command.
    Exec(&'a str),
    /// End the Hyprland session.
    Exit,
}

/// Writes `text` as a Lua string literal.
fn lua_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl Dispatch<'_> {
    /// The action written for the given dialect, without the leading
    /// `dispatch` keyword.
    pub fn render(&self, dialect: Dialect) -> String {
        match (dialect, self) {
            (Dialect::Lua, Dispatch::Workspace(id)) => {
                format!("hl.dsp.focus({{ workspace = \"{id}\" }})")
            }
            (Dialect::Lua, Dispatch::FocusWindow(address)) => {
                format!(
                    "hl.dsp.focus({{ window = {} }})",
                    lua_string(&format!("address:{address}"))
                )
            }
            (Dialect::Lua, Dispatch::Exec(command)) => {
                format!("hl.dsp.exec_cmd({})", lua_string(command))
            }
            (Dialect::Lua, Dispatch::Exit) => "hl.dsp.exit()".to_string(),
            (Dialect::Legacy, Dispatch::Workspace(id)) => format!("workspace {id}"),
            (Dialect::Legacy, Dispatch::FocusWindow(address)) => {
                format!("focuswindow address:{address}")
            }
            (Dialect::Legacy, Dispatch::Exec(command)) => format!("exec {command}"),
            (Dialect::Legacy, Dispatch::Exit) => "exit".to_string(),
        }
    }
}

/// A handle to one running Hyprland instance.
#[derive(Debug, Clone)]
pub struct Hyprland {
    command_socket: PathBuf,
    event_socket: PathBuf,
    /// The dialect that last worked, shared between clones. 0 is unknown.
    dialect: Arc<AtomicU8>,
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
            dialect: Arc::new(AtomicU8::new(0)),
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
        Ok(list
            .into_iter()
            .filter(|c| c.mapped && !c.title.is_empty())
            .collect())
    }

    /// The focused window, if any.
    pub fn active_window(&self) -> io::Result<Option<Client>> {
        let client: Client = self.json("activewindow")?;
        Ok(if client.address.is_empty() {
            None
        } else {
            Some(client)
        })
    }

    /// All connected monitors.
    pub fn monitors(&self) -> io::Result<Vec<Monitor>> {
        self.json("monitors")
    }

    /// Runs an action in the compositor.
    ///
    /// Hyprland 0.55 changed how actions are written: with a Lua config
    /// they are Lua expressions, before that they were plain words. The
    /// new form is tried first and the old form second, and whichever
    /// the compositor accepts is remembered for next time.
    pub fn run(&self, action: Dispatch<'_>) -> io::Result<()> {
        let order = match self.dialect.load(Ordering::Relaxed) {
            LEGACY => [Dialect::Legacy, Dialect::Lua],
            _ => [Dialect::Lua, Dialect::Legacy],
        };
        let mut last_reply = String::new();
        for dialect in order {
            let reply = self.request(&format!("dispatch {}", action.render(dialect)))?;
            if reply.trim() == "ok" {
                self.dialect.store(dialect as u8, Ordering::Relaxed);
                return Ok(());
            }
            last_reply = reply.trim().to_string();
        }
        Err(io::Error::other(format!(
            "Hyprland refused the request: {last_reply}"
        )))
    }

    /// Opens the event stream. Iterating blocks until the next event.
    pub fn events(&self) -> io::Result<EventStream> {
        let stream = UnixStream::connect(&self.event_socket)?;
        Ok(EventStream {
            reader: BufReader::new(stream),
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_the_lua_dialect() {
        let lua = |d: Dispatch| d.render(Dialect::Lua);
        assert_eq!(
            lua(Dispatch::Workspace(3)),
            r#"hl.dsp.focus({ workspace = "3" })"#
        );
        assert_eq!(
            lua(Dispatch::FocusWindow("0x55d0")),
            r#"hl.dsp.focus({ window = "address:0x55d0" })"#
        );
        assert_eq!(
            lua(Dispatch::Exec("firefox")),
            r#"hl.dsp.exec_cmd("firefox")"#
        );
        assert_eq!(lua(Dispatch::Exit), "hl.dsp.exit()");
    }

    #[test]
    fn renders_the_legacy_dialect() {
        let old = |d: Dispatch| d.render(Dialect::Legacy);
        assert_eq!(old(Dispatch::Workspace(3)), "workspace 3");
        assert_eq!(
            old(Dispatch::FocusWindow("0x55d0")),
            "focuswindow address:0x55d0"
        );
        assert_eq!(old(Dispatch::Exec("kitty -e htop")), "exec kitty -e htop");
        assert_eq!(old(Dispatch::Exit), "exit");
    }

    #[test]
    fn commands_cannot_break_out_of_the_lua_string() {
        let nasty = r#"sh -c 'echo "hi"' \ ") os.exit() --"#;
        let rendered = Dispatch::Exec(nasty).render(Dialect::Lua);
        assert_eq!(
            rendered,
            r#"hl.dsp.exec_cmd("sh -c 'echo \"hi\"' \\ \") os.exit() --")"#
        );
        assert_eq!(lua_string("a\nb"), r#""a\nb""#);
    }
}
