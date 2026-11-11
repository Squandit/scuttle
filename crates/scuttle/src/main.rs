//! Scuttle: a desktop shell for Hyprland.
//!
//! Run with no arguments to start the shell. Every other subcommand
//! sends a request to the shell that is already running.

mod actions;
mod bar;
mod msg;
mod overlay;
mod services;
mod shell;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use gtk::prelude::*;
use gtk::{gio, glib};
use scuttle_core::config::Config;
use scuttle_core::ipc::{self, Request, Response, Surface};
use scuttle_core::paths;

/// The annotated config that `scuttle config example` prints.
const EXAMPLE_CONFIG: &str = include_str!("../../../config/scuttle.example.toml");

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
    /// Re-read the config file
    Reload,
    /// Stop the shell
    Quit,
    /// Check whether the shell is running
    Ping,
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
    fn cli_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
