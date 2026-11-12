//! The running shell: owns the config, the stylesheet and every
//! surface, and reacts to messages from the background threads.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_channel::Sender;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use scuttle_core::config::{Config, SoundWhen};
use scuttle_core::ipc::{Playback, Request, Response, Step, Surface, Switch};
use scuttle_core::{css, paths};
use scuttle_hypr::Hyprland;
use scuttle_launcher::history::History;
use scuttle_launcher::Internal;
use scuttle_media::{Control, Controller, Track};
use scuttle_notify::Store;
use scuttle_sound::{Feedback, When};
use scuttle_sys::audio::{self, Volume, VolumeChange};
use scuttle_sys::brightness;

use crate::assistant::Panel;
use crate::bar::Bar;
use crate::msg::{DesktopState, Msg, Stats};
use crate::osd::{Gauge, Osd};
use crate::overlay::Overlay;
use crate::services::{self, SocketError};
use crate::settings::Settings;
use crate::toasts::Toasts;
use crate::tray::TrayRequest;

fn sound_when(setting: SoundWhen) -> When {
    match setting {
        SoundWhen::Never => When::Never,
        SoundWhen::Headphones => When::Headphones,
        SoundWhen::Always => When::Always,
    }
}

/// Turns off everything but the launcher, whatever the config says.
/// This is what `scuttle --launcher-only` runs with, for people who
/// want the launcher beside another bar and notification daemon.
pub fn launcher_only(mut config: Config) -> Config {
    config.bar.position = scuttle_core::config::BarPosition::None;
    config.notifications.enabled = false;
    config.assistant.enabled = false;
    config
}

pub struct Shell {
    app: gtk::Application,
    /// Only the launcher runs. See [`launcher_only`].
    launcher_only: bool,
    config_path: PathBuf,
    home: PathBuf,
    config: RefCell<Config>,
    provider: gtk::CssProvider,
    hypr: Option<Hyprland>,
    overlay: Overlay,
    feedback: Rc<Feedback>,
    /// `None` when notifications are turned off or another daemon has
    /// them.
    toasts: Option<Toasts>,
    notifications: Option<Arc<Mutex<Store>>>,
    osd: Osd,
    /// `None` when the assistant is turned off in the config.
    assistant: Option<Panel>,
    /// The settings window, built the first time it is opened.
    settings: RefCell<Option<Settings>>,
    volume: Cell<Option<Volume>>,
    /// `None` when there is no D-Bus session to find players on.
    media: Option<Controller>,
    track: RefCell<Option<Track>>,
    /// The system tray, once a bar has asked for one. `None` until
    /// then, and when there is no D-Bus session.
    tray: RefCell<Option<scuttle_tray::Tray>>,
    tray_items: RefCell<Vec<scuttle_tray::Item>>,
    bars: RefCell<Vec<Bar>>,
    bars_hidden: Cell<bool>,
    desktop: RefCell<DesktopState>,
    stats: RefCell<Stats>,
    tx: Sender<Msg>,
    /// Keeps the config file watcher alive.
    watcher: RefCell<Option<gio::FileMonitor>>,
    /// Watches the wallpaper palette while the theme follows it.
    palette_watcher: RefCell<Option<(PathBuf, gio::FileMonitor)>>,
    /// A handle to this shell for callbacks set up after start.
    this: RefCell<Weak<Shell>>,
    reload_pending: Cell<bool>,
}

impl Shell {
    /// Builds the shell and starts everything. Returns an error message
    /// when the shell cannot start at all.
    pub fn start(
        app: &gtk::Application,
        config_path: &Path,
        only_launcher: bool,
    ) -> Result<Rc<Shell>, String> {
        let Some(display) = gdk::Display::default() else {
            return Err(
                "No Wayland display found. Scuttle has to be started inside a Wayland session."
                    .into(),
            );
        };
        if !gtk4_layer_shell::is_supported() {
            return Err(
                "This compositor does not support the layer-shell protocol, which Scuttle needs. \
                 Hyprland, Sway, niri and river all do."
                    .into(),
            );
        }

        let (tx, rx) = async_channel::unbounded::<Msg>();
        match services::serve_ipc(&paths::socket_path(), tx.clone()) {
            Ok(()) => {}
            Err(SocketError::AlreadyRunning) => {
                return Err("Scuttle is already running in this session.".into());
            }
            Err(SocketError::Io(e)) => {
                return Err(format!(
                    "Could not open the control socket at {}: {e}",
                    paths::socket_path().display()
                ));
            }
        }

        let config = match Config::load(config_path) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("{e}\nStarting with the default settings instead.");
                Config::default()
            }
        };
        let config = if only_launcher {
            launcher_only(config)
        } else {
            config
        };

        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let hypr = Hyprland::from_env();
        let stats = services::initial_stats();
        let on_battery = stats.battery.map(|b| b.discharging()).unwrap_or(false);

        // The launcher fills its search box before it gets focus; GTK
        // would otherwise select that text and the next key would
        // replace it.
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_entry_select_on_focus(false);
            // Text in the assistant panel can be selected. Moving
            // focus onto it with Tab should not select all of it.
            settings.set_gtk_label_select_on_focus(false);
        }

        crate::scale::set(config.appearance.scale);

        let provider = gtk::CssProvider::new();
        provider.load_from_string(&css::stylesheet(&config.theme(), &config, on_battery));
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let history = History::load(&paths::data_dir().join("launcher-history"));
        let feedback = Rc::new(Feedback::new(
            sound_when(config.sound.feedback),
            config.sound.volume,
        ));
        let overlay = Overlay::new(
            app,
            &config,
            &home,
            history,
            hypr.clone(),
            Rc::clone(&feedback),
        );
        // Beside another bar, the launcher has no need to show the
        // clock and status itself.
        overlay.set_glance_allowed(!only_launcher);
        overlay.apply_config(&config, on_battery);
        overlay.update_stats(&stats);

        // Notifications. If another daemon already handles them, say so
        // once and carry on without.
        let mut toasts = None;
        let mut notifications = None;
        if config.notifications.enabled {
            let store = Arc::new(Mutex::new(Store::default()));
            let notify_tx = tx.clone();
            match scuttle_notify::serve(Arc::clone(&store), move |change| {
                let _ = notify_tx.send_blocking(Msg::Notification(change));
            }) {
                Ok(handle) => {
                    let stack = Toasts::new(app, &config, handle, Rc::clone(&feedback));
                    stack.apply_config(&config, on_battery);
                    toasts = Some(stack);
                    notifications = Some(store);
                }
                Err(reason) => eprintln!("{reason}"),
            }
        }

        // What is playing is shown on the bar, so with no bar there is
        // nothing to watch players for.
        let media_tx = tx.clone();
        let media = if only_launcher {
            None
        } else {
            match scuttle_media::watch(move |track| {
                let _ = media_tx.send_blocking(Msg::Media(track));
            }) {
                Ok(controller) => Some(controller),
                Err(reason) => {
                    eprintln!("{reason}");
                    None
                }
            }
        };

        let osd = Osd::new(app, &config);
        osd.apply_config(&config, on_battery);

        let assistant = config.assistant.enabled.then(|| {
            let panel = Panel::new(app, &config, Rc::clone(&feedback), tx.clone());
            panel.apply_config(&config, on_battery);
            panel
        });

        let shell = Rc::new(Shell {
            app: app.clone(),
            launcher_only: only_launcher,
            config_path: config_path.to_path_buf(),
            home,
            config: RefCell::new(config),
            provider,
            hypr,
            overlay,
            feedback,
            toasts,
            notifications,
            osd,
            assistant,
            settings: RefCell::new(None),
            volume: Cell::new(None),
            media,
            track: RefCell::new(None),
            tray: RefCell::new(None),
            tray_items: RefCell::new(Vec::new()),
            bars: RefCell::new(Vec::new()),
            bars_hidden: Cell::new(false),
            desktop: RefCell::new(DesktopState::default()),
            stats: RefCell::new(stats),
            tx: tx.clone(),
            watcher: RefCell::new(None),
            palette_watcher: RefCell::new(None),
            this: RefCell::new(Weak::new()),
            reload_pending: Cell::new(false),
        });

        shell.overlay.on_clipboard_wanted({
            let tx = tx.clone();
            move || services::load_clipboard(tx.clone())
        });

        *shell.this.borrow_mut() = Rc::downgrade(&shell);
        shell.watch_palette();

        if let Some(panel) = &shell.assistant {
            // A question typed into the launcher goes to the panel.
            shell.overlay.on_ask({
                let panel = panel.clone();
                move |question| {
                    if let Err(problem) = panel.ask(question) {
                        eprintln!("{problem}");
                    }
                }
            });
            // The assistant changes the desktop the same way the
            // `scuttle` command does, and can do nothing the command
            // cannot.
            let weak = Rc::downgrade(&shell);
            panel.on_desktop_action(move |args| {
                let shell = weak.upgrade().ok_or("Scuttle is shutting down.")?;
                match shell.handle_request(crate::request_from_args(args)?) {
                    Response::Error { message } => Err(message),
                    _ => Ok(()),
                }
            });
        }

        shell.overlay.on_internal({
            let weak = Rc::downgrade(&shell);
            move |which| {
                let Some(shell) = weak.upgrade() else { return };
                match which {
                    Internal::Reload => {
                        let _ = shell.reload();
                    }
                    Internal::Quit => shell.app.quit(),
                    Internal::Logout => {}
                    Internal::ToggleDnd => {
                        shell.set_do_not_disturb(Switch::Toggle);
                    }
                    Internal::Assistant => {
                        if let Some(panel) = &shell.assistant {
                            panel.show();
                        }
                    }
                    Internal::Settings => shell.settings().show(),
                }
            }
        });

        if let Some(toasts) = &shell.toasts {
            let weak = Rc::downgrade(&shell);
            toasts.on_change(move || {
                if let Some(shell) = weak.upgrade() {
                    shell.update_badges();
                }
            });
        }

        // Read the compositor once up front so the first frame of the
        // bar already has its workspaces.
        if let Some(hypr) = &shell.hypr {
            if let Ok(desktop) = services::snapshot(hypr) {
                shell.overlay.update_desktop(&desktop);
                if let Some(panel) = &shell.assistant {
                    panel.update_desktop(&desktop);
                }
                *shell.desktop.borrow_mut() = desktop;
            }
            services::watch_hyprland(hypr.clone(), tx.clone());
        }
        shell.rebuild_bars();

        services::watch_stats(tx.clone());
        services::watch_volume(tx.clone());
        services::scan_apps(tx.clone());
        shell.index_files();
        shell.watch_config();

        // Monitors coming and going.
        display.monitors().connect_items_changed({
            let weak = Rc::downgrade(&shell);
            move |_, _, _, _| {
                if let Some(shell) = weak.upgrade() {
                    shell.rebuild_bars();
                }
            }
        });

        glib::spawn_future_local({
            let weak = Rc::downgrade(&shell);
            async move {
                while let Ok(msg) = rx.recv().await {
                    match weak.upgrade() {
                        Some(shell) => shell.handle(msg),
                        None => break,
                    }
                }
            }
        });

        Ok(shell)
    }

    fn on_battery(&self) -> bool {
        self.stats
            .borrow()
            .battery
            .map(|b| b.discharging())
            .unwrap_or(false)
    }

    fn handle(&self, msg: Msg) {
        match msg {
            Msg::Ipc(request, reply) => {
                let _ = reply.send(self.handle_request(request));
            }
            Msg::Desktop(desktop) => {
                for bar in self.bars.borrow().iter() {
                    bar.update_desktop(&desktop);
                }
                self.overlay.update_desktop(&desktop);
                if let Some(panel) = &self.assistant {
                    panel.update_desktop(&desktop);
                }
                *self.desktop.borrow_mut() = desktop;
            }
            Msg::Stats(stats) => {
                let was_on_battery = self.on_battery();
                for bar in self.bars.borrow().iter() {
                    bar.update_stats(&stats);
                }
                self.overlay.update_stats(&stats);
                *self.stats.borrow_mut() = stats;
                // Plugging in or unplugging changes blur and motion.
                if was_on_battery != self.on_battery() {
                    self.restyle();
                }
            }
            Msg::Apps(apps) => self.overlay.set_apps(apps),
            Msg::Files(files) => self.overlay.set_files(files),
            Msg::Clipboard(history) => self.overlay.set_clipboard(history),
            Msg::NotificationsSeen => self.mark_notifications_seen(),
            Msg::Media(track) => {
                for bar in self.bars.borrow().iter() {
                    bar.update_media(track.as_ref());
                }
                *self.track.borrow_mut() = track;
            }
            Msg::MediaRequest(control) => {
                if let Err(problem) = self.control_media(control) {
                    eprintln!("{problem}");
                }
            }
            Msg::Volume(volume) => self.set_volume(volume),
            Msg::VolumeRequest(change) => {
                if let Err(problem) = self.change_volume(change) {
                    eprintln!("{problem}");
                }
            }
            Msg::Notification(change) => {
                if let Some(toasts) = &self.toasts {
                    toasts.apply(change);
                }
            }
            Msg::Tray(items) => {
                for bar in self.bars.borrow().iter() {
                    bar.update_tray(&items);
                }
                *self.tray_items.borrow_mut() = items;
            }
            Msg::TrayRequest(request) => self.tray_request(request),
            Msg::TrayMenu { key, menu, entries } => {
                if let Err(problem) = &entries {
                    eprintln!("{problem}");
                }
                for bar in self.bars.borrow().iter() {
                    bar.show_tray_menu(&key, &menu, entries.clone());
                }
            }
            Msg::Assistant(generation, msg) => {
                if let Some(panel) = &self.assistant {
                    panel.handle(generation, msg);
                }
            }
        }
    }

    /// Starts the system tray if the bar has a place for it and it is
    /// not running yet. Scuttle only becomes the tray that apps
    /// register with when the user has a tray to show them in.
    fn start_tray(&self) {
        let wanted = {
            let config = self.config.borrow();
            let bar = &config.bar;
            bar.position != scuttle_core::config::BarPosition::None
                && [&bar.left, &bar.center, &bar.right]
                    .iter()
                    .any(|modules| modules.contains(&scuttle_core::config::Module::Tray))
        };
        if !wanted || self.tray.borrow().is_some() {
            return;
        }
        let tx = self.tx.clone();
        match scuttle_tray::watch(move |items| {
            let _ = tx.send_blocking(Msg::Tray(items));
        }) {
            Ok(tray) => *self.tray.borrow_mut() = Some(tray),
            Err(reason) => eprintln!("{reason}"),
        }
    }

    /// Passes something the user did to a tray icon on to its app.
    /// The app may be slow to answer, so this happens on a thread of
    /// its own and never holds up the bar.
    fn tray_request(&self, request: TrayRequest) {
        let Some(tray) = self.tray.borrow().clone() else {
            return;
        };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let outcome = match request {
                TrayRequest::Activate { key } => tray.activate(&key, 0, 0),
                TrayRequest::Secondary { key } => tray.secondary_activate(&key, 0, 0),
                TrayRequest::Scroll {
                    key,
                    delta,
                    horizontal,
                } => tray.scroll(&key, delta, horizontal),
                TrayRequest::MenuClick { key, menu, id } => tray.click_menu(&key, &menu, id),
                // An app with no menu to describe draws its own.
                TrayRequest::Menu { key, menu: None } => tray.context_menu(&key, 0, 0),
                TrayRequest::Menu {
                    key,
                    menu: Some(menu),
                } => {
                    let entries = tray.menu(&key, &menu);
                    let _ = tx.send_blocking(Msg::TrayMenu { key, menu, entries });
                    Ok(())
                }
            };
            if let Err(problem) = outcome {
                eprintln!("{problem}");
            }
        });
    }

    /// The settings window, built on first use.
    fn settings(&self) -> Settings {
        self.settings
            .borrow_mut()
            .get_or_insert_with(|| Settings::new(&self.app, &self.config_path))
            .clone()
    }

    /// The assistant panel, or why there is none.
    fn panel(&self) -> Result<&Panel, Response> {
        self.assistant.as_ref().ok_or_else(|| Response::Error {
            message: if self.launcher_only {
                "Scuttle is running with --launcher-only, so there is no assistant panel.".into()
            } else {
                "The assistant is turned off. Set enabled = true under [assistant] in \
                 config.toml and restart Scuttle."
                    .into()
            },
        })
    }

    fn handle_request(&self, request: Request) -> Response {
        match request {
            Request::Ping => Response::Pong {
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            Request::Show {
                surface: Surface::Launcher,
            } => {
                self.overlay.show(None);
                Response::Ok
            }
            Request::Hide {
                surface: Surface::Launcher,
            } => {
                self.overlay.hide();
                Response::Ok
            }
            Request::Toggle {
                surface: Surface::Launcher,
            } => {
                self.overlay.toggle();
                Response::Ok
            }
            Request::Launcher { query } => {
                self.overlay.show(Some(&query));
                Response::Ok
            }
            Request::Show {
                surface: Surface::Assistant,
            } => match self.panel() {
                Ok(panel) => {
                    panel.show();
                    Response::Ok
                }
                Err(off) => off,
            },
            Request::Hide {
                surface: Surface::Assistant,
            } => match self.panel() {
                Ok(panel) => {
                    panel.hide();
                    Response::Ok
                }
                Err(off) => off,
            },
            Request::Toggle {
                surface: Surface::Assistant,
            } => match self.panel() {
                Ok(panel) => {
                    panel.toggle();
                    Response::Ok
                }
                Err(off) => off,
            },
            Request::Ask { message } => match self.panel() {
                Ok(panel) => match panel.ask(&message) {
                    Ok(()) => Response::Ok,
                    Err(message) => Response::Error { message },
                },
                Err(off) => off,
            },
            Request::Show {
                surface: Surface::Settings,
            } => {
                self.settings().show();
                Response::Ok
            }
            Request::Hide {
                surface: Surface::Settings,
            } => {
                if let Some(settings) = self.settings.borrow().as_ref() {
                    settings.hide();
                }
                Response::Ok
            }
            Request::Toggle {
                surface: Surface::Settings,
            } => {
                self.settings().toggle();
                Response::Ok
            }
            Request::Show {
                surface: Surface::Bar,
            } => self.set_bars_hidden(false),
            Request::Hide {
                surface: Surface::Bar,
            } => self.set_bars_hidden(true),
            Request::Toggle {
                surface: Surface::Bar,
            } => self.set_bars_hidden(!self.bars_hidden.get()),
            Request::Dnd { state } => match self.set_do_not_disturb(state) {
                Some(_) => Response::Ok,
                None => Response::Error {
                    message: "Scuttle is not handling notifications in this session.".into(),
                },
            },
            Request::Media { action } => {
                let control = match action {
                    Playback::Toggle => Control::PlayPause,
                    Playback::Next => Control::Next,
                    Playback::Previous => Control::Previous,
                };
                match self.control_media(control) {
                    Ok(()) => Response::Ok,
                    Err(message) => Response::Error { message },
                }
            }
            Request::Volume { step, amount } => {
                let change = match step {
                    Step::Up => VolumeChange::Up(amount),
                    Step::Down => VolumeChange::Down(amount),
                    Step::Mute => VolumeChange::ToggleMute,
                };
                match self.change_volume(change) {
                    Ok(()) => Response::Ok,
                    Err(message) => Response::Error { message },
                }
            }
            Request::Brightness { step, amount } => {
                let delta = match step {
                    Step::Up => amount.min(100) as i32,
                    Step::Down => -(amount.min(100) as i32),
                    Step::Mute => {
                        return Response::Error {
                            message: "Brightness can go up or down, but it cannot be muted.".into(),
                        }
                    }
                };
                match brightness::step(delta) {
                    Ok(percent) => {
                        self.osd.show(Gauge::Brightness(percent));
                        Response::Ok
                    }
                    Err(message) => Response::Error { message },
                }
            }
            Request::Dismiss => {
                if let Some(toasts) = &self.toasts {
                    toasts.dismiss_all();
                }
                Response::Ok
            }
            Request::Reload => match self.reload() {
                Ok(()) => Response::Ok,
                Err(message) => Response::Error { message },
            },
            Request::Quit => {
                // Answer first, then stop.
                let app = self.app.clone();
                glib::idle_add_local_once(move || app.quit());
                Response::Ok
            }
        }
    }

    fn set_bars_hidden(&self, hidden: bool) -> Response {
        self.bars_hidden.set(hidden);
        self.rebuild_bars();
        Response::Ok
    }

    /// Sends a playback command to the player being shown.
    fn control_media(&self, control: Control) -> Result<(), String> {
        match &self.media {
            Some(controller) => controller.send(control),
            None => Err("Media controls need a D-Bus session, and none was found.".into()),
        }
    }

    /// Records a volume reading and shows it on every bar.
    fn set_volume(&self, volume: Option<Volume>) {
        self.volume.set(volume);
        for bar in self.bars.borrow().iter() {
            bar.update_volume(volume);
        }
    }

    /// Changes the volume and shows the popup with the result.
    fn change_volume(&self, change: VolumeChange) -> Result<(), String> {
        let volume = audio::change(change)?;
        self.set_volume(Some(volume));
        self.osd.show(Gauge::Volume(volume));
        Ok(())
    }

    /// Turns "do not disturb" on or off. Returns the new state, or
    /// `None` when Scuttle is not the notification daemon.
    fn set_do_not_disturb(&self, switch: Switch) -> Option<bool> {
        let store = self.notifications.as_ref()?;
        let on = {
            let mut store = store.lock().ok()?;
            let on = switch.apply(store.do_not_disturb());
            store.set_do_not_disturb(on);
            on
        };
        self.update_badges();
        Some(on)
    }

    /// Refreshes the notification badge on every bar.
    fn update_badges(&self) {
        let Some(store) = &self.notifications else {
            return;
        };
        let (unseen, dnd) = match store.lock() {
            Ok(store) => (store.unseen(), store.do_not_disturb()),
            Err(_) => return,
        };
        for bar in self.bars.borrow().iter() {
            bar.update_notifications(unseen, dnd);
        }
    }

    /// Clears the unread count, as when the badge is clicked.
    fn mark_notifications_seen(&self) {
        if let Some(store) = &self.notifications {
            if let Ok(mut store) = store.lock() {
                store.mark_seen();
            }
        }
        self.update_badges();
    }

    /// Regenerates the stylesheet from the current config and power state.
    fn restyle(&self) {
        let config = self.config.borrow();
        crate::scale::set(config.appearance.scale);
        let on_battery = self.on_battery();
        self.provider
            .load_from_string(&css::stylesheet(&config.theme(), &config, on_battery));
        self.overlay.apply_config(&config, on_battery);
        if let Some(toasts) = &self.toasts {
            toasts.apply_config(&config, on_battery);
        }
        self.osd.apply_config(&config, on_battery);
        if let Some(panel) = &self.assistant {
            panel.apply_config(&config, on_battery);
        }
        self.feedback
            .configure(sound_when(config.sound.feedback), config.sound.volume);
    }

    /// Re-reads the config file. On an error the running config stays
    /// in place and the reason is returned.
    pub fn reload(&self) -> Result<(), String> {
        let loaded = Config::load(&self.config_path);
        // The settings window shows the file as it is, broken or not.
        let refresh_settings = || {
            if let Some(settings) = self.settings.borrow().as_ref() {
                if settings.is_open() {
                    settings.refresh();
                }
            }
        };
        let new = match loaded {
            Ok(new) if self.launcher_only => launcher_only(new),
            Ok(new) => new,
            Err(e) => {
                let message = e.to_string();
                eprintln!("{message}\nKeeping the previous settings.");
                refresh_settings();
                return Err(message);
            }
        };
        let (roots_changed, bar_changed) = {
            let old = self.config.borrow();
            (
                new.launcher.file_roots != old.launcher.file_roots,
                // The bar is rebuilt only when something it is built
                // from changes. Colours and fonts arrive through the
                // stylesheet, so a theme change does not flicker it.
                new.bar != old.bar || new.appearance.scale != old.appearance.scale,
            )
        };
        *self.config.borrow_mut() = new;
        self.restyle();
        self.watch_palette();
        if bar_changed {
            self.rebuild_bars();
        }
        if roots_changed {
            self.index_files();
        }
        refresh_settings();
        Ok(())
    }

    fn index_files(&self) {
        let roots = self.config.borrow().launcher.file_roots.clone();
        services::index_files(roots, self.home.clone(), self.tx.clone());
    }

    /// Closes every bar and builds one per connected monitor.
    fn rebuild_bars(&self) {
        for bar in self.bars.borrow_mut().drain(..) {
            bar.close();
        }
        if self.bars_hidden.get() {
            return;
        }
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let monitors = display.monitors();
        let config = self.config.borrow();
        let mut bars = Vec::new();
        for i in 0..monitors.n_items() {
            let Some(monitor) = monitors.item(i).and_downcast::<gdk::Monitor>() else {
                continue;
            };
            if let Some(bar) = Bar::new(&self.app, &monitor, &config, self.hypr.clone()) {
                bar.update_desktop(&self.desktop.borrow());
                bar.update_stats(&self.stats.borrow());
                let tx = self.tx.clone();
                bar.connect_notifications_clicked(move || {
                    let _ = tx.try_send(Msg::NotificationsSeen);
                });
                bar.update_media(self.track.borrow().as_ref());
                let tx = self.tx.clone();
                bar.connect_media_requested(move |control| {
                    let _ = tx.try_send(Msg::MediaRequest(control));
                });
                bar.update_volume(self.volume.get());
                let tx = self.tx.clone();
                bar.connect_volume_requested(move |change| {
                    let _ = tx.try_send(Msg::VolumeRequest(change));
                });
                bar.update_tray(&self.tray_items.borrow());
                let tx = self.tx.clone();
                bar.connect_tray_requested(move |request| {
                    let _ = tx.try_send(Msg::TrayRequest(request));
                });
                bars.push(bar);
            }
        }
        *self.bars.borrow_mut() = bars;
        drop(config);
        self.start_tray();
        self.update_badges();
    }

    /// While the theme follows the wallpaper, restyles whenever the
    /// palette file is rewritten, which is what happens when the
    /// wallpaper changes. With any other theme nothing is watched.
    fn watch_palette(&self) {
        let wanted = {
            let config = self.config.borrow();
            (config.appearance.theme == scuttle_core::theme::WALLPAPER)
                .then(|| config.palette_path())
                .flatten()
        };
        let current = self
            .palette_watcher
            .borrow()
            .as_ref()
            .map(|(path, _)| path.clone());
        if wanted == current {
            return;
        }
        *self.palette_watcher.borrow_mut() = None;
        let Some(path) = wanted else { return };
        let Ok(monitor) = gio::File::for_path(&path)
            .monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        else {
            return;
        };
        let weak = self.this.borrow().clone();
        let pending = Rc::new(Cell::new(false));
        monitor.connect_changed(move |_, _, _, _| {
            // A palette is written in one go but can fire several
            // events. Fold them into one restyle.
            if pending.replace(true) {
                return;
            }
            let weak = weak.clone();
            let pending = Rc::clone(&pending);
            glib::timeout_add_local_once(Duration::from_millis(200), move || {
                pending.set(false);
                if let Some(shell) = weak.upgrade() {
                    shell.restyle();
                }
            });
        });
        *self.palette_watcher.borrow_mut() = Some((path, monitor));
    }

    /// Reloads automatically when config.toml is saved.
    fn watch_config(self: &Rc<Self>) {
        let file = gio::File::for_path(&self.config_path);
        let Ok(monitor) = file.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        else {
            return;
        };
        let weak = Rc::downgrade(self);
        monitor.connect_changed(move |_, _, _, _| {
            let Some(shell) = weak.upgrade() else { return };
            // Editors write a file in several steps. Wait for them to
            // finish, and fold a burst of events into one reload.
            if shell.reload_pending.replace(true) {
                return;
            }
            let weak = Rc::downgrade(&shell);
            glib::timeout_add_local_once(Duration::from_millis(150), move || {
                if let Some(shell) = weak.upgrade() {
                    shell.reload_pending.set(false);
                    let _ = shell.reload();
                }
            });
        });
        *self.watcher.borrow_mut() = Some(monitor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scuttle_core::config::BarPosition;

    #[test]
    fn launcher_only_turns_off_everything_else_and_keeps_the_rest() {
        let mut config = Config::default();
        config.appearance.theme = "tide".into();
        config.launcher.max_results = 12;
        let only = launcher_only(config.clone());
        assert_eq!(only.bar.position, BarPosition::None);
        assert!(!only.notifications.enabled);
        assert!(!only.assistant.enabled);
        // The look and the launcher's own settings are untouched.
        assert_eq!(only.appearance, config.appearance);
        assert_eq!(only.launcher, config.launcher);
        assert_eq!(only.sound, config.sound);
    }
}
