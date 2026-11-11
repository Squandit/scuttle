//! Shared building blocks for Scuttle: configuration, theme tokens, the
//! stylesheet generator, filesystem paths and the IPC protocol.
//!
//! Nothing in this crate touches GTK or Wayland, so all of it can be unit
//! tested on any machine.

pub mod color;
pub mod config;
pub mod css;
pub mod edit;
pub mod ipc;
pub mod motion;
pub mod paths;
pub mod schema;
pub mod theme;

pub use color::Rgba;
pub use config::Config;
pub use theme::Theme;
