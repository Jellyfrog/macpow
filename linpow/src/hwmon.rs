use crate::cpufreq::CpuTopology;
use crate::sysfs;
use crate::types::{FanInfo, TempSensor};
use std::path::PathBuf;

const HWMON_ROOT: &str = "/sys/class/hwmon";
const MAX_FAN_W: f32 = 1.0;

/// One discovered hwmon device, with its driver `name`.
#[derive(Debug, Clone)]
pub struct HwmonDevice {
    pub path: PathBuf,
    pub name: String,
}

pub fn enumerate() -> Vec<HwmonDevice> {
    let mut out = Vec::new();
    for p in sysfs::dir_entries(HWMON_ROOT) {
        let name = sysfs::read_string(p.join("name")).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        out.push(HwmonDevice { path: p, name });
    }
    out
}

fn read_temp_input(dev_path: &std::path::Path, idx: usize) -> Option<(String, f32)> {
    let raw_path = dev_path.join(format!("temp{}_input", idx));
    if !raw_path.exists() {
        return None;
    }
    let raw: i64 = sysfs::read_parse(&raw_path)?;
    let value = raw as f32 / 1000.0;
    let label = sysfs::read_string(dev_path.join(format!("temp{}_label", idx))).unwrap_or_default();
    Some((label, value))
}

fn iter_temps(dev_path: &std::path::Path) -> Vec<(usize, String, f32)> {
    let mut out = Vec::new();
    for i in 1..=64 {
        if let Some((label, value)) = read_temp_input(dev_path, i) {
            out.push((i, label, value));
        }
    }
    out
}

/// Maps a coretemp "Core N" label to its hybrid kind (P/E) and position within
/// the cluster, returning the SMC-style key that the app's CPU-temp parser expects.
fn coretemp_key(label: &str, topo: &CpuTopology) -> Option<(String, &'static str)> {
    let cpu = label
        .strip_prefix("Core ")
        .and_then(|s| s.parse::<u32>().ok())?;
    let (kind, pos) = if topo.is_hybrid() {
        if let Some(i) = topo.pcpus.iter().position(|&c| c == cpu) {
            ('p', i)
        } else if let Some(i) = topo.ecpus.iter().position(|&c| c == cpu) {
            ('e', i)
        } else {
            return None;
        }
    } else if let Some(i) = topo.all.iter().position(|&c| c == cpu) {
        ('p', i)
    } else {
        return None;
    };
    let suffix = base62_char(pos)?;
    let key = format!("T{}{}{}", kind, '0', suffix);
    Some((key, "CPU"))
}

/// Maps base62 position (0..62) to a single character: '0'-'9', 'a'-'z', 'A'-'Z'.
fn base62_char(pos: usize) -> Option<char> {
    let c = if pos < 10 {
        (b'0' + pos as u8) as char
    } else if pos < 36 {
        (b'a' + (pos - 10) as u8) as char
    } else if pos < 62 {
        (b'A' + (pos - 36) as u8) as char
    } else {
        return None;
    };
    Some(c)
}

/// Read all temperature sensors and classify them into TempSensor records.
pub fn read_temperatures(topo: &CpuTopology) -> Vec<TempSensor> {
    let mut out = Vec::new();
    let devices = enumerate();
    for d in &devices {
        match d.name.as_str() {
            "coretemp" | "k10temp" | "zenpower" => {
                for (_i, label, value) in iter_temps(&d.path) {
                    if let Some((key, cat)) = coretemp_key(&label, topo) {
                        out.push(TempSensor {
                            key,
                            category: cat.to_string(),
                            value_celsius: value,
                            stale: false,
                        });
                    } else if label == "Package id 0" || label.contains("Tdie") {
                        out.push(TempSensor {
                            key: "Tp00".to_string(),
                            category: "CPU".to_string(),
                            value_celsius: value,
                            stale: false,
                        });
                    }
                }
            }
            "nvme" => {
                for (i, label, value) in iter_temps(&d.path) {
                    let key = if label.is_empty() {
                        format!("nvme{}", i)
                    } else {
                        label
                    };
                    out.push(TempSensor {
                        key,
                        category: "SSD".to_string(),
                        value_celsius: value,
                        stale: false,
                    });
                }
            }
            n if n.starts_with("iwlwifi") => {
                for (i, _label, value) in iter_temps(&d.path) {
                    out.push(TempSensor {
                        key: format!("WiFi{}", i),
                        category: "WiFi".to_string(),
                        value_celsius: value,
                        stale: false,
                    });
                }
            }
            "thinkpad" => {
                for (i, label, value) in iter_temps(&d.path) {
                    let key = if label.is_empty() {
                        format!("ThinkPad{}", i)
                    } else {
                        label
                    };
                    out.push(TempSensor {
                        key,
                        category: "Other".to_string(),
                        value_celsius: value,
                        stale: false,
                    });
                }
            }
            "acpitz" => {
                for (i, _label, value) in iter_temps(&d.path) {
                    out.push(TempSensor {
                        key: format!("acpitz{}", i),
                        category: "Chassis".to_string(),
                        value_celsius: value,
                        stale: false,
                    });
                }
            }
            _ => {
                for (i, _label, value) in iter_temps(&d.path) {
                    out.push(TempSensor {
                        key: format!("{}{}", d.name, i),
                        category: "Other".to_string(),
                        value_celsius: value,
                        stale: false,
                    });
                }
            }
        }
    }
    out
}

/// Read all fan inputs across hwmon devices.
/// For each fan we expose actual RPM and the maximum we've seen this session
/// (kernel doesn't publish a fixed max_rpm anywhere stable).
pub fn read_fans(prev_max: &mut std::collections::HashMap<String, f32>) -> Vec<FanInfo> {
    let mut out = Vec::new();
    let devices = enumerate();
    let mut fan_id = 0u32;
    for d in &devices {
        // Allow any device that exposes fanN_input; thinkpad_acpi is the usual
        // source on ThinkPads, but nct67xx etc. show up on desktops.
        for i in 1..=8 {
            let path = d.path.join(format!("fan{}_input", i));
            if !path.exists() {
                continue;
            }
            let rpm: f32 = sysfs::read_parse::<i64, _>(&path)
                .map(|v| v.max(0) as f32)
                .unwrap_or(0.0);
            let label =
                sysfs::read_string(d.path.join(format!("fan{}_label", i))).unwrap_or_default();
            let name = if label.is_empty() {
                if i == 1 {
                    "Fan".to_string()
                } else {
                    format!("Fan {}", i)
                }
            } else {
                label
            };
            let key = format!("{}:{}", d.name, i);
            let prev = prev_max.entry(key).or_insert(rpm);
            if rpm > *prev {
                *prev = rpm;
            }
            // Stable visible "max"; min stays 0 (unknown without write access).
            let max_rpm = (*prev).max(1.0);
            let frac = (rpm / max_rpm).clamp(0.0, 1.0);
            let estimated_power_w = MAX_FAN_W * frac.powi(3);
            out.push(FanInfo {
                id: fan_id,
                name,
                actual_rpm: rpm,
                min_rpm: 0.0,
                max_rpm,
                estimated_power_w,
            });
            fan_id += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base62_basic() {
        assert_eq!(base62_char(0), Some('0'));
        assert_eq!(base62_char(9), Some('9'));
        assert_eq!(base62_char(10), Some('a'));
        assert_eq!(base62_char(35), Some('z'));
        assert_eq!(base62_char(36), Some('A'));
        assert_eq!(base62_char(61), Some('Z'));
        assert_eq!(base62_char(62), None);
    }

    #[test]
    fn coretemp_key_hybrid() {
        let topo = CpuTopology {
            all: (0..16).collect(),
            pcpus: (0..6).collect(),
            ecpus: (6..14).collect(),
        };
        let (k, c) = coretemp_key("Core 0", &topo).unwrap();
        assert_eq!(c, "CPU");
        assert_eq!(k, "Tp00");

        let (k, _) = coretemp_key("Core 5", &topo).unwrap();
        assert_eq!(k, "Tp05");

        let (k, _) = coretemp_key("Core 6", &topo).unwrap();
        assert_eq!(k, "Te00");
    }
}
