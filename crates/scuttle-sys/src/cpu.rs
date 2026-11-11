//! CPU load from `/proc/stat`.
//!
//! The kernel reports cumulative time counters, so load is the change
//! between two samples.

/// Cumulative CPU time counters from the first line of `/proc/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuSample {
    /// Time spent doing work.
    pub busy: u64,
    /// All time, busy and idle.
    pub total: u64,
}

/// Parses the aggregate `cpu` line of `/proc/stat`.
pub fn parse(stat: &str) -> Option<CpuSample> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|f| f.parse().ok())
        .collect();
    if fields.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal [guest guest_nice]
    // Guest time is already included in user time, so stop at steal.
    let total: u64 = fields.iter().take(8).sum();
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    Some(CpuSample {
        busy: total.saturating_sub(idle),
        total,
    })
}

/// Percentage of CPU time spent busy between two samples, 0 to 100.
pub fn usage(previous: CpuSample, current: CpuSample) -> u8 {
    let total = current.total.saturating_sub(previous.total);
    if total == 0 {
        return 0;
    }
    let busy = current.busy.saturating_sub(previous.busy);
    ((busy * 100 + total / 2) / total).min(100) as u8
}

/// Reads a sample from the running system.
pub fn sample() -> Option<CpuSample> {
    parse(&std::fs::read_to_string("/proc/stat").ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAT: &str =
        "cpu  1000 50 300 8000 200 0 50 0 0 0\ncpu0 500 25 150 4000 100 0 25 0 0 0\nintr 12345\n";

    #[test]
    fn parses_the_aggregate_line() {
        let s = parse(STAT).unwrap();
        assert_eq!(s.total, 9600);
        assert_eq!(s.busy, 1400);
    }

    #[test]
    fn usage_is_the_busy_share_of_the_interval() {
        let a = CpuSample {
            busy: 1000,
            total: 10_000,
        };
        let b = CpuSample {
            busy: 1250,
            total: 11_000,
        };
        assert_eq!(usage(a, b), 25);
    }

    #[test]
    fn no_elapsed_time_is_zero_not_a_division_error() {
        let a = CpuSample { busy: 5, total: 10 };
        assert_eq!(usage(a, a), 0);
    }

    #[test]
    fn counters_going_backwards_do_not_panic() {
        let a = CpuSample {
            busy: 500,
            total: 1000,
        };
        let b = CpuSample {
            busy: 100,
            total: 200,
        };
        assert_eq!(usage(a, b), 0);
    }

    #[test]
    fn rejects_text_without_a_cpu_line() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("cpu0 1 2 3 4\n"), None);
        assert_eq!(parse("cpu  1 2\n"), None);
    }
}
