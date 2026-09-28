//! World-space particle rendering for the XR build.
//!
//! The desktop renderer draws screen-space NDC with one instance per alive
//! particle and no camera. A headset needs particles that live in meters in 3D,
//! drawn once per eye through that eye's view and projection, depth tested
//! against other geometry. This module holds the public camera/target types and
//! the lazily built GPU state behind [`ParticleSystem::render_world`]; nothing
//! here exists until that entry is first called, so desktop never builds it.
//!
//! [`ParticleSystem::render_world`]: super::ParticleSystem::render_world

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::{
    BindGroup, BindGroupLayout, BlendComponent, BlendFactor, BlendOperation, BlendState, Device,
    PipelineLayout, Queue, RenderPipeline, ShaderModule, TextureFormat,
};

/// One eye's camera for [`ParticleSystem::render_world`].
///
/// [`ParticleSystem::render_world`]: super::ParticleSystem::render_world
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WorldCamera {
    /// World to view transform (the inverse of the eye pose).
    pub view: Mat4,
    /// View to clip projection, wgpu depth convention (z in 0..1).
    pub proj: Mat4,
    /// Effect anchor in world space: added to every particle position, which a
    /// world-layout sim writes relative to it.
    pub anchor: Vec3,
}

/// Where [`ParticleSystem::render_world`] draws.
///
/// [`ParticleSystem::render_world`]: super::ParticleSystem::render_world
#[derive(Debug, Clone, Copy)]
pub struct WorldTarget<'a> {
    pub color: &'a wgpu::TextureView,
    pub color_format: TextureFormat,
    /// Depth to test against (`Less`, loaded, never written). `None` draws
    /// without a depth test.
    pub depth: Option<&'a wgpu::TextureView>,
    /// Format of `depth`. Ignored when `depth` is `None`.
    pub depth_format: TextureFormat,
}

/// GPU side of [`WorldCamera`], matching `WorldCamera` in
/// `particle_render_world.wgsl`: 160 bytes.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub(super) struct WorldCameraUniforms {
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    anchor: [f32; 4],
    gain: f32,
    _pad: [f32; 3],
}

impl WorldCameraUniforms {
    pub(super) fn new(camera: &WorldCamera, gain: f32) -> Self {
        Self {
            view: camera.view.to_cols_array_2d(),
            proj: camera.proj.to_cols_array_2d(),
            anchor: camera.anchor.extend(0.0).to_array(),
            gain,
            _pad: [0.0; 3],
        }
    }
}

/// Camera slots in the uniform ring. Each `render_world` call writes its own
/// slot, so up to this many calls between two queue submissions (both eyes, a
/// spectator view, ...) each see their own camera. A single slot would not do:
/// `Queue::write_buffer` lands at the next submit, before every pass in it, so
/// two eyes recorded into one encoder would both draw with the last camera.
const CAMERA_SLOTS: u64 = 16;

/// Render pipeline cache key: color format, depth format (`None` = no depth
/// test), alpha blend (else additive).
type PipelineKey = (TextureFormat, Option<TextureFormat>, bool);

/// Lazily built state of the world-space path.
pub(super) struct WorldRender {
    shader: ShaderModule,
    layout: PipelineLayout,
    pipelines: HashMap<PipelineKey, RenderPipeline>,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: BindGroup,
    camera_stride: u64,
    camera_cursor: u64,
}

impl WorldRender {
    /// `render_bgl` is the 2D renderer's group-0 layout (SoA buffers, render
    /// uniforms, alive indices), reused unchanged as group 0 here. Group 1
    /// is the camera ring plus `counter_buffer`, whose alive count bounds
    /// the direct draw in the vertex shader.
    pub(super) fn new(
        device: &Device,
        render_bgl: &BindGroupLayout,
        counter_buffer: &wgpu::Buffer,
    ) -> Self {
        let size = std::mem::size_of::<WorldCameraUniforms>() as u64;
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment);
        let camera_stride = size.div_ceil(align) * align;

        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle-world-camera-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(size),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
            ],
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particle-world-camera"),
            size: camera_stride * CAMERA_SLOTS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("particle-world-camera-bg"),
            layout: &camera_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &camera_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(size),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: counter_buffer.as_entire_binding(),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particle-render-world"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../../assets/shaders/builtin/particle_render_world.wgsl")
                    .into(),
            ),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particle-render-world-layout"),
            bind_group_layouts: &[render_bgl, &camera_bgl],
            push_constant_ranges: &[],
        });

        Self {
            shader,
            layout,
            pipelines: HashMap::new(),
            camera_buffer,
            camera_bind_group,
            camera_stride,
            camera_cursor: 0,
        }
    }

    /// Write `uniforms` into the next ring slot; returns its dynamic offset.
    pub(super) fn write_camera(&mut self, queue: &Queue, uniforms: &WorldCameraUniforms) -> u32 {
        let offset = self.camera_cursor * self.camera_stride;
        self.camera_cursor = (self.camera_cursor + 1) % CAMERA_SLOTS;
        queue.write_buffer(&self.camera_buffer, offset, bytemuck::bytes_of(uniforms));
        offset as u32
    }

    pub(super) fn camera_bind_group(&self) -> &BindGroup {
        &self.camera_bind_group
    }

    /// Build the pipeline for `key` if this is its first use.
    pub(super) fn ensure_pipeline(&mut self, device: &Device, key: PipelineKey) {
        if !self.pipelines.contains_key(&key) {
            let pipeline = create_render_pipeline(device, &self.shader, &self.layout, key);
            self.pipelines.insert(key, pipeline);
        }
    }

    /// The pipeline for `key`; `ensure_pipeline` must have built it.
    pub(super) fn pipeline(&self, key: PipelineKey) -> &RenderPipeline {
        &self.pipelines[&key]
    }
}

/// Same blend states as the 2D renderer's additive and alpha pipelines.
fn blend_state(alpha: bool) -> BlendState {
    let dst = if alpha {
        BlendFactor::OneMinusSrcAlpha
    } else {
        BlendFactor::One
    };
    BlendState {
        color: BlendComponent {
            src_factor: BlendFactor::SrcAlpha,
            dst_factor: dst,
            operation: BlendOperation::Add,
        },
        alpha: BlendComponent {
            src_factor: BlendFactor::One,
            dst_factor: dst,
            operation: BlendOperation::Add,
        },
    }
}

fn create_render_pipeline(
    device: &Device,
    shader: &ShaderModule,
    layout: &PipelineLayout,
    (color_format, depth_format, alpha): PipelineKey,
) -> RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(if alpha {
            "particle-render-world-alpha"
        } else {
            "particle-render-world-additive"
        }),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(blend_state(alpha)),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            // Billboards always face the eye, but a mirrored projection would
            // flip their winding; culling would buy nothing.
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
            format,
            // Test against the scene, never occlude other sprites: they are
            // blended, not sorted.
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Rust uniform must match the WGSL struct byte for byte; naga reports
    /// the WGSL side's size, so this runs without a GPU.
    #[test]
    fn camera_uniform_matches_wgsl_layout() {
        let src = include_str!("../../../../../assets/shaders/builtin/particle_render_world.wgsl");
        crate::trama::effect::validate_wgsl(src).expect("world render shader validates");
        let module = naga::front::wgsl::parse_str(src).unwrap();
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("WorldCamera"))
            .expect("WorldCamera struct");
        let naga::TypeInner::Struct { span, .. } = ty.inner else {
            panic!("WorldCamera is not a struct");
        };
        assert_eq!(span as usize, std::mem::size_of::<WorldCameraUniforms>());
        assert_eq!(std::mem::size_of::<WorldCameraUniforms>(), 160);
    }

    /// The draw is bounded by the alive count in the vertex shader, so the
    /// counters buffer must be bound where `WorldRender::new` puts it.
    #[test]
    fn world_shader_reads_counters_at_group_1_binding_1() {
        let src = include_str!("../../../../../assets/shaders/builtin/particle_render_world.wgsl");
        let module = naga::front::wgsl::parse_str(src).unwrap();
        let (_, counters) = module
            .global_variables
            .iter()
            .find(|(_, v)| v.name.as_deref() == Some("counters"))
            .expect("counters binding");
        let binding = counters
            .binding
            .as_ref()
            .expect("counters is a resource binding");
        assert_eq!((binding.group, binding.binding), (1, 1));
        assert_eq!(
            counters.space,
            naga::AddressSpace::Storage {
                access: naga::StorageAccess::LOAD
            }
        );
    }

    /// `sample_flow_field_3d` compiles as part of the particle library at
    /// baseline WebGPU capabilities, called the way a world-layout sim would.
    #[test]
    fn particle_lib_flow_3d_validates() {
        let noise = include_str!("../../../../../assets/shaders/lib/noise.wgsl");
        let palette = include_str!("../../../../../assets/shaders/lib/palette.wgsl");
        let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
        let sim = "
@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    var p = read_particle(gid.x);
    p.vel_size = vec4f(sample_flow_field_3d(p.pos_life.xyz, 1.0), p.vel_size.w);
    write_particle(gid.x, p);
}";
        crate::trama::effect::validate_wgsl(&format!("{noise}\n{palette}\n{plib}\n{sim}"))
            .expect("particle_lib with sample_flow_field_3d validates");
    }

    // ---- Flux XR World (C3b): the first world-layout sim -------------------

    const XR_FLUX_PRESET: &str = include_str!("../../../../../assets/xr/effects/flux_xr_world.pfx");
    const XR_FLUX_SIM: &str = include_str!("../../../../../assets/xr/shaders/flux_xr_sim.wgsl");

    /// What `EffectLoader::load_compute_source` puts in front of a compute
    /// shader: every shared library, then the particle library.
    fn compute_preamble() -> String {
        [
            include_str!("../../../../../assets/shaders/lib/noise.wgsl"),
            include_str!("../../../../../assets/shaders/lib/palette.wgsl"),
            include_str!("../../../../../assets/shaders/lib/sdf.wgsl"),
            include_str!("../../../../../assets/shaders/lib/tonemap.wgsl"),
            include_str!("../../../../../assets/shaders/lib/chronoflow.wgsl"),
            include_str!("../../../../../assets/shaders/lib/overlay_lib.wgsl"),
            include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl"),
        ]
        .join("\n")
    }

    fn xr_flux_preset() -> crate::effect::format::PfxEffect {
        serde_json::from_str(XR_FLUX_PRESET).expect("flux_xr_world.pfx parses")
    }

    /// The XR Flux sim compiles behind the loader's full compute preamble at
    /// baseline WebGPU capabilities, so its `xr_` helpers collide with no
    /// library name.
    #[test]
    fn flux_xr_sim_validates() {
        crate::trama::effect::validate_wgsl(&format!("{}\n{XR_FLUX_SIM}", compute_preamble()))
            .expect("flux_xr_sim.wgsl validates");
    }

    /// The aux block the sim reads ends where the XR app's upload ends: one
    /// head row plus `ObstacleSet` (2 header rows, 64 spheres, 3 x 32 box
    /// rows) = 163 rows, which `crates/fosfora-xr/src/scene.rs` asserts on its
    /// side at compile time.
    #[test]
    fn flux_xr_sim_aux_layout_is_contiguous() {
        let src = format!("{}\n{XR_FLUX_SIM}", compute_preamble());
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let get = |name: &str| -> u32 {
            let (_, c) = module
                .constants
                .iter()
                .find(|(_, c)| c.name.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("const {name}"));
            match module.global_expressions[c.init] {
                naga::Expression::Literal(naga::Literal::U32(v)) => v,
                ref e => panic!("const {name} is {e:?}"),
            }
        };
        let (spheres, boxes) = (get("XR_MAX_SPHERES"), get("XR_MAX_BOXES"));
        assert_eq!(get("XR_AUX_HEAD"), 0);
        assert_eq!(get("XR_AUX_HEADER"), 1);
        assert_eq!(get("XR_AUX_SPHERES"), 3);
        assert_eq!(get("XR_AUX_BOX_CENTER"), 3 + spheres);
        assert_eq!(get("XR_AUX_BOX_ROT"), get("XR_AUX_BOX_CENTER") + boxes);
        assert_eq!(get("XR_AUX_BOX_HALF"), get("XR_AUX_BOX_ROT") + boxes);
        assert_eq!(get("XR_AUX_BOX_HALF") + boxes, 163);
        assert_eq!((spheres, boxes), (64, 32));
    }

    /// The preset is hidden, points at the XR sim through the loader's
    /// resolution (`assets/shaders/` + `compute_shader`, the same relative
    /// path in a checkout and in the APK), keeps the aux buffer the sim's
    /// per-frame inputs ride in, and keeps everything the world path cannot
    /// draw off: the compute raster, velocity feedback, trails, sprites.
    #[test]
    fn flux_xr_preset_is_a_hidden_world_variant() {
        let pfx = xr_flux_preset();
        assert_eq!(pfx.name, "Flux XR World");
        assert!(
            pfx.hidden,
            "an XR preset must stay out of the desktop library"
        );
        let pd = pfx.particles.as_ref().expect("particles");
        let sim = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/shaders")
            .join(&pd.compute_shader);
        let on_disk = std::fs::read_to_string(&sim)
            .unwrap_or_else(|e| panic!("{} does not resolve: {e}", sim.display()));
        assert_eq!(on_disk, XR_FLUX_SIM);
        assert_eq!(pd.render_mode, "billboard");
        assert!(!pd.velocity_field);
        assert!(pd.trail_length < 2);
        assert!(pd.sprite.is_none());
        assert!(!pd.morph, "morph interleaves aux at a 4x stride");
        assert_ne!(
            pd.blend, "alpha",
            "additive: the eye gets premultiplied color"
        );
        assert!(
            pd.composite_decay.is_none(),
            "no feedback background, gain 1"
        );
        assert!(pd.flow_field);
        // aux[0..163] carries the XR inputs; the buffer holds max_count rows.
        assert!(pd.max_count >= 163);
        assert!(pfx.passes.iter().all(|p| !p.feedback));
        assert!(pfx.postprocess.as_ref().is_some_and(|p| !p.enabled));
    }

    /// C3b: the XR app's world-mode sequence end to end, headless. The preset
    /// loads through `SceneRenderer` with its layer disabled (so the scene
    /// renderer runs no 2D frame), the sim is dispatched after each
    /// `SceneRenderer::step`, and one frame renders through `render_world`
    /// without validation errors. A box obstacle filling x < 0, fed through
    /// the aux block as the XR app does, must keep every visible sprite in
    /// the right half of a view looking down -Z from the anchor.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn flux_xr_world_steps_and_renders() {
        use crate::audio::AudioFrame;
        use crate::audio::analyzer::{SPECTROGRAM_MELS, SPECTRUM_BINS};
        use crate::audio::hop::HopOutput;
        use crate::gpu::particle::types::ParticleAux;
        use crate::gpu::test_gpu::{gpu_guard, test_gpu};
        use crate::headless::scene_renderer::SceneRenderer;

        const DIM: u32 = 256;
        const FPS: u32 = 60;
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        // As the scene-renderer probes do: `assets_dir()` is CWD-relative.
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }
        // The one-cue scene the XR app writes for `mode world`.
        let dir = std::env::temp_dir().join("fosfora_flux_xr_world");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Cue.json"),
            r#"{"layers":[{"effect_name":"Flux XR World"}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("_scene.json"),
            r#"{"version":1,"name":"XR Flux XR World","loop_mode":false,"advance_mode":"Manual",
               "cues":[{"preset_name":"Cue","transition":"Cut","label":"Flux XR World"}]}"#,
        )
        .unwrap();

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut sr = SceneRenderer::new(
            (*device).clone(),
            (*queue).clone(),
            64,
            64,
            crate::settings::ParticleQuality::High,
            dir.clone(),
        )
        .expect("renderer");
        // The desktop library does not scan assets/xr/; the APK stages the
        // preset among the effects. Scaled down for a software rasterizer.
        let mut pfx = xr_flux_preset();
        let pd = pfx.particles.as_mut().unwrap();
        pd.max_count = 20_000;
        pd.emit_rate = 60_000.0;
        sr.effect_loader.effects.push(pfx);
        sr.install_scene(crate::headless::load::load_scene_dir(&dir).expect("scene loads"));
        sr.start();
        assert!(sr.warnings.is_empty(), "warnings: {:?}", sr.warnings);
        assert_eq!(sr.layer_stack.layers.len(), 1);
        sr.layer_stack.layers[0].enabled = false;

        // Head at the anchor with the 0.3 m near fade; one box filling x < 0.
        let mut aux = vec![ParticleAux { home: [0.0; 4] }; 163];
        aux[0].home = [0.0, 0.0, 0.0, 0.3];
        aux[1].home = [f32::from_bits(0), f32::from_bits(1), 0.4, 0.005];
        aux[2].home = [0.0, 0.3, 0.0, 0.0];
        aux[67].home = [-2.0, 0.0, 0.0, 0.0];
        aux[99].home = [0.0, 0.0, 0.0, 1.0];
        aux[131].home = [2.0, 3.0, 3.0, 0.0];

        // The eye at the anchor (where the aux head sits), looking down -Z.
        let anchor = Vec3::new(0.0, 1.0, 0.0);
        let camera = WorldCamera {
            view: Mat4::from_translation(-anchor),
            proj: Mat4::perspective_rh(90f32.to_radians(), 1.0, 0.05, 100.0),
            anchor,
        };
        let texture = |format, usage, label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: DIM,
                    height: DIM,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
                view_formats: &[],
            })
        };
        let color = texture(
            TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureUsages::COPY_SRC,
            "flux-xr-color",
        );
        let depth = texture(
            TextureFormat::Depth32Float,
            wgpu::TextureUsages::empty(),
            "flux-xr-depth",
        );
        let (color_view, depth_view) = (
            color.create_view(&Default::default()),
            depth.create_view(&Default::default()),
        );

        let frames = 40u32;
        for frame in 0..frames {
            let ts = f64::from(frame) / f64::from(FPS);
            let mut f = crate::headless::loop_driver::synth_features(frame, FPS, 120.0);
            (f.rms, f.bass, f.mid, f.onset) = (0.6, 0.7, 0.5, 0.4);
            let hop = HopOutput {
                frame: AudioFrame {
                    features: f,
                    spectrum: vec![0.3; SPECTRUM_BINS].into(),
                    mel: vec![0.3; SPECTROGRAM_MELS].into(),
                    dmfcc: [0.0; 13],
                    timestamp: ts,
                    phase_frozen: false,
                    bar_duration: 2.0,
                    beat_time: None,
                    section_boundary: None,
                },
                beat_fired: f.beat > 0.5,
                downbeat_fired: false,
                drop_fired: false,
                pre_norm: f,
            };
            let wave = vec![0.0; crate::gpu::audio_textures::WAVEFORM_PEEK];
            let ps = sr.layer_stack.layers[0]
                .as_effect_mut()
                .and_then(|e| e.pass_executor.particle_system.as_mut())
                .expect("particle system");
            ps.update_aux_in_place(&queue, &aux);
            sr.step(ts, 1.0 / FPS as f32, &hop, &wave, false);

            let ps = sr.layer_stack.layers[0]
                .as_effect_mut()
                .and_then(|e| e.pass_executor.particle_system.as_mut())
                .unwrap();
            let mut enc = device.create_command_encoder(&Default::default());
            ps.poll_counter_readback();
            ps.dispatch(&mut enc, &queue);
            if frame + 1 == frames {
                // The eye pass: clear color to alpha 0 (passthrough) and
                // depth to far, then the world pass loads both.
                drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("flux-xr-eye"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                }));
                ps.render_world(
                    &device,
                    &mut enc,
                    &queue,
                    WorldTarget {
                        color: &color_view,
                        color_format: TextureFormat::Rgba8UnormSrgb,
                        depth: Some(&depth_view),
                        depth_format: TextureFormat::Depth32Float,
                    },
                    &camera,
                    wgpu::LoadOp::Load,
                );
            }
            queue.submit([enc.finish()]);
        }
        let wait = || {
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .expect("poll");
        };
        wait();
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");

        // The alive count the XR log prints keeps moving with the sim
        // dispatched outside the scene renderer's frame.
        let ps = sr.layer_stack.layers[0]
            .as_effect_mut()
            .and_then(|e| e.pass_executor.particle_system.as_mut())
            .unwrap();
        ps.request_counter_readback();
        wait();
        ps.poll_counter_readback();
        assert!(
            (1..=20_000).contains(&ps.alive_count),
            "alive count {}",
            ps.alive_count
        );

        let bpr = DIM * 4;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flux-xr-readback"),
            size: u64::from(bpr * DIM),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(DIM),
                },
            },
            color.size(),
        );
        queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        wait();
        let rgba = buf.slice(..).get_mapped_range().to_vec();
        let lit = |px: &[u8]| px[0].max(px[1]).max(px[2]) > 0;
        let (mut left, mut right, mut opaque) = (0usize, 0usize, 0usize);
        for (i, px) in rgba.chunks_exact(4).enumerate() {
            let x = i as u32 % DIM;
            if lit(px) {
                // A sprite pushed onto the box face (x = margin) spans a
                // centimeter or so, a few pixels either side of the center
                // column at the nearest distance the near fade leaves visible.
                if x < DIM / 2 - 8 {
                    left += 1;
                } else if x >= DIM / 2 {
                    right += 1;
                }
            }
            opaque += usize::from(px[3] == 255);
        }
        assert!(right > 200, "the cloud drew almost nothing: {right} lit px");
        assert_eq!(left, 0, "{left} lit px inside the x < 0 obstacle");
        assert!(
            opaque < rgba.len() / 4 / 2,
            "{opaque} opaque px: the sprites should leave the room visible"
        );
    }

    #[test]
    fn uniforms_carry_camera_columns_and_anchor() {
        let camera = WorldCamera {
            view: Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)),
            proj: Mat4::IDENTITY,
            anchor: Vec3::new(0.0, 1.5, -1.5),
        };
        let u = WorldCameraUniforms::new(&camera, 0.5);
        // Column-major, as WGSL's mat4x4f: translation is the fourth column.
        assert_eq!(u.view[3], [1.0, 2.0, 3.0, 1.0]);
        assert_eq!(u.anchor, [0.0, 1.5, -1.5, 0.0]);
        assert_eq!(u.gain, 0.5);
    }
}
