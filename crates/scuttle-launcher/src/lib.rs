//! Everything behind the launcher's search box: finding applications,
//! windows, files, system commands, sums and web searches, and ranking
//! them against what was typed.
//!
//! The crate has no UI and performs no actions. It turns a query into a
//! list of [`Item`]s, each carrying an [`Action`] for the shell to run.

pub mod calc;
pub mod desktop;
pub mod engine;
pub mod files;
pub mod history;
pub mod item;
pub mod matching;

pub use engine::{Engine, Mode, WindowInfo};
pub use item::{Action, Internal, Item, Kind};
