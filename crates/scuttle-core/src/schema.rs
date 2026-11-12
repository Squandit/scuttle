//! A description of every setting: its name, what it does, and what
//! kind of control changes it.
//!
//! The settings window is drawn from this, so a setting added to the
//! config has to be described here too. A test checks the two lists
//! match in both directions.

/// The kind of value a setting holds, and so the control it gets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// On or off.
    Toggle,
    /// One of a fixed set: the value written, and how it is shown.
    Choice(&'static [(&'static str, &'static str)]),
    /// A whole number in a range.
    Integer {
        min: i64,
        max: i64,
        step: i64,
        unit: &'static str,
    },
    /// A number with decimals in a range.
    Decimal { min: f64, max: f64, step: f64 },
    /// Free text.
    Text,
    /// A colour, or nothing to use the theme's own.
    Colour,
    /// A list of words separated by commas, such as bar widgets.
    List,
}

/// One setting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setting {
    /// Where it lives in the config file, as `section.name`.
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    /// Whether leaving it out of the file is a real choice of its own
    /// (follow the theme, find it automatically) rather than a default.
    pub optional: bool,
    /// Whether a change only applies after Scuttle restarts.
    pub restart: bool,
}

/// A page of settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Section {
    /// The table name in the config file.
    pub id: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
    pub settings: &'static [Setting],
}

const fn setting(
    key: &'static str,
    label: &'static str,
    help: &'static str,
    kind: Kind,
) -> Setting {
    Setting {
        key,
        label,
        help,
        kind,
        optional: false,
        restart: false,
    }
}

const fn optional(mut s: Setting) -> Setting {
    s.optional = true;
    s
}

const fn restart(mut s: Setting) -> Setting {
    s.restart = true;
    s
}

/// The bar widgets, as written in the config file.
pub const MODULES: &[&str] = &[
    "workspaces",
    "title",
    "clock",
    "media",
    "cpu",
    "memory",
    "network",
    "volume",
    "battery",
    "tray",
    "notifications",
];

const MODULE_HELP: &str = "Widgets in order, separated by commas: workspaces, title, clock, media, cpu, memory, network, volume, battery, tray, notifications.";

pub const SECTIONS: &[Section] = &[
    Section {
        id: "appearance",
        title: "Appearance",
        blurb: "Colours, size, shape and motion. Everything Scuttle draws follows these.",
        settings: &[
            setting(
                "appearance.theme",
                "Theme",
                "Wallpaper takes the colours from matugen's palette, so Scuttle matches the rest of the desktop.",
                Kind::Choice(&[
                    ("ferris", "Ferris"),
                    ("tide", "Tide"),
                    ("dune", "Dune"),
                    ("bone", "Bone"),
                    ("wallpaper", "Wallpaper"),
                ]),
            ),
            setting(
                "appearance.mode",
                "Mode",
                "Dark or light version of the theme.",
                Kind::Choice(&[("dark", "Dark"), ("light", "Light")]),
            ),
            optional(setting(
                "appearance.accent",
                "Accent colour",
                "Replaces the theme's accent. Text on it is adjusted to stay readable.",
                Kind::Colour,
            )),
            setting(
                "appearance.scale",
                "Size",
                "Makes the bar, launcher, text and spacing bigger or smaller together.",
                Kind::Decimal {
                    min: 0.75,
                    max: 2.5,
                    step: 0.05,
                },
            ),
            setting(
                "appearance.radius",
                "Corner radius",
                "How round panels are. 0 is square.",
                Kind::Integer {
                    min: 0,
                    max: 32,
                    step: 1,
                    unit: "px",
                },
            ),
            setting(
                "appearance.motion",
                "Motion",
                "How much things animate.",
                Kind::Choice(&[("off", "Off"), ("subtle", "Subtle"), ("full", "Full")]),
            ),
            setting(
                "appearance.blur",
                "Blur behind panels",
                "Panels are translucent with blur and nearly solid without. Needs the blur rules from `scuttle hyprland`.",
                Kind::Toggle,
            ),
            setting(
                "appearance.font",
                "Font",
                "For most text. Falls back to Inter when not installed.",
                Kind::Text,
            ),
            setting(
                "appearance.font_display",
                "Display font",
                "For large text such as the launcher's search box and clocks.",
                Kind::Text,
            ),
            setting(
                "appearance.font_mono",
                "Monospace font",
                "For numbers, paths and code.",
                Kind::Text,
            ),
            optional(setting(
                "appearance.palette_file",
                "Palette file",
                "The colours file read by the Wallpaper theme. Leave empty to find matugen's automatically.",
                Kind::Text,
            )),
        ],
    },
    Section {
        id: "bar",
        title: "Bar",
        blurb: "Where the bar sits, its shape, and what is on it.",
        settings: &[
            setting(
                "bar.position",
                "Position",
                "With no bar, the clock and status show in the launcher instead.",
                Kind::Choice(&[("top", "Top"), ("bottom", "Bottom"), ("none", "No bar")]),
            ),
            setting(
                "bar.style",
                "Shape",
                "One strip, three floating islands, or a small notch.",
                Kind::Choice(&[("strip", "Strip"), ("islands", "Islands"), ("notch", "Notch")]),
            ),
            setting(
                "bar.height",
                "Height",
                "Before the size setting is applied.",
                Kind::Integer {
                    min: 24,
                    max: 72,
                    step: 1,
                    unit: "px",
                },
            ),
            setting(
                "bar.margin",
                "Gap from the screen edge",
                "",
                Kind::Integer {
                    min: 0,
                    max: 64,
                    step: 1,
                    unit: "px",
                },
            ),
            setting("bar.left", "Left side", MODULE_HELP, Kind::List),
            setting("bar.center", "Centre", MODULE_HELP, Kind::List),
            setting("bar.right", "Right side", MODULE_HELP, Kind::List),
            setting(
                "bar.clock_format",
                "Clock format",
                "%H:%M is 24 hour time, %I:%M %p is 12 hour, %a %d %b is the date.",
                Kind::Text,
            ),
            setting(
                "bar.workspaces",
                "Workspace buttons",
                "How many are always shown. More appear as you use them.",
                Kind::Integer {
                    min: 1,
                    max: 20,
                    step: 1,
                    unit: "",
                },
            ),
        ],
    },
    Section {
        id: "launcher",
        title: "Launcher",
        blurb: "Search for apps, windows, files and the rest.",
        settings: &[
            setting(
                "launcher.max_results",
                "Results shown",
                "",
                Kind::Integer {
                    min: 1,
                    max: 50,
                    step: 1,
                    unit: "",
                },
            ),
            setting(
                "launcher.web_search",
                "Web search",
                "The address searches open. {query} is replaced with what you typed.",
                Kind::Text,
            ),
            setting(
                "launcher.terminal",
                "Terminal",
                "For apps that need one. Leave empty to use $TERMINAL or the first one installed.",
                Kind::Text,
            ),
            setting(
                "launcher.file_roots",
                "Folders to search",
                "Separated by commas. Names without a leading / start at your home folder.",
                Kind::List,
            ),
        ],
    },
    Section {
        id: "notifications",
        title: "Notifications",
        blurb: "Toasts from apps, and where they appear.",
        settings: &[
            restart(setting(
                "notifications.enabled",
                "Show notifications",
                "Turn off to keep using mako, dunst or another notification daemon.",
                Kind::Toggle,
            )),
            setting(
                "notifications.position",
                "Corner",
                "",
                Kind::Choice(&[
                    ("top-right", "Top right"),
                    ("top-left", "Top left"),
                    ("bottom-right", "Bottom right"),
                    ("bottom-left", "Bottom left"),
                ]),
            ),
            setting(
                "notifications.timeout_ms",
                "Time on screen",
                "When the app does not say. Critical notifications stay until dismissed.",
                Kind::Integer {
                    min: 1000,
                    max: 60_000,
                    step: 500,
                    unit: "ms",
                },
            ),
            setting(
                "notifications.max_visible",
                "Most at once",
                "Older ones make room and stay in the history.",
                Kind::Integer {
                    min: 1,
                    max: 10,
                    step: 1,
                    unit: "",
                },
            ),
        ],
    },
    Section {
        id: "sound",
        title: "Sound",
        blurb: "Soft tones for opening the launcher, moving through results and the like.",
        settings: &[
            setting(
                "sound.feedback",
                "Interface sounds",
                "Headphones means only while the output is headphones or a headset.",
                Kind::Choice(&[
                    ("never", "Never"),
                    ("headphones", "With headphones"),
                    ("always", "Always"),
                ]),
            ),
            setting(
                "sound.volume",
                "Loudness",
                "",
                Kind::Decimal {
                    min: 0.0,
                    max: 1.0,
                    step: 0.05,
                },
            ),
        ],
    },
    Section {
        id: "assistant",
        title: "Assistant",
        blurb: "The side panel that asks Claude. The API key is set with `scuttle assistant set-key`, never here.",
        settings: &[
            restart(setting(
                "assistant.enabled",
                "Assistant panel",
                "",
                Kind::Toggle,
            )),
            setting(
                "assistant.backend",
                "Reach Claude through",
                "Automatic uses your subscription and switches to the API key when it reaches its limit.",
                Kind::Choice(&[
                    ("auto", "Automatic"),
                    ("subscription", "Subscription only"),
                    ("api", "API key only"),
                ]),
            ),
            setting(
                "assistant.subscription_model",
                "Model on the subscription",
                "",
                Kind::Text,
            ),
            setting(
                "assistant.api_model",
                "Model on the API key",
                "",
                Kind::Text,
            ),
            setting(
                "assistant.side",
                "Side of the screen",
                "",
                Kind::Choice(&[("right", "Right"), ("left", "Left")]),
            ),
            setting(
                "assistant.width",
                "Width",
                "Before the size setting is applied.",
                Kind::Integer {
                    min: 320,
                    max: 900,
                    step: 10,
                    unit: "px",
                },
            ),
        ],
    },
    Section {
        id: "power",
        title: "Battery",
        blurb: "What changes on a laptop running on battery. A desktop PC ignores these.",
        settings: &[
            setting(
                "power.battery_disables_blur",
                "Turn blur off on battery",
                "",
                Kind::Toggle,
            ),
            setting(
                "power.battery_limits_motion",
                "Limit motion on battery",
                "Caps motion at Subtle.",
                Kind::Toggle,
            ),
        ],
    },
];

/// Every setting, across all sections.
pub fn all() -> impl Iterator<Item = &'static Setting> {
    SECTIONS.iter().flat_map(|section| section.settings.iter())
}

/// The setting stored at `key`, if there is one.
pub fn find(key: &str) -> Option<&'static Setting> {
    all().find(|setting| setting.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::edit::{self, Value};
    use std::collections::BTreeSet;

    /// Every `section.name` in the default config, plus the optional
    /// ones that are left out of it.
    fn config_keys() -> BTreeSet<String> {
        let table: toml::Table = toml::from_str(&Config::default().to_toml()).unwrap();
        let mut keys = BTreeSet::new();
        for (section, value) in &table {
            for name in value.as_table().unwrap().keys() {
                keys.insert(format!("{section}.{name}"));
            }
        }
        keys.insert("appearance.accent".to_string());
        keys.insert("appearance.palette_file".to_string());
        keys
    }

    #[test]
    fn every_setting_in_the_config_is_described_and_nothing_else() {
        let described: BTreeSet<String> = all().map(|s| s.key.to_string()).collect();
        assert_eq!(described, config_keys());
        assert_eq!(described.len(), all().count(), "a key is described twice");
    }

    #[test]
    fn settings_live_in_their_own_section() {
        for section in SECTIONS {
            for setting in section.settings {
                assert!(
                    setting.key.starts_with(&format!("{}.", section.id)),
                    "{} is on the {} page",
                    setting.key,
                    section.title
                );
                assert!(!setting.label.is_empty(), "{}", setting.key);
            }
        }
    }

    #[test]
    fn every_choice_is_accepted_by_the_config() {
        for setting in all() {
            let Kind::Choice(choices) = setting.kind else {
                continue;
            };
            for (value, label) in choices {
                assert!(!label.is_empty());
                edit::set("", setting.key, &Value::Str(value.to_string()))
                    .unwrap_or_else(|e| panic!("{} = {value}: {e}", setting.key));
            }
        }
    }

    #[test]
    fn the_default_is_one_of_the_choices() {
        let config = Config::default();
        for setting in all() {
            let Kind::Choice(choices) = setting.kind else {
                continue;
            };
            let Some(Value::Str(current)) = edit::get(&config, setting.key) else {
                panic!("{} has no default", setting.key);
            };
            assert!(
                choices.iter().any(|(value, _)| *value == current),
                "{} defaults to {current}, which is not offered",
                setting.key
            );
        }
    }

    #[test]
    fn both_ends_of_every_range_are_accepted() {
        for setting in all() {
            match setting.kind {
                Kind::Integer { min, max, .. } => {
                    for value in [min, max] {
                        edit::set("", setting.key, &Value::Int(value))
                            .unwrap_or_else(|e| panic!("{} = {value}: {e}", setting.key));
                    }
                    for value in [min - 1, max + 1] {
                        assert!(
                            edit::set("", setting.key, &Value::Int(value)).is_err(),
                            "{} = {value} should be refused",
                            setting.key
                        );
                    }
                }
                Kind::Decimal { min, max, .. } => {
                    for value in [min, max] {
                        edit::set("", setting.key, &Value::Float(value))
                            .unwrap_or_else(|e| panic!("{} = {value}: {e}", setting.key));
                    }
                }
                _ => {}
            }
        }
    }

    #[test]
    fn the_module_list_matches_the_config() {
        // Every listed widget is accepted, and the default bar uses
        // only listed ones.
        let all_modules = Value::List(MODULES.iter().map(|m| m.to_string()).collect());
        edit::set("", "bar.right", &all_modules).unwrap();
        let config = Config::default();
        for key in ["bar.left", "bar.center", "bar.right"] {
            let Some(Value::List(items)) = edit::get(&config, key) else {
                panic!("{key} is not a list");
            };
            for item in items {
                assert!(MODULES.contains(&item.as_str()), "{item}");
            }
        }
    }

    #[test]
    fn settings_can_be_looked_up() {
        assert_eq!(find("bar.style").unwrap().label, "Shape");
        assert!(find("bar.nonsense").is_none());
        assert!(find("appearance.accent").unwrap().optional);
        assert!(find("notifications.enabled").unwrap().restart);
    }
}
