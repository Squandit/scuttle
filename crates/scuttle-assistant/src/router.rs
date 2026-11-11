//! Chooses between the two ways of reaching Claude.
//!
//! The subscription (the `claude` command) is tried first. When it
//! reports its usage limit, is not installed, or fails before saying
//! anything, the API key answers instead, and the subscription is left
//! alone until its limit resets.
//!
//! Answering is split in three so the slow part can run on a worker
//! thread without holding the assistant: [`Assistant::begin`] records
//! the message and hands back a [`Job`], [`Job::run`] does the talking,
//! and [`Assistant::finish`] records how it went.

use std::path::PathBuf;

use crate::api;
use crate::cli::{self, Cancel};
use crate::conversation::{Conversation, Role, Turn};
use crate::key::Keyring;
use crate::Event;

/// A way of reaching Claude.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The `claude` command, on the user's Claude subscription.
    Subscription,
    /// The Claude API, on the user's API key.
    ApiKey,
}

impl Backend {
    /// A short name for the panel's header.
    pub fn label(self) -> &'static str {
        match self {
            Backend::Subscription => "Subscription",
            Backend::ApiKey => "API key",
        }
    }
}

/// Which backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Prefer {
    /// The subscription first, the API key when it cannot answer.
    #[default]
    Auto,
    /// Only the subscription.
    Subscription,
    /// Only the API key.
    ApiKey,
}

/// How the assistant is set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub prefer: Prefer,
    /// The `claude` program to run.
    pub claude_program: String,
    /// The model used on the subscription.
    pub subscription_model: String,
    /// The model used on the API key.
    pub api_model: String,
    pub api_base: String,
    pub keyring: Keyring,
    /// The folder the `claude` command runs in. `None` runs it
    /// wherever Scuttle is running.
    pub claude_dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            prefer: Prefer::Auto,
            claude_program: "claude".to_string(),
            subscription_model: "claude-opus-5-5".to_string(),
            api_model: "claude-haiku-5-5".to_string(),
            api_base: api::DEFAULT_BASE.to_string(),
            keyring: Keyring::default(),
            claude_dir: None,
        }
    }
}

/// Why the subscription could not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// Its usage limit is reached. `resets_at` is seconds since the
    /// Unix epoch, when known.
    Limited { resets_at: Option<u64> },
    /// The `claude` command is not installed.
    NotInstalled,
    /// The `claude` command failed before saying anything, for example
    /// because it is not signed in.
    Failing(String),
}

/// How an answer went. Produced by [`Job::run`], given back to
/// [`Assistant::finish`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    generation: u64,
    /// Everything the assistant said, even if it was stopped or failed
    /// part way.
    pub reply: String,
    /// The backend that produced the reply.
    pub backend: Option<Backend>,
    /// The `claude` session holding the conversation, when the
    /// subscription answered.
    session: Option<String>,
    /// What was learned about the subscription on the way.
    pub subscription: Option<Unavailable>,
    /// What went wrong, in words for the user.
    pub error: Option<String>,
    /// Whether the user stopped it.
    pub stopped: bool,
}

/// How long to leave the subscription alone when it did not say when
/// its limit resets.
const RETRY_LIMITED: u64 = 15 * 60;
/// How long before checking again whether `claude` has been installed.
const RETRY_NOT_INSTALLED: u64 = 60 * 60;
/// How long to leave a failing `claude` command alone.
const RETRY_FAILING: u64 = 5 * 60;

/// A model id as people say it: "claude-opus-5-5" is "Opus 5.5".
pub fn pretty_model(id: &str) -> String {
    let mut name = String::new();
    let mut version: Vec<&str> = Vec::new();
    for part in id.strip_prefix("claude-").unwrap_or(id).split('-') {
        let numeric = !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
        if numeric && part.len() <= 2 {
            version.push(part);
        } else if numeric {
            // A release date on the end of the id. Not part of the name.
        } else if !part.is_empty() {
            let mut letters = part.chars();
            if let Some(first) = letters.next() {
                if !name.is_empty() {
                    name.push(' ');
                }
                name.extend(first.to_uppercase());
                name.push_str(letters.as_str());
            }
        }
    }
    match (name.is_empty(), version.is_empty()) {
        (true, _) => id.to_string(),
        (false, true) => name,
        (false, false) => format!("{name} {}", version.join(".")),
    }
}

/// "in about 2 hours", for a moment in the future.
pub fn time_until(then: u64, now: u64) -> String {
    let minutes = then.saturating_sub(now).div_ceil(60);
    match minutes {
        0 | 1 => "in about a minute".to_string(),
        2..=89 => format!("in about {minutes} minutes"),
        _ => {
            let hours = (minutes + 30) / 60;
            format!("in about {hours} hours")
        }
    }
}

impl Unavailable {
    /// When to try the subscription again.
    fn retry_at(&self, now: u64) -> u64 {
        match self {
            Unavailable::Limited { resets_at } => resets_at
                .filter(|at| *at > now)
                .unwrap_or(now + RETRY_LIMITED),
            Unavailable::NotInstalled => now + RETRY_NOT_INSTALLED,
            Unavailable::Failing(_) => now + RETRY_FAILING,
        }
    }

    /// What to tell the user when the API key takes over.
    fn handover_notice(&self, now: u64) -> Option<String> {
        match self {
            Unavailable::Limited { resets_at } => {
                let back = match resets_at.filter(|at| *at > now) {
                    Some(at) => format!(" Back on the subscription {}.", time_until(at, now)),
                    None => String::new(),
                };
                Some(format!(
                    "Your Claude subscription has reached its limit, so this uses your API key.{back}"
                ))
            }
            // Someone with only an API key chose that. No need to say so.
            Unavailable::NotInstalled => None,
            Unavailable::Failing(why) => Some(format!(
                "The claude command failed, so this uses your API key. It said: {why}"
            )),
        }
    }

    /// What to tell the user when there is no API key to take over.
    fn dead_end(&self, now: u64, api_allowed: bool) -> String {
        let add_key = if api_allowed {
            " To keep going until then, add an API key with `scuttle assistant set-key`."
        } else {
            ""
        };
        match self {
            Unavailable::Limited { resets_at } => {
                let when = match resets_at.filter(|at| *at > now) {
                    Some(at) => format!(" It resets {}.", time_until(at, now)),
                    None => String::new(),
                };
                format!("Your Claude subscription has reached its limit.{when}{add_key}")
            }
            Unavailable::NotInstalled if api_allowed => {
                "The assistant needs a way to reach Claude. To use your Claude subscription, \
                 install Claude Code and sign in by running `claude` in a terminal. To use an \
                 API key, run `scuttle assistant set-key`."
                    .to_string()
            }
            Unavailable::NotInstalled => {
                "The claude command is not installed. Install Claude Code and sign in by \
                 running `claude` in a terminal."
                    .to_string()
            }
            Unavailable::Failing(why) => why.clone(),
        }
    }
}

/// One message on its way to an answer. Owns everything it needs, so
/// it can be moved to a worker thread.
#[derive(Debug)]
pub struct Job {
    generation: u64,
    settings: Settings,
    system: String,
    /// Whether the subscription is worth trying first.
    subscription_first: bool,
    cli_prompt: String,
    resume: Option<String>,
    turns: Vec<Turn>,
    cancel: Cancel,
}

impl Job {
    /// Which chat this job belongs to. Events from a job whose chat
    /// has since been cleared should be ignored.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn report(&self) -> Report {
        Report {
            generation: self.generation,
            reply: String::new(),
            backend: None,
            session: None,
            subscription: None,
            error: None,
            stopped: false,
        }
    }

    /// Gets the answer, calling `emit` as it arrives. Blocks until the
    /// answer is complete, so call it off the main thread.
    pub fn run(self, now: u64, mut emit: impl FnMut(Event)) -> Report {
        let mut report = self.report();
        let api_allowed = self.settings.prefer != Prefer::Subscription;
        let subscription_allowed = self.settings.prefer != Prefer::ApiKey;
        let key = if api_allowed {
            self.settings.keyring.anthropic_key()
        } else {
            None
        };

        // With no key to fall back on, the subscription is tried even
        // inside its cooling-off period: it is the only option.
        if subscription_allowed && (self.subscription_first || key.is_none()) {
            emit(Event::Started(Backend::Subscription));
            let outcome = cli::run(
                &cli::Invocation {
                    program: &self.settings.claude_program,
                    model: &self.settings.subscription_model,
                    system_prompt: &self.system,
                    resume: self.resume.as_deref(),
                    prompt: &self.cli_prompt,
                    dir: self.settings.claude_dir.as_deref(),
                },
                &self.cancel,
                |piece| {
                    report.reply.push_str(piece);
                    emit(Event::Delta(piece.to_string()));
                },
            );
            let said_something = !report.reply.is_empty();
            match outcome {
                cli::Outcome::Answered { session } => {
                    report.backend = Some(Backend::Subscription);
                    report.session = session;
                    return report;
                }
                cli::Outcome::Stopped => {
                    report.backend = said_something.then_some(Backend::Subscription);
                    report.stopped = true;
                    return report;
                }
                cli::Outcome::Failed(why) if said_something => {
                    // Half an answer is on screen. Starting again on the
                    // other backend would be more confusing than saying so.
                    report.backend = Some(Backend::Subscription);
                    report.error = Some(why);
                    return report;
                }
                cli::Outcome::Limited { resets_at } => {
                    report.subscription = Some(Unavailable::Limited { resets_at });
                }
                cli::Outcome::NotInstalled => {
                    report.subscription = Some(Unavailable::NotInstalled);
                }
                cli::Outcome::Failed(why) => {
                    report.subscription = Some(Unavailable::Failing(why));
                }
            }
        }

        let Some(key) = key else {
            report.error = Some(match &report.subscription {
                Some(problem) => problem.dead_end(now, api_allowed),
                None => "No API key is set. Add one with `scuttle assistant set-key`.".to_string(),
            });
            return report;
        };
        if let Some(notice) = report
            .subscription
            .as_ref()
            .and_then(|p| p.handover_notice(now))
        {
            emit(Event::Notice(notice));
        }
        emit(Event::Started(Backend::ApiKey));
        let cancel = self.cancel.clone();
        let outcome = api::run(
            &self.settings.api_base,
            &key,
            &self.settings.api_model,
            &self.system,
            &self.turns,
            |piece| {
                report.reply.push_str(piece);
                emit(Event::Delta(piece.to_string()));
                !cancel.is_stopped()
            },
        );
        if !report.reply.is_empty() {
            report.backend = Some(Backend::ApiKey);
        }
        match outcome {
            api::Outcome::Answered => {}
            api::Outcome::CutShort => {
                emit(Event::Notice(
                    "That answer hit the length limit and was cut short.".to_string(),
                ));
            }
            api::Outcome::Refused => {
                report.error = Some("Claude declined to answer that.".to_string());
            }
            api::Outcome::Stopped => report.stopped = true,
            api::Outcome::Failed(why) => report.error = Some(why),
        }
        if self.cancel.is_stopped() {
            report.stopped = true;
            report.error = None;
        }
        report
    }
}

/// The assistant: its settings, the conversation, and what it knows
/// about the subscription.
#[derive(Debug, Default)]
pub struct Assistant {
    settings: Settings,
    conversation: Conversation,
    generation: u64,
    /// Leave the subscription alone until this moment.
    subscription_back_at: Option<u64>,
    /// Things that happened since the last message which Claude should
    /// know about, such as the output of a command the user ran.
    notes: Vec<String>,
    /// The handle for the answer in progress.
    running: Option<Cancel>,
}

impl Assistant {
    pub fn new(settings: Settings) -> Self {
        Assistant {
            settings,
            ..Default::default()
        }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Applies new settings. The conversation carries on.
    pub fn set_settings(&mut self, settings: Settings) {
        if settings != self.settings {
            // A different program or preference may well work now.
            self.subscription_back_at = None;
            self.settings = settings;
        }
    }

    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    /// Whether an answer is in progress.
    pub fn is_busy(&self) -> bool {
        self.running.is_some()
    }

    /// The model the next message will be answered by, as people say it.
    pub fn next_model(&self, now: u64) -> String {
        pretty_model(match self.next_backend(now) {
            Backend::Subscription => &self.settings.subscription_model,
            Backend::ApiKey => &self.settings.api_model,
        })
    }

    /// Whether there are notes waiting to be sent.
    pub fn has_notes(&self) -> bool {
        !self.notes.is_empty()
    }

    /// The backend the next message will try first.
    pub fn next_backend(&self, now: u64) -> Backend {
        match self.settings.prefer {
            Prefer::Subscription => Backend::Subscription,
            Prefer::ApiKey => Backend::ApiKey,
            Prefer::Auto if self.subscription_back_at.is_some_and(|at| now < at) => Backend::ApiKey,
            Prefer::Auto => Backend::Subscription,
        }
    }

    /// Remembers something for Claude to be told with the next message.
    pub fn note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    /// Records the user's message and prepares the job that answers it.
    pub fn begin(&mut self, message: &str, now: u64) -> Job {
        let mut text = String::new();
        for note in self.notes.drain(..) {
            text.push_str(&format!("[Scuttle: {note}]\n\n"));
        }
        text.push_str(message.trim());
        self.conversation.push(Role::User, text.trim_end());

        let cancel = Cancel::default();
        self.running = Some(cancel.clone());
        Job {
            generation: self.generation,
            settings: self.settings.clone(),
            system: crate::prompt::system_prompt(),
            subscription_first: self.next_backend(now) == Backend::Subscription,
            cli_prompt: self.conversation.cli_prompt(),
            resume: self.conversation.resumable_session().map(str::to_string),
            turns: self.conversation.turns().to_vec(),
            cancel,
        }
    }

    /// Records how a job went. Returns false when the report belongs
    /// to a chat that has since been cleared, in which case nothing
    /// changes.
    pub fn finish(&mut self, report: &Report, now: u64) -> bool {
        if report.generation != self.generation {
            return false;
        }
        self.running = None;
        if let Some(problem) = &report.subscription {
            self.subscription_back_at = Some(problem.retry_at(now));
        } else if report.backend == Some(Backend::Subscription) && report.error.is_none() {
            self.subscription_back_at = None;
        }
        if report.reply.trim().is_empty() {
            // Nothing came back, so the message is not left hanging in
            // the transcript without an answer.
            self.conversation.drop_unanswered();
            return true;
        }
        self.conversation
            .push(Role::Assistant, report.reply.clone());
        let complete = !report.stopped && report.error.is_none();
        match (&report.session, report.backend) {
            (Some(session), Some(Backend::Subscription)) if complete => {
                self.conversation.synced_with(session);
            }
            _ => {}
        }
        true
    }

    /// Stops the answer in progress. The job still finishes, with
    /// whatever had been said so far.
    pub fn stop(&mut self) {
        if let Some(cancel) = &self.running {
            cancel.stop();
        }
    }

    /// Starts a new chat, dropping the conversation.
    pub fn new_chat(&mut self) {
        self.stop();
        self.running = None;
        self.generation += 1;
        self.conversation = Conversation::default();
        self.notes.clear();
    }

    /// The current chat's generation, to match against [`Job::generation`].
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::testing::{error, stream, FakeApi};
    use crate::cli::testing::{calls, delta, fake, INIT, REJECTED, RESULT_LIMIT, RESULT_OK};
    use crate::key::testing::fake_keyring;
    use crate::key::ANTHROPIC;

    const NOW: u64 = 1_794_420_000;
    const RESET: u64 = 1_794_424_200; // 70 minutes after NOW

    fn settings(name: &str, claude: &str, api: &FakeApi, key: Option<&str>) -> Settings {
        let keyring = fake_keyring(name);
        if let Some(key) = key {
            keyring.store(ANTHROPIC, key).unwrap();
        }
        Settings {
            claude_program: claude.to_string(),
            api_base: api.base.clone(),
            keyring,
            ..Settings::default()
        }
    }

    fn no_api() -> FakeApi {
        FakeApi::start(vec![])
    }

    /// Runs one message through and returns the report and the events.
    fn ask(assistant: &mut Assistant, message: &str, now: u64) -> (Report, Vec<Event>) {
        let job = assistant.begin(message, now);
        let mut events = Vec::new();
        let report = job.run(now, |e| events.push(e));
        assert!(assistant.finish(&report, now));
        (report, events)
    }

    fn started(events: &[Event]) -> Vec<Backend> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Started(b) => Some(*b),
                _ => None,
            })
            .collect()
    }

    fn notices(events: &[Event]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Notice(n) => Some(n.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_defaults_are_opus_on_the_subscription_and_haiku_on_the_key() {
        let settings = Settings::default();
        assert_eq!(settings.prefer, Prefer::Auto);
        assert_eq!(settings.subscription_model, "claude-opus-5-5");
        assert_eq!(settings.api_model, "claude-haiku-5-5");
    }

    #[test]
    fn the_subscription_answers_when_it_can() {
        let claude = fake(
            "sub-ok",
            &[INIT, &delta("It is "), &delta("the fan."), RESULT_OK],
            0,
        );
        let api = no_api();
        let mut assistant = Assistant::new(settings("sub-ok", &claude, &api, Some("key")));
        let (report, events) = ask(&mut assistant, "what is loud", NOW);

        assert_eq!(report.reply, "It is the fan.");
        assert_eq!(report.backend, Some(Backend::Subscription));
        assert_eq!(report.error, None);
        assert_eq!(started(&events), [Backend::Subscription]);
        assert!(notices(&events).is_empty());
        let args = std::fs::read_to_string(format!("{claude}.args")).unwrap();
        assert!(args.contains("--model claude-opus-5-5"));
        assert_eq!(assistant.conversation().turns().len(), 2);
        assert!(!assistant.is_busy());
    }

    #[test]
    fn a_second_message_resumes_the_session_and_sends_only_itself() {
        let claude = fake("resume", &[INIT, &delta("ok"), RESULT_OK], 0);
        let api = no_api();
        let mut assistant = Assistant::new(settings("resume", &claude, &api, None));
        ask(&mut assistant, "first", NOW);
        ask(&mut assistant, "second", NOW + 5);
        let args = std::fs::read_to_string(format!("{claude}.args")).unwrap();
        assert!(
            args.contains("--resume db00ecbf-aede-497b-8340-707208b454bd"),
            "{args}"
        );
        assert_eq!(
            std::fs::read_to_string(format!("{claude}.stdin")).unwrap(),
            "second"
        );
    }

    #[test]
    fn a_reached_limit_hands_over_to_the_api_key_and_says_so() {
        let claude = fake("limit", &[INIT, REJECTED, RESULT_LIMIT], 1);
        let api = FakeApi::start(vec![stream(&["From the key."], "end_turn")]);
        let mut assistant = Assistant::new(settings("limit", &claude, &api, Some("sk-test")));
        let (report, events) = ask(&mut assistant, "hello", NOW);

        assert_eq!(report.reply, "From the key.");
        assert_eq!(report.backend, Some(Backend::ApiKey));
        assert_eq!(
            report.subscription,
            Some(Unavailable::Limited {
                resets_at: Some(RESET)
            })
        );
        assert_eq!(started(&events), [Backend::Subscription, Backend::ApiKey]);
        let notice = notices(&events)[0];
        assert!(notice.contains("reached its limit"), "{notice}");
        assert!(notice.contains("in about 70 minutes"), "{notice}");

        let request = api.request();
        assert!(request.to_lowercase().contains("x-api-key: sk-test"));
        assert!(request.contains("\"model\":\"claude-haiku-5-5\""));
        assert!(request.contains("hello"));
    }

    #[test]
    fn while_limited_the_subscription_is_left_alone_then_tried_again_after_the_reset() {
        let claude = fake("cooldown", &[INIT, REJECTED, RESULT_LIMIT], 1);
        let api = FakeApi::start(vec![
            stream(&["one"], "end_turn"),
            stream(&["two"], "end_turn"),
            stream(&["three"], "end_turn"),
        ]);
        let mut assistant = Assistant::new(settings("cooldown", &claude, &api, Some("k")));
        ask(&mut assistant, "a", NOW);
        assert_eq!(calls(&claude), 1);
        assert_eq!(assistant.next_backend(NOW + 60), Backend::ApiKey);

        // Still inside the limit: straight to the key, no notice.
        let (_, events) = ask(&mut assistant, "b", NOW + 60);
        assert_eq!(calls(&claude), 1);
        assert_eq!(started(&events), [Backend::ApiKey]);
        assert!(notices(&events).is_empty());

        // After the reset the subscription gets another go.
        assert_eq!(assistant.next_backend(RESET + 1), Backend::Subscription);
        ask(&mut assistant, "c", RESET + 1);
        assert_eq!(calls(&claude), 2);
        // Its session never saw the earlier turns, so they are sent along.
        let sent = std::fs::read_to_string(format!("{claude}.stdin")).unwrap();
        assert!(
            sent.contains("User: a") && sent.contains("You: one") && sent.ends_with("c"),
            "{sent}"
        );
        assert!(!std::fs::read_to_string(format!("{claude}.args"))
            .unwrap()
            .contains("--resume"));
    }

    #[test]
    fn a_working_subscription_clears_the_cooling_off() {
        let limited = fake("recover-a", &[INIT, REJECTED, RESULT_LIMIT], 1);
        let working = fake("recover-b", &[INIT, &delta("back"), RESULT_OK], 0);
        let api = FakeApi::start(vec![stream(&["key"], "end_turn")]);
        let mut first = settings("recover", &limited, &api, Some("k"));
        let mut assistant = Assistant::new(first.clone());
        ask(&mut assistant, "a", NOW);
        assert_eq!(assistant.next_backend(NOW + 1), Backend::ApiKey);
        // Changing the settings gives the subscription a fresh start.
        first.claude_program = working;
        assistant.set_settings(first);
        let (report, _) = ask(&mut assistant, "b", NOW + 2);
        assert_eq!(report.backend, Some(Backend::Subscription));
        assert_eq!(assistant.next_backend(NOW + 3), Backend::Subscription);
    }

    #[test]
    fn a_limit_with_no_key_explains_what_to_do_and_forgets_the_message() {
        let claude = fake("limit-nokey", &[INIT, REJECTED, RESULT_LIMIT], 1);
        let api = no_api();
        let mut assistant = Assistant::new(settings("limit-nokey", &claude, &api, None));
        let (report, events) = ask(&mut assistant, "hello", NOW);
        let error = report.error.unwrap();
        assert!(error.contains("reached its limit"), "{error}");
        assert!(error.contains("resets in about 70 minutes"), "{error}");
        assert!(error.contains("scuttle assistant set-key"), "{error}");
        assert_eq!(started(&events), [Backend::Subscription]);
        assert!(assistant.conversation().is_empty());

        // With no key, the subscription is still tried next time.
        ask(&mut assistant, "again", NOW + 30);
        assert_eq!(calls(&claude), 2);
    }

    #[test]
    fn with_no_claude_command_the_key_answers_quietly() {
        let api = FakeApi::start(vec![stream(&["hi"], "end_turn")]);
        let mut assistant =
            Assistant::new(settings("noclaude", "/nonexistent/claude", &api, Some("k")));
        let (report, events) = ask(&mut assistant, "hello", NOW);
        assert_eq!(report.backend, Some(Backend::ApiKey));
        assert_eq!(report.subscription, Some(Unavailable::NotInstalled));
        assert!(notices(&events).is_empty());
        assert_eq!(assistant.next_backend(NOW + 60), Backend::ApiKey);
    }

    #[test]
    fn with_neither_backend_the_error_explains_both_ways_in() {
        let api = no_api();
        let mut assistant = Assistant::new(settings("neither", "/nonexistent/claude", &api, None));
        let (report, _) = ask(&mut assistant, "hello", NOW);
        let error = report.error.unwrap();
        assert!(error.contains("install Claude Code"), "{error}");
        assert!(error.contains("scuttle assistant set-key"), "{error}");
    }

    #[test]
    fn a_claude_command_that_fails_silently_hands_over_and_passes_on_why() {
        let line =
            r#"{"type":"result","is_error":true,"result":"Invalid API key · Please run /login"}"#;
        let claude = fake("failing", &[line], 1);
        let api = FakeApi::start(vec![stream(&["hi"], "end_turn")]);
        let mut assistant = Assistant::new(settings("failing", &claude, &api, Some("k")));
        let (report, events) = ask(&mut assistant, "hello", NOW);
        assert_eq!(report.backend, Some(Backend::ApiKey));
        assert!(notices(&events)[0].contains("Please run /login"));
    }

    #[test]
    fn a_failure_after_half_an_answer_does_not_start_again_on_the_key() {
        let line = r#"{"type":"result","is_error":true,"result":"Connection lost"}"#;
        let claude = fake("midway", &[INIT, &delta("Half an "), line], 1);
        let api = FakeApi::start(vec![stream(&["should not be used"], "end_turn")]);
        let mut assistant = Assistant::new(settings("midway", &claude, &api, Some("k")));
        let (report, events) = ask(&mut assistant, "hello", NOW);
        assert_eq!(report.reply, "Half an ");
        assert_eq!(report.error.as_deref(), Some("Connection lost"));
        assert_eq!(started(&events), [Backend::Subscription]);
        // The half answer stays in the transcript, but the session is
        // not trusted to hold it.
        assert_eq!(assistant.conversation().turns().len(), 2);
        assert_eq!(assistant.conversation().resumable_session(), None);
    }

    #[test]
    fn api_only_never_runs_the_claude_command() {
        let claude = fake("api-only", &[INIT, &delta("no"), RESULT_OK], 0);
        let api = FakeApi::start(vec![stream(&["yes"], "end_turn")]);
        let mut s = settings("api-only", &claude, &api, Some("k"));
        s.prefer = Prefer::ApiKey;
        let mut assistant = Assistant::new(s);
        let (report, events) = ask(&mut assistant, "hello", NOW);
        assert_eq!(report.reply, "yes");
        assert_eq!(started(&events), [Backend::ApiKey]);
        assert_eq!(calls(&claude), 0);
    }

    #[test]
    fn api_only_with_no_key_says_to_set_one() {
        let api = no_api();
        let mut s = settings("api-nokey", "/nonexistent/claude", &api, None);
        s.prefer = Prefer::ApiKey;
        let mut assistant = Assistant::new(s);
        let (report, events) = ask(&mut assistant, "hello", NOW);
        assert!(report.error.unwrap().contains("scuttle assistant set-key"));
        assert!(started(&events).is_empty());
    }

    #[test]
    fn subscription_only_never_touches_the_key() {
        let claude = fake("sub-only", &[INIT, REJECTED, RESULT_LIMIT], 1);
        let api = FakeApi::start(vec![stream(&["should not be used"], "end_turn")]);
        let mut s = settings("sub-only", &claude, &api, Some("k"));
        s.prefer = Prefer::Subscription;
        let mut assistant = Assistant::new(s);
        let (report, events) = ask(&mut assistant, "hello", NOW);
        let error = report.error.unwrap();
        assert!(error.contains("reached its limit"));
        assert!(!error.contains("set-key"), "{error}");
        assert_eq!(started(&events), [Backend::Subscription]);
        assert_eq!(assistant.next_backend(NOW + 1), Backend::Subscription);
    }

    #[test]
    fn an_api_failure_is_reported_in_plain_words() {
        let api = FakeApi::start(vec![error(
            401,
            "authentication_error",
            "invalid x-api-key",
        )]);
        let mut assistant = Assistant::new(settings(
            "api-401",
            "/nonexistent/claude",
            &api,
            Some("bad"),
        ));
        let (report, _) = ask(&mut assistant, "hello", NOW);
        assert!(report.error.unwrap().contains("was not accepted"));
        assert!(assistant.conversation().is_empty());
    }

    #[test]
    fn a_refusal_and_a_cut_off_answer_are_both_reported() {
        let api = FakeApi::start(vec![
            stream(&[], "refusal"),
            stream(&["half"], "max_tokens"),
        ]);
        let mut assistant =
            Assistant::new(settings("stops", "/nonexistent/claude", &api, Some("k")));
        let (report, _) = ask(&mut assistant, "one", NOW);
        assert_eq!(
            report.error.as_deref(),
            Some("Claude declined to answer that.")
        );
        let (report, events) = ask(&mut assistant, "two", NOW);
        assert_eq!(report.reply, "half");
        assert!(notices(&events)[0].contains("cut short"));
    }

    #[test]
    fn stopping_keeps_what_was_said_so_far() {
        let claude = fake(
            "stopped",
            &[INIT, &delta("So far"), "sleep 30", RESULT_OK],
            0,
        );
        let api = no_api();
        let mut assistant = Assistant::new(settings("stopped", &claude, &api, None));
        let job = assistant.begin("hello", NOW);
        assert!(assistant.is_busy());
        let worker = std::thread::spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let report = job.run(NOW, |e| tx.send(e).unwrap());
            (report, rx.try_iter().collect::<Vec<_>>())
        });
        // Give the answer time to start, then stop it.
        std::thread::sleep(std::time::Duration::from_millis(400));
        assistant.stop();
        let (report, _) = worker.join().unwrap();
        assert!(report.stopped);
        assert_eq!(report.reply, "So far");
        assert!(assistant.finish(&report, NOW));
        assert_eq!(assistant.conversation().turns().len(), 2);
        assert_eq!(assistant.conversation().resumable_session(), None);
        assert!(!assistant.is_busy());
    }

    #[test]
    fn a_report_from_a_cleared_chat_is_ignored() {
        let claude = fake("stale", &[INIT, &delta("old answer"), RESULT_OK], 0);
        let api = no_api();
        let mut assistant = Assistant::new(settings("stale", &claude, &api, None));
        let job = assistant.begin("old question", NOW);
        assert_eq!(job.generation(), assistant.generation());
        assistant.new_chat();
        assert_ne!(job.generation(), assistant.generation());
        let report = job.run(NOW, |_| {});
        assert!(!assistant.finish(&report, NOW));
        assert!(assistant.conversation().is_empty());
    }

    #[test]
    fn notes_ride_along_with_the_next_message_only() {
        let claude = fake("notes", &[INIT, &delta("ok"), RESULT_OK], 0);
        let api = no_api();
        let mut assistant = Assistant::new(settings("notes", &claude, &api, None));
        assistant.note("the user ran `uptime`. It printed: up 3 days");
        ask(&mut assistant, "what does that mean", NOW);
        let sent = std::fs::read_to_string(format!("{claude}.stdin")).unwrap();
        assert!(sent.starts_with("[Scuttle: the user ran `uptime`. It printed: up 3 days]"));
        assert!(sent.ends_with("what does that mean"));
        ask(&mut assistant, "thanks", NOW);
        assert_eq!(
            std::fs::read_to_string(format!("{claude}.stdin")).unwrap(),
            "thanks"
        );
    }

    #[test]
    fn notes_can_be_sent_without_a_message() {
        let claude = fake("followup", &[INIT, &delta("ok"), RESULT_OK], 0);
        let api = no_api();
        let mut assistant = Assistant::new(settings("followup", &claude, &api, None));
        assert!(!assistant.has_notes());
        assistant.note("the user ran `true`. It exited with status 0.");
        assert!(assistant.has_notes());
        ask(&mut assistant, "", NOW);
        assert_eq!(
            std::fs::read_to_string(format!("{claude}.stdin")).unwrap(),
            "[Scuttle: the user ran `true`. It exited with status 0.]"
        );
    }

    #[test]
    fn model_ids_read_as_names() {
        assert_eq!(pretty_model("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(pretty_model("claude-haiku-5-5"), "Haiku 5.5");
        assert_eq!(pretty_model("claude-fable-5-2"), "Fable 5.2");
        assert_eq!(pretty_model("claude-sonnet-5"), "Sonnet 5");
        assert_eq!(pretty_model("claude-haiku-4-5-20251001"), "Haiku 4.5");
        assert_eq!(pretty_model("opus"), "Opus");
        assert_eq!(pretty_model("12345"), "12345");
        let assistant = Assistant::default();
        assert_eq!(assistant.next_model(NOW), "Opus 5.5");
    }

    #[test]
    fn time_until_reads_naturally() {
        assert_eq!(time_until(NOW + 20, NOW), "in about a minute");
        assert_eq!(time_until(NOW + 35 * 60, NOW), "in about 35 minutes");
        assert_eq!(time_until(NOW + 2 * 3600, NOW), "in about 2 hours");
        assert_eq!(time_until(NOW + 100 * 60, NOW), "in about 2 hours");
        assert_eq!(time_until(NOW - 5, NOW), "in about a minute");
    }
}
