//! Events from Hyprland's event socket. Each arrives as one line of the
//! form `name>>data`.

/// A compositor event Scuttle reacts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Focus moved to another workspace.
    Workspace {
        id: i32,
        name: String,
    },
    /// Focus moved to another monitor.
    FocusedMonitor {
        monitor: String,
        workspace: String,
    },
    /// A different window got focus. Both fields are empty when nothing
    /// is focused.
    ActiveWindow {
        class: String,
        title: String,
    },
    OpenWindow {
        address: String,
        workspace: String,
        class: String,
        title: String,
    },
    CloseWindow {
        address: String,
    },
    /// A window moved to another workspace.
    MoveWindow {
        address: String,
        workspace: String,
    },
    CreateWorkspace {
        id: i32,
        name: String,
    },
    DestroyWorkspace {
        id: i32,
        name: String,
    },
    /// A window's title changed.
    WindowTitle {
        address: String,
    },
    Fullscreen {
        on: bool,
    },
    MonitorAdded {
        name: String,
    },
    MonitorRemoved {
        name: String,
    },
    /// The compositor reloaded its config.
    ConfigReloaded,
    /// Anything this crate does not model.
    Other {
        name: String,
        data: String,
    },
}

impl Event {
    /// True when the event can change what the workspace list shows.
    pub fn affects_workspaces(&self) -> bool {
        matches!(
            self,
            Event::Workspace { .. }
                | Event::FocusedMonitor { .. }
                | Event::OpenWindow { .. }
                | Event::CloseWindow { .. }
                | Event::MoveWindow { .. }
                | Event::CreateWorkspace { .. }
                | Event::DestroyWorkspace { .. }
                | Event::MonitorAdded { .. }
                | Event::MonitorRemoved { .. }
        )
    }

    /// True when the event can change the focused window's title.
    pub fn affects_title(&self) -> bool {
        matches!(
            self,
            Event::ActiveWindow { .. }
                | Event::WindowTitle { .. }
                | Event::CloseWindow { .. }
                | Event::Workspace { .. }
                | Event::FocusedMonitor { .. }
        )
    }
}

/// Parses one line from the event socket. Returns `None` for blank or
/// malformed lines.
pub fn parse_event(line: &str) -> Option<Event> {
    let line = line.trim_end_matches(['\n', '\r']);
    let (name, data) = line.split_once(">>")?;
    if name.is_empty() {
        return None;
    }
    // Titles can contain commas, so only split off the leading fields.
    let fields = |n: usize| -> Vec<String> { data.splitn(n, ',').map(str::to_string).collect() };
    let hex = |s: &str| format!("0x{}", s.trim_start_matches("0x"));

    let event = match name {
        // The v2 variants carry the numeric id; prefer them and ignore
        // the v1 duplicates Hyprland sends alongside.
        "workspacev2" => {
            let f = fields(2);
            Event::Workspace {
                id: f.first()?.parse().ok()?,
                name: f.get(1)?.clone(),
            }
        }
        "createworkspacev2" => {
            let f = fields(2);
            Event::CreateWorkspace {
                id: f.first()?.parse().ok()?,
                name: f.get(1)?.clone(),
            }
        }
        "destroyworkspacev2" => {
            let f = fields(2);
            Event::DestroyWorkspace {
                id: f.first()?.parse().ok()?,
                name: f.get(1)?.clone(),
            }
        }
        "workspace" | "createworkspace" | "destroyworkspace" | "activewindowv2"
        | "focusedmonv2" | "movewindowv2" | "monitoraddedv2" | "monitorremovedv2" => return None,
        "focusedmon" => {
            let f = fields(2);
            Event::FocusedMonitor {
                monitor: f.first()?.clone(),
                workspace: f.get(1)?.clone(),
            }
        }
        "activewindow" => {
            let f = fields(2);
            Event::ActiveWindow {
                class: f.first().cloned().unwrap_or_default(),
                title: f.get(1).cloned().unwrap_or_default(),
            }
        }
        "openwindow" => {
            let f = fields(4);
            Event::OpenWindow {
                address: hex(f.first()?),
                workspace: f.get(1)?.clone(),
                class: f.get(2)?.clone(),
                title: f.get(3).cloned().unwrap_or_default(),
            }
        }
        "closewindow" => Event::CloseWindow { address: hex(data) },
        "movewindow" => {
            let f = fields(2);
            Event::MoveWindow {
                address: hex(f.first()?),
                workspace: f.get(1)?.clone(),
            }
        }
        "windowtitle" => Event::WindowTitle { address: hex(data) },
        "windowtitlev2" => return None,
        "fullscreen" => Event::Fullscreen {
            on: data.trim() == "1",
        },
        "monitoradded" => Event::MonitorAdded {
            name: data.to_string(),
        },
        "monitorremoved" => Event::MonitorRemoved {
            name: data.to_string(),
        },
        "configreloaded" => Event::ConfigReloaded,
        _ => Event::Other {
            name: name.to_string(),
            data: data.to_string(),
        },
    };
    Some(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_change_uses_the_v2_event() {
        assert_eq!(
            parse_event("workspacev2>>3,web\n"),
            Some(Event::Workspace {
                id: 3,
                name: "web".into()
            })
        );
        assert_eq!(parse_event("workspace>>web\n"), None);
    }

    #[test]
    fn window_titles_keep_their_commas() {
        assert_eq!(
            parse_event("activewindow>>firefox,Hello, world, again"),
            Some(Event::ActiveWindow {
                class: "firefox".into(),
                title: "Hello, world, again".into()
            })
        );
        assert_eq!(
            parse_event("openwindow>>55d0c0a0,2,kitty,a,b"),
            Some(Event::OpenWindow {
                address: "0x55d0c0a0".into(),
                workspace: "2".into(),
                class: "kitty".into(),
                title: "a,b".into()
            })
        );
    }

    #[test]
    fn no_focus_is_two_empty_strings() {
        assert_eq!(
            parse_event("activewindow>>,"),
            Some(Event::ActiveWindow {
                class: String::new(),
                title: String::new()
            })
        );
    }

    #[test]
    fn unknown_events_are_kept_not_dropped() {
        assert_eq!(
            parse_event("submap>>resize"),
            Some(Event::Other {
                name: "submap".into(),
                data: "resize".into()
            })
        );
    }

    #[test]
    fn junk_is_ignored() {
        assert_eq!(parse_event(""), None);
        assert_eq!(parse_event("no separator"), None);
        assert_eq!(parse_event(">>data"), None);
        assert_eq!(parse_event("workspacev2>>notanumber,x"), None);
    }

    #[test]
    fn relevance_helpers() {
        let ws = parse_event("workspacev2>>1,1").unwrap();
        assert!(ws.affects_workspaces() && ws.affects_title());
        let title = parse_event("windowtitle>>abc").unwrap();
        assert!(!title.affects_workspaces() && title.affects_title());
        let other = parse_event("submap>>x").unwrap();
        assert!(!other.affects_workspaces() && !other.affects_title());
    }
}
