//! Now playing, for any media player that speaks MPRIS: the D-Bus
//! interface that music and video players on Linux expose so other
//! programs can show and control what is playing.

pub mod model;
pub mod mpris;

pub use model::{pick, Control, Track};
pub use mpris::{watch, watch_on, Controller};
