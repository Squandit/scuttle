//! Battery detection and state.
//!
//! A desktop PC has no system battery, and on those machines
//! [`read`] returns `None` so the shell can leave the battery widget
//! out entirely. Wireless mice and keyboards also show up under
//! `/sys/class/power_supply` as batteries; they are told apart by their
//! `scope` and ignored.

use std::fs;
use std::path::Path;

/// The kernel's power supply directory.
pub const POWER_SUPPLY_DIR: &str = "/sys/class/power_supply";

/// The system battery's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    /// Charge from 0 to 100. With several batteries this is their
    /// combined charge.
    pub percent: u8,
    /// Mains power is connected and the battery is charging or full.
    pub plugged_in: bool,
}

impl Battery {
    /// Running on battery power.
    pub fn discharging(self) -> bool {
        !self.plugged_in
    }

    /// Low enough to warn about.
    pub fn low(self) -> bool {
        self.discharging() && self.percent <= 20
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Reads the system battery from a power supply directory. Returns
/// `None` when the machine has no battery of its own.
pub fn read(dir: &Path) -> Option<Battery> {
    let entries = fs::read_dir(dir).ok()?;
    let mut total_now = 0u64;
    let mut total_full = 0u64;
    let mut percent_sum = 0u32;
    let mut count = 0u32;
    let mut any_discharging = false;
    let mut mains_online = false;

    for entry in entries.flatten() {
        let path = entry.path();
        let kind = read_trimmed(&path.join("type")).unwrap_or_default();
        match kind.as_str() {
            "Mains" | "USB" => {
                if read_trimmed(&path.join("online")).as_deref() == Some("1") {
                    mains_online = true;
                }
            }
            "Battery" => {
                // Peripherals report scope "Device". System batteries
                // report "System" or have no scope file at all.
                if read_trimmed(&path.join("scope")).as_deref() == Some("Device") {
                    continue;
                }
                let Some(capacity) =
                    read_trimmed(&path.join("capacity")).and_then(|s| s.parse::<u32>().ok())
                else {
                    continue;
                };
                count += 1;
                percent_sum += capacity.min(100);
                if read_trimmed(&path.join("status")).as_deref() == Some("Discharging") {
                    any_discharging = true;
                }
                let now = ["energy_now", "charge_now"]
                    .iter()
                    .find_map(|f| read_trimmed(&path.join(f))?.parse::<u64>().ok());
                let full = ["energy_full", "charge_full"]
                    .iter()
                    .find_map(|f| read_trimmed(&path.join(f))?.parse::<u64>().ok());
                if let (Some(now), Some(full)) = (now, full) {
                    total_now += now;
                    total_full += full;
                }
            }
            _ => {}
        }
    }

    if count == 0 {
        return None;
    }
    // Weigh several batteries by their real capacity when the kernel
    // exposes it; fall back to a plain average.
    let percent = if count > 1 && total_full > 0 {
        ((total_now * 100 + total_full / 2) / total_full).min(100) as u8
    } else {
        (percent_sum / count) as u8
    };
    Some(Battery {
        percent,
        plugged_in: mains_online || !any_discharging,
    })
}

/// Reads the real system battery.
pub fn read_system() -> Option<Battery> {
    read(Path::new(POWER_SUPPLY_DIR))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str) -> Fixture {
            let dir =
                std::env::temp_dir().join(format!("scuttle-bat-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Fixture(dir)
        }

        fn supply(&self, name: &str, files: &[(&str, &str)]) -> &Fixture {
            let dir = self.0.join(name);
            fs::create_dir_all(&dir).unwrap();
            for (file, content) in files {
                fs::write(dir.join(file), format!("{content}\n")).unwrap();
            }
            self
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn desktop_pc_has_no_battery() {
        let f = Fixture::new("desktop");
        f.supply("AC", &[("type", "Mains"), ("online", "1")]);
        assert_eq!(read(&f.0), None);
    }

    #[test]
    fn empty_or_missing_directory_has_no_battery() {
        let f = Fixture::new("empty");
        assert_eq!(read(&f.0), None);
        assert_eq!(read(Path::new("/nonexistent/scuttle")), None);
    }

    #[test]
    fn laptop_on_battery() {
        let f = Fixture::new("laptop");
        f.supply("AC", &[("type", "Mains"), ("online", "0")]);
        f.supply(
            "BAT0",
            &[
                ("type", "Battery"),
                ("capacity", "82"),
                ("status", "Discharging"),
            ],
        );
        let b = read(&f.0).unwrap();
        assert_eq!(
            b,
            Battery {
                percent: 82,
                plugged_in: false
            }
        );
        assert!(b.discharging() && !b.low());
    }

    #[test]
    fn laptop_charging() {
        let f = Fixture::new("charging");
        f.supply("AC", &[("type", "Mains"), ("online", "1")]);
        f.supply(
            "BAT0",
            &[
                ("type", "Battery"),
                ("capacity", "40"),
                ("status", "Charging"),
            ],
        );
        assert_eq!(
            read(&f.0),
            Some(Battery {
                percent: 40,
                plugged_in: true
            })
        );
    }

    #[test]
    fn full_battery_without_a_mains_entry_counts_as_plugged_in() {
        let f = Fixture::new("full");
        f.supply(
            "BAT0",
            &[("type", "Battery"), ("capacity", "100"), ("status", "Full")],
        );
        assert_eq!(
            read(&f.0),
            Some(Battery {
                percent: 100,
                plugged_in: true
            })
        );
    }

    #[test]
    fn wireless_mouse_is_not_a_system_battery() {
        let f = Fixture::new("mouse");
        f.supply("AC", &[("type", "Mains"), ("online", "1")]);
        f.supply(
            "hidpp_battery_0",
            &[
                ("type", "Battery"),
                ("scope", "Device"),
                ("capacity", "55"),
                ("status", "Discharging"),
            ],
        );
        assert_eq!(read(&f.0), None);
    }

    #[test]
    fn mouse_does_not_skew_the_laptop_reading() {
        let f = Fixture::new("both");
        f.supply(
            "BAT0",
            &[
                ("type", "Battery"),
                ("capacity", "90"),
                ("status", "Discharging"),
            ],
        );
        f.supply(
            "hidpp_battery_0",
            &[
                ("type", "Battery"),
                ("scope", "Device"),
                ("capacity", "10"),
                ("status", "Discharging"),
            ],
        );
        assert_eq!(read(&f.0).unwrap().percent, 90);
    }

    #[test]
    fn two_batteries_combine_by_capacity() {
        let f = Fixture::new("dual");
        f.supply(
            "BAT0",
            &[
                ("type", "Battery"),
                ("capacity", "100"),
                ("status", "Full"),
                ("energy_now", "20"),
                ("energy_full", "20"),
            ],
        );
        f.supply(
            "BAT1",
            &[
                ("type", "Battery"),
                ("capacity", "50"),
                ("status", "Discharging"),
                ("energy_now", "40"),
                ("energy_full", "80"),
            ],
        );
        let b = read(&f.0).unwrap();
        // 60 of 100 units, not the plain average of 75.
        assert_eq!(b.percent, 60);
        assert!(b.discharging());
    }

    #[test]
    fn low_battery_only_warns_when_unplugged() {
        assert!(Battery {
            percent: 15,
            plugged_in: false
        }
        .low());
        assert!(!Battery {
            percent: 15,
            plugged_in: true
        }
        .low());
        assert!(!Battery {
            percent: 21,
            plugged_in: false
        }
        .low());
    }

    #[test]
    fn garbage_capacity_is_ignored() {
        let f = Fixture::new("garbage");
        f.supply(
            "BAT0",
            &[
                ("type", "Battery"),
                ("capacity", "lots"),
                ("status", "Discharging"),
            ],
        );
        assert_eq!(read(&f.0), None);
    }
}
