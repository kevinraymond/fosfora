use wgpu::{Device, Queue};

/// 2D obstacle texture for particle collision.
/// Stores alpha-channel shape data. Particles test alpha against a threshold
/// and respond with bounce/stick/flow-around behavior.
pub struct ObstacleTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub width: u32,
    pub height: u32,
}

impl ObstacleTexture {
    /// Create a 1x1 transparent placeholder (no collision).
    pub fn placeholder(device: &Device, queue: &Queue) -> Self {
        let size = wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("obstacle-placeholder"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("obstacle-placeholder-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Transparent black
        let data = [0u8; 4];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            size,
        );

        Self {
            texture,
            view,
            sampler,
            width: 1,
            height: 1,
        }
    }

    /// Create an empty render-target obstacle (`RENDER_ATTACHMENT | TEXTURE_BINDING`).
    ///
    /// Used by the 3D-model obstacle source (#1851): a render pass draws the
    /// posed model's depth into this texture every frame, and the particle sim
    /// samples it as an ordinary obstacle field. No `COPY_DST` — it is written
    /// only by rendering, never by `write_texture`.
    pub fn render_target(device: &Device, w: u32, h: u32) -> Self {
        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("obstacle-model-target"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            // COPY_SRC so the field can be read back (offscreen probe / debug snapshot).
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("obstacle-model-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            texture,
            view,
            sampler,
            width: w,
            height: h,
        }
    }

    /// Create from RGBA byte data.
    ///
    /// If the image has no meaningful alpha variation (e.g. JPEG, opaque PNG
    /// where all pixels are alpha=255), luminance is written into the alpha
    /// channel so the obstacle shape comes from brightness instead.
    pub fn from_rgba(device: &Device, queue: &Queue, data: &[u8], w: u32, h: u32) -> Self {
        let processed = preprocess_alpha(data);

        let size = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("obstacle-texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("obstacle-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &processed,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            size,
        );

        Self {
            texture,
            view,
            sampler,
            width: w,
            height: h,
        }
    }

    /// Update texture data in-place (for webcam per-frame updates).
    /// If dimensions match, reuses existing texture. Otherwise recreates.
    pub fn update(&mut self, device: &Device, queue: &Queue, data: &[u8], w: u32, h: u32) {
        let processed = preprocess_alpha(data);
        if w == self.width && h == self.height {
            // Same size — just write new data
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &processed,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        } else {
            // Different size — recreate
            *self = Self::from_rgba(device, queue, data, w, h);
        }
    }
}

/// If the image has no meaningful alpha (all pixels ≥ 250), replace alpha
/// with luminance so opaque images (JPEG, opaque PNG) work as obstacles
/// based on their brightness.
fn preprocess_alpha(data: &[u8]) -> Vec<u8> {
    // Check if alpha channel has meaningful variation
    let has_alpha = data.chunks_exact(4).any(|px| px[3] < 250);

    if has_alpha {
        // Image has real alpha — use as-is
        data.to_vec()
    } else {
        // No alpha variation — write luminance into alpha channel
        let mut out = data.to_vec();
        for px in out.chunks_exact_mut(4) {
            let lum = (px[0] as f32 * 0.299 + px[1] as f32 * 0.587 + px[2] as f32 * 0.114) as u8;
            px[3] = lum;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};

    #[test]
    fn preprocess_alpha_preserves_real_alpha() {
        // Image with transparent pixels — alpha should be preserved
        let data = vec![
            255, 0, 0, 128, // semi-transparent red
            0, 255, 0, 0, // fully transparent green
        ];
        let result = preprocess_alpha(&data);
        assert_eq!(result[3], 128);
        assert_eq!(result[7], 0);
    }

    #[test]
    fn preprocess_alpha_uses_luminance_for_opaque() {
        // All alpha=255 — should replace with luminance
        let data = vec![
            255, 255, 255, 255, // white → lum ≈ 255
            0, 0, 0, 255, // black → lum = 0
        ];
        let result = preprocess_alpha(&data);
        // White pixel should have high alpha
        assert!(result[3] > 200);
        // Black pixel should have zero alpha
        assert_eq!(result[7], 0);
    }

    // --- GPU probe: obstacle field is aspect-correct in screen space (#1790) ---
    //
    // Runs the real particle_lib.wgsl obstacle functions on a non-16:9
    // 1000×500 "viewport" against a 200×200 circle texture and asserts, per
    // fit mode, that the alpha region and collision normals are correct in
    // SCREEN space. Fails under any of the four legacy defects (stretched
    // UVs, anisotropic gradients, y-inverted normals, squashed fit).
    // Run: cargo test -p fosfora-app -- --ignored obstacle_gpu_probe
    // (lavapipe works: it validates + executes the edited lib headlessly).

    const PROBE_W: f32 = 1000.0;
    const PROBE_H: f32 = 500.0;

    /// Screen-space (x right, y up, pixels) → clip-space probe position.
    fn clip(sx: f32, sy_up: f32) -> [f32; 4] {
        [
            sx / PROBE_W * 2.0 - 1.0,
            sy_up / PROBE_H * 2.0 - 1.0,
            0.0,
            0.0,
        ]
    }

    struct ObstacleProbe {
        device: std::sync::Arc<wgpu::Device>,
        queue: std::sync::Arc<wgpu::Queue>,
        pipeline: wgpu::ComputePipeline,
        uniform_buf: wgpu::Buffer,
        in_buf: wgpu::Buffer,
        out_buf: wgpu::Buffer,
        staging: wgpu::Buffer,
        bg0: wgpu::BindGroup,
        bg1: wgpu::BindGroup,
        capacity: usize,
    }

    impl ObstacleProbe {
        /// Evaluate (obstacle_alpha, obstacle_normal) at each clip-space probe
        /// position under the given fit mode. Returns (alpha, nx, ny) tuples.
        fn run(&self, fit: u32, probes: &[[f32; 4]]) -> Vec<(f32, f32, f32)> {
            assert!(probes.len() <= self.capacity);
            use bytemuck::Zeroable;
            let mut uniforms = super::super::types::ParticleUniforms::zeroed();
            uniforms.resolution = [PROBE_W, PROBE_H];
            uniforms.obstacle_enabled = 1.0;
            uniforms.obstacle_threshold = 0.5;
            uniforms.obstacle_fit = fit;
            self.queue
                .write_buffer(&self.uniform_buf, 0, bytemuck::bytes_of(&uniforms));
            self.queue
                .write_buffer(&self.in_buf, 0, bytemuck::cast_slice(probes));

            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("obstacle-probe"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bg0, &[]);
                pass.set_bind_group(1, &self.bg1, &[]);
                pass.dispatch_workgroups(probes.len().div_ceil(64) as u32, 1, 1);
            }
            let bytes = (probes.len() * 16) as u64;
            encoder.copy_buffer_to_buffer(&self.out_buf, 0, &self.staging, 0, bytes);
            self.queue.submit([encoder.finish()]);

            let slice = self.staging.slice(..bytes);
            slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
            self.device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let data: Vec<[f32; 4]> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
            self.staging.unmap();
            data.iter().map(|v| (v[0], v[1], v[2])).collect()
        }
    }

    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn obstacle_gpu_probe() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        // 200×200 circle, radius 80 texels, alpha feathered over ~3 texels —
        // a hard binary edge under bilinear sampling gives staircase
        // gradients; the feather keeps the normals testing the fit/space
        // math, not sampling aliasing. Uploaded through the production path
        // (also exercises preprocess_alpha).
        let (tw, th, r_tex) = (200u32, 200u32, 80.0f32);
        let mut rgba = vec![0u8; (tw * th * 4) as usize];
        for y in 0..th {
            for x in 0..tw {
                let (dx, dy) = (x as f32 + 0.5 - 100.0, y as f32 + 0.5 - 100.0);
                let d = (dx * dx + dy * dy).sqrt();
                let a = ((r_tex + 1.5 - d) / 3.0).clamp(0.0, 1.0);
                if a > 0.0 {
                    let i = ((y * tw + x) * 4) as usize;
                    let v = (a * 255.0) as u8;
                    rgba[i..i + 4].copy_from_slice(&[v, v, v, v]);
                }
            }
        }
        let obstacle = ObstacleTexture::from_rgba(&device, &queue, &rgba, tw, th);

        // Probe kernel appended to the real libs, concatenated in the same
        // order as prepend_compute_libraries (noise first — particle_lib's
        // curl helpers call fosfora_noise2). Writes (alpha, normal.xy).
        let noise = include_str!("../../../../../assets/shaders/lib/noise.wgsl");
        let lib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
        let src = format!(
            "{noise}\n{lib}\n\
             @compute @workgroup_size(64)\n\
             fn probe_main(@builtin(global_invocation_id) gid: vec3u) {{\n\
                 let idx = gid.x;\n\
                 if idx >= arrayLength(&pos_life_out) {{ return; }}\n\
                 let pos = pos_life_in[idx].xy;\n\
                 let a = obstacle_alpha(pos);\n\
                 let n = obstacle_normal(pos);\n\
                 pos_life_out[idx] = vec4f(a, n.x, n.y, 0.0);\n\
             }}\n"
        );
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("obstacle-probe"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("obstacle-probe"),
            layout: None,
            module: &module,
            entry_point: Some("probe_main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let capacity = 4096usize;
        let storage = |usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (capacity * 16) as u64,
                usage,
                mapped_at_creation: false,
            })
        };
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("obstacle-probe-uniforms"),
            size: std::mem::size_of::<super::super::types::ParticleUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let in_buf = storage(wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST);
        let out_buf = storage(wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
        let staging = storage(wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST);

        // Auto layout: group 0 holds only the statically-used u/in/out,
        // group 1 only the obstacle texture + sampler.
        let bg0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: in_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: out_buf.as_entire_binding(),
                },
            ],
        });
        // Water is off in this probe: bind a 1×1 zero at binding 4 so the
        // collision's h_eff == terrain alpha (regression guard for #1851).
        let (_water_tex, water_view) = crate::gpu::particle::water::placeholder(&device, &queue);
        let bg1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(1),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&obstacle.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&obstacle.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&water_view),
                },
            ],
        });

        let probe = ObstacleProbe {
            device,
            queue,
            pipeline,
            uniform_buf,
            in_buf,
            out_buf,
            staging,
            bg0,
            bg1,
            capacity,
        };

        // === Fit (Contain, 1): circle → centered 500×500 px square, screen
        // radius 200 px at center (500, 250). Must be a CIRCLE in screen
        // space — legacy stretch renders it as a 400×200 ellipse instead.
        let (cx, cy, r) = (500.0f32, 250.0f32, 200.0f32);
        let margin = 25.0f32;
        let mut grid = Vec::new();
        let mut expect_inside = Vec::new();
        for gy in 0..21 {
            for gx in 0..41 {
                let (sx, sy) = (gx as f32 * 25.0, gy as f32 * 25.0);
                let d = ((sx - cx).powi(2) + (sy - cy).powi(2)).sqrt();
                if (d - r).abs() <= margin {
                    continue; // skip the fuzzy bilinear band at the edge
                }
                grid.push(clip(sx, sy));
                expect_inside.push(d < r);
            }
        }
        let results = probe.run(1, &grid);
        for (i, (a, _, _)) in results.iter().enumerate() {
            let inside = *a >= 0.5;
            assert_eq!(
                inside, expect_inside[i],
                "fit=Contain probe {i} at clip {:?}: alpha={a}",
                grid[i]
            );
        }

        // Normals on the circle's surface must point radially outward in
        // screen space (unit length). Fails under the anisotropy defect
        // (wrong angle off-axis) and the y-inversion defect (sign-flipped).
        let ring: Vec<[f32; 4]> = (0..256)
            .map(|k| {
                let th = k as f32 / 256.0 * std::f32::consts::TAU;
                clip(cx + r * th.cos(), cy + r * th.sin())
            })
            .collect();
        let results = probe.run(1, &ring);
        for (k, (_, nx, ny)) in results.iter().enumerate() {
            let th = k as f32 / 256.0 * std::f32::consts::TAU;
            let dot = nx * th.cos() + ny * th.sin();
            let len = (nx * nx + ny * ny).sqrt();
            assert!(
                (len - 1.0).abs() < 0.01,
                "ring {k}: normal not unit length: |n|={len}"
            );
            assert!(
                dot > 0.9,
                "ring {k} (θ={th:.2}): normal ({nx:.3},{ny:.3}) not radial (dot={dot:.3})"
            );
        }

        // === Fill (Cover, 2): scale 5 → screen radius 400 px, vertically
        // cropped. Check the horizontal chord through the center row.
        let chord: Vec<[f32; 4]> = (0..40).map(|k| clip(k as f32 * 25.0, cy)).collect();
        let results = probe.run(2, &chord);
        for (k, (a, _, _)) in results.iter().enumerate() {
            let d = (k as f32 * 25.0 - cx).abs();
            if (d - 400.0).abs() <= margin {
                continue;
            }
            assert_eq!(
                *a >= 0.5,
                d < 400.0,
                "fit=Cover chord probe {k} at x={} px: alpha={a}",
                k * 25
            );
        }

        // === Stretch (0, legacy): sanity-check the distorted ellipse
        // (semi-axes 400×200 px) still behaves as before the fix.
        let legacy = [
            (clip(cx + 350.0, cy), true),  // inside horizontally (350 < 400)
            (clip(cx + 430.0, cy), false), // outside horizontally
            (clip(cx, cy + 170.0), true),  // inside vertically (170 < 200)
            (clip(cx, cy + 235.0), false), // outside vertically
        ];
        let positions: Vec<[f32; 4]> = legacy.iter().map(|(p, _)| *p).collect();
        let results = probe.run(0, &positions);
        for (k, (a, _, _)) in results.iter().enumerate() {
            assert_eq!(
                *a >= 0.5,
                legacy[k].1,
                "fit=Stretch legacy probe {k}: alpha={a}"
            );
        }

        let err = pollster::block_on(probe.device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }
}
