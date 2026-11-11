//! A stand-in Hyprland that speaks the same socket protocol.
//!
//! It keeps a tiny model of workspaces and windows, answers the JSON
//! queries Scuttle makes, and emits events when a `dispatch workspace`
//! arrives. Tests and the headless demo script run against it, so the
//! shell can be exercised on machines without Hyprland.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::types::{Client, Workspace, WorkspaceRef};

/// Which ways of writing actions a mock accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialects {
    Both,
    /// Behaves like Hyprland 0.55+ with a Lua config.
    LuaOnly,
    /// Behaves like older Hyprland.
    LegacyOnly,
}

#[derive(Debug)]
struct State {
    dialects: Dialects,
    active: i32,
    clients: Vec<Client>,
    subscribers: Vec<UnixStream>,
}

impl State {
    fn workspaces(&self) -> Vec<Workspace> {
        let mut ids: Vec<i32> = self.clients.iter().map(|c| c.workspace.id).collect();
        ids.push(self.active);
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .map(|id| Workspace {
                id,
                name: id.to_string(),
                monitor: "MOCK-1".into(),
                windows: self.clients.iter().filter(|c| c.workspace.id == id).count() as u32,
                has_fullscreen: false,
                last_window_title: self
                    .clients
                    .iter()
                    .find(|c| c.workspace.id == id)
                    .map(|c| c.title.clone())
                    .unwrap_or_default(),
            })
            .collect()
    }

    fn active_window(&self) -> Option<&Client> {
        self.clients
            .iter()
            .filter(|c| c.workspace.id == self.active)
            .min_by_key(|c| c.focus_history_id)
    }

    fn emit(&mut self, line: &str) {
        let payload = format!("{line}\n");
        self.subscribers
            .retain_mut(|s| s.write_all(payload.as_bytes()).is_ok());
    }

    fn emit_focus(&mut self) {
        let (class, title) = self
            .active_window()
            .map(|c| (c.class.clone(), c.title.clone()))
            .unwrap_or_default();
        self.emit(&format!("activewindow>>{class},{title}"));
    }
}

/// A running mock compositor. Dropping it removes its sockets.
pub struct MockHyprland {
    dir: PathBuf,
    state: Arc<Mutex<State>>,
}

fn window(address: &str, workspace: i32, class: &str, title: &str, order: i32) -> Client {
    Client {
        address: address.into(),
        mapped: true,
        hidden: false,
        workspace: WorkspaceRef {
            id: workspace,
            name: workspace.to_string(),
        },
        floating: false,
        monitor: 0,
        class: class.into(),
        title: title.into(),
        pid: 1000 + order,
        focus_history_id: order,
    }
}

impl MockHyprland {
    /// Starts a mock with a small, believable desktop: a terminal and a
    /// browser on workspace 1, a browser on 2, a file manager on 3.
    pub fn start(dir: &Path) -> io::Result<MockHyprland> {
        MockHyprland::start_with(dir, Dialects::Both)
    }

    /// Like [`MockHyprland::start`], but accepting only the given
    /// dialects, to imitate a specific Hyprland generation.
    pub fn start_with(dir: &Path, dialects: Dialects) -> io::Result<MockHyprland> {
        fs::create_dir_all(dir)?;
        let command_path = dir.join(".socket.sock");
        let event_path = dir.join(".socket2.sock");
        let _ = fs::remove_file(&command_path);
        let _ = fs::remove_file(&event_path);
        let commands = UnixListener::bind(&command_path)?;
        let events = UnixListener::bind(&event_path)?;

        let state = Arc::new(Mutex::new(State {
            dialects,
            active: 1,
            clients: vec![
                window("0xa1", 1, "kitty", "~/projects/scuttle", 0),
                window("0xa2", 1, "firefox", "Hyprland Wiki", 1),
                window("0xb1", 2, "firefox", "Arch package search", 2),
                window("0xc1", 3, "org.gnome.Nautilus", "Documents", 3),
            ],
            subscribers: Vec::new(),
        }));

        let for_events = Arc::clone(&state);
        thread::spawn(move || {
            for stream in events.incoming().flatten() {
                for_events.lock().unwrap().subscribers.push(stream);
            }
        });

        let for_commands = Arc::clone(&state);
        thread::spawn(move || {
            for mut stream in commands.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let reply = handle(&for_commands, request.trim());
                let _ = stream.write_all(reply.as_bytes());
            }
        });

        Ok(MockHyprland {
            dir: dir.to_path_buf(),
            state,
        })
    }

    /// The directory holding the sockets, for [`crate::Hyprland::at`].
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The workspace the mock currently considers focused.
    pub fn active_workspace(&self) -> i32 {
        self.state.lock().unwrap().active
    }

    /// Number of clients connected to the event socket.
    pub fn subscriber_count(&self) -> usize {
        self.state.lock().unwrap().subscribers.len()
    }
}

impl Drop for MockHyprland {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.dir.join(".socket.sock"));
        let _ = fs::remove_file(self.dir.join(".socket2.sock"));
    }
}

fn handle(state: &Arc<Mutex<State>>, request: &str) -> String {
    let mut state = state.lock().unwrap();
    let json = |value: &dyn erased::Json| value.to_json();
    match request {
        "j/workspaces" => json(&state.workspaces()),
        "j/activeworkspace" => {
            let active = state.active;
            let ws = state
                .workspaces()
                .into_iter()
                .find(|w| w.id == active)
                .unwrap_or_default();
            json(&ws)
        }
        "j/clients" => json(&state.clients),
        "j/activewindow" => match state.active_window() {
            Some(client) => json(client),
            None => "{}".into(),
        },
        "j/monitors" => {
            let active = state.active;
            format!(
                r#"[{{"id":0,"name":"MOCK-1","description":"Mock monitor","width":1440,"height":900,"scale":1.0,"focused":true,"activeWorkspace":{{"id":{active},"name":"{active}"}}}}]"#
            )
        }
        other => match other.strip_prefix("dispatch ") {
            Some(action) => dispatch(&mut state, action.trim()),
            None => "unknown request".into(),
        },
    }
}

/// Pulls the quoted value out of a Lua call such as
/// `hl.dsp.focus({ workspace = "3" })` given the text before the quote.
fn lua_value<'a>(action: &'a str, before: &str) -> Option<&'a str> {
    let rest = action.strip_prefix(before)?;
    let end = rest.rfind('"')?;
    Some(&rest[..end])
}

/// Understands the handful of actions Scuttle sends, in whichever
/// dialects this mock was started with.
fn dispatch(state: &mut State, action: &str) -> String {
    enum Parsed {
        Workspace(String),
        FocusWindow(String),
        Other,
    }
    let lua = || -> Option<Parsed> {
        if let Some(ws) = lua_value(action, "hl.dsp.focus({ workspace = \"") {
            return Some(Parsed::Workspace(ws.to_string()));
        }
        if let Some(window) = lua_value(action, "hl.dsp.focus({ window = \"address:") {
            return Some(Parsed::FocusWindow(window.to_string()));
        }
        (action.starts_with("hl.dsp.exec_cmd(\"") || action == "hl.dsp.exit()")
            .then_some(Parsed::Other)
    };
    let legacy = || -> Option<Parsed> {
        if let Some(ws) = action.strip_prefix("workspace ") {
            return Some(Parsed::Workspace(ws.trim().to_string()));
        }
        if let Some(window) = action.strip_prefix("focuswindow address:") {
            return Some(Parsed::FocusWindow(window.trim().to_string()));
        }
        (action.starts_with("exec ") || action == "exit").then_some(Parsed::Other)
    };
    let parsed = match state.dialects {
        Dialects::Both => lua().or_else(legacy),
        Dialects::LuaOnly => lua(),
        Dialects::LegacyOnly => legacy(),
    };
    match parsed {
        None => "Invalid dispatcher".into(),
        Some(Parsed::Other) => "ok".into(),
        Some(Parsed::Workspace(target)) => match target.parse::<i32>() {
            Ok(id) => {
                state.active = id;
                state.emit(&format!("workspace>>{id}"));
                state.emit(&format!("workspacev2>>{id},{id}"));
                state.emit_focus();
                "ok".into()
            }
            Err(_) => "Invalid workspace".into(),
        },
        Some(Parsed::FocusWindow(address)) => {
            match state.clients.iter().position(|c| c.address == address) {
                Some(index) => {
                    for c in state.clients.iter_mut() {
                        c.focus_history_id += 1;
                    }
                    state.clients[index].focus_history_id = 0;
                    let id = state.clients[index].workspace.id;
                    if id != state.active {
                        state.active = id;
                        state.emit(&format!("workspacev2>>{id},{id}"));
                    }
                    state.emit_focus();
                    "ok".into()
                }
                None => "Window not found".into(),
            }
        }
    }
}

/// Lets `handle` serialise different reply types through one closure.
mod erased {
    pub trait Json {
        fn to_json(&self) -> String;
    }

    impl<T: serde::Serialize> Json for T {
        fn to_json(&self) -> String {
            serde_json::to_string(self).expect("mock replies always serialise")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dispatch, Event, Hyprland};
    use std::time::{Duration, Instant};

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("scuttle-hypr-{name}-{}", std::process::id()))
    }

    fn wait_for_subscriber(mock: &MockHyprland) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while mock.subscriber_count() == 0 {
            assert!(
                Instant::now() < deadline,
                "event subscriber never connected"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn client_reads_the_mock_desktop() {
        let mock = MockHyprland::start(&scratch("read")).unwrap();
        let hypr = Hyprland::at(mock.dir());

        let workspaces = hypr.workspaces().unwrap();
        assert_eq!(
            workspaces.iter().map(|w| w.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(workspaces[0].windows, 2);

        assert_eq!(hypr.active_workspace().unwrap().id, 1);
        assert_eq!(hypr.clients().unwrap().len(), 4);
        assert_eq!(hypr.active_window().unwrap().unwrap().class, "kitty");
        assert_eq!(hypr.monitors().unwrap()[0].name, "MOCK-1");
    }

    #[test]
    fn dispatch_changes_workspace_and_emits_events() {
        let mock = MockHyprland::start(&scratch("dispatch")).unwrap();
        let hypr = Hyprland::at(mock.dir());
        let mut events = hypr.events().unwrap();
        wait_for_subscriber(&mock);

        hypr.run(Dispatch::Workspace(3)).unwrap();
        assert_eq!(mock.active_workspace(), 3);
        assert_eq!(
            events.next(),
            Some(Event::Workspace {
                id: 3,
                name: "3".into()
            })
        );
        assert_eq!(
            events.next(),
            Some(Event::ActiveWindow {
                class: "org.gnome.Nautilus".into(),
                title: "Documents".into()
            })
        );
        assert_eq!(hypr.active_window().unwrap().unwrap().title, "Documents");
    }

    #[test]
    fn empty_workspace_has_no_active_window() {
        let mock = MockHyprland::start(&scratch("empty")).unwrap();
        let hypr = Hyprland::at(mock.dir());
        hypr.run(Dispatch::Workspace(4)).unwrap();
        assert_eq!(hypr.active_window().unwrap(), None);
        // An empty but focused workspace still shows up in the list.
        assert!(hypr
            .workspaces()
            .unwrap()
            .iter()
            .any(|w| w.id == 4 && w.windows == 0));
    }

    #[test]
    fn focusing_a_window_switches_to_its_workspace() {
        let mock = MockHyprland::start(&scratch("focus")).unwrap();
        let hypr = Hyprland::at(mock.dir());
        hypr.run(Dispatch::FocusWindow("0xb1")).unwrap();
        assert_eq!(mock.active_workspace(), 2);
        assert_eq!(hypr.active_window().unwrap().unwrap().address, "0xb1");
    }

    #[test]
    fn unknown_window_is_an_error() {
        let mock = MockHyprland::start(&scratch("bad")).unwrap();
        let hypr = Hyprland::at(mock.dir());
        assert!(hypr.run(Dispatch::FocusWindow("0xdead")).is_err());
    }

    #[test]
    fn works_against_new_hyprland_that_only_takes_lua() {
        let mock = MockHyprland::start_with(&scratch("lua"), Dialects::LuaOnly).unwrap();
        let hypr = Hyprland::at(mock.dir());
        hypr.run(Dispatch::Workspace(2)).unwrap();
        assert_eq!(mock.active_workspace(), 2);
        hypr.run(Dispatch::Exec("kitty")).unwrap();
        hypr.run(Dispatch::FocusWindow("0xc1")).unwrap();
        assert_eq!(mock.active_workspace(), 3);
    }

    #[test]
    fn falls_back_for_old_hyprland_and_remembers_it() {
        let mock = MockHyprland::start_with(&scratch("legacy"), Dialects::LegacyOnly).unwrap();
        let hypr = Hyprland::at(mock.dir());
        hypr.run(Dispatch::Workspace(2)).unwrap();
        assert_eq!(mock.active_workspace(), 2);
        assert_eq!(
            hypr.dialect.load(std::sync::atomic::Ordering::Relaxed),
            crate::Dialect::Legacy as u8
        );
        // Clones share what was learned.
        let clone = hypr.clone();
        clone.run(Dispatch::Workspace(3)).unwrap();
        assert_eq!(mock.active_workspace(), 3);
    }

    #[test]
    fn missing_compositor_is_an_error_not_a_hang() {
        let hypr = Hyprland::at(Path::new("/nonexistent/scuttle-test"));
        assert!(hypr.workspaces().is_err());
        assert!(hypr.events().is_err());
    }
}
