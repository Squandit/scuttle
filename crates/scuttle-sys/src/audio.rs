//! Output volume, read and changed through `wpctl` (WirePlumber, the
//! session manager that ships with PipeWire).

use std::process::{Command, Stdio};

/// The default audio output's volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    /// 0 to 100, or above 100 when boosted.
    pub percent: u32,
    pub muted: bool,
}

impl Volume {
    /// The level to draw: zero when muted.
    pub fn audible_percent(self) -> u32 {
        if self.muted {
            0
        } else {
            self.percent
        }
    }
}

/// A requested change to the volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeChange {
    /// Raise by this many percentage points.
    Up(u32),
    /// Lower by this many percentage points.
    Down(u32),
    ToggleMute,
}

const SINK: &str = "@DEFAULT_AUDIO_SINK@";

/// Parses the output of `wpctl get-volume`, which looks like
/// `Volume: 0.40` or `Volume: 0.40 [MUTED]`.
pub fn parse(output: &str) -> Option<Volume> {
    let rest = output.trim().strip_prefix("Volume:")?.trim();
    let mut parts = rest.split_whitespace();
    let level: f32 = parts.next()?.parse().ok()?;
    if !level.is_finite() || level < 0.0 {
        return None;
    }
    let muted = parts.any(|p| p == "[MUTED]");
    Some(Volume {
        percent: (level * 100.0).round() as u32,
        muted,
    })
}

/// The `wpctl` arguments that carry out a change.
pub fn command(change: VolumeChange) -> Vec<String> {
    let step = |points: u32, sign: char| {
        vec![
            "set-volume".to_string(),
            // Never go past 100% from the keyboard. Boosting beyond
            // that distorts and is easy to do by accident.
            "--limit".to_string(),
            "1.0".to_string(),
            SINK.to_string(),
            format!("{}%{sign}", points.clamp(1, 100)),
        ]
    };
    match change {
        VolumeChange::Up(points) => step(points, '+'),
        VolumeChange::Down(points) => step(points, '-'),
        VolumeChange::ToggleMute => {
            vec![
                "set-mute".to_string(),
                SINK.to_string(),
                "toggle".to_string(),
            ]
        }
    }
}

fn wpctl(args: &[String]) -> Option<String> {
    let output = Command::new("wpctl")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

/// Reads the current volume. `None` when `wpctl` is missing or there is
/// no audio output.
pub fn read() -> Option<Volume> {
    parse(&wpctl(&["get-volume".to_string(), SINK.to_string()])?)
}

/// Applies a change and returns the resulting volume.
pub fn change(change: VolumeChange) -> Result<Volume, String> {
    // Raising the volume while muted should be heard.
    if matches!(change, VolumeChange::Up(_)) && read().is_some_and(|v| v.muted) {
        let _ = wpctl(&["set-mute".to_string(), SINK.to_string(), "0".to_string()]);
    }
    wpctl(&command(change)).ok_or_else(|| {
        "Could not change the volume. This needs wpctl, which comes with the wireplumber package."
            .to_string()
    })?;
    read().ok_or_else(|| "The volume changed but could not be read back.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_muted_output() {
        assert_eq!(
            parse("Volume: 0.40\n"),
            Some(Volume {
                percent: 40,
                muted: false
            })
        );
        assert_eq!(
            parse("Volume: 0.40 [MUTED]\n"),
            Some(Volume {
                percent: 40,
                muted: true
            })
        );
        assert_eq!(
            parse("Volume: 1.00"),
            Some(Volume {
                percent: 100,
                muted: false
            })
        );
        assert_eq!(
            parse("Volume: 0.00"),
            Some(Volume {
                percent: 0,
                muted: false
            })
        );
    }

    #[test]
    fn rounds_instead_of_truncating() {
        // 0.35 is not exact in binary and would truncate to 34.
        assert_eq!(parse("Volume: 0.35").unwrap().percent, 35);
        assert_eq!(parse("Volume: 0.07").unwrap().percent, 7);
    }

    #[test]
    fn boosted_volume_is_reported_above_100() {
        assert_eq!(parse("Volume: 1.25").unwrap().percent, 125);
    }

    #[test]
    fn rejects_anything_else() {
        for bad in [
            "",
            "Volume:",
            "Volume: loud",
            "Volume: -1",
            "Volume: NaN",
            "something else",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn muted_volume_draws_as_zero() {
        assert_eq!(
            Volume {
                percent: 60,
                muted: true
            }
            .audible_percent(),
            0
        );
        assert_eq!(
            Volume {
                percent: 60,
                muted: false
            }
            .audible_percent(),
            60
        );
    }

    #[test]
    fn builds_the_right_commands() {
        assert_eq!(
            command(VolumeChange::Up(5)),
            [
                "set-volume",
                "--limit",
                "1.0",
                "@DEFAULT_AUDIO_SINK@",
                "5%+"
            ]
        );
        assert_eq!(
            command(VolumeChange::Down(10)),
            [
                "set-volume",
                "--limit",
                "1.0",
                "@DEFAULT_AUDIO_SINK@",
                "10%-"
            ]
        );
        assert_eq!(
            command(VolumeChange::ToggleMute),
            ["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]
        );
    }

    #[test]
    fn steps_are_kept_sensible() {
        assert_eq!(command(VolumeChange::Up(0)).last().unwrap(), "1%+");
        assert_eq!(command(VolumeChange::Down(5000)).last().unwrap(), "100%-");
    }
}
