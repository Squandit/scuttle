//! Motion settings. Every animated surface in Scuttle reads its timing
//! from here so the whole desktop moves the same way.

use serde::{Deserialize, Serialize};

/// How much the shell animates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Motion {
    /// No animation at all. State changes are instant.
    Off,
    /// Short, quiet transitions. The default.
    #[default]
    Subtle,
    /// Slightly longer transitions with more travel.
    Full,
}

/// The one easing curve used everywhere: quick start, soft landing, no
/// overshoot. Expressed as cubic-bezier control points.
pub const EASE: [f32; 4] = [0.2, 0.8, 0.2, 1.0];

impl Motion {
    /// Duration of a standard enter transition, in milliseconds.
    pub fn duration_ms(self) -> u32 {
        match self {
            Motion::Off => 0,
            Motion::Subtle => 170,
            Motion::Full => 280,
        }
    }

    /// Exits run a little faster than entrances so closing never feels slow.
    pub fn exit_ms(self) -> u32 {
        self.duration_ms() * 7 / 10
    }

    /// Delay between consecutive list items arriving, in milliseconds.
    pub fn stagger_ms(self) -> u32 {
        self.duration_ms() / 7
    }

    /// How far, in pixels, an element travels while it fades in.
    pub fn travel_px(self) -> u32 {
        match self {
            Motion::Off => 0,
            Motion::Subtle => 12,
            Motion::Full => 24,
        }
    }

    /// The easing curve as a CSS timing function.
    pub fn css_ease(self) -> String {
        let [a, b, c, d] = EASE;
        format!("cubic-bezier({a}, {b}, {c}, {d})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_means_zero_everywhere() {
        assert_eq!(Motion::Off.duration_ms(), 0);
        assert_eq!(Motion::Off.exit_ms(), 0);
        assert_eq!(Motion::Off.stagger_ms(), 0);
        assert_eq!(Motion::Off.travel_px(), 0);
    }

    #[test]
    fn exits_are_never_slower_than_entrances() {
        for m in [Motion::Subtle, Motion::Full] {
            assert!(m.exit_ms() < m.duration_ms());
        }
    }

    #[test]
    fn nothing_is_slower_than_280ms() {
        for m in [Motion::Off, Motion::Subtle, Motion::Full] {
            assert!(m.duration_ms() <= 280);
        }
    }
}
