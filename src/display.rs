use crate::sysfs;
use crate::types::{DisplayInfo, KeyboardInfo, PanelClass};

const BACKLIGHT_ROOT: &str = "/sys/class/backlight";
const KBD_LED_CANDIDATES: &[&str] = &[
    "/sys/class/leds/tpacpi::kbd_backlight",
    "/sys/class/leds/platform::kbd_backlight",
];

pub const MAX_DISPLAY_W: f32 = 5.0;
pub const MAX_KEYBOARD_W: f32 = 0.5;

fn pick_backlight() -> Option<std::path::PathBuf> {
    let mut entries = sysfs::dir_entries(BACKLIGHT_ROOT);
    entries.sort();
    // Prefer intel_backlight; fall back to whichever exists.
    for e in &entries {
        if e.file_name().and_then(|s| s.to_str()) == Some("intel_backlight") {
            return Some(e.clone());
        }
    }
    entries.into_iter().next()
}

pub fn read_display() -> DisplayInfo {
    let Some(bl) = pick_backlight() else {
        return DisplayInfo::default();
    };
    let cur: i64 = sysfs::read_parse(bl.join("brightness")).unwrap_or(0);
    let max: i64 = sysfs::read_parse(bl.join("max_brightness")).unwrap_or(1);
    let pct = if max > 0 {
        100.0 * cur as f32 / max as f32
    } else {
        0.0
    };
    let estimated_power_w = (pct / 100.0).clamp(0.0, 1.0) * MAX_DISPLAY_W;

    // EDID — diagonal_inches (cm in bytes 21/22), refresh from drm modes.
    let (width_px, height_px, diagonal_inches) = read_edid_geometry().unwrap_or((0, 0, 0.0));
    let refresh_hz = read_primary_refresh_hz();

    DisplayInfo {
        brightness_pct: pct,
        nits: 0.0,
        max_nits: 0.0,
        estimated_power_w,
        available: true,
        width_px,
        height_px,
        diagonal_inches,
        panel_class: PanelClass::Sdr,
        refresh_hz,
        supports_promotion: false,
        hdr_active: false,
        dpb_factor: 1.0,
        preset_name: String::new(),
        preset_max_sdr_nits: 0.0,
        preset_max_hdr_nits: 0.0,
        preset_max_edr_headroom: 1.0,
        peak_nits: 0.0,
    }
}

pub fn read_keyboard() -> KeyboardInfo {
    for cand in KBD_LED_CANDIDATES {
        let path = std::path::PathBuf::from(cand);
        if !path.exists() {
            continue;
        }
        let cur: i64 = sysfs::read_parse(path.join("brightness")).unwrap_or(0);
        let max: i64 = sysfs::read_parse(path.join("max_brightness")).unwrap_or(1);
        let pct = if max > 0 {
            100.0 * cur as f32 / max as f32
        } else {
            0.0
        };
        return KeyboardInfo {
            brightness_pct: pct,
            estimated_power_w: (pct / 100.0).clamp(0.0, 1.0) * MAX_KEYBOARD_W,
        };
    }
    KeyboardInfo::default()
}

fn read_edid_geometry() -> Option<(u32, u32, f32)> {
    // Pick the first connected card output with an EDID.
    let cards = sysfs::dir_entries("/sys/class/drm");
    for c in cards {
        let name = c.file_name()?.to_string_lossy().into_owned();
        if !name.contains('-') {
            continue;
        }
        let status = sysfs::read_string(c.join("status")).unwrap_or_default();
        if status != "connected" {
            continue;
        }
        let edid = sysfs::read_bytes(c.join("edid")).ok()?;
        if edid.len() < 68 {
            continue;
        }
        // EDID detailed timing block at offset 54 (first detailed timing).
        let dt = &edid[54..72];
        let h_active = ((dt[4] as u32) & 0xF0) << 4 | dt[2] as u32;
        let v_active = ((dt[7] as u32) & 0xF0) << 4 | dt[5] as u32;
        // Physical size at offsets 12/13/14 — width_mm, height_mm with high nibble at 14.
        let w_mm = ((dt[14] as u32) & 0xF0) << 4 | dt[12] as u32;
        let h_mm = ((dt[14] as u32) & 0x0F) << 8 | dt[13] as u32;
        let diag_mm = ((w_mm * w_mm + h_mm * h_mm) as f32).sqrt();
        let diag_in = diag_mm / 25.4;
        return Some((h_active, v_active, diag_in));
    }
    None
}

fn read_primary_refresh_hz() -> f32 {
    // /sys/class/drm/cardN-XX/modes lists modes; first line is current. We don't
    // get refresh directly here without DRM ioctls. Default 60 if a connected
    // panel exists; full nl/drm wiring is a later phase.
    let cards = sysfs::dir_entries("/sys/class/drm");
    for c in cards {
        if let Ok(status) = sysfs::read_string(c.join("status")) {
            if status == "connected" {
                return 60.0;
            }
        }
    }
    0.0
}
