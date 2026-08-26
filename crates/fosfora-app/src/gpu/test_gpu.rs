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

use crate::gpu::context::GpuContext;
use crate::gpu::fullscreen_quad::FULLSCREEN_TRIANGLE_VS;

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

/// Read back a render target that has no `COPY_SRC`.
///
/// `RenderTarget` is RENDER_ATTACHMENT | TEXTURE_BINDING, and adding COPY_SRC
/// in production just to satisfy a probe would be the tail wagging the dog —
/// so sample it through a throwaway pass into a test-owned texture instead.
/// Bytes come back UNDECODED: an exact passthrough assertion beats an f16
/// tolerance.
pub fn snapshot(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    src: &wgpu::TextureView,
    dim: u32,
) -> Vec<u8> {
    const BLIT_FS: &str = "
@group(0) @binding(0) var src_tex: texture_2d<f32>;
@fragment
fn fs_main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
return textureLoad(src_tex, vec2i(pos.xy), 0);
}";
    let format = GpuContext::hdr_format();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("trama-test-snapshot"),
        source: wgpu::ShaderSource::Wgsl(format!("{FULLSCREEN_TRIANGLE_VS}\n{BLIT_FS}").into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("trama-test-snapshot"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            targets: &[Some(format.into())],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });

    let dst = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("trama-test-snapshot-dst"),
        size: wgpu::Extent3d {
            width: dim,
            height: dim,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let dst_view = dst.create_view(&Default::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(src),
        }],
    });

    // dim x 8 bytes must be a multiple of the 256-byte row alignment
    // `copy_texture_to_buffer` requires; 64 px gives 512.
    let bpr = dim * 8;
    assert_eq!(bpr % 256, 0, "snapshot needs a 256-byte-aligned row");
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("trama-test-readback"),
        size: u64::from(bpr * dim),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("trama-test-snapshot-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &dst_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &dst,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(dim),
            },
        },
        wgpu::Extent3d {
            width: dim,
            height: dim,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("poll");
    let data = slice.get_mapped_range().to_vec();
    readback.unmap();
    data
}
