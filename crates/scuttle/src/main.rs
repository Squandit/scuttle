//! Scuttle: a desktop shell for Hyprland.
//!
//! Run with no arguments to start the shell. Every other subcommand
//! sends a request to the shell that is already running.

mod actions;
mod bar;
mod msg;
mod osd;
mod overlay;
mod scale;
mod services;
mod shell;
mod toasts;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use gtk::prelude::*;
use gtk::{gio, glib};
use scuttle_core::config::Config;
use scuttle_core::ipc::{self, Playback, Request, Response, Step, Surface, Switch};
use scuttle_core::paths;

/// The annotated config that `scuttle config example` prints.
const EXAMPLE_CONFIG: &str = include_str!("../../../config/scuttle.example.toml");

/// What `scuttle hyprland` prints.
const HYPRLAND_LUA: &str = include_str!("../../../config/hyprland.lua");
const HYPRLAND_LEGACY: &str = include_str!("../../../config/hyprland-legacy.conf");

#[derive(Parser)]
#[command(name = "scuttle", version, about = "A desktop shell for Hyprland")]
struct Cli {
    /// Use this config file instead of ~/.config/scuttle/config.toml
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the shell (the default when no command is given)
    Run,
    /// Show the launcher or bar if hidden, hide it if shown
    Toggle { surface: SurfaceArg },
    /// Show the launcher or bar
    Show { surface: SurfaceArg },
    /// Hide the launcher or bar
    Hide { surface: SurfaceArg },
    /// Open the launcher with text already typed, e.g. `scuttle launcher "> "`
    Launcher { query: Vec<String> },
    /// Turn "do not disturb" on or off (toggles when no state is given)
    Dnd {
        #[arg(default_value = "toggle")]
        state: SwitchArg,
    },
    /// Dismiss every notification on screen
    Dismiss,
    /// Control the media player: toggle, next or previous
    Media { action: PlaybackArg },
    /// Change the volume: up, down or mute
    Volume {
        step: StepArg,
        /// How many percentage points to move
        #[arg(long, default_value_t = 5)]
        by: u32,
    },
    /// Change the screen brightness: up or down
    Brightness {
        step: StepArg,
        /// How many percentage points to move
        #[arg(long, default_value_t = 5)]
        by: u32,
    },
    /// Re-read the config file
    Reload,
    /// Stop the shell
    Quit,
    /// Check whether the shell is running
    Ping,
    /// Print the lines to add to your Hyprland config
    Hyprland {
        /// Print the older hyprland.conf syntax instead of Lua
        #[arg(long)]
        legacy: bool,
    },
    /// Inspect the config file
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SurfaceArg {
    Launcher,
    Bar,
}

impl From<SurfaceArg> for Surface {
    fn from(arg: SurfaceArg) -> Surface {
        match arg {
            SurfaceArg::Launcher => Surface::Launcher,
            SurfaceArg::Bar => Surface::Bar,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum PlaybackArg {
    Toggle,
    Next,
    Previous,
}

#[derive(Clone, Copy, ValueEnum)]
enum StepArg {
    Up,
    Down,
    Mute,
}

impl From<StepArg> for Step {
    fn from(arg: StepArg) -> Step {
        match arg {
            StepArg::Up => Step::Up,
            StepArg::Down => Step::Down,
            StepArg::Mute => Step::Mute,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum SwitchArg {
    On,
    Off,
    Toggle,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print where the config file is
    Path,
    /// Print an annotated config with every setting at its default
    Example,
    /// Check the config file for mistakes
    Check,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(paths::config_file);

    let request = match cli.command.unwrap_or(Command::Run) {
        Command::Run => return run(config_path),
        Command::Config { action } => return config_command(action, config_path),
        Command::Hyprland { legacy } => {
            print!(
                "{}",
                if legacy {
                    HYPRLAND_LEGACY
                } else {
                    HYPRLAND_LUA
                }
            );
            return ExitCode::SUCCESS;
        }
        Command::Toggle { surface } => Request::Toggle {
            surface: surface.into(),
        },
        Command::Show { surface } => Request::Show {
            surface: surface.into(),
        },
        Command::Hide { surface } => Request::Hide {
            surface: surface.into(),
        },
        Command::Launcher { query } => Request::Launcher {
            query: query.join(" "),
        },
        Command::Dnd { state } => Request::Dnd {
            state: match state {
                SwitchArg::On => Switch::On,
                SwitchArg::Off => Switch::Off,
                SwitchArg::Toggle => Switch::Toggle,
            },
        },
        Command::Dismiss => Request::Dismiss,
        Command::Media { action } => Request::Media {
            action: match action {
                PlaybackArg::Toggle => Playback::Toggle,
                PlaybackArg::Next => Playback::Next,
                PlaybackArg::Previous => Playback::Previous,
            },
        },
        Command::Volume { step, by } => Request::Volume {
            step: step.into(),
            amount: by,
        },
        Command::Brightness { step, by } => Request::Brightness {
            step: step.into(),
            amount: by,
        },
        Command::Reload => Request::Reload,
        Command::Quit => Request::Quit,
        Command::Ping => Request::Ping,
    };

    match ipc::send(&paths::socket_path(), &request) {
        Ok(Response::Ok) => ExitCode::SUCCESS,
        Ok(Response::Pong { version }) => {
            println!("Scuttle {version} is running.");
            ExitCode::SUCCESS
        }
        Ok(Response::Error { message }) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
        Err(_) => {
            eprintln!("Scuttle is not running in this session. Start it with `scuttle`.");
            ExitCode::FAILURE
        }
    }
}

fn config_command(action: ConfigAction, path: PathBuf) -> ExitCode {
    match action {
        ConfigAction::Path => println!("{}", path.display()),
        ConfigAction::Example => print!("{EXAMPLE_CONFIG}"),
        ConfigAction::Check => match Config::load(&path) {
            Ok(_) if path.exists() => println!("{} is valid.", path.display()),
            Ok(_) => println!("{} does not exist, so the defaults apply.", path.display()),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
        },
    }
    ExitCode::SUCCESS
}

fn run(config_path: PathBuf) -> ExitCode {
    // Scuttle does its own single-instance check over its socket, which
    // also works when there is no D-Bus session.
    let app = gtk::Application::builder()
        .application_id("dev.scuttle.Shell")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let failed = std::rc::Rc::new(std::cell::Cell::new(false));
    let shell_slot = std::rc::Rc::new(std::cell::RefCell::new(None));

    app.connect_activate({
        let failed = failed.clone();
        let shell_slot = shell_slot.clone();
        move |app| {
            if shell_slot.borrow().is_some() {
                return;
            }
            match shell::Shell::start(app, &config_path) {
                Ok(shell) => {
                    // The shell has no ordinary window, so hold the
                    // application open explicitly.
                    let hold = app.hold();
                    *shell_slot.borrow_mut() = Some((shell, hold));
                }
                Err(message) => {
                    eprintln!("{message}");
                    failed.set(true);
                    app.quit();
                }
            }
        }
    });

    // GTK would otherwise try to parse Scuttle's own arguments.
    let status = app.run_with_args::<glib::GString>(&[]);

    let started = shell_slot.borrow_mut().take().is_some();
    if started {
        let _ = std::fs::remove_file(paths::socket_path());
    }
    if failed.get() || status != glib::ExitCode::SUCCESS {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_is_valid_and_matches_the_defaults() {
        let parsed = Config::parse(EXAMPLE_CONFIG).expect("the example config must parse");
        assert_eq!(
            parsed,
            Config::default(),
            "the example must show the real defaults"
        );
    }

    #[test]
    fn hyprland_snippets_cover_autostart_keybind_and_blur() {
        for snippet in [HYPRLAND_LUA, HYPRLAND_LEGACY] {
            assert!(snippet.contains("scuttle toggle launcher"));
            for surface in [
                "scuttle-bar",
                "scuttle-launcher",
                "scuttle-toasts",
                "scuttle-osd",
            ] {
                assert!(
                    snippet.contains(surface),
                    "{surface} is missing a blur rule"
                );
            }
        }
        assert!(HYPRLAND_LUA.contains("hl.exec_cmd(\"scuttle\")"));
        assert!(HYPRLAND_LEGACY.contains("exec-once = scuttle"));
        // Hyprland's own layer animation must be off, or the launcher
        // opens as a growing rectangle.
        assert!(HYPRLAND_LUA.contains("no_anim = true"));
        assert_eq!(
            HYPRLAND_LEGACY
                .matches("layerrule = noanim, scuttle-")
                .count(),
            4
        );
        for snippet in [HYPRLAND_LUA, HYPRLAND_LEGACY] {
            for command in [
                "scuttle volume up",
                "scuttle volume down",
                "scuttle volume mute",
                "scuttle brightness up",
                "scuttle brightness down",
                "scuttle media toggle",
                "scuttle media next",
                "scuttle media previous",
            ] {
                assert!(snippet.contains(command), "{command} has no key");
            }
        }
    }

    #[test]
    fn cli_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
