//! Notification toasts: a small layer surface in a corner of the screen
//! that exists only while there is something to show.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, glib, pango};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use scuttle_core::config::{Config, Corner};
use scuttle_notify::{Change, CloseReason, Handle, Notification, Urgency};
use scuttle_sound::{Feedback, Sound};

const WIDTH: i32 = 400;

/// One toast on screen.
struct Card {
    id: u32,
    root: gtk::Box,
    /// The pending "time is up" timer. Shared with the timer itself so
    /// it can forget its own id once it has fired.
    timer: Rc<RefCell<Option<glib::SourceId>>>,
}

struct Inner {
    window: gtk::ApplicationWindow,
    stack: gtk::Box,
    cards: RefCell<Vec<Card>>,
    handle: Handle,
    feedback: Rc<Feedback>,
    default_ms: Cell<u32>,
    exit_ms: Cell<u32>,
    max_visible: Cell<usize>,
    corner: Cell<Corner>,
    /// Called whenever the number of unseen notifications may have
    /// changed, so the bar can update its badge.
    on_change: RefCell<Option<Box<dyn Fn()>>>,
}

/// The stack of notification toasts. Cheap to clone.
#[derive(Clone)]
pub struct Toasts(Rc<Inner>);

impl Toasts {
    pub fn new(
        app: &gtk::Application,
        config: &Config,
        handle: Handle,
        feedback: Rc<Feedback>,
    ) -> Toasts {
        let window = gtk::ApplicationWindow::new(app);
        window.add_css_class("scuttle");
        window.init_layer_shell();
        window.set_namespace(Some("scuttle-toasts"));
        // Above fullscreen windows: a notification that hides behind a
        // video is not much of a notification.
        window.set_layer(Layer::Overlay);
        window.set_keyboard_mode(KeyboardMode::None);
        window.set_default_size(WIDTH, 1);

        let stack = gtk::Box::new(gtk::Orientation::Vertical, 10);
        stack.set_width_request(WIDTH);
        window.set_child(Some(&stack));

        let toasts = Toasts(Rc::new(Inner {
            window,
            stack,
            cards: RefCell::new(Vec::new()),
            handle,
            feedback,
            default_ms: Cell::new(5000),
            exit_ms: Cell::new(0),
            max_visible: Cell::new(4),
            corner: Cell::new(Corner::TopRight),
            on_change: RefCell::new(None),
        }));
        toasts.apply_config(config, false);
        toasts
    }

    pub fn on_change(&self, callback: impl Fn() + 'static) {
        *self.0.on_change.borrow_mut() = Some(Box::new(callback));
    }

    fn changed(&self) {
        if let Some(callback) = self.0.on_change.borrow().as_ref() {
            callback();
        }
    }

    pub fn apply_config(&self, config: &Config, on_battery: bool) {
        let inner = &self.0;
        let settings = &config.notifications;
        inner.default_ms.set(settings.timeout_ms);
        inner.max_visible.set(settings.max_visible);
        inner
            .exit_ms
            .set(config.effective_motion(on_battery).exit_ms());
        inner.corner.set(settings.position);

        let corner = settings.position;
        let window = &inner.window;
        window.set_anchor(Edge::Top, corner.is_top());
        window.set_anchor(Edge::Bottom, !corner.is_top());
        window.set_anchor(Edge::Right, corner.is_right());
        window.set_anchor(Edge::Left, !corner.is_right());
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_margin(edge, 12);
        }
        // New toasts slide in from the side of the screen they sit on.
        if corner.is_right() {
            inner.stack.remove_css_class("from-left");
        } else {
            inner.stack.add_css_class("from-left");
        }
    }

    /// Applies a change reported by the notification service.
    pub fn apply(&self, change: Change) {
        match change {
            Change::Show(notification) => self.show(notification),
            Change::Replace(notification) => self.replace(notification),
            Change::Hide(id) => self.remove_card(id),
        }
        self.changed();
    }

    fn show(&self, notification: Notification) {
        let inner = &self.0;
        // A volume popup already makes itself heard.
        if notification.sync_tag.is_none() {
            inner.feedback.play(Sound::Notify);
        }

        // Keep the stack short: the oldest toast that is not critical
        // makes room, and stays available in the history.
        while inner.cards.borrow().len() >= inner.max_visible.get() {
            let victim = {
                let cards = inner.cards.borrow();
                let store = inner.handle.store().lock().ok();
                let urgency = |id: u32| {
                    store
                        .as_ref()
                        .and_then(|s| s.get(id))
                        .map(|n| n.urgency)
                        .unwrap_or_default()
                };
                cards
                    .iter()
                    .find(|c| urgency(c.id) != Urgency::Critical)
                    .or(cards.first())
                    .map(|c| c.id)
            };
            match victim {
                Some(id) => self.close(id, CloseReason::Expired),
                None => break,
            }
        }

        let card = self.build_card(&notification);
        // Newest nearest the corner.
        if inner.corner.get().is_top() {
            inner.stack.prepend(&card.root);
        } else {
            inner.stack.append(&card.root);
        }
        self.arm_timer(&card, &notification);
        inner.cards.borrow_mut().push(card);
        inner.window.set_visible(true);
    }

    fn replace(&self, notification: Notification) {
        let inner = &self.0;
        let position = inner
            .cards
            .borrow()
            .iter()
            .position(|c| c.id == notification.id);
        let Some(position) = position else {
            return self.show(notification);
        };
        let fresh = self.build_card(&notification);
        // Already on screen, so no entrance animation.
        fresh.root.add_css_class("shown");
        let old = std::mem::replace(&mut inner.cards.borrow_mut()[position], fresh);
        if let Some(timer) = old.timer.borrow_mut().take() {
            timer.remove();
        }
        let cards = inner.cards.borrow();
        let fresh = &cards[position];
        fresh.root.insert_after(&inner.stack, Some(&old.root));
        inner.stack.remove(&old.root);
        self.arm_timer(fresh, &notification);
    }

    /// Closes a toast because of something the user or a timer did, and
    /// tells the app that sent it.
    fn close(&self, id: u32, reason: CloseReason) {
        self.0.handle.close(id, reason);
        self.remove_card(id);
        self.changed();
    }

    /// Dismisses every toast on screen.
    pub fn dismiss_all(&self) {
        let ids: Vec<u32> = self.0.cards.borrow().iter().map(|c| c.id).collect();
        for id in ids {
            self.close(id, CloseReason::Dismissed);
        }
    }

    /// Takes a toast off the screen, animating it out.
    fn remove_card(&self, id: u32) {
        let inner = &self.0;
        let position = inner.cards.borrow().iter().position(|c| c.id == id);
        let Some(position) = position else { return };
        let card = inner.cards.borrow_mut().remove(position);
        if let Some(timer) = card.timer.borrow_mut().take() {
            timer.remove();
        }
        card.root.remove_css_class("shown");
        card.root.set_can_target(false);

        let weak = Rc::downgrade(inner);
        let root = card.root.clone();
        let finish = move || {
            if let Some(inner) = weak.upgrade() {
                inner.stack.remove(&root);
                if inner.cards.borrow().is_empty() && inner.stack.first_child().is_none() {
                    inner.window.set_visible(false);
                }
            }
        };
        match inner.exit_ms.get() {
            0 => finish(),
            exit => {
                glib::timeout_add_local_once(Duration::from_millis(exit as u64 + 20), finish);
            }
        }
    }

    fn arm_timer(&self, card: &Card, notification: &Notification) {
        let Some(millis) = notification.display_millis(self.0.default_ms.get()) else {
            return;
        };
        start_timer(Rc::downgrade(&self.0), &card.timer, card.id, millis);
    }

    fn build_card(&self, notification: &Notification) -> Card {
        let id = notification.id;
        let weak = Rc::downgrade(&self.0);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.add_css_class("toast");
        if notification.urgency == Urgency::Critical {
            root.add_css_class("critical");
        }

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 14);
        if let Some(image) = icon_for(&notification.icon) {
            top.append(&image);
        }
        let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let summary = gtk::Label::new(Some(&notification.summary));
        summary.add_css_class("toast-title");
        summary.set_xalign(0.0);
        summary.set_ellipsize(pango::EllipsizeMode::End);
        text.append(&summary);
        if !notification.body.is_empty() {
            let body = gtk::Label::new(Some(&notification.body));
            body.add_css_class("toast-body");
            body.set_xalign(0.0);
            body.set_wrap(true);
            body.set_wrap_mode(pango::WrapMode::WordChar);
            body.set_lines(3);
            body.set_ellipsize(pango::EllipsizeMode::End);
            text.append(&body);
        }
        top.append(&text);
        root.append(&top);

        if let Some(level) = notification.progress {
            let bar = gtk::ProgressBar::new();
            bar.set_fraction(level as f64 / 100.0);
            root.append(&bar);
        }

        let buttons: Vec<_> = notification.buttons().cloned().collect();
        if !buttons.is_empty() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_homogeneous(true);
            for (index, action) in buttons.into_iter().take(3).enumerate() {
                let button = gtk::Button::with_label(&action.label);
                button.add_css_class(if index == 0 { "primary" } else { "quiet" });
                let weak = weak.clone();
                button.connect_clicked(move |_| {
                    if let Some(inner) = weak.upgrade() {
                        inner.handle.invoke(id, &action.key);
                        Toasts(inner).close(id, CloseReason::Dismissed);
                    }
                });
                row.append(&button);
            }
            root.append(&row);
        }

        // Clicking the toast itself runs its default action, if it has
        // one, and dismisses it either way.
        let has_default = notification.default_action().is_some();
        let click = gtk::GestureClick::new();
        click.connect_released({
            let weak = weak.clone();
            move |gesture, _, _, _| {
                if let Some(inner) = weak.upgrade() {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    if has_default {
                        inner.handle.invoke(id, "default");
                    }
                    Toasts(inner).close(id, CloseReason::Dismissed);
                }
            }
        });
        root.add_controller(click);

        // A toast being read should not vanish: pause its timer while
        // the pointer is over it.
        let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
        let remaining = notification.display_millis(self.0.default_ms.get());
        let hover = gtk::EventControllerMotion::new();
        hover.connect_enter({
            let timer = Rc::clone(&timer);
            move |_, _, _| {
                if let Some(id) = timer.borrow_mut().take() {
                    id.remove();
                }
            }
        });
        hover.connect_leave({
            let timer = Rc::clone(&timer);
            let weak = weak.clone();
            move |_| {
                if let Some(millis) = remaining {
                    // Give a fresh, shorter wait after the pointer leaves.
                    start_timer(weak.clone(), &timer, id, millis.min(2500));
                }
            }
        });
        root.add_controller(hover);

        // Added a moment later so the entrance animation has a state to
        // start from.
        let card_root = root.clone();
        glib::timeout_add_local_once(Duration::from_millis(16), move || {
            card_root.add_css_class("shown");
        });

        Card { id, root, timer }
    }
}

/// Starts (or restarts) the countdown that expires toast `id`.
fn start_timer(
    weak: Weak<Inner>,
    slot: &Rc<RefCell<Option<glib::SourceId>>>,
    id: u32,
    millis: u32,
) {
    if let Some(old) = slot.borrow_mut().take() {
        old.remove();
    }
    let slot_inner = Rc::clone(slot);
    let source = glib::timeout_add_local_once(Duration::from_millis(millis as u64), move || {
        // This timer has fired; its id must not be cancelled later.
        slot_inner.borrow_mut().take();
        if let Some(inner) = weak.upgrade() {
            Toasts(inner).close(id, CloseReason::Expired);
        }
    });
    *slot.borrow_mut() = Some(source);
}

/// An image for a notification icon given as a file path or icon name.
/// Returns `None` when there is nothing usable, so the text can take
/// the full width.
fn icon_for(icon: &str) -> Option<gtk::Image> {
    if icon.is_empty() {
        return None;
    }
    let image = gtk::Image::new();
    image.set_pixel_size(40);
    image.set_valign(gtk::Align::Start);
    image.add_css_class("toast-icon");
    if icon.starts_with('/') {
        if !std::path::Path::new(icon).is_file() {
            return None;
        }
        image.set_from_file(Some(icon));
    } else {
        let known = gdk::Display::default()
            .map(|d| gtk::IconTheme::for_display(&d).has_icon(icon))
            .unwrap_or(false);
        if !known {
            return None;
        }
        image.set_icon_name(Some(icon));
    }
    Some(image)
}
