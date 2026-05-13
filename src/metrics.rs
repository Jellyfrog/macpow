use crate::battery;
use crate::cpufreq::{self, CpuTicks, CpuTopology};
use crate::disk;
use crate::display;
use crate::hwmon;
use crate::igpu;
use crate::meminfo;
use crate::netif;
use crate::rapl::{self, DomainKind, DomainState};
use crate::types::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct StaticSnapshot {
    pub gpu_cores: u32,
    pub dram_gb: u32,
    pub ssd_model: String,
}

fn spawn_periodic(
    handles: &mut Vec<JoinHandle<()>>,
    running: &Arc<AtomicBool>,
    period: Duration,
    mut tick: impl FnMut() + Send + 'static,
) {
    let running = running.clone();
    handles.push(std::thread::spawn(move || {
        while running.load(Ordering::Relaxed) {
            tick();
            std::thread::sleep(period);
        }
    }));
}

pub struct Sampler {
    shared: Arc<Mutex<Metrics>>,
    #[allow(dead_code)]
    static_snapshot: StaticSnapshot,
    running: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
}

impl Sampler {
    pub fn new(interval_ms: u64) -> Self {
        let dt = Duration::from_millis(interval_ms.max(100));
        let dt_slow = Duration::from_secs(1);

        let topo = CpuTopology::detect();
        let ssd_model = disk::read_ssd_model();
        let (_, dram_gb) = meminfo::read_mem_used_total_gb().unwrap_or((0.0, 0));

        let static_snapshot = StaticSnapshot {
            gpu_cores: 0,
            dram_gb,
            ssd_model: ssd_model.clone(),
        };

        let initial_metrics = Metrics {
            dram_gb,
            ssd_model,
            ..Default::default()
        };
        let shared = Arc::new(Mutex::new(initial_metrics));
        let running = Arc::new(AtomicBool::new(true));
        let mut handles: Vec<JoinHandle<()>> = Vec::new();

        // ── CPU util + freq ───────────────────────────────────────────────
        {
            let m = shared.clone();
            let topo = topo.clone();
            let mut prev: HashMap<u32, CpuTicks> = HashMap::new();
            spawn_periodic(&mut handles, &running, dt, move || {
                let Some((_, cur)) = cpufreq::read_proc_stat() else {
                    return;
                };

                let (pcpus, ecpus): (Vec<u32>, Vec<u32>) = if topo.is_hybrid() {
                    (topo.pcpus.clone(), topo.ecpus.clone())
                } else {
                    // Treat every CPU as a P-core on non-hybrid CPUs.
                    (topo.all.clone(), Vec::new())
                };

                let mut cpu_usage = Vec::with_capacity(pcpus.len() + ecpus.len());
                let mut p_cores = Vec::with_capacity(pcpus.len());
                for (i, cpu) in pcpus.iter().enumerate() {
                    let u = prev
                        .get(cpu)
                        .and_then(|p| cur.get(cpu).map(|c| cpufreq::util_pct(*p, *c)))
                        .unwrap_or(0.0);
                    cpu_usage.push(u);
                    p_cores.push(CpuCore {
                        name: format!("PCPU{}", i),
                        watts: 0.0,
                    });
                }
                let mut e_cores = Vec::with_capacity(ecpus.len());
                for (i, cpu) in ecpus.iter().enumerate() {
                    let u = prev
                        .get(cpu)
                        .and_then(|p| cur.get(cpu).map(|c| cpufreq::util_pct(*p, *c)))
                        .unwrap_or(0.0);
                    cpu_usage.push(u);
                    e_cores.push(CpuCore {
                        name: format!("ECPU{}", i),
                        watts: 0.0,
                    });
                }

                let p_freq = avg_freq_mhz(&pcpus);
                let e_freq = avg_freq_mhz(&ecpus);

                if let Ok(mut mg) = m.lock() {
                    mg.cpu_usage_pct = cpu_usage;
                    mg.soc.pcpu_cluster = CpuCluster {
                        name: "PCPU".to_string(),
                        total_w: mg.soc.pcpu_cluster.total_w, // preserved across ticks; RAPL updates it
                        cores: p_cores,
                    };
                    mg.soc.ecpu_clusters = if e_cores.is_empty() {
                        Vec::new()
                    } else {
                        vec![CpuCluster {
                            name: "ECPU".to_string(),
                            total_w: mg
                                .soc
                                .ecpu_clusters
                                .first()
                                .map(|c| c.total_w)
                                .unwrap_or(0.0),
                            cores: e_cores,
                        }]
                    };
                    mg.soc.pcpu_freq_mhz = p_freq;
                    mg.soc.ecpu_freq_mhz = e_freq;
                }

                prev = cur;
            });
        }

        // ── Memory ────────────────────────────────────────────────────────
        {
            let m = shared.clone();
            spawn_periodic(&mut handles, &running, dt_slow, move || {
                if let Some((used, total)) = meminfo::read_mem_used_total_gb() {
                    if let Ok(mut mg) = m.lock() {
                        mg.mem_used_gb = used;
                        mg.dram_gb = total;
                    }
                }
            });
        }

        // ── Battery + AC adapter ──────────────────────────────────────────
        {
            let m = shared.clone();
            spawn_periodic(&mut handles, &running, dt_slow, move || {
                let bat = battery::read_battery();
                let adapter = battery::read_adapter();
                let ext = adapter.connected;
                if let Ok(mut mg) = m.lock() {
                    mg.battery = BatteryInfo {
                        external_connected: ext,
                        ..bat
                    };
                    mg.adapter = adapter;
                }
            });
        }

        // ── Display + keyboard backlight ──────────────────────────────────
        {
            let m = shared.clone();
            spawn_periodic(&mut handles, &running, dt_slow, move || {
                let d = display::read_display();
                let k = display::read_keyboard();
                if let Ok(mut mg) = m.lock() {
                    mg.display = d;
                    mg.keyboard = k;
                    mg.backlight_power_w = mg.display.estimated_power_w;
                }
            });
        }

        // ── Network (per-interface and total) ─────────────────────────────
        {
            let m = shared.clone();
            let mut state = netif::NetState::new();
            spawn_periodic(&mut handles, &running, dt, move || {
                let cur = netif::read_proc_net_dev();
                let ifaces = netif::classify();
                let total = state.sample_total_rate(&cur);
                let eth_rate = ifaces
                    .ethernet
                    .as_ref()
                    .map(|n| state.sample_iface_rate(&cur, n))
                    .unwrap_or_default();
                let wifi_rate = ifaces
                    .wifi
                    .as_ref()
                    .map(|n| state.sample_iface_rate(&cur, n))
                    .unwrap_or_default();
                let eth_info = netif::read_ethernet(ifaces.ethernet.as_ref());
                let wifi_info = netif::read_wifi(ifaces.wifi.as_ref());
                state.commit(cur);
                if let Ok(mut mg) = m.lock() {
                    mg.network = total;
                    mg.eth_network = eth_rate;
                    mg.wifi_network = wifi_rate;
                    mg.ethernet = eth_info;
                    mg.wifi = wifi_info;
                }
            });
        }

        // ── iGPU (freq, util) ─────────────────────────────────────────────
        {
            let m = shared.clone();
            spawn_periodic(&mut handles, &running, dt, move || {
                let g = igpu::read();
                if let Ok(mut mg) = m.lock() {
                    mg.soc.gpu_freq_mhz = g.freq_mhz;
                    mg.soc.gpu_util_device = g.util_device;
                    mg.soc.gpu_util_renderer = g.util_renderer;
                    mg.soc.gpu_util_tiler = g.util_tiler;
                }
            });
        }

        // ── hwmon (temperatures + fans) ───────────────────────────────────
        {
            let m = shared.clone();
            let topo = topo.clone();
            let mut fan_max: std::collections::HashMap<String, f32> =
                std::collections::HashMap::new();
            spawn_periodic(&mut handles, &running, dt, move || {
                let temps = hwmon::read_temperatures(&topo);
                let fans = hwmon::read_fans(&mut fan_max);
                if let Ok(mut mg) = m.lock() {
                    mg.temperatures = temps;
                    mg.fans = fans;
                }
            });
        }

        // ── RAPL ──────────────────────────────────────────────────────────
        {
            let m = shared.clone();
            let domains = rapl::enumerate();
            let mut states: Vec<DomainState> =
                (0..domains.len()).map(|_| DomainState::default()).collect();
            // Prime the counters so the first published value reflects a real interval.
            for (d, s) in domains.iter().zip(states.iter_mut()) {
                let _ = s.tick(d);
            }
            spawn_periodic(&mut handles, &running, dt, move || {
                if domains.is_empty() {
                    return;
                }
                let mut cores_w = 0.0;
                let mut gpu_w = 0.0;
                let mut dram_w = 0.0;
                let mut psys_w = 0.0;
                let mut package_w = 0.0;
                for (d, s) in domains.iter().zip(states.iter_mut()) {
                    let w = s.tick(d);
                    match d.kind {
                        DomainKind::Package => package_w += w,
                        DomainKind::Cores => cores_w += w,
                        DomainKind::GpuUncore => gpu_w += w,
                        DomainKind::Dram => dram_w += w,
                        DomainKind::Psys => psys_w += w,
                        DomainKind::Other => {}
                    }
                }
                // If we only have package power, attribute it all to CPU.
                if cores_w == 0.0 && package_w > 0.0 {
                    cores_w = package_w - gpu_w - dram_w;
                    if cores_w < 0.0 {
                        cores_w = package_w;
                    }
                }
                if let Ok(mut mg) = m.lock() {
                    mg.soc.cpu_w = cores_w;
                    mg.soc.gpu_w = gpu_w;
                    mg.soc.dram_w = dram_w;
                    mg.soc.pcpu_cluster.total_w = cores_w;
                    mg.soc.compute_total();
                    mg.sys_power_w = if psys_w > 0.0 {
                        psys_w
                    } else if package_w > 0.0 {
                        package_w + dram_w
                    } else {
                        cores_w + gpu_w + dram_w
                    };
                }
            });
        }

        // ── Disk I/O ──────────────────────────────────────────────────────
        {
            let m = shared.clone();
            let mut state = disk::DiskState::new();
            spawn_periodic(&mut handles, &running, dt, move || {
                let info = state.sample();
                let est = disk::estimate_ssd_power_w(&info);
                if let Ok(mut mg) = m.lock() {
                    mg.disk = info;
                    mg.ssd_power_w = est;
                }
            });
        }

        Self {
            shared,
            static_snapshot,
            running,
            handles,
        }
    }

    pub fn snapshot(&self) -> Metrics {
        self.shared.lock().map(|m| m.clone()).unwrap_or_default()
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

fn avg_freq_mhz(cpus: &[u32]) -> u32 {
    if cpus.is_empty() {
        return 0;
    }
    let mut sum: u64 = 0;
    let mut n: u64 = 0;
    for c in cpus {
        if let Some(mhz) = cpufreq::read_cpu_freq_mhz(*c) {
            sum += mhz as u64;
            n += 1;
        }
    }
    if n == 0 {
        0
    } else {
        (sum / n) as u32
    }
}
