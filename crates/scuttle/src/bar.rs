//! The bar: one layer-shell surface per monitor, built from the modules
//! listed in the config.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use scuttle_core::config::{BarPosition, BarStyle, Config, Module};
use scuttle_hypr::{Dispatch, Hyprland};
use scuttle_launcher::engine::pretty_class;

use crate::msg::{DesktopState, Stats};

/// Workspace buttons and the ids they currently show.
struct Workspaces {
    container: gtk::Box,
    buttons: RefCell<Vec<(i32, gtk::Button)>>,
    /// How many numbered workspaces are always shown.
    always: i32,
    hypr: Option<Hyprland>,
}

impl Workspaces {
    fn new(always: u32, hypr: Option<Hyprland>) -> Workspaces {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        container.set_valign(gtk::Align::Center);
        Workspaces {
            container,
            buttons: RefCell::new(Vec::new()),
            always: always as i32,
            hypr,
        }
    }

    fn update(&self, desktop: &DesktopState) {
        let mut ids: Vec<i32> = (1..=self.always).collect();
        ids.extend(
            desktop
                .workspaces
                .iter()
                .filter(|w| !w.is_special())
                .map(|w| w.id),
        );
        ids.sort_unstable();
        ids.dedup();

        let current: Vec<i32> = self.buttons.borrow().iter().map(|(id, _)| *id).collect();
        if current != ids {
            let mut buttons = self.buttons.borrow_mut();
            for (_, button) in buttons.drain(..) {
                self.container.remove(&button);
            }
            for id in ids {
                let button = gtk::Button::with_label(&id.to_string());
                button.add_css_class("ws");
                button.set_tooltip_text(Some(&format!("Workspace {id}")));
                if let Some(hypr) = self.hypr.clone() {
                    button.connect_clicked(move |_| {
                        let _ = hypr.run(Dispatch::Workspace(id));
                    });
                }
                self.container.append(&button);
                buttons.push((id, button));
            }
        }

        for (id, button) in self.buttons.borrow().iter() {
            let occupied = desktop
                .workspaces
                .iter()
                .any(|w| w.id == *id && w.windows > 0);
            set_class(button, "active", *id == desktop.active_workspace);
            set_class(button, "occupied", occupied);
        }
    }
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

/// A "key value" pair in monospace, e.g. `cpu 12%`.
fn stat(key: &str) -> (gtk::Box, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    row.add_css_class("module");
    let key = gtk::Label::new(Some(key));
    key.add_css_class("key");
    let value = gtk::Label::new(Some("-"));
    value.add_css_class("value");
    row.append(&key);
    row.append(&value);
    (row, value)
}

/// One bar on one monitor.
pub struct Bar {
    window: gtk::ApplicationWindow,
    workspaces: Option<Rc<Workspaces>>,
    title: Option<(gtk::Label, gtk::Label)>,
    cpu: Option<gtk::Label>,
    memory: Option<gtk::Label>,
    network: Option<gtk::Image>,
    battery: Option<(gtk::Box, gtk::Label)>,
    notifications: Option<gtk::Button>,
    /// The pending clock tick, shared with the timer that re-arms it.
    clock_timer: Option<ClockTimer>,
}

type ClockTimer = Rc<RefCell<Option<glib::SourceId>>>;

impl Bar {
    /// Builds and shows a bar on `monitor`. Returns `None` when the
    /// config turns the bar off.
    pub fn new(
        app: &gtk::Application,
        monitor: &gdk::Monitor,
        config: &Config,
        hypr: Option<Hyprland>,
    ) -> Option<Bar> {
        let bar_config = &config.bar;
        let edge = match bar_config.position {
            BarPosition::Top => Edge::Top,
            BarPosition::Bottom => Edge::Bottom,
            BarPosition::None => return None,
        };
        let notch = bar_config.style == BarStyle::Notch;

        let window = gtk::ApplicationWindow::new(app);
        window.add_css_class("scuttle");
        window.init_layer_shell();
        window.set_namespace(Some("scuttle-bar"));
        window.set_layer(Layer::Top);
        window.set_monitor(Some(monitor));
        window.set_anchor(edge, true);
        if !notch {
            // Stretch across the screen. A notch stays centred instead.
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            let margin = bar_config.margin as i32;
            window.set_margin(edge, margin);
            window.set_margin(Edge::Left, margin.max(2) + 2);
            window.set_margin(Edge::Right, margin.max(2) + 2);
        }
        window.auto_exclusive_zone_enable();

        let mut bar = Bar {
            window: window.clone(),
            workspaces: None,
            title: None,
            cpu: None,
            memory: None,
            network: None,
            battery: None,
            notifications: None,
            clock_timer: None,
        };

        let section = |modules: &[Module], bar: &mut Bar, align: gtk::Align| -> gtk::Box {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
            row.set_halign(align);
            for module in modules {
                if let Some(widget) = bar.build(*module, config, hypr.clone()) {
                    row.append(&widget);
                }
            }
            row
        };

        let position_class = if edge == Edge::Bottom {
            "bottom"
        } else {
            "top"
        };
        match bar_config.style {
            BarStyle::Strip => {
                let root = gtk::CenterBox::new();
                root.add_css_class("bar");
                root.add_css_class(position_class);
                root.set_start_widget(Some(&section(
                    &bar_config.left,
                    &mut bar,
                    gtk::Align::Start,
                )));
                root.set_center_widget(Some(&section(
                    &bar_config.center,
                    &mut bar,
                    gtk::Align::Center,
                )));
                root.set_end_widget(Some(&section(&bar_config.right, &mut bar, gtk::Align::End)));
                window.set_child(Some(&root));
            }
            BarStyle::Islands => {
                let root = gtk::CenterBox::new();
                let island = |modules: &[Module], bar: &mut Bar, align: gtk::Align| {
                    let row = section(modules, bar, align);
                    row.add_css_class("bar");
                    row.add_css_class("island");
                    row.add_css_class(position_class);
                    // An island with nothing in it would draw an empty pill.
                    row.set_visible(row.first_child().is_some());
                    row
                };
                root.set_start_widget(Some(&island(&bar_config.left, &mut bar, gtk::Align::Start)));
                root.set_center_widget(Some(&island(
                    &bar_config.center,
                    &mut bar,
                    gtk::Align::Center,
                )));
                root.set_end_widget(Some(&island(&bar_config.right, &mut bar, gtk::Align::End)));
                window.set_child(Some(&root));
            }
            BarStyle::Notch => {
                // A notch is small, so it carries only the essentials
                // from each section, in order.
                let compact = [Module::Workspaces, Module::Clock, Module::Battery];
                let wanted: Vec<Module> = bar_config
                    .left
                    .iter()
                    .chain(&bar_config.center)
                    .chain(&bar_config.right)
                    .copied()
                    .filter(|m| compact.contains(m))
                    .collect();
                let root = section(&wanted, &mut bar, gtk::Align::Center);
                root.set_spacing(22);
                root.add_css_class("bar");
                root.add_css_class("notch");
                root.add_css_class(position_class);
                window.set_child(Some(&root));
            }
        }

        window.present();
        Some(bar)
    }

    fn build(
        &mut self,
        module: Module,
        config: &Config,
        hypr: Option<Hyprland>,
    ) -> Option<gtk::Widget> {
        match module {
            Module::Workspaces => {
                // Without a compositor to ask there is nothing to show.
                hypr.as_ref()?;
                let workspaces = Rc::new(Workspaces::new(config.bar.workspaces, hypr));
                let widget = workspaces.container.clone();
                self.workspaces = Some(workspaces);
                Some(widget.upcast())
            }
            Module::Title => {
                hypr.as_ref()?;
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let app = gtk::Label::new(None);
                app.add_css_class("bar-title");
                let title = gtk::Label::new(None);
                title.add_css_class("bar-subtitle");
                title.set_ellipsize(pango::EllipsizeMode::End);
                title.set_max_width_chars(48);
                row.append(&app);
                row.append(&title);
                self.title = Some((app, title));
                Some(row.upcast())
            }
            Module::Clock => {
                let label = gtk::Label::new(None);
                label.add_css_class("clock");
                let format = config.bar.clock_format.clone();
                set_clock(&label, &format);
                self.clock_timer = Some(schedule_clock(&label, format));
                Some(label.upcast())
            }
            Module::Cpu => {
                let (row, value) = stat("cpu");
                self.cpu = Some(value);
                Some(row.upcast())
            }
            Module::Memory => {
                let (row, value) = stat("mem");
                self.memory = Some(value);
                Some(row.upcast())
            }
            Module::Network => {
                let image = gtk::Image::new();
                image.set_pixel_size(16);
                image.add_css_class("module");
                self.network = Some(image.clone());
                Some(image.upcast())
            }
            Module::Battery => {
                let (row, value) = stat("bat");
                // Hidden until a battery is actually found, so desktop
                // PCs never show the widget.
                row.set_visible(false);
                self.battery = Some((row.clone(), value));
                Some(row.upcast())
            }
            Module::Notifications => {
                let button = gtk::Button::new();
                button.add_css_class("badge");
                button.set_valign(gtk::Align::Center);
                // Hidden until there is something to count.
                button.set_visible(false);
                self.notifications = Some(button.clone());
                Some(button.upcast())
            }
            // These need services that are not built yet.
            Module::Media | Module::Volume => None,
        }
    }

    pub fn update_desktop(&self, desktop: &DesktopState) {
        if let Some(workspaces) = &self.workspaces {
            workspaces.update(desktop);
        }
        if let Some((app, title)) = &self.title {
            app.set_text(&pretty_class(&desktop.active_class));
            // Showing "Firefox  Firefox" helps nobody.
            let redundant = desktop
                .active_title
                .eq_ignore_ascii_case(&desktop.active_class);
            title.set_text(if redundant { "" } else { &desktop.active_title });
        }
    }

    /// Shows how many notifications have not been looked at, or that
    /// "do not disturb" is on. With neither, the badge is hidden.
    pub fn update_notifications(&self, unseen: usize, do_not_disturb: bool) {
        let Some(button) = &self.notifications else {
            return;
        };
        button.set_visible(unseen > 0 || do_not_disturb);
        set_class(button, "muted", do_not_disturb);
        if do_not_disturb {
            button.set_label("dnd");
            button.set_tooltip_text(Some("Do not disturb is on"));
        } else {
            button.set_label(&unseen.min(99).to_string());
            button.set_tooltip_text(Some("Unread notifications. Click to clear."));
        }
    }

    /// Runs `callback` when the notification badge is clicked.
    pub fn connect_notifications_clicked(&self, callback: impl Fn() + 'static) {
        if let Some(button) = &self.notifications {
            button.connect_clicked(move |_| callback());
        }
    }

    pub fn update_stats(&self, stats: &Stats) {
        if let Some(label) = &self.cpu {
            label.set_text(&format!("{}%", stats.cpu));
        }
        if let (Some(label), Some(memory)) = (&self.memory, stats.memory) {
            label.set_text(&memory.used_label());
        }
        if let Some(image) = &self.network {
            image.set_icon_name(Some(stats.link.icon()));
        }
        if let Some((row, label)) = &self.battery {
            match stats.battery {
                None => row.set_visible(false),
                Some(battery) => {
                    row.set_visible(true);
                    let charging = if battery.plugged_in && battery.percent < 100 {
                        "+"
                    } else {
                        ""
                    };
                    label.set_text(&format!("{}%{charging}", battery.percent));
                    set_class(row, "warn", battery.low());
                }
            }
        }
    }

    /// Removes the bar from the screen.
    pub fn close(mut self) {
        if let Some(id) = self
            .clock_timer
            .take()
            .and_then(|slot| slot.borrow_mut().take())
        {
            id.remove();
        }
        self.window.close();
    }
}

fn set_clock(label: &gtk::Label, format: &str) {
    let text = glib::DateTime::now_local()
        .and_then(|now| now.format(format))
        .map(|s| s.to_string())
        .unwrap_or_default();
    label.set_text(&text);
}

/// Updates the clock exactly when the displayed text changes: on the
/// minute, or on the second if the format shows seconds. A clock that
/// only shows minutes wakes the process once a minute, not once a
/// second.
///
/// Each tick schedules the next one against the wall clock, so the
/// display never drifts. The returned slot always holds the pending
/// tick, which is what [`Bar::close`] cancels.
fn schedule_clock(label: &gtk::Label, format: String) -> ClockTimer {
    let shows_seconds = ["%S", "%T", "%s", "%X", "%c", "%r"]
        .iter()
        .any(|f| format.contains(f));
    let period: i64 = if shows_seconds { 1 } else { 60 };

    fn arm(label: gtk::Label, format: String, period: i64, slot: ClockTimer) {
        let now = glib::DateTime::now_local()
            .map(|d| d.to_unix())
            .unwrap_or(0);
        let wait = (period - now.rem_euclid(period)) as u64;
        let next = Rc::clone(&slot);
        let id = glib::timeout_add_local_once(Duration::from_millis(wait * 1000 + 30), move || {
            // This source has fired and is gone; forget its id before
            // anything could try to cancel it.
            next.borrow_mut().take();
            set_clock(&label, &format);
            arm(label, format, period, next);
        });
        *slot.borrow_mut() = Some(id);
    }

    let slot: ClockTimer = Rc::new(RefCell::new(None));
    arm(label.clone(), format, period, Rc::clone(&slot));
    slot
}
