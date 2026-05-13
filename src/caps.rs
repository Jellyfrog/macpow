use crate::rapl;
use crate::sysfs;
use std::path::Path;

/// One-line capability summary printed to stderr at startup so users know
/// which data sources are blocked by permissions in their environment.
pub fn summary() -> String {
    let rapl_state = probe_rapl();
    let pmu_state = probe_perf_paranoid();
    let nvme_smart = if Path::new("/dev/nvme0").exists() {
        if can_open("/dev/nvme0") {
            "ok"
        } else {
            "denied"
        }
    } else {
        "absent"
    };
    let procs = if running_as_root() {
        "all-procs=ok"
    } else {
        "all-procs=denied(non-root)"
    };
    format!(
        "linpow: rapl={} i915-pmu={} nvme-smart={} {}",
        rapl_state, pmu_state, nvme_smart, procs
    )
}

fn probe_rapl() -> String {
    let domains = rapl::enumerate();
    if domains.is_empty() {
        return "absent".to_string();
    }
    for d in &domains {
        if rapl::read_energy(d).is_none() {
            return "denied".to_string();
        }
    }
    "ok".to_string()
}

fn probe_perf_paranoid() -> String {
    match sysfs::read_parse::<i32, _>("/proc/sys/kernel/perf_event_paranoid") {
        Some(v) if v <= 1 => "ok".to_string(),
        Some(v) => format!("denied(perf_event_paranoid={})", v),
        None => "absent".to_string(),
    }
}

fn running_as_root() -> bool {
    // SAFETY: getuid never fails.
    unsafe { libc::getuid() == 0 }
}

fn can_open(path: &str) -> bool {
    std::fs::OpenOptions::new().read(true).open(path).is_ok()
}
