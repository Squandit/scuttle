//! Reads `.desktop` application entries, following the parts of the
//! freedesktop Desktop Entry spec that decide whether and how an app
//! appears in a launcher.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// An installed application.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DesktopEntry {
    /// The desktop file id, e.g. `firefox` for `firefox.desktop`.
    pub id: String,
    pub name: String,
    /// What kind of program it is, e.g. "Web Browser".
    pub generic_name: String,
    pub comment: String,
    /// The command line, still containing field codes such as `%U`.
    pub exec: String,
    pub icon: Option<String>,
    /// The app needs to run inside a terminal emulator.
    pub terminal: bool,
    pub keywords: Vec<String>,
}

/// Parses the text of a `.desktop` file. Returns `None` when the entry
/// is not an application or asks to be hidden.
///
/// `desktops` is the current desktop's names (from
/// `XDG_CURRENT_DESKTOP`), used for `OnlyShowIn` and `NotShowIn`.
pub fn parse_entry(id: &str, text: &str, desktops: &[String]) -> Option<DesktopEntry> {
    let mut entry = DesktopEntry { id: id.to_string(), ..Default::default() };
    let mut in_main_group = false;
    let mut entry_type = String::new();
    let mut hidden = false;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            // Only the main group matters. Action groups follow it.
            if in_main_group {
                break;
            }
            in_main_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_main_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), unescape(value.trim()));
        match key {
            "Type" => entry_type = value,
            "Name" => entry.name = value,
            "GenericName" => entry.generic_name = value,
            "Comment" => entry.comment = value,
            "Exec" => entry.exec = value,
            "Icon" if !value.is_empty() => entry.icon = Some(value),
            "Terminal" => entry.terminal = value == "true",
            "NoDisplay" | "Hidden" if value == "true" => hidden = true,
            "Keywords" => entry.keywords = list(&value),
            "OnlyShowIn" => {
                let only = list(&value);
                if !only.iter().any(|d| desktops.contains(d)) {
                    hidden = true;
                }
            }
            "NotShowIn" => {
                if list(&value).iter().any(|d| desktops.contains(d)) {
                    hidden = true;
                }
            }
            _ => {}
        }
    }

    if hidden || entry_type != "Application" || entry.name.is_empty() || entry.exec.is_empty() {
        return None;
    }
    Some(entry)
}

fn list(value: &str) -> Vec<String> {
    value.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// Undoes the escapes the spec defines for string values.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Turns an `Exec` value into a command line that can be run with no
/// files or URLs: field codes are dropped and `%%` becomes `%`.
pub fn exec_command(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            // Any other field code expands to nothing.
            Some(_) | None => {}
        }
    }
    // Dropping a code can leave doubled or trailing spaces behind.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The desktop names from `XDG_CURRENT_DESKTOP`.
pub fn current_desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Reads every application entry in `dirs`. When the same id appears in
/// more than one directory the earlier directory wins, including when
/// the earlier file hides the app.
pub fn scan(dirs: &[PathBuf], desktops: &[String]) -> Vec<DesktopEntry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        collect(dir, dir, 0, &mut files);
        files.sort();
        for (id, path) in files {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else { continue };
            if let Some(entry) = parse_entry(&id, &text, desktops) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    entries
}

/// Finds `.desktop` files under `dir`. Per the spec, files in
/// subdirectories get ids with `-` in place of the path separator.
fn collect(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
    if depth > 3 {
        return;
    }
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, depth + 1, out);
        } else if path.extension().is_some_and(|e| e == "desktop") {
            let Ok(relative) = path.strip_prefix(root) else { continue };
            let id = relative.with_extension("").to_string_lossy().replace('/', "-");
            out.push((id, path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "[Desktop Entry]\nVersion=1.0\nName=Firefox\nGenericName=Web Browser\n\
        Comment=Browse the World Wide Web\nExec=/usr/lib/firefox/firefox %u\nIcon=firefox\n\
        Terminal=false\nType=Application\nKeywords=Internet;WWW;Browser;Web;\n\n\
        [Desktop Action new-window]\nName=New Window\nExec=/usr/lib/firefox/firefox --new-window %u\n";

    fn parse(text: &str) -> Option<DesktopEntry> {
        parse_entry("test", text, &["Hyprland".to_string()])
    }

    #[test]
    fn parses_a_typical_entry() {
        let e = parse(FIREFOX).unwrap();
        assert_eq!(e.name, "Firefox");
        assert_eq!(e.generic_name, "Web Browser");
        assert_eq!(e.exec, "/usr/lib/firefox/firefox %u");
        assert_eq!(e.icon.as_deref(), Some("firefox"));
        assert!(!e.terminal);
        assert_eq!(e.keywords, vec!["Internet", "WWW", "Browser", "Web"]);
    }

    #[test]
    fn action_groups_do_not_override_the_main_entry() {
        let e = parse(FIREFOX).unwrap();
        assert_eq!(e.name, "Firefox");
        assert!(!e.exec.contains("--new-window"));
    }

    #[test]
    fn hidden_and_non_applications_are_skipped() {
        let base = "[Desktop Entry]\nType=Application\nName=X\nExec=x\n";
        assert!(parse(base).is_some());
        assert!(parse(&format!("{base}NoDisplay=true\n")).is_none());
        assert!(parse(&format!("{base}Hidden=true\n")).is_none());
        assert!(parse("[Desktop Entry]\nType=Link\nName=X\nURL=https://x\n").is_none());
        assert!(parse("[Desktop Entry]\nType=Application\nName=X\n").is_none());
    }

    #[test]
    fn respects_only_show_in_and_not_show_in() {
        let base = "[Desktop Entry]\nType=Application\nName=X\nExec=x\n";
        assert!(parse(&format!("{base}OnlyShowIn=GNOME;KDE;\n")).is_none());
        assert!(parse(&format!("{base}OnlyShowIn=GNOME;Hyprland;\n")).is_some());
        assert!(parse(&format!("{base}NotShowIn=Hyprland;\n")).is_none());
        assert!(parse(&format!("{base}NotShowIn=GNOME;\n")).is_some());
    }

    #[test]
    fn localised_names_do_not_replace_the_default() {
        let text = "[Desktop Entry]\nType=Application\nName=Files\nName[de]=Dateien\nExec=nautilus\n";
        assert_eq!(parse(text).unwrap().name, "Files");
    }

    #[test]
    fn strips_field_codes_from_exec() {
        assert_eq!(exec_command("firefox %u"), "firefox");
        assert_eq!(exec_command("code --new-window %F"), "code --new-window");
        assert_eq!(exec_command("env A=1 app %i --flag %c"), "env A=1 app --flag");
        assert_eq!(exec_command("printf 100%%"), "printf 100%");
        assert_eq!(exec_command("app %"), "app");
    }

    #[test]
    fn unescapes_values() {
        assert_eq!(unescape(r"a\sb\\c"), r"a b\c");
        assert_eq!(unescape(r"tab\there"), "tab\there");
    }

    #[test]
    fn scan_prefers_earlier_directories_and_sorts_by_name() {
        let root = std::env::temp_dir().join(format!("scuttle-desktop-{}", std::process::id()));
        let user = root.join("user");
        let system = root.join("system");
        fs::create_dir_all(user.join("sub")).unwrap();
        fs::create_dir_all(&system).unwrap();
        let app = |name: &str| format!("[Desktop Entry]\nType=Application\nName={name}\nExec=x\n");
        fs::write(system.join("zed.desktop"), app("Zed")).unwrap();
        fs::write(system.join("alpha.desktop"), app("alpha")).unwrap();
        fs::write(system.join("shadowed.desktop"), app("System Version")).unwrap();
        fs::write(user.join("shadowed.desktop"), app("User Version")).unwrap();
        fs::write(system.join("gone.desktop"), app("Gone")).unwrap();
        fs::write(user.join("gone.desktop"), format!("{}Hidden=true\n", app("Gone"))).unwrap();
        fs::write(user.join("sub").join("nested.desktop"), app("Nested")).unwrap();
        fs::write(user.join("notes.txt"), "not a desktop file").unwrap();

        let entries = scan(&[user, system, root.join("missing")], &[]);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Nested", "User Version", "Zed"]);
        assert!(entries.iter().any(|e| e.id == "sub-nested"));
        fs::remove_dir_all(&root).unwrap();
    }
}
