//! The settings window: every setting from the config file with a
//! control beside it.
//!
//! The window keeps no state of its own. Each change is written to
//! `config.toml` straight away, the running shell picks the file up as
//! it does for a hand edit, and each row shows the line it wrote. So
//! the window and a text editor are two views of the same file.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use scuttle_core::config::Config;
use scuttle_core::edit::{self, Value};
use scuttle_core::schema::{Kind, Section, Setting, SECTIONS};
use scuttle_core::Rgba;

use crate::scale::px;

/// The window's title. The Hyprland rule that floats the window
/// matches on it.
pub const TITLE: &str = "Scuttle Settings";

/// How a number is shown beside its slider.
pub fn show_number(kind: Kind, value: f64) -> String {
    match kind {
        Kind::Integer { unit: "", .. } => format!("{}", value.round() as i64),
        Kind::Integer { unit, .. } => format!("{} {unit}", value.round() as i64),
        _ => format!("{value:.2}"),
    }
}

/// A path as people write it, with `~` for the home folder.
pub fn show_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// What a text box should hold for a value.
pub fn show_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Str(text)) => text.clone(),
        Some(Value::List(items)) => items.join(", "),
        Some(other) => other.written(),
        None => String::new(),
    }
}

/// Turns what was typed into the value to store. `None` means the
/// setting should be removed from the file.
pub fn typed_value(setting: &Setting, typed: &str) -> Option<Value> {
    let typed = typed.trim();
    match setting.kind {
        Kind::List => Some(Value::List(edit::parse_list(typed))),
        _ if typed.is_empty() && setting.optional => None,
        Kind::Colour => {
            // Accept the colour with or without its #.
            let hex = typed.trim_start_matches('#');
            Some(Value::Str(format!("#{hex}")))
        }
        _ => Some(Value::Str(typed.to_string())),
    }
}

/// The control on a row.
enum Control {
    Switch(gtk::Switch),
    /// One toggle button per choice, in the order of the schema.
    Segments(Vec<gtk::ToggleButton>),
    Slider {
        scale: gtk::Scale,
        readout: gtk::Label,
    },
    Text(gtk::Entry),
    Colour {
        entry: gtk::Entry,
        swatch: gtk::DrawingArea,
        shown: Rc<Cell<Option<Rgba>>>,
    },
}

/// One setting as drawn.
struct Row {
    setting: &'static Setting,
    control: Control,
    /// The config line this row writes.
    line: gtk::Label,
    reset: gtk::Button,
    problem: gtk::Label,
}

struct Inner {
    window: gtk::ApplicationWindow,
    config_path: PathBuf,
    title: gtk::Label,
    blurb: gtk::Label,
    page: gtk::Box,
    footer: gtk::Label,
    rows: RefCell<Vec<Row>>,
    /// True while controls are being filled in from the file, so that
    /// filling them in is not mistaken for the user changing them.
    filling: Cell<bool>,
    /// A slider change waiting to be written, so dragging does not
    /// rewrite the file on every pixel.
    pending: RefCell<Option<(glib::SourceId, &'static str, Value)>>,
}

/// The settings window. Cheap to clone.
#[derive(Clone)]
pub struct Settings(Rc<Inner>);

impl Settings {
    pub fn new(app: &gtk::Application, config_path: &Path) -> Settings {
        let window = gtk::ApplicationWindow::new(app);
        window.set_title(Some(TITLE));
        window.add_css_class("scuttle");
        window.add_css_class("settings");
        window.set_decorated(false);
        window.set_default_size(px(920), px(680));
        window.set_hide_on_close(true);

        let frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        window.set_child(Some(&frame));

        // Sections down the left.
        let side = gtk::Box::new(gtk::Orientation::Vertical, px(14));
        side.add_css_class("settings-side");
        side.set_width_request(px(210));
        let heading = gtk::Label::new(Some("Settings"));
        heading.add_css_class("settings-heading");
        heading.set_xalign(0.0);
        // One button per section. Tab walks past them without changing
        // the page, and Space or Enter opens the one in focus.
        let nav = gtk::Box::new(gtk::Orientation::Vertical, px(2));
        nav.add_css_class("settings-nav");
        let mut tabs: Vec<gtk::ToggleButton> = Vec::new();
        for section in SECTIONS {
            let tab = gtk::ToggleButton::new();
            let label = gtk::Label::new(Some(section.title));
            label.set_xalign(0.0);
            tab.set_child(Some(&label));
            if let Some(first) = tabs.first() {
                tab.set_group(Some(first));
            }
            nav.append(&tab);
            tabs.push(tab);
        }
        side.append(&heading);
        side.append(&nav);
        frame.append(&side);

        // The chosen section on the right.
        let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main.set_hexpand(true);
        let top = gtk::Box::new(gtk::Orientation::Horizontal, px(12));
        top.add_css_class("settings-top");
        let titles = gtk::Box::new(gtk::Orientation::Vertical, px(4));
        titles.set_hexpand(true);
        let title = gtk::Label::new(None);
        title.add_css_class("settings-title");
        title.set_xalign(0.0);
        let blurb = gtk::Label::new(None);
        blurb.add_css_class("settings-help");
        blurb.set_xalign(0.0);
        blurb.set_wrap(true);
        titles.append(&title);
        titles.append(&blurb);
        let close = gtk::Button::with_label("✕");
        close.add_css_class("icon");
        close.set_valign(gtk::Align::Start);
        close.set_tooltip_text(Some("Close (Esc)"));
        top.append(&titles);
        top.append(&close);
        main.append(&top);

        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.add_css_class("settings-page");
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&page));
        main.append(&scroller);

        let footer = gtk::Label::new(None);
        footer.add_css_class("settings-footer");
        footer.set_xalign(0.0);
        footer.set_wrap(true);
        footer.set_selectable(true);
        main.append(&footer);
        frame.append(&main);

        let settings = Settings(Rc::new(Inner {
            window,
            config_path: config_path.to_path_buf(),
            title,
            blurb,
            page,
            footer,
            rows: RefCell::new(Vec::new()),
            filling: Cell::new(false),
            pending: RefCell::new(None),
        }));

        let weak = Rc::downgrade(&settings.0);
        for (index, tab) in tabs.iter().enumerate() {
            let weak = weak.clone();
            tab.connect_toggled(move |tab| {
                if !tab.is_active() {
                    return;
                }
                if let Some(inner) = weak.upgrade() {
                    Settings(inner).show_section(index);
                }
            });
        }
        close.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    inner.window.set_visible(false);
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(inner) = weak.upgrade() {
                    inner.window.set_visible(false);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        settings.0.window.add_controller(keys);

        if let Some(first) = tabs.first() {
            first.set_active(true);
        }
        settings
    }

    pub fn is_open(&self) -> bool {
        self.0.window.is_visible()
    }

    pub fn show(&self) {
        self.refresh();
        self.0.window.present();
    }

    pub fn hide(&self) {
        self.flush();
        self.0.window.set_visible(false);
    }

    pub fn toggle(&self) {
        if self.is_open() {
            self.hide();
        } else {
            self.show();
        }
    }

    /// Builds the rows for one section.
    fn show_section(&self, index: usize) {
        let inner = &self.0;
        let Some(section) = SECTIONS.get(index) else {
            return;
        };
        self.flush();
        inner.title.set_text(section.title);
        inner.blurb.set_text(section.blurb);
        while let Some(child) = inner.page.first_child() {
            inner.page.remove(&child);
        }
        let rows: Vec<Row> = section
            .settings
            .iter()
            .map(|setting| self.build_row(section, setting))
            .collect();
        *inner.rows.borrow_mut() = rows;
        self.refresh();
    }

    fn build_row(&self, _section: &Section, setting: &'static Setting) -> Row {
        let inner = &self.0;
        let weak = Rc::downgrade(inner);
        let root = gtk::Box::new(gtk::Orientation::Vertical, px(6));
        root.add_css_class("settings-row");

        let upper = gtk::Box::new(gtk::Orientation::Horizontal, px(16));
        let words = gtk::Box::new(gtk::Orientation::Vertical, px(3));
        words.set_hexpand(true);
        words.set_valign(gtk::Align::Center);
        let label = gtk::Label::new(Some(setting.label));
        label.add_css_class("settings-label");
        label.set_xalign(0.0);
        words.append(&label);
        let mut help = setting.help.to_string();
        if setting.restart {
            if !help.is_empty() {
                help.push(' ');
            }
            help.push_str("Applies after Scuttle restarts.");
        }
        if !help.is_empty() {
            let help = gtk::Label::new(Some(&help));
            help.add_css_class("settings-help");
            help.set_xalign(0.0);
            help.set_wrap(true);
            help.set_max_width_chars(24);
            words.append(&help);
        }
        upper.append(&words);

        let control = self.build_control(setting, &weak);
        let widget: gtk::Widget = match &control {
            Control::Switch(switch) => switch.clone().upcast(),
            Control::Segments(buttons) => {
                let group = gtk::Box::new(gtk::Orientation::Horizontal, px(2));
                group.add_css_class("segmented");
                for button in buttons {
                    group.append(button);
                }
                group.upcast()
            }
            Control::Slider { scale, readout } => {
                let group = gtk::Box::new(gtk::Orientation::Horizontal, px(10));
                group.append(scale);
                group.append(readout);
                group.upcast()
            }
            Control::Text(entry) => entry.clone().upcast(),
            Control::Colour { entry, swatch, .. } => {
                let group = gtk::Box::new(gtk::Orientation::Horizontal, px(10));
                group.append(swatch);
                group.append(entry);
                group.upcast()
            }
        };
        widget.set_valign(gtk::Align::Center);
        widget.set_halign(gtk::Align::End);
        upper.append(&widget);
        root.append(&upper);

        // The line this row writes, and a way back to the default.
        let lower = gtk::Box::new(gtk::Orientation::Horizontal, px(10));
        let line = gtk::Label::new(None);
        line.add_css_class("settings-line");
        line.set_xalign(0.0);
        line.set_hexpand(true);
        line.set_ellipsize(pango::EllipsizeMode::End);
        // Selectable with the mouse, for copying into the file by
        // hand, but not a stop on the way when tabbing to a control.
        line.set_selectable(true);
        line.set_focusable(false);
        let reset = gtk::Button::with_label("Reset");
        reset.add_css_class("settings-reset");
        reset.set_tooltip_text(Some("Remove this line and go back to the default"));
        reset.connect_clicked({
            let weak = weak.clone();
            move |_| {
                if let Some(inner) = weak.upgrade() {
                    Settings(inner).apply(setting.key, None);
                }
            }
        });
        lower.append(&line);
        lower.append(&reset);
        root.append(&lower);

        let problem = gtk::Label::new(None);
        problem.add_css_class("msg-error");
        problem.set_xalign(0.0);
        problem.set_wrap(true);
        problem.set_max_width_chars(24);
        problem.set_visible(false);
        root.append(&problem);

        inner.page.append(&root);
        Row {
            setting,
            control,
            line,
            reset,
            problem,
        }
    }

    fn build_control(&self, setting: &'static Setting, weak: &Weak<Inner>) -> Control {
        let key = setting.key;
        match setting.kind {
            Kind::Toggle => {
                let switch = gtk::Switch::new();
                let weak = weak.clone();
                switch.connect_state_set(move |_, on| {
                    if let Some(inner) = weak.upgrade() {
                        if !inner.filling.get() {
                            Settings(inner).apply(key, Some(Value::Bool(on)));
                        }
                    }
                    glib::Propagation::Proceed
                });
                Control::Switch(switch)
            }
            Kind::Choice(choices) => {
                let mut buttons: Vec<gtk::ToggleButton> = Vec::new();
                for (value, label) in choices {
                    let button = gtk::ToggleButton::with_label(label);
                    if let Some(first) = buttons.first() {
                        button.set_group(Some(first));
                    }
                    let weak = weak.clone();
                    button.connect_toggled(move |button| {
                        if !button.is_active() {
                            return;
                        }
                        if let Some(inner) = weak.upgrade() {
                            if !inner.filling.get() {
                                Settings(inner).apply(key, Some(Value::Str(value.to_string())));
                            }
                        }
                    });
                    buttons.push(button);
                }
                Control::Segments(buttons)
            }
            Kind::Integer { .. } | Kind::Decimal { .. } => {
                let (min, max, step, whole) = match setting.kind {
                    Kind::Integer { min, max, step, .. } => {
                        (min as f64, max as f64, step as f64, true)
                    }
                    Kind::Decimal { min, max, step } => (min, max, step, false),
                    _ => unreachable!(),
                };
                let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
                scale.set_draw_value(false);
                scale.set_width_request(px(220));
                scale.set_round_digits(if whole { 0 } else { 2 });
                let readout = gtk::Label::new(None);
                readout.add_css_class("settings-readout");
                readout.set_width_chars(8);
                readout.set_xalign(1.0);
                let weak = weak.clone();
                let kind = setting.kind;
                scale.connect_value_changed({
                    let readout = readout.clone();
                    move |scale| {
                        // Snap to the step, so the file never gets 17.38.
                        let value = (scale.value() / step).round() * step;
                        readout.set_text(&show_number(kind, value));
                        if let Some(inner) = weak.upgrade() {
                            if !inner.filling.get() {
                                let value = if whole {
                                    Value::Int(value.round() as i64)
                                } else {
                                    Value::Float(value)
                                };
                                Settings(inner).apply_soon(key, value);
                            }
                        }
                    }
                });
                Control::Slider { scale, readout }
            }
            Kind::Text | Kind::List | Kind::Colour => {
                let entry = gtk::Entry::new();
                entry.add_css_class("settings-entry");
                let colour = setting.kind == Kind::Colour;
                entry.set_width_request(px(if colour { 130 } else { 300 }));
                if colour {
                    entry.set_placeholder_text(Some("theme colour"));
                }
                // Typing does not write on every key. Enter does, and
                // so does moving on to something else.
                let commit = {
                    let weak = weak.clone();
                    move |entry: &gtk::Entry| {
                        if let Some(inner) = weak.upgrade() {
                            if !inner.filling.get() {
                                let settings = Settings(inner);
                                let value = typed_value(setting, &entry.text());
                                if value != settings.current(key) {
                                    settings.apply(key, value);
                                }
                            }
                        }
                    }
                };
                entry.connect_activate(commit.clone());
                let focus = gtk::EventControllerFocus::new();
                focus.connect_leave({
                    let entry = entry.downgrade();
                    move |_| {
                        if let Some(entry) = entry.upgrade() {
                            commit(&entry);
                        }
                    }
                });
                entry.add_controller(focus);
                if !colour {
                    return Control::Text(entry);
                }
                let shown: Rc<Cell<Option<Rgba>>> = Rc::new(Cell::new(None));
                let swatch = gtk::DrawingArea::new();
                swatch.set_content_width(px(26));
                swatch.set_content_height(px(26));
                swatch.set_valign(gtk::Align::Center);
                swatch.set_draw_func({
                    let shown = Rc::clone(&shown);
                    move |_, cr, width, height| {
                        let Some(colour) = shown.get() else { return };
                        let radius = f64::from(width.min(height)) / 2.0;
                        cr.arc(
                            f64::from(width) / 2.0,
                            f64::from(height) / 2.0,
                            radius,
                            0.0,
                            std::f64::consts::TAU,
                        );
                        cr.set_source_rgb(
                            f64::from(colour.r) / 255.0,
                            f64::from(colour.g) / 255.0,
                            f64::from(colour.b) / 255.0,
                        );
                        let _ = cr.fill();
                    }
                });
                Control::Colour {
                    entry,
                    swatch,
                    shown,
                }
            }
        }
    }

    /// The value `key` has in the file as it stands.
    fn current(&self, key: &str) -> Option<Value> {
        let text = edit::read(&self.0.config_path).ok()?;
        let config = Config::parse(&text).ok()?;
        match edit::get(&config, key) {
            // An optional setting only has a value when the file says so.
            Some(value) if edit::is_set(&text, key) || !is_optional(key) => Some(value),
            _ => None,
        }
    }

    /// Fills every control on the page from the config file. Called
    /// when the window opens and whenever the file changes.
    pub fn refresh(&self) {
        let inner = &self.0;
        let text = match edit::read(&inner.config_path) {
            Ok(text) => text,
            Err(e) => {
                inner.footer.set_text(&format!(
                    "Could not read {}: {e}",
                    inner.config_path.display()
                ));
                return;
            }
        };
        let config = match Config::parse(&text) {
            Ok(config) => {
                inner.footer.remove_css_class("bad");
                let home = std::env::var_os("HOME").map(PathBuf::from);
                inner.footer.set_text(&format!(
                    "Changes are saved to {} as you make them.",
                    show_path(&inner.config_path, home.as_deref())
                ));
                config
            }
            Err(e) => {
                // Showing defaults over a broken file would be a lie,
                // and writing to it could make things worse.
                inner.footer.add_css_class("bad");
                inner.footer.set_text(&format!(
                    "{e} Fix the file by hand, and the settings here will come back."
                ));
                for row in inner.rows.borrow().iter() {
                    row.sensitive(false);
                }
                return;
            }
        };
        let theme_accent = config.theme().accent;

        inner.filling.set(true);
        for row in inner.rows.borrow().iter() {
            row.sensitive(true);
            let key = row.setting.key;
            let written = edit::is_set(&text, key);
            let value = edit::get(&config, key);
            // An optional setting left out of the file has no value of
            // its own to show.
            let shown = if row.setting.optional && !written {
                None
            } else {
                value.clone()
            };
            row.fill(shown.as_ref(), theme_accent);
            row.line.set_text(&edit::line(key, shown.as_ref()));
            // Hidden but still taking its space, so rows do not jump
            // when a setting is first changed.
            row.reset.set_opacity(if written { 1.0 } else { 0.0 });
            row.reset.set_can_target(written);
            row.reset.set_focusable(written);
        }
        inner.filling.set(false);
    }

    /// Writes a slider's value shortly, unless it moves again first.
    fn apply_soon(&self, key: &'static str, value: Value) {
        let inner = &self.0;
        if let Some((timer, _, _)) = inner.pending.borrow_mut().take() {
            timer.remove();
        }
        let weak = Rc::downgrade(inner);
        let timer = glib::timeout_add_local_once(Duration::from_millis(220), move || {
            if let Some(inner) = weak.upgrade() {
                let pending = inner.pending.borrow_mut().take();
                if let Some((_, key, value)) = pending {
                    Settings(inner).apply(key, Some(value));
                }
            }
        });
        *inner.pending.borrow_mut() = Some((timer, key, value));
    }

    /// Writes any slider change still waiting.
    fn flush(&self) {
        let pending = self.0.pending.borrow_mut().take();
        if let Some((timer, key, value)) = pending {
            timer.remove();
            self.apply(key, Some(value));
        }
    }

    /// Changes one setting in the file. `None` removes it.
    fn apply(&self, key: &str, value: Option<Value>) {
        let inner = &self.0;
        let result = edit::read(&inner.config_path)
            .map_err(|e| format!("Could not read the config file: {e}"))
            .and_then(|text| match &value {
                Some(value) => edit::set(&text, key, value),
                None => edit::unset(&text, key),
            })
            .and_then(|text| {
                edit::write(&inner.config_path, &text)
                    .map_err(|e| format!("Could not save the config file: {e}"))
            });
        let problem = result.err();
        for row in inner.rows.borrow().iter() {
            if row.setting.key == key {
                row.problem.set_visible(problem.is_some());
                row.problem.set_text(problem.as_deref().unwrap_or(""));
            }
        }
        // Show the result either way: the new value, or the old one
        // back in place when the change was refused.
        if problem.is_none() {
            self.refresh();
        }
    }
}

fn is_optional(key: &str) -> bool {
    scuttle_core::schema::find(key).is_some_and(|setting| setting.optional)
}

impl Row {
    fn sensitive(&self, on: bool) {
        match &self.control {
            Control::Switch(switch) => switch.set_sensitive(on),
            Control::Segments(buttons) => buttons.iter().for_each(|b| b.set_sensitive(on)),
            Control::Slider { scale, .. } => scale.set_sensitive(on),
            Control::Text(entry) | Control::Colour { entry, .. } => entry.set_sensitive(on),
        }
        self.reset.set_sensitive(on);
    }

    /// Shows `value` in the control.
    fn fill(&self, value: Option<&Value>, theme_accent: Rgba) {
        match (&self.control, value) {
            (Control::Switch(switch), Some(Value::Bool(on))) => switch.set_active(*on),
            (Control::Segments(buttons), Some(Value::Str(chosen))) => {
                if let Kind::Choice(choices) = self.setting.kind {
                    for (button, (value, _)) in buttons.iter().zip(choices) {
                        if value == chosen {
                            button.set_active(true);
                        }
                    }
                }
            }
            (Control::Slider { scale, readout }, Some(value)) => {
                let number = match value {
                    Value::Int(v) => *v as f64,
                    Value::Float(v) => *v,
                    _ => return,
                };
                scale.set_value(number);
                readout.set_text(&show_number(self.setting.kind, number));
            }
            (Control::Text(entry), value) => {
                // Leave the box alone while it is being typed in.
                if !entry.has_focus() || entry.text().is_empty() {
                    entry.set_text(&show_text(value));
                }
            }
            (
                Control::Colour {
                    entry,
                    swatch,
                    shown,
                },
                value,
            ) => {
                if !entry.has_focus() {
                    entry.set_text(&show_text(value));
                }
                let colour = match value {
                    Some(Value::Str(hex)) => hex.parse().unwrap_or(theme_accent),
                    _ => theme_accent,
                };
                shown.set(Some(colour));
                swatch.queue_draw();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scuttle_core::schema::find;

    #[test]
    fn numbers_read_naturally_beside_their_slider() {
        let px = Kind::Integer {
            min: 0,
            max: 32,
            step: 1,
            unit: "px",
        };
        let count = Kind::Integer {
            min: 1,
            max: 20,
            step: 1,
            unit: "",
        };
        let decimal = Kind::Decimal {
            min: 0.75,
            max: 2.5,
            step: 0.05,
        };
        assert_eq!(show_number(px, 18.0), "18 px");
        assert_eq!(show_number(count, 4.0), "4");
        assert_eq!(show_number(decimal, 1.25), "1.25");
        assert_eq!(show_number(decimal, 1.2000000000000002), "1.20");
    }

    #[test]
    fn text_boxes_show_values_the_way_they_are_typed() {
        assert_eq!(show_text(Some(&Value::Str("Geist".into()))), "Geist");
        assert_eq!(
            show_text(Some(&Value::List(vec!["cpu".into(), "clock".into()]))),
            "cpu, clock"
        );
        assert_eq!(show_text(None), "");
    }

    #[test]
    fn typed_text_becomes_the_right_kind_of_value() {
        let font = find("appearance.font").unwrap();
        let accent = find("appearance.accent").unwrap();
        let palette = find("appearance.palette_file").unwrap();
        let left = find("bar.left").unwrap();
        let terminal = find("launcher.terminal").unwrap();

        assert_eq!(
            typed_value(font, " Inter "),
            Some(Value::Str("Inter".into()))
        );
        // An empty box on an optional setting removes it from the file.
        assert_eq!(typed_value(accent, ""), None);
        assert_eq!(typed_value(palette, "  "), None);
        // On any other setting, empty is a value like any other.
        assert_eq!(typed_value(terminal, ""), Some(Value::Str(String::new())));
        assert_eq!(
            typed_value(accent, "7c5cff"),
            Some(Value::Str("#7c5cff".into()))
        );
        assert_eq!(
            typed_value(accent, "#7c5cff"),
            Some(Value::Str("#7c5cff".into()))
        );
        assert_eq!(
            typed_value(left, "workspaces , clock"),
            Some(Value::List(vec!["workspaces".into(), "clock".into()]))
        );
        assert_eq!(typed_value(left, ""), Some(Value::List(Vec::new())));
    }

    #[test]
    fn paths_under_home_are_shown_with_a_tilde() {
        let home = Path::new("/home/sam");
        assert_eq!(
            show_path(
                Path::new("/home/sam/.config/scuttle/config.toml"),
                Some(home)
            ),
            "~/.config/scuttle/config.toml"
        );
        assert_eq!(
            show_path(Path::new("/etc/scuttle.toml"), Some(home)),
            "/etc/scuttle.toml"
        );
        assert_eq!(show_path(Path::new("/x/y"), None), "/x/y");
    }

    #[test]
    fn optional_settings_are_recognised_by_key() {
        assert!(is_optional("appearance.accent"));
        assert!(!is_optional("appearance.theme"));
        assert!(!is_optional("no.such"));
    }
}
