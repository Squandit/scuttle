//! The on-screen display: a small pill that appears for a moment when
//! the volume or brightness changes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use scuttle_core::config::Config;
use scuttle_sys::audio::Volume;

/// How long the pill stays after the last change.
const LINGER: Duration = Duration::from_millis(1400);

/// What the pill is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gauge {
    Volume(Volume),
    /// Brightness from 0 to 100.
    Brightness(u32),
}

impl Gauge {
    /// The symbolic icon for this reading.
    pub fn icon(self) -> &'static str {
        match self {
            Gauge::Volume(v) if v.muted || v.percent == 0 => "audio-volume-muted-symbolic",
            Gauge::Volume(v) if v.percent < 34 => "audio-volume-low-symbolic",
            Gauge::Volume(v) if v.percent < 67 => "audio-volume-medium-symbolic",
            Gauge::Volume(_) => "audio-volume-high-symbolic",
            Gauge::Brightness(_) => "display-brightness-symbolic",
        }
    }

    /// How full the bar is, 0.0 to 1.0.
    pub fn fraction(self) -> f64 {
        let percent = match self {
            Gauge::Volume(v) => v.audible_percent(),
            Gauge::Brightness(p) => p,
        };
        (percent.min(100) as f64) / 100.0
    }

    /// The text beside the bar.
    pub fn label(self) -> String {
        match self {
            Gauge::Volume(v) if v.muted => "Muted".to_string(),
            Gauge::Volume(v) => format!("{}%", v.percent),
            Gauge::Brightness(p) => format!("{p}%"),
        }
    }
}

struct Inner {
    window: gtk::ApplicationWindow,
    pill: gtk::Box,
    icon: gtk::Image,
    bar: gtk::ProgressBar,
    label: gtk::Label,
    exit_ms: Cell<u32>,
    /// The pending "hide now" timer.
    timer: RefCell<Option<glib::SourceId>>,
    showing: Cell<bool>,
}

/// The volume and brightness popup. Cheap to clone.
#[derive(Clone)]
pub struct Osd(Rc<Inner>);

impl Osd {
    pub fn new(app: &gtk::Application, config: &Config) -> Osd {
        let window = gtk::ApplicationWindow::new(app);
        window.add_css_class("scuttle");
        window.init_layer_shell();
        window.set_namespace(Some("scuttle-osd"));
        window.set_layer(Layer::Overlay);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_anchor(Edge::Bottom, true);
        window.set_margin(Edge::Bottom, 80);
        // Clicks go straight through to whatever is underneath.
        window.set_can_target(false);

        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        pill.add_css_class("osd");
        pill.set_width_request(400);

        let icon = gtk::Image::new();
        icon.set_pixel_size(20);
        let bar = gtk::ProgressBar::new();
        bar.set_hexpand(true);
        bar.set_valign(gtk::Align::Center);
        let label = gtk::Label::new(None);
        label.add_css_class("osd-value");
        label.set_width_chars(5);
        label.set_xalign(1.0);

        pill.append(&icon);
        pill.append(&bar);
        pill.append(&label);
        window.set_child(Some(&pill));

        let osd = Osd(Rc::new(Inner {
            window,
            pill,
            icon,
            bar,
            label,
            exit_ms: Cell::new(0),
            timer: RefCell::new(None),
            showing: Cell::new(false),
        }));
        osd.apply_config(config, false);
        osd
    }

    pub fn apply_config(&self, config: &Config, on_battery: bool) {
        self.0
            .exit_ms
            .set(config.effective_motion(on_battery).exit_ms());
    }

    /// Shows `gauge`, or updates the pill if it is already up, and
    /// restarts the countdown to hiding it.
    pub fn show(&self, gauge: Gauge) {
        let inner = &self.0;
        inner.icon.set_icon_name(Some(gauge.icon()));
        inner.bar.set_fraction(gauge.fraction());
        inner.label.set_text(&gauge.label());

        if !inner.showing.replace(true) {
            inner.window.set_visible(true);
            let pill = inner.pill.clone();
            glib::timeout_add_local_once(Duration::from_millis(16), move || {
                pill.add_css_class("shown");
            });
        } else {
            // A change during the fade-out brings it straight back.
            inner.pill.add_css_class("shown");
        }

        if let Some(timer) = inner.timer.borrow_mut().take() {
            timer.remove();
        }
        let weak = Rc::downgrade(inner);
        let timer = glib::timeout_add_local_once(LINGER, move || {
            let Some(inner) = weak.upgrade() else { return };
            inner.timer.borrow_mut().take();
            inner.showing.set(false);
            inner.pill.remove_css_class("shown");
            let weak = Rc::downgrade(&inner);
            let unmap = move || {
                if let Some(inner) = weak.upgrade() {
                    // Unless something was shown again meanwhile.
                    if !inner.showing.get() {
                        inner.window.set_visible(false);
                    }
                }
            };
            match inner.exit_ms.get() {
                0 => unmap(),
                exit => {
                    glib::timeout_add_local_once(Duration::from_millis(exit as u64 + 20), unmap);
                }
            }
        });
        *inner.timer.borrow_mut() = Some(timer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(percent: u32, muted: bool) -> Gauge {
        Gauge::Volume(Volume { percent, muted })
    }

    #[test]
    fn volume_icon_follows_the_level() {
        assert_eq!(vol(0, false).icon(), "audio-volume-muted-symbolic");
        assert_eq!(vol(80, true).icon(), "audio-volume-muted-symbolic");
        assert_eq!(vol(20, false).icon(), "audio-volume-low-symbolic");
        assert_eq!(vol(50, false).icon(), "audio-volume-medium-symbolic");
        assert_eq!(vol(90, false).icon(), "audio-volume-high-symbolic");
        assert_eq!(Gauge::Brightness(40).icon(), "display-brightness-symbolic");
    }

    #[test]
    fn muted_draws_an_empty_bar_and_says_so() {
        assert_eq!(vol(70, true).fraction(), 0.0);
        assert_eq!(vol(70, true).label(), "Muted");
        assert_eq!(vol(70, false).fraction(), 0.7);
        assert_eq!(vol(70, false).label(), "70%");
    }

    #[test]
    fn the_bar_never_overflows() {
        assert_eq!(vol(140, false).fraction(), 1.0);
        assert_eq!(vol(140, false).label(), "140%");
        assert_eq!(Gauge::Brightness(100).fraction(), 1.0);
    }
}
