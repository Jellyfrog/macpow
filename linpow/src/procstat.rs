use crate::types::ProcessPower;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// One pid's accumulated state across ticks.
#[derive(Debug, Clone, Default)]
struct PidState {
    name: String,
    last_ticks: u64,
    last_render_ns: u64,
    last_disk_read: u64,
    last_disk_write: u64,
    energy_mj: f64,
    alive: bool,
    /// Counts ticks where the pid was missing; pruned after a few cycles to
    /// preserve the "recently dead" sparkline trail.
    dead_count: u32,
}

#[derive(Debug, Default)]
pub struct ProcStatState {
    by_pid: HashMap<i32, PidState>,
    last_pp0_uj: Option<u64>,
    last_pp1_uj: Option<u64>,
}

impl ProcStatState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sample(&mut self, pp0_uj: u64, pp1_uj: u64) -> (Vec<ProcessPower>, f32, f64) {
        let pp0_delta_uj = self.last_pp0_uj.map(|p| pp0_uj.saturating_sub(p));
        let pp1_delta_uj = self.last_pp1_uj.map(|p| pp1_uj.saturating_sub(p));
        self.last_pp0_uj = Some(pp0_uj);
        self.last_pp1_uj = Some(pp1_uj);

        // Snapshot every pid currently visible.
        let mut cur: HashMap<i32, (String, u64, u64, u64, u64, u64)> = HashMap::new();
        let mut total_tick_delta: u64 = 0;
        let mut total_render_delta: u64 = 0;
        let mut seen: HashSet<i32> = HashSet::new();

        let pids = enumerate_pids();
        for pid in pids {
            let Some((name, ticks)) = read_pid_stat(pid) else {
                continue;
            };
            let render_ns = read_pid_render_ns(pid);
            let (disk_r, disk_w) = read_pid_io(pid).unwrap_or((0, 0));
            let rss_bytes = read_pid_rss_bytes(pid).unwrap_or(0);
            cur.insert(pid, (name, ticks, render_ns, disk_r, disk_w, rss_bytes));
            seen.insert(pid);
        }

        // First, compute the totals across all pids so we can apportion.
        for (pid, &(_, ticks, render_ns, _, _, _)) in &cur {
            if let Some(prev) = self.by_pid.get(pid) {
                total_tick_delta += ticks.saturating_sub(prev.last_ticks);
                total_render_delta += render_ns.saturating_sub(prev.last_render_ns);
            }
        }

        let cpu_uj_per_tick = match (pp0_delta_uj, total_tick_delta) {
            (Some(uj), t) if t > 0 => Some(uj as f64 / t as f64),
            _ => None,
        };
        let gpu_uj_per_ns = match (pp1_delta_uj, total_render_delta) {
            (Some(uj), t) if t > 0 => Some(uj as f64 / t as f64),
            _ => None,
        };

        // Update or insert per-pid state.
        for (pid, (name, ticks, render_ns, disk_r, disk_w, _rss)) in &cur {
            let entry = self.by_pid.entry(*pid).or_default();
            entry.name = name.clone();
            let dt_ticks = ticks.saturating_sub(entry.last_ticks);
            let dr_ns = render_ns.saturating_sub(entry.last_render_ns);

            // If a pid suddenly resets backward, treat as restart (PID reuse).
            let pp0_uj = match cpu_uj_per_tick {
                Some(rate) if entry.last_ticks <= *ticks => rate * dt_ticks as f64,
                _ => 0.0,
            };
            let pp1_uj = match gpu_uj_per_ns {
                Some(rate) if entry.last_render_ns <= *render_ns => rate * dr_ns as f64,
                _ => 0.0,
            };
            entry.energy_mj += (pp0_uj + pp1_uj) / 1000.0;
            entry.last_ticks = *ticks;
            entry.last_render_ns = *render_ns;
            entry.last_disk_read = *disk_r;
            entry.last_disk_write = *disk_w;
            entry.alive = true;
            entry.dead_count = 0;
        }

        // Sweep for newly-dead pids.
        let mut to_drop: Vec<i32> = Vec::new();
        for (pid, st) in self.by_pid.iter_mut() {
            if !seen.contains(pid) {
                st.alive = false;
                st.dead_count = st.dead_count.saturating_add(1);
                // Drop after ~30 ticks; keeps the sparkline visible for a while.
                if st.dead_count > 30 {
                    to_drop.push(*pid);
                }
            }
        }
        for pid in to_drop {
            self.by_pid.remove(&pid);
        }

        // Build the rendered Vec.
        let mut all: Vec<ProcessPower> = self
            .by_pid
            .iter()
            .map(|(pid, st)| {
                // Power this tick (W) = derivative of accumulated energy across the
                // delta interval, but we don't have an easy interval pinned here;
                // attribute proportional CPU+GPU energy this cycle.
                let pp0_uj = match (cpu_uj_per_tick, cur.get(pid)) {
                    (Some(rate), Some(&(_, ticks, _, _, _, _))) => {
                        rate * ticks.saturating_sub(st.last_ticks) as f64
                    }
                    _ => 0.0,
                };
                let pp1_uj = match (gpu_uj_per_ns, cur.get(pid)) {
                    (Some(rate), Some(&(_, _, render_ns, _, _, _))) => {
                        rate * render_ns.saturating_sub(st.last_render_ns) as f64
                    }
                    _ => 0.0,
                };
                let watts = ((pp0_uj + pp1_uj) / 1_000_000.0) as f32;
                let (disk_r, disk_w, phys_mem) =
                    cur.get(pid).map(|t| (t.3, t.4, t.5)).unwrap_or_default();
                ProcessPower {
                    pid: *pid,
                    name: st.name.clone(),
                    power_w: watts,
                    energy_mj: st.energy_mj,
                    alive: st.alive,
                    disk_read_bytes: disk_r,
                    disk_write_bytes: disk_w,
                    phys_mem_bytes: phys_mem,
                    net_rx_bytes: 0,
                    net_tx_bytes: 0,
                }
            })
            .collect();
        all.sort_by(|a, b| {
            b.energy_mj
                .partial_cmp(&a.energy_mj)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let total_w: f32 = all.iter().map(|p| p.power_w).sum();
        let total_energy_mj: f64 = all.iter().map(|p| p.energy_mj).sum();
        all.truncate(50);

        (all, total_w, total_energy_mj)
    }
}

fn enumerate_pids() -> Vec<i32> {
    let Ok(rd) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        if let Some(name) = entry.file_name().to_str() {
            if let Ok(pid) = name.parse::<i32>() {
                out.push(pid);
            }
        }
    }
    out
}

/// Returns (comm, total CPU ticks = utime + stime).
fn read_pid_stat(pid: i32) -> Option<(String, u64)> {
    let text = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let bytes = text.as_bytes();
    // comm is parenthesised but can contain spaces and parens; find LAST ')'.
    let start = bytes.iter().position(|&b| b == b'(')?;
    let end = bytes.iter().rposition(|&b| b == b')')?;
    let comm = String::from_utf8_lossy(&bytes[start + 1..end]).to_string();
    let after = std::str::from_utf8(&bytes[end + 2..]).ok()?;
    let fields: Vec<&str> = after.split_ascii_whitespace().collect();
    // After "(comm) " we have: state ppid pgrp ... utime(13th) stime(14th) ...
    // Indexing into `fields`: state=0, ppid=1, ..., utime=11, stime=12 (0-indexed).
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((comm, utime + stime))
}

/// Sum DRM render-engine busy nanoseconds across all fdinfo entries for `pid`.
fn read_pid_render_ns(pid: i32) -> u64 {
    let dir = format!("/proc/{}/fdinfo", pid);
    let Ok(rd) = fs::read_dir(&dir) else {
        return 0;
    };
    let mut total: u64 = 0;
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        // Only consider fdinfo blocks that include a `drm-driver:` marker.
        if !text.contains("drm-driver:") {
            continue;
        }
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("drm-engine-render:") {
                let ns: u64 = rest
                    .trim()
                    .split_ascii_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                total = total.saturating_add(ns);
            }
        }
    }
    total
}

fn read_pid_io(pid: i32) -> Option<(u64, u64)> {
    let text = fs::read_to_string(format!("/proc/{}/io", pid)).ok()?;
    let mut r = 0u64;
    let mut w = 0u64;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("read_bytes:") {
            r = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("write_bytes:") {
            w = v.trim().parse().unwrap_or(0);
        }
    }
    Some((r, w))
}

fn read_pid_rss_bytes(pid: i32) -> Option<u64> {
    // /proc/<pid>/statm: size resident shared text lib data dt   (all in pages)
    let text = fs::read_to_string(format!("/proc/{}/statm", pid)).ok()?;
    let resident_pages: u64 = text.split_ascii_whitespace().nth(1)?.parse().ok()?;
    // SAFETY: sysconf is always-safe.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = if page_size <= 0 {
        4096
    } else {
        page_size as u64
    };
    Some(resident_pages.saturating_mul(page_size))
}

#[allow(dead_code)]
pub fn proc_path_exists() -> bool {
    Path::new("/proc/self/stat").exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_pid_appears_in_enumeration() {
        let pids = enumerate_pids();
        let me = unsafe { libc::getpid() };
        assert!(pids.contains(&me), "expected own pid {} in list", me);
    }

    #[test]
    fn read_own_stat_returns_comm_and_ticks() {
        let me = unsafe { libc::getpid() };
        let s = read_pid_stat(me);
        assert!(s.is_some());
        let (comm, ticks) = s.unwrap();
        assert!(!comm.is_empty());
        let _ = ticks; // value depends on test run
    }
}
