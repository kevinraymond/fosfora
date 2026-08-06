//! A GPU device with no window, for the offline scene renderer (#2027).
//!
//! `GpuContext` cannot serve here — its `surface` field is non-optional and its
//! constructor takes a winit window. This requests a device the way the *app*
//! does rather than the way the test probes do: the probes ask for
//! `adapter.limits()` (`gpu/test_gpu.rs`), which is a superset of what the app
//! requests, so a scene that renders under probe limits is not guaranteed to
//! render in the app. Judging must predict the app, so the limits below mirror
//! `GpuContext::new` exactly.

use anyhow::{Context, Result};

/// Request an adapter + device suitable for offline scene rendering.
pub fn create() -> Result<(wgpu::Device, wgpu::Queue, String)> {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..Default::default()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .or_else(|_| {
        // Vulkan-first like the app; fall back to any backend so lavapipe-only
        // hosts still work (slowly).
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
    })
    .context("no GPU adapter available — offline rendering needs Vulkan (or lavapipe)")?;

    let info = adapter.get_info();
    let adapter_desc = format!("{} ({:?})", info.name, info.backend);
    let adapter_limits = adapter.limits();

    // Mirror GpuContext::new's request (gpu/context.rs) — the whole point of a
    // headless render is that its success predicts the app's.
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("fosfora-headless-device"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits {
            max_storage_buffers_per_shader_stage: 16,
            max_bind_groups: 5, // groups 0-3 standard + group 4 for R-D texture
            max_storage_buffer_binding_size: adapter_limits.max_storage_buffer_binding_size,
            max_buffer_size: adapter_limits.max_buffer_size,
            ..wgpu::Limits::default()
        },
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .context("device request failed")?;

    Ok((device, queue, adapter_desc))
}
