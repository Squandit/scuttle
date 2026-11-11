//! A small RGBA colour type with the handful of operations the theme
//! engine needs: hex parsing, CSS output, mixing and WCAG contrast.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An sRGB colour with straight (non-premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    /// Opacity from 0.0 (transparent) to 1.0 (opaque).
    pub a: f32,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// The same colour with a different opacity.
    pub fn with_alpha(self, a: f32) -> Self {
        Self {
            a: a.clamp(0.0, 1.0),
            ..self
        }
    }

    /// Linear blend towards `other`. `t = 0` is `self`, `t = 1` is `other`.
    pub fn mix(self, other: Rgba, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let ch = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Self {
            r: ch(self.r, other.r),
            g: ch(self.g, other.g),
            b: ch(self.b, other.b),
            a: self.a + (other.a - self.a) * t,
        }
    }

    /// Relative luminance as defined by WCAG 2.x. Alpha is ignored.
    pub fn luminance(self) -> f32 {
        fn lin(c: u8) -> f32 {
            let c = c as f32 / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// WCAG contrast ratio between two opaque colours, from 1.0 to 21.0.
    pub fn contrast(self, other: Rgba) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Picks whichever of `dark` or `light` reads better on top of `self`.
    pub fn best_on(self, dark: Rgba, light: Rgba) -> Rgba {
        if self.contrast(dark) >= self.contrast(light) {
            dark
        } else {
            light
        }
    }

    /// `#rrggbb`, dropping alpha.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// A CSS colour: `#rrggbb` when opaque, `rgba(r, g, b, a)` otherwise.
    pub fn css(self) -> String {
        if self.a >= 0.999 {
            self.hex()
        } else {
            format!("rgba({}, {}, {}, {:.3})", self.r, self.g, self.b, self.a)
        }
    }
}

impl fmt::Display for Rgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.a >= 0.999 {
            f.write_str(&self.hex())
        } else {
            write!(
                f,
                "#{:02x}{:02x}{:02x}{:02x}",
                self.r,
                self.g,
                self.b,
                (self.a * 255.0).round() as u8
            )
        }
    }
}

/// Error returned when a colour string is not valid hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseColorError(pub String);

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "\"{}\" is not a colour. Use #rgb, #rrggbb or #rrggbbaa.",
            self.0
        )
    }
}

impl std::error::Error for ParseColorError {}

impl FromStr for Rgba {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseColorError(s.to_string());
        let hex = s.trim().strip_prefix('#').ok_or_else(err)?;
        if !hex.is_ascii() {
            return Err(err());
        }
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| err());
        let nibble = |i: usize| {
            u8::from_str_radix(&hex[i..i + 1], 16)
                .map(|n| n * 17)
                .map_err(|_| err())
        };
        match hex.len() {
            3 => Ok(Rgba::rgb(nibble(0)?, nibble(1)?, nibble(2)?)),
            6 => Ok(Rgba::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Ok(Rgba::rgb(byte(0)?, byte(2)?, byte(4)?).with_alpha(byte(6)? as f32 / 255.0)),
            _ => Err(err()),
        }
    }
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_hex_forms() {
        assert_eq!(
            "#f74c00".parse::<Rgba>().unwrap(),
            Rgba::rgb(0xf7, 0x4c, 0x00)
        );
        assert_eq!("#fff".parse::<Rgba>().unwrap(), Rgba::rgb(255, 255, 255));
        let half = "#00000080".parse::<Rgba>().unwrap();
        assert!((half.a - 0.502).abs() < 0.01);
    }

    #[test]
    fn rejects_bad_input() {
        for bad in ["f74c00", "#f74c0", "#gggggg", "", "#ééé"] {
            assert!(bad.parse::<Rgba>().is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn round_trips_through_display() {
        for s in ["#f74c00", "#1a0d05", "#ffffff80"] {
            assert_eq!(s.parse::<Rgba>().unwrap().to_string(), s);
        }
    }

    #[test]
    fn contrast_matches_wcag_reference_values() {
        let black = Rgba::rgb(0, 0, 0);
        let white = Rgba::rgb(255, 255, 255);
        assert!((black.contrast(white) - 21.0).abs() < 0.01);
        assert!((white.contrast(white) - 1.0).abs() < 0.001);
    }

    #[test]
    fn picks_readable_text_colour() {
        let orange: Rgba = "#f74c00".parse().unwrap();
        let dark: Rgba = "#1a0d05".parse().unwrap();
        let light: Rgba = "#f6efe8".parse().unwrap();
        assert_eq!(orange.best_on(dark, light), dark);
        assert_eq!(dark.best_on(dark, light), light);
    }

    #[test]
    fn css_output() {
        assert_eq!(Rgba::rgb(1, 2, 3).css(), "#010203");
        assert_eq!(
            Rgba::rgb(1, 2, 3).with_alpha(0.5).css(),
            "rgba(1, 2, 3, 0.500)"
        );
    }
}
