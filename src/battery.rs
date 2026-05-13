use crate::sysfs;
use crate::types::{AdapterInfo, BatteryInfo};
use std::path::PathBuf;

const PSU_ROOT: &str = "/sys/class/power_supply";

fn psu_paths_by_type(want: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for p in sysfs::dir_entries(PSU_ROOT) {
        if let Ok(t) = sysfs::read_string(p.join("type")) {
            if t == want {
                out.push(p);
            }
        }
    }
    out
}

pub fn read_battery() -> BatteryInfo {
    let bats = psu_paths_by_type("Battery");
    let Some(bat) = bats.first() else {
        return BatteryInfo::default();
    };

    let status = sysfs::read_string(bat.join("status")).unwrap_or_default();
    let charging = matches!(status.as_str(), "Charging" | "Full");

    // Energy/charge interface — kernels expose one of two units.
    // energy_* is µWh (preferred — direct energy).
    // charge_* is µAh (need voltage to derive energy).
    let voltage_uv: i64 = sysfs::read_parse(bat.join("voltage_now")).unwrap_or(0);
    let voltage_mv = voltage_uv as f64 / 1000.0;

    // Power draw — power_now is in µW; sign convention varies, take absolute.
    let power_uw: i64 = sysfs::read_parse(bat.join("power_now")).unwrap_or(0);
    let current_ua: i64 = sysfs::read_parse(bat.join("current_now")).unwrap_or(0);
    let drain_w = (power_uw.unsigned_abs() as f64) / 1_000_000.0;
    let amperage_ma = (current_ua as f64) / 1000.0;

    // Capacity in µWh or µAh.
    let (current_uwh, max_uwh, design_uwh) = if let (Some(c), Some(m)) = (
        sysfs::read_parse::<i64, _>(bat.join("energy_now")),
        sysfs::read_parse::<i64, _>(bat.join("energy_full")),
    ) {
        let d: i64 = sysfs::read_parse(bat.join("energy_full_design")).unwrap_or(m);
        (c, m, d)
    } else if let (Some(c_ua), Some(m_ua)) = (
        sysfs::read_parse::<i64, _>(bat.join("charge_now")),
        sysfs::read_parse::<i64, _>(bat.join("charge_full")),
    ) {
        let d_ua: i64 = sysfs::read_parse(bat.join("charge_full_design")).unwrap_or(m_ua);
        let v = voltage_uv.max(3_700_000); // sane fallback
        let scale = v as i128 / 1_000_000;
        (
            (c_ua as i128 * scale) as i64,
            (m_ua as i128 * scale) as i64,
            (d_ua as i128 * scale) as i64,
        )
    } else {
        (0, 0, 0)
    };

    let percent_field: f64 = sysfs::read_parse::<u32, _>(bat.join("capacity"))
        .map(|v| v as f64)
        .unwrap_or_else(|| {
            if max_uwh > 0 {
                100.0 * current_uwh as f64 / max_uwh as f64
            } else {
                0.0
            }
        });

    let cycle_count: i64 = sysfs::read_parse(bat.join("cycle_count")).unwrap_or(0);
    let temperature_c = sysfs::read_parse::<i64, _>(bat.join("temp"))
        .map(|t| t as f64 / 10.0)
        .unwrap_or(0.0);

    // Time remaining: drain_w is W now. capacity headroom / drain.
    let headroom_uwh = if charging {
        max_uwh.saturating_sub(current_uwh)
    } else {
        current_uwh
    };
    let time_remaining_min = if drain_w > 0.5 {
        ((headroom_uwh as f64 / 1_000_000.0) / drain_w * 60.0) as i64
    } else {
        0
    };

    let max_capacity_mah = (max_uwh as f64 / voltage_uv.max(1) as f64) * 1000.0;
    let design_capacity_mah = (design_uwh as f64 / voltage_uv.max(1) as f64) * 1000.0;
    let health_pct = if design_uwh > 0 {
        100.0 * max_uwh as f64 / design_uwh as f64
    } else {
        0.0
    };

    BatteryInfo {
        present: true,
        charging,
        voltage_mv,
        amperage_ma,
        drain_w,
        capacity_wh: max_uwh as f64 / 1_000_000.0,
        current_capacity: current_uwh / 1000, // mWh
        max_capacity: max_uwh / 1000,         // mWh
        percent: percent_field,
        time_remaining_min,
        external_connected: false, // set by adapter sampler
        temperature_c,
        cycle_count,
        design_capacity_mah,
        max_capacity_mah,
        health_pct,
    }
}

pub fn read_adapter() -> AdapterInfo {
    let macs = psu_paths_by_type("Mains");
    for p in macs {
        let online: i64 = sysfs::read_parse(p.join("online")).unwrap_or(0);
        if online == 0 {
            continue;
        }
        // input_power_now is in µW on Lenovo EC kernels.
        let power_uw: i64 = sysfs::read_parse(p.join("input_power_now"))
            .or_else(|| sysfs::read_parse(p.join("power_now")))
            .unwrap_or(0);
        let voltage_uv: i64 = sysfs::read_parse(p.join("voltage_now")).unwrap_or(0);
        let current_ua: i64 = sysfs::read_parse(p.join("current_now")).unwrap_or(0);
        let watts = if power_uw > 0 {
            (power_uw as f64 / 1_000_000.0).round() as u32
        } else if voltage_uv > 0 && current_ua > 0 {
            ((voltage_uv as f64 / 1_000_000.0) * (current_ua as f64 / 1_000_000.0)).round() as u32
        } else {
            0
        };
        return AdapterInfo {
            connected: true,
            watts,
            voltage_mv: (voltage_uv / 1000) as u32,
            current_ma: (current_ua / 1000) as u32,
            is_wireless: false,
        };
    }
    AdapterInfo::default()
}
