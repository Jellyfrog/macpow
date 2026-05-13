use std::collections::HashMap;

/// Returns (mem_used_gb, mem_total_gb).
pub fn read_mem_used_total_gb() -> Option<(f32, u32)> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut map: HashMap<&str, u64> = HashMap::new();
    for line in text.lines() {
        if let Some((key, rest)) = line.split_once(':') {
            let kb = rest
                .trim()
                .split_ascii_whitespace()
                .next()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            map.insert(key, kb);
        }
    }
    let total = *map.get("MemTotal")?;
    // MemAvailable mirrors the kernel's "usable" estimate (free + reclaimable
    // pagecache/slab). Falling back to MemFree is wrong on a healthy system —
    // most of free RAM appears as cached. Prefer MemAvailable.
    let available = map
        .get("MemAvailable")
        .copied()
        .or_else(|| map.get("MemFree").copied())
        .unwrap_or(0);
    let used_kb = total.saturating_sub(available);
    let used_gb = used_kb as f32 / 1024.0 / 1024.0;
    let total_gb = total.div_ceil(1024 * 1024) as u32;
    Some((used_gb, total_gb))
}
