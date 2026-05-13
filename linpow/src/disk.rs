use crate::sysfs;
use crate::types::DiskInfo;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

const SECTOR_BYTES: u64 = 512;

#[derive(Debug, Clone, Copy, Default)]
pub struct DiskStats {
    pub read_bytes: u64,
    pub write_bytes: u64,
}

fn block_devices() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in sysfs::dir_entries("/sys/block") {
        let name = match p.file_name().and_then(|s| s.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        // Keep NVMe and SATA disks; skip loop/zram/dm/virtual.
        if name.starts_with("loop")
            || name.starts_with("zram")
            || name.starts_with("dm-")
            || name.starts_with("md")
        {
            continue;
        }
        out.push(p);
    }
    out
}

pub fn read_disk_totals() -> DiskStats {
    let mut total = DiskStats::default();
    for dev in block_devices() {
        if let Some(s) = read_one(&dev) {
            total.read_bytes = total.read_bytes.saturating_add(s.read_bytes);
            total.write_bytes = total.write_bytes.saturating_add(s.write_bytes);
        }
    }
    total
}

fn read_one(dev: &std::path::Path) -> Option<DiskStats> {
    // /sys/block/<dev>/stat layout:
    //   reads reads_merged sectors_read time_reading writes writes_merged sectors_written ...
    let text = sysfs::read_string(dev.join("stat")).ok()?;
    let parts: Vec<&str> = text.split_ascii_whitespace().collect();
    if parts.len() < 7 {
        return None;
    }
    let sec_r: u64 = parts.get(2)?.parse().ok()?;
    let sec_w: u64 = parts.get(6)?.parse().ok()?;
    Some(DiskStats {
        read_bytes: sec_r.saturating_mul(SECTOR_BYTES),
        write_bytes: sec_w.saturating_mul(SECTOR_BYTES),
    })
}

#[derive(Debug, Clone, Default)]
pub struct DiskState {
    last: DiskStats,
    last_at: Option<Instant>,
}

impl DiskState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sample(&mut self) -> DiskInfo {
        let cur = read_disk_totals();
        let info = if let Some(t) = self.last_at {
            let dt = t.elapsed().as_secs_f64().max(0.05);
            DiskInfo {
                read_bytes_per_sec: cur.read_bytes.saturating_sub(self.last.read_bytes) as f64 / dt,
                write_bytes_per_sec: cur.write_bytes.saturating_sub(self.last.write_bytes) as f64
                    / dt,
            }
        } else {
            DiskInfo::default()
        };
        self.last = cur;
        self.last_at = Some(Instant::now());
        info
    }
}

pub fn read_ssd_model() -> String {
    // Prefer the first NVMe; fall back to first SATA.
    let preferred = ["nvme0n1", "nvme1n1", "sda", "sdb"];
    let known: HashMap<String, PathBuf> = block_devices()
        .into_iter()
        .filter_map(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| (s.to_string(), p.clone()))
        })
        .collect();
    for name in preferred {
        if let Some(p) = known.get(name) {
            if let Ok(m) = sysfs::read_string(p.join("device/model")) {
                if !m.is_empty() {
                    return m;
                }
            }
        }
    }
    // Generic fallback: any device's model field.
    for (_, p) in known {
        if let Ok(m) = sysfs::read_string(p.join("device/model")) {
            if !m.is_empty() {
                return m;
            }
        }
    }
    String::new()
}

/// Estimate SSD power from I/O rate (matches macpow's existing estimate model).
pub fn estimate_ssd_power_w(d: &DiskInfo) -> f32 {
    const IDLE_W: f32 = 0.03;
    const MAX_W: f32 = 2.5;
    // Saturate at ~2 GB/s — typical PCIe 4.0 NVMe peak.
    let rate = (d.read_bytes_per_sec + d.write_bytes_per_sec) as f32;
    let frac = (rate / 2.0e9).clamp(0.0, 1.0);
    IDLE_W + frac * (MAX_W - IDLE_W)
}
