//! The assistant panel: a column at the side of the screen for asking
//! Claude things and having it act on the desktop.
//!
//! The thinking happens in `scuttle-assistant`. This file draws the
//! conversation, runs the actions Claude asks for, and keeps the slow
//! parts (waiting for an answer, running a command) on other threads.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_channel::Sender;
use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use scuttle_assistant::command::{self, Outcome};
use scuttle_assistant::markdown::{self, Block};
use scuttle_assistant::{
    segments, Action, Assistant, Backend, Event, Prefer, Report, Segment, Settings,
};
use scuttle_core::config::{AssistantBackend, Config, Side};
use scuttle_sound::{Feedback, Sound};

use crate::msg::{DesktopState, Msg};
use crate::scale::px;

/// How long an approved command may run before it is stopped.
const COMMAND_LIMIT: Duration = Duration::from_secs(60);

/// Lines of a command's output shown in its card. Claude gets more.
const OUTPUT_LINES: usize = 14;

/// Things to try, shown in an empty chat.
const SUGGESTIONS: [&str; 4] = [
    "Why is my fan loud?",
    "What is using my memory?",
    "Turn the volume down a bit",
    "How much disk space is left?",
];

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The assistant's settings, from the config file.
pub fn settings(config: &Config) -> Settings {
    Settings {
        prefer: match config.assistant.backend {
            AssistantBackend::Auto => Prefer::Auto,
            AssistantBackend::Subscription => Prefer::Subscription,
            AssistantBackend::Api => Prefer::ApiKey,
        },
        subscription_model: config.assistant.subscription_model.clone(),
        api_model: config.assistant.api_model.clone(),
        api_base: api_base(std::env::var("ANTHROPIC_BASE_URL").ok().as_deref()),
        claude_dir: Some(scuttle_core::paths::cache_dir().join("assistant")),
        ..Settings::default()
    }
}

/// Where the API lives. `ANTHROPIC_BASE_URL` overrides it, as it does
/// for Anthropic's own tools, for people who go through a gateway.
pub fn api_base(from_env: Option<&str>) -> String {
    match from_env.map(str::trim) {
        Some(url) if url.starts_with("https://") || url.starts_with("http://") => url.to_string(),
        _ => scuttle_assistant::api::DEFAULT_BASE.to_string(),
    }
}

/// The font family list for inline code: the configured font, then
/// the same fallbacks the stylesheet uses.
pub fn mono_family(configured: &str) -> String {
    let fallbacks = "JetBrains Mono, DejaVu Sans Mono, monospace";
    // A comma would start a new family, so the name cannot contain one.
    let name: String = configured
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .collect();
    match name.trim() {
        "" => fallbacks.to_string(),
        name => format!("{name}, {fallbacks}"),
    }
}

/// What the header chip says: the model and how it is being reached.
pub fn chip_text(model: &str, backend: Backend) -> String {
    let via = match backend {
        Backend::Subscription => "subscription",
        Backend::ApiKey => "API key",
    };
    format!("{model} · {via}")
}

/// What a desktop action is called once it has run.
pub fn describe_desktop(args: &[String]) -> String {
    let text = args.join(" ");
    let known = scuttle_assistant::actions::DESKTOP_COMMANDS
        .iter()
        .find(|(command, _)| *command == text)
        .map(|(_, meaning)| *meaning);
    match known {
        // "raise the volume a step" reads better as "Raise the volume a step".
        Some(meaning) => {
            let mut letters = meaning.chars();
            match letters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + letters.as_str(),
                None => text,
            }
        }
        None => format!("scuttle {text}"),
    }
}

/// The first lines of a command's output, for its card.
pub fn output_preview(output: &str, lines: usize) -> String {
    let all: Vec<&str> = output.trim_end().lines().collect();
    if all.len() <= lines {
        return all.join("\n");
    }
    format!(
        "{}\n… {} more lines",
        all[..lines].join("\n"),
        all.len() - lines
    )
}

/// What came from background threads for the panel.
pub enum AssistantMsg {
    Event(Event),
    Finished(Report),
    /// An approved command finished.
    Ran {
        answer: usize,
        action: usize,
        outcome: Outcome,
    },
}

#[derive(Clone)]
enum ShellState {
    Waiting,
    Running,
    Done(Outcome),
    Skipped,
}

/// An action in an answer, and what became of it.
#[derive(Clone)]
enum ActionState {
    Desktop {
        label: String,
        result: Result<(), String>,
    },
    Shell {
        command: String,
        why: String,
        state: ShellState,
    },
    Rejected(String),
}

/// One answer from the assistant, as shown.
struct Answer {
    root: gtk::Box,
    text: String,
    /// The actions found in the text so far, in order. An action is
    /// added once, the first time it appears, so re-drawing the answer
    /// never runs anything twice.
    actions: Vec<ActionState>,
    error: Option<String>,
    /// Waiting for the first words.
    thinking: bool,
}

type DesktopRunner = Box<dyn Fn(&[String]) -> Result<(), String>>;

struct Inner {
    window: gtk::ApplicationWindow,
    root: gtk::Box,
    chip: gtk::Label,
    scroller: gtk::ScrolledWindow,
    messages: gtk::Box,
    input: gtk::TextView,
    placeholder: gtk::Label,
    send: gtk::Button,

    engine: RefCell<Assistant>,
    answers: RefCell<Vec<Answer>>,
    /// The answer being streamed, as an index into `answers`.
    live: Cell<Option<usize>>,
    render_pending: Cell<bool>,
    open: Cell<bool>,
    /// Keep the newest message in view while the reader is at the bottom.
    stick: Cell<bool>,
    exit_ms: Cell<u32>,
    hide_timer: RefCell<Option<glib::SourceId>>,
    mono: RefCell<String>,
    focused_monitor: RefCell<Option<String>>,
    feedback: Rc<Feedback>,
    tx: Sender<Msg>,
    run_desktop: RefCell<Option<DesktopRunner>>,
}

/// The assistant panel. Cheap to clone; clones share one surface.
#[derive(Clone)]
pub struct Panel(Rc<Inner>);

fn text_label(markup: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_markup(markup);
    label.set_wrap(true);
    label.set_wrap_mode(pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label.set_hexpand(true);
    // A small natural width, so a long line wraps instead of making
    // the panel wider.
    label.set_width_chars(12);
    label.set_max_width_chars(12);
    label.set_selectable(true);
    label.set_focus_on_click(false);
    label
}

fn plain_label(text: &str, class: &str) -> gtk::Label {
    let label = text_label(&markdown::escape(text));
    label.add_css_class(class);
    label
}

impl Panel {
    pub fn new(
        app: &gtk::Application,
        config: &Config,
        feedback: Rc<Feedback>,
        tx: Sender<Msg>,
    ) -> Panel {
        let window = gtk::ApplicationWindow::new(app);
        window.add_css_class("scuttle");
        window.init_layer_shell();
        window.set_namespace(Some("scuttle-assistant"));
        // Above windows, below the launcher and notifications.
        window.set_layer(Layer::Top);
        window.set_keyboard_mode(KeyboardMode::OnDemand);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("assistant");
        root.set_overflow(gtk::Overflow::Hidden);
        window.set_child(Some(&root));

        // Header.
        let head = gtk::Box::new(gtk::Orientation::Horizontal, px(10));
        head.add_css_class("assistant-head");
        let title = gtk::Label::new(Some("Assistant"));
        title.add_css_class("assistant-title");
        let chip = gtk::Label::new(None);
        chip.add_css_class("chip");
        chip.set_valign(gtk::Align::Center);
        chip.set_ellipsize(pango::EllipsizeMode::End);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let new_chat = gtk::Button::with_label("New chat");
        new_chat.add_css_class("quiet");
        new_chat.add_css_class("small");
        new_chat.set_valign(gtk::Align::Center);
        new_chat.set_tooltip_text(Some("Start again with an empty chat"));
        let close = gtk::Button::with_label("✕");
        close.add_css_class("icon");
        close.set_valign(gtk::Align::Center);
        close.set_tooltip_text(Some("Close (Esc)"));
        head.append(&title);
        head.append(&chip);
        head.append(&spacer);
        head.append(&new_chat);
        head.append(&close);
        root.append(&head);

        // Conversation.
        let messages = gtk::Box::new(gtk::Orientation::Vertical, px(14));
        messages.add_css_class("msgs");
        messages.set_valign(gtk::Align::End);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&messages));
        root.append(&scroller);

        // Composer.
        let composer = gtk::Box::new(gtk::Orientation::Horizontal, px(10));
        composer.add_css_class("composer");
        let input = gtk::TextView::new();
        input.set_wrap_mode(gtk::WrapMode::WordChar);
        input.set_accepts_tab(false);
        input.set_hexpand(true);
        let input_scroller = gtk::ScrolledWindow::new();
        input_scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        // Scrolls when the message is long, without a scrollbar that
        // would make an empty composer taller than one line.
        input_scroller.set_vscrollbar_policy(gtk::PolicyType::External);
        input_scroller.set_propagate_natural_height(true);
        input_scroller.set_max_content_height(px(150));
        input_scroller.set_child(Some(&input));
        input_scroller.set_valign(gtk::Align::Center);
        let placeholder = gtk::Label::new(Some("Ask, or say what to change"));
        placeholder.add_css_class("composer-placeholder");
        placeholder.set_halign(gtk::Align::Start);
        placeholder.set_valign(gtk::Align::Center);
        placeholder.set_can_target(false);
        let input_stack = gtk::Overlay::new();
        input_stack.set_hexpand(true);
        input_stack.set_child(Some(&input_scroller));
        input_stack.add_overlay(&placeholder);
        let send = gtk::Button::with_label("↑");
        send.add_css_class("send");
        send.set_valign(gtk::Align::End);
        send.set_sensitive(false);
        composer.append(&input_stack);
        composer.append(&send);
        root.append(&composer);

        let panel = Panel(Rc::new(Inner {
            window,
            root,
            chip,
            scroller,
            messages,
            input,
            placeholder,
            send,
            engine: RefCell::new(Assistant::new(settings(config))),
            answers: RefCell::new(Vec::new()),
            live: Cell::new(None),
            render_pending: Cell::new(false),
            open: Cell::new(false),
            stick: Cell::new(true),
            exit_ms: Cell::new(0),
            hide_timer: RefCell::new(None),
            mono: RefCell::new(String::new()),
            focused_monitor: RefCell::new(None),
            feedback,
            tx,
            run_desktop: RefCell::new(None),
        }));
        panel.connect(&new_chat, &close);
        panel.apply_config(config, false);
        panel.show_empty_state();
        panel
    }

    fn connect(&self, new_chat: &gtk::Button, close: &gtk::Button) {
        let inner = &self.0;
        let weak = Rc::downgrade(inner);
        let with = move |weak: &Weak<Inner>, f: &dyn Fn(Panel)| {
            if let Some(inner) = weak.upgrade() {
                f(Panel(inner));
            }
        };

        new_chat.connect_clicked({
            let weak = weak.clone();
            move |_| with(&weak, &|panel| panel.new_chat())
        });
        close.connect_clicked({
            let weak = weak.clone();
            move |_| with(&weak, &|panel| panel.hide())
        });
        inner.send.connect_clicked({
            let weak = weak.clone();
            move |_| with(&weak, &|panel| panel.send_or_stop())
        });

        // Escape closes the panel from anywhere in it.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let weak = weak.clone();
            move |_, key, _, modifiers| {
                let Some(inner) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let panel = Panel(inner);
                let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
                let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
                match key {
                    gdk::Key::Escape => panel.hide(),
                    // Ctrl+Enter runs the command that is waiting, so
                    // approving one never needs the mouse.
                    gdk::Key::Return | gdk::Key::KP_Enter if ctrl => {
                        if !panel.run_waiting_command() {
                            return glib::Propagation::Proceed;
                        }
                    }
                    // Enter sends. Shift+Enter starts a new line.
                    gdk::Key::Return | gdk::Key::KP_Enter
                        if !shift && panel.0.input.has_focus() =>
                    {
                        panel.send_typed()
                    }
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        inner.window.add_controller(keys);

        inner.input.buffer().connect_changed({
            let weak = weak.clone();
            move |_| with(&weak, &|panel| panel.update_composer())
        });

        // Follow the conversation down as it grows, unless the reader
        // has scrolled up to look at something.
        let adjustment = inner.scroller.vadjustment();
        adjustment.connect_value_changed({
            let weak = weak.clone();
            move |adj| {
                if let Some(inner) = weak.upgrade() {
                    let at_bottom = adj.value() + adj.page_size() >= adj.upper() - 48.0;
                    inner.stick.set(at_bottom);
                }
            }
        });
        adjustment.connect_changed({
            let weak = weak.clone();
            move |adj| {
                if let Some(inner) = weak.upgrade() {
                    if inner.stick.get() {
                        adj.set_value(adj.upper() - adj.page_size());
                    }
                }
            }
        });
    }

    /// Sets what runs a desktop action. The shell provides this, since
    /// it owns the things the actions change.
    pub fn on_desktop_action(&self, run: impl Fn(&[String]) -> Result<(), String> + 'static) {
        *self.0.run_desktop.borrow_mut() = Some(Box::new(run));
    }

    pub fn apply_config(&self, config: &Config, on_battery: bool) {
        let inner = &self.0;
        let assistant = &config.assistant;
        let window = &inner.window;
        let right = assistant.side == Side::Right;
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Bottom, true);
        window.set_anchor(Edge::Right, right);
        window.set_anchor(Edge::Left, !right);
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_margin(edge, px(10));
        }
        let width = px(assistant.width as i32);
        inner.root.set_width_request(width);
        window.set_default_size(width, -1);
        if right {
            inner.root.remove_css_class("from-left");
        } else {
            inner.root.add_css_class("from-left");
        }
        inner
            .exit_ms
            .set(config.effective_motion(on_battery).exit_ms());
        *inner.mono.borrow_mut() = mono_family(&config.appearance.font_mono);
        inner.engine.borrow_mut().set_settings(settings(config));
        self.update_chip(None);
    }

    pub fn update_desktop(&self, desktop: &DesktopState) {
        *self.0.focused_monitor.borrow_mut() = desktop.focused_monitor.clone();
    }

    pub fn is_open(&self) -> bool {
        self.0.open.get()
    }

    pub fn toggle(&self) {
        if self.is_open() {
            self.hide();
        } else {
            self.show();
        }
    }

    pub fn show(&self) {
        let inner = &self.0;
        if let Some(timer) = inner.hide_timer.borrow_mut().take() {
            timer.remove();
        }
        if !inner.open.replace(true) {
            self.place_on_focused_monitor();
            inner.feedback.play(Sound::Open);
            self.update_chip(None);
        }
        // Ask for the keyboard outright while opening, so typing works
        // at once, then relax so a click on a window takes it back.
        inner.window.set_keyboard_mode(KeyboardMode::Exclusive);
        inner.window.set_visible(true);
        inner.input.grab_focus();
        let weak = Rc::downgrade(inner);
        glib::timeout_add_local_once(Duration::from_millis(16), {
            let weak = weak.clone();
            move || {
                if let Some(inner) = weak.upgrade() {
                    if inner.open.get() {
                        inner.root.add_css_class("open");
                    }
                }
            }
        });
        glib::timeout_add_local_once(Duration::from_millis(300), move || {
            if let Some(inner) = weak.upgrade() {
                if inner.open.get() {
                    inner.window.set_keyboard_mode(KeyboardMode::OnDemand);
                }
            }
        });
    }

    pub fn hide(&self) {
        let inner = &self.0;
        if !inner.open.replace(false) {
            return;
        }
        inner.feedback.play(Sound::Close);
        inner.root.remove_css_class("open");
        let exit = inner.exit_ms.get();
        if exit == 0 {
            inner.window.set_visible(false);
            return;
        }
        let weak = Rc::downgrade(inner);
        let timer =
            glib::timeout_add_local_once(Duration::from_millis(exit as u64 + 20), move || {
                if let Some(inner) = weak.upgrade() {
                    inner.hide_timer.borrow_mut().take();
                    if !inner.open.get() {
                        inner.window.set_visible(false);
                    }
                }
            });
        *inner.hide_timer.borrow_mut() = Some(timer);
    }

    /// Opens the panel and sends `message`, as `scuttle ask` does.
    pub fn ask(&self, message: &str) -> Result<(), String> {
        self.show();
        if self.0.engine.borrow().is_busy() {
            return Err("The assistant is still answering the last message.".into());
        }
        self.send(message);
        Ok(())
    }

    fn place_on_focused_monitor(&self) {
        let inner = &self.0;
        let Some(wanted) = inner.focused_monitor.borrow().clone() else {
            return;
        };
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let monitors = display.monitors();
        for i in 0..monitors.n_items() {
            let Some(monitor) = monitors.item(i).and_downcast::<gdk::Monitor>() else {
                continue;
            };
            if monitor.connector().map(|c| c.to_string()).as_deref() == Some(wanted.as_str()) {
                inner.window.set_monitor(Some(&monitor));
                return;
            }
        }
    }

    fn typed(&self) -> String {
        let buffer = self.0.input.buffer();
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string()
    }

    fn busy(&self) -> bool {
        self.0.engine.borrow().is_busy()
    }

    /// Keeps the placeholder and the send button in step with what is
    /// typed and whether an answer is under way.
    fn update_composer(&self) {
        let inner = &self.0;
        let empty = self.typed().trim().is_empty();
        inner.placeholder.set_visible(self.typed().is_empty());
        if self.busy() {
            inner.send.set_label("■");
            inner.send.add_css_class("stop");
            inner.send.set_sensitive(true);
            inner.send.set_tooltip_text(Some("Stop answering"));
        } else {
            inner.send.set_label("↑");
            inner.send.remove_css_class("stop");
            inner.send.set_sensitive(!empty);
            inner.send.set_tooltip_text(Some("Send (Enter)"));
        }
    }

    /// Shows which model the next message goes to, or `now` while one
    /// is answering.
    fn update_chip(&self, now: Option<Backend>) {
        let inner = &self.0;
        let engine = inner.engine.borrow();
        let backend = now.unwrap_or_else(|| engine.next_backend(now_secs()));
        let model = scuttle_assistant::pretty_model(match backend {
            Backend::Subscription => &engine.settings().subscription_model,
            Backend::ApiKey => &engine.settings().api_model,
        });
        inner.chip.set_text(&chip_text(&model, backend));
        // Tinted when the API key is standing in for the subscription.
        let standing_in = backend == Backend::ApiKey && engine.settings().prefer == Prefer::Auto;
        if standing_in {
            inner.chip.add_css_class("fallback");
        } else {
            inner.chip.remove_css_class("fallback");
        }
    }

    fn clear_messages(&self) {
        let messages = &self.0.messages;
        while let Some(child) = messages.first_child() {
            messages.remove(&child);
        }
    }

    fn show_empty_state(&self) {
        let inner = &self.0;
        self.clear_messages();
        let empty = gtk::Box::new(gtk::Orientation::Vertical, px(10));
        empty.add_css_class("msg");
        empty.add_css_class("enter");
        let title = gtk::Label::new(Some("What do you need?"));
        title.add_css_class("assistant-empty-title");
        title.set_xalign(0.0);
        title.set_margin_bottom(px(6));
        empty.append(&title);
        for suggestion in SUGGESTIONS {
            let button = gtk::Button::new();
            let label = gtk::Label::new(Some(suggestion));
            label.set_xalign(0.0);
            button.set_child(Some(&label));
            button.add_css_class("suggestion");
            let weak = Rc::downgrade(inner);
            button.connect_clicked(move |_| {
                if let Some(inner) = weak.upgrade() {
                    Panel(inner).send(suggestion);
                }
            });
            empty.append(&button);
        }
        let hint = gtk::Label::new(Some(
            "It can change volume, brightness, playback and do not disturb. \
             Terminal commands only run after you press Run.",
        ));
        hint.add_css_class("notice");
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        hint.set_max_width_chars(12);
        hint.set_margin_top(px(6));
        empty.append(&hint);
        inner.messages.append(&empty);
    }

    fn new_chat(&self) {
        let inner = &self.0;
        inner.engine.borrow_mut().new_chat();
        inner.answers.borrow_mut().clear();
        inner.live.set(None);
        inner.stick.set(true);
        self.show_empty_state();
        self.update_composer();
        self.update_chip(None);
        inner.input.grab_focus();
    }

    fn send_or_stop(&self) {
        if self.busy() {
            self.0.engine.borrow_mut().stop();
        } else {
            self.send_typed();
        }
    }

    fn send_typed(&self) {
        if self.busy() {
            return;
        }
        let text = self.typed();
        if text.trim().is_empty() {
            return;
        }
        self.0.input.buffer().set_text("");
        self.send(&text);
    }

    /// Sends a message. An empty one sends just the notes that are
    /// waiting, which is how a command's output reaches Claude.
    fn send(&self, message: &str) {
        let inner = &self.0;
        let message = message.trim();
        if self.busy() || (message.is_empty() && !inner.engine.borrow().has_notes()) {
            return;
        }
        if inner.answers.borrow().is_empty() {
            // The first message replaces the suggestions.
            self.clear_messages();
        }
        inner.stick.set(true);
        if !message.is_empty() {
            let bubble = plain_label(message, "msg");
            bubble.add_css_class("user");
            bubble.add_css_class("enter");
            inner.messages.append(&bubble);
            inner.feedback.play(Sound::Activate);
        }

        let root = gtk::Box::new(gtk::Orientation::Vertical, px(10));
        root.add_css_class("msg");
        root.add_css_class("enter");
        inner.messages.append(&root);
        let index = {
            let mut answers = inner.answers.borrow_mut();
            answers.push(Answer {
                root,
                text: String::new(),
                actions: Vec::new(),
                error: None,
                thinking: true,
            });
            answers.len() - 1
        };
        inner.live.set(Some(index));
        self.render(index);

        let job = inner.engine.borrow_mut().begin(message, now_secs());
        let generation = job.generation();
        let tx = inner.tx.clone();
        std::thread::spawn(move || {
            let events = tx.clone();
            let report = job.run(now_secs(), move |event| {
                let _ =
                    events.send_blocking(Msg::Assistant(generation, AssistantMsg::Event(event)));
            });
            let _ = tx.send_blocking(Msg::Assistant(generation, AssistantMsg::Finished(report)));
        });
        self.update_composer();
    }

    /// Takes in something from a background thread.
    pub fn handle(&self, generation: u64, msg: AssistantMsg) {
        let inner = &self.0;
        // Anything from before "New chat" belongs to a chat that is gone.
        if generation != inner.engine.borrow().generation() {
            return;
        }
        match msg {
            AssistantMsg::Event(Event::Started(backend)) => self.update_chip(Some(backend)),
            AssistantMsg::Event(Event::Notice(text)) => {
                let notice = plain_label(&text, "notice");
                // A notice explains the answer under it, so it goes
                // above the answer being written.
                let live = inner
                    .live
                    .get()
                    .and_then(|i| inner.answers.borrow().get(i).map(|a| a.root.clone()));
                match live.and_then(|root| root.prev_sibling()) {
                    Some(before) => inner.messages.insert_child_after(&notice, Some(&before)),
                    None => inner.messages.prepend(&notice),
                }
            }
            AssistantMsg::Event(Event::Delta(piece)) => {
                let Some(index) = inner.live.get() else {
                    return;
                };
                if let Some(answer) = inner.answers.borrow_mut().get_mut(index) {
                    answer.text.push_str(&piece);
                    answer.thinking = false;
                }
                self.render_soon(index);
            }
            AssistantMsg::Finished(report) => {
                if !inner.engine.borrow_mut().finish(&report, now_secs()) {
                    return;
                }
                if let Some(index) = inner.live.take() {
                    if let Some(answer) = inner.answers.borrow_mut().get_mut(index) {
                        answer.text = report.reply.clone();
                        answer.thinking = false;
                        answer.error = match (&report.error, report.stopped) {
                            (Some(error), _) => Some(error.clone()),
                            (None, true) if report.reply.trim().is_empty() => {
                                Some("Stopped.".to_string())
                            }
                            _ => None,
                        };
                    }
                    self.render(index);
                }
                if report.error.is_some() {
                    inner.feedback.play(Sound::Error);
                }
                self.update_composer();
                self.update_chip(None);
            }
            AssistantMsg::Ran {
                answer,
                action,
                outcome,
            } => {
                let command = {
                    let mut answers = inner.answers.borrow_mut();
                    let Some(ActionState::Shell { command, state, .. }) = answers
                        .get_mut(answer)
                        .and_then(|a| a.actions.get_mut(action))
                    else {
                        return;
                    };
                    *state = ShellState::Done(outcome.clone());
                    command.clone()
                };
                inner.engine.borrow_mut().note(outcome.note(&command));
                self.render(answer);
                // Hand the output straight to Claude, so it can say
                // what it means without the user having to ask again.
                if !self.busy() {
                    self.send("");
                }
            }
        }
    }

    /// Re-draws an answer shortly, folding a burst of new words into
    /// one pass.
    fn render_soon(&self, index: usize) {
        let inner = &self.0;
        if inner.render_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(inner);
        glib::timeout_add_local_once(Duration::from_millis(40), move || {
            if let Some(inner) = weak.upgrade() {
                inner.render_pending.set(false);
                Panel(inner).render(index);
            }
        });
    }

    /// Draws an answer from its text: paragraphs, code and action cards.
    fn render(&self, index: usize) {
        let inner = &self.0;
        let (root, text, error, thinking) = {
            let answers = inner.answers.borrow();
            let Some(answer) = answers.get(index) else {
                return;
            };
            (
                answer.root.clone(),
                answer.text.clone(),
                answer.error.clone(),
                answer.thinking,
            )
        };
        // Re-drawing removes whatever had focus in the answer, such as
        // the Run button just pressed. Send focus back to the composer
        // instead of letting it land somewhere arbitrary.
        let had_focus = gtk::prelude::RootExt::focus(&inner.window)
            .is_some_and(|focused| focused.is_ancestor(&root));
        while let Some(child) = root.first_child() {
            root.remove(&child);
        }
        let mono = inner.mono.borrow().clone();
        let mut seen = 0;
        for segment in segments(&text) {
            match segment {
                Segment::Text(text) => {
                    for block in markdown::blocks(&text, &mono) {
                        root.append(&self.block_widget(block));
                    }
                }
                other => {
                    let known = inner.answers.borrow()[index].actions.len();
                    if seen >= known {
                        let state = self.start_action(other);
                        inner.answers.borrow_mut()[index].actions.push(state);
                    }
                    let state = inner.answers.borrow()[index].actions[seen].clone();
                    root.append(&self.action_widget(index, seen, &state));
                    seen += 1;
                }
            }
        }
        if thinking {
            let label = gtk::Label::new(Some("Thinking"));
            label.add_css_class("thinking");
            label.set_xalign(0.0);
            root.append(&label);
        }
        if let Some(error) = error {
            root.append(&plain_label(&error, "msg-error"));
        }
        if had_focus {
            inner.input.grab_focus();
        }
    }

    /// Decides what an action that has just appeared becomes. A desktop
    /// action runs here, once. A terminal command only waits.
    fn start_action(&self, segment: Segment) -> ActionState {
        let inner = &self.0;
        match segment {
            Segment::Action(Action::Desktop(args)) => {
                let result = match inner.run_desktop.borrow().as_ref() {
                    Some(run) => run(&args),
                    None => Err("Scuttle is not ready to do that yet.".to_string()),
                };
                if result.is_err() {
                    inner.feedback.play(Sound::Error);
                }
                ActionState::Desktop {
                    label: describe_desktop(&args),
                    result,
                }
            }
            Segment::Action(Action::Shell { command, why }) => {
                inner.feedback.play(Sound::Ask);
                ActionState::Shell {
                    command,
                    why,
                    state: ShellState::Waiting,
                }
            }
            Segment::Rejected(reason) => ActionState::Rejected(reason),
            Segment::Text(_) => ActionState::Rejected(String::new()),
        }
    }

    fn block_widget(&self, block: Block) -> gtk::Widget {
        match block {
            Block::Para(markup) => text_label(&markup).upcast(),
            Block::Item { marker, markup } => {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, px(6));
                let dot = gtk::Label::new(Some(&marker));
                dot.add_css_class("msg-marker");
                dot.set_valign(gtk::Align::Start);
                dot.set_xalign(0.0);
                row.append(&dot);
                row.append(&text_label(&markup));
                row.upcast()
            }
            Block::Code { text, .. } => self.code_widget(&text, true).upcast(),
        }
    }

    /// A block of monospace text, optionally with a button to copy it.
    fn code_widget(&self, text: &str, copyable: bool) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, px(8));
        row.add_css_class("msg-code");
        let label = plain_label(text, "mono");
        row.append(&label);
        if copyable {
            let copy = gtk::Button::with_label("Copy");
            copy.add_css_class("icon");
            copy.set_valign(gtk::Align::Start);
            let text = text.to_string();
            copy.connect_clicked(move |button| {
                button.clipboard().set_text(&text);
                button.set_label("Copied");
                let weak = button.downgrade();
                glib::timeout_add_local_once(Duration::from_millis(1400), move || {
                    if let Some(button) = weak.upgrade() {
                        button.set_label("Copy");
                    }
                });
            });
            row.append(&copy);
        }
        row
    }

    fn action_widget(&self, answer: usize, action: usize, state: &ActionState) -> gtk::Widget {
        let inner = &self.0;
        match state {
            ActionState::Desktop { label, result } => {
                let text = match result {
                    Ok(()) => format!("✓  {label}"),
                    Err(why) => format!("{label} did not work. {why}"),
                };
                let class = if result.is_ok() {
                    "action-done"
                } else {
                    "msg-error"
                };
                plain_label(&text, class).upcast()
            }
            ActionState::Rejected(reason) => plain_label(reason, "msg-error").upcast(),
            ActionState::Shell {
                command,
                why,
                state,
            } => {
                let card = gtk::Box::new(gtk::Orientation::Vertical, px(10));
                card.add_css_class("action-card");
                if !why.is_empty() {
                    card.append(&plain_label(why, "action-why"));
                }
                card.append(&self.code_widget(command, false));
                match state {
                    ShellState::Waiting => {
                        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, px(8));
                        let run = gtk::Button::with_label("Run");
                        run.add_css_class("primary");
                        run.add_css_class("small");
                        let skip = gtk::Button::with_label("Skip");
                        skip.add_css_class("quiet");
                        skip.add_css_class("small");
                        let weak = Rc::downgrade(inner);
                        run.connect_clicked({
                            let weak = weak.clone();
                            move |_| {
                                if let Some(inner) = weak.upgrade() {
                                    Panel(inner).run_command(answer, action);
                                }
                            }
                        });
                        skip.connect_clicked(move |_| {
                            if let Some(inner) = weak.upgrade() {
                                Panel(inner).skip_command(answer, action);
                            }
                        });
                        let hint = gtk::Label::new(Some("Ctrl+Enter runs it"));
                        hint.add_css_class("action-status");
                        hint.set_hexpand(true);
                        hint.set_xalign(1.0);
                        buttons.append(&run);
                        buttons.append(&skip);
                        buttons.append(&hint);
                        card.append(&buttons);
                    }
                    ShellState::Running => {
                        card.add_css_class("settled");
                        let status = gtk::Label::new(Some("Running"));
                        status.add_css_class("thinking");
                        status.set_xalign(0.0);
                        card.append(&status);
                    }
                    ShellState::Skipped => {
                        card.add_css_class("settled");
                        card.append(&plain_label("Skipped", "action-status"));
                    }
                    ShellState::Done(outcome) => {
                        card.add_css_class("settled");
                        let status = plain_label(&outcome.summary(), "action-status");
                        if !outcome.succeeded() {
                            status.add_css_class("bad");
                        }
                        card.append(&status);
                        let preview = output_preview(&outcome.output, OUTPUT_LINES);
                        if !preview.is_empty() {
                            card.append(&self.code_widget(&preview, false));
                        }
                    }
                }
                card.upcast()
            }
        }
    }

    /// Runs the newest command still waiting for a decision. Returns
    /// false when there is none.
    fn run_waiting_command(&self) -> bool {
        let waiting = {
            let answers = self.0.answers.borrow();
            answers.iter().enumerate().rev().find_map(|(a, answer)| {
                answer
                    .actions
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(i, action)| {
                        matches!(
                            action,
                            ActionState::Shell {
                                state: ShellState::Waiting,
                                ..
                            }
                        )
                        .then_some((a, i))
                    })
            })
        };
        match waiting {
            Some((answer, action)) => {
                self.run_command(answer, action);
                true
            }
            None => false,
        }
    }

    /// Runs a proposed command. Only ever called because the user
    /// asked: from its Run button, or Ctrl+Enter.
    fn run_command(&self, answer: usize, action: usize) {
        let inner = &self.0;
        let command = {
            let mut answers = inner.answers.borrow_mut();
            let Some(ActionState::Shell { command, state, .. }) = answers
                .get_mut(answer)
                .and_then(|a| a.actions.get_mut(action))
            else {
                return;
            };
            // A second click while it runs must not start it again.
            if !matches!(state, ShellState::Waiting) {
                return;
            }
            *state = ShellState::Running;
            command.clone()
        };
        inner.feedback.play(Sound::Activate);
        self.render(answer);
        let generation = inner.engine.borrow().generation();
        let tx = inner.tx.clone();
        std::thread::spawn(move || {
            let outcome = command::run(&command, COMMAND_LIMIT);
            let _ = tx.send_blocking(Msg::Assistant(
                generation,
                AssistantMsg::Ran {
                    answer,
                    action,
                    outcome,
                },
            ));
        });
    }

    fn skip_command(&self, answer: usize, action: usize) {
        let inner = &self.0;
        let command = {
            let mut answers = inner.answers.borrow_mut();
            let Some(ActionState::Shell { command, state, .. }) = answers
                .get_mut(answer)
                .and_then(|a| a.actions.get_mut(action))
            else {
                return;
            };
            if !matches!(state, ShellState::Waiting) {
                return;
            }
            *state = ShellState::Skipped;
            command.clone()
        };
        inner
            .engine
            .borrow_mut()
            .note(format!("the user chose not to run `{command}`."));
        self.render(answer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_maps_onto_the_assistant_s_settings() {
        let mut config = Config::default();
        let s = settings(&config);
        assert_eq!(s.prefer, Prefer::Auto);
        assert_eq!(s.subscription_model, "claude-opus-5-5");
        assert_eq!(s.api_model, "claude-haiku-5-5");
        assert_eq!(s.claude_program, "claude");
        assert!(s.claude_dir.is_some_and(|dir| dir.ends_with("assistant")));

        config.assistant.backend = AssistantBackend::Api;
        config.assistant.api_model = "claude-sonnet-5-5".into();
        let s = settings(&config);
        assert_eq!(s.prefer, Prefer::ApiKey);
        assert_eq!(s.api_model, "claude-sonnet-5-5");
        config.assistant.backend = AssistantBackend::Subscription;
        assert_eq!(settings(&config).prefer, Prefer::Subscription);
    }

    #[test]
    fn the_api_base_can_be_overridden_but_only_with_a_url() {
        let default = scuttle_assistant::api::DEFAULT_BASE;
        assert_eq!(api_base(None), default);
        assert_eq!(api_base(Some("")), default);
        assert_eq!(api_base(Some("not a url")), default);
        assert_eq!(
            api_base(Some(" https://gateway.example.com ")),
            "https://gateway.example.com"
        );
    }

    #[test]
    fn inline_code_falls_back_to_a_real_monospace_font() {
        assert_eq!(
            mono_family("Geist Mono"),
            "Geist Mono, JetBrains Mono, DejaVu Sans Mono, monospace"
        );
        assert_eq!(
            mono_family("  "),
            "JetBrains Mono, DejaVu Sans Mono, monospace"
        );
        assert!(!mono_family("Evil, Font").starts_with("Evil,"));
    }

    #[test]
    fn the_chip_names_the_model_and_the_route() {
        assert_eq!(
            chip_text("Opus 5.5", Backend::Subscription),
            "Opus 5.5 · subscription"
        );
        assert_eq!(
            chip_text("Haiku 5.5", Backend::ApiKey),
            "Haiku 5.5 · API key"
        );
    }

    #[test]
    fn desktop_actions_are_described_in_words() {
        let args = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(
            describe_desktop(&args("volume down")),
            "Lower the volume a step"
        );
        assert_eq!(describe_desktop(&args("dnd on")), "Turn do not disturb on");
        assert_eq!(describe_desktop(&args("mystery")), "scuttle mystery");
    }

    #[test]
    fn every_desktop_action_has_a_description() {
        for (command, _) in scuttle_assistant::actions::DESKTOP_COMMANDS {
            let args: Vec<String> = command.split(' ').map(str::to_string).collect();
            assert!(
                !describe_desktop(&args).starts_with("scuttle "),
                "{command}"
            );
        }
    }

    #[test]
    fn long_output_is_cut_for_the_card() {
        assert_eq!(output_preview("a\nb\n", 5), "a\nb");
        assert_eq!(output_preview("", 5), "");
        let long = (1..=20)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(output_preview(&long, 3), "1\n2\n3\n… 17 more lines");
    }
}
