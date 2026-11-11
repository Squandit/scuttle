//! The assistant behind Scuttle's side panel.
//!
//! It talks to Claude two ways. The first choice is the `claude`
//! command, which runs on the user's Claude subscription. When that
//! reports its usage limit is reached, or is not installed, the
//! assistant falls back to the Claude API with an API key, and returns
//! to the subscription once the limit resets.
//!
//! Nothing here draws anything. The panel feeds it a message and
//! receives a stream of [`Event`]s.

pub mod actions;
pub mod api;
pub mod cli;
pub mod command;
pub mod conversation;
pub mod key;
pub mod markdown;
mod proc;
pub mod prompt;
pub mod router;

pub use actions::{segments, Action, Segment};
pub use cli::Cancel;
pub use conversation::{Conversation, Role, Turn};
pub use key::Keyring;
pub use router::{pretty_model, Assistant, Backend, Job, Prefer, Report, Settings, Unavailable};

/// What the assistant reports while answering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A backend has started on this message. Sent again if the
    /// second backend takes over from the first.
    Started(Backend),
    /// Something the user should know, such as why the backend changed.
    Notice(String),
    /// More of the answer.
    Delta(String),
}
