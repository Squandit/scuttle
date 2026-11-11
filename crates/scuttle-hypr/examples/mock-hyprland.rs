//! Runs the mock compositor until interrupted. Used by the headless
//! demo script so Scuttle has workspaces and windows to show.
//!
//! Usage: mock-hyprland <socket-directory>

use std::path::PathBuf;

fn main() {
    let dir = match std::env::args().nth(1) {
        Some(dir) => PathBuf::from(dir),
        None => {
            eprintln!("Usage: mock-hyprland <socket-directory>");
            std::process::exit(2);
        }
    };
    let _mock = match scuttle_hypr::mock::MockHyprland::start(&dir) {
        Ok(mock) => mock,
        Err(e) => {
            eprintln!(
                "Could not start the mock compositor in {}: {e}",
                dir.display()
            );
            std::process::exit(1);
        }
    };
    println!("Mock Hyprland listening in {}", dir.display());
    loop {
        std::thread::park();
    }
}
