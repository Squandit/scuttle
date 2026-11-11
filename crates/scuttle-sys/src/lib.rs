//! Reads machine state straight from `/sys` and `/proc`.
//!
//! Every reader takes the directory or text it parses as an argument,
//! so the logic is tested against fixtures and never depends on the
//! machine the tests run on.

pub mod battery;
pub mod cpu;
pub mod memory;
pub mod network;
