//! Memory use from `/proc/meminfo`.

/// Memory in use, the way `free` reports it: total minus available.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Memory {
    pub used_kib: u64,
    pub total_kib: u64,
}

impl Memory {
    /// Share of memory in use, 0 to 100.
    pub fn percent(self) -> u8 {
        if self.total_kib == 0 {
            return 0;
        }
        ((self.used_kib * 100 + self.total_kib / 2) / self.total_kib).min(100) as u8
    }

    /// Used memory as a short label, e.g. `4.1G` or `512M`.
    pub fn used_label(self) -> String {
        format_kib(self.used_kib)
    }
}

/// Formats an amount of memory in the largest sensible unit.
pub fn format_kib(kib: u64) -> String {
    const MIB: u64 = 1024;
    const GIB: u64 = 1024 * 1024;
    if kib >= GIB {
        let tenths = (kib * 10 + GIB / 2) / GIB;
        format!("{}.{}G", tenths / 10, tenths % 10)
    } else {
        format!("{}M", (kib + MIB / 2) / MIB)
    }
}

/// Parses `/proc/meminfo`.
pub fn parse(meminfo: &str) -> Option<Memory> {
    let field = |name: &str| -> Option<u64> {
        meminfo
            .lines()
            .find_map(|l| l.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|rest| rest.split_whitespace().next()?.parse().ok())
    };
    let total = field("MemTotal")?;
    let available = field("MemAvailable")?;
    Some(Memory {
        used_kib: total.saturating_sub(available),
        total_kib: total,
    })
}

/// Reads memory use from the running system.
pub fn read() -> Option<Memory> {
    parse(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMINFO: &str = "MemTotal:       16303428 kB\nMemFree:         1200000 kB\n\
        MemAvailable:   12004324 kB\nBuffers:          300000 kB\nSwapTotal:       8388604 kB\n";

    #[test]
    fn used_is_total_minus_available() {
        let m = parse(MEMINFO).unwrap();
        assert_eq!(m.total_kib, 16_303_428);
        assert_eq!(m.used_kib, 4_299_104);
        assert_eq!(m.percent(), 26);
        assert_eq!(m.used_label(), "4.1G");
    }

    #[test]
    fn labels_pick_a_sensible_unit() {
        assert_eq!(format_kib(0), "0M");
        assert_eq!(format_kib(524_288), "512M");
        assert_eq!(format_kib(1_048_575), "1024M");
        assert_eq!(format_kib(1_048_576), "1.0G");
        assert_eq!(format_kib(33_554_432), "32.0G");
    }

    #[test]
    fn similar_field_names_are_not_confused() {
        // "MemTotal" must not match a hypothetical "MemTotalHuge".
        let text = "MemTotalHuge: 1 kB\nMemTotal: 100 kB\nMemAvailable: 40 kB\n";
        assert_eq!(
            parse(text).unwrap(),
            Memory {
                used_kib: 60,
                total_kib: 100
            }
        );
    }

    #[test]
    fn missing_fields_give_none() {
        assert_eq!(parse("MemTotal: 100 kB\n"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn zero_total_does_not_divide_by_zero() {
        assert_eq!(
            Memory {
                used_kib: 0,
                total_kib: 0
            }
            .percent(),
            0
        );
    }
}
