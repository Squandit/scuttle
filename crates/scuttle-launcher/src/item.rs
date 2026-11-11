//! What the launcher shows and what happens when a result is chosen.

use std::path::PathBuf;

/// The category of a result. Results are grouped by kind in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Calculator,
    App,
    Window,
    System,
    File,
    Web,
}

impl Kind {
    /// The heading shown above a group of results.
    pub fn section(self) -> &'static str {
        match self {
            Kind::Calculator => "Result",
            Kind::App => "Apps",
            Kind::Window => "Windows",
            Kind::System => "System",
            Kind::File => "Files",
            Kind::Web => "Web",
        }
    }
}

/// Something the shell itself does, as opposed to starting a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Internal {
    /// Re-read the config file.
    Reload,
    /// Stop Scuttle.
    Quit,
    /// End the desktop session.
    Logout,
    /// Turn "do not disturb" on or off.
    ToggleDnd,
}

/// What choosing a result does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Start an application. `exec` is a shell command with desktop
    /// entry field codes already removed.
    Launch {
        exec: String,
        terminal: bool,
    },
    /// Focus an open window by its compositor address.
    FocusWindow {
        address: String,
    },
    /// Open a file or folder with its default application.
    OpenPath(PathBuf),
    /// Open a URL in the default browser.
    OpenUrl(String),
    /// Put text on the clipboard.
    Copy(String),
    /// Run a shell command.
    Shell(String),
    Internal(Internal),
}

/// One row in the launcher.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: Kind,
    pub title: String,
    /// Secondary text shown on the right of the row.
    pub subtitle: String,
    /// A freedesktop icon name or an absolute path to an image.
    pub icon: Option<String>,
    pub action: Action,
    /// Destructive actions ask for a second Enter before running.
    pub confirm: bool,
    /// A stable identifier used to remember how often the item is used.
    pub key: String,
    /// Match quality. Higher is better. Only comparable within a query.
    pub score: u32,
}
