//! Screen backlight brightness.
//!
//! Reading comes from `/sys/class/backlight`. Writing goes through
//! systemd-logind, which lets the user at the keyboard change the
//! backlight without root or udev rules.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The kernel's backlight directory.
pub const BACKLIGHT_DIR: &str = "/sys/class/backlight";

/// One backlight device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlight {
    /// The device's directory name, e.g. `intel_backlight`.
    pub name: String,
    pub value: u32,
    pub max: u32,
}

impl Backlight {
    /// Brightness from 0 to 100.
    pub fn percent(&self) -> u32 {
        if self.max == 0 {
            return 0;
        }
        ((self.value as u64 * 100 + self.max as u64 / 2) / self.max as u64).min(100) as u32
    }

    /// The raw value for a brightness percentage. Never returns less
    /// than the smallest step above zero, so the screen cannot be
    /// turned fully black from the keyboard.
    pub fn raw_for(&self, percent: u32) -> u32 {
        let percent = percent.clamp(1, 100) as u64;
        let raw = (self.max as u64 * percent + 50) / 100;
        raw.clamp(1, self.max.max(1) as u64) as u32
    }

    /// The brightness after stepping by `delta` percentage points,
    /// kept between 1 and 100.
    pub fn stepped(&self, delta: i32) -> u32 {
        (self.percent() as i32 + delta).clamp(1, 100) as u32
    }
}

fn read_number(path: &Path) -> Option<u32> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Finds the backlight to control in `dir`. Machines without one (most
/// desktop PCs) give `None`.
///
/// When several exist, the kernel's own ranking of interface types is
/// followed: `firmware` first, then `platform`, then `raw`.
pub fn read(dir: &Path) -> Option<Backlight> {
    let mut found: Vec<(u8, Backlight)> = Vec::new();
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path: PathBuf = entry.path();
        let (Some(value), Some(max)) = (
            read_number(&path.join("brightness")),
            read_number(&path.join("max_brightness")),
        ) else {
            continue;
        };
        if max == 0 {
            continue;
        }
        let rank = match fs::read_to_string(path.join("type"))
            .unwrap_or_default()
            .trim()
        {
            "firmware" => 0,
            "platform" => 1,
            "raw" => 2,
            _ => 3,
        };
        let name = entry.file_name().to_string_lossy().to_string();
        found.push((
            rank,
            Backlight {
                name,
                value: value.min(max),
                max,
            },
        ));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));
    found.into_iter().next().map(|(_, backlight)| backlight)
}

/// Reads the real backlight.
pub fn read_system() -> Option<Backlight> {
    read(Path::new(BACKLIGHT_DIR))
}

/// The `busctl` arguments that ask logind to set a backlight.
pub fn command(name: &str, raw: u32) -> Vec<String> {
    [
        "call",
        "org.freedesktop.login1",
        "/org/freedesktop/login1/session/auto",
        "org.freedesktop.login1.Session",
        "SetBrightness",
        "ssu",
        "backlight",
        name,
        &raw.to_string(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Steps the brightness by `delta` percentage points and returns the
/// new percentage.
pub fn step(delta: i32) -> Result<u32, String> {
    let backlight = read_system()
        .ok_or_else(|| "This machine has no screen backlight to adjust.".to_string())?;
    let target = backlight.stepped(delta);
    let status = Command::new("busctl")
        .args(command(&backlight.name, backlight.raw_for(target)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("Could not run busctl to set the brightness: {e}"))?;
    if !status.success() {
        return Err("The system refused the brightness change.".to_string());
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str, devices: &[(&str, &str, u32, u32)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("scuttle-bl-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for (device, kind, value, max) in devices {
            let path = dir.join(device);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("type"), format!("{kind}\n")).unwrap();
            fs::write(path.join("brightness"), format!("{value}\n")).unwrap();
            fs::write(path.join("max_brightness"), format!("{max}\n")).unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn desktop_pc_has_no_backlight() {
        let dir = fixture("none", &[]);
        assert_eq!(read(&dir), None);
        assert_eq!(read(Path::new("/nonexistent/scuttle")), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_a_laptop_backlight() {
        let dir = fixture("laptop", &[("intel_backlight", "raw", 480, 960)]);
        let b = read(&dir).unwrap();
        assert_eq!(b.name, "intel_backlight");
        assert_eq!(b.percent(), 50);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prefers_firmware_over_raw_interfaces() {
        let dir = fixture(
            "two",
            &[
                ("amdgpu_bl1", "raw", 100, 255),
                ("acpi_video0", "firmware", 5, 10),
            ],
        );
        assert_eq!(read(&dir).unwrap().name, "acpi_video0");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn broken_devices_are_skipped() {
        let dir = fixture("broken", &[("zero", "raw", 0, 0), ("good", "raw", 3, 10)]);
        assert_eq!(read(&dir).unwrap().name, "good");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stepping_stays_between_one_and_a_hundred() {
        let b = Backlight {
            name: "x".into(),
            value: 480,
            max: 960,
        };
        assert_eq!(b.stepped(5), 55);
        assert_eq!(b.stepped(-5), 45);
        assert_eq!(b.stepped(500), 100);
        assert_eq!(b.stepped(-500), 1);
    }

    #[test]
    fn the_screen_never_goes_fully_black() {
        let coarse = Backlight {
            name: "x".into(),
            value: 1,
            max: 10,
        };
        assert_eq!(coarse.raw_for(1), 1);
        assert_eq!(coarse.raw_for(0), 1);
        let fine = Backlight {
            name: "x".into(),
            value: 1,
            max: 96_000,
        };
        assert_eq!(fine.raw_for(1), 960);
        assert_eq!(fine.raw_for(100), 96_000);
        assert_eq!(fine.raw_for(250), 96_000);
    }

    #[test]
    fn percent_rounds_and_handles_odd_values() {
        assert_eq!(
            Backlight {
                name: "x".into(),
                value: 1,
                max: 3
            }
            .percent(),
            33
        );
        assert_eq!(
            Backlight {
                name: "x".into(),
                value: 2,
                max: 3
            }
            .percent(),
            67
        );
        assert_eq!(
            Backlight {
                name: "x".into(),
                value: 0,
                max: 0
            }
            .percent(),
            0
        );
    }

    #[test]
    fn builds_the_logind_call() {
        let args = command("intel_backlight", 480);
        assert_eq!(
            args[..5],
            [
                "call",
                "org.freedesktop.login1",
                "/org/freedesktop/login1/session/auto",
                "org.freedesktop.login1.Session",
                "SetBrightness"
            ]
        );
        assert_eq!(args[5..], ["ssu", "backlight", "intel_backlight", "480"]);
    }
}
