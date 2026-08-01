use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BufferBindingType, ColorTargetState,
    CommandEncoder, Device, FragmentState, PipelineCompilationOptions, PipelineLayoutDescriptor,
    PrimitiveState, Queue, RenderPipeline, SamplerBindingType, ShaderStages, TextureFormat,
    TextureSampleType, TextureViewDimension, VertexState,
};

use super::fullscreen_quad::FULLSCREEN_TRIANGLE_VS_WITH_UV;
use super::layer::BlendMode;
use super::render_target::{PingPongTarget, RenderTarget};

const COMPOSITE_FS: &str = include_str!("../../../../assets/shaders/builtin/composite.wgsl");
const BLIT_FS: &str = include_str!("../../../../assets/shaders/builtin/blit.wgsl");

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct CompositeUniforms {
    blend_mode: u32,
    opacity: f32,
    /// Warp strength for the displacement modes (#1478); ignored by the color
    /// blends. Took a spare pad slot, so this stayed a 16-byte struct.
    displace_amount: f32,
    _pad1: f32,
}

/// One layer's contribution to the composite.
pub struct LayerComposite<'a> {
    pub target: &'a RenderTarget,
    pub blend_mode: BlendMode,
    pub opacity: f32,
    pub displace_amount: f32,
}

/// GPU compositor that blends multiple layer outputs together.
pub struct Compositor {
    composite_pipeline: RenderPipeline,
    blit_pipeline: RenderPipeline,
    composite_bgl: BindGroupLayout,
    blit_bgl: BindGroupLayout,
    uniform_buffers: Vec<wgpu::Buffer>,
    /// Ping-pong accumulator for sequential compositing.
    pub accumulator: PingPongTarget,
    /// The `@backdrop` special input (#2061): a stable snapshot of the composite
    /// of every layer BELOW the one currently executing, filled by `frame_graph`
    /// via [`Self::snapshot_backdrop`] / [`Self::clear_backdrop`]. A dedicated
    /// target rather than the accumulator itself because effect bind groups are
    /// prebuilt and need one stable texture identity between resizes.
    pub backdrop: RenderTarget,
}

impl Compositor {
    pub fn new(device: &Device, hdr_format: TextureFormat, width: u32, height: u32) -> Self {
        // Composite pipeline: bg + fg + uniforms → blended output
        let composite_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("compositor-composite-bgl"),
            entries: &[
                tex_entry(0),     // background
                sampler_entry(1), // bg sampler
                tex_entry(2),     // foreground
                sampler_entry(3), // fg sampler
                uniform_entry(4, std::mem::size_of::<CompositeUniforms>()),
            ],
        });
        let composite_pipeline = create_fs_pipeline(
            device,
            "compositor-composite",
            &composite_bgl,
            COMPOSITE_FS,
            hdr_format,
        );

        // Blit pipeline: copy first layer to accumulator
        let blit_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("compositor-blit-bgl"),
            entries: &[tex_entry(0), sampler_entry(1)],
        });
        let blit_pipeline =
            create_fs_pipeline(device, "compositor-blit", &blit_bgl, BLIT_FS, hdr_format);

        // One uniform buffer per composite pass (max 8: 1 for first-layer opacity + 7 for layers[1..])
        let uniform_buffers: Vec<wgpu::Buffer> = (0..8)
            .map(|i| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("compositor-uniforms-{i}")),
                    size: std::mem::size_of::<CompositeUniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();

        let accumulator = PingPongTarget::new(device, width, height, hdr_format, 1.0);
        let backdrop = RenderTarget::new(device, width, height, hdr_format, 1.0, "backdrop");

        Self {
            composite_pipeline,
            blit_pipeline,
            composite_bgl,
            blit_bgl,
            uniform_buffers,
            accumulator,
            backdrop,
        }
    }

    /// Blit `src` (the composite of the layers below the about-to-execute one)
    /// into the stable `@backdrop` target.
    pub fn snapshot_backdrop(
        &self,
        device: &Device,
        encoder: &mut CommandEncoder,
        src: &RenderTarget,
    ) {
        let bg = device.create_bind_group(&BindGroupDescriptor {
            label: Some("backdrop-snapshot-bg"),
            layout: &self.blit_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&src.view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&src.sampler),
                },
            ],
        });
        run_fullscreen_pass(
            encoder,
            "backdrop-snapshot",
            &self.blit_pipeline,
            &bg,
            &self.backdrop.view,
        );
    }

    /// Clear `@backdrop` to transparent — a backdrop consumer with nothing
    /// beneath it (bottom layer, or the single-layer fast path) sees emptiness,
    /// not last frame's stale composite.
    pub fn clear_backdrop(&self, encoder: &mut CommandEncoder) {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("backdrop-clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.backdrop.view,
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
    }

    /// Composite multiple layer outputs into a single HDR result.
    /// Returns a reference to the final composited render target.
    ///
    /// Note that `layers[0]`'s blend mode is never used — there is nothing
    /// beneath the bottom layer to blend against, so it is blitted (or, when
    /// translucent, composited over black as Normal). A displacement mode
    /// (#1478) on the bottom layer is therefore inert; the layer panel says so.
    pub fn composite<'a>(
        &'a self,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        layers: &[LayerComposite<'_>],
    ) -> &'a RenderTarget {
        assert!(!layers.is_empty());

        let first = layers[0].target;
        let first_opacity = layers[0].opacity;

        // Handle first layer: blit if fully opaque, composite against black if not
        if first_opacity < 1.0 {
            // Composite first layer against cleared-to-black accumulator to apply opacity.
            // run_fullscreen_pass clears to black, so bg is black and fg is the first layer.
            let uniforms = CompositeUniforms {
                blend_mode: BlendMode::Normal.as_u32(),
                opacity: first_opacity,
                displace_amount: 0.0,
                _pad1: 0.0,
            };
            queue.write_buffer(&self.uniform_buffers[0], 0, bytemuck::bytes_of(&uniforms));

            // We need a black background. Use the other accumulator target (cleared to black).
            let write_idx = self.accumulator.current;
            let bg_idx = 1 - write_idx;
            // Clear the bg target by running a blit-like pass (it will be cleared by LoadOp::Clear)
            // Actually, just use the composite pass — bg will be the cleared target.
            // We need to clear bg_idx first. Run a dummy clear by beginning+ending a pass.
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("compositor-clear-bg"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.accumulator.targets[bg_idx].view,
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
            }

            let composite_bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("compositor-first-layer-bg"),
                layout: &self.composite_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(
                            &self.accumulator.targets[bg_idx].view,
                        ),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(
                            &self.accumulator.targets[bg_idx].sampler,
                        ),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::TextureView(&first.view),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::Sampler(&first.sampler),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: self.uniform_buffers[0].as_entire_binding(),
                    },
                ],
            });

            run_fullscreen_pass(
                encoder,
                "compositor-first-opacity",
                &self.composite_pipeline,
                &composite_bg,
                &self.accumulator.targets[write_idx].view,
            );
        } else {
            // Fast path: blit first layer directly (opacity == 1.0)
            let blit_bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("compositor-blit-bg"),
                layout: &self.blit_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&first.view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(&first.sampler),
                    },
                ],
            });
            run_fullscreen_pass(
                encoder,
                "compositor-blit",
                &self.blit_pipeline,
                &blit_bg,
                &self.accumulator.write_target().view,
            );
        }

        if layers.len() == 1 {
            return self.accumulator.write_target();
        }

        // Composite subsequent layers using per-pass uniform buffers.
        // After first layer handling, result is in write_target (accumulator.current).
        let mut read_idx = self.accumulator.current;

        for (pass_idx, layer) in layers[1..].iter().enumerate() {
            let fg = layer.target;
            let write_idx = 1 - read_idx;
            // Use buffer [pass_idx + 1] since buffer [0] may be used for first layer opacity
            let buf_idx = pass_idx + 1;

            let uniforms = CompositeUniforms {
                blend_mode: layer.blend_mode.as_u32(),
                opacity: layer.opacity,
                displace_amount: layer.displace_amount,
                _pad1: 0.0,
            };
            queue.write_buffer(
                &self.uniform_buffers[buf_idx],
                0,
                bytemuck::bytes_of(&uniforms),
            );

            let bg_target = &self.accumulator.targets[read_idx];
            let write_target = &self.accumulator.targets[write_idx];

            let composite_bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("compositor-composite-bg"),
                layout: &self.composite_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&bg_target.view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(&bg_target.sampler),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::TextureView(&fg.view),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::Sampler(&fg.sampler),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: self.uniform_buffers[buf_idx].as_entire_binding(),
                    },
                ],
            });

            run_fullscreen_pass(
                encoder,
                "compositor-composite",
                &self.composite_pipeline,
                &composite_bg,
                &write_target.view,
            );

            read_idx = write_idx;
        }

        &self.accumulator.targets[read_idx]
    }

    pub fn resize(&mut self, device: &Device, width: u32, height: u32) {
        self.accumulator.resize(device, width, height);
        // Effect bind groups reference this texture; App/headless resize layers
        // AFTER the compositor so their rebuild binds the new one.
        self.backdrop.resize(device, width, height);
    }
}

// --- Helper functions (same pattern as postprocess.rs) ---

fn tex_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Texture {
            sample_type: TextureSampleType::Float { filterable: true },
            view_dimension: TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Sampler(SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(binding: u32, size: usize) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::FRAGMENT,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: std::num::NonZeroU64::new(size as u64),
        },
        count: None,
    }
}

fn create_fs_pipeline(
    device: &Device,
    label: &str,
    bgl: &BindGroupLayout,
    fragment_src: &str,
    target_format: TextureFormat,
) -> RenderPipeline {
    let full_source = format!("{FULLSCREEN_TRIANGLE_VS_WITH_UV}\n{fragment_src}");
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(full_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some(&format!("{label}-layout")),
        bind_group_layouts: &[bgl],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&format!("{label}-pipeline")),
        layout: Some(&pipeline_layout),
        vertex: VertexState {
            module: &shader_module,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: PipelineCompilationOptions::default(),
        },
        fragment: Some(FragmentState {
            module: &shader_module,
            entry_point: Some("fs_main"),
            targets: &[Some(ColorTargetState {
                format: target_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: PipelineCompilationOptions::default(),
        }),
        primitive: PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

fn run_fullscreen_pass(
    encoder: &mut CommandEncoder,
    label: &str,
    pipeline: &RenderPipeline,
    bind_group: &BindGroup,
    target: &wgpu::TextureView,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
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
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bind_group, &[]);
    pass.draw(0..3, 0..1);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `composite.wgsl` and `blit.wgsl` are builtins: they are `include_str!`'d
    /// straight into pipeline creation, so the `.pfx` compile sweeps never touch
    /// them and a WGSL error surfaces only when the app runs with 2+ layers.
    /// Building a real Compositor validates both, including the displacement
    /// branch added for #1478.
    ///
    /// Run: cargo test -p phosphor-app -- --ignored compositor_builtin_shaders_compile
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn compositor_builtin_shaders_compile() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, _queue) = crate::gpu::test_gpu::test_gpu();

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _compositor = Compositor::new(&device, TextureFormat::Rgba16Float, 64, 64);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(
            err.is_none(),
            "compositor shaders failed to compile: {err:?}"
        );
    }

    /// The WGSL mirror of `CompositeUniforms` is maintained by hand. Adding
    /// `displace_amount` took a spare pad slot rather than growing the struct;
    /// if that ever stops being true the shader's copy must change with it.
    #[test]
    fn composite_uniforms_stay_sixteen_bytes() {
        assert_eq!(std::mem::size_of::<CompositeUniforms>(), 16);
    }

    const PROBE_DIM: u32 = 64;

    /// Run one composite pass over synthetic inputs and read the result back.
    ///
    /// Rgba8Unorm rather than the production Rgba16Float: the blend math is
    /// identical, the readback needs no half-float decode, and one row is
    /// exactly the 256-byte copy alignment.
    fn probe_composite(
        device: &Device,
        queue: &Queue,
        dim: u32,
        bg_pixels: &[u8],
        fg_pixels: &[u8],
        uniforms: CompositeUniforms,
    ) -> Vec<u8> {
        let format = TextureFormat::Rgba8Unorm;

        let make_input = |label: &str, data: &[u8]| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: dim,
                    height: dim,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(dim * 4),
                    rows_per_image: Some(dim),
                },
                wgpu::Extent3d {
                    width: dim,
                    height: dim,
                    depth_or_array_layers: 1,
                },
            );
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (tex, view)
        };

        let (_bg_tex, bg_view) = make_input("probe-bg", bg_pixels);
        let (_fg_tex, fg_view) = make_input("probe-fg", fg_pixels);

        // ClampToEdge + Linear, matching RenderTarget's sampler — the warp
        // relies on both (clamped borders, interpolated off-grid samples).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("probe-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let out = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("probe-out"),
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
        let out_view = out.create_view(&wgpu::TextureViewDescriptor::default());

        let bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("probe-bgl"),
            entries: &[
                tex_entry(0),
                sampler_entry(1),
                tex_entry(2),
                sampler_entry(3),
                uniform_entry(4, std::mem::size_of::<CompositeUniforms>()),
            ],
        });
        let pipeline = create_fs_pipeline(device, "probe-composite", &bgl, COMPOSITE_FS, format);

        let ubo = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe-uniforms"),
            size: std::mem::size_of::<CompositeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ubo, 0, bytemuck::bytes_of(&uniforms));

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("probe-bg-group"),
            layout: &bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&bg_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::TextureView(&fg_view),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::Sampler(&sampler),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: ubo.as_entire_binding(),
                },
            ],
        });

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe-readback"),
            size: (dim * dim * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        run_fullscreen_pass(
            &mut encoder,
            "probe-pass",
            &pipeline,
            &bind_group,
            &out_view,
        );
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &out,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(dim * 4),
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

    /// Background: a horizontal red ramp, so a pixel's red channel encodes the
    /// x it was sampled from. Any horizontal warp shows up as a red shift.
    fn ramp_background(dim: u32) -> Vec<u8> {
        let mut px = vec![0u8; (dim * dim * 4) as usize];
        for y in 0..dim {
            for x in 0..dim {
                let i = ((y * dim + x) * 4) as usize;
                px[i] = (x * 255 / (dim - 1)) as u8;
                px[i + 3] = 255;
            }
        }
        px
    }

    /// Foreground: opaque, black on the left half and white on the right. The
    /// luminance gradient is zero everywhere except at the step in the middle.
    fn step_foreground(dim: u32) -> Vec<u8> {
        let mut px = vec![0u8; (dim * dim * 4) as usize];
        for y in 0..dim {
            for x in 0..dim {
                let i = ((y * dim + x) * 4) as usize;
                let v = if x >= dim / 2 { 255 } else { 0 };
                px[i] = v;
                px[i + 1] = v;
                px[i + 2] = v;
                px[i + 3] = 255;
            }
        }
        px
    }

    /// Foreground: a smooth horizontal cosine whose period is a fixed fraction
    /// of the frame (0.25 UV), not a fixed number of texels.
    ///
    /// This is what real content looks like — a bloomed ring covers the same
    /// share of the frame at 1080p and 4K. A hard step cannot substitute: its
    /// edge is always exactly one texel wide whatever the resolution, so a
    /// texel-sized gradient stencil measures the same slope either way and the
    /// resolution bug hides completely.
    fn soft_foreground(dim: u32) -> Vec<u8> {
        let mut px = vec![0u8; (dim * dim * 4) as usize];
        for y in 0..dim {
            for x in 0..dim {
                let i = ((y * dim + x) * 4) as usize;
                let u = x as f32 / dim as f32;
                let v = (0.5 + 0.5 * (u / 0.25 * std::f32::consts::TAU).cos()).clamp(0.0, 1.0);
                let b = (v * 255.0) as u8;
                px[i] = b;
                px[i + 1] = b;
                px[i + 2] = b;
                px[i + 3] = 255;
            }
        }
        px
    }

    fn red_at(pixels: &[u8], dim: u32, x: u32, y: u32) -> i32 {
        pixels[((y * dim + x) * 4) as usize] as i32
    }

    /// Displace must warp where the foreground has structure and leave the rest
    /// alone. Compiling proves nothing here: a gradient that always evaluates to
    /// zero, or a `displace_amount` that never reaches the GPU, both compile fine
    /// and silently render as a plain background.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn displace_warps_at_the_gradient_and_nowhere_else() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();

        let bg = ramp_background(PROBE_DIM);
        let fg = step_foreground(PROBE_DIM);
        let mid = PROBE_DIM / 2;
        let y = PROBE_DIM / 2;

        let out = probe_composite(
            &device,
            &queue,
            PROBE_DIM,
            &bg,
            &fg,
            CompositeUniforms {
                blend_mode: BlendMode::Displace.as_u32(),
                opacity: 1.0,
                displace_amount: 1.0,
                _pad1: 0.0,
            },
        );

        // Flat foreground region: gradient is zero, so the background is passed
        // through untouched.
        for x in [4u32, 16, 48, 60] {
            assert_eq!(
                red_at(&out, PROBE_DIM, x, y),
                red_at(&bg, PROBE_DIM, x, y),
                "x={x} should be untouched (flat foreground)"
            );
        }

        // At the step the gradient is ~0.5, giving a right-shifted sample, so
        // the red ramp reads higher than it does at that column in the source.
        let shifted = red_at(&out, PROBE_DIM, mid, y) - red_at(&bg, PROBE_DIM, mid, y);
        assert!(
            shifted > 8,
            "expected a visible rightward warp at the step, got {shifted}"
        );

        // Zero amount must be a no-op — proves displace_amount is actually
        // plumbed through to the shader rather than defaulted somewhere.
        let unwarped = probe_composite(
            &device,
            &queue,
            PROBE_DIM,
            &bg,
            &fg,
            CompositeUniforms {
                blend_mode: BlendMode::Displace.as_u32(),
                opacity: 1.0,
                displace_amount: 0.0,
                _pad1: 0.0,
            },
        );
        assert_eq!(
            red_at(&unwarped, PROBE_DIM, mid, y),
            red_at(&bg, PROBE_DIM, mid, y),
            "displace_amount = 0 must leave the background alone"
        );
    }

    /// The displacement family draws none of the foreground (Kevin's call:
    /// a displacing layer is a pure warp field). Normal, given the same inputs,
    /// replaces the background with it — so the two must not agree.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn displacement_modes_draw_none_of_the_foreground() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();

        let bg = ramp_background(PROBE_DIM);
        let fg = step_foreground(PROBE_DIM);
        let y = PROBE_DIM / 2;
        // Deep in the BLACK half, away from the step. The two behaviours are
        // maximally far apart here: Normal paints the black foreground (0),
        // while a warp mode has a flat, unlit field and passes the background
        // straight through (~40). Sampling the white half instead would only
        // separate 255 from ~246 — a margin thin enough to pass by luck.
        let x = 10;
        let bg_here = red_at(&bg, PROBE_DIM, x, y);
        assert!(bg_here > 30, "probe background too dark to discriminate");

        for mode in BlendMode::DISPLACEMENT {
            let out = probe_composite(
                &device,
                &queue,
                PROBE_DIM,
                &bg,
                &fg,
                CompositeUniforms {
                    blend_mode: mode.as_u32(),
                    opacity: 1.0,
                    displace_amount: 1.0,
                    _pad1: 0.0,
                },
            );
            assert_eq!(
                red_at(&out, PROBE_DIM, x, y),
                bg_here,
                "{mode:?} should pass the background through, not draw the foreground"
            );
        }

        let normal = probe_composite(
            &device,
            &queue,
            PROBE_DIM,
            &bg,
            &fg,
            CompositeUniforms {
                blend_mode: BlendMode::Normal.as_u32(),
                opacity: 1.0,
                displace_amount: 1.0,
                _pad1: 0.0,
            },
        );
        assert_eq!(
            red_at(&normal, PROBE_DIM, x, y),
            0,
            "Normal should paint the black foreground over the background"
        );
    }

    /// Background with independent structure per channel, at three different
    /// frequencies — the only kind that reveals a per-channel sampling offset.
    /// A monochrome or single-hue background hides dispersion completely.
    fn chromatic_background(dim: u32) -> Vec<u8> {
        let mut px = vec![0u8; (dim * dim * 4) as usize];
        for y in 0..dim {
            for x in 0..dim {
                let i = ((y * dim + x) * 4) as usize;
                let u = x as f32 / dim as f32;
                let v = y as f32 / dim as f32;
                px[i] = ((0.5 + 0.5 * (u * 18.0).sin()) * 255.0) as u8;
                px[i + 1] = ((0.5 + 0.5 * (v * 25.0).sin()) * 255.0) as u8;
                px[i + 2] = ((0.5 + 0.5 * ((u + v) * 31.0).sin()) * 255.0) as u8;
                px[i + 3] = 255;
            }
        }
        px
    }

    fn mean_abs_diff(a: &[u8], b: &[u8]) -> f32 {
        (a.iter()
            .zip(b)
            .map(|(p, q)| (*p as i32 - *q as i32).unsigned_abs() as f64)
            .sum::<f64>()
            / a.len() as f64) as f32
    }

    /// Each displacement mode must look meaningfully unlike the others.
    ///
    /// This is finding #1925 as a test: a mode that is not perceptibly distinct
    /// is clutter in the dropdown, not a feature. Refract originally reused
    /// Displace's offset vector with green at 0.94x and blue at 0.88x, leaving
    /// red byte-identical — measured at 0.61 against a warp of 7.83, i.e. 8%.
    /// Kevin spotted it by eye in the live review. It is now body-driven
    /// (magnitude from luminance, so shape interiors move) with symmetric
    /// dispersion, which puts it at ~70% of the warp magnitude.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn displacement_modes_are_mutually_distinct() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();
        let dim = 256u32;

        let bg = chromatic_background(dim);
        let fg = soft_foreground(dim);
        let render = |mode: BlendMode| {
            probe_composite(
                &device,
                &queue,
                dim,
                &bg,
                &fg,
                CompositeUniforms {
                    blend_mode: mode.as_u32(),
                    opacity: 1.0,
                    displace_amount: 0.35,
                    _pad1: 0.0,
                },
            )
        };

        let displace = render(BlendMode::Displace);
        let refract = render(BlendMode::Refract);
        let lens = render(BlendMode::Lens);

        // Scale the bar to how much Displace moves the image at all, so the test
        // measures separation rather than absolute warp strength.
        let warp = mean_abs_diff(&displace, &bg);
        assert!(
            warp > 2.0,
            "Displace barely warped ({warp}) — bar is meaningless"
        );
        let floor = 0.25 * warp;

        for (name, other) in [("Refract", &refract), ("Lens", &lens)] {
            let d = mean_abs_diff(&displace, other);
            assert!(
                d > floor,
                "{name} is too close to Displace: {d} vs floor {floor} (warp {warp})"
            );
        }
        let rl = mean_abs_diff(&refract, &lens);
        assert!(
            rl > floor,
            "Refract and Lens are too close: {rl} vs floor {floor}"
        );
    }

    /// The same scene must warp by the same amount at any output resolution.
    ///
    /// The first cut sampled the luminance gradient one *texel* apart, which
    /// made the warp scale with output size — a look dialled in at 1080p would
    /// come out different at 4K, which is useless to a VJ. Sampling a fixed UV
    /// step fixed it, and this test is what stops it coming back. It also
    /// catches the related regression: a one-texel stencil measures per-texel
    /// slope, so on soft glow — most of what this engine renders — the gradient
    /// collapses toward zero and the effect quietly stops doing anything.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn warp_strength_is_independent_of_resolution() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();

        // Peak warp as a FRACTION of frame width, over the identical scene at two
        // resolutions. The red ramp encodes source x, so a shift in red is a
        // shift in x. Taking the max over the row rather than one column keeps
        // the measurement independent of where the cosine's steepest slope lands.
        let measure = |dim: u32| -> f32 {
            let bg = ramp_background(dim);
            let fg = soft_foreground(dim);
            let out = probe_composite(
                &device,
                &queue,
                dim,
                &bg,
                &fg,
                CompositeUniforms {
                    blend_mode: BlendMode::Displace.as_u32(),
                    opacity: 1.0,
                    displace_amount: 1.0,
                    _pad1: 0.0,
                },
            );
            let y = dim / 2;
            // Middle 60% only — the sampler clamps at the borders, which would
            // cap the shift and understate the warp.
            (dim / 5..dim * 4 / 5)
                .map(|x| (red_at(&out, dim, x, y) - red_at(&bg, dim, x, y)).abs() as f32 / 255.0)
                .fold(0.0, f32::max)
        };

        let small = measure(64);
        let large = measure(256);

        assert!(
            small > 0.02,
            "no measurable warp at 64px (got {small}) — the probe can't tell"
        );
        assert!(
            (small - large).abs() < 0.25 * small.max(large),
            "warp scaled with resolution: {small} at 64px vs {large} at 256px"
        );
    }
}
