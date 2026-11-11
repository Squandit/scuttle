//! The launcher overlay: a full-screen layer surface with a search box,
//! grouped results and a detail pane for the selected result.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::{Rc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use scuttle_core::config::{BarPosition, Config};
use scuttle_hypr::{Dispatch, Hyprland};
use scuttle_launcher::clipboard::ClipEntry;
use scuttle_launcher::desktop::DesktopEntry;
use scuttle_launcher::engine::pretty_class;
use scuttle_launcher::files::FileEntry;
use scuttle_launcher::history::History;
use scuttle_launcher::{Action, Engine, Internal, Item, Kind, Mode, WindowInfo};
use scuttle_sound::{Feedback, Sound};

use crate::actions;
use crate::msg::{DesktopState, Stats};
use crate::scale::px;

/// Width of the launcher column on screens wide enough to fit it.
const PANEL_WIDTH: i32 = 980;
const LIST_WIDTH: i32 = 440;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A short description of what Enter will do, plus any detail worth
/// showing in monospace (a command line, path or URL).
pub fn describe(action: &Action) -> (&'static str, String) {
    match action {
        Action::Launch {
            exec,
            terminal: false,
        } => ("Start", exec.clone()),
        Action::Launch {
            exec,
            terminal: true,
        } => ("Start in a terminal", exec.clone()),
        Action::FocusWindow { .. } => ("Switch to this window", String::new()),
        Action::OpenPath(path) => ("Open", path.display().to_string()),
        Action::OpenUrl(url) => ("Open in the browser", url.clone()),
        Action::Copy(text) => ("Copy to the clipboard", text.clone()),
        Action::RestoreClip(_) => ("Copy back to the clipboard", String::new()),
        Action::Shell(command) => ("Run", command.clone()),
        Action::Internal(Internal::Reload) => ("Re-read config.toml", String::new()),
        Action::Internal(Internal::Quit) => ("Stop Scuttle", String::new()),
        Action::Internal(Internal::Logout) => ("End this session", String::new()),
        Action::Internal(Internal::ToggleDnd) => ("Turn do not disturb on or off", String::new()),
    }
}

/// The status readout shown in the corners when there is no bar.
struct Glance {
    clock: gtk::Label,
    date: gtk::Label,
    status: gtk::Box,
    workspaces: gtk::Box,
    corners: [gtk::Box; 3],
}

struct Inner {
    window: gtk::ApplicationWindow,
    scrim: gtk::Overlay,
    panel: gtk::Box,
    mode_chip: gtk::Label,
    entry: gtk::Entry,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    empty: gtk::Label,
    detail: gtk::Box,
    detail_icon: gtk::Image,
    detail_title: gtk::Label,
    detail_sub: gtk::Label,
    detail_action: gtk::Label,
    detail_command: gtk::Label,
    message: gtk::Label,
    glance: Glance,

    engine: RefCell<Engine>,
    items: RefCell<Vec<Item>>,
    /// Row waiting for a second Enter before a destructive action runs.
    confirming: Cell<Option<usize>>,
    open: Cell<bool>,
    /// Rows animate in once, when the overlay opens, not on every key.
    animate_rows: Cell<bool>,
    /// The clipboard history has been asked for since the overlay
    /// opened, so it is not fetched again on every key press.
    clipboard_requested: Cell<bool>,
    on_clipboard_wanted: RefCell<Option<Box<dyn Fn()>>>,
    exit_ms: Cell<u32>,
    hide_timer: RefCell<Option<glib::SourceId>>,
    terminal: RefCell<String>,
    hypr: Option<Hyprland>,
    feedback: Rc<Feedback>,
    focused_monitor: RefCell<Option<String>>,
    on_internal: RefCell<Option<InternalHandler>>,
}

/// Called for results that act on the shell itself.
type InternalHandler = Box<dyn Fn(Internal)>;

/// The launcher overlay. Cheap to clone; clones share one surface.
#[derive(Clone)]
pub struct Overlay(Rc<Inner>);

impl Overlay {
    pub fn new(
        app: &gtk::Application,
        config: &Config,
        home: &Path,
        history: History,
        hypr: Option<Hyprland>,
        feedback: Rc<Feedback>,
    ) -> Overlay {
        let window = gtk::ApplicationWindow::new(app);
        window.add_css_class("scuttle");
        window.init_layer_shell();
        window.set_namespace(Some("scuttle-launcher"));
        window.set_layer(Layer::Overlay);
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        // Cover the whole output, bar included.
        window.set_exclusive_zone(-1);
        window.set_keyboard_mode(KeyboardMode::Exclusive);

        let scrim = gtk::Overlay::new();
        scrim.add_css_class("scrim");
        window.set_child(Some(&scrim));

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        panel.add_css_class("launcher");
        panel.set_halign(gtk::Align::Center);
        panel.set_valign(gtk::Align::Start);
        panel.set_margin_top(px(150));
        panel.set_width_request(px(PANEL_WIDTH));
        scrim.set_child(Some(&panel));

        // Search row.
        let search = gtk::Box::new(gtk::Orientation::Horizontal, px(14));
        search.add_css_class("search-row");
        let mode_chip = gtk::Label::new(None);
        mode_chip.add_css_class("mode-chip");
        mode_chip.set_valign(gtk::Align::Center);
        mode_chip.set_visible(false);
        let entry = gtk::Entry::new();
        entry.add_css_class("query");
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some("Search"));
        search.append(&mode_chip);
        search.append(&entry);
        panel.append(&search);

        // Results and detail.
        let body = gtk::Box::new(gtk::Orientation::Horizontal, px(20));
        body.set_margin_top(px(20));
        panel.append(&body);

        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.set_width_request(px(LIST_WIDTH));
        let list = gtk::ListBox::new();
        list.add_css_class("results");
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_activate_on_single_click(true);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height(px(460));
        scroller.set_child(Some(&list));
        let empty = gtk::Label::new(Some("Nothing matches. Enter searches the web."));
        empty.add_css_class("hint");
        empty.set_xalign(0.0);
        empty.set_margin_start(px(14));
        empty.set_margin_top(px(10));
        empty.set_visible(false);
        left.append(&scroller);
        left.append(&empty);
        body.append(&left);

        let detail = gtk::Box::new(gtk::Orientation::Vertical, px(6));
        detail.add_css_class("detail");
        detail.set_hexpand(true);
        detail.set_valign(gtk::Align::Start);
        let detail_icon = gtk::Image::new();
        detail_icon.set_pixel_size(px(48));
        detail_icon.set_halign(gtk::Align::Start);
        detail_icon.set_margin_bottom(px(8));
        let label = |class: &str| {
            let l = gtk::Label::new(None);
            l.add_css_class(class);
            l.set_xalign(0.0);
            l.set_wrap(true);
            l.set_wrap_mode(pango::WrapMode::WordChar);
            l
        };
        let detail_title = label("detail-title");
        let detail_sub = label("detail-sub");
        let detail_action = label("detail-action");
        detail_action.set_margin_top(px(18));
        let detail_command = label("detail-key");
        detail_command.set_lines(3);
        detail_command.set_ellipsize(pango::EllipsizeMode::End);
        detail.append(&detail_icon);
        detail.append(&detail_title);
        detail.append(&detail_sub);
        detail.append(&detail_action);
        detail.append(&detail_command);
        body.append(&detail);

        // Footer.
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, px(16));
        footer.set_margin_top(px(22));
        let prefixes = gtk::Label::new(Some(
            "> system   / files   ? web   = calc   @ windows   ; clipboard   : emoji",
        ));
        prefixes.add_css_class("hint");
        prefixes.add_css_class("mono");
        prefixes.set_xalign(0.0);
        prefixes.set_hexpand(true);
        let message = gtk::Label::new(None);
        message.add_css_class("confirm");
        let closes = gtk::Label::new(Some("Esc closes"));
        closes.add_css_class("hint");
        footer.append(&prefixes);
        footer.append(&message);
        footer.append(&closes);
        panel.append(&footer);

        // Corner status, used when the bar is turned off.
        let corner = |h: gtk::Align, v: gtk::Align| {
            let b = gtk::Box::new(gtk::Orientation::Vertical, px(6));
            b.add_css_class("glance");
            b.set_halign(h);
            b.set_valign(v);
            b.set_margin_top(px(40));
            b.set_margin_bottom(px(40));
            b.set_margin_start(px(48));
            b.set_margin_end(px(48));
            b.set_can_target(false);
            b
        };
        let top_left = corner(gtk::Align::Start, gtk::Align::Start);
        let clock = gtk::Label::new(None);
        clock.add_css_class("glance-clock");
        clock.set_xalign(0.0);
        let date = gtk::Label::new(None);
        date.add_css_class("dim");
        date.set_xalign(0.0);
        top_left.append(&clock);
        top_left.append(&date);
        let status = corner(gtk::Align::End, gtk::Align::Start);
        let workspaces = corner(gtk::Align::Start, gtk::Align::End);
        for c in [&top_left, &status, &workspaces] {
            scrim.add_overlay(c);
        }
        let glance = Glance {
            clock,
            date,
            status: status.clone(),
            workspaces: workspaces.clone(),
            corners: [top_left, status, workspaces],
        };

        let engine = Engine::new(
            &config.launcher.web_search,
            config.launcher.max_results,
            home,
            history,
        );

        let inner = Rc::new(Inner {
            window,
            scrim,
            panel,
            mode_chip,
            entry,
            list,
            scroller,
            empty,
            detail,
            detail_icon,
            detail_title,
            detail_sub,
            detail_action,
            detail_command,
            message,
            glance,
            engine: RefCell::new(engine),
            items: RefCell::new(Vec::new()),
            confirming: Cell::new(None),
            open: Cell::new(false),
            animate_rows: Cell::new(false),
            clipboard_requested: Cell::new(false),
            on_clipboard_wanted: RefCell::new(None),
            exit_ms: Cell::new(0),
            hide_timer: RefCell::new(None),
            terminal: RefCell::new(String::new()),
            hypr,
            focused_monitor: RefCell::new(None),
            feedback,
            on_internal: RefCell::new(None),
        });
        let overlay = Overlay(inner);
        overlay.apply_config(config, false);
        overlay.connect_signals();
        overlay
    }

    fn connect_signals(&self) {
        let inner = &self.0;
        let weak = Rc::downgrade(inner);

        inner.entry.connect_changed({
            let weak = weak.clone();
            move |_| with(&weak, |o| o.refresh())
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let weak = weak.clone();
            move |_, key, _, modifiers| {
                let Some(inner) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                let overlay = Overlay(inner);
                let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
                match key {
                    gdk::Key::Escape => overlay.hide(),
                    gdk::Key::Return | gdk::Key::KP_Enter => overlay.activate_selected(),
                    gdk::Key::Down | gdk::Key::Tab => overlay.move_selection(1),
                    gdk::Key::Up | gdk::Key::ISO_Left_Tab => overlay.move_selection(-1),
                    gdk::Key::n | gdk::Key::j if ctrl => overlay.move_selection(1),
                    gdk::Key::p | gdk::Key::k if ctrl => overlay.move_selection(-1),
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        inner.window.add_controller(keys);

        inner.list.connect_row_selected({
            let weak = weak.clone();
            move |_, row| {
                with(&weak, |o| {
                    // Moving away from a row cancels its pending confirmation.
                    let index = row.map(|r| r.index() as usize);
                    if o.0.confirming.get() != index {
                        o.0.confirming.set(None);
                        o.0.message.set_text("");
                    }
                    o.update_detail();
                })
            }
        });

        inner.list.connect_row_activated({
            let weak = weak.clone();
            move |_, row| with(&weak, |o| o.activate(row.index() as usize))
        });

        // Section headings appear wherever the kind of result changes.
        inner.list.set_header_func({
            let weak = weak.clone();
            move |row, before| {
                let Some(inner) = weak.upgrade() else { return };
                let items = inner.items.borrow();
                let kind = items.get(row.index() as usize).map(|i| i.kind);
                let previous = before
                    .and_then(|b| items.get(b.index() as usize))
                    .map(|i| i.kind);
                match kind {
                    Some(kind) if Some(kind) != previous => {
                        let heading = gtk::Label::new(Some(kind.section()));
                        heading.add_css_class("section");
                        heading.set_xalign(0.0);
                        row.set_header(Some(&heading));
                    }
                    _ => row.set_header(None::<&gtk::Widget>),
                }
            }
        });

        // A click on the backdrop, outside the panel, closes the overlay.
        let click = gtk::GestureClick::new();
        click.connect_pressed({
            let weak = weak.clone();
            move |_, _, x, y| {
                with(&weak, |o| {
                    let target = o.0.scrim.pick(x, y, gtk::PickFlags::DEFAULT);
                    let on_backdrop = target
                        .map(|w| w == *o.0.scrim.upcast_ref::<gtk::Widget>())
                        .unwrap_or(true);
                    if on_backdrop {
                        o.hide();
                    }
                })
            }
        });
        inner.scrim.add_controller(click);
    }

    /// Registers what happens for results that act on the shell itself.
    pub fn on_internal(&self, callback: impl Fn(Internal) + 'static) {
        *self.0.on_internal.borrow_mut() = Some(Box::new(callback));
    }

    /// Applies config values that affect the overlay.
    pub fn apply_config(&self, config: &Config, on_battery: bool) {
        let inner = &self.0;
        inner
            .engine
            .borrow_mut()
            .set_options(&config.launcher.web_search, config.launcher.max_results);
        *inner.terminal.borrow_mut() = config.launcher.terminal.clone();
        inner
            .exit_ms
            .set(config.effective_motion(on_battery).exit_ms());
        // Sizes set from code follow the size setting when it changes.
        inner.scroller.set_width_request(px(LIST_WIDTH));
        inner.detail_icon.set_pixel_size(px(48));
        let show_glance = config.bar.position == BarPosition::None;
        for corner in &inner.glance.corners {
            corner.set_visible(show_glance);
        }
    }

    pub fn set_apps(&self, apps: Vec<DesktopEntry>) {
        self.0.engine.borrow_mut().set_apps(apps);
        if self.0.open.get() {
            self.refresh();
        }
    }

    /// Registers what to do when the clipboard history is needed.
    pub fn on_clipboard_wanted(&self, callback: impl Fn() + 'static) {
        *self.0.on_clipboard_wanted.borrow_mut() = Some(Box::new(callback));
    }

    /// Receives the clipboard history, or the reason there is none.
    pub fn set_clipboard(&self, history: Result<Vec<ClipEntry>, String>) {
        let inner = &self.0;
        match history {
            Ok(entries) => {
                inner.engine.borrow_mut().set_clipboard(entries);
                if inner.open.get() {
                    self.refresh();
                }
            }
            Err(problem) => {
                inner.engine.borrow_mut().set_clipboard(Vec::new());
                if inner.open.get() {
                    self.refresh();
                    inner.message.set_text(&problem);
                }
            }
        }
    }

    pub fn set_files(&self, files: Vec<FileEntry>) {
        self.0.engine.borrow_mut().set_files(files);
    }

    /// Feeds compositor state to the window search and corner status.
    pub fn update_desktop(&self, desktop: &DesktopState) {
        let inner = &self.0;
        let mut clients = desktop.clients.clone();
        clients.sort_by_key(|c| c.focus_history_id);
        let windows = clients
            .into_iter()
            .map(|c| WindowInfo {
                address: c.address,
                class: c.class,
                title: c.title,
                workspace: c.workspace.name,
            })
            .collect();
        inner.engine.borrow_mut().set_windows(windows);
        *inner.focused_monitor.borrow_mut() = desktop.focused_monitor.clone();

        let list = &inner.glance.workspaces;
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        for workspace in desktop.workspaces.iter().filter(|w| !w.is_special()) {
            let mut classes: Vec<String> = desktop
                .clients
                .iter()
                .filter(|c| c.workspace.id == workspace.id)
                .map(|c| pretty_class(&c.class))
                .collect();
            classes.dedup();
            let text = if classes.is_empty() {
                format!("{}  empty", workspace.name)
            } else {
                format!("{}  {}", workspace.name, classes.join(", "))
            };
            let line = gtk::Label::new(Some(&text));
            line.set_xalign(0.0);
            line.add_css_class(if workspace.id == desktop.active_workspace {
                "active"
            } else if classes.is_empty() {
                "dim"
            } else {
                "plain"
            });
            list.append(&line);
        }
    }

    /// Feeds machine stats to the corner status.
    pub fn update_stats(&self, stats: &Stats) {
        let status = &self.0.glance.status;
        while let Some(child) = status.first_child() {
            status.remove(&child);
        }
        let mut lines = Vec::new();
        if let Some(battery) = stats.battery {
            let state = if battery.plugged_in { ", charging" } else { "" };
            lines.push(("Battery", format!("{}%{state}", battery.percent)));
        }
        lines.push((
            "Network",
            match &stats.link {
                scuttle_sys::network::Link::Offline => "Offline".to_string(),
                scuttle_sys::network::Link::Wired(_) => "Wired".to_string(),
                scuttle_sys::network::Link::Wireless(_) => "Wi-Fi".to_string(),
            },
        ));
        if let Some(memory) = stats.memory {
            lines.push(("Memory", memory.used_label()));
        }
        for (key, value) in lines {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, px(6));
            row.set_halign(gtk::Align::End);
            let k = gtk::Label::new(Some(key));
            k.add_css_class("dim");
            row.append(&k);
            row.append(&gtk::Label::new(Some(&value)));
            status.append(&row);
        }
    }

    pub fn is_open(&self) -> bool {
        self.0.open.get()
    }

    pub fn toggle(&self) {
        if self.is_open() {
            self.hide();
        } else {
            self.show(None);
        }
    }

    /// Opens the overlay, optionally with text already typed.
    pub fn show(&self, query: Option<&str>) {
        let inner = &self.0;
        if let Some(timer) = inner.hide_timer.borrow_mut().take() {
            timer.remove();
        }
        let was_open = inner.open.replace(true);

        if !was_open {
            self.place_on_focused_monitor();
            let now = glib::DateTime::now_local().ok();
            let fmt = |f: &str| {
                now.as_ref()
                    .and_then(|d| d.format(f).ok())
                    .map(|s| s.to_string())
            };
            inner
                .glance
                .clock
                .set_text(&fmt("%H:%M").unwrap_or_default());
            inner
                .glance
                .date
                .set_text(&fmt("%A %-d %B").unwrap_or_default());
            inner.animate_rows.set(true);
            inner.feedback.play(Sound::Open);
        }

        inner.message.set_text("");
        inner.confirming.set(None);
        let text = query.unwrap_or("");
        if inner.entry.text() == text {
            self.refresh();
        } else {
            // Triggers `refresh` through the changed signal.
            inner.entry.set_text(text);
        }
        inner.entry.set_position(-1);

        inner.window.set_visible(true);
        inner.entry.grab_focus_without_selecting();

        // The first frame has to be drawn closed before the "open"
        // class is added, or there is nothing to animate from.
        let weak = Rc::downgrade(inner);
        glib::timeout_add_local_once(Duration::from_millis(16), move || {
            if let Some(inner) = weak.upgrade() {
                if inner.open.get() {
                    inner.scrim.add_css_class("open");
                }
            }
        });
    }

    /// Closes the overlay without running anything.
    pub fn hide(&self) {
        self.close(Some(Sound::Close));
    }

    /// Closes the overlay, playing `sound` if it was open.
    fn close(&self, sound: Option<Sound>) {
        let inner = &self.0;
        if !inner.open.replace(false) {
            return;
        }
        if let Some(sound) = sound {
            inner.feedback.play(sound);
        }
        inner.clipboard_requested.set(false);
        inner.scrim.remove_css_class("open");
        let exit = inner.exit_ms.get();
        if exit == 0 {
            inner.window.set_visible(false);
            return;
        }
        // Keep the surface mapped until the closing animation is done.
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

    /// Puts the overlay on the monitor that has focus, and fits the
    /// panel to that monitor's width.
    fn place_on_focused_monitor(&self) {
        let inner = &self.0;
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let monitors = display.monitors();
        let wanted = inner.focused_monitor.borrow().clone();
        let mut chosen: Option<gdk::Monitor> = None;
        for i in 0..monitors.n_items() {
            let Some(monitor) = monitors.item(i).and_downcast::<gdk::Monitor>() else {
                continue;
            };
            let matches = wanted.is_some() && monitor.connector().map(|c| c.to_string()) == wanted;
            if matches || chosen.is_none() {
                chosen = Some(monitor);
                if matches {
                    break;
                }
            }
        }
        if let Some(monitor) = chosen {
            if wanted.is_some() {
                inner.window.set_monitor(Some(&monitor));
            }
            let geometry = monitor.geometry();
            let (width, height) = (geometry.width(), geometry.height());
            inner
                .panel
                .set_width_request(px(PANEL_WIDTH).min(width - px(96)).max(360));
            // Fit the height too: on a short screen, or at a large
            // size setting, start higher up and let the results scroll
            // instead of running off the bottom.
            let top = px(150).min(height / 7);
            inner.panel.set_margin_top(top);
            // Search row, the gaps around the results, the footer and
            // some room to breathe at the bottom.
            let chrome = px(88 + 20 + 22 + 24 + 40);
            let room = (height - top - chrome).clamp(px(96), px(460));
            inner.scroller.set_max_content_height(room);
        }
    }

    /// Re-runs the search and rebuilds the result list.
    fn refresh(&self) {
        let inner = &self.0;
        let query = inner.entry.text().to_string();
        let (mode, _) = Mode::parse(&query);
        match mode.label() {
            Some(label) => {
                inner.mode_chip.set_text(label);
                inner.mode_chip.set_visible(true);
            }
            None => inner.mode_chip.set_visible(false),
        }

        // The first time clipboard mode is entered after opening,
        // fetch the history. The list fills in when it arrives.
        if mode == Mode::Clipboard && !inner.clipboard_requested.replace(true) {
            if let Some(callback) = inner.on_clipboard_wanted.borrow().as_ref() {
                callback();
            }
        }

        let results = inner.engine.borrow_mut().search(&query, now_secs());
        // Store the items before touching the list: adding rows calls
        // the header function, which reads them.
        *inner.items.borrow_mut() = results.clone();
        inner.confirming.set(None);
        inner.message.set_text("");

        inner.list.remove_all();
        let animate = inner.animate_rows.replace(false);
        for (index, item) in results.iter().enumerate() {
            let row = build_row(item);
            if animate {
                row.add_css_class("enter");
                if (1..=7).contains(&index) {
                    row.add_css_class(&format!("d{index}"));
                }
            }
            inner.list.append(&row);
        }
        let has_results = !results.is_empty();
        let typed_something = !Mode::parse(&query).1.is_empty();
        inner.empty.set_text(match (mode, typed_something) {
            (Mode::Clipboard, false) => "Nothing has been copied yet.",
            (Mode::Clipboard, true) => "Nothing in the clipboard history matches.",
            (Mode::Emoji, _) => "No emoji by that name.",
            (Mode::Files, false) => "Type part of a file name.",
            (Mode::Calc, _) => "Type a sum, like 12 * (3 + 4).",
            (Mode::Web, false) => "Type what to search the web for.",
            _ => "Nothing matches.",
        });
        inner
            .empty
            .set_visible(!has_results && (mode != Mode::All || typed_something));
        inner.detail.set_visible(has_results);
        if let Some(first) = inner.list.row_at_index(0) {
            inner.list.select_row(Some(&first));
        }
        inner.scroller.vadjustment().set_value(0.0);
        self.update_detail();
    }

    fn selected_index(&self) -> Option<usize> {
        self.0.list.selected_row().map(|r| r.index() as usize)
    }

    fn move_selection(&self, delta: i32) {
        let inner = &self.0;
        let count = inner.items.borrow().len() as i32;
        if count == 0 {
            return;
        }
        let current = self.selected_index().map(|i| i as i32).unwrap_or(-1);
        let next = (current + delta).rem_euclid(count);
        if let Some(row) = inner.list.row_at_index(next) {
            inner.list.select_row(Some(&row));
            self.scroll_to(&row);
            if next != current {
                inner.feedback.play(Sound::Tick);
            }
        }
    }

    /// Scrolls the list just far enough to show `row`. Focus stays in
    /// the search box, so this cannot rely on focus scrolling.
    fn scroll_to(&self, row: &gtk::ListBoxRow) {
        let inner = &self.0;
        let Some(bounds) = row.compute_bounds(&inner.list) else {
            return;
        };
        let adjustment = inner.scroller.vadjustment();
        let (top, bottom) = (bounds.y() as f64, (bounds.y() + bounds.height()) as f64);
        let (view_top, page) = (adjustment.value(), adjustment.page_size());
        if top < view_top {
            // Leave room for a section heading above the first row of a group.
            adjustment.set_value((top - 40.0).max(0.0));
        } else if bottom > view_top + page {
            adjustment.set_value(bottom - page);
        }
    }

    fn update_detail(&self) {
        let inner = &self.0;
        let items = inner.items.borrow();
        let Some(item) = self.selected_index().and_then(|i| items.get(i)) else {
            return;
        };
        // Emoji carry their own picture in the title.
        inner.detail_icon.set_visible(item.kind != Kind::Emoji);
        set_icon(&inner.detail_icon, item.icon.as_deref());
        inner.detail_title.set_text(&item.title);
        inner.detail_sub.set_text(&item.subtitle);
        inner.detail_sub.set_visible(!item.subtitle.is_empty());
        let (verb, extra) = describe(&item.action);
        inner.detail_action.set_text(&format!("Enter   {verb}"));
        inner.detail_command.set_text(&extra);
        inner.detail_command.set_visible(!extra.is_empty());
    }

    fn activate_selected(&self) {
        if let Some(index) = self.selected_index() {
            self.activate(index);
        }
    }

    fn activate(&self, index: usize) {
        let inner = &self.0;
        let Some(item) = inner.items.borrow().get(index).cloned() else {
            return;
        };

        if item.confirm && inner.confirming.get() != Some(index) {
            inner.confirming.set(Some(index));
            inner.message.set_text(&format!(
                "Press Enter again to {}",
                item.title.to_lowercase()
            ));
            inner.feedback.play(Sound::Ask);
            return;
        }

        match self.run(&item.action) {
            Ok(()) => {
                inner.engine.borrow_mut().record_use(&item, now_secs());
                self.close(Some(Sound::Activate));
            }
            Err(problem) => {
                inner.confirming.set(None);
                inner.message.set_text(&problem);
                inner.feedback.play(Sound::Error);
            }
        }
    }

    fn run(&self, action: &Action) -> Result<(), String> {
        let inner = &self.0;
        let hypr = inner.hypr.as_ref();
        match action {
            Action::Launch { exec, terminal } => {
                let command = if *terminal {
                    let term =
                        actions::find_terminal(&inner.terminal.borrow()).ok_or_else(|| {
                            "No terminal emulator found. Set launcher.terminal in config.toml."
                                .to_string()
                        })?;
                    actions::in_terminal(&term, exec)
                } else {
                    exec.clone()
                };
                actions::spawn_detached(&command, hypr)
            }
            Action::FocusWindow { address } => match hypr {
                Some(hypr) => hypr
                    .run(Dispatch::FocusWindow(address))
                    .map_err(|e| e.to_string()),
                None => Err("Switching windows needs Hyprland.".into()),
            },
            Action::OpenPath(path) => actions::open_path(path, hypr),
            Action::OpenUrl(url) => actions::open(url, hypr),
            Action::Copy(text) => {
                inner.window.clipboard().set_text(text);
                Ok(())
            }
            Action::RestoreClip(line) => actions::restore_clip(line),
            Action::Shell(command) => actions::spawn_detached(command, hypr),
            Action::Internal(which) => {
                if *which == Internal::Logout {
                    return actions::logout(hypr);
                }
                if let Some(callback) = inner.on_internal.borrow().as_ref() {
                    callback(*which);
                }
                Ok(())
            }
        }
    }
}

fn with(weak: &Weak<Inner>, f: impl FnOnce(&Overlay)) {
    if let Some(inner) = weak.upgrade() {
        f(&Overlay(inner));
    }
}

/// Shows an icon by theme name or file path, falling back to a generic
/// icon when the theme has no match so a row never shows a broken image.
fn set_icon(image: &gtk::Image, icon: Option<&str>) {
    const FALLBACK: &str = "application-x-executable-symbolic";
    match icon {
        Some(path) if path.starts_with('/') => image.set_from_file(Some(path)),
        Some(name) => {
            let known = gdk::Display::default()
                .map(|d| gtk::IconTheme::for_display(&d).has_icon(name))
                .unwrap_or(false);
            image.set_icon_name(Some(if known { name } else { FALLBACK }));
        }
        None => image.set_icon_name(Some(FALLBACK)),
    }
}

fn build_row(item: &Item) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let content = gtk::Box::new(gtk::Orientation::Horizontal, px(12));
    content.add_css_class("result");

    // Emoji carry their own picture in the title.
    if item.kind != Kind::Emoji {
        let icon = gtk::Image::new();
        icon.set_pixel_size(px(20));
        set_icon(&icon, item.icon.as_deref());
        content.append(&icon);
    }

    let title = gtk::Label::new(Some(&item.title));
    title.add_css_class("result-title");
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_ellipsize(pango::EllipsizeMode::End);
    content.append(&title);

    if !item.subtitle.is_empty() {
        let meta = gtk::Label::new(Some(&item.subtitle));
        meta.add_css_class("result-meta");
        meta.set_ellipsize(pango::EllipsizeMode::Start);
        meta.set_max_width_chars(26);
        content.append(&meta);
    }

    row.set_child(Some(&content));
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_description() {
        let cases = [
            (
                Action::Launch {
                    exec: "firefox".into(),
                    terminal: false,
                },
                "Start",
                "firefox",
            ),
            (
                Action::Launch {
                    exec: "htop".into(),
                    terminal: true,
                },
                "Start in a terminal",
                "htop",
            ),
            (
                Action::FocusWindow {
                    address: "0x1".into(),
                },
                "Switch to this window",
                "",
            ),
            (Action::OpenPath("/a/b.txt".into()), "Open", "/a/b.txt"),
            (
                Action::OpenUrl("https://x.y".into()),
                "Open in the browser",
                "https://x.y",
            ),
            (Action::Copy("84".into()), "Copy to the clipboard", "84"),
            (
                Action::RestoreClip("7\ttext".into()),
                "Copy back to the clipboard",
                "",
            ),
            (
                Action::Shell("systemctl suspend".into()),
                "Run",
                "systemctl suspend",
            ),
            (
                Action::Internal(Internal::Reload),
                "Re-read config.toml",
                "",
            ),
            (Action::Internal(Internal::Quit), "Stop Scuttle", ""),
            (Action::Internal(Internal::Logout), "End this session", ""),
            (
                Action::Internal(Internal::ToggleDnd),
                "Turn do not disturb on or off",
                "",
            ),
        ];
        for (action, verb, extra) in cases {
            assert_eq!(describe(&action), (verb, extra.to_string()));
        }
    }
}
