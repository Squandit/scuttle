//! Which kind of network connection carries the default route.

use std::fs;
use std::path::Path;

/// How the machine reaches the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Offline,
    /// A wired interface, by name.
    Wired(String),
    /// A wireless interface, by name.
    Wireless(String),
}

impl Link {
    /// A freedesktop icon name for the link type.
    pub fn icon(&self) -> &'static str {
        match self {
            Link::Offline => "network-offline-symbolic",
            Link::Wired(_) => "network-wired-symbolic",
            Link::Wireless(_) => "network-wireless-symbolic",
        }
    }
}

/// Finds the interface that carries the IPv4 default route in the text
/// of `/proc/net/route`.
pub fn default_interface(route_table: &str) -> Option<String> {
    route_table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let iface = fields.next()?;
            let destination = fields.next()?;
            let _gateway = fields.next()?;
            let flags = u32::from_str_radix(fields.next()?, 16).ok()?;
            let metric: u32 = fields.nth(2)?.parse().ok()?;
            // Destination 0 with the "up" flag set is a default route.
            const RTF_UP: u32 = 0x1;
            (destination == "00000000" && flags & RTF_UP != 0).then(|| (metric, iface.to_string()))
        })
        // The lowest metric is the route the kernel prefers.
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, iface)| iface)
}

/// Works out the link type from the route table text and the kernel's
/// network class directory (normally `/sys/class/net`).
pub fn read(route_table: &str, net_dir: &Path) -> Link {
    match default_interface(route_table) {
        None => Link::Offline,
        Some(iface) => {
            let dir = net_dir.join(&iface);
            if dir.join("wireless").exists() || dir.join("phy80211").exists() {
                Link::Wireless(iface)
            } else {
                Link::Wired(iface)
            }
        }
    }
}

/// Reads the link type of the running system.
pub fn read_system() -> Link {
    let table = fs::read_to_string("/proc/net/route").unwrap_or_default();
    read(&table, Path::new("/sys/class/net"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n";

    fn table(rows: &[&str]) -> String {
        format!("{HEADER}{}\n", rows.join("\n"))
    }

    #[test]
    fn finds_the_default_route() {
        let t = table(&[
            "wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0",
            "wlan0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0",
        ]);
        assert_eq!(default_interface(&t), Some("wlan0".into()));
    }

    #[test]
    fn prefers_the_lowest_metric() {
        let t = table(&[
            "wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0",
            "enp5s0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0",
        ]);
        assert_eq!(default_interface(&t), Some("enp5s0".into()));
    }

    #[test]
    fn no_default_route_means_offline() {
        let t = table(&["wlan0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0"]);
        assert_eq!(default_interface(&t), None);
        assert_eq!(default_interface(""), None);
        assert_eq!(read(&t, Path::new("/nonexistent")), Link::Offline);
    }

    #[test]
    fn downed_routes_are_ignored() {
        let t = table(&["wlan0\t00000000\t0101A8C0\t0002\t0\t0\t600\t00000000\t0\t0\t0"]);
        assert_eq!(default_interface(&t), None);
    }

    #[test]
    fn tells_wireless_from_wired() {
        let dir = std::env::temp_dir().join(format!("scuttle-net-{}", std::process::id()));
        fs::create_dir_all(dir.join("wlan0/wireless")).unwrap();
        fs::create_dir_all(dir.join("enp5s0")).unwrap();
        let wifi = table(&["wlan0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0"]);
        let wired = table(&["enp5s0\t00000000\t0101A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0"]);
        assert_eq!(read(&wifi, &dir), Link::Wireless("wlan0".into()));
        assert_eq!(read(&wired, &dir), Link::Wired("enp5s0".into()));
        assert_eq!(Link::Offline.icon(), "network-offline-symbolic");
        fs::remove_dir_all(&dir).unwrap();
    }
}
