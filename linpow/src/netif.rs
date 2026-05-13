use crate::sysfs;
use crate::types::{EthernetInfo, NetworkInfo, WifiInfo};
use std::collections::HashMap;
use std::time::Instant;

/// Per-interface RX/TX byte counters from /proc/net/dev.
#[derive(Debug, Clone, Copy, Default)]
pub struct IfCounters {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

pub fn read_proc_net_dev() -> HashMap<String, IfCounters> {
    let mut out = HashMap::new();
    let Ok(text) = std::fs::read_to_string("/proc/net/dev") else {
        return out;
    };
    for line in text.lines().skip(2) {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name == "lo" {
            continue;
        }
        let parts: Vec<&str> = rest.split_ascii_whitespace().collect();
        // Layout: rx_bytes rx_packets rx_errs rx_drop rx_fifo rx_frame rx_compressed rx_multicast
        //         tx_bytes tx_packets ...
        if parts.len() < 9 {
            continue;
        }
        let rx_bytes: u64 = parts[0].parse().unwrap_or(0);
        let tx_bytes: u64 = parts[8].parse().unwrap_or(0);
        out.insert(name.to_string(), IfCounters { rx_bytes, tx_bytes });
    }
    out
}

fn read_iface_type(name: &str) -> Option<String> {
    sysfs::read_string(format!("/sys/class/net/{}/uevent", name))
        .ok()
        .and_then(|s| {
            for l in s.lines() {
                if let Some(v) = l.strip_prefix("DEVTYPE=") {
                    return Some(v.to_string());
                }
            }
            None
        })
}

fn iface_operstate(name: &str) -> String {
    sysfs::read_string(format!("/sys/class/net/{}/operstate", name)).unwrap_or_default()
}

fn iface_speed_mbps(name: &str) -> u32 {
    sysfs::read_parse::<i64, _>(format!("/sys/class/net/{}/speed", name))
        .map(|v| if v > 0 { v as u32 } else { 0 })
        .unwrap_or(0)
}

fn iface_is_wireless(name: &str) -> bool {
    std::path::Path::new(&format!("/sys/class/net/{}/wireless", name)).exists()
}

/// Classified interfaces; the first connected of each kind wins.
pub struct Interfaces {
    pub ethernet: Option<String>,
    pub wifi: Option<String>,
}

pub fn classify() -> Interfaces {
    let mut eth = None;
    let mut wifi = None;
    let Ok(rd) = std::fs::read_dir("/sys/class/net") else {
        return Interfaces {
            ethernet: None,
            wifi: None,
        };
    };
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "lo" {
            continue;
        }
        let up = iface_operstate(&name) == "up";
        if iface_is_wireless(&name) {
            if wifi.is_none() || up {
                wifi = Some(name);
            }
        } else if read_iface_type(&name).is_none() {
            // Plain "ether" devices have no DEVTYPE; tunnels, bridges, etc. do.
            if eth.is_none() || up {
                eth = Some(name);
            }
        }
    }
    Interfaces {
        ethernet: eth,
        wifi,
    }
}

#[derive(Debug, Clone, Default)]
pub struct NetState {
    last: HashMap<String, IfCounters>,
    last_at: Option<Instant>,
}

impl NetState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns rate (bytes/sec) for the named interface, or zero if unknown.
    pub fn sample_iface_rate(&self, cur: &HashMap<String, IfCounters>, name: &str) -> NetworkInfo {
        let prev = self.last.get(name).copied().unwrap_or_default();
        let now = self
            .last_at
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(1.0)
            .max(0.05);
        let cur = cur.get(name).copied().unwrap_or_default();
        NetworkInfo {
            bytes_in_per_sec: cur.rx_bytes.saturating_sub(prev.rx_bytes) as f64 / now,
            bytes_out_per_sec: cur.tx_bytes.saturating_sub(prev.tx_bytes) as f64 / now,
        }
    }

    /// Returns total rate across all known interfaces.
    pub fn sample_total_rate(&self, cur: &HashMap<String, IfCounters>) -> NetworkInfo {
        let now = self
            .last_at
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(1.0)
            .max(0.05);
        let mut rx = 0u64;
        let mut tx = 0u64;
        for (k, v) in cur {
            let p = self.last.get(k).copied().unwrap_or_default();
            rx = rx.saturating_add(v.rx_bytes.saturating_sub(p.rx_bytes));
            tx = tx.saturating_add(v.tx_bytes.saturating_sub(p.tx_bytes));
        }
        NetworkInfo {
            bytes_in_per_sec: rx as f64 / now,
            bytes_out_per_sec: tx as f64 / now,
        }
    }

    pub fn commit(&mut self, cur: HashMap<String, IfCounters>) {
        self.last = cur;
        self.last_at = Some(Instant::now());
    }
}

pub fn read_ethernet(name: Option<&String>) -> EthernetInfo {
    let Some(n) = name else {
        return EthernetInfo::default();
    };
    EthernetInfo {
        connected: iface_operstate(n) == "up",
        interface_name: n.clone(),
        link_speed_mbps: iface_speed_mbps(n),
    }
}

pub fn read_wifi(name: Option<&String>) -> WifiInfo {
    let Some(n) = name else {
        return WifiInfo::default();
    };
    WifiInfo {
        connected: iface_operstate(n) == "up",
        interface_name: n.clone(),
        // ssid / rssi / channel / tx_rate are populated by the nl80211 sampler
        // in a later phase. Here we only emit basic link state so the row renders.
        ..Default::default()
    }
}
