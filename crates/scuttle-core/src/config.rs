//! The user's configuration: one TOML file, every field optional.
//!
//! A missing file means defaults. An invalid file is reported with the
//! reason and never silently half-applied: the shell keeps running on
//! the last good config.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::color::Rgba;
use crate::motion::Motion;
use crate::theme::{Mode, Theme, WALLPAPER};

/// Everything in `config.toml`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub appearance: Appearance,
    pub bar: Bar,
    pub launcher: Launcher,
    pub notifications: Notifications,
    pub sound: Sound,
    pub assistant: Assistant,
    pub power: Power,
}

/// `[appearance]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    /// Name of a built-in theme, or "wallpaper" to take the colours
    /// from the palette file.
    pub theme: String,
    /// The palette file read when `theme = "wallpaper"`: lines of
    /// `@define-color name #hex;`. Unset means look in the usual
    /// matugen locations.
    pub palette_file: Option<PathBuf>,
    pub mode: Mode,
    /// Overrides the theme's accent colour.
    pub accent: Option<Rgba>,
    /// Corner radius of panels, in pixels.
    pub radius: u32,
    /// Size of everything Scuttle draws, as a multiple of the base
    /// design size: text, the bar, the launcher, spacing.
    pub scale: f32,
    /// Ask the compositor to blur what is behind translucent panels.
    pub blur: bool,
    pub motion: Motion,
    /// Font for interface text.
    pub font: String,
    /// Font for large display text such as the launcher input and clocks.
    pub font_display: String,
    /// Font for numbers, paths and code.
    pub font_mono: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: "ferris".into(),
            palette_file: None,
            mode: Mode::Dark,
            accent: None,
            radius: 18,
            scale: 1.25,
            blur: true,
            motion: Motion::Subtle,
            font: "Geist".into(),
            font_display: "Sora".into(),
            font_mono: "Geist Mono".into(),
        }
    }
}

/// Where the bar sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BarPosition {
    #[default]
    Top,
    Bottom,
    /// No permanent bar. Status shows in the launcher overlay instead.
    None,
}

/// The shape of the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BarStyle {
    /// One full-width floating bar.
    #[default]
    Strip,
    /// Three separate floating pills.
    Islands,
    /// A small tab attached to the screen edge.
    Notch,
}

/// A widget that can be placed in the bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Module {
    Workspaces,
    Title,
    Clock,
    Media,
    Cpu,
    Memory,
    Network,
    Volume,
    /// Shown only on machines that have a battery.
    Battery,
    /// Icons that apps put in the system tray.
    Tray,
    Notifications,
}

/// `[bar]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bar {
    pub position: BarPosition,
    pub style: BarStyle,
    /// Height in pixels.
    pub height: u32,
    /// Gap between the bar and the screen edge, in pixels.
    pub margin: u32,
    pub left: Vec<Module>,
    pub center: Vec<Module>,
    pub right: Vec<Module>,
    /// strftime-style format for the clock.
    pub clock_format: String,
    /// How many workspace buttons to always show.
    pub workspaces: u32,
}

impl Default for Bar {
    fn default() -> Self {
        Self {
            position: BarPosition::Top,
            style: BarStyle::Strip,
            height: 40,
            margin: 10,
            left: vec![Module::Workspaces, Module::Title],
            center: vec![Module::Clock],
            right: vec![
                Module::Media,
                Module::Cpu,
                Module::Memory,
                Module::Network,
                Module::Volume,
                Module::Battery,
                Module::Tray,
                Module::Notifications,
            ],
            clock_format: "%a %d %b   %H:%M".into(),
            workspaces: 4,
        }
    }
}

/// `[launcher]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Launcher {
    /// URL template for web searches. `{query}` is replaced with the
    /// percent-encoded search text.
    pub web_search: String,
    /// Maximum number of results shown at once.
    pub max_results: usize,
    /// Terminal used for apps that need one. Empty means auto-detect.
    pub terminal: String,
    /// Folders searched for files, relative to the home directory when
    /// not absolute.
    pub file_roots: Vec<PathBuf>,
}

impl Default for Launcher {
    fn default() -> Self {
        Self {
            web_search: "https://duckduckgo.com/?q={query}".into(),
            max_results: 8,
            terminal: String::new(),
            file_roots: vec![
                "Documents".into(),
                "Downloads".into(),
                "Pictures".into(),
                "Desktop".into(),
                "Projects".into(),
            ],
        }
    }
}

/// When interface sounds play.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SoundWhen {
    Never,
    /// Only while the audio output is headphones or a headset.
    #[default]
    Headphones,
    Always,
}

/// `[sound]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sound {
    pub feedback: SoundWhen,
    /// Loudness of interface sounds from 0.0 to 1.0.
    pub volume: f32,
}

impl Default for Sound {
    fn default() -> Self {
        Self {
            feedback: SoundWhen::Headphones,
            volume: 0.25,
        }
    }
}

/// Which corner notifications appear in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Corner {
    #[default]
    TopRight,
    TopLeft,
    BottomRight,
    BottomLeft,
}

impl Corner {
    pub fn is_top(self) -> bool {
        matches!(self, Corner::TopRight | Corner::TopLeft)
    }

    pub fn is_right(self) -> bool {
        matches!(self, Corner::TopRight | Corner::BottomRight)
    }
}

/// `[notifications]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Notifications {
    /// Let Scuttle show notifications. Turn off to keep using another
    /// notification daemon.
    pub enabled: bool,
    pub position: Corner,
    /// How long a notification stays when the app does not say, in
    /// milliseconds.
    pub timeout_ms: u32,
    /// Most notifications on screen at once.
    pub max_visible: usize,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            position: Corner::TopRight,
            timeout_ms: 5000,
            max_visible: 4,
        }
    }
}

/// How the assistant reaches Claude.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssistantBackend {
    /// The Claude subscription first, the API key when it cannot answer.
    #[default]
    Auto,
    /// Only the Claude subscription, through the `claude` command.
    Subscription,
    /// Only the API key.
    Api,
}

/// A side of the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    Right,
    Left,
}

/// `[assistant]`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Assistant {
    pub enabled: bool,
    pub backend: AssistantBackend,
    /// The model used on the Claude subscription.
    pub subscription_model: String,
    /// The model used on the API key.
    pub api_model: String,
    /// Which side of the screen the panel opens on.
    pub side: Side,
    /// Width of the panel in pixels, before `appearance.scale`.
    pub width: u32,
}

impl Default for Assistant {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: AssistantBackend::Auto,
            subscription_model: "claude-opus-5-5".to_string(),
            api_model: "claude-haiku-5-5".to_string(),
            side: Side::Right,
            width: 440,
        }
    }
}

/// `[power]`: what changes when a laptop runs on battery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Power {
    /// Turn blur off while on battery.
    pub battery_disables_blur: bool,
    /// Cap motion at "subtle" while on battery.
    pub battery_limits_motion: bool,
}

impl Default for Power {
    fn default() -> Self {
        Self {
            battery_disables_blur: true,
            battery_limits_motion: true,
        }
    }
}

/// Why a config file could not be used.
#[derive(Debug)]
pub enum ConfigError {
    Read(PathBuf, io::Error),
    Parse(PathBuf, toml::de::Error),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read(path, e) => write!(f, "Could not read {}: {e}", path.display()),
            ConfigError::Parse(path, e) => write!(f, "{} has an error: {e}", path.display()),
            ConfigError::Invalid(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Parses and validates a config from TOML text.
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let config: Config = toml::from_str(text)
            .map_err(|e| ConfigError::Parse(PathBuf::from("config.toml"), e))?;
        config.validate()?;
        Ok(config)
    }

    /// Loads a config file. A missing file yields the defaults.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(e) => return Err(ConfigError::Read(path.to_path_buf(), e)),
        };
        let config: Config =
            toml::from_str(&text).map_err(|e| ConfigError::Parse(path.to_path_buf(), e))?;
        config.validate()?;
        Ok(config)
    }

    /// Serialises the config back to TOML.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config is always serialisable")
    }

    /// Checks values that the type system cannot.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let bad = |msg: String| Err(ConfigError::Invalid(msg));
        let a = &self.appearance;
        if a.theme != WALLPAPER && Theme::builtin(&a.theme, a.mode).is_none() {
            return bad(format!(
                "appearance.theme = \"{}\" is not a theme. Choose one of: {}, {WALLPAPER}.",
                a.theme,
                crate::theme::BUILTIN.join(", ")
            ));
        }
        if !(0.75..=2.5).contains(&a.scale) {
            return bad(format!(
                "appearance.scale = {} is out of range. Use 0.75 to 2.5.",
                a.scale
            ));
        }
        if a.radius > 32 {
            return bad(format!(
                "appearance.radius = {} is too large. Use 0 to 32.",
                a.radius
            ));
        }
        if !(24..=72).contains(&self.bar.height) {
            return bad(format!(
                "bar.height = {} is out of range. Use 24 to 72.",
                self.bar.height
            ));
        }
        if self.bar.margin > 64 {
            return bad(format!(
                "bar.margin = {} is too large. Use 0 to 64.",
                self.bar.margin
            ));
        }
        if !(1..=20).contains(&self.bar.workspaces) {
            return bad(format!(
                "bar.workspaces = {} is out of range. Use 1 to 20.",
                self.bar.workspaces
            ));
        }
        if !(1..=50).contains(&self.launcher.max_results) {
            return bad(format!(
                "launcher.max_results = {} is out of range. Use 1 to 50.",
                self.launcher.max_results
            ));
        }
        if !self.launcher.web_search.contains("{query}") {
            return bad(
                "launcher.web_search must contain {query} where the search text goes.".into(),
            );
        }
        if !(1000..=60_000).contains(&self.notifications.timeout_ms) {
            return bad(format!(
                "notifications.timeout_ms = {} is out of range. Use 1000 to 60000.",
                self.notifications.timeout_ms
            ));
        }
        if !(1..=10).contains(&self.notifications.max_visible) {
            return bad(format!(
                "notifications.max_visible = {} is out of range. Use 1 to 10.",
                self.notifications.max_visible
            ));
        }
        if !(320..=900).contains(&self.assistant.width) {
            return bad(format!(
                "assistant.width = {} is out of range. Use 320 to 900.",
                self.assistant.width
            ));
        }
        for (name, model) in [
            ("subscription_model", &self.assistant.subscription_model),
            ("api_model", &self.assistant.api_model),
        ] {
            // A model name ends up on a command line and in a request,
            // so it has to look like one.
            let plain = model
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '[' | ']'));
            if model.is_empty() || !plain || model.starts_with('-') || model.len() > 80 {
                return bad(format!(
                    "assistant.{name} = \"{model}\" is not a model name. Use something like \"claude-opus-5-5\"."
                ));
            }
        }
        if !(0.0..=1.0).contains(&self.sound.volume) {
            return bad(format!(
                "sound.volume = {} is out of range. Use 0.0 to 1.0.",
                self.sound.volume
            ));
        }
        Ok(())
    }

    /// The palette file to read for the "wallpaper" theme: the one
    /// named in the config, else the first usual location that exists.
    pub fn palette_path(&self) -> Option<PathBuf> {
        match &self.appearance.palette_file {
            Some(path) => Some(path.clone()),
            None => crate::paths::palette_candidates()
                .into_iter()
                .find(|p| p.is_file()),
        }
    }

    /// The theme this config selects, with any accent override applied.
    ///
    /// The "wallpaper" theme is read from the palette file. If that
    /// file is missing or unusable, the default theme is used so the
    /// shell always has colours.
    pub fn theme(&self) -> Theme {
        let a = &self.appearance;
        let from_wallpaper = (a.theme == WALLPAPER)
            .then(|| self.palette_path())
            .flatten()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| Theme::from_palette(&crate::theme::parse_define_colors(&text)));
        let theme = from_wallpaper
            .or_else(|| Theme::builtin(&a.theme, a.mode))
            .or_else(|| Theme::builtin("ferris", a.mode))
            .expect("the default theme always exists");
        match a.accent {
            Some(accent) => theme.with_accent(accent),
            None => theme,
        }
    }

    /// Motion after the battery rule is applied.
    pub fn effective_motion(&self, on_battery: bool) -> Motion {
        match (
            self.appearance.motion,
            on_battery && self.power.battery_limits_motion,
        ) {
            (Motion::Full, true) => Motion::Subtle,
            (motion, _) => motion,
        }
    }

    /// Blur after the battery rule is applied.
    pub fn effective_blur(&self, on_battery: bool) -> bool {
        self.appearance.blur && !(on_battery && self.power.battery_disables_blur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_is_all_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn defaults_are_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn defaults_round_trip_through_toml() {
        let text = Config::default().to_toml();
        assert_eq!(Config::parse(&text).unwrap(), Config::default());
    }

    #[test]
    fn the_assistant_defaults_to_opus_on_the_subscription_and_haiku_on_the_key() {
        let a = Config::default().assistant;
        assert!(a.enabled);
        assert_eq!(a.backend, AssistantBackend::Auto);
        assert_eq!(a.subscription_model, "claude-opus-5-5");
        assert_eq!(a.api_model, "claude-haiku-5-5");
        assert_eq!(a.side, Side::Right);
    }

    #[test]
    fn the_assistant_section_is_read_and_checked() {
        let c = Config::parse(
            "[assistant]\nbackend = \"api\"\napi_model = \"claude-sonnet-5-5\"\nside = \"left\"\nwidth = 520\n",
        )
        .unwrap();
        assert_eq!(c.assistant.backend, AssistantBackend::Api);
        assert_eq!(c.assistant.api_model, "claude-sonnet-5-5");
        assert_eq!(c.assistant.side, Side::Left);
        assert_eq!(c.assistant.width, 520);

        for bad in [
            "[assistant]\nwidth = 100\n",
            "[assistant]\nbackend = \"ollama\"\n",
            "[assistant]\napi_model = \"\"\n",
            "[assistant]\nsubscription_model = \"--dangerously-skip-permissions\"\n",
            "[assistant]\napi_model = \"opus; rm -rf ~\"\n",
            "[assistant]\napi_key = \"sk-ant-oops\"\n",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let c = Config::parse("[appearance]\naccent = \"#3355ff\"\nradius = 4\n").unwrap();
        assert_eq!(c.appearance.radius, 4);
        assert_eq!(c.appearance.accent, Some("#3355ff".parse().unwrap()));
        assert_eq!(c.appearance.theme, "ferris");
        assert_eq!(c.bar, Bar::default());
    }

    #[test]
    fn unknown_keys_are_rejected_with_the_key_named() {
        let err = Config::parse("[appearance]\nraduis = 4\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("raduis"), "{err}");
    }

    #[test]
    fn out_of_range_values_explain_the_range() {
        let err = Config::parse("[bar]\nheight = 500\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("24 to 72"), "{err}");
        let err = Config::parse("[appearance]\ntheme = \"nope\"\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("ferris"), "{err}");
    }

    #[test]
    fn bad_colour_is_rejected() {
        assert!(Config::parse("[appearance]\naccent = \"orange\"\n").is_err());
    }

    #[test]
    fn modules_parse_by_lowercase_name() {
        let c = Config::parse("[bar]\nright = [\"battery\", \"clock\"]\n").unwrap();
        assert_eq!(c.bar.right, vec![Module::Battery, Module::Clock]);
    }

    #[test]
    fn wallpaper_theme_reads_the_palette_file() {
        let dir = std::env::temp_dir().join(format!("scuttle-palette-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("colors.css");
        fs::write(
            &file,
            "@define-color surface #1a110d;\n@define-color surface_container #271d19;\n\
             @define-color on_surface #f1dfd8;\n@define-color primary #ffb597;\n\
             @define-color on_primary #5a1c00;\n",
        )
        .unwrap();
        let mut c = Config::default();
        c.appearance.theme = "wallpaper".into();
        c.appearance.palette_file = Some(file.clone());
        c.validate().unwrap();
        assert_eq!(c.theme().name, "wallpaper");
        assert_eq!(c.theme().accent, "#ffb597".parse().unwrap());

        // An accent set in the config still wins.
        c.appearance.accent = Some("#3355ff".parse().unwrap());
        assert_eq!(c.theme().accent, "#3355ff".parse().unwrap());
        c.appearance.accent = None;

        // A missing or broken palette falls back to the default theme.
        fs::write(&file, "not a palette").unwrap();
        assert_eq!(c.theme().name, "ferris");
        c.appearance.palette_file = Some(dir.join("missing.css"));
        assert_eq!(c.theme().name, "ferris");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scale_is_validated() {
        assert_eq!(Config::default().appearance.scale, 1.25);
        let c = Config::parse("[appearance]\nscale = 1.5\n").unwrap();
        assert_eq!(c.appearance.scale, 1.5);
        let err = Config::parse("[appearance]\nscale = 9\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("0.75 to 2.5"), "{err}");
        assert!(Config::parse("[appearance]\nscale = 0\n").is_err());
    }

    #[test]
    fn notification_settings_parse_and_validate() {
        let c = Config::parse("[notifications]\nposition = \"bottom-left\"\ntimeout_ms = 8000\n")
            .unwrap();
        assert_eq!(c.notifications.position, Corner::BottomLeft);
        assert!(!c.notifications.position.is_top() && !c.notifications.position.is_right());
        assert_eq!(c.notifications.timeout_ms, 8000);
        assert!(c.notifications.enabled);
        let err = Config::parse("[notifications]\ntimeout_ms = 5\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("1000 to 60000"), "{err}");
        assert!(Config::parse("[notifications]\nmax_visible = 0\n").is_err());
        assert!(Config::parse("[notifications]\nposition = \"middle\"\n").is_err());
    }

    #[test]
    fn battery_limits_motion_and_blur() {
        let mut c = Config::default();
        c.appearance.motion = Motion::Full;
        assert_eq!(c.effective_motion(false), Motion::Full);
        assert_eq!(c.effective_motion(true), Motion::Subtle);
        assert!(c.effective_blur(false));
        assert!(!c.effective_blur(true));
        c.power.battery_limits_motion = false;
        c.power.battery_disables_blur = false;
        assert_eq!(c.effective_motion(true), Motion::Full);
        assert!(c.effective_blur(true));
    }

    #[test]
    fn accent_override_reaches_the_theme() {
        let c = Config::parse("[appearance]\naccent = \"#3355ff\"\n").unwrap();
        assert_eq!(c.theme().accent, "#3355ff".parse().unwrap());
    }

    #[test]
    fn missing_file_loads_defaults() {
        let c = Config::load(Path::new("/definitely/not/here/config.toml")).unwrap();
        assert_eq!(c, Config::default());
    }
}
