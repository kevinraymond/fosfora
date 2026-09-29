//! One Fosfora effect run through the core's offline scene renderer, driven
//! by live or synthetic audio features, in one of two outputs:
//!
//! - **Quad** (S4): `SceneRenderer` is the app's render loop minus the window
//!   and devices; it post-processes into its own capture texture, which has
//!   no sampling usage, so each frame ends with a copy into `quad`, the
//!   texture the world-locked quad samples.
//! - **World** (C3b, `mode world`): a world-layout effect (a hidden
//!   `assets/xr/effects/*_xr_world.pfx`) whose sim writes meters around an
//!   anchor. The scene renderer still owns the per-frame state (uniforms,
//!   audio, bindings, timeline, counter readbacks, the ping-pong flip), but
//!   the effect's layer is disabled so none of its 2D frame runs (fragment
//!   passes, the instanced 2D particle draw, compositing: on the Adreno 740
//!   an instanced draw of a few hundred thousand sprites costs milliseconds,
//!   and nothing of that frame is ever shown). The sim is dispatched here
//!   instead, and `ParticleSystem::render_world` draws it once per eye into
//!   the eye targets.

use std::path::Path;

use anyhow::{Context, Result, bail};
use fosfora_app::audio::AudioFrame;
use fosfora_app::audio::analyzer::{SPECTROGRAM_MELS, SPECTRUM_BINS};
use fosfora_app::audio::hop::HopOutput;
use fosfora_app::gpu::audio_textures::WAVEFORM_PEEK;
use fosfora_app::gpu::layer::Layer;
use fosfora_app::gpu::particle::types::ParticleAux;
use fosfora_app::gpu::particle::{ParticleSystem, WorldCamera, WorldDraw};
use fosfora_app::headless::scene_renderer::{CAPTURE_FORMAT, SceneRenderer};
use fosfora_app::settings::ParticleQuality;
use log::info;

use crate::gfx::{EyeCamera, SWAPCHAIN_FORMAT};
use crate::particles3d::{DEPTH_FORMAT, ObstacleSet, WARMUP_FRAMES};
use crate::surfaces::SurfaceWeights;

/// Simulated tempo for the synthetic features.
const BPM: f64 = 120.0;
/// Offscreen size in world mode. Nothing renders into it (the layer is
/// disabled), but the renderer allocates its compositor and capture at this
/// size, so keep it small.
const WORLD_SCENE_SIZE: u32 = 64;

pub struct XrScene {
    renderer: SceneRenderer,
    /// The texture the S4 quad samples; `None` in world mode.
    pub quad: Option<(wgpu::Texture, wgpu::TextureView)>,
    pub width: u32,
    pub height: u32,
    frame: u32,
    fps: u32,
    waveform: Vec<f32>,
    world: Option<World>,
}

/// Rows of the world-mode aux block: the head, the obstacle block,
/// Murmur's per-hand lanes, then Flux's instrument rows. The obstacle block
/// ends at row 163 (`XR_AUX_*` in `assets/xr/shaders/flux_xr_sim.wgsl`),
/// the lanes at 170 (`XR_AUX_END` in `murmur_xr_sim.wgsl`), the
/// instruments at 173 (`XR_AUX_INSTRUMENTS` + `XR_AUX_INSTRUMENT_ROWS` in
/// the Flux sim); core tests pin the sims' side.
const OBSTACLE_END: usize = 1 + std::mem::size_of::<ObstacleSet>() / 16;
const _: () = assert!(OBSTACLE_END == 163, "flux_xr_sim.wgsl reads 163 aux rows");
const HAND_LANES_END: usize = OBSTACLE_END + crate::pose::HAND_LANE_ROWS;
const _: () = assert!(
    HAND_LANES_END == 170,
    "murmur_xr_sim.wgsl reads 170 aux rows"
);
const WORLD_AUX_ROWS: usize = HAND_LANES_END + crate::instruments::INSTRUMENT_ROWS;
const _: () = assert!(
    WORLD_AUX_ROWS == 173,
    "flux_xr_sim.wgsl reads the instrument rows 170..173"
);

/// World-mode state (see the module docs).
struct World {
    /// Effect anchor in the reference space: the sim's origin.
    anchor: [f32; 3],
    /// False freezes the sim after `warmup` dispatches, so a sweep can
    /// isolate the draw (`debug.fosfora.sim 0`).
    sim_enabled: bool,
    /// Dispatches before a frozen sim stops: long enough for the emitter to
    /// fill the particle count, and never shorter than the S5 test sim's.
    warmup: u32,
    dispatches: u32,
    /// Per-frame inputs for the sim, uploaded into the head of the effect's
    /// aux buffer (layout in `assets/xr/shaders/flux_xr_sim.wgsl`).
    aux: Vec<ParticleAux>,
    /// Half edge of the sim's volume around the anchor (the preset's
    /// `emitter.radius`, floored as the sim floors it): a surface whose top
    /// face does not reach into it gets emitter weight 0.
    emitter_half: f32,
    /// This frame's summed emitter weight (for the log).
    emitter_weight: f32,
    /// The particle count (`max_count`): with the alive count, how many
    /// dead slots a burst can claim.
    max_count: u32,
}

/// How a world-mode effect is set up for this run.
#[derive(Debug, Clone, Copy)]
pub struct WorldOptions {
    /// Particle count (`max_count`); `None` keeps the preset's.
    pub count: Option<u32>,
    /// Sprite radius multiplier (initial and end size).
    pub size_scale: f32,
    /// See `World::sim_enabled`.
    pub sim_enabled: bool,
    /// Initial anchor in the reference space.
    pub anchor: [f32; 3],
}

impl XrScene {
    /// S4: `width` x `height` offscreen, copied each frame into `quad`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        quality: ParticleQuality,
        scene_dir: &Path,
        fps: u32,
    ) -> Result<Self> {
        let renderer =
            start_renderer(device, queue, width, height, quality, scene_dir, |_| Ok(()))?;
        let quad_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xr-quad-source"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CAPTURE_FORMAT,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let quad_view = quad_texture.create_view(&wgpu::TextureViewDescriptor::default());

        Ok(Self {
            renderer,
            quad: Some((quad_texture, quad_view)),
            width,
            height,
            frame: 0,
            fps,
            waveform: vec![0.0; WAVEFORM_PEEK],
            world: None,
        })
    }

    /// C3b: the world-layout effect `effect` (a hidden XR preset), simulated
    /// around `options.anchor` and drawn by [`Self::render_world`].
    pub fn new_world(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene_dir: &Path,
        effect: &str,
        fps: u32,
        options: WorldOptions,
    ) -> Result<Self> {
        let mut warmup = WARMUP_FRAMES;
        let mut emitter_half = 0.0;
        let mut max_count = 0;
        // `count` is the particle count itself, so no quality scaling.
        let mut renderer = start_renderer(
            device,
            queue,
            WORLD_SCENE_SIZE,
            WORLD_SCENE_SIZE,
            ParticleQuality::High,
            scene_dir,
            |renderer| {
                let Some(pfx) = renderer
                    .effect_loader
                    .effects
                    .iter_mut()
                    .find(|e| e.name == effect)
                else {
                    bail!("effect '{effect}' not found");
                };
                let Some(particles) = pfx.particles.as_mut() else {
                    bail!("effect '{effect}' has no particle system");
                };
                if let Some(count) = options.count {
                    // The aux buffer holds one row per particle and the XR
                    // inputs need `WORLD_AUX_ROWS` of it (`update_aux_in_place`
                    // drops the upload otherwise, silently losing the
                    // obstacles), so the count never goes below that.
                    let count = count.max(WORLD_AUX_ROWS as u32);
                    // Keep the preset's fill: emission scales with the count.
                    particles.emit_rate *= count as f32 / particles.max_count.max(1) as f32;
                    particles.max_count = count;
                    particles.max_scaled_count = 0;
                }
                // `max(u.emitter_radius, 0.05)` in flux_xr_sim.wgsl.
                emitter_half = particles.emitter.radius.max(0.05);
                max_count = particles.max_count;
                particles.initial_size *= options.size_scale;
                particles.size_end *= options.size_scale;
                // Fill time at the emission rate, plus a quarter for the
                // particles that die and respawn meanwhile.
                let fill_s = particles.max_count as f32 / particles.emit_rate.max(1.0);
                warmup = warmup.max((fill_s * 1.25 * fps as f32).ceil() as u32);
                info!(
                    "world effect '{effect}': {} particles, emit {}/s, sprite radius {} -> {} m",
                    particles.max_count,
                    particles.emit_rate,
                    particles.initial_size,
                    particles.size_end
                );
                Ok(())
            },
        )?;
        // The layer stays loaded (its particle system is ours to drive) but
        // disabled, so `SceneRenderer::step` renders nothing of it.
        let mut found = false;
        for layer in &mut renderer.layer_stack.layers {
            if layer
                .as_effect()
                .is_some_and(|e| e.pass_executor.particle_system.is_some())
            {
                layer.enabled = false;
                found = true;
            }
        }
        if !found {
            bail!("effect '{effect}' built no particle system");
        }
        Ok(Self {
            renderer,
            quad: None,
            width: WORLD_SCENE_SIZE,
            height: WORLD_SCENE_SIZE,
            frame: 0,
            fps,
            waveform: vec![0.0; WAVEFORM_PEEK],
            world: Some(World {
                anchor: options.anchor,
                sim_enabled: options.sim_enabled,
                warmup,
                dispatches: 0,
                aux: Vec::new(),
                emitter_half,
                emitter_weight: 0.0,
                max_count,
            }),
        })
    }

    /// World mode: the summed emitter weight of the boxes last uploaded (0
    /// keeps a surface-mode sim on its volume emitter).
    pub fn emitter_weight(&self) -> f32 {
        self.world.as_ref().map_or(0.0, |w| w.emitter_weight)
    }

    /// Whether this scene draws through [`Self::render_world`] (else the quad).
    pub fn is_world(&self) -> bool {
        self.world.is_some()
    }

    /// Move the effect's anchor (reference space, meters). The sim's
    /// positions are relative to it, so the whole cloud moves with it.
    pub fn set_anchor(&mut self, anchor: [f32; 3]) {
        if let Some(world) = self.world.as_mut() {
            world.anchor = anchor;
        }
    }

    /// Upload this frame's world-mode sim inputs: the wearer's head (for the
    /// near fade within `near_fade_m`; 0 = off), the settle drift
    /// (`drift_m_s` downward; 0 = none), the obstacles with their emitter
    /// weights (`surfaces`, from the kinds and flags the boxes carry and
    /// where they sit against this effect's volume), Murmur's per-hand
    /// lanes (`pose::lane_rows`, already in the anchor's frame) and the
    /// instrument rows (`instruments::rows`, likewise; their steal
    /// fraction is set here from this sim's alive count), the positions
    /// moved into the anchor's frame. Call before [`Self::dispatch_world`].
    #[allow(
        clippy::too_many_arguments,
        reason = "one call per frame, each argument a separate input"
    )]
    pub fn set_world_inputs(
        &mut self,
        queue: &wgpu::Queue,
        head: [f32; 3],
        near_fade_m: f32,
        drift_m_s: f32,
        hand_scare: f32,
        obstacles: &ObstacleSet,
        surfaces: SurfaceWeights,
        hand_lanes: &[[f32; 4]; crate::pose::HAND_LANE_ROWS],
        instruments: &[[f32; 4]; crate::instruments::INSTRUMENT_ROWS],
    ) {
        let Some(world) = self.world.as_mut() else {
            return;
        };
        let a = world.anchor;
        world.aux.clear();
        world.aux.reserve(WORLD_AUX_ROWS);
        world.aux.push(ParticleAux {
            home: [head[0] - a[0], head[1] - a[1], head[2] - a[2], near_fade_m],
        });
        // The weights are taken against the volume around today's anchor,
        // so a dragged anchor never emits from a surface it has left.
        let mut relative = obstacles.relative_to(a);
        relative.set_emitter_weights(world.emitter_half, surfaces);
        world.emitter_weight = relative.emitter_weight_sum();
        world
            .aux
            .extend(relative.as_vec4s().iter().map(|&home| ParticleAux { home }));
        // The obstacle block's second header row has two unused lanes; the
        // sim reads the drift from the third (`flux_xr_sim.wgsl`, aux[2].z).
        world.aux[2].home[2] = drift_m_s;
        // The fourth lane: how calm the hands leave Murmur's flock, 1 -
        // scare, so a writer that never sets it keeps the full scare. Flux
        // ignores it.
        world.aux[2].home[3] = 1.0 - hand_scare.clamp(0.0, 1.0);
        world
            .aux
            .extend(hand_lanes.iter().map(|&home| ParticleAux { home }));
        let mut instruments = *instruments;
        let alive =
            particle_system(&mut self.renderer.layer_stack.layers).map_or(0, |ps| ps.alive_count);
        instruments[0][3] =
            crate::instruments::steal_fraction(instruments[0][0].to_bits(), alive, world.max_count);
        world
            .aux
            .extend(instruments.iter().map(|&home| ParticleAux { home }));
        debug_assert_eq!(world.aux.len(), WORLD_AUX_ROWS);
        if let Some(ps) = particle_system(&mut self.renderer.layer_stack.layers) {
            ps.update_aux_in_place(queue, &world.aux);
        }
    }

    /// World mode: record this frame's sim dispatch (after [`Self::step`],
    /// before the eye passes). Draws nothing by itself.
    pub fn dispatch_world(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(world) = self.world.as_mut() else {
            return;
        };
        if !world.sim_enabled && world.dispatches >= world.warmup {
            return;
        }
        world.dispatches = world.dispatches.saturating_add(1);
        let queue = &self.renderer.queue;
        if let Some(ps) = particle_system(&mut self.renderer.layer_stack.layers) {
            // `step` requested the counter map before this dispatch exists;
            // collecting it now (it is usually complete by the renderer's
            // poll) lets the dispatch copy this frame's count, so the alive
            // count in the log keeps moving. Nothing reads it but the log.
            ps.poll_counter_readback();
            ps.dispatch(encoder, queue);
        }
    }

    /// World mode, before an eye pass: the pipeline and camera for drawing
    /// the effect into that eye with [`Self::draw_world`]. `None` outside
    /// world mode.
    pub fn prepare_world(
        &mut self,
        device: &wgpu::Device,
        camera: &EyeCamera,
    ) -> Option<WorldDraw> {
        let anchor = self.world.as_ref().map(|w| w.anchor)?;
        let queue = &self.renderer.queue;
        let ps = particle_system(&mut self.renderer.layer_stack.layers)?;
        Some(ps.prepare_world(
            device,
            queue,
            SWAPCHAIN_FORMAT,
            Some(DEPTH_FORMAT),
            &WorldCamera {
                view: camera.view,
                proj: camera.proj,
                anchor: anchor.into(),
            },
        ))
    }

    /// World mode, inside the eye pass after the occluders and the primer:
    /// draw the effect (depth tested against them, never written). The
    /// sprites share the pass with the depth-writing draws on purpose: on
    /// the Adreno a sprite pass without one costs up to 3x, and passthrough
    /// several milliseconds more (`docs/xr/MEASURED.md`, S7 and C3b).
    pub fn draw_world(&mut self, pass: &mut wgpu::RenderPass<'_>, draw: WorldDraw) {
        if let Some(ps) = particle_system(&mut self.renderer.layer_stack.layers) {
            ps.draw_world(pass, draw);
        }
    }

    /// Override every particle system's emission rate (particles per second).
    /// Steady-state count is roughly rate x lifetime. Spike knob for the
    /// count sweep.
    pub fn set_emit_rate(&mut self, rate: f32) {
        for layer in &mut self.renderer.layer_stack.layers {
            if let Some(effect) = layer.as_effect_mut() {
                if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                    info!("emit rate {} -> {rate} particles/s", ps.emit_rate);
                    ps.emit_rate = rate;
                }
            }
        }
    }

    /// Alive particles as of the last counter readback (first particle layer).
    pub fn alive_count(&self) -> u32 {
        self.renderer
            .layer_stack
            .layers
            .iter()
            .filter_map(|l| l.as_effect())
            .filter_map(|e| e.pass_executor.particle_system.as_ref())
            .map(|ps| ps.alive_count)
            .next()
            .unwrap_or(0)
    }

    /// The synthetic groove for this frame (S4), with a matching fake
    /// waveform.
    pub fn synth(&mut self, ts: f64) -> HopOutput {
        let hop = synth_hop(self.frame, self.fps, ts);
        self.frame = self.frame.wrapping_add(1);
        let rms = hop.frame.features.rms;
        for (i, s) in self.waveform.iter_mut().enumerate() {
            *s = rms
                * ((ts * 110.0 + i as f64 * 110.0 / 48_000.0) * std::f64::consts::TAU).sin() as f32;
        }
        hop
    }

    /// The synthetic waveform from the last `synth` call.
    pub fn synth_waveform(&self) -> &[f32] {
        &self.waveform
    }

    /// Advance the effect by one frame on `hop`: in quad mode render it and
    /// refresh the quad texture; in world mode update its per-frame state
    /// only (the sim runs in [`Self::dispatch_world`]). Submits its own
    /// command buffers; call before recording the eye passes.
    pub fn step(&mut self, ts: f64, dt: f32, hop: &HopOutput, waveform: &[f32]) {
        let Some((quad_texture, _)) = &self.quad else {
            self.renderer.step(ts, dt, hop, waveform, false);
            return;
        };
        self.renderer.step(ts, dt, hop, waveform, true);

        let mut encoder =
            self.renderer
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xr-quad-copy"),
                });
        encoder.copy_texture_to_texture(
            self.renderer.capture.texture.as_image_copy(),
            quad_texture.as_image_copy(),
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.renderer.queue.submit([encoder.finish()]);
    }
}

/// Build the scene renderer for `scene_dir`, let `tweak` adjust the loaded
/// effect definitions (before any layer is built from them), then install
/// and start the scene.
fn start_renderer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    width: u32,
    height: u32,
    quality: ParticleQuality,
    scene_dir: &Path,
    tweak: impl FnOnce(&mut SceneRenderer) -> Result<()>,
) -> Result<SceneRenderer> {
    let mut renderer = SceneRenderer::new(
        device.clone(),
        queue.clone(),
        width,
        height,
        quality,
        scene_dir.to_path_buf(),
    )
    .context("SceneRenderer::new")?;
    info!(
        "effects loaded: {} ({})",
        renderer.effect_loader.effects.len(),
        renderer
            .effect_loader
            .effects
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    tweak(&mut renderer)?;
    let loaded = fosfora_app::headless::load::load_scene_dir(scene_dir)
        .with_context(|| format!("loading scene {}", scene_dir.display()))?;
    info!(
        "scene '{}': {} presets, {} cues",
        loaded.scene.name,
        loaded.presets.len(),
        loaded.scene.cues.len()
    );
    renderer.install_scene(loaded);
    renderer.start();
    for w in &renderer.warnings {
        log::warn!("scene renderer: {w}");
    }
    Ok(renderer)
}

/// The first effect layer's particle system.
fn particle_system(layers: &mut [Layer]) -> Option<&mut ParticleSystem> {
    layers
        .iter_mut()
        .filter_map(|l| l.as_effect_mut())
        .find_map(|e| e.pass_executor.particle_system.as_mut())
}

/// A steady 120 BPM groove: the core's synthetic beat grid plus band energies
/// that pump on the beat and drift slowly, so Flux has bass, onsets, rms and
/// a moving centroid to react to.
pub fn synth_hop(frame: u32, fps: u32, ts: f64) -> HopOutput {
    let mut f = fosfora_app::headless::loop_driver::synth_features(frame, fps, BPM);
    let beat_env = (-f.beat_phase * 5.0).exp();
    let slow = (ts * 0.25).sin() as f32 * 0.5 + 0.5;
    f.onset = beat_env;
    f.kick = beat_env;
    f.sub_bass = 0.25 + 0.6 * beat_env;
    f.bass = 0.3 + 0.55 * beat_env;
    f.low_mid = 0.3 + 0.2 * slow;
    f.mid = 0.3 + 0.4 * slow;
    f.upper_mid = 0.2 + 0.3 * (1.0 - slow);
    f.presence = 0.15 + 0.3 * beat_env * (1.0 - slow);
    f.brilliance = 0.1 + 0.2 * beat_env;
    f.rms = 0.35 + 0.35 * beat_env;
    f.centroid = 0.3 + 0.4 * slow;
    f.flux = 0.2 + 0.6 * beat_env;
    f.beat_strength = 0.8;
    f.dominant_chroma = ((ts / 8.0) as f32).fract();

    let spectrum: Box<[f32]> = (0..SPECTRUM_BINS)
        .map(|i| {
            let x = i as f32 / SPECTRUM_BINS as f32;
            ((-x * 6.0).exp() * (0.4 + 0.6 * beat_env)
                + 0.15 * slow * (-(x - 0.5).powi(2) * 40.0).exp())
            .min(1.0)
        })
        .collect();
    let mel: Box<[f32]> = (0..SPECTROGRAM_MELS)
        .map(|i| {
            let x = i as f32 / SPECTROGRAM_MELS as f32;
            ((-x * 4.0).exp() * (0.4 + 0.6 * beat_env)).min(1.0)
        })
        .collect();

    HopOutput {
        frame: AudioFrame {
            features: f,
            spectrum,
            mel,
            dmfcc: [0.0; 13],
            timestamp: ts,
            phase_frozen: false,
            bar_duration: 4.0 * 60.0 / BPM,
            beat_time: None,
            section_boundary: None,
        },
        beat_fired: f.beat > 0.5,
        downbeat_fired: f.downbeat > 0.5,
        drop_fired: false,
        pre_norm: f,
    }
}
