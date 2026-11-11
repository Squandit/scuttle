//! Where API keys live: the system keyring.
//!
//! Keys are stored through `secret-tool`, the command line front end to
//! the desktop's secret service (GNOME Keyring, KeePassXC, KWallet).
//! They are never written to Scuttle's config file, never logged and
//! never passed on a command line, where other programs could read
//! them from the process list.

use std::io::Write;
use std::process::{Command, Stdio};

/// The account name for the Claude API key.
pub const ANTHROPIC: &str = "anthropic";

/// The environment variable read when the keyring has no key.
pub const ANTHROPIC_ENV: &str = "ANTHROPIC_API_KEY";

/// Access to the keyring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keyring {
    /// The `secret-tool` program. Tests swap in a stand-in.
    pub program: String,
    /// Whether to fall back to the environment when the keyring has
    /// no key.
    pub read_env: bool,
}

impl Default for Keyring {
    fn default() -> Self {
        Keyring {
            program: "secret-tool".to_string(),
            read_env: true,
        }
    }
}

/// Why a key could not be stored or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// `secret-tool` is not installed.
    NoSecretTool,
    /// `secret-tool` ran and failed, usually because no keyring is
    /// running or it is locked.
    Refused,
    /// The key was empty.
    Empty,
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::NoSecretTool => write!(
                f,
                "secret-tool is not installed. Install libsecret and a keyring such as \
                 gnome-keyring, or set {ANTHROPIC_ENV} in the environment instead."
            ),
            KeyError::Refused => write!(
                f,
                "The keyring refused. Check that a keyring such as gnome-keyring is running \
                 and unlocked, or set {ANTHROPIC_ENV} in the environment instead."
            ),
            KeyError::Empty => write!(f, "No key was given."),
        }
    }
}

impl std::error::Error for KeyError {}

impl Keyring {
    fn attributes(account: &str) -> [&str; 4] {
        ["service", "scuttle", "account", account]
    }

    /// The key stored for `account`, if there is one.
    pub fn lookup(&self, account: &str) -> Option<String> {
        let mut command = Command::new(&self.program);
        command
            .arg("lookup")
            .args(Self::attributes(account))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let output = crate::proc::spawn(&mut command)
            .ok()?
            .wait_with_output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let key = String::from_utf8(output.stdout).ok()?.trim().to_string();
        (!key.is_empty()).then_some(key)
    }

    /// Stores `secret` for `account`, replacing any earlier one.
    pub fn store(&self, account: &str, secret: &str) -> Result<(), KeyError> {
        let secret = secret.trim();
        if secret.is_empty() {
            return Err(KeyError::Empty);
        }
        let label = format!("Scuttle: {account} API key");
        let mut command = Command::new(&self.program);
        command
            .arg("store")
            .arg(format!("--label={label}"))
            .args(Self::attributes(account))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = crate::proc::spawn(&mut command);
        let mut child = match child {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(KeyError::NoSecretTool)
            }
            Err(_) => return Err(KeyError::Refused),
        };
        // secret-tool reads the secret from standard input.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(secret.as_bytes());
        }
        match child.wait() {
            Ok(status) if status.success() => Ok(()),
            _ => Err(KeyError::Refused),
        }
    }

    /// Removes the key for `account`. Removing one that is not there
    /// succeeds.
    pub fn clear(&self, account: &str) -> Result<(), KeyError> {
        let mut command = Command::new(&self.program);
        command
            .arg("clear")
            .args(Self::attributes(account))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = crate::proc::spawn(&mut command).and_then(|mut child| child.wait());
        match status {
            Ok(status) if status.success() => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(KeyError::NoSecretTool),
            _ => Err(KeyError::Refused),
        }
    }

    /// The Claude API key: the keyring first, then the environment.
    pub fn anthropic_key(&self) -> Option<String> {
        self.lookup(ANTHROPIC).or_else(|| {
            if !self.read_env {
                return None;
            }
            std::env::var(ANTHROPIC_ENV)
                .ok()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
        })
    }
}

/// Shows a key safely: enough to recognise it, never enough to use it.
pub fn masked(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if key.chars().count() < 12 {
        "••••".to_string()
    } else {
        format!("••••{tail}")
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::Keyring;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in `secret-tool` that keeps secrets in a directory.
    pub fn fake_keyring(name: &str) -> Keyring {
        let dir =
            std::env::temp_dir().join(format!("scuttle-keyring-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("secret-tool");
        let body = r#"#!/bin/sh
dir="$(dirname "$0")"
verb="$1"; shift
case "$1" in --label=*) shift ;; esac
echo "$verb $*" >> "$dir/calls"
file="$dir/$(echo "$*" | tr ' /' '__')"
case "$verb" in
  store) cat > "$file" ;;
  lookup) [ -f "$file" ] && cat "$file" || exit 1 ;;
  clear) rm -f "$file" ;;
esac
"#;
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Keyring {
            program: script.to_string_lossy().to_string(),
            read_env: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::fake_keyring;
    use super::*;

    #[test]
    fn a_stored_key_can_be_read_back_and_cleared() {
        let ring = fake_keyring("roundtrip");
        assert_eq!(ring.lookup(ANTHROPIC), None);
        ring.store(ANTHROPIC, "  sk-test-123456789  \n").unwrap();
        assert_eq!(ring.lookup(ANTHROPIC).as_deref(), Some("sk-test-123456789"));
        ring.store(ANTHROPIC, "sk-newer").unwrap();
        assert_eq!(ring.lookup(ANTHROPIC).as_deref(), Some("sk-newer"));
        ring.clear(ANTHROPIC).unwrap();
        assert_eq!(ring.lookup(ANTHROPIC), None);
    }

    #[test]
    fn the_secret_never_appears_on_a_command_line() {
        let ring = fake_keyring("argv");
        ring.store(ANTHROPIC, "sk-do-not-leak").unwrap();
        let calls = std::path::Path::new(&ring.program).with_file_name("calls");
        let calls = std::fs::read_to_string(calls).unwrap();
        assert!(calls.contains("store service scuttle account anthropic"));
        assert!(!calls.contains("sk-do-not-leak"));
    }

    #[test]
    fn accounts_are_kept_apart() {
        let ring = fake_keyring("accounts");
        ring.store(ANTHROPIC, "claude-key").unwrap();
        ring.store("fish-audio", "fish-key").unwrap();
        assert_eq!(ring.lookup(ANTHROPIC).as_deref(), Some("claude-key"));
        assert_eq!(ring.lookup("fish-audio").as_deref(), Some("fish-key"));
    }

    #[test]
    fn an_empty_key_is_refused() {
        let ring = fake_keyring("empty");
        assert_eq!(ring.store(ANTHROPIC, "   "), Err(KeyError::Empty));
    }

    #[test]
    fn a_missing_secret_tool_is_explained() {
        let ring = Keyring {
            program: "/nonexistent/secret-tool".to_string(),
            read_env: false,
        };
        assert_eq!(ring.lookup(ANTHROPIC), None);
        assert_eq!(ring.store(ANTHROPIC, "k"), Err(KeyError::NoSecretTool));
        assert_eq!(ring.clear(ANTHROPIC), Err(KeyError::NoSecretTool));
        assert!(KeyError::NoSecretTool.to_string().contains("libsecret"));
    }

    #[test]
    fn masking_shows_only_the_tail() {
        assert_eq!(masked("sk-ant-api03-abcdefghijklmnop"), "••••mnop");
        assert_eq!(masked("short"), "••••");
        assert!(!masked("sk-ant-api03-abcdefghijklmnop").contains("abc"));
    }
}
