//! The API key backend: the Claude Messages API over plain HTTPS.
//!
//! Used when the subscription is at its limit or the `claude` command
//! is not set up. The answer is streamed, so the panel fills in as it
//! arrives.

use std::io::{BufRead, BufReader};
use std::time::Duration;

use serde_json::{json, Value};

use crate::conversation::{Role, Turn};

/// Where the API lives. Tests point this at a local stand-in.
pub const DEFAULT_BASE: &str = "https://api.anthropic.com";

const API_VERSION: &str = "2026-10-21";

/// Room for the answer. A side panel answer is short; this is a
/// ceiling, not a target.
const MAX_TOKENS: u32 = 16_000;

/// The JSON body for one request.
///
/// The API wants turns to alternate, starting with the user. Scuttle's
/// transcript nearly always does, but a stopped answer can leave two
/// turns from the same side in a row, so neighbours are merged.
pub fn request_body(model: &str, system: &str, turns: &[Turn]) -> Value {
    let mut messages: Vec<(Role, String)> = Vec::new();
    for turn in turns {
        if turn.text.trim().is_empty() {
            continue;
        }
        match messages.last_mut() {
            Some((role, text)) if *role == turn.role => {
                text.push_str("\n\n");
                text.push_str(&turn.text);
            }
            _ => messages.push((turn.role, turn.text.clone())),
        }
    }
    // A transcript cannot open with the assistant.
    while messages
        .first()
        .is_some_and(|(role, _)| *role == Role::Assistant)
    {
        messages.remove(0);
    }
    let messages: Vec<Value> = messages
        .into_iter()
        .map(|(role, text)| json!({ "role": role.api_name(), "content": text }))
        .collect();
    json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "stream": true,
        "system": system,
        "messages": messages,
    })
}

/// What one line of the event stream means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// More of the answer.
    Delta(String),
    /// Why the answer ended: `end_turn`, `max_tokens`, `refusal`, ...
    Stopped(String),
    /// The stream reported an error part way through.
    Error(String),
    /// The message is complete.
    End,
    Other,
}

/// Reads one line of the server-sent event stream.
pub fn parse_line(line: &str) -> Line {
    let Some(data) = line.strip_prefix("data:") else {
        return Line::Other;
    };
    let Ok(value) = serde_json::from_str::<Value>(data.trim()) else {
        return Line::Other;
    };
    match value["type"].as_str().unwrap_or("") {
        "content_block_delta" if value["delta"]["type"] == "text_delta" => {
            match value["delta"]["text"].as_str() {
                Some(text) if !text.is_empty() => Line::Delta(text.to_string()),
                _ => Line::Other,
            }
        }
        "message_delta" => match value["delta"]["stop_reason"].as_str() {
            Some(reason) => Line::Stopped(reason.to_string()),
            None => Line::Other,
        },
        "message_stop" => Line::End,
        "error" => Line::Error(
            value["error"]["message"]
                .as_str()
                .unwrap_or("The API reported an error.")
                .to_string(),
        ),
        _ => Line::Other,
    }
}

/// How a request ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Answered,
    /// The answer hit the length ceiling and is incomplete.
    CutShort,
    /// The model declined to answer.
    Refused,
    /// The user stopped it.
    Stopped,
    Failed(String),
}

/// Turns an HTTP error into something the user can act on.
pub fn explain_status(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_default();
    let with_detail = |lead: &str| {
        if detail.is_empty() {
            lead.to_string()
        } else {
            format!("{lead} The API said: {detail}")
        }
    };
    match status {
        401 => "The API key was not accepted. Set a new one with `scuttle assistant set-key`."
            .to_string(),
        403 => with_detail("The API key is not allowed to do this."),
        404 => with_detail("The API did not recognise the model. Check `api_model` in the config."),
        413 => "The conversation is too long to send. Start a new chat.".to_string(),
        429 => with_detail("The API key has hit its rate limit. Wait a moment and try again."),
        500..=599 => "Claude's API is having trouble right now. Try again in a moment.".to_string(),
        _ => with_detail(&format!("The API refused the request (HTTP {status}).")),
    }
}

/// Whether `base` points at this machine.
fn is_local(base: &str) -> bool {
    let host = base.split("://").nth(1).unwrap_or(base);
    let host = host.split(['/', ':']).next().unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "[")
}

/// Sends the conversation and streams the answer, calling `on_delta`
/// with each piece. `on_delta` returns false to stop early.
pub fn run(
    base: &str,
    key: &str,
    model: &str,
    system: &str,
    turns: &[Turn],
    mut on_delta: impl FnMut(&str) -> bool,
) -> Outcome {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        // No overall timeout: a streamed answer can run for minutes.
        // This bounds the gap between pieces instead.
        .timeout_read(Duration::from_secs(120))
        // Honour HTTPS_PROXY and friends, except for a server on this
        // machine, which a proxy could not reach anyway.
        .try_proxy_from_env(!is_local(base))
        .build();
    let body = request_body(model, system, turns).to_string();
    let response = agent
        .post(&format!("{}/v1/messages", base.trim_end_matches('/')))
        .set("x-api-key", key)
        .set("anthropic-version", API_VERSION)
        .set("content-type", "application/json")
        .send_string(&body);
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Outcome::Failed(explain_status(status, &body));
        }
        Err(ureq::Error::Transport(_)) => {
            // The transport error can quote the request, so it is not
            // passed on. The cause is nearly always the same anyway.
            return Outcome::Failed(
                "Could not reach Claude's API. Check the internet connection.".to_string(),
            );
        }
    };

    let mut stop_reason = String::new();
    let mut answered = false;
    for line in BufReader::new(response.into_reader()).lines() {
        let Ok(line) = line else {
            return Outcome::Failed(
                "The connection dropped before the answer finished.".to_string(),
            );
        };
        match parse_line(&line) {
            Line::Delta(text) => {
                answered = true;
                if !on_delta(&text) {
                    return Outcome::Stopped;
                }
            }
            Line::Stopped(reason) => stop_reason = reason,
            Line::Error(message) => return Outcome::Failed(message),
            Line::End => break,
            Line::Other => {}
        }
    }
    match stop_reason.as_str() {
        "refusal" => Outcome::Refused,
        "max_tokens" => Outcome::CutShort,
        _ if answered => Outcome::Answered,
        // A finished stream with no text and no stop reason means the
        // connection closed early.
        "" => Outcome::Failed("The connection dropped before the answer started.".to_string()),
        _ => Outcome::Answered,
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! A stand-in for the API: a local HTTP server that answers one
    //! request with a fixed response and keeps what it was sent.

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    pub struct FakeApi {
        pub base: String,
        requests: mpsc::Receiver<String>,
    }

    impl FakeApi {
        /// Serves `responses` in order, one per connection.
        pub fn start(responses: Vec<String>) -> FakeApi {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (tx, requests) = mpsc::channel();
            thread::spawn(move || {
                for response in responses {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    let mut request = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let n = stream.read(&mut buf).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        request.extend_from_slice(&buf[..n]);
                        let text = String::from_utf8_lossy(&request);
                        if let Some(head_end) = text.find("\r\n\r\n") {
                            let length = text[..head_end]
                                .lines()
                                .find_map(|l| {
                                    let (name, value) = l.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())?
                                })
                                .unwrap_or(0);
                            if request.len() >= head_end + 4 + length {
                                break;
                            }
                        }
                    }
                    let _ = tx.send(String::from_utf8_lossy(&request).to_string());
                    let _ = stream.write_all(response.as_bytes());
                }
            });
            FakeApi { base, requests }
        }

        /// The next request the server received, headers and body.
        pub fn request(&self) -> String {
            self.requests
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
        }
    }

    /// A successful streamed answer made of `pieces`.
    pub fn stream(pieces: &[&str], stop_reason: &str) -> String {
        let mut body = String::from(
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"role\":\"assistant\",\"content\":[]}}\n\n\
             event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
             event: ping\ndata: {\"type\":\"ping\"}\n\n",
        );
        for piece in pieces {
            let delta = serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": { "type": "text_delta", "text": piece },
            });
            body.push_str(&format!("event: content_block_delta\ndata: {delta}\n\n"));
        }
        body.push_str(&format!(
            "event: content_block_stop\ndata: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n\
             event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"{stop_reason}\"}},\"usage\":{{\"output_tokens\":5}}}}\n\n\
             event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
        ));
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
        )
    }

    /// An HTTP error with a JSON body in the API's shape.
    pub fn error(status: u16, kind: &str, message: &str) -> String {
        let body = serde_json::json!({
            "type": "error",
            "error": { "type": kind, "message": message },
        })
        .to_string();
        format!(
            "HTTP/1.1 {status} Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{error, stream, FakeApi};
    use super::*;

    fn turn(role: Role, text: &str) -> Turn {
        Turn {
            role,
            text: text.to_string(),
        }
    }

    #[test]
    fn the_body_carries_the_model_the_system_prompt_and_the_transcript() {
        let turns = [
            turn(Role::User, "hi"),
            turn(Role::Assistant, "hello"),
            turn(Role::User, "why is my fan loud"),
        ];
        let body = request_body("claude-haiku-5-5", "be brief", &turns);
        assert_eq!(body["model"], "claude-haiku-5-5");
        assert_eq!(body["system"], "be brief");
        assert_eq!(body["stream"], true);
        assert_eq!(body["messages"].as_array().unwrap().len(), 3);
        assert_eq!(
            body["messages"][0],
            json!({ "role": "user", "content": "hi" })
        );
        assert_eq!(body["messages"][2]["content"], "why is my fan loud");
    }

    #[test]
    fn the_body_leaves_sampling_and_thinking_to_the_api_s_defaults() {
        let body = request_body("m", "s", &[turn(Role::User, "hi")]);
        let keys: Vec<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for absent in ["temperature", "top_p", "top_k", "thinking", "tool_choice"] {
            assert!(!keys.contains(&absent), "{absent} should not be sent");
        }
    }

    #[test]
    fn turns_always_alternate_and_start_with_the_user() {
        let turns = [
            turn(Role::Assistant, "stray opener"),
            turn(Role::User, "one"),
            turn(Role::User, "two"),
            turn(Role::Assistant, ""),
            turn(Role::Assistant, "answer"),
        ];
        let body = request_body("m", "s", &turns);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[0],
            json!({ "role": "user", "content": "one\n\ntwo" })
        );
        assert_eq!(
            messages[1],
            json!({ "role": "assistant", "content": "answer" })
        );
    }

    #[test]
    fn reads_the_stream_lines_that_matter() {
        assert_eq!(
            parse_line(
                r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#
            ),
            Line::Delta("Hi".into())
        );
        assert_eq!(
            parse_line(r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#),
            Line::Stopped("end_turn".into())
        );
        assert_eq!(parse_line(r#"data: {"type":"message_stop"}"#), Line::End);
        assert_eq!(
            parse_line(
                r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#
            ),
            Line::Error("Overloaded".into())
        );
        for other in [
            "event: content_block_delta",
            "",
            r#"data: {"type":"ping"}"#,
            r#"data: {"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hm"}}"#,
            "data: not json",
        ] {
            assert_eq!(parse_line(other), Line::Other, "{other}");
        }
    }

    #[test]
    fn a_streamed_answer_arrives_piece_by_piece() {
        let api = FakeApi::start(vec![stream(&["Your fan ", "is loud."], "end_turn")]);
        let mut heard = Vec::new();
        let outcome = run(
            &api.base,
            "test-key",
            "claude-haiku-5-5",
            "sys",
            &[turn(Role::User, "why")],
            |d| {
                heard.push(d.to_string());
                true
            },
        );
        assert_eq!(outcome, Outcome::Answered);
        assert_eq!(heard, ["Your fan ", "is loud."]);

        let request = api.request();
        let lower = request.to_lowercase();
        assert!(request.starts_with("POST /v1/messages "));
        assert!(lower.contains("x-api-key: test-key"));
        assert!(lower.contains("anthropic-version: 2026-10-21"));
        assert!(request.contains("\"model\":\"claude-haiku-5-5\""));
    }

    #[test]
    fn a_refusal_and_a_cut_off_answer_are_told_apart_from_success() {
        let api = FakeApi::start(vec![
            stream(&[], "refusal"),
            stream(&["half an ans"], "max_tokens"),
        ]);
        let turns = [turn(Role::User, "q")];
        assert_eq!(
            run(&api.base, "k", "m", "s", &turns, |_| true),
            Outcome::Refused
        );
        assert_eq!(
            run(&api.base, "k", "m", "s", &turns, |_| true),
            Outcome::CutShort
        );
    }

    #[test]
    fn stopping_early_ends_the_request() {
        let api = FakeApi::start(vec![stream(&["one", "two", "three"], "end_turn")]);
        let mut heard = 0;
        let outcome = run(&api.base, "k", "m", "s", &[turn(Role::User, "q")], |_| {
            heard += 1;
            false
        });
        assert_eq!(outcome, Outcome::Stopped);
        assert_eq!(heard, 1);
    }

    #[test]
    fn a_rejected_key_says_how_to_fix_it() {
        let api = FakeApi::start(vec![error(
            401,
            "authentication_error",
            "invalid x-api-key",
        )]);
        let Outcome::Failed(message) =
            run(&api.base, "bad", "m", "s", &[turn(Role::User, "q")], |_| {
                true
            })
        else {
            panic!("expected a failure");
        };
        assert!(message.contains("scuttle assistant set-key"), "{message}");
        assert!(!message.contains("bad"), "the key must never be echoed");
    }

    #[test]
    fn other_errors_pass_on_what_the_api_said() {
        assert!(explain_status(429, r#"{"error":{"message":"Slow down"}}"#).contains("Slow down"));
        assert!(explain_status(
            400,
            r#"{"error":{"message":"Your credit balance is too low"}}"#
        )
        .contains("credit balance"));
        assert!(explain_status(529, "").contains("Try again"));
        assert!(explain_status(404, "not json").contains("api_model"));
    }

    #[test]
    fn only_this_machine_skips_the_proxy() {
        assert!(is_local("http://127.0.0.1:8080"));
        assert!(is_local("http://localhost:9/v1"));
        assert!(is_local("http://[::1]:8080"));
        assert!(!is_local(DEFAULT_BASE));
        assert!(!is_local("https://localhost.example.com"));
    }

    #[test]
    fn an_unreachable_api_fails_without_leaking_the_request() {
        // Nothing listens on this port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let Outcome::Failed(message) = run(
            &base,
            "secret-key",
            "m",
            "s",
            &[turn(Role::User, "q")],
            |_| true,
        ) else {
            panic!("expected a failure");
        };
        assert!(message.contains("Could not reach"));
        assert!(!message.contains("secret-key"));
    }

    #[test]
    fn a_connection_that_closes_with_nothing_is_a_failure() {
        let api = FakeApi::start(vec![
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
                .to_string(),
        ]);
        assert!(matches!(
            run(&api.base, "k", "m", "s", &[turn(Role::User, "q")], |_| true),
            Outcome::Failed(_)
        ));
    }
}
