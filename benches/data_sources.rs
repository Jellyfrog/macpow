use criterion::{criterion_group, criterion_main, Criterion};

fn bench_ioreport(c: &mut Criterion) {
    let sampler = macpow::ioreport::IOReportSampler::new().expect("IOReportSampler::new");
    // warm up: need two samples to compute deltas
    let _ = sampler.sample();
    std::thread::sleep(std::time::Duration::from_millis(100));

    c.bench_function("ioreport_sample", |b| b.iter(|| sampler.sample().unwrap()));
}

fn bench_smc(c: &mut Criterion) {
    let mut smc = macpow::smc::SmcConnection::open().expect("SmcConnection::open");

    c.bench_function("smc_temperatures", |b| b.iter(|| smc.read_temperatures()));
    c.bench_function("smc_fans", |b| b.iter(|| smc.read_fans()));
    c.bench_function("smc_system_power", |b| b.iter(|| smc.read_system_power()));
    c.bench_function("smc_keyboard_backlight", |b| {
        b.iter(|| smc.read_keyboard_backlight())
    });
}

fn bench_battery(c: &mut Criterion) {
    c.bench_function("battery", |b| b.iter(|| macpow::battery::read_battery()));
}

fn bench_peripherals(c: &mut Criterion) {
    c.bench_function("wifi", |b| b.iter(|| macpow::peripherals::read_wifi_info()));
    c.bench_function("bluetooth", |b| {
        b.iter(|| macpow::peripherals::read_bluetooth_devices())
    });
    c.bench_function("usb_devices", |b| {
        b.iter(|| macpow::peripherals::list_usb_devices())
    });
    c.bench_function("power_assertions", |b| {
        b.iter(|| macpow::peripherals::list_power_assertions())
    });
}

fn bench_metrics(c: &mut Criterion) {
    c.bench_function("display_brightness", |b| {
        b.iter(|| macpow::metrics::read_display_brightness())
    });
    c.bench_function("audio_volume", |b| {
        b.iter(|| macpow::metrics::read_audio_volume())
    });
    c.bench_function("cpu_ticks", |b| {
        b.iter(|| macpow::metrics::read_cpu_ticks())
    });
    c.bench_function("memory", |b| b.iter(|| macpow::metrics::read_mem_used_gb()));
    c.bench_function("gpu_utilization", |b| {
        b.iter(|| macpow::metrics::read_gpu_utilization())
    });
    c.bench_function("process_energy", |b| {
        b.iter(|| macpow::metrics::read_all_process_energy())
    });
}

criterion_group!(
    benches,
    bench_ioreport,
    bench_smc,
    bench_battery,
    bench_peripherals,
    bench_metrics,
);
criterion_main!(benches);
