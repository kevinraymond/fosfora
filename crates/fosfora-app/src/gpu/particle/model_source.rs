//! 3D-model particle source (#1993 — the unbuilt third of #1990's "BYO media").
//!
//! Models could already be *obstacles* (#1851) but never a particle **source**:
//! `morph::load_morph_target` knew `geometry:`, `text:`, `image:` and `video:`, and
//! the image emitter knew stills, video and webcam. A user could not point Raster,
//! Morph, Pegboard or Etch at their own mesh or scanned capture.
//!
//! # How a model becomes particles
//!
//! By being rendered to a frame. The geometry is rasterized offscreen into an RGBA
//! buffer with a transparent background (`model_sample.wgsl`), and that buffer is
//! handed to [`image_source::sample_rgba_buffer`] — the same call video and webcam
//! frames already make. So a model arrives at every media effect as just another
//! frame, and inherits the sampler whole: grid/threshold/random modes, the
//! ±0.4-cell jitter, the luminance gradient in `home.w`, aspect correction and the
//! `max_particles` cap. Nothing downstream needed to change, and in particular
//! `ParticleAux` did not have to widen — its one `vec4f` has all four lanes
//! spoken for (xy position, z packed RGBA, w gradient).
//!
//! The cost of that reuse is honest and worth stating: particles get 2D screen
//! positions, so what lands is a *view* of a model, not a point cloud.
//!
//! # Still, not live
//!
//! Sampling happens at load and again whenever the pose changes — not per frame.
//! Video pre-decodes to CPU frames, but a GPU render needs a readback, and doing
//! that every frame would stall the pipeline at up to 2M particles.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8};

use glam::{Mat4, Vec3};
use wgpu::{Device, Queue};

use super::image_source;
use super::types::{ImageSampleDef, ModelSampleDef, ParticleAux};

/// Square render-target resolution.
///
/// Fixed at the sampler's own `MAX_DIM` (image_source.rs) so it never downscales
/// what we just rendered, and so even a 2M-particle effect gets more candidate
/// samples than it can hold. A smaller target silently starves the effect and
/// reads as a broken model rather than as a budget problem — the same shape of
/// trap as the emit budget in #1999.
const TARGET_RES: u32 = 2048;

/// Cap on splat instances rasterized. Matches the obstacle path's cap: plenty for
/// a dense silhouette, and bounds the draw for multi-million-splat scenes.
const SPLAT_CAP: u32 = 150_000;

/// Billboard inflation for splat discs.
///
/// Deliberately smaller than the obstacle path's 1.5. That one inflates splats to
/// close every gap, because a collision silhouette with holes lets particles
/// through. Here the gaps cost nothing — a transparent texel is simply a texel
/// with no particle on it — and inflating past 1.0 visibly smears the capture's
/// detail into blobs, which is the opposite of what a source is for.
const SPLAT_RADIUS_SCALE: f32 = 1.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SampleUniforms {
    mv: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    radius_scale: f32,
    ambient: f32,
    light_mix: f32,
    ray_strength: f32,
    base_color: [f32; 4],
    // Scalar lanes, mirroring the WGSL exactly — a `[f32; 3]` here would still be
    // 12 bytes but the shader-side `vec3f` it invites would align to 16 and
    // desync the two. See the note atop `SampleUniforms` in model_sample.wgsl.
    light_x: f32,
    light_y: f32,
    light_z: f32,
    light_u: f32,
    light_v: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

/// File extensions this source accepts — the same set the obstacle model dialog
/// offers, so anything loadable as an obstacle is loadable as a source.
pub const MODEL_EXTENSIONS: &[&str] = &["glb", "gltf", "ply", "splat"];

/// Resolve a `.pfx` model reference against the assets tree.
///
/// Absolute paths pass straight through, so a `.pfx` written by the UI after a
/// user picked a file from anywhere on disk still loads. Bare names resolve
/// under `assets/models/`, matching how `image` and `video` references work.
pub fn resolve_model_path(assets_dir: &Path, name: &str) -> std::path::PathBuf {
    let p = Path::new(name);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        assets_dir.join("models").join(name)
    }
}

/// Whether `path` looks like a model this source can read.
pub fn is_model_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| MODEL_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub(crate) enum Geometry {
    Mesh {
        vbuf: wgpu::Buffer,
        ibuf: wgpu::Buffer,
        index_count: u32,
    },
    Splat {
        instances: wgpu::Buffer,
        count: u32,
    },
}

/// Load a model file and decompose it into particle aux data.
///
/// Dispatches on extension: `.glb`/`.gltf` → shaded triangle mesh, everything else
/// (`.ply`/`.splat`) → splat point cloud carrying its own captured colour.
pub fn sample_model(
    device: &Device,
    queue: &Queue,
    path: &Path,
    sample_def: &ImageSampleDef,
    model_def: &ModelSampleDef,
    max_particles: u32,
) -> Result<Vec<ParticleAux>, String> {
    // Guard the extension up front. Without it `load_geometry` sends anything
    // that is not glTF to the splat parser, so `model:art.png` fails with a PLY
    // header complaint that says nothing about the real mistake.
    if !is_model_path(path) {
        return Err(format!(
            "'{}' is not a model — expected one of {}",
            path.display(),
            MODEL_EXTENSIONS.join(", ")
        ));
    }
    let geometry = load_geometry(device, path)?;
    let (rgba, w, h) = render_to_rgba(device, queue, &geometry, model_def)?;
    let aux = image_source::sample_rgba_buffer(&rgba, w, h, sample_def, max_particles);
    if aux.is_empty() {
        // Every pixel was transparent: the model projected outside the frame, or
        // the file held no drawable geometry. Failing here beats handing back an
        // empty buffer that renders as a working effect with nothing in it.
        return Err(format!(
            "Model '{}' produced no particles — nothing rendered inside the frame",
            path.display()
        ));
    }
    Ok(aux)
}

fn load_geometry(device: &Device, path: &Path) -> Result<Geometry, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "glb" || ext == "gltf" {
        load_mesh(device, path)
    } else {
        load_splat(device, path)
    }
}

/// Read a glTF/GLB into an interleaved position+normal vertex buffer.
///
/// Reuses `obstacle_model::load_mesh_data`, which merges the scene into one
/// world-space unit-normalized mesh and guarantees a normal per vertex.
fn load_mesh(device: &Device, path: &Path) -> Result<Geometry, String> {
    let mesh = super::obstacle_model::load_mesh_data(path)?;

    let mut vbytes: Vec<u8> = Vec::with_capacity(mesh.positions.len() * 24);
    for (p, n) in mesh.positions.iter().zip(mesh.normals.iter()) {
        for v in [p.x, p.y, p.z, n.x, n.y, n.z] {
            vbytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    Ok(Geometry::Mesh {
        vbuf: upload(
            device,
            "model-sample-vbuf",
            &vbytes,
            wgpu::BufferUsages::VERTEX,
        ),
        ibuf: upload(
            device,
            "model-sample-ibuf",
            bytemuck::cast_slice(&mesh.indices),
            wgpu::BufferUsages::INDEX,
        ),
        index_count: mesh.indices.len() as u32,
    })
}

/// Read a `.ply`/`.splat` into instances of `(center.xyz, radius)` + `(rgb, 1)`.
///
/// Deliberately packs its own instances rather than extending the obstacle path's
/// loader: that one's instances are `(center.xyz, radius)` with no colour on
/// purpose, and it backs shipped collision behaviour.
fn load_splat(device: &Device, path: &Path) -> Result<Geometry, String> {
    let progress = AtomicU8::new(0);
    let cancel = AtomicBool::new(false);
    let cloud = super::splat_source::load_splat_file(
        path,
        SPLAT_CAP,
        super::splat_source::SceneOptions::default(),
        &progress,
        &cancel,
    )?;
    if cloud.count == 0 {
        return Err("splat scene has no points".to_string());
    }

    let mut instances: Vec<f32> = Vec::with_capacity(cloud.count * 8);
    for i in 0..cloud.count {
        let p = cloud.positions[i];
        let s = cloud.scales[i];
        let c = cloud.colors[i];
        let radius = s[0].max(s[1]).max(s[2]).max(1e-4);
        // 3DGS/COLMAP clouds are Y-down; rotate 180° about X (negate Y and Z,
        // handedness-preserving) into the Y-up frame this raster's camera expects,
        // so the capture stands upright rather than inverted. glTF meshes are
        // already Y-up and need no flip. Same convention as the obstacle path.
        instances.extend_from_slice(&[p[0], -p[1], -p[2], radius, c[0], c[1], c[2], 1.0]);
    }
    let bytes: Vec<u8> = instances.iter().flat_map(|v| v.to_le_bytes()).collect();
    Ok(Geometry::Splat {
        instances: upload(
            device,
            "model-sample-splat-instances",
            &bytes,
            wgpu::BufferUsages::VERTEX,
        ),
        count: cloud.count as u32,
    })
}

fn upload(device: &Device, label: &str, data: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: data,
        usage,
    })
}

/// Which vertex layout a raster's pipeline is built for.
///
/// The pipeline is baked at construction so it can be reused frame after frame,
/// and mesh and splat feed it completely different vertex buffers — so the kind
/// has to be known before any geometry is handed over.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RasterKind {
    Mesh,
    Splat,
}

impl Geometry {
    fn kind(&self) -> RasterKind {
        match self {
            Geometry::Mesh { .. } => RasterKind::Mesh,
            Geometry::Splat { .. } => RasterKind::Splat,
        }
    }
}

/// Camera, projection and light for one pose, in the form the shader wants.
///
/// Split out because it is the only part of a frame that changes when a slider
/// moves — everything else in `ModelRaster` is built once and reused. Pure maths,
/// no device access, so the live path can rebuild it every frame for free.
fn build_uniforms(model_def: &ModelSampleDef) -> SampleUniforms {
    let model = Mat4::from_rotation_y(model_def.yaw_degrees.to_radians())
        * Mat4::from_rotation_x(model_def.pitch_degrees.to_radians());
    let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y);
    // Ortho box fit to the unit-normalized model (bounding radius 1, so the model
    // stays inside the frame at every rotation) with margin for splat billboards.
    // `scale` above 1 crops in for detail, below 1 pulls back.
    let half = (1.4 / model_def.scale.max(0.01)).max(0.01);
    let proj = Mat4::orthographic_rh(-half, half, -half, half, 1.6, 4.4);

    // The light is authored in MODEL space, so `view * model` carries it exactly
    // as it carries the geometry — a light inside a skull stays inside it at
    // every yaw, with no separate transform to keep in sync (#1996).
    let mv = view * model;
    let light_model = Vec3::new(model_def.light_x, model_def.light_y, model_def.light_z);
    let light_view = mv.transform_point3(light_model);
    // ...and again through the projection for the radial march's screen origin.
    // The projection is orthographic, so w is 1 and clip space IS ndc; the v flip
    // is the usual ndc-y-up to texture-v-down.
    let light_clip = proj.project_point3(light_view);

    SampleUniforms {
        mv: mv.to_cols_array_2d(),
        proj: proj.to_cols_array_2d(),
        radius_scale: SPLAT_RADIUS_SCALE,
        ambient: model_def.ambient.clamp(0.0, 1.0),
        light_mix: model_def.light_mix.clamp(0.0, 1.0),
        ray_strength: model_def.ray_strength.clamp(0.0, 1.0),
        base_color: [1.0, 1.0, 1.0, 1.0],
        light_x: light_view.x,
        light_y: light_view.y,
        light_z: light_view.z,
        light_u: light_clip.x * 0.5 + 0.5,
        light_v: 0.5 - light_clip.y * 0.5,
        _pad0: 0.0,
        _pad1: 0.0,
        _pad2: 0.0,
    }
}

/// Persistent GPU resources for rasterizing a posed model.
///
/// Everything expensive — shader module, pipelines, render targets — is built
/// once and reused, so re-rendering a new pose costs one uniform write and two
/// render passes. Nothing here touches the CPU.
///
/// That is the whole point (#2010). Rendering a model was never the reason a
/// model source is a STILL; the passes have always been GPU-only. The cost was
/// the 16MB `copy_texture_to_buffer` + `map_async` that followed, which is why
/// the readback lives in [`render_to_rgba`] rather than in here — the live path
/// records the same passes and simply never asks for the pixels back.
pub(crate) struct ModelRaster {
    kind: RasterKind,
    uniforms_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    color_tex: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    // God-ray resources. Built eagerly alongside the rest: deciding per frame
    // whether rays are on is a uniform check, not a reason to compile a pipeline
    // mid-flight.
    godray_pipeline: wgpu::RenderPipeline,
    godray_bind_group: wgpu::BindGroup,
    rays_tex: wgpu::Texture,
    rays_view: wgpu::TextureView,
    extent: wgpu::Extent3d,
}

impl ModelRaster {
    pub(crate) fn new(device: &Device, kind: RasterKind) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model-sample-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../../assets/shaders/model_sample.wgsl").into(),
            ),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model-sample-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // COPY_DST, unlike the old create-per-call buffer: a new pose is a
        // `write_buffer`, not a fresh allocation.
        let uniforms_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("model-sample-uniforms"),
            size: std::mem::size_of::<SampleUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model-sample-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("model-sample-layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let depth_state = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        // sRGB target: the shader writes linear and the hardware encodes, so the
        // bytes that come back look like a PNG's — which is what the sampler's
        // luminance and gradient maths already assume.
        let color_target = wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        };

        let (vs, fs, buffers): (_, _, Vec<wgpu::VertexBufferLayout>) = match kind {
            RasterKind::Mesh => (
                "vs_mesh",
                "fs_mesh",
                vec![wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
                }],
            ),
            RasterKind::Splat => (
                "vs_splat",
                "fs_splat",
                vec![wgpu::VertexBufferLayout {
                    array_stride: 32,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 16,
                            shader_location: 1,
                        },
                    ],
                }],
            ),
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("model-sample-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None, // model winding is unreliable; keep both faces
                ..Default::default()
            },
            depth_stencil: Some(depth_state),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &[Some(color_target)],
            }),
            multiview: None,
            cache: None,
        });

        let extent = wgpu::Extent3d {
            width: TARGET_RES,
            height: TARGET_RES,
            depth_or_array_layers: 1,
        };
        let color_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("model-sample-color"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("model-sample-depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let (godray_pipeline, godray_bind_group, rays_tex, rays_view) =
            build_godray(device, &shader, &bgl, &color_view, extent);

        Self {
            kind,
            uniforms_buf,
            bind_group,
            pipeline,
            color_tex,
            color_view,
            depth_view,
            godray_pipeline,
            godray_bind_group,
            rays_tex,
            rays_view,
            extent,
        }
    }

    /// Record one frame's passes. Returns the texture holding the result.
    ///
    /// Panics if `geometry` is not the kind this raster's pipeline was built for
    /// — a mesh drawn through the splat vertex layout is a wgpu validation error
    /// with a far less obvious message.
    pub(crate) fn record(
        &self,
        queue: &Queue,
        encoder: &mut wgpu::CommandEncoder,
        geometry: &Geometry,
        model_def: &ModelSampleDef,
    ) -> &wgpu::Texture {
        assert!(
            geometry.kind() == self.kind,
            "geometry kind changed under a ModelRaster built for the other one"
        );
        let uniforms = build_uniforms(model_def);
        queue.write_buffer(&self.uniforms_buf, 0, bytemuck::bytes_of(&uniforms));

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("model-sample-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.color_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Transparent, and load-bearing: the sampler rejects
                        // alpha < 10, so this is what makes the silhouette free
                        // and stops particles being spent on background.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            match geometry {
                Geometry::Mesh {
                    vbuf,
                    ibuf,
                    index_count,
                } => {
                    pass.set_vertex_buffer(0, vbuf.slice(..));
                    pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..*index_count, 0, 0..1);
                }
                Geometry::Splat { instances, count } => {
                    pass.set_vertex_buffer(0, instances.slice(..));
                    pass.draw(0..6, 0..*count);
                }
            }
        }

        // God-ray pass (#1996). Skipped outright when rays are off, so a model
        // source with no interior light costs exactly what it did in v1.28.0 and
        // its output is byte-identical.
        if uniforms.ray_strength <= 0.0 {
            return &self.color_tex;
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("model-godray-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.rays_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Every texel is written by the fullscreen triangle, but
                        // clear transparent anyway: the invariant that untouched
                        // frame is alpha 0 is what the sampler leans on.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.godray_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_bind_group(1, &self.godray_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        &self.rays_tex
    }
}

/// Build the radial-scattering pass that turns escaping light into shafts.
///
/// A render pass cannot sample the target it writes, so this is a second target
/// rather than an in-place blend. It composites the model back over the rays it
/// produces, so the result is a drop-in replacement for the first pass's output —
/// same format, same transparent background, same contract with the sampler.
fn build_godray(
    device: &Device,
    shader: &wgpu::ShaderModule,
    uniform_bgl: &wgpu::BindGroupLayout,
    src_view: &wgpu::TextureView,
    extent: wgpu::Extent3d,
) -> (
    wgpu::RenderPipeline,
    wgpu::BindGroup,
    wgpu::Texture,
    wgpu::TextureView,
) {
    let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("model-godray-tex-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    // Clamp-to-edge is safe precisely because the model is fitted inside the frame
    // with margin: the border texels are the transparent clear, so a march that
    // walks off the edge reads emission 0 rather than smearing a lit pixel.
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("model-godray-sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("model-godray-tex-bg"),
        layout: &tex_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(src_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("model-godray-layout"),
        bind_group_layouts: &[uniform_bgl, &tex_bgl],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("model-godray-pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_fullscreen"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            ..Default::default()
        },
        // No depth: this pass covers the frame once and blends nothing.
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_godray"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    });

    let rays_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("model-godray-color"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let rays_view = rays_tex.create_view(&wgpu::TextureViewDescriptor::default());
    (pipeline, bind_group, rays_tex, rays_view)
}

/// Raster the posed geometry into a `TARGET_RES`² RGBA frame and read it back.
///
/// Returns `(rgba8 bytes, width, height)` — exactly the shape a decoded video
/// frame arrives in, which is what lets the existing sampler take it unchanged.
///
/// The readback is the expensive half and the reason a model source is a STILL.
/// It lives here rather than in [`ModelRaster`] so the live path (#2010) can
/// record the identical passes and never pay it.
fn render_to_rgba(
    device: &Device,
    queue: &Queue,
    geometry: &Geometry,
    model_def: &ModelSampleDef,
) -> Result<(Vec<u8>, u32, u32), String> {
    let raster = ModelRaster::new(device, geometry.kind());

    // TARGET_RES * 4 = 8192, already a multiple of COPY_BYTES_PER_ROW_ALIGNMENT,
    // so the readback needs no per-row padding dance.
    let bytes_per_row = TARGET_RES * 4;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model-sample-staging"),
        size: (bytes_per_row * TARGET_RES) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("model-sample-encoder"),
    });
    let readback_tex = raster.record(queue, &mut encoder, geometry, model_def);
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: readback_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(TARGET_RES),
            },
        },
        raster.extent,
    );
    queue.submit([encoder.finish()]);

    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| {
        if let Err(e) = r {
            log::error!("model sample readback failed: {e}");
        }
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .map_err(|e| format!("model sample readback poll: {e}"))?;
    let rgba = slice.get_mapped_range().to_vec();
    staging.unmap();

    Ok((rgba, TARGET_RES, TARGET_RES))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};

    fn defaults() -> (ImageSampleDef, ModelSampleDef) {
        (
            ImageSampleDef {
                mode: "grid".to_string(),
                threshold: 0.1,
                scale: 1.0,
            },
            ModelSampleDef::default(),
        )
    }

    /// An axis-aligned cube spanning [-0.5, 0.5]: enough geometry to project a
    /// solid, obviously-bounded silhouette without committing a `.glb` to the repo.
    fn cube_mesh(device: &Device) -> Geometry {
        let c = 0.5f32;
        let corners = [
            Vec3::new(-c, -c, -c),
            Vec3::new(c, -c, -c),
            Vec3::new(c, c, -c),
            Vec3::new(-c, c, -c),
            Vec3::new(-c, -c, c),
            Vec3::new(c, -c, c),
            Vec3::new(c, c, c),
            Vec3::new(-c, c, c),
        ];
        let faces: [[usize; 4]; 6] = [
            [0, 1, 2, 3],
            [5, 4, 7, 6],
            [4, 0, 3, 7],
            [1, 5, 6, 2],
            [3, 2, 6, 7],
            [4, 5, 1, 0],
        ];
        let mut vbytes: Vec<u8> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for f in faces {
            let base = (indices.len() / 6 * 4) as u32;
            let n = (corners[f[1]] - corners[f[0]])
                .cross(corners[f[3]] - corners[f[0]])
                .normalize();
            for i in f {
                for v in [corners[i].x, corners[i].y, corners[i].z, n.x, n.y, n.z] {
                    vbytes.extend_from_slice(&v.to_le_bytes());
                }
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        Geometry::Mesh {
            vbuf: upload(device, "test-vbuf", &vbytes, wgpu::BufferUsages::VERTEX),
            ibuf: upload(
                device,
                "test-ibuf",
                bytemuck::cast_slice(&indices),
                wgpu::BufferUsages::INDEX,
            ),
            index_count: indices.len() as u32,
        }
    }

    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn model_sample_produces_bounded_varied_aux() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let (sample_def, model_def) = defaults();

        let geometry = cube_mesh(&device);
        // Three-quarter view, not the default front-on one: a cube seen head-on
        // shows exactly ONE face, so it has no shading variation to measure and the
        // whole tone assertion below would be riding on silhouette edge pixels.
        let posed = ModelSampleDef {
            yaw_degrees: 35.0,
            pitch_degrees: 20.0,
            ..model_def
        };
        let (rgba, w, h) = render_to_rgba(&device, &queue, &geometry, &posed).unwrap();
        let aux = image_source::sample_rgba_buffer(&rgba, w, h, &sample_def, 50_000);

        assert!(!aux.is_empty(), "cube produced no particles");

        // A signature that is merely STABLE looks like a passing probe; what marks
        // a blank frame is that it has no SPREAD (#1999). The cube is lit from one
        // side, so its faces must differ in tone.
        let lum: Vec<f32> = aux
            .iter()
            .map(|a| {
                let bits = a.home[2].to_bits();
                let (r, g, b) = (
                    (bits & 0xff) as f32,
                    ((bits >> 8) & 0xff) as f32,
                    ((bits >> 16) & 0xff) as f32,
                );
                (r * 0.299 + g * 0.587 + b * 0.114) / 255.0
            })
            .collect();
        let mean = lum.iter().sum::<f32>() / lum.len() as f32;
        let sd = (lum.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / lum.len() as f32).sqrt();

        // Percentiles, not min/max. The sampler bilinear-blends across the
        // silhouette edge into the transparent clear colour, so a handful of rim
        // samples come out near-black on ANY model — a raw range would report a
        // healthy spread even if every face rendered the same shade.
        let mut sorted = lum.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p = |q: f32| sorted[((sorted.len() - 1) as f32 * q) as usize];
        let (p10, p90) = (p(0.10), p(0.90));
        println!("cube tone: mean {mean:.3} sd {sd:.3} p10..p90 {p10:.3}..{p90:.3}");

        assert!(sd > 0.01, "flat frame — luminance sd {sd} (mean {mean})");
        // The shading is encoded perceptually (see srgb_to_linear in
        // model_sample.wgsl) so the cube's lit and shadowed faces land far apart.
        // A model whose tones all bunch together gives Pegboard's peg tray and
        // Etch's scan bands nothing to quantize.
        //
        // The threshold is calibrated against the regression it exists to catch,
        // by measuring BOTH ways on this fixed cube and pose: with the perceptual
        // encode the spread is 0.459, with the lambert term written as linear light
        // it is 0.286. 0.35 sits between them. A looser bound passes either way and
        // guards nothing — which is exactly what 0.2 did on the first cut.
        assert!(
            p90 - p10 > 0.35,
            "tonal spread collapsed to {:.3} (p10 {p10:.3}, p90 {p90:.3}) — shading \
             is not reaching the byte the sampler reads",
            p90 - p10
        );

        // Every sample must be alpha-opaque: the transparent background is supposed
        // to be rejected outright, not sampled as black particles.
        for a in &aux {
            assert!(
                (a.home[2].to_bits() >> 24) >= 10,
                "a transparent pixel was sampled"
            );
        }

        // The cube spans a bounding radius of 0.87 inside a half-extent-1.4 ortho
        // box, so nothing may land beyond ~0.62 of clip space. A background leak
        // would push samples out to the frame edge.
        let max_extent = aux
            .iter()
            .map(|a| a.home[0].abs().max(a.home[1].abs()))
            .fold(0.0f32, f32::max);
        assert!(
            max_extent < 0.7,
            "samples reached {max_extent} — background leaked into the silhouette"
        );
    }

    /// Write the raster to a PNG so the frame the sampler actually sees can be
    /// LOOKED at. A numeric signature can read healthy on a frame that is blank or
    /// inside-out (#1999) — an eyeball is the only check that catches that.
    ///
    /// `MODEL_FILE=/path/to/thing.glb` aims it at a real asset; with nothing set it
    /// renders the built-in cube. `MODEL_PNG_DIR` overrides the output directory.
    /// A closed box with a square hole in the wall facing the camera, normals
    /// OUTWARD throughout — the convention a real `.glb` uses.
    ///
    /// This is the #1996 picture reduced to its essentials: a hollow form whose
    /// interior is visible only through an opening. A light at the origin sits
    /// inside it, so the near wall is lit from behind (and must stay dark) while
    /// the far wall's interior shows through the hole (and must not).
    fn holed_box_mesh(device: &Device) -> Geometry {
        let (o, h) = (0.5f32, 0.2f32);
        let mut vbytes: Vec<u8> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut quad = |c: [Vec3; 4], n: Vec3| {
            let base = (vbytes.len() / 24) as u32;
            for p in c {
                for v in [p.x, p.y, p.z, n.x, n.y, n.z] {
                    vbytes.extend_from_slice(&v.to_le_bytes());
                }
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        };

        // Five solid walls.
        for (n, axis) in [
            (Vec3::new(0.0, 0.0, -1.0), 2),
            (Vec3::new(-1.0, 0.0, 0.0), 0),
            (Vec3::new(1.0, 0.0, 0.0), 0),
            (Vec3::new(0.0, -1.0, 0.0), 1),
            (Vec3::new(0.0, 1.0, 0.0), 1),
        ] {
            let k = if n[axis] > 0.0 { o } else { -o };
            let corners = match axis {
                0 => [
                    Vec3::new(k, -o, -o),
                    Vec3::new(k, o, -o),
                    Vec3::new(k, o, o),
                    Vec3::new(k, -o, o),
                ],
                1 => [
                    Vec3::new(-o, k, -o),
                    Vec3::new(o, k, -o),
                    Vec3::new(o, k, o),
                    Vec3::new(-o, k, o),
                ],
                _ => [
                    Vec3::new(-o, -o, k),
                    Vec3::new(o, -o, k),
                    Vec3::new(o, o, k),
                    Vec3::new(-o, o, k),
                ],
            };
            quad(corners, n);
        }

        // Near wall, as four strips around a hole spanning [-h, h]².
        let n = Vec3::new(0.0, 0.0, 1.0);
        for (x0, x1, y0, y1) in [
            (-o, -h, -o, o),
            (h, o, -o, o),
            (-h, h, -o, -h),
            (-h, h, h, o),
        ] {
            quad(
                [
                    Vec3::new(x0, y0, o),
                    Vec3::new(x1, y0, o),
                    Vec3::new(x1, y1, o),
                    Vec3::new(x0, y1, o),
                ],
                n,
            );
        }

        Geometry::Mesh {
            vbuf: upload(
                device,
                "test-holed-vbuf",
                &vbytes,
                wgpu::BufferUsages::VERTEX,
            ),
            ibuf: upload(
                device,
                "test-holed-ibuf",
                bytemuck::cast_slice(&indices),
                wgpu::BufferUsages::INDEX,
            ),
            index_count: indices.len() as u32,
        }
    }

    /// A single small quad in the model's upper-LEFT, and nothing else.
    ///
    /// Deliberately asymmetric in both axes. A mirrored or flipped raster is
    /// invisible to every other fixture here — a cube, a centred box and a
    /// silhouette coverage count all survive any global transform — which is the
    /// same blind spot that let the splat renderer ship mirrored for two
    /// releases (#1912). Only an off-centre landmark can catch it.
    fn marker_mesh(device: &Device) -> Geometry {
        // Model space: -x is left, +y is up.
        let c = [
            Vec3::new(-0.6, 0.3, 0.0),
            Vec3::new(-0.2, 0.3, 0.0),
            Vec3::new(-0.2, 0.7, 0.0),
            Vec3::new(-0.6, 0.7, 0.0),
        ];
        let n = Vec3::new(0.0, 0.0, 1.0);
        let mut vbytes: Vec<u8> = Vec::new();
        for p in c {
            for v in [p.x, p.y, p.z, n.x, n.y, n.z] {
                vbytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        let indices: Vec<u32> = vec![0, 1, 2, 0, 2, 3];
        Geometry::Mesh {
            vbuf: upload(
                device,
                "test-marker-vbuf",
                &vbytes,
                wgpu::BufferUsages::VERTEX,
            ),
            ibuf: upload(
                device,
                "test-marker-ibuf",
                bytemuck::cast_slice(&indices),
                wgpu::BufferUsages::INDEX,
            ),
            index_count: indices.len() as u32,
        }
    }

    /// The raster must not flip or mirror the model it was handed.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn model_raster_preserves_orientation() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let geometry = marker_mesh(&device);
        // Rays ON. The god-ray pass writes to a SECOND target and the readback
        // switches to it, so orientation has to be asserted through that path
        // too — checking only the default (rays off) exercises the branch that
        // was never in question, which is exactly how the flip shipped.
        let posed = ModelSampleDef {
            light_mix: 1.0,
            ray_strength: 0.6,
            ..ModelSampleDef::default()
        };
        let (rgba, res, _) = render_to_rgba(&device, &queue, &geometry, &posed).unwrap();

        // Centroid of everything the sampler would keep.
        let (mut sx, mut sy, mut n) = (0f64, 0f64, 0u32);
        for (i, p) in rgba.chunks_exact(4).enumerate() {
            if p[3] >= 10 {
                sx += (i as u32 % res) as f64;
                sy += (i as u32 / res) as f64;
                n += 1;
            }
        }
        assert!(n > 1000, "marker did not render ({n} opaque texels)");
        let (cx, cy) = (sx / n as f64, sy / n as f64);
        let half = res as f64 / 2.0;
        println!("marker centroid: col {cx:.0}, row {cy:.0} (frame {res}, half {half})");

        // The quad sits left of centre and above it in MODEL space. Row 0 is the
        // top of a rendered target and also the top of the image the sampler
        // reads, so it must land in the upper-left of the buffer too.
        assert!(
            cx < half,
            "marker is at model -x (left) but rasterized to column {cx:.0} of {res} \
             — the raster is MIRRORED horizontally"
        );
        assert!(
            cy < half,
            "marker is at model +y (up) but rasterized to row {cy:.0} of {res} \
             — the raster is FLIPPED vertically"
        );

        // ...and the same landmark must survive the sampler into particle space,
        // where -x is left and +y is up.
        let (sample_def, _) = defaults();
        let aux = image_source::sample_rgba_buffer(&rgba, res, res, &sample_def, 100_000);
        assert!(!aux.is_empty(), "marker sampled to no particles");
        let mx = aux.iter().map(|a| a.home[0] as f64).sum::<f64>() / aux.len() as f64;
        let my = aux.iter().map(|a| a.home[1] as f64).sum::<f64>() / aux.len() as f64;
        println!("marker in particle space: x {mx:.3}, y {my:.3}");
        assert!(mx < 0.0, "marker at model -x landed at particle x {mx:.3}");
        assert!(my > 0.0, "marker at model +y landed at particle y {my:.3}");
    }

    /// Mean red of a small patch, to keep assertions off single-texel noise.
    fn patch_red(rgba: &[u8], res: u32, cx: u32, cy: u32) -> f32 {
        let mut sum = 0u32;
        let mut n = 0u32;
        for y in cy - 8..cy + 8 {
            for x in cx - 8..cx + 8 {
                sum += rgba[((y * res + x) * 4) as usize] as u32;
                n += 1;
            }
        }
        sum as f32 / n as f32
    }

    /// Texels the sampler would accept (alpha >= 10) outside a centred square of
    /// half-width `margin`. `cube_mesh` spans a bounding radius of 0.87 inside a
    /// half-extent-1.4 ortho box, so at any pose its silhouette stays well inside
    /// margin 500 of 1024 — anything counted here is background the first pass
    /// left transparent.
    fn alpha_outside(rgba: &[u8], res: u32, margin: u32) -> usize {
        let half = res / 2;
        rgba.chunks_exact(4)
            .enumerate()
            .filter(|(i, p)| {
                let (x, y) = (*i as u32 % res, *i as u32 / res);
                let out = x.abs_diff(half) > margin || y.abs_diff(half) > margin;
                out && p[3] >= 10
            })
            .count()
    }

    /// The #1996 mechanism, end to end: a light inside a hollow form lights only
    /// what faces the cavity.
    ///
    /// The near wall and the far wall's interior are the SAME distance-ish from
    /// the same light and differ only in which way they face, so this separates
    /// one-sided shading from every other reason a pixel might be dark. Restoring
    /// the key light's `abs()` in the point-light term lights the near wall to
    /// roughly the value the hole reads, and this test fails.
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn light_inside_a_hollow_form_lights_only_what_faces_the_cavity() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let geometry = holed_box_mesh(&device);

        let (rgba, res, _) = render_to_rgba(
            &device,
            &queue,
            &geometry,
            &ModelSampleDef {
                light_mix: 1.0,
                // Origin: inside the box, behind the near wall, facing the far one.
                ambient: 0.0,
                ..ModelSampleDef::default()
            },
        )
        .unwrap();

        // The box spans ±0.36 of clip inside the half-extent-1.4 ortho box (±366
        // texels of centre) and the hole ±146, so the centre samples the far
        // wall through the opening and (1300, 1024) samples the near wall.
        let through_hole = patch_red(&rgba, res, res / 2, res / 2);
        let near_wall = patch_red(&rgba, res, 1300, res / 2);
        println!("hollow form: through-hole {through_hole:.1}, near wall {near_wall:.1}");

        assert!(
            through_hole > 150.0,
            "the cavity wall visible through the opening reads {through_hole:.1}/255 \
             — a light inside the form is not reaching what faces it"
        );
        assert!(
            near_wall < 8.0,
            "the near wall reads {near_wall:.1}/255 with the light BEHIND it — the \
             point-light term is two-sided, so there is no inside to be lit from"
        );
    }

    /// God rays must reach the SAMPLER, not merely the screenshot (#1996).
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn godrays_write_sampler_visible_alpha() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let geometry = holed_box_mesh(&device);

        let lit = ModelSampleDef {
            light_mix: 1.0,
            ambient: 0.0,
            ..ModelSampleDef::default()
        };

        let (off, res, _) = render_to_rgba(&device, &queue, &geometry, &lit).unwrap();
        let (on, _, _) = render_to_rgba(
            &device,
            &queue,
            &geometry,
            &ModelSampleDef {
                ray_strength: 1.0,
                ..lit
            },
        )
        .unwrap();

        // Alpha stepping outward from the silhouette edge — a diffable signature of
        // how far the shafts actually carry, which the counts below cannot show.
        let profile: Vec<u8> = (0..8)
            .map(|k| on[((((res / 2) * res) + (1440 + k * 80).min(res - 1)) * 4 + 3) as usize])
            .collect();
        println!("rays-on alpha from silhouette edge outward: {profile:?}");

        let (off_n, on_n) = (alpha_outside(&off, res, 500), alpha_outside(&on, res, 500));
        println!("godray alpha outside silhouette: off {off_n}, on {on_n}");

        assert_eq!(
            off_n, 0,
            "rays off yet {off_n} background texels carry alpha — the transparent \
             clear is no longer load-bearing"
        );
        // The specific regression: rays that render as colour but carry no alpha.
        // That frame looks correct in a PNG and produces not one extra particle,
        // so a colour-only check would pass while the feature does nothing.
        assert!(
            on_n > 10_000,
            "rays on but only {on_n} background texels are sampler-visible \
             (alpha >= 10) — shafts that the sampler cannot see yield no particles"
        );

        // ...and finally through the sampler itself, which is the actual claim:
        // the shafts are MADE OF particles. The raster assertions above would all
        // hold for a frame the sampler still declined to place anything on.
        let (sample_def, _) = defaults();
        let beyond = |rgba: &[u8]| {
            image_source::sample_rgba_buffer(rgba, res, res, &sample_def, 200_000)
                .iter()
                // The box spans ±0.36 of the frame, so anything past 0.45 sits on
                // a shaft rather than on the model.
                .filter(|a| a.home[0].abs().max(a.home[1].abs()) > 0.45)
                .count()
        };
        let (off_p, on_p) = (beyond(&off), beyond(&on));
        println!("particles on shafts: off {off_p}, on {on_p}");
        assert_eq!(off_p, 0, "{off_p} particles off-model with rays off");
        assert!(
            on_p > 1_000,
            "only {on_p} particles landed on the shafts — the rays reach the raster \
             but not the particle field"
        );
    }

    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn model_source_preview() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let (_, model_def) = defaults();
        let out_dir = std::env::var("MODEL_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());

        let (geometry, label) = match std::env::var("MODEL_FILE") {
            Ok(f) => (
                load_geometry(&device, Path::new(&f)).expect("load model"),
                Path::new(&f)
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "model".to_string()),
            ),
            // MODEL_LIGHT renders the hollow-form fixture lit from inside with
            // rays on, which is #1996's picture without needing a `.glb` on hand.
            Err(_) if std::env::var("MODEL_LIGHT").is_ok() => {
                (holed_box_mesh(&device), "holed-box-lit".to_string())
            }
            Err(_) => (cube_mesh(&device), "cube".to_string()),
        };

        // Three-quarter view: a front-on cube is a flat square with one face lit,
        // which would hide exactly the shading bugs this preview exists to catch.
        let posed = if std::env::var("MODEL_LIGHT").is_ok() {
            ModelSampleDef {
                light_mix: 1.0,
                ambient: 0.0,
                ray_strength: 1.0,
                ..model_def
            }
        } else {
            ModelSampleDef {
                yaw_degrees: 35.0,
                pitch_degrees: 20.0,
                ..model_def
            }
        };
        let (rgba, w, h) = render_to_rgba(&device, &queue, &geometry, &posed).unwrap();

        let opaque = rgba.chunks_exact(4).filter(|p| p[3] >= 10).count();
        let coverage = opaque as f32 / (w * h) as f32;
        println!(
            "{label}: {w}x{h}, {opaque} opaque texels ({:.1}% coverage)",
            coverage * 100.0
        );
        assert!(
            coverage > 0.001,
            "nothing rendered — {:.4}% coverage",
            coverage * 100.0
        );

        let path = format!("{out_dir}/model_source_{label}.png");
        image::RgbaImage::from_raw(w, h, rgba)
            .expect("raster into image")
            .save(&path)
            .expect("write png");
        println!("wrote {path}");
    }

    #[test]
    fn model_extension_detection() {
        assert!(is_model_path(Path::new("/tmp/skull.glb")));
        assert!(is_model_path(Path::new("/tmp/skull.GLTF")));
        assert!(is_model_path(Path::new("/tmp/room.ply")));
        assert!(is_model_path(Path::new("/tmp/room.splat")));
        assert!(!is_model_path(Path::new("/tmp/art.png")));
        assert!(!is_model_path(Path::new("/tmp/noext")));
    }
}
