//! The running shell: owns the config, the stylesheet and every
//! surface, and reacts to messages from the background threads.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use async_channel::Sender;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use scuttle_core::config::Config;
use scuttle_core::ipc::{Request, Response, Surface};
use scuttle_core::{css, paths};
use scuttle_hypr::Hyprland;
use scuttle_launcher::history::History;
use scuttle_launcher::Internal;

use crate::bar::Bar;
use crate::msg::{DesktopState, Msg, Stats};
use crate::overlay::Overlay;
use crate::services::{self, SocketError};

pub struct Shell {
    app: gtk::Application,
    config_path: PathBuf,
    home: PathBuf,
    config: RefCell<Config>,
    provider: gtk::CssProvider,
    hypr: Option<Hyprland>,
    overlay: Overlay,
    bars: RefCell<Vec<Bar>>,
    bars_hidden: Cell<bool>,
    desktop: RefCell<DesktopState>,
    stats: RefCell<Stats>,
    tx: Sender<Msg>,
    /// Keeps the config file watcher alive.
    watcher: RefCell<Option<gio::FileMonitor>>,
    reload_pending: Cell<bool>,
}

impl Shell {
    /// Builds the shell and starts everything. Returns an error message
    /// when the shell cannot start at all.
    pub fn start(app: &gtk::Application, config_path: &Path) -> Result<Rc<Shell>, String> {
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
        }

        let provider = gtk::CssProvider::new();
        provider.load_from_string(&css::stylesheet(&config.theme(), &config, on_battery));
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let history = History::load(&paths::data_dir().join("launcher-history"));
        let overlay = Overlay::new(app, &config, &home, history, hypr.clone());
        overlay.apply_config(&config, on_battery);
        overlay.update_stats(&stats);

        let shell = Rc::new(Shell {
            app: app.clone(),
            config_path: config_path.to_path_buf(),
            home,
            config: RefCell::new(config),
            provider,
            hypr,
            overlay,
            bars: RefCell::new(Vec::new()),
            bars_hidden: Cell::new(false),
            desktop: RefCell::new(DesktopState::default()),
            stats: RefCell::new(stats),
            tx: tx.clone(),
            watcher: RefCell::new(None),
            reload_pending: Cell::new(false),
        });

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
                }
            }
        });

        // Read the compositor once up front so the first frame of the
        // bar already has its workspaces.
        if let Some(hypr) = &shell.hypr {
            if let Ok(desktop) = services::snapshot(hypr) {
                shell.overlay.update_desktop(&desktop);
                *shell.desktop.borrow_mut() = desktop;
            }
            services::watch_hyprland(hypr.clone(), tx.clone());
        }
        shell.rebuild_bars();

        services::watch_stats(tx.clone());
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
        }
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
                surface: Surface::Bar,
            } => self.set_bars_hidden(false),
            Request::Hide {
                surface: Surface::Bar,
            } => self.set_bars_hidden(true),
            Request::Toggle {
                surface: Surface::Bar,
            } => self.set_bars_hidden(!self.bars_hidden.get()),
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

    /// Regenerates the stylesheet from the current config and power state.
    fn restyle(&self) {
        let config = self.config.borrow();
        let on_battery = self.on_battery();
        self.provider
            .load_from_string(&css::stylesheet(&config.theme(), &config, on_battery));
        self.overlay.apply_config(&config, on_battery);
    }

    /// Re-reads the config file. On an error the running config stays
    /// in place and the reason is returned.
    pub fn reload(&self) -> Result<(), String> {
        let new = Config::load(&self.config_path).map_err(|e| {
            let message = e.to_string();
            eprintln!("{message}\nKeeping the previous settings.");
            message
        })?;
        let roots_changed = new.launcher.file_roots != self.config.borrow().launcher.file_roots;
        *self.config.borrow_mut() = new;
        self.restyle();
        self.rebuild_bars();
        if roots_changed {
            self.index_files();
        }
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
                bars.push(bar);
            }
        }
        *self.bars.borrow_mut() = bars;
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
