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
    _pad0: f32,
    _pad1: f32,
    base_color: [f32; 4],
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

enum Geometry {
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

/// Raster the posed geometry into a `TARGET_RES`² RGBA frame and read it back.
///
/// Returns `(rgba8 bytes, width, height)` — exactly the shape a decoded video
/// frame arrives in, which is what lets the existing sampler take it unchanged.
fn render_to_rgba(
    device: &Device,
    queue: &Queue,
    geometry: &Geometry,
    model_def: &ModelSampleDef,
) -> Result<(Vec<u8>, u32, u32), String> {
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

    let model = Mat4::from_rotation_y(model_def.yaw_degrees.to_radians())
        * Mat4::from_rotation_x(model_def.pitch_degrees.to_radians());
    let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y);
    // Ortho box fit to the unit-normalized model (bounding radius 1, so the model
    // stays inside the frame at every rotation) with margin for splat billboards.
    // `scale` above 1 crops in for detail, below 1 pulls back.
    let half = (1.4 / model_def.scale.max(0.01)).max(0.01);
    let proj = Mat4::orthographic_rh(-half, half, -half, half, 1.6, 4.4);

    let uniforms = SampleUniforms {
        mv: (view * model).to_cols_array_2d(),
        proj: proj.to_cols_array_2d(),
        radius_scale: SPLAT_RADIUS_SCALE,
        ambient: model_def.ambient.clamp(0.0, 1.0),
        _pad0: 0.0,
        _pad1: 0.0,
        base_color: [1.0, 1.0, 1.0, 1.0],
    };
    let uniforms_buf = upload(
        device,
        "model-sample-uniforms",
        bytemuck::bytes_of(&uniforms),
        wgpu::BufferUsages::UNIFORM,
    );
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
    // sRGB target: the shader writes linear and the hardware encodes, so the bytes
    // that come back look like a PNG's — which is what the sampler's luminance and
    // gradient maths already assume.
    let color_target = wgpu::ColorTargetState {
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    };

    let (vs, fs, buffers): (_, _, Vec<wgpu::VertexBufferLayout>) = match geometry {
        Geometry::Mesh { .. } => (
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
        Geometry::Splat { .. } => (
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
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
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
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("model-sample-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Transparent, and load-bearing: the sampler rejects alpha < 10,
                    // so this is what makes the silhouette free and stops particles
                    // being spent on background.
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
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
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
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &color_tex,
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
        extent,
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
            Err(_) => (cube_mesh(&device), "cube".to_string()),
        };

        // Three-quarter view: a front-on cube is a flat square with one face lit,
        // which would hide exactly the shading bugs this preview exists to catch.
        let posed = ModelSampleDef {
            yaw_degrees: 35.0,
            pitch_degrees: 20.0,
            ..model_def
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
