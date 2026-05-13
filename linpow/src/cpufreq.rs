use crate::sysfs;
use std::collections::HashMap;

/// CPU topology — discovered once at startup.
#[derive(Debug, Clone, Default)]
pub struct CpuTopology {
    /// All logical CPUs in kernel order.
    pub all: Vec<u32>,
    /// P-core logical CPU ids (empty on non-hybrid CPUs).
    pub pcpus: Vec<u32>,
    /// E-core logical CPU ids (empty on non-hybrid CPUs).
    pub ecpus: Vec<u32>,
}

impl CpuTopology {
    pub fn detect() -> Self {
        let online = read_cpu_list("/sys/devices/system/cpu/online").unwrap_or_default();
        let pcpus = read_cpu_list("/sys/devices/cpu_core/cpus").unwrap_or_default();
        let ecpus = read_cpu_list("/sys/devices/cpu_atom/cpus").unwrap_or_default();
        Self {
            all: online,
            pcpus,
            ecpus,
        }
    }

    pub fn is_hybrid(&self) -> bool {
        !self.pcpus.is_empty() && !self.ecpus.is_empty()
    }
}

/// Parses a Linux CPU list spec like "0-3,8,10-11" into a Vec.
pub fn parse_cpu_list(s: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((lo, hi)) = part.split_once('-') {
            if let (Ok(lo), Ok(hi)) = (lo.parse::<u32>(), hi.parse::<u32>()) {
                for v in lo..=hi {
                    out.push(v);
                }
            }
        } else if let Ok(v) = part.parse::<u32>() {
            out.push(v);
        }
    }
    out
}

fn read_cpu_list(path: &str) -> Option<Vec<u32>> {
    sysfs::read_string(path).ok().map(|s| parse_cpu_list(&s))
}

/// Per-CPU tick counters from /proc/stat.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuTicks {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTicks {
    pub fn total(&self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }

    pub fn idle_all(&self) -> u64 {
        self.idle.saturating_add(self.iowait)
    }
}

/// Reads /proc/stat and returns (overall, per-cpu) tick counters keyed by cpu id.
pub fn read_proc_stat() -> Option<(CpuTicks, HashMap<u32, CpuTicks>)> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    let mut overall = CpuTicks::default();
    let mut per = HashMap::new();
    for line in text.lines() {
        if !line.starts_with("cpu") {
            continue;
        }
        let mut it = line.split_ascii_whitespace();
        let head = it.next()?;
        let parse = |it: &mut std::str::SplitAsciiWhitespace| -> CpuTicks {
            let mut v = [0u64; 8];
            for slot in v.iter_mut() {
                if let Some(s) = it.next() {
                    *slot = s.parse().unwrap_or(0);
                } else {
                    break;
                }
            }
            CpuTicks {
                user: v[0],
                nice: v[1],
                system: v[2],
                idle: v[3],
                iowait: v[4],
                irq: v[5],
                softirq: v[6],
                steal: v[7],
            }
        };
        if head == "cpu" {
            overall = parse(&mut it);
        } else if let Some(idx) = head.strip_prefix("cpu") {
            if let Ok(idx) = idx.parse::<u32>() {
                per.insert(idx, parse(&mut it));
            }
        }
    }
    Some((overall, per))
}

/// Utilization % between two tick samples (0–100).
pub fn util_pct(prev: CpuTicks, cur: CpuTicks) -> f32 {
    let dt = cur.total().saturating_sub(prev.total());
    let di = cur.idle_all().saturating_sub(prev.idle_all());
    if dt == 0 {
        0.0
    } else {
        (100.0 * (1.0 - di as f64 / dt as f64)).clamp(0.0, 100.0) as f32
    }
}

/// Current frequency for one CPU, in MHz. Reads scaling_cur_freq (kHz).
pub fn read_cpu_freq_mhz(cpu: u32) -> Option<u32> {
    let path = format!(
        "/sys/devices/system/cpu/cpu{}/cpufreq/scaling_cur_freq",
        cpu
    );
    sysfs::read_parse::<u64, _>(path).map(|khz| (khz / 1000) as u32)
}
