//! 3D-model obstacle source (#1851 — "waterfall of roses / skulls").
//!
//! Turns a triangle mesh (`.glb`/`.gltf`) or a splat point cloud (`.ply`/
//! `.splat`) into a live, audio-reactive collision field. Every frame the posed
//! model is depth-rasterized into the layer's [`ObstacleTexture`] render target
//! (`obstacle_model.wgsl`, near-bright depth → alpha), which the particle sim
//! then samples exactly like an image/video/depth obstacle. All ~16
//! collision-capable effects inherit it for free.
//!
//! The pose (a slow yaw spin plus a subtle audio-driven boost/tilt) is applied
//! here in view space; the projection is orthographic and fit to a normalized
//! unit model, so framebuffer depth is linear and `1 - depth` is a true
//! near→far ramp regardless of rotation.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8};

use glam::{Mat4, Vec3};
use wgpu::{Device, Queue};

use crate::audio::features::AudioFeatures;

use super::obstacle::ObstacleTexture;

/// Square render-target resolution for the obstacle field. The obstacle `Fit`
/// mode maps this square to the (non-square) screen, so a fixed square keeps
/// the depth raster aspect-correct.
pub const TARGET_RES: u32 = 512;

/// Cap on splat instances rasterized into the field — plenty for a collision
/// silhouette, and bounds the per-frame draw for multi-million-splat scenes.
const SPLAT_CAP: u32 = 150_000;

// Pose tuning (radians / radians·s⁻¹). Fixed rather than wired to the effect's
// param sliders: each effect already assigns its 8 params their own meaning, so
// hijacking them for pose would break the host effect.
const BASE_SPIN: f32 = 0.4;
const AUDIO_SPIN: f32 = 1.2;
const TILT_AMT: f32 = 0.25;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ModelUniforms {
    mv: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    radius_scale: f32,
    _pad: [f32; 3],
}

enum ModelGeometry {
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

struct PoseState {
    yaw: f32,
}

/// A loaded model obstacle: geometry on the GPU plus the pipelines and target
/// needed to re-raster its depth every frame.
pub struct ObstacleModel {
    geometry: ModelGeometry,
    mesh_pipeline: wgpu::RenderPipeline,
    splat_pipeline: wgpu::RenderPipeline,
    uniforms_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    _depth_tex: wgpu::Texture,
    depth_view: wgpu::TextureView,
    pose: PoseState,
}

impl ObstacleModel {
    /// Load a model from a file, dispatching on extension: `.glb`/`.gltf` →
    /// triangle mesh, everything else (`.ply`/`.splat`) → splat point cloud.
    pub fn load(device: &Device, path: &Path) -> Result<Self, String> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let geometry = if ext == "glb" || ext == "gltf" {
            load_mesh(device, path)?
        } else {
            load_splat(device, path)?
        };
        Ok(Self::from_geometry(device, geometry))
    }

    fn from_geometry(device: &Device, geometry: ModelGeometry) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("obstacle-model-shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../../../../assets/shaders/obstacle_model.wgsl").into(),
            ),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("obstacle-model-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniforms_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("obstacle-model-uniforms"),
            size: std::mem::size_of::<ModelUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("obstacle-model-bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms_buf.as_entire_binding(),
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("obstacle-model-layout"),
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
        let color_target = wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8Unorm,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        };

        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("obstacle-model-mesh-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x3,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None, // model winding is unreliable; keep both faces
                ..Default::default()
            },
            depth_stencil: Some(depth_state.clone()),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &[Some(color_target.clone())],
            }),
            multiview: None,
            cache: None,
        });

        let splat_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("obstacle-model-splat-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_splat"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 0,
                        shader_location: 0,
                    }],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(depth_state),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_splat"),
                compilation_options: Default::default(),
                targets: &[Some(color_target)],
            }),
            multiview: None,
            cache: None,
        });

        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("obstacle-model-depth"),
            size: wgpu::Extent3d {
                width: TARGET_RES,
                height: TARGET_RES,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            geometry,
            mesh_pipeline,
            splat_pipeline,
            uniforms_buf,
            bind_group,
            _depth_tex: depth_tex,
            depth_view,
            pose: PoseState { yaw: 0.0 },
        }
    }

    /// Create the matching render-target obstacle texture this model draws into.
    pub fn make_target(device: &Device) -> ObstacleTexture {
        ObstacleTexture::render_target(device, TARGET_RES, TARGET_RES)
    }

    /// Advance the pose from `dt` + audio and re-raster the depth field into
    /// `target_view`. Submits its own encoder — call it during the update loop,
    /// before the particle compute pass samples the obstacle.
    pub fn render(
        &mut self,
        device: &Device,
        queue: &Queue,
        target_view: &wgpu::TextureView,
        audio: &AudioFeatures,
        dt: f32,
    ) {
        self.pose.yaw += dt * (BASE_SPIN + AUDIO_SPIN * audio.rms);
        let tilt = TILT_AMT * audio.bass;
        let model = Mat4::from_rotation_y(self.pose.yaw) * Mat4::from_rotation_x(tilt);
        let view = Mat4::look_at_rh(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, Vec3::Y);
        // Ortho box fit tight to the normalized model (unit bounding radius, so
        // z ∈ [-1,1] at every rotation) with just enough margin for splat
        // billboards. Tight near/far is deliberate: it spreads the model's
        // depth across most of [0,1], so the alpha field has real contrast and
        // the collision Threshold can carve surface relief instead of the whole
        // silhouette snapping solid at once.
        let proj = Mat4::orthographic_rh(-1.4, 1.4, -1.4, 1.4, 1.6, 4.4);

        let uniforms = ModelUniforms {
            mv: (view * model).to_cols_array_2d(),
            proj: proj.to_cols_array_2d(),
            radius_scale: 1.5,
            _pad: [0.0; 3],
        };
        queue.write_buffer(&self.uniforms_buf, 0, bytemuck::bytes_of(&uniforms));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("obstacle-model-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("obstacle-model-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
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
            pass.set_bind_group(0, &self.bind_group, &[]);
            match &self.geometry {
                ModelGeometry::Mesh {
                    vbuf,
                    ibuf,
                    index_count,
                } => {
                    pass.set_pipeline(&self.mesh_pipeline);
                    pass.set_vertex_buffer(0, vbuf.slice(..));
                    pass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..*index_count, 0, 0..1);
                }
                ModelGeometry::Splat { instances, count } => {
                    pass.set_pipeline(&self.splat_pipeline);
                    pass.set_vertex_buffer(0, instances.slice(..));
                    pass.draw(0..6, 0..*count);
                }
            }
        }
        queue.submit([encoder.finish()]);
    }
}

/// Upload `data` into a fresh vertex-style buffer via a mapped-at-creation copy
/// (no `COPY_DST`/queue round-trip needed for immutable geometry).
fn upload_buffer(
    device: &Device,
    label: &str,
    bytes: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage,
        mapped_at_creation: true,
    });
    buffer
        .slice(..)
        .get_mapped_range_mut()
        .copy_from_slice(bytes);
    buffer.unmap();
    buffer
}

/// Load a glTF/GLB file into a single merged, world-space, unit-normalized
/// triangle mesh.
fn load_mesh(device: &Device, path: &Path) -> Result<ModelGeometry, String> {
    let (doc, buffers, _images) =
        gltf::import(path).map_err(|e| format!("glTF import {}: {e}", path.display()))?;

    let mut positions: Vec<Vec3> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let scene = doc
        .default_scene()
        .or_else(|| doc.scenes().next())
        .ok_or("glTF has no scenes")?;
    for node in scene.nodes() {
        walk_node(
            &node,
            Mat4::IDENTITY,
            &buffers,
            &mut positions,
            &mut indices,
        );
    }

    if positions.is_empty() || indices.is_empty() {
        return Err("glTF model has no triangle geometry".to_string());
    }

    normalize(&mut positions);

    let vbytes: Vec<u8> = positions
        .iter()
        .flat_map(|p| [p.x, p.y, p.z])
        .flat_map(f32::to_le_bytes)
        .collect();
    let vbuf = upload_buffer(
        device,
        "obstacle-model-vbuf",
        &vbytes,
        wgpu::BufferUsages::VERTEX,
    );
    let ibuf = upload_buffer(
        device,
        "obstacle-model-ibuf",
        bytemuck::cast_slice(&indices),
        wgpu::BufferUsages::INDEX,
    );
    Ok(ModelGeometry::Mesh {
        vbuf,
        ibuf,
        index_count: indices.len() as u32,
    })
}

/// Recursively accumulate a node's (and children's) triangle geometry in world
/// space.
fn walk_node(
    node: &gltf::Node,
    parent: Mat4,
    buffers: &[gltf::buffer::Data],
    positions: &mut Vec<Vec3>,
    indices: &mut Vec<u32>,
) {
    let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
    if let Some(mesh) = node.mesh() {
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
            let Some(pos_iter) = reader.read_positions() else {
                continue;
            };
            let base = positions.len() as u32;
            for p in pos_iter {
                let w = world.transform_point3(Vec3::from(p));
                positions.push(w);
            }
            match reader.read_indices() {
                Some(idx) => indices.extend(idx.into_u32().map(|i| i + base)),
                None => {
                    // Non-indexed: sequential triangles.
                    let added = positions.len() as u32 - base;
                    indices.extend((0..added).map(|i| i + base));
                }
            }
        }
    }
    for child in node.children() {
        walk_node(&child, world, buffers, positions, indices);
    }
}

/// Recenter to the AABB center and scale so the max half-extent is 1.
fn normalize(positions: &mut [Vec3]) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for p in positions.iter() {
        min = min.min(*p);
        max = max.max(*p);
    }
    let center = (min + max) * 0.5;
    let half = ((max - min) * 0.5).max_element().max(1e-6);
    let inv = 1.0 / half;
    for p in positions.iter_mut() {
        *p = (*p - center) * inv;
    }
}

/// Load a splat `.ply`/`.splat` and pack it into an instance buffer of
/// `(center.xyz, radius)`, reusing the splat parser + its normalization.
fn load_splat(device: &Device, path: &Path) -> Result<ModelGeometry, String> {
    let progress = AtomicU8::new(0);
    let cancel = AtomicBool::new(false);
    let cloud = super::splat_source::load_splat_file(
        path,
        SPLAT_CAP,
        super::splat_source::SceneOptions::default(),
        &progress,
        &cancel,
    )?;

    let mut instances: Vec<f32> = Vec::with_capacity(cloud.count * 4);
    for i in 0..cloud.count {
        let p = cloud.positions[i];
        let s = cloud.scales[i];
        let radius = s[0].max(s[1]).max(s[2]).max(1e-4);
        // 3DGS/COLMAP clouds are Y-down; rotate 180° about X (negate Y and Z,
        // handedness-preserving) into the Y-up frame this raster's camera
        // expects, so the model stands upright rather than inverted. glTF
        // meshes are already Y-up and need no flip.
        instances.extend_from_slice(&[p[0], -p[1], -p[2], radius]);
    }
    if instances.is_empty() {
        return Err("splat scene has no points".to_string());
    }
    let bytes: Vec<u8> = instances.iter().flat_map(|v| v.to_le_bytes()).collect();
    let buffer = upload_buffer(
        device,
        "obstacle-model-splat-instances",
        &bytes,
        wgpu::BufferUsages::VERTEX,
    );
    Ok(ModelGeometry::Splat {
        instances: buffer,
        count: cloud.count as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};

    // Axis-aligned cube spanning [-1,1]^3 (already unit-normalized). 8 corners,
    // 12 triangles.
    fn cube() -> (Vec<Vec3>, Vec<u32>) {
        let v = vec![
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, -1.0),
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, 1.0, 1.0),
        ];
        #[rustfmt::skip]
        let idx = vec![
            0,1,2, 0,2,3, // -z
            4,6,5, 4,7,6, // +z
            0,4,5, 0,5,1, // -y
            3,2,6, 3,6,7, // +y
            0,3,7, 0,7,4, // -x
            1,5,6, 1,6,2, // +x
        ];
        (v, idx)
    }

    fn mesh_geometry(device: &Device, verts: &[Vec3], indices: &[u32]) -> ModelGeometry {
        let vbytes: Vec<u8> = verts
            .iter()
            .flat_map(|p| [p.x, p.y, p.z])
            .flat_map(f32::to_le_bytes)
            .collect();
        ModelGeometry::Mesh {
            vbuf: upload_buffer(device, "test-vbuf", &vbytes, wgpu::BufferUsages::VERTEX),
            ibuf: upload_buffer(
                device,
                "test-ibuf",
                bytemuck::cast_slice(indices),
                wgpu::BufferUsages::INDEX,
            ),
            index_count: indices.len() as u32,
        }
    }

    /// Render `model` once and read back the target's alpha channel (row-major,
    /// TARGET_RES²). bytes_per_row (512×4=2048) is already 256-aligned.
    fn render_alpha(
        device: &std::sync::Arc<wgpu::Device>,
        queue: &std::sync::Arc<wgpu::Queue>,
        model: &mut ObstacleModel,
        target: &ObstacleTexture,
    ) -> Vec<u8> {
        let audio = AudioFeatures::default(); // rms=0, bass=0 → yaw 0, no tilt
        model.render(device, queue, &target.view, &audio, 0.0);

        let res = TARGET_RES;
        let bpr = res * 4;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-readback"),
            size: (bpr * res) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(res),
                },
            },
            wgpu::Extent3d {
                width: res,
                height: res,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([enc.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        let rgba = slice.get_mapped_range().to_vec();
        readback.unmap();
        // Extract alpha channel (the collision height the sim samples).
        rgba.chunks_exact(4).map(|px| px[3]).collect()
    }

    fn at(alpha: &[u8], x: u32, y: u32) -> u8 {
        alpha[(y * TARGET_RES + x) as usize]
    }

    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn obstacle_model_mesh_depth_field() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let (verts, idx) = cube();
        let mut model = ObstacleModel::from_geometry(&device, mesh_geometry(&device, &verts, &idx));
        let target = ObstacleModel::make_target(&device);
        let alpha = render_alpha(&device, &queue, &mut model, &target);

        let c = TARGET_RES / 2;
        let center = at(&alpha, c, c);
        // Front face is near-bright but NOT saturated (graded depth, not a flat
        // silhouette): world z=+1 → normalized depth 0.222 → alpha ≈ 0.778.
        assert!(
            center > 150,
            "cube front-face alpha should be near-bright, got {center}"
        );
        assert!(
            center < 250,
            "cube front-face alpha should be graded (<1.0), got {center}"
        );
        // Corners are background — cleared to 0.
        assert_eq!(at(&alpha, 8, 8), 0, "top-left corner should be empty");
        assert_eq!(
            at(&alpha, TARGET_RES - 8, TARGET_RES - 8),
            0,
            "bottom-right corner should be empty"
        );

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }

    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn obstacle_model_splat_depth_field() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        device.push_error_scope(wgpu::ErrorFilter::Validation);

        // Three overlapping splats near the origin (center, radius) — a small
        // disc cluster in the middle of the field.
        let inst: Vec<f32> = vec![
            0.0, 0.0, 0.0, 0.4, //
            0.2, 0.0, 0.1, 0.3, //
            -0.15, 0.1, 0.0, 0.3,
        ];
        let bytes: Vec<u8> = inst.iter().flat_map(|v| v.to_le_bytes()).collect();
        let geom = ModelGeometry::Splat {
            instances: upload_buffer(&device, "test-splat", &bytes, wgpu::BufferUsages::VERTEX),
            count: 3,
        };
        let mut model = ObstacleModel::from_geometry(&device, geom);
        let target = ObstacleModel::make_target(&device);
        let alpha = render_alpha(&device, &queue, &mut model, &target);

        let c = TARGET_RES / 2;
        assert!(
            at(&alpha, c, c) > 120,
            "splat cluster center should be covered, got {}",
            at(&alpha, c, c)
        );
        // The round-quad discard leaves the far corners empty.
        assert_eq!(at(&alpha, 8, 8), 0, "corner should be empty");

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }
}
