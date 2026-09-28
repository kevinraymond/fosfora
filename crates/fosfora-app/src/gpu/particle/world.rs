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
pub(super) type PipelineKey = (TextureFormat, Option<TextureFormat>, bool);

/// One world draw, prepared by [`ParticleSystem::prepare_world`] before a
/// render pass and issued by [`ParticleSystem::draw_world`] inside it: the
/// pipeline the pass's attachments need and this call's camera slot.
///
/// [`ParticleSystem::prepare_world`]: super::ParticleSystem::prepare_world
/// [`ParticleSystem::draw_world`]: super::ParticleSystem::draw_world
#[derive(Clone, Copy, Debug)]
pub struct WorldDraw {
    pub(super) key: PipelineKey,
    pub(super) camera_offset: u32,
}

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

    // ---- Flux XR Room (board #3317): the room's surfaces as emitters --------

    const XR_FLUX_ROOM_PRESET: &str =
        include_str!("../../../../../assets/xr/effects/flux_xr_world_room.pfx");
    const XR_FLUX_COARSE_PRESET: &str =
        include_str!("../../../../../assets/xr/effects/flux_xr_world_coarse.pfx");

    fn xr_flux_room_preset() -> crate::effect::format::PfxEffect {
        serde_json::from_str(XR_FLUX_ROOM_PRESET).expect("flux_xr_world_room.pfx parses")
    }

    /// Flux XR Room is Flux XR World plus one input, `surface_emit` = 1 in
    /// the slot the sim reads (`XR_SURFACE_PARAM`), on the same sim, count
    /// and sizes. The other Flux XR presets have fewer inputs than that slot,
    /// so they read 0 there and keep the volume emitter unchanged.
    #[test]
    fn flux_xr_room_preset_turns_on_surface_emission() {
        use crate::params::types::ParamDef;
        let room = xr_flux_room_preset();
        let world = xr_flux_preset();
        assert_eq!(room.name, "Flux XR Room");
        assert!(
            room.hidden,
            "an XR preset must stay out of the desktop library"
        );
        let slot = sim_u32_const(XR_FLUX_SIM, "XR_SURFACE_PARAM") as usize;
        assert_eq!(room.inputs.len(), slot + 1);
        assert_eq!(room.inputs[..slot], world.inputs[..]);
        assert!(
            matches!(&room.inputs[slot], ParamDef::Float { name, default, .. }
                if name == "surface_emit" && *default > 0.5),
            "{:?}",
            room.inputs[slot]
        );
        let coarse: crate::effect::format::PfxEffect =
            serde_json::from_str(XR_FLUX_COARSE_PRESET).expect("coarse parses");
        for other in [&world, &coarse] {
            assert!(
                other.inputs.len() <= slot,
                "{} would read param({slot}) as its own input",
                other.name
            );
        }
        let (rp, wp) = (
            room.particles.as_ref().unwrap(),
            world.particles.as_ref().unwrap(),
        );
        assert_eq!(rp.compute_shader, wp.compute_shader);
        assert_eq!(rp.max_count, wp.max_count);
        assert_eq!(rp.emitter.radius, wp.emitter.radius);
        assert_eq!(
            (rp.initial_size, rp.size_end, rp.emit_rate, rp.burst_on_beat),
            (wp.initial_size, wp.size_end, wp.emit_rate, wp.burst_on_beat)
        );
    }

    /// The room tests' table: a 1.0 x 0.7 m plane 0.5 m below, 0.4 m ahead
    /// of and 0.3 m right of the anchor, in a runtime table's pose (local X
    /// -> world X, local Y -> world -Z, local +Z -> world +Y), so its top
    /// face is y = -0.48 over x 0.3 +- 0.5, z -0.4 +- 0.35.
    const ROOM_TABLE: Vec3 = Vec3::new(0.3, -0.5, -0.4);
    const ROOM_TABLE_HALF: Vec3 = Vec3::new(0.5, 0.35, 0.02);

    /// The aux block with the room table as the only obstacle, surface kind
    /// table, emitter weight 1; the head at the anchor, settle drift 0.3 m/s.
    fn room_table_aux() -> Vec<crate::gpu::particle::types::ParticleAux> {
        use crate::gpu::particle::types::ParticleAux;
        let rot = glam::Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let (c, h) = (ROOM_TABLE, ROOM_TABLE_HALF);
        let mut aux = vec![ParticleAux { home: [0.0; 4] }; 163];
        aux[0].home = [0.0, 0.0, 0.0, 0.15];
        aux[1].home = [f32::from_bits(0), f32::from_bits(1), 0.4, 0.005];
        aux[2].home = [0.0, 0.4, 0.3, 0.0];
        aux[67].home = [c.x, c.y, c.z, 1.0];
        aux[99].home = rot.to_array();
        aux[131].home = [h.x, h.y, h.z, 1.0];
        aux
    }

    /// `pfx` (scaled to `count`, 60K/s) stepped at 60 fps with `aux` every
    /// frame, and the alive positions after each frame count in `capture`.
    fn flux_room_run(
        mut pfx: crate::effect::format::PfxEffect,
        count: u32,
        aux: &[crate::gpu::particle::types::ParticleAux],
        capture: &[u32],
    ) -> Vec<Vec<Vec3>> {
        use crate::gpu::test_gpu::test_gpu;
        use crate::headless::scene_renderer::SceneRenderer;
        const FPS: u32 = 60;
        let (device, queue) = test_gpu();
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }
        let dir = std::env::temp_dir().join(format!(
            "fosfora_flux_xr_room_{}",
            pfx.name.replace(' ', "_")
        ));
        write_world_scene(&dir, &pfx.name);
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
        let pd = pfx.particles.as_mut().unwrap();
        pd.max_count = count;
        pd.emit_rate = 60_000.0;
        sr.effect_loader.effects.push(pfx);
        sr.install_scene(crate::headless::load::load_scene_dir(&dir).expect("scene loads"));
        sr.start();
        assert!(sr.warnings.is_empty(), "warnings: {:?}", sr.warnings);
        sr.layer_stack.layers[0].enabled = false;
        let frames = capture.iter().copied().max().unwrap_or(0);
        let mut out = Vec::new();
        for frame in 0..frames {
            let ts = f64::from(frame) / f64::from(FPS);
            let hop = murmur_hop(frame, FPS);
            let wave = vec![0.0; crate::gpu::audio_textures::WAVEFORM_PEEK];
            particles(&mut sr).update_aux_in_place(&queue, aux);
            sr.step(ts, 1.0 / FPS as f32, &hop, &wave, false);
            let ps = particles(&mut sr);
            let mut enc = device.create_command_encoder(&Default::default());
            ps.poll_counter_readback();
            ps.dispatch(&mut enc, &queue);
            queue.submit([enc.finish()]);
            if capture.contains(&(frame + 1)) {
                out.push(alive(&read_slots(
                    &device,
                    &queue,
                    particles(&mut sr),
                    count,
                )));
            }
        }
        wait(&device);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
        out
    }

    /// The Flux XR Room sim, headless: with one table in the aux block (kind
    /// table, weight 1, a runtime table's pose: local +Z up), every particle
    /// is born on the table's top face, and a third of a second later each
    /// one is still on it or has slid off its edge and is falling beside it
    /// (the settle drift holds it to the top, the flow slides it); none
    /// appears anywhere else in the volume.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn flux_xr_room_emits_on_a_table() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let captures = flux_room_run(xr_flux_room_preset(), 20_000, &room_table_aux(), &[5, 20]);
        let (center, half_x, half_z, half_t) = (
            ROOM_TABLE,
            ROOM_TABLE_HALF.x,
            ROOM_TABLE_HALF.y,
            ROOM_TABLE_HALF.z,
        );
        let top = center.y + half_t;
        // How far outside the footprint a point is (0 inside).
        let beyond = |p: Vec3| {
            ((p.x - center.x).abs() - half_x)
                .max((p.z - center.z).abs() - half_z)
                .max(0.0)
        };
        // After 5 frames: born on the top face (1 cm above it, held there
        // by the drift and the collide step), inside the footprint.
        let early = &captures[0];
        assert!(early.len() > 200, "only {} particles born", early.len());
        const SLACK_M: f32 = 0.02;
        let off: Vec<Vec3> = early
            .iter()
            .copied()
            .filter(|&p| (p.y - top).abs() > SLACK_M || beyond(p) > SLACK_M)
            .collect();
        assert!(
            off.is_empty(),
            "{} of {} new particles off the table top (y {top}), first {:?}",
            off.len(),
            early.len(),
            off.first()
        );
        // After a third of a second: still on the top, or slid off its
        // edge and falling beside it; nothing above it or out in the volume.
        let late = &captures[1];
        assert!(
            late.len() > early.len(),
            "{} -> {}",
            early.len(),
            late.len()
        );
        let stray: Vec<Vec3> = late
            .iter()
            .copied()
            .filter(|&p| {
                let on_top = (p.y - top).abs() <= SLACK_M && beyond(p) <= SLACK_M;
                let falling_off = p.y < top && beyond(p) <= 0.15;
                !(on_top || falling_off)
            })
            .collect();
        let on_top = late
            .iter()
            .filter(|&&p| (p.y - top).abs() <= SLACK_M && beyond(p) <= SLACK_M)
            .count();
        assert!(
            stray.is_empty(),
            "{} of {} particles neither on the table nor falling off it, first {:?}",
            stray.len(),
            late.len(),
            stray.first()
        );
        assert!(
            on_top * 10 > late.len() * 9,
            "{on_top} of {} on the top",
            late.len()
        );
    }

    /// The surface lanes change nothing for a preset without `surface_emit`:
    /// Flux XR World, fed the same weighted table, still spawns through the
    /// whole volume.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn flux_xr_world_ignores_the_surface_lanes() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let born = flux_room_run(xr_flux_preset(), 20_000, &room_table_aux(), &[5])
            .pop()
            .expect("one capture");
        let above = born.iter().filter(|p| p.y > 0.0).count();
        assert!(
            above * 4 > born.len(),
            "{above} of {} particles in the upper half of the volume",
            born.len()
        );
    }

    // ---- Murmur XR World (C3b): boids over the 3D spatial hash --------------

    const XR_MURMUR_PRESET: &str =
        include_str!("../../../../../assets/xr/effects/murmur_xr_world.pfx");
    const XR_MURMUR_SIM: &str = include_str!("../../../../../assets/xr/shaders/murmur_xr_sim.wgsl");
    const MURMUR_PRESET: &str = include_str!("../../../../../assets/effects/murmur.pfx");

    fn xr_murmur_preset() -> crate::effect::format::PfxEffect {
        serde_json::from_str(XR_MURMUR_PRESET).expect("murmur_xr_world.pfx parses")
    }

    /// A module-scope `u32` constant of a sim compiled behind the preamble.
    fn sim_u32_const(sim: &str, name: &str) -> u32 {
        let src = format!("{}\n{sim}", compute_preamble());
        let module = naga::front::wgsl::parse_str(&src).unwrap();
        let (_, c) = module
            .constants
            .iter()
            .find(|(_, c)| c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("const {name}"));
        match module.global_expressions[c.init] {
            naga::Expression::Literal(naga::Literal::U32(v)) => v,
            ref e => panic!("const {name} is {e:?}"),
        }
    }

    /// The XR Murmur sim compiles behind the loader's full compute preamble at
    /// baseline WebGPU capabilities, `array<f32, K>` sized by its tuning const.
    #[test]
    fn murmur_xr_sim_validates() {
        crate::trama::effect::validate_wgsl(&format!("{}\n{XR_MURMUR_SIM}", compute_preamble()))
            .expect("murmur_xr_sim.wgsl validates");
        assert_eq!(sim_u32_const(XR_MURMUR_SIM, "K"), 7);
        assert_eq!(sim_u32_const(XR_MURMUR_SIM, "MAX_PER_CELL"), 16);
    }

    /// Murmur reads the XR inputs where Flux does: the same 163-row block the
    /// app uploads (`flux_xr_sim_aux_layout_is_contiguous` pins Flux's side).
    #[test]
    fn murmur_xr_sim_aux_layout_matches_flux() {
        for name in [
            "XR_AUX_HEAD",
            "XR_AUX_HEADER",
            "XR_MAX_SPHERES",
            "XR_MAX_BOXES",
            "XR_AUX_SPHERES",
            "XR_AUX_BOX_CENTER",
            "XR_AUX_BOX_ROT",
            "XR_AUX_BOX_HALF",
        ] {
            assert_eq!(
                sim_u32_const(XR_MURMUR_SIM, name),
                sim_u32_const(XR_FLUX_SIM, name),
                "{name}"
            );
        }
    }

    /// Murmur's per-hand behavior lanes (board #3314) start where the obstacle
    /// block ends, one shared row and three per hand, and the XR app's
    /// upload ends with them (`crates/fosfora-xr/src/scene.rs` asserts 170
    /// rows on its side).
    #[test]
    fn murmur_xr_hand_lanes_follow_the_obstacle_block() {
        let get = |name: &str| sim_u32_const(XR_MURMUR_SIM, name);
        assert_eq!(
            get("XR_AUX_HANDS"),
            get("XR_AUX_BOX_HALF") + get("XR_MAX_BOXES")
        );
        assert_eq!(get("XR_AUX_HANDS"), 163);
        assert_eq!(get("XR_AUX_HAND_ROWS"), 3);
        assert_eq!(
            get("XR_AUX_END"),
            get("XR_AUX_HANDS") + 1 + 2 * get("XR_AUX_HAND_ROWS")
        );
        assert_eq!(get("XR_AUX_END"), MURMUR_AUX_ROWS as u32);
        let pd = xr_murmur_preset().particles.expect("particles");
        assert!(pd.max_count as usize >= MURMUR_AUX_ROWS);
    }

    /// The preset is a hidden world variant of desktop Murmur: the loader
    /// resolves its sim, it asks for the 3D hash over the 3 m cube and the
    /// alpha pipeline, it keeps desktop's inputs and audio mappings, and it
    /// drops everything the world path cannot draw (compute raster, velocity
    /// field, the velocity and history passes, post).
    #[test]
    fn murmur_xr_preset_is_a_hidden_world_variant() {
        let pfx = xr_murmur_preset();
        assert_eq!(pfx.name, "Murmur XR World");
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
        assert_eq!(on_disk, XR_MURMUR_SIM);

        assert!(pd.interaction && pd.interaction_3d);
        assert_eq!(
            pd.spatial_hash_mode(),
            Some(super::super::spatial_hash::SpatialHashMode::Volume)
        );
        assert_eq!(pd.blend, "alpha", "dark silhouettes over passthrough");
        assert_eq!(pd.render_mode, "billboard");
        assert!(!pd.velocity_field);
        assert!(pd.trail_length < 2);
        assert!(pd.sprite.is_none());
        assert!(!pd.morph, "morph interleaves aux at a 4x stride");
        assert!(
            pd.composite_decay.is_none(),
            "no feedback background, gain 1"
        );
        assert_eq!(
            pd.max_count, 40_000,
            "a flock that holds 72 Hz (MEASURED.md)"
        );
        assert_eq!(pd.max_scaled_count, 0);
        assert!((pd.emitter.radius - 1.5).abs() < 1e-6, "the 3 m cube");
        assert!((pd.initial_size - 0.012).abs() < 1e-6);
        assert!((pd.size_end - 0.012).abs() < 1e-6);
        assert!((pd.lifetime - 15.0).abs() < 1e-6);
        let fill_s = pd.max_count as f32 / pd.emit_rate;
        assert!((3.5..=4.5).contains(&fill_s), "fills in {fill_s} s");

        assert_eq!(
            pfx.passes
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["background"]
        );
        assert!(pfx.passes.iter().all(|p| !p.feedback));
        assert!(pfx.postprocess.as_ref().is_some_and(|p| !p.enabled));

        let xr: serde_json::Value = serde_json::from_str(XR_MURMUR_PRESET).unwrap();
        let desktop: serde_json::Value = serde_json::from_str(MURMUR_PRESET).unwrap();
        for key in ["inputs", "audio_mappings"] {
            assert_eq!(xr[key], desktop[key], "{key} match desktop Murmur");
        }
        let (xp, dp) = (&xr["particles"], &desktop["particles"]);
        for key in ["opacity_curve", "color_gradient"] {
            assert_eq!(xp[key], dp[key], "particles.{key} match desktop Murmur");
        }
    }

    /// Scene files for a one-cue scene showing `effect`, as the XR app writes
    /// for `mode world`.
    fn write_world_scene(dir: &std::path::Path, effect: &str) {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("Cue.json"),
            serde_json::json!({ "layers": [{ "effect_name": effect }] }).to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.join("_scene.json"),
            serde_json::json!({
                "version": 1,
                "name": format!("XR {effect}"),
                "loop_mode": false,
                "advance_mode": "Manual",
                "cues": [{ "preset_name": "Cue", "transition": "Cut", "label": effect }]
            })
            .to_string(),
        )
        .unwrap();
    }

    /// A `DIM` x `DIM` eye color target (sRGB, as the Quest swapchain) and
    /// depth target.
    fn eye_targets(device: &wgpu::Device, dim: u32) -> (wgpu::Texture, wgpu::Texture) {
        let texture = |format, usage, label| {
            device.create_texture(&wgpu::TextureDescriptor {
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
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
                view_formats: &[],
            })
        };
        (
            texture(
                TextureFormat::Rgba8UnormSrgb,
                wgpu::TextureUsages::COPY_SRC,
                "xr-eye-color",
            ),
            texture(
                TextureFormat::Depth32Float,
                wgpu::TextureUsages::empty(),
                "xr-eye-depth",
            ),
        )
    }

    /// The eye pass as the XR app records it: color cleared to alpha 0
    /// (passthrough), depth to far, then the world draw loads both.
    fn clear_eye(
        enc: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("xr-eye-clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        }));
    }

    fn wait(device: &wgpu::Device) {
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("poll");
    }

    /// The RGBA8 bytes of a `dim` x `dim` color target, undecoded.
    fn read_rgba(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color: &wgpu::Texture,
        dim: u32,
    ) -> Vec<u8> {
        let bpr = dim * 4;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("xr-eye-readback"),
            size: u64::from(bpr * dim),
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
                    rows_per_image: Some(dim),
                },
            },
            color.size(),
        );
        queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        wait(device);
        buf.slice(..).get_mapped_range().to_vec()
    }

    /// An sRGB-encoded byte back to linear.
    fn srgb_to_linear(b: u8) -> f32 {
        let s = f32::from(b) / 255.0;
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }

    /// C3b Murmur: the XR app's world-mode sequence end to end, headless, as
    /// `flux_xr_world_steps_and_renders` runs it. `interaction_3d` builds the
    /// 3D hash through the scene renderer, the flock steps, and one frame
    /// renders through `render_world`'s alpha pipeline without validation
    /// errors. A box obstacle filling x < 0, fed through the aux block as the
    /// XR app does, must keep every bird in the right half of a view from
    /// behind the roost; every bird drawn is dark, and the room stays visible.
    /// The Murmur XR preset in a headless scene renderer, scaled down for a
    /// software rasterizer, its layer disabled so a test drives the sim
    /// itself (`particles(&mut sr)`). Opens a validation error scope the
    /// caller pops.
    fn murmur_renderer(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        count: u32,
        emit_rate: f32,
    ) -> crate::headless::scene_renderer::SceneRenderer {
        use crate::headless::scene_renderer::SceneRenderer;
        // As the scene-renderer probes do: `assets_dir()` is CWD-relative.
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }
        let dir = std::env::temp_dir().join(format!("fosfora_murmur_xr_world_{count}"));
        write_world_scene(&dir, "Murmur XR World");

        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut sr = SceneRenderer::new(
            device.clone(),
            queue.clone(),
            64,
            64,
            crate::settings::ParticleQuality::High,
            dir.clone(),
        )
        .expect("renderer");
        // The desktop library does not scan assets/xr/; the APK stages the
        // preset among the effects.
        let mut pfx = xr_murmur_preset();
        let pd = pfx.particles.as_mut().unwrap();
        pd.max_count = count;
        pd.emit_rate = emit_rate;
        sr.effect_loader.effects.push(pfx);
        sr.install_scene(crate::headless::load::load_scene_dir(&dir).expect("scene loads"));
        sr.start();
        assert!(sr.warnings.is_empty(), "warnings: {:?}", sr.warnings);
        assert_eq!(sr.layer_stack.layers.len(), 1);
        sr.layer_stack.layers[0].enabled = false;
        let (_, grid_d) = particles(&mut sr)
            .spatial_hash_dims()
            .expect("interaction_3d builds a spatial hash");
        assert!(grid_d > 1, "3D grid edge {grid_d}");
        sr
    }

    fn particles(
        sr: &mut crate::headless::scene_renderer::SceneRenderer,
    ) -> &mut crate::gpu::particle::ParticleSystem {
        sr.layer_stack.layers[0]
            .as_effect_mut()
            .and_then(|e| e.pass_executor.particle_system.as_mut())
            .expect("particle system")
    }

    /// One hop of loud, bass-heavy synthetic audio at 120 BPM.
    fn murmur_hop(frame: u32, fps: u32) -> crate::audio::hop::HopOutput {
        use crate::audio::AudioFrame;
        use crate::audio::analyzer::{SPECTROGRAM_MELS, SPECTRUM_BINS};
        use crate::audio::hop::HopOutput;
        let ts = f64::from(frame) / f64::from(fps);
        let mut f = crate::headless::loop_driver::synth_features(frame, fps, 120.0);
        (f.rms, f.bass, f.mid, f.onset) = (0.6, 0.7, 0.5, 0.4);
        HopOutput {
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
        }
    }

    /// Rows of the XR inputs Murmur reads: the head, the obstacle block and
    /// the per-hand lanes (`XR_AUX_END`).
    const MURMUR_AUX_ROWS: usize = 170;

    /// The Murmur tests' aux block: the wearer's head at `eye` with the
    /// world default 0.15 m near fade, the obstacle header (no spheres, one
    /// box, restitution 0.4, margin 5 mm, kick 0.4), the box filling x < 0.
    /// The per-hand lanes are all zero: the behavior from before they
    /// existed.
    fn murmur_aux(eye: Vec3) -> Vec<crate::gpu::particle::types::ParticleAux> {
        use crate::gpu::particle::types::ParticleAux;
        let mut aux = vec![ParticleAux { home: [0.0; 4] }; MURMUR_AUX_ROWS];
        aux[0].home = [eye.x, eye.y, eye.z, 0.15];
        aux[1].home = [f32::from_bits(0), f32::from_bits(1), 0.4, 0.005];
        aux[2].home = [0.0, 0.4, 0.0, 0.0];
        aux[67].home = [-2.0, 0.0, 0.0, 0.0];
        aux[99].home = [0.0, 0.0, 0.0, 1.0];
        aux[131].home = [2.0, 3.0, 3.0, 0.0];
        aux
    }

    /// `ROOST_OFFSET_M` in the sim: where the flock lives, relative to the
    /// anchor.
    const ROOST: Vec3 = Vec3::new(0.0, 0.4, -1.0);

    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn murmur_xr_world_steps_and_renders() {
        use crate::gpu::test_gpu::{gpu_guard, test_gpu};

        const DIM: u32 = 256;
        const FPS: u32 = 60;
        const COUNT: u32 = 20_000;
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let mut sr = murmur_renderer(&device, &queue, COUNT, 60_000.0);

        // Eye 1.2 m behind the roost, looking at it; one box filling x < 0.
        // The head is a hawk within 0.6 m, so from 1.2 m it does not touch
        // the flock.
        let roost = ROOST;
        let eye = roost + Vec3::new(0.0, 0.1, 1.2);
        let aux = murmur_aux(eye);

        let anchor = Vec3::new(0.0, 1.0, 0.0);
        let camera = WorldCamera {
            view: Mat4::look_at_rh(anchor + eye, anchor + roost, Vec3::Y),
            proj: Mat4::perspective_rh(90f32.to_radians(), 1.0, 0.05, 100.0),
            anchor,
        };
        let (color, depth) = eye_targets(&device, DIM);
        let (color_view, depth_view) = (
            color.create_view(&Default::default()),
            depth.create_view(&Default::default()),
        );

        let frames = 60u32;
        for frame in 0..frames {
            let ts = f64::from(frame) / f64::from(FPS);
            let hop = murmur_hop(frame, FPS);
            let wave = vec![0.0; crate::gpu::audio_textures::WAVEFORM_PEEK];
            particles(&mut sr).update_aux_in_place(&queue, &aux);
            sr.step(ts, 1.0 / FPS as f32, &hop, &wave, false);

            let ps = particles(&mut sr);
            let mut enc = device.create_command_encoder(&Default::default());
            ps.poll_counter_readback();
            ps.dispatch(&mut enc, &queue);
            if frame + 1 == frames {
                clear_eye(&mut enc, &color_view, &depth_view);
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
        wait(&device);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");

        let ps = particles(&mut sr);
        ps.request_counter_readback();
        wait(&device);
        ps.poll_counter_readback();
        assert!(
            (1..=COUNT).contains(&ps.alive_count),
            "alive count {}",
            ps.alive_count
        );

        let rgba = read_rgba(&device, &queue, &color, DIM);
        let (mut left, mut right, mut opaque) = (0usize, 0usize, 0usize);
        let mut brightest = 0.0f32;
        for (i, px) in rgba.chunks_exact(4).enumerate() {
            let x = i as u32 % DIM;
            if px[3] > 0 {
                // A bird pushed onto the box face (x = margin) spans up to
                // ~10 px either side of the center column at the nearest
                // distance the near fade leaves visible.
                if x < DIM / 2 - 12 {
                    left += 1;
                } else if x >= DIM / 2 {
                    right += 1;
                }
                brightest = px[..3]
                    .iter()
                    .map(|&b| srgb_to_linear(b))
                    .fold(brightest, f32::max);
            }
            opaque += usize::from(px[3] == 255);
        }
        assert!(right > 200, "the flock drew almost nothing: {right} px");
        assert_eq!(left, 0, "{left} px inside the x < 0 obstacle");
        // The palette tops out at 0.10 and the rim adds a few hundredths.
        assert!(brightest < 0.15, "a bird reads bright: {brightest}");
        assert!(
            opaque < rgba.len() / 4 / 2,
            "{opaque} opaque px: the birds should leave the room visible"
        );
    }

    /// Alive bird positions after `frames` frames of the sim with `aux`.
    fn murmur_positions(
        count: u32,
        frames: u32,
        aux: &[crate::gpu::particle::types::ParticleAux],
    ) -> Vec<Vec3> {
        alive(
            &murmur_run(count, frames, |_| aux.to_vec(), &[frames])
                .pop()
                .expect("one capture"),
        )
    }

    /// The alive birds of a capture.
    fn alive(slots: &[Option<Vec3>]) -> Vec<Vec3> {
        slots.iter().flatten().copied().collect()
    }

    /// `frames` frames of the sim at 60 fps with frame `f`'s aux block from
    /// `aux_at(f)`, and every slot's position (`None` when dead) after each
    /// frame count in `capture` (ascending).
    fn murmur_run(
        count: u32,
        frames: u32,
        aux_at: impl Fn(u32) -> Vec<crate::gpu::particle::types::ParticleAux>,
        capture: &[u32],
    ) -> Vec<Vec<Option<Vec3>>> {
        use crate::gpu::test_gpu::test_gpu;
        const FPS: u32 = 60;
        let (device, queue) = test_gpu();
        let mut sr = murmur_renderer(&device, &queue, count, 60_000.0);
        let mut out = Vec::new();
        for frame in 0..frames {
            let ts = f64::from(frame) / f64::from(FPS);
            let hop = murmur_hop(frame, FPS);
            let wave = vec![0.0; crate::gpu::audio_textures::WAVEFORM_PEEK];
            particles(&mut sr).update_aux_in_place(&queue, &aux_at(frame));
            sr.step(ts, 1.0 / FPS as f32, &hop, &wave, false);
            let ps = particles(&mut sr);
            let mut enc = device.create_command_encoder(&Default::default());
            ps.poll_counter_readback();
            ps.dispatch(&mut enc, &queue);
            queue.submit([enc.finish()]);
            if capture.contains(&(frame + 1)) {
                out.push(read_slots(&device, &queue, particles(&mut sr), count));
            }
        }
        wait(&device);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
        out
    }

    /// Every slot's position in the buffer the last dispatch wrote, `None`
    /// for a dead one.
    fn read_slots(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        ps: &crate::gpu::particle::ParticleSystem,
        count: u32,
    ) -> Vec<Option<Vec3>> {
        let bytes = u64::from(count) * 16;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("murmur-positions"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(ps.pos_life_written(), 0, &staging, 0, bytes);
        queue.submit([enc.finish()]);
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        wait(device);
        let out: Vec<Option<Vec3>> =
            bytemuck::cast_slice::<u8, [f32; 4]>(&staging.slice(..).get_mapped_range())
                .iter()
                .map(|p| (p[3] > 0.0).then(|| Vec3::new(p[0], p[1], p[2])))
                .collect();
        staging.unmap();
        out
    }

    /// The headline behavior of the port: a hand at the roost is a hawk. Five
    /// joint spheres (a palm and four fingertips, 2 cm) sit where the flock
    /// lives; after two seconds of sim the birds within 0.25 m of the palm
    /// number a small fraction of the count without the hand (3723 -> 512
    /// after one second on lavapipe; emission keeps seeding birds beside
    /// the survivors, so the roost never fully empties), and the flock as a
    /// whole is still there.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn murmur_xr_flock_flees_a_hand() {
        use crate::gpu::test_gpu::gpu_guard;
        const COUNT: u32 = 20_000;
        let _guard = gpu_guard();
        let eye = ROOST + Vec3::new(0.0, 0.1, 1.2);
        let palm = ROOST;
        let near = |pts: &[Vec3]| pts.iter().filter(|p| p.distance(palm) < 0.25).count();

        let without = murmur_positions(COUNT, 120, &murmur_aux(eye));
        let mut aux = murmur_aux(eye);
        aux[1].home[0] = f32::from_bits(5);
        for (k, offset) in [
            Vec3::ZERO,
            Vec3::new(0.08, 0.0, 0.0),
            Vec3::new(-0.08, 0.0, 0.0),
            Vec3::new(0.0, 0.08, 0.0),
            Vec3::new(0.0, -0.08, 0.0),
        ]
        .iter()
        .enumerate()
        {
            let c = palm + *offset;
            aux[3 + k].home = [c.x, c.y, c.z, 0.02];
        }
        let with = murmur_positions(COUNT, 120, &aux);

        let (n0, n1) = (near(&without), near(&with));
        assert!(
            n0 > 200,
            "no flock at the roost without a hand: {n0} birds within 0.25 m"
        );
        assert!(
            n1 * 5 < n0,
            "the hand should clear the roost: {n1} birds within 0.25 m against {n0} without it"
        );
        assert!(
            with.len() * 2 > without.len(),
            "the flock should survive the hand: {} alive against {}",
            with.len(),
            without.len()
        );
    }

    /// The same hand as `murmur_xr_flock_flees_a_hand`, calmed through the
    /// XR hand-calm lane (aux[2].w = 1, the app's hand scare 0): without the
    /// scare, five 2 cm joint spheres (no pad, no kick) barely dent the roost.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn murmur_xr_calm_hand_does_not_clear_the_roost() {
        use crate::gpu::test_gpu::gpu_guard;
        const COUNT: u32 = 20_000;
        let _guard = gpu_guard();
        let eye = ROOST + Vec3::new(0.0, 0.1, 1.2);
        let palm = ROOST;
        let near = |pts: &[Vec3]| pts.iter().filter(|p| p.distance(palm) < 0.25).count();

        let without = murmur_positions(COUNT, 120, &murmur_aux(eye));
        let mut aux = murmur_aux(eye);
        aux[1].home[0] = f32::from_bits(5);
        aux[2].home[3] = 1.0;
        for (k, offset) in [
            Vec3::ZERO,
            Vec3::new(0.08, 0.0, 0.0),
            Vec3::new(-0.08, 0.0, 0.0),
            Vec3::new(0.0, 0.08, 0.0),
            Vec3::new(0.0, -0.08, 0.0),
        ]
        .iter()
        .enumerate()
        {
            let c = palm + *offset;
            aux[3 + k].home = [c.x, c.y, c.z, 0.02];
        }
        let calm = murmur_positions(COUNT, 120, &aux);

        let (n0, n1) = (near(&without), near(&calm));
        eprintln!("birds within 0.25 m of the palm: {n0} without a hand, {n1} with a calm hand");
        assert!(
            n1 * 2 > n0,
            "a calm hand should leave most of the roost: {n1} birds within 0.25 m against {n0} without it"
        );
    }

    /// The Murmur tests' hand: a palm and four fingertips (2 cm spheres)
    /// around `palm`, as the left hand's spheres when `left`.
    fn murmur_hand(aux: &mut [crate::gpu::particle::types::ParticleAux], palm: Vec3, left: bool) {
        aux[1].home[0] = f32::from_bits(5);
        aux[163].home[0] = f32::from_bits(if left { 5 } else { 0 });
        for (k, offset) in [
            Vec3::ZERO,
            Vec3::new(0.08, 0.0, 0.0),
            Vec3::new(-0.08, 0.0, 0.0),
            Vec3::new(0.0, 0.08, 0.0),
            Vec3::new(0.0, -0.08, 0.0),
        ]
        .iter()
        .enumerate()
        {
            let c = palm + *offset;
            aux[3 + k].home = [c.x, c.y, c.z, 0.02];
        }
    }

    /// Hand `h`'s lane rows: behavior, hold center and radius, hold
    /// velocity.
    fn murmur_lanes(h: usize) -> [usize; 3] {
        let base = 164 + 3 * h;
        [base, base + 1, base + 2]
    }

    /// Board #3314: the per-hand calm lane calms only the hand its spheres
    /// belong to. The flee test's hand, as the left hand's spheres: calmed
    /// through the left lane it leaves the roost; the right lane calmed
    /// instead, the left hand is still a hawk and clears it.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn murmur_xr_hand_lanes_calm_only_their_own_hand() {
        use crate::gpu::test_gpu::gpu_guard;
        const COUNT: u32 = 20_000;
        let _guard = gpu_guard();
        let eye = ROOST + Vec3::new(0.0, 0.1, 1.2);
        let palm = ROOST;
        let near = |pts: &[Vec3]| pts.iter().filter(|p| p.distance(palm) < 0.25).count();
        let calmed = |h: usize| {
            let mut aux = murmur_aux(eye);
            murmur_hand(&mut aux, palm, true);
            aux[murmur_lanes(h)[0]].home[0] = 1.0;
            near(&murmur_positions(COUNT, 120, &aux))
        };
        let without = near(&murmur_positions(COUNT, 120, &murmur_aux(eye)));
        let (left, right) = (calmed(0), calmed(1));
        eprintln!(
            "birds within 0.25 m of a left hand: {without} without it, {left} left calmed, {right} right calmed"
        );
        assert!(
            left * 2 > without,
            "the calmed left hand should leave the roost: {left} against {without}"
        );
        assert!(
            right * 5 < without,
            "the right lane must not calm the left hand: {right} against {without}"
        );
    }

    /// Board #3314, the piece #3311 found missing: a hold carries birds. A
    /// calm hold (radius 0.25 m) opens at the roost after a second, latches
    /// the birds inside it, holds still a quarter second, moves 0.8 m to the
    /// side in 0.4 s (2 m/s, a far hand's speed) with its velocity in the
    /// lane, and stays there half a second. The birds within its radius at
    /// the end are the group it brought: against the few there with the
    /// lanes zero (today's hands, no hold), and against the birds it latched
    /// (a part of the flock, not the flock). Without its velocity the group
    /// trails the moving hand on the spring alone: fewer are inside the
    /// radius on arrival. Also reports the hold released on arrival (its
    /// birds fly on at cruise speed and drift home slowly).
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn murmur_xr_hold_carries_a_group() {
        use crate::gpu::test_gpu::gpu_guard;
        const COUNT: u32 = 20_000;
        const RADIUS: f32 = 0.25;
        let _guard = gpu_guard();
        let eye = ROOST + Vec3::new(0.0, 0.1, 1.2);
        let travel = Vec3::new(0.8, 0.0, 0.0);
        let end = ROOST + travel;
        // Frames at 60 fps: flock 0..60, hold opens at 60, still to 75,
        // moves 75..99, stays 99..129.
        let (open, go, arrive, frames) = (60u32, 75u32, 99u32, 129u32);
        let center =
            |f: u32| ROOST + travel * (f.saturating_sub(go) as f32 / (arrive - go) as f32).min(1.0);
        let run = |strength: f32, carry: bool, release_at: u32| {
            let aux_at = |f: u32| {
                let mut aux = murmur_aux(eye);
                let [lanes, hold, vel] = murmur_lanes(1);
                if (open..release_at).contains(&f) {
                    aux[lanes].home = [1.0, 1.0, 0.0, strength];
                    let c = center(f);
                    aux[hold].home = [c.x, c.y, c.z, RADIUS];
                    let v = if carry && (go..arrive).contains(&f) {
                        travel * 60.0 / (arrive - go) as f32
                    } else {
                        Vec3::ZERO
                    };
                    aux[vel].home = [v.x, v.y, v.z, (f - open) as f32 / 60.0];
                }
                aux
            };
            let caps = murmur_run(COUNT, frames, aux_at, &[go, arrive, frames]);
            let caps: Vec<Vec<Vec3>> = caps.iter().map(|c| alive(c)).collect();
            let near =
                |pts: &[Vec3], at: Vec3| pts.iter().filter(|p| p.distance(at) < RADIUS).count();
            (
                near(&caps[0], ROOST),
                near(&caps[1], end),
                near(&caps[2], end),
                caps[2].len(),
            )
        };
        let (_, _, none, alive_none) = run(0.0, true, u32::MAX);
        let (latched, arrived, held, alive_held) = run(1.0, true, u32::MAX);
        let (_, spring_arrived, spring_held, _) = run(1.0, false, u32::MAX);
        let (_, _, released, _) = run(1.0, true, arrive);
        eprintln!(
            "hold: {latched} birds within {RADIUS} m a quarter second after it opened; on arrival {arrived} (spring alone {spring_arrived}); half a second later {held} (spring alone {spring_held}, released on arrival {released}, no hold {none}); {alive_held} / {alive_none} alive"
        );
        assert!(
            held > 5 * none.max(20),
            "the hold should bring a group: {held} birds at the end point against {none} without it"
        );
        assert!(
            held * 2 > latched,
            "the hold should keep most of what it latched: {held} of {latched}"
        );
        assert!(
            (held as u32) * 3 < COUNT,
            "a hold takes part of the flock, not the flock: {held} of {COUNT}"
        );
        assert!(
            spring_arrived * 2 < arrived,
            "the group should move with the hand, not trail it: {arrived} inside on arrival, {spring_arrived} on the spring alone"
        );
        assert!(
            alive_held * 10 > alive_none * 9,
            "a hold must not kill birds: {alive_held} against {alive_none}"
        );
    }

    /// Board #3314 step 4, a measurement: an open hand (calm, push only) is
    /// meant to part the flock without flinging it, and the worn A/B could
    /// not split the pad from the kick. A flat hand (5 x 5 joint spheres of
    /// 1 cm over 16 cm, facing its motion) sweeps 1.6 m along -Z through the
    /// flock (which lives at x > 0: the test box fills x < 0) at `SPEED`, a
    /// far hand's speed. For each pad and kick: parting, the birds in the
    /// hand's wake (within 8 cm of its path, the 30 cm behind it) once it is
    /// 0.3 m past the roost; flinging, the birds displaced more than 0.5 m
    /// while the hand travels from 0.4 m before the roost to 0.4 m past it.
    /// Both against the flock with no hand.
    #[test]
    #[ignore = "requires a GPU/software adapter; a measurement, prints a table"]
    fn murmur_xr_open_hand_pad_kick_sweep() {
        use crate::gpu::test_gpu::gpu_guard;
        const COUNT: u32 = 20_000;
        let _guard = gpu_guard();
        let speed: f32 = std::env::var("MURMUR_SWEEP_SPEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.5);
        let eye = ROOST + Vec3::new(0.0, 0.1, 1.2);
        // The path: z from 0.8 m behind the roost to 0.8 m ahead of it, 15 cm
        // into the flock's side of the box.
        let line = ROOST + Vec3::new(0.15, 0.0, 0.0);
        // Frames at 60 fps: settle 0..60, then the sweep.
        let start = 60u32;
        let at = |m: f32| start + (m / speed * 60.0).round() as u32;
        let sweep = at(1.6) - start;
        let palm = |f: u32| {
            let t = (f.saturating_sub(start)).min(sweep) as f32 / 60.0;
            line + Vec3::new(0.0, 0.0, 0.8 - speed * t)
        };
        let (before, wake_at, after) = (at(0.4), at(1.1), at(1.2));
        let run = |hand: Option<(f32, f32)>| {
            let aux_at = |f: u32| {
                let mut aux = murmur_aux(eye);
                let Some((pad, kick)) = hand else {
                    return aux;
                };
                let p = palm(f);
                let mut n = 0u32;
                for i in 0..5 {
                    for j in 0..5 {
                        let c =
                            p + Vec3::new((i as f32 - 2.0) * 0.04, (j as f32 - 2.0) * 0.04, 0.0);
                        aux[3 + n as usize].home = [c.x, c.y, c.z, 0.01 + pad];
                        n += 1;
                    }
                }
                aux[1].home[0] = f32::from_bits(n);
                // The header kick is 0.4; the hand's kick calm scales it.
                let [lanes, ..] = murmur_lanes(1);
                aux[lanes].home = [1.0, 0.0, 1.0 - kick / 0.4, 0.0];
                aux
            };
            let caps = murmur_run(COUNT, after, aux_at, &[before, wake_at, after]);
            let wake = caps[1]
                .iter()
                .flatten()
                .filter(|b| {
                    let d = **b - line;
                    (-0.3..0.0).contains(&d.z) && (d.x * d.x + d.y * d.y).sqrt() < 0.08
                })
                .count();
            let flung = caps[0]
                .iter()
                .zip(&caps[2])
                .filter(|(a, b)| matches!((a, b), (Some(a), Some(b)) if a.distance(*b) > 0.5))
                .count();
            (wake, flung)
        };
        let (wake0, flung0) = run(None);
        eprintln!(
            "open hand sweep at {speed} m/s: {} frames across the roost",
            after - before
        );
        eprintln!("pad m  kick m/s  wake birds  displaced > 0.5 m");
        eprintln!("no hand           {wake0:10}  {flung0:16}");
        for pad in [0.0, 0.02, 0.05, 0.10] {
            for kick in [0.0, 0.1, 0.2, 0.4] {
                let (wake, flung) = run(Some((pad, kick)));
                eprintln!("{pad:5.2}  {kick:8.1}  {wake:10}  {flung:16}");
            }
        }
    }

    /// Stands one bird 0.6 m ahead of the eye (see
    /// `murmur_xr_bird_in_front_lands_dark`): flying away at 0.2 m/s, 5 s into
    /// a 15 s life, born 10 s ago (spawn fade done), colorless until the real
    /// sim's first frame computes its look. Every other slot stays dead.
    const PLACE_ONE_BIRD: &str = "
@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }
    var p = read_particle(idx);
    if idx == 0u {
        p.pos_life = vec4f(0.0, 0.5, -0.6, 1.0);
        p.vel_size = vec4f(0.0, 0.0, -0.2, 0.012);
        p.color = vec4f(0.0);
        p.flags = vec4f(5.0, 15.0, 0.012, u.time - 10.0);
        mark_alive(idx);
    }
    write_particle(idx, p);
}";

    /// One bird in front of the camera, one frame of the real sim, drawn by
    /// `render_world`'s alpha pipeline over a target cleared to alpha 0: it
    /// lands as a dark silhouette (linear rgb < 0.1, as stored: what it adds
    /// over passthrough) that mostly hides what is behind it (alpha > 0.5).
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn murmur_xr_bird_in_front_lands_dark() {
        use crate::gpu::particle::ParticleSystem;
        use crate::gpu::particle::types::ParticleAux;
        use crate::gpu::test_gpu::{gpu_guard, test_gpu};

        const DIM: u32 = 256;
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        let mut def = xr_murmur_preset().particles.expect("particles");
        def.max_count = 256;
        def.emit_rate = 0.0;
        def.burst_on_beat = 0;
        let preamble = compute_preamble();
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut ps = ParticleSystem::new(
            &device,
            &queue,
            TextureFormat::Rgba8UnormSrgb,
            &def,
            &format!("{preamble}\n{PLACE_ONE_BIRD}"),
            def.interaction,
        );
        assert!(
            ps.spatial_hash_dims().is_some_and(|(_, d)| d > 1),
            "interaction_3d builds the 3D hash"
        );
        let dt = 1.0 / 60.0;
        ps.update_uniforms(dt, 0.0, [DIM as f32; 2], 0.0);
        let mut enc = device.create_command_encoder(&Default::default());
        ps.dispatch(&mut enc, &queue);
        queue.submit([enc.finish()]);
        ps.flip();

        // The real sim from here. Head at the eye with the world near fade;
        // no obstacles.
        ps.set_compute_shader(&device, &format!("{preamble}\n{XR_MURMUR_SIM}"));
        let eye = Vec3::new(0.0, 0.5, 0.0);
        let mut aux = vec![ParticleAux { home: [0.0; 4] }; 163];
        aux[0].home = [eye.x, eye.y, eye.z, 0.15];
        ps.update_aux_in_place(&queue, &aux);
        ps.update_uniforms(dt, dt, [DIM as f32; 2], 0.0);

        let anchor = Vec3::new(0.0, 1.0, 0.0);
        let camera = WorldCamera {
            view: Mat4::from_translation(-(anchor + eye)),
            proj: Mat4::perspective_rh(60f32.to_radians(), 1.0, 0.05, 100.0),
            anchor,
        };
        let (color, depth) = eye_targets(&device, DIM);
        let (color_view, depth_view) = (
            color.create_view(&Default::default()),
            depth.create_view(&Default::default()),
        );
        let mut enc = device.create_command_encoder(&Default::default());
        ps.dispatch(&mut enc, &queue);
        clear_eye(&mut enc, &color_view, &depth_view);
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
        queue.submit([enc.finish()]);
        wait(&device);
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");

        let rgba = read_rgba(&device, &queue, &color, DIM);
        let (i, px) = rgba
            .chunks_exact(4)
            .enumerate()
            .max_by_key(|(_, px)| px[3])
            .unwrap();
        let (x, y) = (i as u32 % DIM, i as u32 / DIM);
        assert!(
            x.abs_diff(DIM / 2) <= 3 && y.abs_diff(DIM / 2) <= 3,
            "the bird drew at ({x}, {y}), not ahead of the eye"
        );
        let alpha = f32::from(px[3]) / 255.0;
        let rgb = [0, 1, 2].map(|c| srgb_to_linear(px[c]));
        assert!(
            alpha > 0.5,
            "alpha {alpha}: the bird does not hide the room"
        );
        assert!(
            rgb.iter().all(|&c| c < 0.1),
            "rgb {rgb:?}: the bird is not a dark silhouette"
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
