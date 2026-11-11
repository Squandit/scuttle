//! Messages sent from background threads to the UI thread.
//!
//! GTK widgets live on one thread. Everything that blocks (sockets,
//! disk scans, sleeping between samples) runs elsewhere and reports
//! back with one of these.

use std::sync::mpsc::SyncSender;

use scuttle_core::ipc::{Request, Response};
use scuttle_hypr::{Client, Workspace};
use scuttle_launcher::desktop::DesktopEntry;
use scuttle_launcher::files::FileEntry;
use scuttle_sys::battery::Battery;
use scuttle_sys::memory::Memory;
use scuttle_sys::network::Link;

/// What the compositor is showing right now.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DesktopState {
    pub workspaces: Vec<Workspace>,
    /// Id of the focused workspace.
    pub active_workspace: i32,
    /// Class of the focused window, or empty.
    pub active_class: String,
    /// Title of the focused window, or empty.
    pub active_title: String,
    pub clients: Vec<Client>,
    /// Name of the focused monitor, when known.
    pub focused_monitor: Option<String>,
}

/// A periodic reading of machine state.
#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    /// CPU load, 0 to 100.
    pub cpu: u8,
    pub memory: Option<Memory>,
    /// `None` on machines without a battery.
    pub battery: Option<Battery>,
    pub link: Link,
}

pub enum Msg {
    /// A command from the `scuttle` CLI, with where to send the answer.
    Ipc(Request, SyncSender<Response>),
    Desktop(DesktopState),
    Stats(Stats),
    Apps(Vec<DesktopEntry>),
    Files(Vec<FileEntry>),
    /// The clipboard history, or why it could not be read.
    Clipboard(Result<Vec<scuttle_launcher::clipboard::ClipEntry>, String>),
    /// The notification service reporting a change.
    Notification(scuttle_notify::Change),
    /// The notification badge on a bar was clicked.
    NotificationsSeen,
    /// The output volume as last read. `None` when there is no audio
    /// output or it cannot be read.
    Volume(Option<scuttle_sys::audio::Volume>),
    /// What the media player is doing. `None` when none is running.
    Media(Option<scuttle_media::Track>),
    /// The media widget on a bar was clicked or scrolled.
    MediaRequest(scuttle_media::Control),
    /// The volume widget on a bar was scrolled or clicked.
    VolumeRequest(scuttle_sys::audio::VolumeChange),
    /// Something for the assistant panel, with the chat it belongs to.
    Assistant(u64, crate::assistant::AssistantMsg),
}
