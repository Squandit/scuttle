//! Scuttle: a desktop shell for Hyprland.
//!
//! Run with no arguments to start the shell. Every other subcommand
//! sends a request to the shell that is already running.

mod actions;
mod assistant;
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
    /// Show the launcher, bar or assistant if hidden, hide it if shown
    Toggle { surface: SurfaceArg },
    /// Show the launcher, bar or assistant
    Show { surface: SurfaceArg },
    /// Hide the launcher, bar or assistant
    Hide { surface: SurfaceArg },
    /// Open the launcher with text already typed, e.g. `scuttle launcher "> "`
    Launcher { query: Vec<String> },
    /// Open the assistant and ask it something, e.g. `scuttle ask why is my fan loud`
    Ask {
        #[arg(required = true)]
        message: Vec<String>,
    },
    /// Set up the assistant: its API key and how it reaches Claude
    Assistant {
        #[command(subcommand)]
        action: AssistantAction,
    },
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
    /// Inspect the theme, or apply its colours to the rest of the desktop
    Theme {
        #[command(subcommand)]
        action: ThemeAction,
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
    Assistant,
}

impl From<SurfaceArg> for Surface {
    fn from(arg: SurfaceArg) -> Surface {
        match arg {
            SurfaceArg::Launcher => Surface::Launcher,
            SurfaceArg::Bar => Surface::Bar,
            SurfaceArg::Assistant => Surface::Assistant,
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
enum ThemeAction {
    /// Print the colours in use and where they come from
    Show,
    /// Recolour the rest of the desktop to match, through matugen
    Push,
}

#[derive(Subcommand)]
enum AssistantAction {
    /// Store a Claude API key in the system keyring
    SetKey,
    /// Remove the stored API key
    ClearKey,
    /// Show which ways of reaching Claude are set up
    Status,
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

/// The request a command sends to the running shell, or the command
/// back when it is one that runs without the shell.
fn ipc_request(command: Command) -> Result<Request, Command> {
    Ok(match command {
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
        Command::Ask { message } => Request::Ask {
            message: message.join(" "),
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
        local => return Err(local),
    })
}

/// Turns the words after `scuttle` into the request they would send.
/// The assistant's desktop actions go through this, so they mean
/// exactly what the same words mean in a terminal.
pub fn request_from_args(args: &[String]) -> Result<Request, String> {
    let words = std::iter::once("scuttle").chain(args.iter().map(String::as_str));
    let cli = Cli::try_parse_from(words).map_err(|_| {
        format!(
            "\"scuttle {}\" is not a command Scuttle knows.",
            args.join(" ")
        )
    })?;
    cli.command
        .ok_or(())
        .and_then(|command| ipc_request(command).map_err(|_| ()))
        .map_err(|_| format!("\"scuttle {}\" cannot be run from here.", args.join(" ")))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config_path = cli.config.unwrap_or_else(paths::config_file);

    let request = match ipc_request(cli.command.unwrap_or(Command::Run)) {
        Ok(request) => request,
        Err(Command::Config { action }) => return config_command(action, config_path),
        Err(Command::Theme { action }) => return theme_command(action, config_path),
        Err(Command::Assistant { action }) => return assistant_command(action, config_path),
        Err(Command::Hyprland { legacy }) => {
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
        Err(_) => return run(config_path),
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

/// The matugen command line that recolours the desktop from one colour.
fn matugen_args(accent: scuttle_core::Rgba, mode: scuttle_core::theme::Mode) -> Vec<String> {
    let mode = match mode {
        scuttle_core::theme::Mode::Dark => "dark",
        scuttle_core::theme::Mode::Light => "light",
    };
    vec![
        format!("--mode={mode}"),
        "color".to_string(),
        "hex".to_string(),
        accent.hex(),
    ]
}

fn theme_command(action: ThemeAction, path: PathBuf) -> ExitCode {
    let config = match Config::load(&path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let theme = config.theme();
    let follows_wallpaper = config.appearance.theme == scuttle_core::theme::WALLPAPER;

    match action {
        ThemeAction::Show => {
            println!("theme        {} ({:?})", theme.name, theme.mode);
            if follows_wallpaper {
                match config.palette_path() {
                    Some(file) if theme.name == scuttle_core::theme::WALLPAPER => {
                        println!("palette      {}", file.display())
                    }
                    Some(file) => println!(
                        "palette      {} could not be used, so the default theme is showing",
                        file.display()
                    ),
                    None => println!(
                        "palette      no palette file was found, so the default theme is showing"
                    ),
                }
            }
            for (name, colour) in [
                ("ground", theme.ground),
                ("surface", theme.surface),
                ("text", theme.text),
                ("text_mid", theme.text_mid),
                ("text_dim", theme.text_dim),
                ("accent", theme.accent),
                ("on_accent", theme.on_accent),
                ("accent_text", theme.accent_text),
            ] {
                println!("{name:<12} {}", colour.hex());
            }
            ExitCode::SUCCESS
        }
        ThemeAction::Push => {
            if follows_wallpaper {
                eprintln!(
                    "The theme already follows the wallpaper palette, so there is nothing to push. \
                     Pick a built-in theme first if you want its colour everywhere."
                );
                return ExitCode::FAILURE;
            }
            let args = matugen_args(theme.accent, theme.mode);
            println!("Running: matugen {}", args.join(" "));
            match std::process::Command::new("matugen").args(&args).status() {
                Ok(status) if status.success() => {
                    println!(
                        "Done. Apps themed by matugen now use {}. Changing the wallpaper \
                         through your usual tool will recolour them from the wallpaper again.",
                        theme.accent.hex()
                    );
                    ExitCode::SUCCESS
                }
                Ok(status) => {
                    eprintln!("matugen stopped with an error ({status}).");
                    ExitCode::FAILURE
                }
                Err(_) => {
                    eprintln!(
                        "matugen is not installed. It is what recolours other apps; \
                         dusky ships it, and it is in the AUR as matugen-bin."
                    );
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// Reads one line from the terminal without showing it.
fn read_secret(prompt: &str) -> std::io::Result<String> {
    use std::io::{BufRead, IsTerminal, Write};
    let stdin = std::io::stdin();
    let hide = stdin.is_terminal();
    if hide {
        eprint!("{prompt}");
        std::io::stderr().flush()?;
        // Turn the terminal's echo off while the key is typed or pasted.
        let _ = std::process::Command::new("stty").arg("-echo").status();
    }
    let mut line = String::new();
    let read = stdin.lock().read_line(&mut line);
    if hide {
        let _ = std::process::Command::new("stty").arg("echo").status();
        eprintln!();
    }
    read.map(|_| line.trim().to_string())
}

fn assistant_command(action: AssistantAction, path: PathBuf) -> ExitCode {
    use scuttle_assistant::key::{masked, Keyring, ANTHROPIC, ANTHROPIC_ENV};
    let keyring = Keyring::default();
    match action {
        AssistantAction::SetKey => {
            let key = match read_secret(
                "Paste your Claude API key and press Enter (it stays hidden): ",
            ) {
                Ok(key) => key,
                Err(e) => {
                    eprintln!("Could not read the key: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match keyring.store(ANTHROPIC, &key) {
                Ok(()) => {
                    println!(
                        "Stored {} in the system keyring. The assistant will use it when your \
                         Claude subscription is at its limit or is not set up.",
                        masked(&key)
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        AssistantAction::ClearKey => match keyring.clear(ANTHROPIC) {
            Ok(()) => {
                println!("Removed the API key from the system keyring.");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        },
        AssistantAction::Status => {
            let config = Config::load(&path).unwrap_or_else(|e| {
                eprintln!("{e}\nShowing the default settings instead.");
                Config::default()
            });
            let settings = assistant::settings(&config);
            let a = &config.assistant;
            println!("enabled       {}", if a.enabled { "yes" } else { "no" });
            println!(
                "backend       {}",
                match settings.prefer {
                    scuttle_assistant::Prefer::Auto =>
                        "auto (subscription first, then the API key)",
                    scuttle_assistant::Prefer::Subscription => "subscription only",
                    scuttle_assistant::Prefer::ApiKey => "API key only",
                }
            );
            let claude = actions::on_path(&settings.claude_program);
            println!(
                "subscription  {} ({})",
                if claude {
                    "the claude command is installed"
                } else {
                    "the claude command is not installed"
                },
                scuttle_assistant::pretty_model(&a.subscription_model)
            );
            let key = match keyring.lookup(ANTHROPIC) {
                Some(key) => format!("{} in the keyring", masked(&key)),
                None => match std::env::var(ANTHROPIC_ENV) {
                    Ok(key) if !key.trim().is_empty() => {
                        format!("{} from {ANTHROPIC_ENV}", masked(key.trim()))
                    }
                    _ => "none set (add one with `scuttle assistant set-key`)".to_string(),
                },
            };
            println!(
                "API key       {key} ({})",
                scuttle_assistant::pretty_model(&a.api_model)
            );
            if claude {
                println!(
                    "\nWhether the claude command is signed in only shows when it is used. \
                     Run `claude` in a terminal once to sign in."
                );
            }
            ExitCode::SUCCESS
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
            assert!(snippet.contains("scuttle toggle assistant"));
            for surface in [
                "scuttle-bar",
                "scuttle-launcher",
                "scuttle-toasts",
                "scuttle-osd",
                "scuttle-assistant",
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
            5
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
    fn the_assistant_s_actions_mean_what_the_command_line_means() {
        let args = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(
            request_from_args(&args("volume down")),
            Ok(Request::Volume {
                step: Step::Down,
                amount: 5
            })
        );
        assert_eq!(
            request_from_args(&args("dnd on")),
            Ok(Request::Dnd { state: Switch::On })
        );
        assert_eq!(
            request_from_args(&args("toggle bar")),
            Ok(Request::Toggle {
                surface: Surface::Bar
            })
        );
        // Every action the assistant is allowed maps onto a request.
        for (command, _) in scuttle_assistant::actions::DESKTOP_COMMANDS {
            assert!(request_from_args(&args(command)).is_ok(), "{command}");
        }
    }

    #[test]
    fn words_that_are_not_shell_requests_are_refused() {
        let args = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        for bad in [
            "run",
            "config example",
            "assistant set-key",
            "theme push",
            "nonsense",
            "volume sideways",
        ] {
            assert!(request_from_args(&args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn ask_joins_its_words_into_one_message() {
        assert_eq!(
            request_from_args(&["ask".into(), "why".into(), "is it loud".into()]),
            Ok(Request::Ask {
                message: "why is it loud".into()
            })
        );
    }

    #[test]
    fn theme_push_builds_the_matugen_command() {
        use scuttle_core::theme::Mode;
        let orange = "#f74c00".parse().unwrap();
        assert_eq!(
            matugen_args(orange, Mode::Dark),
            ["--mode=dark", "color", "hex", "#f74c00"]
        );
        assert_eq!(matugen_args(orange, Mode::Light)[0], "--mode=light");
    }

    #[test]
    fn cli_definition_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
