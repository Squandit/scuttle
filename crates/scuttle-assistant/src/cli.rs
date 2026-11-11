//! The subscription backend: the `claude` command.
//!
//! Running `claude --print` answers one message using the user's Claude
//! subscription and prints one JSON object per line as it goes. This
//! module builds the command line and reads those lines.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// A handle for stopping an answer part way through. Cloning it gives
/// another handle to the same answer.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<CancelInner>);

#[derive(Debug, Default)]
struct CancelInner {
    stopped: AtomicBool,
    child: Mutex<Option<Child>>,
}

impl Cancel {
    /// Stops the answer. A running `claude` command is killed, so this
    /// takes effect at once even while the model is still thinking.
    pub fn stop(&self) {
        self.0.stopped.store(true, Ordering::SeqCst);
        if let Ok(mut child) = self.0.child.lock() {
            if let Some(child) = child.as_mut() {
                // `claude` is often a small wrapper around the real
                // program. It runs in its own process group, so the
                // whole group is told to stop, not just the wrapper.
                let _ = Command::new("kill")
                    .args(["-TERM", "--", &format!("-{}", child.id())])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
            }
        }
    }

    pub fn is_stopped(&self) -> bool {
        self.0.stopped.load(Ordering::SeqCst)
    }

    fn hold(&self, child: Child) {
        if let Ok(mut slot) = self.0.child.lock() {
            *slot = Some(child);
        }
    }

    fn release(&self) -> Option<Child> {
        self.0.child.lock().ok().and_then(|mut slot| slot.take())
    }
}

/// What one line of the `claude` command's output means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// More of the answer.
    Delta(String),
    /// The session this conversation is stored under.
    Session(String),
    /// The subscription's usage limit has been reached. `resets_at` is
    /// seconds since the Unix epoch, when known.
    Limited { resets_at: Option<u64> },
    /// The run finished. `error` holds the message when it failed.
    Finished { error: Option<String> },
    /// A line that carries nothing the assistant needs.
    Other,
}

/// Reads one line of `--output-format stream-json` output.
pub fn parse_line(line: &str) -> Line {
    let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
        return Line::Other;
    };
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    match kind {
        "stream_event" => {
            let event = &value["event"];
            let is_text =
                event["type"] == "content_block_delta" && event["delta"]["type"] == "text_delta";
            match event["delta"]["text"].as_str() {
                Some(text) if is_text && !text.is_empty() => Line::Delta(text.to_string()),
                _ => Line::Other,
            }
        }
        "system" if value["subtype"] == "init" => match value["session_id"].as_str() {
            Some(id) => Line::Session(id.to_string()),
            None => Line::Other,
        },
        "rate_limit_event" => {
            let info = &value["rate_limit_info"];
            // "allowed" and "allowed_warning" both mean the request went
            // through. Anything else means the limit stopped it.
            let status = info["status"].as_str().unwrap_or("allowed");
            if status.starts_with("allowed") {
                Line::Other
            } else {
                Line::Limited {
                    resets_at: info["resetsAt"].as_u64(),
                }
            }
        }
        "result" => {
            let failed = value["is_error"].as_bool().unwrap_or(false);
            let message = value["result"].as_str().unwrap_or("").trim().to_string();
            Line::Finished {
                error: failed.then_some(message),
            }
        }
        _ => Line::Other,
    }
}

/// Whether an error message from the `claude` command means the usage
/// limit was reached, for versions that report it only as text.
pub fn looks_like_a_limit(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("usage limit") || lower.contains("rate limit") || lower.contains("limit reached")
}

/// The arguments for one non-interactive run.
///
/// Every built-in tool, MCP server and settings file is switched off:
/// the assistant acts only through Scuttle's own action protocol, so
/// the `claude` command is used purely to produce text.
pub fn arguments(model: &str, system_prompt: &str, resume: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "--print",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--verbose",
        "--model",
        model,
        "--system-prompt",
        system_prompt,
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(session) = resume {
        args.push("--resume".to_string());
        args.push(session.to_string());
    }
    args
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It answered. `session` is where the conversation is stored.
    Answered { session: Option<String> },
    /// The subscription limit stopped it before it answered.
    Limited { resets_at: Option<u64> },
    /// The user stopped it.
    Stopped,
    /// The `claude` command is not installed.
    NotInstalled,
    /// It failed for another reason.
    Failed(String),
}

/// One run of the `claude` command: what to run and what to send it.
#[derive(Debug, Clone, Copy)]
pub struct Invocation<'a> {
    pub program: &'a str,
    pub model: &'a str,
    pub system_prompt: &'a str,
    /// The session to continue, if there is one.
    pub resume: Option<&'a str>,
    /// The message, sent on standard input.
    pub prompt: &'a str,
    /// The folder to run in. `None` runs it wherever Scuttle is.
    pub dir: Option<&'a Path>,
}

/// Runs the `claude` command for one message, calling `on_delta` with
/// each piece of the answer as it arrives.
pub fn run(what: &Invocation, cancel: &Cancel, mut on_delta: impl FnMut(&str)) -> Outcome {
    let Invocation {
        program,
        model,
        system_prompt,
        resume,
        prompt,
        dir,
    } = *what;
    let mut command = Command::new(program);
    // The claude command keeps its sessions per folder, and reads
    // project notes from the folder it runs in. A folder of its own
    // keeps both away from the user's real projects.
    if let Some(dir) = dir {
        if std::fs::create_dir_all(dir).is_ok() {
            command.current_dir(dir);
        }
    }
    command
        .args(arguments(model, system_prompt, resume))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let child = crate::proc::spawn(&mut command);
    let mut child = match child {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Outcome::NotInstalled,
        Err(e) => return Outcome::Failed(format!("Could not start the claude command: {e}")),
    };
    // The message goes in on standard input, so it never appears in the
    // process list and needs no shell quoting.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }
    let stdout = child.stdout.take();
    // From here the child belongs to the cancel handle, so another
    // thread can kill it while this one is blocked reading.
    cancel.hold(child);
    if cancel.is_stopped() {
        cancel.stop();
    }

    let mut session = None;
    let mut limited: Option<Option<u64>> = None;
    let mut error: Option<String> = None;
    let mut answered = false;
    if let Some(stdout) = stdout {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            match parse_line(&line) {
                Line::Delta(text) => {
                    answered = true;
                    on_delta(&text);
                }
                Line::Session(id) => session = Some(id),
                Line::Limited { resets_at } => limited = Some(resets_at),
                Line::Finished { error: e } => error = e,
                Line::Other => {}
            }
        }
    }
    let status = match cancel.release() {
        Some(mut child) => child.wait(),
        None => return Outcome::Failed("The claude command went missing.".to_string()),
    };
    if cancel.is_stopped() {
        return Outcome::Stopped;
    }

    if let Some(resets_at) = limited {
        if !answered {
            return Outcome::Limited { resets_at };
        }
    }
    match error {
        Some(message) if looks_like_a_limit(&message) && !answered => {
            Outcome::Limited { resets_at: None }
        }
        Some(message) if message.is_empty() => {
            Outcome::Failed("The claude command reported an error.".to_string())
        }
        Some(message) => Outcome::Failed(message),
        None if answered => Outcome::Answered { session },
        None => match status {
            Ok(code) if code.success() => Outcome::Answered { session },
            _ => Outcome::Failed(
                "The claude command stopped without answering. Run `claude` in a terminal to \
                 check that it is signed in."
                    .to_string(),
            ),
        },
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in `claude` that prints fixed lines, so runs can be
    /// tested without the real command or a network. It records what
    /// it was given beside itself: `claude.stdin`, `claude.args`, and
    /// one line per run in `claude.calls`.
    pub fn fake(name: &str, lines: &[&str], exit: i32) -> String {
        let dir = std::env::temp_dir().join(format!("scuttle-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("claude");
        let mut body = String::from(
            "#!/bin/sh\ncat > \"$0.stdin\"\necho \"$@\" > \"$0.args\"\necho run >> \"$0.calls\"\npwd > \"$0.cwd\"\n",
        );
        for line in lines {
            if let Some(seconds) = line.strip_prefix("sleep ") {
                body.push_str(&format!("sleep {seconds}\n"));
            } else {
                body.push_str(&format!(
                    "printf '%s\\n' '{}'\n",
                    line.replace('\'', "'\\''")
                ));
            }
        }
        body.push_str(&format!("exit {exit}\n"));
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script.to_string_lossy().to_string()
    }

    /// How many times a fake `claude` has been run.
    pub fn calls(program: &str) -> usize {
        std::fs::read_to_string(format!("{program}.calls"))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    // Trimmed copies of real output lines.
    pub const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"/tmp","session_id":"db00ecbf-aede-497b-8340-707208b454bd","tools":[],"mcp_servers":[],"model":"claude-opus-5-5"}"#;
    pub const RESULT_OK: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"done","session_id":"db00ecbf","stop_reason":"end_turn"}"#;
    pub const REJECTED: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1794424200,"rateLimitType":"five_hour"}}"#;
    pub const RESULT_LIMIT: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Claude usage limit reached. Your limit will reset at 3pm.","session_id":"db00ecbf"}"#;

    /// An output line carrying one piece of the answer.
    pub fn delta(text: &str) -> String {
        serde_json::json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": text },
            },
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call<'a>(program: &'a str, prompt: &'a str, dir: Option<&'a Path>) -> Invocation<'a> {
        Invocation {
            program,
            model: "m",
            system_prompt: "s",
            resume: None,
            prompt,
            dir,
        }
    }

    // These lines are trimmed copies of real output from
    // `claude --print --output-format stream-json`.
    const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"/tmp","session_id":"db00ecbf-aede-497b-8340-707208b454bd","tools":[],"mcp_servers":[],"model":"claude-haiku-5-5"}"#;
    const DELTA: &str = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello from scuttle"}},"session_id":"db00ecbf","parent_tool_use_id":null}"#;
    const BLOCK_START: &str = r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}},"session_id":"db00ecbf"}"#;
    const ALLOWED: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1794424200,"rateLimitType":"five_hour","overageStatus":"rejected","isUsingOverage":false}}"#;
    const REJECTED: &str = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","resetsAt":1794424200,"rateLimitType":"five_hour"}}"#;
    const RESULT_OK: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"hello from scuttle","session_id":"db00ecbf","stop_reason":"end_turn"}"#;
    const RESULT_ERR: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Claude usage limit reached. Your limit will reset at 3pm.","session_id":"db00ecbf"}"#;

    #[test]
    fn reads_the_lines_that_matter() {
        assert_eq!(
            parse_line(INIT),
            Line::Session("db00ecbf-aede-497b-8340-707208b454bd".into())
        );
        assert_eq!(parse_line(DELTA), Line::Delta("hello from scuttle".into()));
        assert_eq!(parse_line(RESULT_OK), Line::Finished { error: None });
        assert_eq!(
            parse_line(RESULT_ERR),
            Line::Finished {
                error: Some("Claude usage limit reached. Your limit will reset at 3pm.".into())
            }
        );
    }

    #[test]
    fn an_allowed_rate_limit_report_is_not_a_limit() {
        assert_eq!(parse_line(ALLOWED), Line::Other);
        assert_eq!(
            parse_line(&ALLOWED.replace("\"allowed\"", "\"allowed_warning\"")),
            Line::Other
        );
    }

    #[test]
    fn a_rejected_rate_limit_report_carries_the_reset_time() {
        assert_eq!(
            parse_line(REJECTED),
            Line::Limited {
                resets_at: Some(1_794_424_200)
            }
        );
    }

    #[test]
    fn everything_else_is_ignored() {
        for line in [
            BLOCK_START,
            r#"{"type":"active_goal","value":null}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hmm"}}}"#,
            "not json at all",
            "",
        ] {
            assert_eq!(parse_line(line), Line::Other, "{line}");
        }
    }

    #[test]
    fn spots_limit_messages_written_as_text() {
        assert!(looks_like_a_limit(
            "Claude usage limit reached. Your limit will reset at 3pm."
        ));
        assert!(looks_like_a_limit("Rate limit exceeded"));
        assert!(!looks_like_a_limit("Invalid API key · Please run /login"));
    }

    #[test]
    fn arguments_turn_every_tool_off() {
        let args = arguments("claude-opus-5-5", "be brief", None);
        let has = |flag: &str, value: &str| {
            args.windows(2)
                .any(|pair| pair[0] == flag && pair[1] == value)
        };
        assert!(args.contains(&"--print".to_string()));
        assert!(has("--output-format", "stream-json"));
        assert!(has("--model", "claude-opus-5-5"));
        assert!(has("--system-prompt", "be brief"));
        assert!(has("--tools", ""), "built-in tools must be off");
        assert!(
            has("--setting-sources", ""),
            "user settings and hooks must not load"
        );
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn a_session_is_resumed_when_given() {
        let args = arguments("m", "s", Some("sess-1"));
        assert_eq!(&args[args.len() - 2..], ["--resume", "sess-1"]);
    }

    use super::testing::fake;

    #[test]
    fn a_normal_run_streams_the_answer_and_keeps_the_session() {
        let program = fake("ok", &[INIT, BLOCK_START, DELTA, ALLOWED, RESULT_OK], 0);
        let mut heard = String::new();
        let outcome = run(
            &call(&program, "the question", None),
            &Cancel::default(),
            |d| heard.push_str(d),
        );
        assert_eq!(heard, "hello from scuttle");
        assert_eq!(
            outcome,
            Outcome::Answered {
                session: Some("db00ecbf-aede-497b-8340-707208b454bd".into())
            }
        );
        // The message went in on standard input, not as an argument.
        assert_eq!(
            std::fs::read_to_string(format!("{program}.stdin")).unwrap(),
            "the question"
        );
        assert!(!std::fs::read_to_string(format!("{program}.args"))
            .unwrap()
            .contains("the question"));
    }

    #[test]
    fn a_limit_is_reported_with_its_reset_time() {
        let program = fake("limited", &[INIT, REJECTED, RESULT_ERR], 1);
        let outcome = run(&call(&program, "q", None), &Cancel::default(), |_| {});
        assert_eq!(
            outcome,
            Outcome::Limited {
                resets_at: Some(1_794_424_200)
            }
        );
    }

    #[test]
    fn a_limit_reported_only_as_text_is_still_recognised() {
        let program = fake("textlimit", &[INIT, RESULT_ERR], 1);
        assert_eq!(
            run(&call(&program, "q", None), &Cancel::default(), |_| {}),
            Outcome::Limited { resets_at: None }
        );
    }

    #[test]
    fn other_errors_are_passed_on_in_the_command_s_own_words() {
        let line =
            r#"{"type":"result","is_error":true,"result":"Invalid API key · Please run /login"}"#;
        let program = fake("login", &[line], 1);
        assert_eq!(
            run(&call(&program, "q", None), &Cancel::default(), |_| {}),
            Outcome::Failed("Invalid API key · Please run /login".into())
        );
    }

    #[test]
    fn a_silent_failure_says_what_to_check() {
        let program = fake("silent", &[], 1);
        let Outcome::Failed(message) = run(&call(&program, "q", None), &Cancel::default(), |_| {})
        else {
            panic!("expected a failure");
        };
        assert!(message.contains("signed in"));
    }

    #[test]
    fn stopping_kills_the_command_straight_away() {
        let program = fake("stop", &[INIT, DELTA, "sleep 30", RESULT_OK], 0);
        let cancel = Cancel::default();
        let stopper = cancel.clone();
        let started = std::time::Instant::now();
        let mut heard = String::new();
        let outcome = run(&call(&program, "q", None), &cancel, |d| {
            heard.push_str(d);
            // Stop from another thread, as the panel does.
            let stopper = stopper.clone();
            std::thread::spawn(move || stopper.stop());
        });
        assert_eq!(outcome, Outcome::Stopped);
        assert_eq!(heard, "hello from scuttle");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn the_command_runs_in_the_folder_it_is_given() {
        let program = fake("cwd", &[INIT, DELTA, RESULT_OK], 0);
        let dir = std::env::temp_dir().join(format!("scuttle-cli-cwd-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let outcome = run(&call(&program, "q", Some(&dir)), &Cancel::default(), |_| {});
        assert!(matches!(outcome, Outcome::Answered { .. }));
        // The folder was made, and the stand-in recorded running there.
        let seen = std::fs::read_to_string(format!("{program}.cwd")).unwrap();
        assert_eq!(
            std::fs::canonicalize(seen.trim()).unwrap(),
            std::fs::canonicalize(&dir).unwrap()
        );
    }

    #[test]
    fn a_missing_command_is_told_apart_from_a_failing_one() {
        assert_eq!(
            run(
                &call("/nonexistent/scuttle/claude", "q", None),
                &Cancel::default(),
                |_| {}
            ),
            Outcome::NotInstalled
        );
    }
}
