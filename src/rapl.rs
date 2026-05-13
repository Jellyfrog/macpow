use crate::sysfs;
use std::path::PathBuf;
use std::time::Instant;

const POWERCAP_ROOT: &str = "/sys/class/powercap";

/// One RAPL domain — package, cores (PP0), uncore (PP1/GPU), DRAM, or PSYS.
#[derive(Debug, Clone)]
pub struct Domain {
    pub path: PathBuf,
    pub kind: DomainKind,
    pub max_energy_uj: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainKind {
    Package,
    Cores,
    GpuUncore,
    Dram,
    Psys,
    Other,
}

impl DomainKind {
    fn classify(name: &str) -> Self {
        let n = name.to_ascii_lowercase();
        if n.starts_with("package") {
            Self::Package
        } else if n == "core" || n == "pp0" {
            Self::Cores
        } else if n == "uncore" || n == "pp1" {
            Self::GpuUncore
        } else if n == "dram" {
            Self::Dram
        } else if n == "psys" {
            Self::Psys
        } else {
            Self::Other
        }
    }
}

/// Enumerate visible RAPL domains under /sys/class/powercap/intel-rapl:*.
/// Filters out `intel-rapl-mmio:*` to avoid double-counting.
pub fn enumerate() -> Vec<Domain> {
    let mut out = Vec::new();
    let entries = sysfs::dir_entries(POWERCAP_ROOT);
    for e in entries {
        let name = match e.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        if !name.starts_with("intel-rapl:") {
            continue;
        }
        // Each "intel-rapl:X" is a package; its subdirs "intel-rapl:X:Y" are subdomains.
        push_domain(&e, &mut out);
        for sub in sysfs::dir_entries(&e) {
            let sname = match sub.file_name().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            if sname.starts_with("intel-rapl:") {
                push_domain(&sub, &mut out);
            }
        }
    }
    out
}

fn push_domain(path: &std::path::Path, out: &mut Vec<Domain>) {
    let dn = sysfs::read_string(path.join("name")).unwrap_or_default();
    let max: u64 = sysfs::read_parse(path.join("max_energy_range_uj")).unwrap_or(0);
    out.push(Domain {
        path: path.to_path_buf(),
        kind: DomainKind::classify(&dn),
        max_energy_uj: max,
    });
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Sample {
    pub energy_uj: u64,
    pub at: Option<Instant>,
}

pub fn read_energy(d: &Domain) -> Option<u64> {
    sysfs::read_parse::<u64, _>(d.path.join("energy_uj"))
}

/// Watts between two samples; handles counter wrap using `max_energy_uj`.
pub fn watts_between(prev: Sample, cur_uj: u64, cur_at: Instant, max: u64) -> f32 {
    let Some(prev_at) = prev.at else {
        return 0.0;
    };
    let dt = cur_at.duration_since(prev_at).as_secs_f64();
    if dt <= 0.0 {
        return 0.0;
    }
    let de = if cur_uj >= prev.energy_uj {
        cur_uj - prev.energy_uj
    } else if max > 0 {
        // Counter wrapped.
        (max - prev.energy_uj).saturating_add(cur_uj)
    } else {
        0
    };
    ((de as f64 / 1_000_000.0) / dt) as f32
}

/// Stateful per-domain power tracker.
#[derive(Debug, Default)]
pub struct DomainState {
    pub last: Sample,
}

impl DomainState {
    pub fn tick(&mut self, d: &Domain) -> f32 {
        let Some(cur_uj) = read_energy(d) else {
            return 0.0;
        };
        let now = Instant::now();
        let w = watts_between(self.last, cur_uj, now, d.max_energy_uj);
        self.last = Sample {
            energy_uj: cur_uj,
            at: Some(now),
        };
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn no_wrap() {
        let start = Instant::now();
        let prev = Sample {
            energy_uj: 1_000_000,
            at: Some(start),
        };
        let later = start + Duration::from_secs(1);
        let w = watts_between(prev, 2_000_000, later, 100_000_000);
        assert!((w - 1.0).abs() < 1e-3, "{w}");
    }

    #[test]
    fn wrap_handled() {
        let start = Instant::now();
        let prev = Sample {
            energy_uj: 99_500_000,
            at: Some(start),
        };
        let later = start + Duration::from_secs(1);
        // cur < prev → wrapped; delta = (max - prev) + cur = 500_000 + 500_000 = 1_000_000 µJ
        let w = watts_between(prev, 500_000, later, 100_000_000);
        assert!((w - 1.0).abs() < 1e-3, "{w}");
    }

    #[test]
    fn classify_names() {
        assert_eq!(DomainKind::classify("package-0"), DomainKind::Package);
        assert_eq!(DomainKind::classify("core"), DomainKind::Cores);
        assert_eq!(DomainKind::classify("uncore"), DomainKind::GpuUncore);
        assert_eq!(DomainKind::classify("dram"), DomainKind::Dram);
        assert_eq!(DomainKind::classify("psys"), DomainKind::Psys);
        assert_eq!(DomainKind::classify("foobar"), DomainKind::Other);
    }
}
