use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BufferBindingType, ColorTargetState,
    CommandEncoder, Device, FragmentState, PipelineCompilationOptions, PipelineLayoutDescriptor,
    PrimitiveState, Queue, RenderPipeline, SamplerBindingType, ShaderStages, TextureFormat,
    TextureSampleType, TextureView, TextureViewDimension, VertexState,
};

use crate::effect::format::PostProcessDef;

use super::fullscreen_quad::FULLSCREEN_TRIANGLE_VS_WITH_UV;
use super::render_target::RenderTarget;

const BLOOM_EXTRACT_FS: &str =
    include_str!("../../../../assets/shaders/builtin/bloom_extract.wgsl");
const BLOOM_BLUR_FS: &str = include_str!("../../../../assets/shaders/builtin/bloom_blur.wgsl");
const POST_COMPOSITE_FS: &str =
    include_str!("../../../../assets/shaders/builtin/post_composite.wgsl");
const BLIT_FS: &str = include_str!("../../../../assets/shaders/builtin/blit.wgsl");
const FLASH_LIMIT_CS: &str = include_str!("../../../../assets/shaders/builtin/flash_limit.wgsl");

/// Size of `FlashState` in flash_limit.wgsl: a 16-byte header (gain, last time,
/// budget, pad) and 32 tracks of two vec4f.
const FLASH_STATE_SIZE: u64 = 16 + 32 * 32;

/// `FlashState` as it must start: gain 1, every track unset. The shader resets
/// on a budget change too, but a zeroed buffer would hold four rises at t = 0
/// and block the first second after launch.
fn flash_state_init() -> Vec<f32> {
    let mut v = vec![1.0, -1.0, -1.0, 0.0];
    for _ in 0..32 {
        v.extend_from_slice(&[0.0, 1e9, 0.0, 0.0]);
        v.extend_from_slice(&[-1e9; 4]);
    }
    v
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct BloomParams {
    threshold: f32,
    soft_knee: f32,
    rms: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct BlurParams {
    direction: [f32; 2],
    _pad: [f32; 2],
}

/// The per-frame resolved output-alpha mode the composite shader executes.
///
/// This is the *resolved* form of [`crate::settings::AlphaOutputMode`] — Auto has
/// already been decided by `frame_graph::resolve_output_alpha` by the time it gets
/// here. Values are the WGSL-side encoding in `PostParams::alpha_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode {
    /// Alpha forced to 1.0 (historical behavior).
    Opaque,
    /// Alpha derived from output brightness (legacy NDI luma key).
    Luma,
    /// The scene's real coverage alpha survives to the output (premultiplied).
    Passthrough,
}

impl AlphaMode {
    fn as_f32(self) -> f32 {
        match self {
            AlphaMode::Opaque => 0.0,
            AlphaMode::Luma => 1.0,
            AlphaMode::Passthrough => 2.0,
        }
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct PostParams {
    bloom_intensity: f32,
    ca_intensity: f32,
    vignette_strength: f32,
    grain_intensity: f32,
    time: f32,
    rms: f32,
    /// 0 = opaque, 1 = luma-derived, 2 = scene-alpha passthrough ([`AlphaMode`]).
    alpha_mode: f32,
    tonemap_mode: f32, // 0 = ACES (house look), 1 = linear passthrough (SuperSplat-faithful)
    grain_rate: f32,   // grain updates per second; <= 0 = every frame (see #1983)
    flash_budget: f32, // flash limiter: flashes allowed per second; 0 = off (#108)
    _pad: [f32; 2],
}

pub struct PostProcessChain {
    pub enabled: bool,
    /// Photosensitivity flash limiter (#108): flashes allowed in any one second,
    /// 0 = off. Applies whether or not post-processing is on.
    pub flash_budget: f32,
    // Quarter-res targets for bloom
    bloom_extract_target: RenderTarget,
    bloom_blur_h_target: RenderTarget,
    bloom_blur_v_target: RenderTarget,
    // Pipelines
    extract_pipeline: RenderPipeline,
    blur_pipeline: RenderPipeline,
    composite_pipeline: RenderPipeline,
    blit_pipeline: RenderPipeline,
    // Bind group layouts
    extract_bgl: BindGroupLayout,
    blur_bgl: BindGroupLayout,
    composite_bgl: BindGroupLayout,
    blit_bgl: BindGroupLayout,
    // Uniform buffers
    bloom_params_buffer: wgpu::Buffer,
    blur_h_params_buffer: wgpu::Buffer,
    blur_v_params_buffer: wgpu::Buffer,
    post_params_buffer: wgpu::Buffer,
    // Flash limiter: its compute pass, persistent state, and the gain the
    // composite reads (a copy of the state's first 16 bytes).
    flash_pipeline: wgpu::ComputePipeline,
    flash_state_bg: BindGroup,
    flash_state_buffer: wgpu::Buffer,
    flash_gain_buffer: wgpu::Buffer,
    // Stored for potential resize rebuilds
    #[allow(dead_code)]
    surface_format: TextureFormat,
    #[allow(dead_code)]
    hdr_format: TextureFormat,
}

impl PostProcessChain {
    pub fn new(
        device: &Device,
        surface_format: TextureFormat,
        hdr_format: TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        // Quarter-res bloom targets
        let bloom_extract_target =
            RenderTarget::new(device, width, height, hdr_format, 0.25, "bloom-extract");
        let bloom_blur_h_target =
            RenderTarget::new(device, width, height, hdr_format, 0.25, "bloom-blur-h");
        let bloom_blur_v_target =
            RenderTarget::new(device, width, height, hdr_format, 0.25, "bloom-blur-v");

        // --- Bloom Extract pipeline ---
        let extract_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("bloom-extract-bgl"),
            entries: &[
                tex_entry(0),
                sampler_entry(1),
                uniform_entry(2, std::mem::size_of::<BloomParams>()),
            ],
        });
        let extract_pipeline = create_fs_pipeline(
            device,
            "bloom-extract",
            &extract_bgl,
            BLOOM_EXTRACT_FS,
            hdr_format,
        );

        // --- Bloom Blur pipeline (same for H and V, direction via uniform) ---
        let blur_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("bloom-blur-bgl"),
            entries: &[
                tex_entry(0),
                sampler_entry(1),
                uniform_entry(2, std::mem::size_of::<BlurParams>()),
            ],
        });
        let blur_pipeline =
            create_fs_pipeline(device, "bloom-blur", &blur_bgl, BLOOM_BLUR_FS, hdr_format);

        // --- Composite pipeline (scene + bloom → surface) ---
        let composite_bgl = composite_bgl(device);
        let composite_pipeline = create_fs_pipeline(
            device,
            "post-composite",
            &composite_bgl,
            POST_COMPOSITE_FS,
            surface_format,
        );

        // --- Simple blit pipeline (fallback when post-processing disabled) ---
        let blit_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("post-blit-bgl"),
            entries: &[tex_entry(0), sampler_entry(1)],
        });
        let blit_pipeline =
            create_fs_pipeline(device, "post-blit", &blit_bgl, BLIT_FS, surface_format);

        // Uniform buffers
        let bloom_params_buffer =
            create_uniform_buffer(device, "bloom-params", std::mem::size_of::<BloomParams>());
        let blur_h_params_buffer =
            create_uniform_buffer(device, "blur-h-params", std::mem::size_of::<BlurParams>());
        let blur_v_params_buffer =
            create_uniform_buffer(device, "blur-v-params", std::mem::size_of::<BlurParams>());
        let post_params_buffer =
            create_uniform_buffer(device, "post-params", std::mem::size_of::<PostParams>());

        // --- Flash limiter (#108) ---
        let flash_state_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("flash-state-bgl"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                visibility: ShaderStages::COMPUTE,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(FLASH_STATE_SIZE),
                },
                count: None,
            }],
        });
        let flash_state_buffer = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("flash-state"),
                contents: bytemuck::cast_slice(&flash_state_init()),
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            },
        );
        let flash_gain_buffer = create_uniform_buffer(device, "flash-gain", 16);
        let flash_state_bg = device.create_bind_group(&BindGroupDescriptor {
            label: Some("flash-state-bg"),
            layout: &flash_state_bgl,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: flash_state_buffer.as_entire_binding(),
            }],
        });
        let flash_pipeline = create_flash_pipeline(device, &composite_bgl, &flash_state_bgl);

        Self {
            enabled: true,
            flash_budget: FLASH_BUDGET_STANDARD,
            bloom_extract_target,
            bloom_blur_h_target,
            bloom_blur_v_target,
            extract_pipeline,
            blur_pipeline,
            composite_pipeline,
            blit_pipeline,
            extract_bgl,
            blur_bgl,
            composite_bgl,
            blit_bgl,
            bloom_params_buffer,
            blur_h_params_buffer,
            blur_v_params_buffer,
            post_params_buffer,
            flash_pipeline,
            flash_state_bg,
            flash_state_buffer,
            flash_gain_buffer,
            surface_format,
            hdr_format,
        }
    }

    pub fn resize(&mut self, device: &Device, width: u32, height: u32) {
        self.bloom_extract_target.resize(device, width, height);
        self.bloom_blur_h_target.resize(device, width, height);
        self.bloom_blur_v_target.resize(device, width, height);
    }

    /// Render the post-processing chain.
    /// `source` is the HDR effect output, renders to `surface_view`.
    pub fn render(
        &self,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        source: &RenderTarget,
        surface_view: &TextureView,
        time: f32,
        rms: f32,
        onset: f32,
        flatness: f32,
        overrides: &PostProcessDef,
        alpha_mode: AlphaMode,
    ) {
        let limit = self.flash_budget > 0.0;
        if !self.enabled && limit {
            // Post off, limiter on: the composite with every effect neutral is
            // the blit below, plus the limiter's gain.
            self.write_post_params(queue, &neutral_post_params(time, self.flash_budget));
            self.limit_and_composite(device, queue, encoder, source, surface_view);
            return;
        }
        if !self.enabled {
            // Simple blit fallback
            let bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("post-blit-bg"),
                layout: &self.blit_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&source.view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(&source.sampler),
                    },
                ],
            });
            run_fullscreen_pass(encoder, "post-blit", &self.blit_pipeline, &bg, surface_view);
            return;
        }

        // --- Update uniforms ---
        let bloom_params = BloomParams {
            threshold: overrides.bloom_threshold,
            soft_knee: 0.3,
            rms,
            _pad: 0.0,
        };
        queue.write_buffer(
            &self.bloom_params_buffer,
            0,
            bytemuck::bytes_of(&bloom_params),
        );

        // Blur directions in texel units
        let blur_w = self.bloom_extract_target.width as f32;
        let blur_h = self.bloom_extract_target.height as f32;
        let blur_h_params = BlurParams {
            direction: [1.0 / blur_w, 0.0],
            _pad: [0.0; 2],
        };
        let blur_v_params = BlurParams {
            direction: [0.0, 1.0 / blur_h],
            _pad: [0.0; 2],
        };
        queue.write_buffer(
            &self.blur_h_params_buffer,
            0,
            bytemuck::bytes_of(&blur_h_params),
        );
        queue.write_buffer(
            &self.blur_v_params_buffer,
            0,
            bytemuck::bytes_of(&blur_v_params),
        );

        let bloom_active = overrides.bloom_enabled;

        let post_params = PostParams {
            bloom_intensity: if bloom_active {
                overrides.bloom_intensity
            } else {
                0.0
            },
            ca_intensity: if overrides.ca_enabled {
                onset * overrides.ca_intensity * 0.03
            } else {
                0.0
            },
            vignette_strength: if overrides.vignette_enabled {
                overrides.vignette
            } else {
                0.0
            },
            grain_intensity: if overrides.grain_enabled {
                flatness * overrides.grain_intensity * 0.08
            } else {
                0.0
            },
            time,
            rms,
            alpha_mode: alpha_mode.as_f32(),
            tonemap_mode: if overrides.tonemap == "linear" {
                1.0
            } else {
                0.0
            },
            grain_rate: if overrides.grain_enabled {
                overrides.grain_rate
            } else {
                0.0
            },
            flash_budget: self.flash_budget,
            _pad: [0.0; 2],
        };
        self.write_post_params(queue, &post_params);

        // --- Bloom passes (skip all 3 when bloom disabled) ---
        if bloom_active {
            // Pass 1: Bloom Extract (HDR scene → quarter-res bright pixels)
            {
                let bg = device.create_bind_group(&BindGroupDescriptor {
                    label: Some("bloom-extract-bg"),
                    layout: &self.extract_bgl,
                    entries: &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::TextureView(&source.view),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Sampler(&source.sampler),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: self.bloom_params_buffer.as_entire_binding(),
                        },
                    ],
                });
                run_fullscreen_pass(
                    encoder,
                    "bloom-extract",
                    &self.extract_pipeline,
                    &bg,
                    &self.bloom_extract_target.view,
                );
            }

            // Pass 2: Horizontal blur
            {
                let bg = device.create_bind_group(&BindGroupDescriptor {
                    label: Some("bloom-blur-h-bg"),
                    layout: &self.blur_bgl,
                    entries: &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::TextureView(&self.bloom_extract_target.view),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Sampler(&self.bloom_extract_target.sampler),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: self.blur_h_params_buffer.as_entire_binding(),
                        },
                    ],
                });
                run_fullscreen_pass(
                    encoder,
                    "bloom-blur-h",
                    &self.blur_pipeline,
                    &bg,
                    &self.bloom_blur_h_target.view,
                );
            }

            // Pass 3: Vertical blur
            {
                let bg = device.create_bind_group(&BindGroupDescriptor {
                    label: Some("bloom-blur-v-bg"),
                    layout: &self.blur_bgl,
                    entries: &[
                        BindGroupEntry {
                            binding: 0,
                            resource: BindingResource::TextureView(&self.bloom_blur_h_target.view),
                        },
                        BindGroupEntry {
                            binding: 1,
                            resource: BindingResource::Sampler(&self.bloom_blur_h_target.sampler),
                        },
                        BindGroupEntry {
                            binding: 2,
                            resource: self.blur_v_params_buffer.as_entire_binding(),
                        },
                    ],
                });
                run_fullscreen_pass(
                    encoder,
                    "bloom-blur-v",
                    &self.blur_pipeline,
                    &bg,
                    &self.bloom_blur_v_target.view,
                );
            }
        }

        // --- Flash limiter, then composite (scene + blurred bloom → surface) ---
        self.limit_and_composite(device, queue, encoder, source, surface_view);
    }

    fn write_post_params(&self, queue: &Queue, params: &PostParams) {
        queue.write_buffer(&self.post_params_buffer, 0, bytemuck::bytes_of(params));
    }

    /// The composite's bind group: scene, bloom, params, and the limiter's gain.
    fn composite_bind_group(&self, device: &Device, source: &RenderTarget) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some("post-composite-bg"),
            layout: &self.composite_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&source.view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&source.sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::TextureView(&self.bloom_blur_v_target.view),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::Sampler(&self.bloom_blur_v_target.sampler),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: self.post_params_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 5,
                    resource: self.flash_gain_buffer.as_entire_binding(),
                },
            ],
        })
    }

    /// Measure the frame and set the limiter's gain (or gain 1 when it is off),
    /// then run the composite into `target`. Once per frame: the limiter's
    /// state advances on every call. Captures reuse the gain through
    /// [`Self::render_composite_to`].
    fn limit_and_composite(
        &self,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        source: &RenderTarget,
        target: &TextureView,
    ) {
        let bg = self.composite_bind_group(device, source);
        if self.flash_budget > 0.0 {
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("flash-limit"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.flash_pipeline);
                pass.set_bind_group(0, &bg, &[]);
                pass.set_bind_group(1, &self.flash_state_bg, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            encoder.copy_buffer_to_buffer(
                &self.flash_state_buffer,
                0,
                &self.flash_gain_buffer,
                0,
                16,
            );
        } else {
            queue.write_buffer(
                &self.flash_gain_buffer,
                0,
                bytemuck::cast_slice(&[1.0f32, 0.0, 0.0, 0.0]),
            );
        }
        run_fullscreen_pass(
            encoder,
            "post-composite",
            &self.composite_pipeline,
            &bg,
            target,
        );
    }

    /// Copy an already-composited target to another view, untouched.
    ///
    /// The display target (#3122) holds the finished frame so the UI can sample
    /// it as a preview; the window still needs that frame blitted onto its
    /// swapchain view. This is that copy — no bloom, no tone map, no second
    /// pass over the post chain, just the pixels that were already produced.
    pub fn blit_target(
        &self,
        device: &Device,
        encoder: &mut CommandEncoder,
        source: &RenderTarget,
        dest_view: &TextureView,
    ) {
        let bg = device.create_bind_group(&BindGroupDescriptor {
            label: Some("display-blit-bg"),
            layout: &self.blit_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&source.view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&source.sampler),
                },
            ],
        });
        run_fullscreen_pass(encoder, "display-blit", &self.blit_pipeline, &bg, dest_view);
    }

    /// Copy an already-composited target onto a surface of a different shape,
    /// letterboxed rather than stretched.
    ///
    /// The second output window (#3122) fills whatever display it was sent to,
    /// and that display's aspect is rarely the render's — a projector at 16:10,
    /// a monitor turned portrait. Stretching to fit would turn every circle in
    /// the composite into an ellipse, so the frame keeps its shape and the
    /// spare edge of the display stays black.
    pub fn blit_target_letterboxed(
        &self,
        device: &Device,
        encoder: &mut CommandEncoder,
        source: &RenderTarget,
        dest_view: &TextureView,
        dest_width: u32,
        dest_height: u32,
    ) {
        let bg = device.create_bind_group(&BindGroupDescriptor {
            label: Some("output-window-blit-bg"),
            layout: &self.blit_bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&source.view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&source.sampler),
                },
            ],
        });

        let (x, y, w, h) = letterbox_rect(source.width, source.height, dest_width, dest_height);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("output-window-blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dest_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Opaque black, not transparent: these are the bars beside
                    // the picture on someone's second screen.
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_pipeline(&self.blit_pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Render the final composite (or blit) to a secondary capture target.
    /// Reuses existing bloom results and uniform buffers — only runs the final pass.
    #[allow(dead_code)]
    pub fn render_composite_to(
        &self,
        device: &Device,
        encoder: &mut CommandEncoder,
        source: &RenderTarget,
        capture_view: &TextureView,
    ) {
        // With the flash limiter on, even a post-off frame goes through the
        // (neutral) composite, so captures carry the same gain as the display.
        if !self.enabled && self.flash_budget <= 0.0 {
            let bg = device.create_bind_group(&BindGroupDescriptor {
                label: Some("output-blit-bg"),
                layout: &self.blit_bgl,
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&source.view),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::Sampler(&source.sampler),
                    },
                ],
            });
            run_fullscreen_pass(
                encoder,
                "output-blit",
                &self.blit_pipeline,
                &bg,
                capture_view,
            );
            return;
        }

        let bg = self.composite_bind_group(device, source);
        run_fullscreen_pass(
            encoder,
            "output-composite",
            &self.composite_pipeline,
            &bg,
            capture_view,
        );
    }
}

// --- Helper functions ---

/// Flashes allowed per second by the Standard limiter: the WCAG 2.x /
/// ITU-R BT.1702 general-flash threshold is "no more than three".
pub const FLASH_BUDGET_STANDARD: f32 = 3.0;
/// The Strict limiter (and Auto under the OS reduced-motion setting, #109).
pub const FLASH_BUDGET_STRICT: f32 = 1.0;

/// PostParams with every effect neutral: the composite then equals the blit
/// (linear tone map is a clamp, alpha passes through) — used when
/// post-processing is off but the flash limiter is on.
fn neutral_post_params(time: f32, flash_budget: f32) -> PostParams {
    PostParams {
        bloom_intensity: 0.0,
        ca_intensity: 0.0,
        vignette_strength: 0.0,
        grain_intensity: 0.0,
        time,
        rms: 0.0,
        alpha_mode: AlphaMode::Passthrough.as_f32(),
        tonemap_mode: 1.0,
        grain_rate: 0.0,
        flash_budget,
        _pad: [0.0; 2],
    }
}

/// The composite's layout. Visible to compute as well: the flash limiter runs
/// `post_color()` over the same bindings.
fn composite_bgl(device: &Device) -> BindGroupLayout {
    let both = |mut e: BindGroupLayoutEntry| {
        e.visibility = ShaderStages::FRAGMENT | ShaderStages::COMPUTE;
        e
    };
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("post-composite-bgl"),
        entries: &[
            both(tex_entry(0)),     // scene
            both(sampler_entry(1)), // scene sampler
            both(tex_entry(2)),     // bloom
            both(sampler_entry(3)), // bloom sampler
            both(uniform_entry(4, std::mem::size_of::<PostParams>())),
            uniform_entry(5, 16), // flash limiter gain
        ],
    })
}

fn create_flash_pipeline(
    device: &Device,
    composite_bgl: &BindGroupLayout,
    state_bgl: &BindGroupLayout,
) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("flash-limit"),
        source: wgpu::ShaderSource::Wgsl(format!("{POST_COMPOSITE_FS}\n{FLASH_LIMIT_CS}").into()),
    });
    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("flash-limit-layout"),
        bind_group_layouts: &[composite_bgl, state_bgl],
        push_constant_ranges: &[],
    });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("flash-limit-pipeline"),
        layout: Some(&layout),
        module: &module,
        entry_point: Some("cs_flash"),
        compilation_options: PipelineCompilationOptions::default(),
        cache: None,
    })
}

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

fn create_uniform_buffer(device: &Device, label: &str, size: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
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

/// Where a `src_w × src_h` picture sits inside a `dst_w × dst_h` surface when
/// it keeps its aspect ratio: `(x, y, width, height)`, centered.
///
/// Every result is inside the destination. A viewport that runs a rounded
/// pixel past the attachment is a validation error, not a cosmetic slip, and
/// zero-sized inputs are possible (a surface mid-resize reports 0).
fn letterbox_rect(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> (f32, f32, f32, f32) {
    let (dw, dh) = (dst_w.max(1) as f32, dst_h.max(1) as f32);
    let src_aspect = src_w.max(1) as f32 / src_h.max(1) as f32;
    let (w, h) = if src_aspect > dw / dh {
        (dw, dw / src_aspect)
    } else {
        (dh * src_aspect, dh)
    };
    let w = w.clamp(1.0, dw);
    let h = h.clamp(1.0, dh);
    (((dw - w) / 2.0).max(0.0), ((dh - h) / 2.0).max(0.0), w, h)
}

fn run_fullscreen_pass(
    encoder: &mut CommandEncoder,
    label: &str,
    pipeline: &RenderPipeline,
    bind_group: &BindGroup,
    target: &TextureView,
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

    /// A stretch-to-fit blit would pass none of these but the matching case:
    /// the output window (#3122) fills a display whose shape is rarely the
    /// render's, and the whole point is that the picture keeps its own.
    #[test]
    fn letterbox_keeps_aspect_and_stays_in_bounds() {
        // 16:9 onto a portrait 2160×3840 monitor: full width, bars above and
        // below, and the picture is still 16:9.
        let (x, y, w, h) = letterbox_rect(1920, 1080, 2160, 3840);
        assert_eq!((x, w), (0.0, 2160.0));
        assert!((w / h - 16.0 / 9.0).abs() < 1e-3, "aspect kept: {w}×{h}");
        assert!((y - (3840.0 - h) / 2.0).abs() < 0.01, "centered: y={y}");

        // 16:9 onto 16:9: the whole surface, no bars.
        assert_eq!(
            letterbox_rect(1920, 1080, 3840, 2160),
            (0.0, 0.0, 3840.0, 2160.0)
        );

        // Square onto a wide surface: pillarboxed, full height.
        let (x, y, w, h) = letterbox_rect(1024, 1024, 3840, 2160);
        assert_eq!((y, h), (0.0, 2160.0));
        assert!(
            (w - 2160.0).abs() < 0.01 && (x - 840.0).abs() < 0.01,
            "{x} {w}"
        );

        // Degenerate sizes (a surface mid-resize reports 0) stay in bounds.
        for (sw, sh, dw, dh) in [(0, 0, 1920, 1080), (1920, 1080, 0, 0), (1, 4000, 640, 480)] {
            let (x, y, w, h) = letterbox_rect(sw, sh, dw, dh);
            assert!(w >= 1.0 && h >= 1.0, "non-empty: {w}×{h}");
            assert!(
                x + w <= dw.max(1) as f32 + 0.01 && y + h <= dh.max(1) as f32 + 0.01,
                "inside {dw}×{dh}: {x},{y} {w}×{h}"
            );
        }
    }

    /// The WGSL mirror of `PostParams` is maintained by hand and uniform structs
    /// need a 16-byte multiple. `grain_rate` took the struct from 32 to 48 with
    /// three pad words; if that stops being true the shader's copy must follow.
    /// Both builtin post shaders through naga on CPU: the composite as the
    /// render pipeline builds it, and the limiter's compute module (the
    /// composite source plus flash_limit.wgsl). The GPU probes are ignored in CI.
    #[test]
    fn post_shaders_validate_on_cpu() {
        crate::trama::effect::validate_wgsl(&format!(
            "{FULLSCREEN_TRIANGLE_VS_WITH_UV}\n{POST_COMPOSITE_FS}"
        ))
        .unwrap();
        crate::trama::effect::validate_wgsl(&format!("{POST_COMPOSITE_FS}\n{FLASH_LIMIT_CS}"))
            .unwrap();
    }

    #[test]
    fn flash_state_init_matches_the_shader_struct() {
        assert_eq!(
            (flash_state_init().len() * 4) as u64,
            FLASH_STATE_SIZE,
            "FlashState in flash_limit.wgsl is 16 + 32 * 32 bytes"
        );
    }

    #[test]
    fn post_params_stay_forty_eight_bytes() {
        assert_eq!(std::mem::size_of::<PostParams>(), 48);
    }

    const PROBE_DIM: u32 = 64;

    /// Render one post-composite pass over a flat mid-grey scene and read it
    /// back. Rgba8Unorm rather than the production surface format: one row is
    /// exactly the 256-byte copy alignment and the readback needs no decode.
    fn probe_composite(device: &Device, queue: &Queue, params: PostParams) -> Vec<u8> {
        // Flat opaque mid-grey — the historical probe scene.
        let dim = PROBE_DIM;
        let mut scene = vec![128u8; (dim * dim * 4) as usize];
        for px in scene.chunks_exact_mut(4) {
            px[3] = 255;
        }
        probe_composite_scene(device, queue, params, &scene)
    }

    fn probe_composite_scene(
        device: &Device,
        queue: &Queue,
        params: PostParams,
        scene_px: &[u8],
    ) -> Vec<u8> {
        let dim = PROBE_DIM;
        let format = TextureFormat::Rgba8Unorm;

        let make_input = |label: &str, px: &[u8]| {
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
                px,
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

        let (_scene_tex, scene_view) = make_input("probe-scene", scene_px);
        let black = vec![0u8; (dim * dim * 4) as usize];
        let (_bloom_tex, bloom_view) = make_input("probe-bloom", &black);

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

        let bgl = composite_bgl(device);
        let pipeline = create_fs_pipeline(device, "probe-post", &bgl, POST_COMPOSITE_FS, format);

        let ubo =
            create_uniform_buffer(device, "probe-post-ubo", std::mem::size_of::<PostParams>());
        queue.write_buffer(&ubo, 0, bytemuck::bytes_of(&params));
        let gain = create_uniform_buffer(device, "probe-flash-gain", 16);
        queue.write_buffer(&gain, 0, bytemuck::cast_slice(&[1.0f32, 0.0, 0.0, 0.0]));

        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("probe-post-bg"),
            layout: &bgl,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&scene_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::TextureView(&bloom_view),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::Sampler(&sampler),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: ubo.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 5,
                    resource: gain.as_entire_binding(),
                },
            ],
        });

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("probe-post-readback"),
            size: (dim * dim * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        run_fullscreen_pass(
            &mut encoder,
            "probe-post-pass",
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

    /// Grain only, no bloom/CA/vignette, linear tonemap so the noise is not
    /// compressed away. `grain_intensity` here is the final shader multiplier —
    /// the CPU side folds flatness and the 0.08 scale in before this point.
    /// P0.2 acceptance (docs/alpha-audit.md): a known premultiplied pattern —
    /// left half 50% grey `(64,64,64,128)`, right half fully transparent — through
    /// the composite in Passthrough with every post effect off and linear tonemap
    /// must read back within ±1/255 on every byte, ALPHA INCLUDED. This is the
    /// surgical "where does alpha die" gate; the sRGB full-chain version lives in
    /// headless::scene_renderer::tests::overlay_scene_alpha_reaches_readback.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn passthrough_preserves_known_premultiplied_pattern() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();

        let dim = PROBE_DIM;
        let mut scene = vec![0u8; (dim * dim * 4) as usize];
        for y in 0..dim {
            for x in 0..dim / 2 {
                let i = ((y * dim + x) * 4) as usize;
                scene[i] = 64;
                scene[i + 1] = 64;
                scene[i + 2] = 64;
                scene[i + 3] = 128;
            }
        }
        let params = PostParams {
            bloom_intensity: 0.0,
            ca_intensity: 0.0,
            vignette_strength: 0.0,
            grain_intensity: 0.0,
            time: 0.0,
            rms: 0.0,
            alpha_mode: 2.0, // passthrough
            tonemap_mode: 1.0,
            grain_rate: 0.0,
            flash_budget: 0.0,
            _pad: [0.0; 2],
        };
        let out = probe_composite_scene(&device, &queue, params, &scene);
        for (i, (&got, &want)) in out.iter().zip(scene.iter()).enumerate() {
            assert!(
                (got as i16 - want as i16).abs() <= 1,
                "byte {i} ({}): got {got}, want {want} ±1",
                ["r", "g", "b", "a"][i % 4]
            );
        }
    }

    fn grain_only(time: f32, grain_rate: f32) -> PostParams {
        PostParams {
            bloom_intensity: 0.0,
            ca_intensity: 0.0,
            vignette_strength: 0.0,
            grain_intensity: 0.5,
            time,
            rms: 0.0,
            alpha_mode: 0.0,
            tonemap_mode: 1.0,
            grain_rate,
            flash_budget: 0.0,
            _pad: [0.0; 2],
        }
    }

    /// #1983: the grain must hold its pattern for the whole of a tick and
    /// change across a tick boundary. That is the entire mitigation — a
    /// display that repeats a frame can then only repeat a pattern the grain
    /// was already going to hold, instead of freezing a boiling field.
    ///
    /// Run: cargo test -p fosfora-app -- --ignored grain_holds_within_a_tick
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn grain_holds_within_a_tick() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();

        // At 24 Hz a tick is 41.7 ms. 1.000 and 1.030 share tick 24; 1.050 is
        // tick 25. All three are more than a 60 Hz frame apart, so without
        // quantization every one of them would differ.
        let a = probe_composite(&device, &queue, grain_only(1.000, 24.0));
        let b = probe_composite(&device, &queue, grain_only(1.030, 24.0));
        let c = probe_composite(&device, &queue, grain_only(1.050, 24.0));

        assert_eq!(a, b, "grain changed inside a single 24 Hz tick");
        assert_ne!(b, c, "grain did not change across a 24 Hz tick boundary");

        // The probe is only meaningful if there is grain to see at all.
        let flat = probe_composite(&device, &queue, {
            let mut p = grain_only(1.000, 24.0);
            p.grain_intensity = 0.0;
            p
        });
        assert_ne!(
            a, flat,
            "no grain in the probe — the assertions prove nothing"
        );
    }

    /// The opt-out has to be exact: `grain_rate = 0` restores the every-frame
    /// grain this field was added to slow down, so a preset that sets it gets
    /// the pre-#1983 look and not an approximation of it.
    ///
    /// Run: cargo test -p fosfora-app -- --ignored grain_rate_zero_updates_every_frame
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn grain_rate_zero_updates_every_frame() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();

        // Same two times that share a tick at 24 Hz: with the rate off they
        // must differ, which is what "every frame" means.
        let a = probe_composite(&device, &queue, grain_only(1.000, 0.0));
        let b = probe_composite(&device, &queue, grain_only(1.030, 0.0));
        assert_ne!(a, b, "grain_rate = 0 should still update every frame");
    }

    // ---- Flash limiter (#108) ----

    /// Every post effect off, linear tone map: the output is the scene, so a
    /// test controls the displayed luminance directly.
    fn plain_post() -> PostProcessDef {
        PostProcessDef {
            bloom_enabled: false,
            ca_enabled: false,
            vignette_enabled: false,
            grain_enabled: false,
            tonemap: "linear".into(),
            ..PostProcessDef::default()
        }
    }

    /// Run a real chain for `levels.len()` frames at 60 fps over a uniform grey
    /// scene of `levels[i]`, returning each output frame's mean luminance.
    fn run_flash_chain(budget: f32, post_enabled: bool, levels: &[f32]) -> Vec<f32> {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();
        // 64 px: one row is exactly the 256-byte copy alignment.
        let dim = 64u32;
        let out_format = TextureFormat::Rgba8Unorm;
        let hdr = TextureFormat::Rgba16Float;
        let mut chain = PostProcessChain::new(&device, out_format, hdr, dim, dim);
        chain.flash_budget = budget;
        chain.enabled = post_enabled;
        let source = RenderTarget::new(&device, dim, dim, hdr, 1.0, "flash-probe-src");
        let out = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("flash-probe-out"),
            size: wgpu::Extent3d {
                width: dim,
                height: dim,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: out_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let out_view = out.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flash-probe-readback"),
            size: u64::from(dim * dim * 4),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let post = plain_post();

        let mut lum = Vec::with_capacity(levels.len());
        for (i, &level) in levels.iter().enumerate() {
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let v = f64::from(level);
                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("flash-probe-clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &source.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: v,
                                g: v,
                                b: v,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            }
            chain.render(
                &device,
                &queue,
                &mut encoder,
                &source,
                &out_view,
                i as f32 / 60.0,
                0.0,
                0.0,
                0.0,
                &post,
                AlphaMode::Opaque,
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
            let px = slice.get_mapped_range().to_vec();
            readback.unmap();
            let sum: f64 = px
                .chunks_exact(4)
                .map(|p| {
                    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
                })
                .sum();
            lum.push((sum / 255.0 / f64::from(dim * dim)) as f32);
        }
        lum
    }

    /// Independent CPU count of general flashes by the WCAG rule: rises of
    /// 0.1 relative luminance from a darker state below 0.8, each ending a
    /// fall of 0.1. Returns the most rises in any one-second window.
    fn max_flashes_per_second(lum: &[f32]) -> usize {
        let mut rises = Vec::new();
        let (mut rising, mut extreme) = (false, lum[0]);
        for (i, &l) in lum.iter().enumerate() {
            if rising {
                extreme = extreme.max(l);
                if extreme - l >= 0.1 {
                    rising = false;
                    extreme = l;
                }
            } else {
                extreme = extreme.min(l);
                if l - extreme >= 0.1 && extreme < 0.8 {
                    rises.push(i);
                    rising = true;
                    extreme = l;
                }
            }
        }
        rises
            .iter()
            .map(|&start| {
                rises
                    .iter()
                    .filter(|&&r| r >= start && r < start + 60)
                    .count()
            })
            .max()
            .unwrap_or(0)
    }

    /// A 10 Hz full-frame black/white strobe, three seconds at 60 fps.
    fn strobe_10hz() -> Vec<f32> {
        (0..180)
            .map(|i| if (i / 3) % 2 == 0 { 0.0 } else { 1.0 })
            .collect()
    }

    /// The limiter throttles a 10 Hz strobe to the budget, on screen and with
    /// post-processing off, and lets through as many flashes as it allows.
    ///
    /// Run: cargo test -p fosfora-app -- --ignored flash_limiter
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn flash_limiter_caps_a_strobe_at_the_budget() {
        let strobe = strobe_10hz();
        let unlimited = max_flashes_per_second(&run_flash_chain(0.0, true, &strobe));
        assert!(
            unlimited >= 9,
            "sanity: the raw strobe is ~10/s, got {unlimited}"
        );

        for (budget, post_on) in [(3.0, true), (1.0, true), (3.0, false)] {
            let out = run_flash_chain(budget, post_on, &strobe);
            let got = max_flashes_per_second(&out);
            assert_eq!(
                got, budget as usize,
                "budget {budget} (post {post_on}): {got} flashes in one second"
            );
        }
    }

    /// Content that never flashes passes through untouched: a single cut to
    /// white that holds, and a 1 Hz pulse (one flash a second, under budget).
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn flash_limiter_leaves_non_flashing_content_alone() {
        let cut: Vec<f32> = (0..120).map(|i| if i < 30 { 0.0 } else { 1.0 }).collect();
        let pulse: Vec<f32> = (0..180)
            .map(|i| if i % 60 < 30 { 0.1 } else { 0.9 })
            .collect();
        for levels in [cut, pulse] {
            let limited = run_flash_chain(FLASH_BUDGET_STANDARD, true, &levels);
            let off = run_flash_chain(0.0, true, &levels);
            for (i, (a, b)) in limited.iter().zip(&off).enumerate() {
                assert!(
                    (a - b).abs() < 1.5 / 255.0,
                    "frame {i}: limited {a}, off {b}"
                );
            }
        }
    }
}
