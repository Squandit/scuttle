//! Running a terminal command the user approved.
//!
//! Nothing here decides whether a command should run. That is the
//! user's call, made with a button in the panel. This only runs it,
//! collects what it printed, and makes sure it cannot hang forever or
//! flood the conversation.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The most output kept, in bytes. Enough for an error message or a
/// process list; a build log is cut down to its start and its end.
pub const MAX_OUTPUT: usize = 6000;

/// What happened when a command ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The exit status. `None` when the command was killed.
    pub status: Option<i32>,
    /// Standard output and standard error, in the order they came.
    pub output: String,
    /// The command ran out of time and was stopped.
    pub timed_out: bool,
}

impl Outcome {
    pub fn succeeded(&self) -> bool {
        self.status == Some(0)
    }

    /// One line saying how it went, for the panel.
    pub fn summary(&self) -> String {
        match (self.timed_out, self.status) {
            (true, _) => "Stopped: it was still running after the time limit".to_string(),
            (false, Some(0)) => "Finished".to_string(),
            (false, Some(code)) => format!("Failed with exit status {code}"),
            (false, None) => "Stopped by a signal".to_string(),
        }
    }

    /// What to tell Claude about it.
    pub fn note(&self, command: &str) -> String {
        let how = match (self.timed_out, self.status) {
            (true, _) => "It was still running after the time limit and was stopped.".to_string(),
            (false, Some(code)) => format!("It exited with status {code}."),
            (false, None) => "It was stopped by a signal.".to_string(),
        };
        if self.output.trim().is_empty() {
            format!("the user ran `{command}`. {how} It printed nothing.")
        } else {
            format!(
                "the user ran `{command}`. {how} It printed:\n{}",
                self.output.trim_end()
            )
        }
    }
}

/// Keeps the start and the end of long output, where the useful parts
/// usually are, and says how much was left out.
pub fn shorten(output: &str, limit: usize) -> String {
    if output.len() <= limit {
        return output.to_string();
    }
    let half = limit / 2;
    let mut head_end = half;
    while !output.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = output.len() - half;
    while !output.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let skipped = output[head_end..tail_start].lines().count();
    format!(
        "{}\n[... {skipped} lines left out ...]\n{}",
        output[..head_end].trim_end(),
        output[tail_start..].trim_start()
    )
}

/// Runs `command` through `sh`, waiting at most `limit`.
///
/// The command gets no terminal and no input, so anything that waits
/// for a key press ends at once instead of hanging.
pub fn run(command: &str, limit: Duration) -> Outcome {
    let mut shell = Command::new("sh");
    // Start in the home folder, as a new terminal would.
    if let Some(home) = std::env::var_os("HOME").filter(|h| Path::new(h).is_dir()) {
        shell.current_dir(home);
    }
    let child = shell
        .arg("-c")
        // One stream, so output and errors stay in order.
        .arg(format!("exec 2>&1\n{command}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Its own process group, so a timeout stops everything it started.
        .process_group(0)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => {
            return Outcome {
                status: None,
                output: format!("Could not start a shell: {e}"),
                timed_out: false,
            }
        }
    };

    // The reader fills a shared buffer, so what has been printed is
    // there to take even if the pipe never closes.
    let collected = Arc::new(Mutex::new(Vec::new()));
    let (done_tx, done_rx) = mpsc::channel();
    if let Some(mut stdout) = child.stdout.take() {
        let collected = Arc::clone(&collected);
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            // Read to the end even past the limit, so the command is
            // never blocked on a full pipe. Only the kept part grows.
            while let Ok(n) = stdout.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut bytes) = collected.lock() {
                    if bytes.len() < MAX_OUTPUT * 40 {
                        bytes.extend_from_slice(&buf[..n]);
                    }
                }
            }
            let _ = done_tx.send(());
        });
    }

    let deadline = Instant::now() + limit;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{}", child.id())])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(40)),
            Err(_) => break None,
        }
    };
    // A background process the command left behind can hold the pipe
    // open. Give the reader a moment to finish, then take what it has.
    let _ = done_rx.recv_timeout(Duration::from_millis(500));
    let bytes = collected
        .lock()
        .map(|bytes| bytes.clone())
        .unwrap_or_default();
    let output = shorten(&String::from_utf8_lossy(&bytes), MAX_OUTPUT);
    Outcome {
        status,
        output,
        timed_out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: Duration = Duration::from_secs(10);

    #[test]
    fn output_and_status_are_collected() {
        let outcome = run("echo hello; echo oops >&2; exit 3", LIMIT);
        assert_eq!(outcome.status, Some(3));
        assert_eq!(outcome.output, "hello\noops\n");
        assert!(!outcome.succeeded());
        assert_eq!(outcome.summary(), "Failed with exit status 3");
    }

    #[test]
    fn a_successful_command_says_so() {
        let outcome = run("true", LIMIT);
        assert!(outcome.succeeded());
        assert_eq!(outcome.summary(), "Finished");
        assert_eq!(
            outcome.note("true"),
            "the user ran `true`. It exited with status 0. It printed nothing."
        );
    }

    #[test]
    fn the_note_carries_the_output() {
        let outcome = run("echo up 3 days", LIMIT);
        assert_eq!(
            outcome.note("uptime"),
            "the user ran `uptime`. It exited with status 0. It printed:\nup 3 days"
        );
    }

    #[test]
    fn a_command_waiting_for_input_ends_instead_of_hanging() {
        let started = Instant::now();
        let outcome = run("read answer; echo got:$answer", LIMIT);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(outcome.output, "got:\n");
    }

    #[test]
    fn a_command_that_runs_too_long_is_stopped_with_everything_it_started() {
        let started = Instant::now();
        let outcome = run(
            "echo before; sleep 30; echo after",
            Duration::from_millis(300),
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(outcome.timed_out);
        assert_eq!(outcome.status, None);
        assert_eq!(outcome.output, "before\n");
        assert!(outcome.summary().contains("time limit"));
    }

    #[test]
    fn a_background_process_does_not_hold_things_up() {
        let started = Instant::now();
        let outcome = run("sleep 30 & echo started", LIMIT);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(outcome.status, Some(0));
        assert_eq!(outcome.output, "started\n");
    }

    #[test]
    fn commands_start_in_the_home_folder() {
        let home = std::env::var("HOME").unwrap();
        let outcome = run("pwd", LIMIT);
        assert_eq!(
            std::fs::canonicalize(outcome.output.trim()).unwrap(),
            std::fs::canonicalize(home).unwrap()
        );
    }

    #[test]
    fn long_output_keeps_its_start_and_end() {
        let outcome = run("seq 1 5000", LIMIT);
        assert!(outcome.output.len() < MAX_OUTPUT + 200);
        assert!(outcome.output.starts_with("1\n2\n3\n"));
        assert!(outcome.output.trim_end().ends_with("5000"));
        assert!(outcome.output.contains("lines left out"));
    }

    #[test]
    fn shortening_respects_character_boundaries() {
        let text = "é".repeat(5000);
        let short = shorten(&text, 1001);
        assert!(short.len() < 1200);
        assert!(short.contains("left out"));
        assert_eq!(shorten("short", 100), "short");
    }

    #[test]
    fn a_missing_program_is_reported_as_output() {
        let outcome = run("definitely-not-a-real-program-xyz", LIMIT);
        assert_eq!(outcome.status, Some(127));
        assert!(outcome.output.contains("not found"));
    }
}
