use crate::sysfs;
use std::path::PathBuf;

const DRM_ROOT: &str = "/sys/class/drm";

#[derive(Debug, Clone, Default)]
pub struct GpuInfo {
    pub freq_mhz: u32,
    pub max_freq_mhz: u32,
    pub util_device: u32,
    pub util_renderer: u32,
    pub util_tiler: u32,
}

/// Pick the primary iGPU card: prefer Intel (i915 / xe drivers).
fn pick_card() -> Option<PathBuf> {
    let mut entries = sysfs::dir_entries(DRM_ROOT);
    entries.sort();
    // First pass: a card with gt_cur_freq_mhz that's i915- or xe-backed.
    for p in &entries {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        if p.join("gt_cur_freq_mhz").exists() || p.join("gt/gt0/cur_freq").exists() {
            return Some(p.clone());
        }
    }
    None
}

pub fn read() -> GpuInfo {
    let Some(card) = pick_card() else {
        return GpuInfo::default();
    };

    // i915 exposes flat files; xe exposes nested gt/gtN/cur_freq.
    let freq_mhz: u32 = sysfs::read_parse(card.join("gt_cur_freq_mhz"))
        .or_else(|| sysfs::read_parse(card.join("gt/gt0/cur_freq")))
        .unwrap_or(0);
    let max_freq_mhz: u32 = sysfs::read_parse(card.join("gt_max_freq_mhz"))
        .or_else(|| sysfs::read_parse(card.join("gt_RP0_freq_mhz")))
        .or_else(|| sysfs::read_parse(card.join("gt/gt0/max_freq")))
        .unwrap_or(0);

    GpuInfo {
        freq_mhz,
        max_freq_mhz,
        // Per-engine busy via perf_event_open(i915/xe PMU) lands in a follow-up
        // phase. Leaving these at zero is the documented graceful-degrade path
        // when the PMU isn't accessible.
        util_device: 0,
        util_renderer: 0,
        util_tiler: 0,
    }
}
