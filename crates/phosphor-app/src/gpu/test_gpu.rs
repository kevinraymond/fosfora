//! One shared wgpu device for the `#[ignore]`d GPU probe tests.
//!
//! Each probe used to build its own `Instance`/`Adapter`/`Device`. Several running
//! concurrently on the NVIDIA/Vulkan driver SIGSEGV partway through `cargo test --
//! --ignored` (#1922). Sharing one device fixes that; a process-wide lock on top
//! keeps their per-device validation error scopes (`push_error_scope` /
//! `pop_error_scope`, which form a per-device stack) from interleaving, and lets
//! the timing probes run uncontended. With both, the whole set passes without
//! `--test-threads=1`.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use wgpu::{Device, Queue};

static GPU: OnceLock<(Arc<Device>, Arc<Queue>)> = OnceLock::new();
static LOCK: Mutex<()> = Mutex::new(());

/// The shared probe device/queue, created once on first use. Requested with the
/// adapter's full limits, which is what every probe asked for individually.
pub fn test_gpu() -> (Arc<Device>, Arc<Queue>) {
    GPU.get_or_init(|| {
        // Vulkan-first (the #1922 shared-device fix targeted the NVIDIA/Vulkan dev box),
        // plus Metal so the probes also run on macOS. Never both on one platform.
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("probe adapter: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("probe-shared"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("no wgpu device");
        (Arc::new(device), Arc::new(queue))
    })
    .clone()
}

/// Serialize probes that push/pop validation error scopes on the shared device.
/// Hold the returned guard for the whole probe body. A poisoned lock is recovered
/// rather than propagated — a panicking probe still leaves the device usable.
pub fn gpu_guard() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
