//! Theme tokens: the single set of colours every Scuttle surface, and
//! every exported app theme, is built from.

use serde::{Deserialize, Serialize};

use crate::color::Rgba;

/// Dark or light variant of a theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

/// A complete set of colour tokens.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    /// Name the theme is selected by in the config file.
    pub name: &'static str,
    pub mode: Mode,
    /// The darkest (or, in light mode, the base) backdrop colour.
    pub ground: Rgba,
    /// Opaque colour of panels, bars and cards before translucency.
    pub surface: Rgba,
    /// Hairline borders and dividers.
    pub line: Rgba,
    /// Primary text.
    pub text: Rgba,
    /// Secondary text that still needs to be easy to read.
    pub text_mid: Rgba,
    /// Labels, hints and metadata.
    pub text_dim: Rgba,
    /// The accent used for fills: selection, toggles, primary buttons.
    pub accent: Rgba,
    /// Text and icons drawn on top of an accent fill.
    pub on_accent: Rgba,
    /// A variant of the accent that is readable as small text on `surface`.
    pub accent_text: Rgba,
}

const fn c(r: u8, g: u8, b: u8) -> Rgba {
    Rgba::rgb(r, g, b)
}

/// Names of the built-in themes, in the order the settings app lists them.
pub const BUILTIN: &[&str] = &["ferris", "tide", "dune", "bone"];

impl Theme {
    /// Looks up a built-in theme. Returns `None` for an unknown name.
    pub fn builtin(name: &str, mode: Mode) -> Option<Theme> {
        let theme = match (name, mode) {
            // Ferris orange on warm charcoal. The default.
            ("ferris", Mode::Dark) => Theme {
                name: "ferris",
                mode,
                ground: c(0x17, 0x13, 0x10),
                surface: c(0x1e, 0x19, 0x16),
                line: Rgba::rgb(0xff, 0xf0, 0xe1).with_alpha(0.10),
                text: c(0xf6, 0xef, 0xe8),
                text_mid: c(0xd9, 0xcd, 0xc1),
                text_dim: c(0xb8, 0xab, 0x9f),
                accent: c(0xf7, 0x4c, 0x00),
                on_accent: c(0x1a, 0x0d, 0x05),
                accent_text: c(0xff, 0x7a, 0x3d),
            },
            ("ferris", Mode::Light) => Theme {
                name: "ferris",
                mode,
                ground: c(0xef, 0xe9, 0xe2),
                surface: c(0xfb, 0xf8, 0xf4),
                line: Rgba::rgb(0x2a, 0x1a, 0x0e).with_alpha(0.12),
                text: c(0x1d, 0x16, 0x12),
                text_mid: c(0x4a, 0x3f, 0x37),
                text_dim: c(0x6b, 0x5f, 0x55),
                accent: c(0xf7, 0x4c, 0x00),
                on_accent: c(0x1a, 0x0d, 0x05),
                accent_text: c(0xb2, 0x38, 0x00),
            },
            // Pale ice on near-black blue.
            ("tide", Mode::Dark) => Theme {
                name: "tide",
                mode,
                ground: c(0x0e, 0x11, 0x15),
                surface: c(0x16, 0x1a, 0x20),
                line: Rgba::rgb(0xe6, 0xf2, 0xff).with_alpha(0.10),
                text: c(0xf1, 0xf3, 0xf5),
                text_mid: c(0xcd, 0xd3, 0xda),
                text_dim: c(0xa7, 0xad, 0xb5),
                accent: c(0xbf, 0xe3, 0xff),
                on_accent: c(0x0b, 0x16, 0x20),
                accent_text: c(0xbf, 0xe3, 0xff),
            },
            ("tide", Mode::Light) => Theme {
                name: "tide",
                mode,
                ground: c(0xe6, 0xea, 0xee),
                surface: c(0xf7, 0xf9, 0xfb),
                line: Rgba::rgb(0x10, 0x20, 0x30).with_alpha(0.12),
                text: c(0x14, 0x1a, 0x21),
                text_mid: c(0x3b, 0x45, 0x50),
                text_dim: c(0x58, 0x63, 0x6e),
                accent: c(0x1f, 0x5f, 0x99),
                on_accent: c(0xf7, 0xf9, 0xfb),
                accent_text: c(0x1f, 0x5f, 0x99),
            },
            // Sand and ink.
            ("dune", Mode::Dark) => Theme {
                name: "dune",
                mode,
                ground: c(0x16, 0x14, 0x10),
                surface: c(0x1f, 0x1c, 0x17),
                line: Rgba::rgb(0xff, 0xf3, 0xdc).with_alpha(0.10),
                text: c(0xf3, 0xee, 0xe3),
                text_mid: c(0xd6, 0xcd, 0xbc),
                text_dim: c(0xb3, 0xa8, 0x95),
                accent: c(0xe2, 0xc9, 0xa0),
                on_accent: c(0x1c, 0x16, 0x0b),
                accent_text: c(0xe2, 0xc9, 0xa0),
            },
            ("dune", Mode::Light) => Theme {
                name: "dune",
                mode,
                ground: c(0xe7, 0xe3, 0xda),
                surface: c(0xfb, 0xf9, 0xf4),
                line: Rgba::rgb(0x2a, 0x22, 0x10).with_alpha(0.12),
                text: c(0x1d, 0x1b, 0x16),
                text_mid: c(0x46, 0x40, 0x35),
                text_dim: c(0x63, 0x5b, 0x4c),
                accent: c(0x7a, 0x55, 0x1e),
                on_accent: c(0xfb, 0xf9, 0xf4),
                accent_text: c(0x7a, 0x55, 0x1e),
            },
            // Monochrome.
            ("bone", Mode::Dark) => Theme {
                name: "bone",
                mode,
                ground: c(0x11, 0x11, 0x11),
                surface: c(0x1a, 0x1a, 0x1a),
                line: Rgba::rgb(0xff, 0xff, 0xff).with_alpha(0.10),
                text: c(0xf2, 0xf2, 0xf0),
                text_mid: c(0xd0, 0xd0, 0xcc),
                text_dim: c(0xa8, 0xa8, 0xa4),
                accent: c(0xef, 0xe9, 0xe2),
                on_accent: c(0x14, 0x14, 0x14),
                accent_text: c(0xef, 0xe9, 0xe2),
            },
            ("bone", Mode::Light) => Theme {
                name: "bone",
                mode,
                ground: c(0xe9, 0xe9, 0xe6),
                surface: c(0xfa, 0xfa, 0xf8),
                line: Rgba::rgb(0x00, 0x00, 0x00).with_alpha(0.12),
                text: c(0x16, 0x16, 0x16),
                text_mid: c(0x40, 0x40, 0x3e),
                text_dim: c(0x5c, 0x5c, 0x59),
                accent: c(0x1c, 0x1c, 0x1c),
                on_accent: c(0xfa, 0xfa, 0xf8),
                accent_text: c(0x1c, 0x1c, 0x1c),
            },
            _ => return None,
        };
        Some(theme)
    }

    /// Replaces the accent with a custom colour and recomputes the tokens
    /// that depend on it so text stays readable.
    pub fn with_accent(mut self, accent: Rgba) -> Theme {
        let accent = accent.with_alpha(1.0);
        self.accent = accent;
        self.on_accent = accent.best_on(Rgba::rgb(0x14, 0x10, 0x0c), Rgba::rgb(0xfb, 0xf8, 0xf4));
        self.accent_text = readable_on(accent, self.surface, self.text);
        self
    }

    /// The accent at low opacity, for tinted backgrounds.
    pub fn accent_soft(&self) -> Rgba {
        self.accent.with_alpha(0.16)
    }

    /// The accent at medium opacity, for borders around tinted areas.
    pub fn accent_edge(&self) -> Rgba {
        self.accent.with_alpha(0.45)
    }

    /// A faint fill used for hover states and inset wells.
    pub fn wash(&self) -> Rgba {
        self.text.with_alpha(0.07)
    }

    /// A stronger fill for pressed and "selected but not accent" states.
    pub fn wash_strong(&self) -> Rgba {
        self.text.with_alpha(0.14)
    }
}

/// Nudges `colour` towards `towards` until it reaches a 4.5:1 contrast
/// ratio against `background`. Returns the first step that passes.
fn readable_on(colour: Rgba, background: Rgba, towards: Rgba) -> Rgba {
    const TARGET: f32 = 4.5;
    for step in 0..=20 {
        let candidate = colour.mix(towards, step as f32 / 20.0);
        if candidate.contrast(background) >= TARGET {
            return candidate;
        }
    }
    towards
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Vec<Theme> {
        let mut out = Vec::new();
        for name in BUILTIN {
            for mode in [Mode::Dark, Mode::Light] {
                out.push(Theme::builtin(name, mode).expect("listed theme must exist"));
            }
        }
        out
    }

    #[test]
    fn every_listed_theme_exists_in_both_modes() {
        assert_eq!(all().len(), BUILTIN.len() * 2);
    }

    #[test]
    fn unknown_theme_is_none() {
        assert!(Theme::builtin("nope", Mode::Dark).is_none());
    }

    #[test]
    fn every_theme_meets_contrast_targets() {
        for t in all() {
            let id = format!("{} {:?}", t.name, t.mode);
            assert!(t.text.contrast(t.surface) >= 7.0, "{id}: text on surface");
            assert!(t.text.contrast(t.ground) >= 7.0, "{id}: text on ground");
            assert!(t.text_mid.contrast(t.surface) >= 4.5, "{id}: text_mid");
            assert!(t.text_dim.contrast(t.surface) >= 4.5, "{id}: text_dim on surface");
            assert!(t.text_dim.contrast(t.ground) >= 4.5, "{id}: text_dim on ground");
            assert!(t.on_accent.contrast(t.accent) >= 4.5, "{id}: on_accent");
            assert!(t.accent_text.contrast(t.surface) >= 4.5, "{id}: accent_text");
        }
    }

    #[test]
    fn custom_accent_keeps_text_readable() {
        for hex in ["#f74c00", "#ffe066", "#101010", "#3355ff", "#ffffff"] {
            let accent: Rgba = hex.parse().unwrap();
            for mode in [Mode::Dark, Mode::Light] {
                let t = Theme::builtin("ferris", mode).unwrap().with_accent(accent);
                assert!(
                    t.on_accent.contrast(t.accent) >= 4.5,
                    "{hex} {mode:?}: on_accent"
                );
                assert!(
                    t.accent_text.contrast(t.surface) >= 4.5,
                    "{hex} {mode:?}: accent_text"
                );
            }
        }
    }
}
