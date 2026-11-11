//! Actions the assistant can ask for, and how they are picked out of
//! its reply.
//!
//! The assistant asks for an action by writing a fenced code block
//! tagged `scuttle` that holds one JSON object. This works the same
//! through the `claude` command and through the API, so there is one
//! protocol for both backends.

use serde::Deserialize;

/// The Scuttle commands the assistant may run without asking, with what
/// each one does. Anything not on this list is refused.
pub const DESKTOP_COMMANDS: &[(&str, &str)] = &[
    ("volume up", "raise the volume a step"),
    ("volume down", "lower the volume a step"),
    ("volume mute", "mute or unmute"),
    ("brightness up", "raise the screen brightness a step"),
    ("brightness down", "lower the screen brightness a step"),
    ("media toggle", "play or pause the current media player"),
    ("media next", "skip to the next track"),
    ("media previous", "go back a track"),
    ("dnd on", "turn do not disturb on"),
    ("dnd off", "turn do not disturb off"),
    ("dismiss", "dismiss every notification on screen"),
    ("toggle bar", "hide or show the bar"),
];

/// Something the assistant wants done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// One of [`DESKTOP_COMMANDS`], as arguments for the `scuttle`
    /// command. Runs straight away.
    Desktop(Vec<String>),
    /// A terminal command. Never runs without the user's approval.
    Shell { command: String, why: String },
}

/// A piece of the assistant's reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Action(Action),
    /// An action block that could not be used, with the reason. Shown
    /// instead of being silently dropped.
    Rejected(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Block {
    scuttle: Option<String>,
    shell: Option<String>,
    #[serde(default)]
    why: String,
}

/// Turns the JSON inside an action block into an action.
fn parse_block(body: &str) -> Result<Action, String> {
    let block: Block = serde_json::from_str(body.trim())
        .map_err(|_| "The assistant wrote an action Scuttle could not read.".to_string())?;
    match (block.scuttle, block.shell) {
        (Some(command), None) => {
            let command = command.split_whitespace().collect::<Vec<_>>().join(" ");
            if DESKTOP_COMMANDS
                .iter()
                .any(|(allowed, _)| *allowed == command)
            {
                Ok(Action::Desktop(
                    command.split(' ').map(str::to_string).collect(),
                ))
            } else {
                Err(format!(
                    "The assistant asked for \"scuttle {command}\", which it is not allowed to run."
                ))
            }
        }
        (None, Some(command)) if !command.trim().is_empty() => Ok(Action::Shell {
            command: command.trim().to_string(),
            why: block.why.trim().to_string(),
        }),
        _ => Err("The assistant wrote an action Scuttle could not read.".to_string()),
    }
}

/// Splits a reply into text and actions, in order.
///
/// A block that is still being written (no closing fence yet) is left
/// out, so this can be called on a partial reply while it streams in
/// without showing half an action as text.
pub fn segments(reply: &str) -> Vec<Segment> {
    const OPEN: &str = "```scuttle";
    let mut out = Vec::new();
    let mut rest = reply;

    let push_text = |out: &mut Vec<Segment>, text: &str| {
        let text = text.trim();
        if !text.is_empty() {
            out.push(Segment::Text(text.to_string()));
        }
    };

    loop {
        // An opening fence only counts at the start of a line.
        let open = rest
            .match_indices(OPEN)
            .find(|(at, _)| *at == 0 || rest.as_bytes()[at - 1] == b'\n')
            .map(|(at, _)| at);
        let Some(open) = open else {
            push_text(&mut out, rest);
            break;
        };
        push_text(&mut out, &rest[..open]);
        let after_tag = &rest[open + OPEN.len()..];
        // The tag must end its line: "```scuttle-notes" is not ours.
        let Some(line_end) = after_tag.find('\n') else {
            break;
        };
        if !after_tag[..line_end].trim().is_empty() {
            push_text(&mut out, &rest[open..open + OPEN.len() + line_end]);
            rest = &after_tag[line_end..];
            continue;
        }
        let body = &after_tag[line_end + 1..];
        let Some(close) = body
            .find("\n```")
            .or_else(|| body.strip_suffix("```").map(|b| b.len()))
        else {
            // Still streaming: hide the unfinished block.
            break;
        };
        out.push(match parse_block(&body[..close]) {
            Ok(action) => Segment::Action(action),
            Err(reason) => Segment::Rejected(reason),
        });
        let tail = &body[close..];
        rest = tail
            .trim_start_matches('\n')
            .strip_prefix("```")
            .unwrap_or("");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Segment {
        Segment::Text(s.to_string())
    }

    #[test]
    fn plain_text_is_one_segment() {
        assert_eq!(segments("Just an answer."), vec![text("Just an answer.")]);
        assert!(segments("").is_empty());
        assert!(segments("  \n ").is_empty());
    }

    #[test]
    fn finds_a_desktop_action_between_text() {
        let reply = "Turning it down.\n\n```scuttle\n{\"scuttle\": \"volume down\"}\n```\n\nDone.";
        assert_eq!(
            segments(reply),
            vec![
                text("Turning it down."),
                Segment::Action(Action::Desktop(vec!["volume".into(), "down".into()])),
                text("Done."),
            ]
        );
    }

    #[test]
    fn finds_a_shell_proposal_with_its_reason() {
        let reply = "rust-analyzer is busy.\n```scuttle\n{\"shell\": \"pkill rust-analyzer\", \"why\": \"Stops it\"}\n```";
        assert_eq!(
            segments(reply),
            vec![
                text("rust-analyzer is busy."),
                Segment::Action(Action::Shell {
                    command: "pkill rust-analyzer".into(),
                    why: "Stops it".into()
                }),
            ]
        );
    }

    #[test]
    fn several_actions_keep_their_order() {
        let reply = "```scuttle\n{\"scuttle\":\"dnd on\"}\n```\nand\n```scuttle\n{\"scuttle\":\"media toggle\"}\n```";
        let found = segments(reply);
        assert_eq!(found.len(), 3);
        assert!(matches!(&found[0], Segment::Action(Action::Desktop(a)) if a == &["dnd", "on"]));
        assert_eq!(found[1], text("and"));
        assert!(
            matches!(&found[2], Segment::Action(Action::Desktop(a)) if a == &["media", "toggle"])
        );
    }

    #[test]
    fn commands_not_on_the_list_are_refused_and_said_so() {
        for sneaky in [
            "quit",
            "config example",
            "volume up; rm -rf ~",
            "launcher x",
            "theme push",
        ] {
            let reply = format!("```scuttle\n{{\"scuttle\": \"{sneaky}\"}}\n```");
            let found = segments(&reply);
            assert!(
                matches!(&found[0], Segment::Rejected(why) if why.contains("not allowed")),
                "{sneaky} should be refused, got {found:?}"
            );
        }
    }

    #[test]
    fn a_desktop_action_can_never_carry_a_shell_command() {
        // Both keys at once is ambiguous, so it is refused outright.
        let reply = "```scuttle\n{\"scuttle\": \"volume up\", \"shell\": \"rm -rf ~\"}\n```";
        assert!(matches!(segments(reply)[0], Segment::Rejected(_)));
        // Unknown keys are refused too.
        let reply = "```scuttle\n{\"scuttle\": \"volume up\", \"sudo\": true}\n```";
        assert!(matches!(segments(reply)[0], Segment::Rejected(_)));
    }

    #[test]
    fn malformed_blocks_are_reported_not_dropped() {
        for bad in [
            "not json",
            "{}",
            "{\"shell\": \"\"}",
            "[1,2]",
            "{\"scuttle\": 5}",
        ] {
            let reply = format!("before\n```scuttle\n{bad}\n```\nafter");
            let found = segments(&reply);
            assert_eq!(found.len(), 3, "{bad}");
            assert!(matches!(found[1], Segment::Rejected(_)), "{bad}");
        }
    }

    #[test]
    fn an_unfinished_block_is_hidden_while_streaming() {
        let partial = "Turning it down.\n\n```scuttle\n{\"scuttle\": \"vol";
        assert_eq!(segments(partial), vec![text("Turning it down.")]);
        let just_opened = "Okay.\n```scuttle";
        assert_eq!(segments(just_opened), vec![text("Okay.")]);
    }

    #[test]
    fn other_code_blocks_are_left_as_text() {
        let reply = "Run this:\n```sh\nls -la\n```\nThat lists files.";
        assert_eq!(segments(reply), vec![text(reply)]);
        let lookalike = "```scuttle-notes\nhello\n```";
        assert!(segments(lookalike)
            .iter()
            .all(|s| matches!(s, Segment::Text(_))));
    }

    #[test]
    fn a_fence_in_the_middle_of_a_line_is_not_an_action() {
        let reply = "Type ```scuttle to start a block.";
        assert_eq!(segments(reply), vec![text(reply)]);
    }

    #[test]
    fn whitespace_in_commands_is_normalised() {
        let reply = "```scuttle\n{\"scuttle\": \"  volume   up \"}\n```";
        assert_eq!(
            segments(reply),
            vec![Segment::Action(Action::Desktop(vec![
                "volume".into(),
                "up".into()
            ]))]
        );
    }

    #[test]
    fn every_listed_command_parses() {
        for (command, _) in DESKTOP_COMMANDS {
            let reply = format!("```scuttle\n{{\"scuttle\": \"{command}\"}}\n```");
            assert!(
                matches!(segments(&reply)[0], Segment::Action(Action::Desktop(_))),
                "{command}"
            );
        }
    }
}
