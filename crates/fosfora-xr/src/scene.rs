//! One Fosfora effect rendered through the core's offline scene renderer into
//! an offscreen texture, driven by synthetic audio features (S4).
//!
//! `SceneRenderer` is the app's render loop minus the window and devices; it
//! post-processes into its own capture texture, which has no sampling usage,
//! so each frame ends with a copy into `quad_texture`, the texture the
//! world-locked quad samples.

use std::path::Path;

use anyhow::{Context, Result};
use fosfora_app::audio::AudioFrame;
use fosfora_app::audio::analyzer::{SPECTROGRAM_MELS, SPECTRUM_BINS};
use fosfora_app::audio::hop::HopOutput;
use fosfora_app::gpu::audio_textures::WAVEFORM_PEEK;
use fosfora_app::headless::scene_renderer::{CAPTURE_FORMAT, SceneRenderer};
use fosfora_app::settings::ParticleQuality;
use log::info;

/// Simulated tempo for the synthetic features.
const BPM: f64 = 120.0;

pub struct XrScene {
    renderer: SceneRenderer,
    pub quad_texture: wgpu::Texture,
    pub quad_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    frame: u32,
    fps: u32,
    waveform: Vec<f32>,
}

impl XrScene {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        quality: ParticleQuality,
        scene_dir: &Path,
        fps: u32,
    ) -> Result<Self> {
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
            quad_texture,
            quad_view,
            width,
            height,
            frame: 0,
            fps,
            waveform: vec![0.0; WAVEFORM_PEEK],
        })
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

    /// Advance the effect by one frame on `hop` and refresh `quad_texture`.
    /// Submits its own command buffers; call before recording the eye passes.
    pub fn step(&mut self, ts: f64, dt: f32, hop: &HopOutput, waveform: &[f32]) {
        self.renderer.step(ts, dt, hop, waveform, true);

        let mut encoder =
            self.renderer
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xr-quad-copy"),
                });
        encoder.copy_texture_to_texture(
            self.renderer.capture.texture.as_image_copy(),
            self.quad_texture.as_image_copy(),
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.renderer.queue.submit([encoder.finish()]);
    }
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
