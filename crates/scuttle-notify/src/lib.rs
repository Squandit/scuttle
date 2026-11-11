//! Notifications for Scuttle.
//!
//! [`model`] holds what a notification is and the rules for showing and
//! remembering them. [`server`] is the `org.freedesktop.Notifications`
//! D-Bus service that apps send notifications to.

pub mod model;
pub mod server;

pub use model::{ActionButton, Change, CloseReason, Notification, Store, Timeout, Urgency};
pub use server::{serve, serve_on, Handle, ServeError};
