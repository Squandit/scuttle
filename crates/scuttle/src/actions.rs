//! Carries out what a launcher result asks for: starting programs,
//! opening files and links, focusing windows.

use std::env;
use std::path::Path;
use std::process::{Command, Stdio};

use scuttle_hypr::{Dispatch, Hyprland};

/// Terminal emulators to try, in order, when the config names none.
const TERMINALS: &[&str] = &[
    "kitty",
    "foot",
    "ghostty",
    "alacritty",
    "wezterm",
    "konsole",
    "gnome-terminal",
    "xterm",
];

/// Quotes a string so a POSIX shell reads it back as one word.
pub fn shell_quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c))
    {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Whether `program` is an executable somewhere on `PATH`.
pub fn on_path(program: &str) -> bool {
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// Picks the terminal emulator: the configured one, else `$TERMINAL`,
/// else the first known one that is installed.
pub fn find_terminal(configured: &str) -> Option<String> {
    if !configured.trim().is_empty() {
        return Some(configured.trim().to_string());
    }
    if let Ok(term) = env::var("TERMINAL") {
        if !term.trim().is_empty() {
            return Some(term.trim().to_string());
        }
    }
    TERMINALS.iter().find(|t| on_path(t)).map(|t| t.to_string())
}

/// The command line that runs `command` inside `terminal`.
pub fn in_terminal(terminal: &str, command: &str) -> String {
    format!("{terminal} -e sh -c {}", shell_quote(command))
}

/// Starts a shell command that outlives Scuttle and is not its child.
///
/// Under Hyprland the compositor starts it, which also puts the new
/// window on the workspace that was focused when it was launched.
pub fn spawn_detached(command: &str, hypr: Option<&Hyprland>) -> Result<(), String> {
    if let Some(hypr) = hypr {
        if hypr.run(Dispatch::Exec(command)).is_ok() {
            return Ok(());
        }
    }
    let quiet = |cmd: &mut Command| {
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
    };
    // `setsid -f` forks and returns at once, so waiting on it is
    // instant and leaves no zombie behind.
    let mut setsid = Command::new("setsid");
    setsid.args(["-f", "sh", "-c", command]);
    quiet(&mut setsid);
    if let Ok(status) = setsid.status() {
        if status.success() {
            return Ok(());
        }
    }
    let mut sh = Command::new("sh");
    sh.args(["-c", &format!("({command}) >/dev/null 2>&1 &")]);
    quiet(&mut sh);
    match sh.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("Could not start \"{command}\" ({status}).")),
        Err(e) => Err(format!("Could not start \"{command}\": {e}")),
    }
}

/// Opens a file, folder or URL with the default application.
pub fn open(target: &str, hypr: Option<&Hyprland>) -> Result<(), String> {
    if !on_path("xdg-open") {
        return Err(
            "xdg-open is not installed, so nothing can be opened. Install xdg-utils.".into(),
        );
    }
    spawn_detached(&format!("xdg-open {}", shell_quote(target)), hypr)
}

/// Opens a path with the default application.
pub fn open_path(path: &Path, hypr: Option<&Hyprland>) -> Result<(), String> {
    open(&path.to_string_lossy(), hypr)
}

/// Ends the desktop session.
pub fn logout(hypr: Option<&Hyprland>) -> Result<(), String> {
    if let Some(hypr) = hypr {
        return hypr.run(Dispatch::Exit).map_err(|e| e.to_string());
    }
    match env::var("XDG_SESSION_ID") {
        Ok(id) if !id.is_empty() => spawn_detached(
            &format!("loginctl terminate-session {}", shell_quote(&id)),
            None,
        ),
        _ => Err("Could not work out which session to end.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_are_left_alone() {
        assert_eq!(shell_quote("firefox"), "firefox");
        assert_eq!(shell_quote("/usr/bin/app-1.2"), "/usr/bin/app-1.2");
        assert_eq!(shell_quote("https://a.b/c=d"), "https://a.b/c=d");
    }

    #[test]
    fn anything_else_is_single_quoted() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("$(rm -rf ~)"), "'$(rm -rf ~)'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("a;b"), "'a;b'");
        assert_eq!(shell_quote("a?b&c"), "'a?b&c'");
    }

    #[test]
    fn quoting_survives_a_real_shell() {
        for nasty in [
            "a b",
            "it's \"here\"",
            "$HOME `id` $(id)",
            "semi;colon | pipe > out",
            "new\nline",
        ] {
            let out = Command::new("sh")
                .args(["-c", &format!("printf %s {}", shell_quote(nasty))])
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), nasty);
        }
    }

    #[test]
    fn terminal_wrapping_keeps_the_command_as_one_argument() {
        assert_eq!(in_terminal("kitty", "htop"), "kitty -e sh -c htop");
        assert_eq!(
            in_terminal("foot", "nvim 'my file'"),
            r"foot -e sh -c 'nvim '\''my file'\'''"
        );
    }

    #[test]
    fn configured_terminal_wins() {
        assert_eq!(find_terminal(" wezterm "), Some("wezterm".into()));
    }

    #[test]
    fn sh_is_on_path_and_nonsense_is_not() {
        assert!(on_path("sh"));
        assert!(!on_path("definitely-not-a-real-program-xyz"));
    }
}
