//! Where Scuttle keeps its files. Everything follows the XDG base
//! directory spec, so nothing here is specific to one user or machine.

use std::env;
use std::path::PathBuf;

fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

/// `~/.config/scuttle`
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("scuttle")
}

/// `~/.config/scuttle/config.toml`
pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// `~/.local/share/scuttle`, for data worth keeping (clipboard pins,
/// launcher history).
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("scuttle")
}

/// `~/.cache/scuttle`, for data that can be rebuilt.
pub fn cache_dir() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("scuttle")
}

/// The per-session runtime directory. Falls back to a per-user folder in
/// the temp directory when `XDG_RUNTIME_DIR` is not set.
pub fn runtime_dir() -> PathBuf {
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            let user = env::var("USER").unwrap_or_else(|_| "user".into());
            env::temp_dir().join(format!("scuttle-{user}"))
        }
    }
}

/// The control socket the `scuttle` command uses to talk to the running
/// shell. One per Wayland session, so nested sessions do not collide.
pub fn socket_path() -> PathBuf {
    let display = env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "default".into());
    let display = display.replace('/', "_");
    runtime_dir().join(format!("scuttle-{display}.sock"))
}

/// Directories that hold `.desktop` application entries, most specific
/// first.
pub fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![xdg("XDG_DATA_HOME", ".local/share").join("applications")];
    let system = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for dir in system.split(':').filter(|s| !s.is_empty()) {
        dirs.push(PathBuf::from(dir).join("applications"));
    }
    // Flatpak exports are not always on XDG_DATA_DIRS.
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    dirs.push(home().join(".local/share/flatpak/exports/share/applications"));
    dirs.dedup();
    dirs
}
