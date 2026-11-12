//! The system tray.
//!
//! Apps such as Discord, Steam and network applets put an icon in the
//! tray through the StatusNotifierItem protocol on D-Bus. This crate
//! is both halves a desktop needs for that: the watcher apps register
//! with, and the host that reads each icon and passes clicks back.
//!
//! Nothing here draws anything. The bar is handed a list of [`Item`]s
//! and calls back through a [`Tray`].

pub mod host;
pub mod menu;
pub mod model;

pub use host::{watch, watch_on, Tray};
pub use menu::MenuEntry;
pub use model::{Icon, Item, Pixels, Status};
