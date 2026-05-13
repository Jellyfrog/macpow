#[cfg(not(target_os = "linux"))]
compile_error!("linpow supports Linux targets only.");

pub mod battery;
pub mod caps;
pub mod cpufreq;
pub mod disk;
pub mod display;
pub mod hwmon;
pub mod igpu;
pub mod meminfo;
pub mod metrics;
pub mod netif;
pub mod process_utils;
pub mod procstat;
pub mod rapl;
pub mod sma;
pub mod sysfs;
pub mod types;
