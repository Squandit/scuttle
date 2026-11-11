//! The conversation so far.
//!
//! Scuttle keeps its own transcript. The `claude` command also keeps
//! one per session, which is cheaper to continue, so the conversation
//! tracks how much of the transcript that session has seen. After a
//! spell on the API backend the two drift apart, and the next
//! subscription message starts a fresh session with the transcript
//! included.

/// Who said something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    /// The role name the Claude API uses.
    pub fn api_name(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

/// One message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub text: String,
}

/// A conversation with the assistant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Conversation {
    turns: Vec<Turn>,
    /// The `claude` session that holds this conversation, if any.
    cli_session: Option<String>,
    /// How many turns that session has seen.
    cli_seen: usize,
}

impl Conversation {
    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    pub fn push(&mut self, role: Role, text: impl Into<String>) {
        let text = text.into();
        self.turns.push(Turn { role, text });
    }

    /// Removes the last turn if it is the user's. Used when a message
    /// failed to send, so it is not left dangling without an answer.
    pub fn drop_unanswered(&mut self) {
        if self.turns.last().is_some_and(|t| t.role == Role::User) {
            self.turns.pop();
        }
    }

    /// The `claude` session to resume, when it has seen everything
    /// except the newest user message.
    pub fn resumable_session(&self) -> Option<&str> {
        let caught_up = self.cli_seen + 1 == self.turns.len();
        self.cli_session.as_deref().filter(|_| caught_up)
    }

    /// Records that `session` now holds the whole transcript.
    pub fn synced_with(&mut self, session: &str) {
        self.cli_session = Some(session.to_string());
        self.cli_seen = self.turns.len();
    }

    /// What to send the `claude` command for the newest user message:
    /// just that message when its session can be resumed, otherwise the
    /// transcript so far followed by the message.
    pub fn cli_prompt(&self) -> String {
        let Some((latest, earlier)) = self.turns.split_last() else {
            return String::new();
        };
        if self.resumable_session().is_some() || earlier.is_empty() {
            return latest.text.clone();
        }
        let mut prompt = String::from(
            "This conversation is already under way. Here is what has been said so far.\n\n",
        );
        for turn in earlier {
            let who = match turn.role {
                Role::User => "User",
                Role::Assistant => "You",
            };
            prompt.push_str(&format!("{who}: {}\n\n", turn.text));
        }
        prompt.push_str("The user's new message follows. Reply to it.\n\n");
        prompt.push_str(&latest.text);
        prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_message_is_sent_as_is_with_no_session() {
        let mut c = Conversation::default();
        c.push(Role::User, "why is my fan loud");
        assert_eq!(c.resumable_session(), None);
        assert_eq!(c.cli_prompt(), "why is my fan loud");
    }

    #[test]
    fn a_synced_session_is_resumed_with_only_the_new_message() {
        let mut c = Conversation::default();
        c.push(Role::User, "one");
        c.push(Role::Assistant, "answer one");
        c.synced_with("sess-1");
        c.push(Role::User, "two");
        assert_eq!(c.resumable_session(), Some("sess-1"));
        assert_eq!(c.cli_prompt(), "two");
    }

    #[test]
    fn turns_answered_elsewhere_force_a_fresh_session_with_the_transcript() {
        let mut c = Conversation::default();
        c.push(Role::User, "one");
        c.push(Role::Assistant, "answer one");
        c.synced_with("sess-1");
        // These two went through the API backend instead.
        c.push(Role::User, "two");
        c.push(Role::Assistant, "answer two");
        c.push(Role::User, "three");
        assert_eq!(c.resumable_session(), None);
        let prompt = c.cli_prompt();
        assert!(prompt.contains("User: one"));
        assert!(prompt.contains("You: answer two"));
        assert!(prompt.ends_with("three"));
    }

    #[test]
    fn a_failed_message_can_be_taken_back() {
        let mut c = Conversation::default();
        c.push(Role::User, "one");
        c.drop_unanswered();
        assert!(c.is_empty());
        c.push(Role::User, "one");
        c.push(Role::Assistant, "answer");
        c.drop_unanswered();
        assert_eq!(c.turns().len(), 2, "answered turns are kept");
    }

    #[test]
    fn an_empty_conversation_has_nothing_to_send() {
        assert_eq!(Conversation::default().cli_prompt(), "");
    }
}
