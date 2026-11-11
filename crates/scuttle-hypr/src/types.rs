//! The parts of Hyprland's JSON replies that Scuttle uses. Every field
//! has a default so a newer or older Hyprland that adds or drops a field
//! never breaks parsing.

use serde::{Deserialize, Serialize};

/// A workspace as listed by `workspaces` and `activeworkspace`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub id: i32,
    pub name: String,
    pub monitor: String,
    /// Number of windows on the workspace.
    pub windows: u32,
    #[serde(rename = "hasfullscreen")]
    pub has_fullscreen: bool,
    #[serde(rename = "lastwindowtitle")]
    pub last_window_title: String,
}

impl Workspace {
    /// Special workspaces (scratchpads) have negative ids.
    pub fn is_special(&self) -> bool {
        self.id < 0
    }
}

/// The short workspace reference embedded in clients and monitors.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceRef {
    pub id: i32,
    pub name: String,
}

/// A window.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Client {
    /// Hyprland's handle for the window, e.g. `0x55d0c0a0`.
    pub address: String,
    pub mapped: bool,
    pub hidden: bool,
    pub workspace: WorkspaceRef,
    pub floating: bool,
    pub monitor: i32,
    pub class: String,
    pub title: String,
    pub pid: i32,
    /// Lower means focused more recently. 0 is the focused window.
    #[serde(rename = "focusHistoryID")]
    pub focus_history_id: i32,
}

/// A monitor.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Monitor {
    pub id: i32,
    pub name: String,
    pub description: String,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub focused: bool,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: WorkspaceRef,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_workspace_reply() {
        let json = r#"[{"id":1,"name":"1","monitor":"eDP-1","monitorID":0,"windows":2,
            "hasfullscreen":false,"lastwindow":"0x55d0","lastwindowtitle":"zsh","ispersistent":false}]"#;
        let list: Vec<Workspace> = serde_json::from_str(json).unwrap();
        assert_eq!(list[0].id, 1);
        assert_eq!(list[0].windows, 2);
        assert_eq!(list[0].last_window_title, "zsh");
    }

    #[test]
    fn parses_a_real_client_reply() {
        let json = r#"{"address":"0x55d0c0a0","mapped":true,"hidden":false,"at":[12,60],
            "size":[900,700],"workspace":{"id":2,"name":"2"},"floating":false,"pseudo":false,
            "monitor":0,"class":"firefox","title":"Hyprland Wiki","initialClass":"firefox",
            "initialTitle":"Mozilla Firefox","pid":4242,"xwayland":false,"pinned":false,
            "fullscreen":0,"fullscreenClient":0,"grouped":[],"tags":[],"swallowing":"0x0",
            "focusHistoryID":1}"#;
        let c: Client = serde_json::from_str(json).unwrap();
        assert_eq!(c.class, "firefox");
        assert_eq!(c.workspace.id, 2);
        assert_eq!(c.focus_history_id, 1);
    }

    #[test]
    fn empty_object_means_no_window() {
        let c: Client = serde_json::from_str("{}").unwrap();
        assert!(c.address.is_empty());
    }

    #[test]
    fn special_workspaces_are_negative() {
        let w = Workspace { id: -99, ..Default::default() };
        assert!(w.is_special());
    }
}
