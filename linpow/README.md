# 🐧🔋 linpow – Real-time power tree TUI for Linux ThinkPad X1

Linux counterpart to [macpow](https://github.com/k06a/macpow), with a 1:1 copy
of the UI. Targets ThinkPad X1 (Intel CPU + iGPU). Reads from sysfs, procfs,
hwmon, and Intel RAPL; designed to run **both as a regular user and as root**
with graceful degradation when permissions are missing.

### Legend

| Symbol | Meaning |
|--------|---------|
| `0.123 W` | Measured power (direct hardware reading) |
| `≈0.123 W` | Estimated power (model-based) |
| `≤0.123 W` | Upper-bound power estimate |
| `▸` | Pinned resource (sparkline chart visible) |
| `▓▓▓░░░░░░░` | CPU core utilization bar |
| `37°C` | Fresh temperature reading |
| `~37°C` | Stale temperature (sensor read failed, last known value) |
| `pending…` | Data source still initializing |
| `[dead]` | Process has exited (energy total preserved) |
| **Bold** | Section headers |
| Green | Low power (< 1 W) |
| Yellow | Moderate power (1–5 W) |
| Orange | High power (5–10 W) |
| Red | Very high power (> 10 W) |

## Data sources

| Subsystem | Source |
|-----------|--------|
| CPU package / cores / uncore / DRAM power | `/sys/class/powercap/intel-rapl:*` |
| Per-core utilization | `/proc/stat` |
| Per-core frequency | `/sys/devices/system/cpu/cpu*/cpufreq/scaling_cur_freq` |
| Hybrid P/E split | `/sys/devices/cpu_core/cpus`, `/sys/devices/cpu_atom/cpus` |
| Memory | `/proc/meminfo` |
| iGPU frequency | `/sys/class/drm/card*/gt_*_freq_mhz` |
| Battery | `/sys/class/power_supply/BAT*/` |
| AC adapter | `/sys/class/power_supply/AC*/` |
| Temperatures | `/sys/class/hwmon/*/temp*_input` (coretemp, nvme, iwlwifi, thinkpad, acpitz) |
| Fans | `/sys/class/hwmon/*/fan*_input` |
| Display backlight | `/sys/class/backlight/intel_backlight/` |
| Keyboard backlight | `/sys/class/leds/tpacpi::kbd_backlight/` |
| Network bytes | `/proc/net/dev`, `/sys/class/net/` |
| Disk I/O | `/sys/block/*/stat` |

## Install

```
cargo install --path .
```

## Permissions

linpow runs in **two modes**:

**Plain user** (no setup): tree renders, but RAPL-derived power columns
(CPU/GPU/DRAM watts) are blank and per-process attribution only sees your own
processes. Everything else (CPU util, frequencies, temps, fans, battery, disk,
network) works.

**Full data**: install the udev rule so all users can read RAPL counters.

```
sudo cp etc/60-linpow.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=powercap
```

Optionally lower `perf_event_paranoid` to enable iGPU per-engine busy
counters via the i915/xe PMU (future phase):

```
echo 1 | sudo tee /proc/sys/kernel/perf_event_paranoid
```

At startup linpow prints a one-line capability summary to stderr so you can
see what's blocked:

```
linpow: rapl=ok i915-pmu=denied(perf_event_paranoid=2) nvme-smart=denied all-procs=denied(non-root)
```

## Usage

```
linpow                  # Launch TUI at 250ms intervals
linpow --interval 500   # Custom interval
linpow --json           # Stream JSON to stdout instead of TUI
linpow --dump           # Show discovered RAPL/hwmon paths (diagnostics)
linpow --dump-hwmon     # Full hwmon enumeration with values
```

### Keybindings

| Key | Action |
|-----|--------|
| `q` / `Esc` | Quit |
| `Up` / `Down` / `j` / `k` | Move cursor |
| `Left` / `Right` / `h` | Collapse / expand tree node |
| `+` / `=` | Expand all nodes |
| `-` | Collapse all nodes |
| `Space` | Pin/unpin resource chart |
| `a` | Cycle SMA window: 0s / 5s / 10s |
| `l` | Cycle refresh interval: 250ms / 500ms / 1s / 2s |
| `r` | Reset all totals and min/max |
| `PgUp` / `PgDn` | Scroll by 10 rows |
| `Home` | Jump to top |
| Mouse click | Select row |

## Architecture

Mirrors macpow's design exactly: one thread per data source, all updating a
shared `Arc<Mutex<Metrics>>`. The TUI snapshots that struct each frame and
never blocks on a slow source. Each sampler probes its inputs once at startup;
on `EACCES` or `ENOENT` it goes silent for the session rather than spinning.

```
src/
├── app.rs        # TUI rendering — preserved from macpow verbatim
├── types.rs      # Metrics struct (== JSON schema) — verbatim
├── sma.rs        # Time-weighted SMA — verbatim
├── metrics.rs    # Sampler skeleton; spawns one thread per source
├── caps.rs       # Capability probe + startup summary
├── rapl.rs       # /sys/class/powercap/intel-rapl with wrap handling
├── cpufreq.rs    # /proc/stat ticks + cpufreq + hybrid topology
├── meminfo.rs    # /proc/meminfo
├── hwmon.rs      # /sys/class/hwmon discovery & classification
├── igpu.rs       # iGPU freq via /sys/class/drm/card*/gt_*_freq_mhz
├── battery.rs    # /sys/class/power_supply BAT* + AC*
├── netif.rs      # /proc/net/dev + /sys/class/net classification
├── disk.rs       # /sys/block/*/stat
├── display.rs    # backlight + EDID geometry
└── sysfs.rs      # small helpers (read_string, read_parse, dir_entries)
```

## License

MIT
