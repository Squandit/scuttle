//! The tray on the bar: one icon per app that put one there, with its
//! menu in a popover.

use std::cell::RefCell;
use std::path::Path;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};
use scuttle_tray::{Icon, Item, MenuEntry, Status};

use crate::scale::px;

/// Something the user did to a tray icon, for the shell to pass on to
/// the app. All of these mean talking to another program, so the shell
/// does them off the UI thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayRequest {
    /// A plain click.
    Activate { key: String },
    /// A middle click.
    Secondary { key: String },
    Scroll {
        key: String,
        delta: i32,
        horizontal: bool,
    },
    /// The menu was asked for. `menu` is where the app keeps it, or
    /// `None` for an app that draws its own.
    Menu { key: String, menu: Option<String> },
    /// An entry in the menu was chosen.
    MenuClick { key: String, menu: String, id: i32 },
}

/// What a click on an icon should do.
pub fn click_request(item: &Item, button: u32) -> Option<TrayRequest> {
    let key = item.key.clone();
    let menu = || TrayRequest::Menu {
        key: item.key.clone(),
        menu: item.menu.clone(),
    };
    match button {
        // An item that is nothing but a menu opens it on any click.
        1 if item.is_menu => Some(menu()),
        1 => Some(TrayRequest::Activate { key }),
        2 => Some(TrayRequest::Secondary { key }),
        3 => Some(menu()),
        _ => None,
    }
}

/// The mark shown before a menu entry that can be ticked.
pub fn toggle_mark(entry: &MenuEntry) -> &'static str {
    match entry.toggle {
        Some(toggle) if toggle.on && toggle.radio => "●",
        Some(toggle) if toggle.on => "✓",
        _ => "",
    }
}

/// Called with what the user did to an icon.
type OnRequest = Rc<dyn Fn(TrayRequest)>;

struct Slot {
    item: Item,
    root: gtk::Box,
    image: gtk::Image,
}

struct Inner {
    container: gtk::Box,
    slots: RefCell<Vec<Slot>>,
    on_request: RefCell<Option<OnRequest>>,
    /// The icon whose menu has been asked for and not yet shown. With
    /// several bars, only the one that asked shows it.
    waiting: RefCell<Option<String>>,
    popover: RefCell<Option<gtk::Popover>>,
    /// The bar is at the bottom of the screen, so menus open upwards.
    bar_at_bottom: bool,
}

/// The row of tray icons. Cheap to clone.
#[derive(Clone)]
pub struct TrayRow(Rc<Inner>);

fn show_icon(image: &gtk::Image, icon: &Icon) {
    match icon {
        Icon::Named { name, .. } if name.starts_with('/') => image.set_from_file(Some(name)),
        Icon::Named { name, theme_path } => {
            // Some apps ship their tray icons in a folder of their own.
            if let (Some(path), Some(display)) = (theme_path, gdk::Display::default()) {
                let theme = gtk::IconTheme::for_display(&display);
                if !theme.search_path().iter().any(|p| p == Path::new(path)) {
                    theme.add_search_path(path);
                }
            }
            image.set_icon_name(Some(name));
        }
        Icon::Pixels(pixels) => {
            let bytes = glib::Bytes::from(&pixels.rgba[..]);
            let texture = gdk::MemoryTexture::new(
                pixels.width,
                pixels.height,
                gdk::MemoryFormat::R8g8b8a8,
                &bytes,
                (pixels.width * 4) as usize,
            );
            image.set_paintable(Some(&texture));
        }
        Icon::Missing => image.set_icon_name(Some("application-x-executable-symbolic")),
    }
}

impl TrayRow {
    pub fn new(bar_at_bottom: bool) -> TrayRow {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, px(4));
        container.add_css_class("tray");
        container.set_valign(gtk::Align::Center);
        // Hidden until an app puts an icon in it.
        container.set_visible(false);
        TrayRow(Rc::new(Inner {
            container,
            slots: RefCell::new(Vec::new()),
            on_request: RefCell::new(None),
            waiting: RefCell::new(None),
            popover: RefCell::new(None),
            bar_at_bottom,
        }))
    }

    pub fn widget(&self) -> gtk::Widget {
        self.0.container.clone().upcast()
    }

    /// Runs `callback` when the user does something to an icon.
    pub fn connect_requested(&self, callback: impl Fn(TrayRequest) + 'static) {
        *self.0.on_request.borrow_mut() = Some(Rc::new(callback));
    }

    fn request(inner: &Inner, request: TrayRequest) {
        let callback = inner.on_request.borrow().clone();
        if let Some(callback) = callback {
            callback(request);
        }
    }

    fn close_menu(&self) {
        if let Some(popover) = self.0.popover.borrow_mut().take() {
            popover.popdown();
            popover.unparent();
        }
    }

    /// Shows `items`, in order.
    pub fn update(&self, items: &[Item]) {
        let inner = &self.0;
        inner.container.set_visible(!items.is_empty());
        let same_icons = {
            let slots = inner.slots.borrow();
            slots.len() == items.len()
                && slots
                    .iter()
                    .zip(items)
                    .all(|(slot, item)| slot.item.key == item.key)
        };
        if !same_icons {
            // A menu belongs to an icon that may be about to go.
            self.close_menu();
            let mut slots = inner.slots.borrow_mut();
            for slot in slots.drain(..) {
                inner.container.remove(&slot.root);
            }
            for item in items {
                slots.push(self.build_slot(item));
            }
        }
        for (slot, item) in inner.slots.borrow_mut().iter_mut().zip(items) {
            if !same_icons || slot.item.icon != item.icon {
                show_icon(&slot.image, &item.icon);
            }
            let label = item.label();
            slot.root
                .set_tooltip_text((!label.is_empty()).then_some(label));
            if item.status == Status::NeedsAttention {
                slot.root.add_css_class("attention");
            } else {
                slot.root.remove_css_class("attention");
            }
            slot.item = item.clone();
        }
    }

    fn build_slot(&self, item: &Item) -> Slot {
        let inner = &self.0;
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("tray-icon");
        let image = gtk::Image::new();
        image.set_pixel_size(px(18));
        root.append(&image);
        inner.container.append(&root);

        let weak = Rc::downgrade(inner);
        let key = item.key.clone();
        // Looks the item up when the click happens, so it acts on what
        // the app says now, not what it said when the icon was made.
        let current = move |weak: &Weak<Inner>| -> Option<(Rc<Inner>, Item)> {
            let inner = weak.upgrade()?;
            let item = inner
                .slots
                .borrow()
                .iter()
                .find(|slot| slot.item.key == key)
                .map(|slot| slot.item.clone())?;
            Some((inner, item))
        };

        let click = gtk::GestureClick::new();
        // Every mouse button, not just the left one.
        click.set_button(0);
        click.connect_released({
            let weak = weak.clone();
            let current = current.clone();
            move |gesture, _, _, _| {
                let Some((inner, item)) = current(&weak) else {
                    return;
                };
                let Some(request) = click_request(&item, gesture.current_button()) else {
                    return;
                };
                if matches!(request, TrayRequest::Menu { .. }) {
                    *inner.waiting.borrow_mut() = Some(item.key.clone());
                }
                TrayRow::request(&inner, request);
            }
        });
        root.add_controller(click);

        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll.connect_scroll(move |_, dx, dy| {
            let Some((inner, item)) = current(&weak) else {
                return glib::Propagation::Proceed;
            };
            // Apps expect the units a mouse wheel reports: 120 a notch.
            let (amount, horizontal) = if dy != 0.0 { (dy, false) } else { (dx, true) };
            if amount != 0.0 {
                TrayRow::request(
                    &inner,
                    TrayRequest::Scroll {
                        key: item.key,
                        delta: (amount * 120.0).round() as i32,
                        horizontal,
                    },
                );
            }
            glib::Propagation::Stop
        });
        root.add_controller(scroll);

        Slot {
            item: item.clone(),
            root,
            image,
        }
    }

    /// Shows a menu that was asked for, under its icon. Does nothing
    /// on a bar that did not ask.
    pub fn show_menu(&self, key: &str, menu: &str, entries: Vec<MenuEntry>) {
        let inner = &self.0;
        if inner.waiting.borrow().as_deref() != Some(key) {
            return;
        }
        inner.waiting.borrow_mut().take();
        self.close_menu();
        if entries.is_empty() {
            return;
        }
        let Some(anchor) = inner
            .slots
            .borrow()
            .iter()
            .find(|slot| slot.item.key == key)
            .map(|slot| slot.root.clone())
        else {
            return;
        };

        let popover = gtk::Popover::new();
        popover.add_css_class("tray-menu");
        popover.set_has_arrow(false);
        popover.set_position(if inner.bar_at_bottom {
            gtk::PositionType::Top
        } else {
            gtk::PositionType::Bottom
        });
        popover.set_parent(&anchor);

        // The pages open so far: the menu itself, then each submenu
        // the user has gone into.
        let pages = Rc::new(RefCell::new(vec![entries]));
        render_page(
            &Rc::downgrade(inner),
            &popover,
            key.to_string(),
            menu.to_string(),
            pages,
        );
        popover.connect_closed({
            let weak = Rc::downgrade(inner);
            move |popover| {
                // Let the click that closed it finish before the
                // popover is taken apart.
                let popover = popover.clone();
                let weak = weak.clone();
                glib::idle_add_local_once(move || {
                    if let Some(inner) = weak.upgrade() {
                        let ours = inner.popover.borrow().as_ref() == Some(&popover);
                        if ours {
                            inner.popover.borrow_mut().take();
                            popover.unparent();
                        }
                    }
                });
            }
        });
        popover.popup();
        *inner.popover.borrow_mut() = Some(popover);
    }

    /// Forgets a menu request that came to nothing.
    pub fn menu_failed(&self, key: &str) {
        let mut waiting = self.0.waiting.borrow_mut();
        if waiting.as_deref() == Some(key) {
            waiting.take();
        }
    }

    /// Takes the row apart, as when the bar is rebuilt.
    pub fn close(&self) {
        self.close_menu();
    }
}

/// Fills the popover with the innermost open page.
fn render_page(
    weak: &Weak<Inner>,
    popover: &gtk::Popover,
    key: String,
    menu: String,
    pages: Rc<RefCell<Vec<Vec<MenuEntry>>>>,
) {
    let list = gtk::Box::new(gtk::Orientation::Vertical, px(1));
    let depth = pages.borrow().len();
    let entries = pages.borrow().last().cloned().unwrap_or_default();

    let row = |mark: &str, text: &str, more: bool| -> gtk::Button {
        let button = gtk::Button::new();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, px(8));
        let mark = gtk::Label::new(Some(mark));
        mark.add_css_class("mark");
        // A fixed column, so labels line up whether or not a row has
        // a tick.
        mark.set_width_request(px(14));
        let label = gtk::Label::new(Some(text));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        line.append(&mark);
        line.append(&label);
        if more {
            let arrow = gtk::Label::new(Some("›"));
            arrow.add_css_class("mark");
            line.append(&arrow);
        }
        button.set_child(Some(&line));
        button
    };

    if depth > 1 {
        let back = row("‹", "Back", false);
        back.connect_clicked({
            let weak = weak.clone();
            let popover = popover.downgrade();
            let (key, menu, pages) = (key.clone(), menu.clone(), Rc::clone(&pages));
            move |_| {
                pages.borrow_mut().pop();
                if let Some(popover) = popover.upgrade() {
                    render_page(
                        &weak,
                        &popover,
                        key.clone(),
                        menu.clone(),
                        Rc::clone(&pages),
                    );
                }
            }
        });
        list.append(&back);
        list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    }

    for entry in entries {
        if entry.separator {
            list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            continue;
        }
        let has_children = !entry.children.is_empty();
        let button = row(toggle_mark(&entry), &entry.label, has_children);
        button.set_sensitive(entry.enabled);
        let weak = weak.clone();
        let popover_weak = popover.downgrade();
        let (key, menu, pages) = (key.clone(), menu.clone(), Rc::clone(&pages));
        button.connect_clicked(move |_| {
            let Some(popover) = popover_weak.upgrade() else {
                return;
            };
            if has_children {
                pages.borrow_mut().push(entry.children.clone());
                render_page(
                    &weak,
                    &popover,
                    key.clone(),
                    menu.clone(),
                    Rc::clone(&pages),
                );
            } else {
                popover.popdown();
                if let Some(inner) = weak.upgrade() {
                    TrayRow::request(
                        &inner,
                        TrayRequest::MenuClick {
                            key: key.clone(),
                            menu: menu.clone(),
                            id: entry.id,
                        },
                    );
                }
            }
        });
        list.append(&button);
    }
    popover.set_child(Some(&list));
}

#[cfg(test)]
mod tests {
    use super::*;
    use scuttle_tray::menu::Toggle;

    fn item(is_menu: bool, menu: Option<&str>) -> Item {
        Item {
            key: ":1.5/StatusNotifierItem".into(),
            is_menu,
            menu: menu.map(str::to_string),
            ..Item::default()
        }
    }

    #[test]
    fn each_mouse_button_does_its_own_thing() {
        let chat = item(false, Some("/Menu"));
        let key = chat.key.clone();
        assert_eq!(
            click_request(&chat, 1),
            Some(TrayRequest::Activate { key: key.clone() })
        );
        assert_eq!(
            click_request(&chat, 2),
            Some(TrayRequest::Secondary { key: key.clone() })
        );
        assert_eq!(
            click_request(&chat, 3),
            Some(TrayRequest::Menu {
                key,
                menu: Some("/Menu".into())
            })
        );
        assert_eq!(click_request(&chat, 8), None);
    }

    #[test]
    fn an_item_that_is_only_a_menu_opens_it_on_a_plain_click() {
        let applet = item(true, Some("/Menu"));
        assert!(matches!(
            click_request(&applet, 1),
            Some(TrayRequest::Menu { .. })
        ));
    }

    #[test]
    fn an_app_that_draws_its_own_menu_is_asked_to() {
        let old_app = item(false, None);
        assert_eq!(
            click_request(&old_app, 3),
            Some(TrayRequest::Menu {
                key: old_app.key.clone(),
                menu: None
            })
        );
    }

    #[test]
    fn ticked_entries_get_a_mark() {
        let entry = |toggle| MenuEntry {
            toggle,
            ..MenuEntry::default()
        };
        assert_eq!(toggle_mark(&entry(None)), "");
        assert_eq!(
            toggle_mark(&entry(Some(Toggle {
                radio: false,
                on: true
            }))),
            "✓"
        );
        assert_eq!(
            toggle_mark(&entry(Some(Toggle {
                radio: true,
                on: true
            }))),
            "●"
        );
        assert_eq!(
            toggle_mark(&entry(Some(Toggle {
                radio: true,
                on: false
            }))),
            ""
        );
    }
}
