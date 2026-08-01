//! The offline scene renderer (#2027): the app's render loop minus the window,
//! the audio device, MIDI, OSC, egui and the wall clock.
//!
//! Everything that decides what a frame looks like is the *same code* the app
//! runs — `bindings::apply`, `gpu::frame_prep`, `gpu::frame_graph`,
//! `gpu::layer_builder`, `scene::cueing` — with this struct supplying the state
//! those functions operate on. The two places this file re-sequences rather
//! than reuses (preset application, timeline-event handling) are thin call
//! chains into those shared cores, cross-referenced with their App
//! counterparts; the parity probe in the test module asserts the observable
//! result (param stores) matches the preset JSON.
//!
//! v1 scope, recorded as warnings rather than silently skipped:
//! media/webcam layers disable, Dissolve plays as Cut, splat scenes and
//! obstacle images stay unloaded.

use std::path::PathBuf;

use crate::bindings::bus::BindingBus;
use crate::effect::loader::EffectLoader;
use crate::gpu::audio_textures::AudioTextures;
use crate::gpu::compositor::Compositor;
use crate::gpu::context::GpuContext;
use crate::gpu::frame_capture::FrameCapture;
use crate::gpu::layer::LayerStack;
use crate::gpu::layer_builder::LayerBuildCtx;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::postprocess::PostProcessChain;
use crate::gpu::uniforms::ShaderUniforms;
use crate::gpu::volumetric::VolumetricParams;
use crate::preset::PresetStore;
use crate::scene::cueing::MorphSnapshot;
use crate::scene::timeline::{Timeline, TimelineEvent};
use crate::settings::ParticleQuality;

/// The capture surface format. `FrameCapture` reads back 4-byte texels only,
/// so the post-process output (not the HDR source) is what gets captured.
pub const CAPTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

pub struct SceneRenderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub width: u32,
    pub height: u32,
    pub effect_loader: EffectLoader,
    pub layer_stack: LayerStack,
    pub compositor: Compositor,
    pub post_process: PostProcessChain,
    pub capture: FrameCapture,
    pub placeholder: PlaceholderTexture,
    pub audio_textures: AudioTextures,
    pub uniforms: ShaderUniforms,
    pub binding_bus: BindingBus,
    pub timeline: Timeline,
    pub preset_store: PresetStore,
    pub particle_quality: ParticleQuality,
    /// Where the scene's presets and `<Name>.bindings.json` sidecars live.
    pub scene_dir: PathBuf,
    pub volumetric_enabled: bool,
    pub volumetric_params: VolumetricParams,
    pub morph_from: Option<MorphSnapshot>,
    pub morph_to: Option<MorphSnapshot>,
    pub pending_cue_overrides: Option<usize>,
    pub frame_count: u32,
    /// Everything this run could not reproduce from the live app, in order.
    pub warnings: Vec<String>,
}

impl SceneRenderer {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u32,
        height: u32,
        particle_quality: ParticleQuality,
        scene_dir: PathBuf,
    ) -> anyhow::Result<Self> {
        let mut effect_loader = EffectLoader::new();
        effect_loader.scan_effects_directory();
        if effect_loader.effects.is_empty() {
            anyhow::bail!(
                "no .pfx effects found — run from the repo root, or beside a build with an \
                 assets/ directory"
            );
        }

        let hdr_format = GpuContext::hdr_format();
        let placeholder = PlaceholderTexture::new(&device, &queue, hdr_format);
        let audio_textures = AudioTextures::new(&device, &queue);
        let compositor = Compositor::new(&device, hdr_format, width, height);
        let post_process =
            PostProcessChain::new(&device, CAPTURE_FORMAT, hdr_format, width, height);
        let capture = FrameCapture::new(&device, width, height, CAPTURE_FORMAT, "headless-capture");

        Ok(Self {
            device,
            queue,
            width,
            height,
            effect_loader,
            layer_stack: LayerStack::new(),
            compositor,
            post_process,
            capture,
            placeholder,
            audio_textures,
            uniforms: ShaderUniforms::zeroed(),
            binding_bus: BindingBus::new_isolated(),
            timeline: Timeline::new(Vec::new(), false, crate::scene::types::AdvanceMode::Manual),
            preset_store: PresetStore::default(),
            particle_quality,
            scene_dir,
            volumetric_enabled: false,
            volumetric_params: VolumetricParams::default(),
            morph_from: None,
            morph_to: None,
            pending_cue_overrides: None,
            frame_count: 0,
            warnings: Vec::new(),
        })
    }

    fn warn(&mut self, msg: String) {
        log::warn!("{msg}");
        self.warnings.push(msg);
    }

    fn build_ctx(&self) -> LayerBuildCtx<'_> {
        LayerBuildCtx {
            device: &self.device,
            queue: &self.queue,
            pipeline_cache: None,
            width: self.width,
            height: self.height,
            placeholder: &self.placeholder,
            audio_textures: &self.audio_textures,
            particle_quality: self.particle_quality,
        }
    }

    /// Load the preset a cue points at: sidecar bindings, then the preset
    /// itself, then the cue's `param_overrides`. The headless counterpart of
    /// `App::load_preset_for_cue` → `load_preset_inner` — kept to the same
    /// sequence; the bodies it calls are the shared ones.
    pub fn load_preset_for_cue(&mut self, index: usize, cue_index: usize) {
        self.pending_cue_overrides = Some(cue_index);
        let Some((name, preset)) = self.preset_store.presets.get(index).cloned() else {
            return;
        };
        let sidecar = self.scene_dir.join(format!("{name}.bindings.json"));
        self.binding_bus.load_preset_bindings_from(&sidecar);
        crate::bindings::apply::upgrade_legacy_targets(&mut self.binding_bus, &preset);
        self.apply_preset(index, &preset);
    }

    /// Apply a preset to the layer stack — the headless counterpart of
    /// `App::apply_preset_immediately`, same observable sequence, minus what a
    /// headless run cannot have (webcam capture, media decode threads, splat
    /// background loads, obstacle images). Every skip lands in `warnings`.
    fn apply_preset(&mut self, index: usize, preset: &crate::preset::Preset) {
        // Remove extra layers or add missing ones to match preset
        while self.layer_stack.layers.len() > preset.layers.len()
            && self.layer_stack.layers.len() > 1
        {
            let last = self.layer_stack.layers.len() - 1;
            self.layer_stack.layers.remove(last);
        }
        while self.layer_stack.layers.len() < preset.layers.len() {
            if self.layer_stack.layers.len() >= crate::bindings::catalog::MAX_LAYERS {
                break;
            }
            let name = format!("Layer {}", self.layer_stack.layers.len() + 1);
            let layer = {
                let ctx = self.build_ctx();
                crate::gpu::layer_builder::new_default_layer(&ctx, name)
            };
            match layer {
                Some(l) => self.layer_stack.layers.push(l),
                None => {
                    self.warn("failed to create a default layer".to_string());
                    break;
                }
            }
        }

        for (i, lp) in preset.layers.iter().enumerate() {
            if self.layer_stack.layers.get(i).is_none() {
                break;
            }
            // Locked layers keep their live state, as in the app. (A freshly
            // constructed renderer has none; this matters across cues.)
            if self.layer_stack.layers[i].locked {
                continue;
            }

            let mut effect_missing = false;

            if lp.webcam_device.is_some() || lp.media_path.is_some() {
                self.warn(format!(
                    "layer {i}: media/webcam layers are not rendered headless — disabled"
                ));
                self.layer_stack.layers[i].enabled = false;
                continue;
            }

            if !lp.effect_name.is_empty() {
                let effect_idx = self
                    .effect_loader
                    .effects
                    .iter()
                    .position(|e| e.name == lp.effect_name);

                // Same morph-safe skip as the app: a layer already running this
                // effect is not rebuilt, so particle state survives transitions.
                let already_loaded = if let Some(idx) = effect_idx {
                    self.layer_stack
                        .layers
                        .get(i)
                        .and_then(|l| l.effect_index())
                        == Some(idx)
                } else {
                    false
                };

                if already_loaded {
                    // params interpolate via morph / apply below
                } else if let Some(idx) = effect_idx {
                    let effect = self.effect_loader.effects[idx].clone();
                    // Ctx built inline (not via build_ctx): prepare_particles
                    // needs the loader mutably, so the borrow must not span self.
                    let particle_system = {
                        let ctx = LayerBuildCtx {
                            device: &self.device,
                            queue: &self.queue,
                            pipeline_cache: None,
                            width: self.width,
                            height: self.height,
                            placeholder: &self.placeholder,
                            audio_textures: &self.audio_textures,
                            particle_quality: self.particle_quality,
                        };
                        crate::gpu::layer_builder::prepare_particles(
                            &ctx,
                            &mut self.effect_loader,
                            &effect,
                        )
                    };
                    if effect
                        .particles
                        .as_ref()
                        .and_then(|pd| pd.splat.as_ref())
                        .is_some()
                    {
                        self.warn(format!(
                            "layer {i}: splat scene loads are App-side — '{}' renders without \
                             its point cloud",
                            effect.name
                        ));
                    }
                    let ctx = LayerBuildCtx {
                        device: &self.device,
                        queue: &self.queue,
                        pipeline_cache: None,
                        width: self.width,
                        height: self.height,
                        placeholder: &self.placeholder,
                        audio_textures: &self.audio_textures,
                        particle_quality: self.particle_quality,
                    };
                    if let Err(e) = crate::gpu::layer_builder::load_effect_into_layer(
                        &ctx,
                        &self.effect_loader,
                        &mut self.layer_stack.layers[i],
                        i,
                        &effect,
                        idx,
                        particle_system,
                    ) {
                        self.warn(format!("layer {i}: effect '{}' failed: {e}", effect.name));
                    }
                } else {
                    self.warn(format!(
                        "layer {i}: effect '{}' not found — layer disabled",
                        lp.effect_name
                    ));
                    effect_missing = true;
                }
            }

            // Particle source restore (#2011): image and model are synchronous
            // and shared; video/webcam are App-side.
            let source_fields = crate::gpu::particle::SourcePresetFields {
                video_path: lp.particle_video_path.clone(),
                video_speed: lp.particle_video_speed,
                video_looping: lp.particle_video_looping,
                webcam: lp.particle_webcam,
                image_path: lp.particle_image_path.clone(),
                model_path: lp.particle_model_path.clone(),
            };
            let declared = self
                .effect_loader
                .effects
                .iter()
                .find(|e| e.name == lp.effect_name)
                .and_then(|e| e.particles.as_ref())
                .and_then(|p| {
                    crate::gpu::particle::source::declared_source(
                        &p.emitter,
                        crate::effect::loader::assets_dir(),
                    )
                });
            match source_fields.resolve().or(declared) {
                Some(crate::gpu::particle::SourceSpec::Image(img_path)) => {
                    if let Some(ps) = self
                        .layer_stack
                        .layers
                        .get_mut(i)
                        .and_then(|l| l.as_effect_mut())
                        .and_then(|e| e.pass_executor.particle_system.as_mut())
                    {
                        crate::gpu::particle::source_restore::restore_image_source(
                            &self.device,
                            &self.queue,
                            ps,
                            &img_path,
                            i,
                        );
                    }
                }
                Some(crate::gpu::particle::SourceSpec::Model(model_path)) => {
                    if let Some(ps) = self
                        .layer_stack
                        .layers
                        .get_mut(i)
                        .and_then(|l| l.as_effect_mut())
                        .and_then(|e| e.pass_executor.particle_system.as_mut())
                    {
                        crate::gpu::particle::source_restore::restore_model_source(
                            &self.device,
                            &self.queue,
                            ps,
                            &model_path,
                            lp.particle_model_pose,
                            lp.particle_model_light,
                            i,
                        );
                    }
                }
                Some(_) => {
                    self.warn(format!(
                        "layer {i}: video/webcam particle sources are not rendered headless"
                    ));
                }
                None => {}
            }
            if lp.splat_scene_path.is_some() {
                self.warn(format!("layer {i}: splat scene not loaded headless"));
            }
            if lp.obstacle_image_path.is_some() || lp.obstacle_depth == Some(true) {
                self.warn(format!("layer {i}: obstacle sources not restored headless"));
            }

            // Params + per-layer prefs — mirrors the app's tail exactly.
            let device = self.device.clone();
            let hdr = GpuContext::hdr_format();
            if let Some(layer) = self.layer_stack.layers.get_mut(i) {
                for (name, value) in &lp.params {
                    if layer.param_store.values.contains_key(name) {
                        layer.param_store.set(name, value.clone());
                    }
                }
                layer.blend_mode = lp.blend_mode;
                layer.opacity = lp.opacity;
                layer.displace_amount = lp.displace_amount;
                layer.enabled = lp.enabled && !effect_missing;
                layer.locked = lp.locked;
                layer.pinned = lp.pinned;
                layer.custom_name = lp.custom_name.clone();
                if let Some(ps) = layer
                    .as_effect_mut()
                    .and_then(|e| e.pass_executor.particle_system.as_mut())
                {
                    if let Some(sim) = &lp.particle_sim {
                        ps.emit_rate = sim.emit_rate;
                        ps.def.emit_rate = sim.emit_rate;
                        ps.burst_on_beat = sim.burst_on_beat;
                        ps.def.burst_on_beat = sim.burst_on_beat;
                        ps.def.lifetime = sim.lifetime;
                        ps.def.initial_speed = sim.initial_speed;
                        ps.def.initial_size = sim.initial_size;
                        ps.def.drag = sim.drag;
                        if let Some(len) = sim.trail_length {
                            ps.def.trail_length = len;
                            ps.set_trail_length(&device, hdr, len);
                        }
                    }
                    if let Some(lat) = lp.lattice {
                        ps.lattice_params = lat;
                        ps.init_lattice(&device, hdr);
                    }
                    if let Some(hx) = lp.helix {
                        ps.helix_params = hx;
                        ps.init_helix(&device, hdr);
                    }
                }
            }
        }

        // Restore active layer + global postprocess (no UI sync headless).
        self.layer_stack.active_layer = preset
            .active_layer
            .min(self.layer_stack.layers.len().saturating_sub(1));
        if let Some(layer) = self.layer_stack.active_mut() {
            layer.postprocess = preset.postprocess.clone();
        }
        self.post_process.enabled = preset.postprocess.enabled;
        if let Some(vol) = &preset.volumetric {
            self.volumetric_enabled = vol.enabled;
            self.volumetric_params = vol.params;
        } else {
            self.volumetric_enabled = false;
        }
        self.preset_store.current_preset = Some(index);
        for layer in &mut self.layer_stack.layers {
            layer.param_store.changed = false;
        }

        // Cue overrides last — the same funnel position as the app's.
        if let Some(cue_idx) = self.pending_cue_overrides.take() {
            if let Some(cue) = self.timeline.cues.get(cue_idx).cloned() {
                crate::scene::cueing::apply_cue_param_overrides(
                    &cue,
                    self.layer_stack.layers.iter_mut().map(|l| {
                        let locked = l.locked;
                        (&mut l.param_store, locked)
                    }),
                );
            }
        }
        if let Some((name, _)) = self.preset_store.presets.get(index) {
            log::info!("[headless] Loaded preset '{name}'");
        }
    }

    /// Handle a timeline event — the headless counterpart of
    /// `App::process_timeline_event`. Dissolve degrades to a hard cut (the
    /// crossfade renderer is a v2 item); everything else is the shared path.
    pub fn process_event(&mut self, event: TimelineEvent) {
        match event {
            TimelineEvent::None | TimelineEvent::TransitionProgress { .. } => {}
            TimelineEvent::LoadCue { cue_index } => {
                let Some(cue) = self.timeline.cues.get(cue_index) else {
                    return;
                };
                let name = cue.preset_name.clone();
                match self
                    .preset_store
                    .presets
                    .iter()
                    .position(|(n, _)| n == &name)
                {
                    Some(idx) => self.load_preset_for_cue(idx, cue_index),
                    None => self.warn(format!("preset '{name}' not found for cue {cue_index}")),
                }
            }
            TimelineEvent::BeginTransition {
                to_cue,
                transition_type,
                ..
            } => match transition_type {
                crate::scene::types::TransitionType::ParamMorph => {
                    self.morph_from = Some(MorphSnapshot::capture(
                        self.layer_stack
                            .layers
                            .iter()
                            .map(|l| (&l.param_store.values, l.opacity)),
                    ));
                    let Some(cue) = self.timeline.cues.get(to_cue) else {
                        return;
                    };
                    let name = cue.preset_name.clone();
                    if let Some(idx) = self
                        .preset_store
                        .presets
                        .iter()
                        .position(|(n, _)| n == &name)
                    {
                        self.load_preset_for_cue(idx, to_cue);
                    }
                    self.morph_to = Some(MorphSnapshot::capture(
                        self.layer_stack
                            .layers
                            .iter()
                            .map(|l| (&l.param_store.values, l.opacity)),
                    ));
                }
                crate::scene::types::TransitionType::Dissolve => {
                    // v1: play as a cut at transition start; note it once.
                    if !self
                        .warnings
                        .iter()
                        .any(|w| w.contains("Dissolve plays as Cut"))
                    {
                        self.warn("Dissolve plays as Cut in headless v1".to_string());
                    }
                    self.process_event(TimelineEvent::LoadCue { cue_index: to_cue });
                }
                crate::scene::types::TransitionType::Cut => {}
            },
            TimelineEvent::TransitionComplete { .. } => {
                self.morph_from = None;
                self.morph_to = None;
            }
        }
    }

    /// Morph interpolation — identical math to the app via `scene::cueing`.
    pub fn apply_morph_interpolation(&mut self, progress: f32) {
        let (Some(from), Some(to)) = (&self.morph_from, &self.morph_to) else {
            return;
        };
        crate::scene::cueing::apply_morph(
            from,
            to,
            progress,
            self.layer_stack
                .layers
                .iter_mut()
                .map(|l| (&mut l.param_store, &mut l.opacity)),
        );
        for layer in &mut self.layer_stack.layers {
            layer.param_store.changed = false;
        }
    }

    /// Param stores per layer, for the parity probe.
    #[cfg(test)]
    pub fn param_stores(&self) -> Vec<&crate::params::ParamStore> {
        self.layer_stack
            .layers
            .iter()
            .map(|l| &l.param_store)
            .collect()
    }

    /// Install a loaded scene: presets into the store, cues into the timeline.
    /// Does not start playback — call [`Self::start`].
    pub fn install_scene(&mut self, loaded: crate::headless::load::LoadedScene) {
        self.preset_store.presets = loaded.presets;
        self.timeline = Timeline::new(
            loaded.scene.cues,
            loaded.scene.loop_mode,
            loaded.scene.advance_mode,
        );
    }

    /// Start the timeline at cue 0 (loads the first preset).
    pub fn start(&mut self) {
        let event = self.timeline.start(0);
        self.process_event(event);
    }

    /// Execute + composite (+ optionally postprocess into the capture texture)
    /// one frame of the current state. Uniforms must already be set for this
    /// frame; the Phase-3 driver owns that. Mirrors `App::render`'s cadence:
    /// poll readbacks before, request + non-blocking poll after submit,
    /// flip() every layer.
    pub fn render_frame(&mut self, capture: bool) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("headless-frame"),
            });

        for layer in &mut self.layer_stack.layers {
            if let Some(effect) = layer.as_effect_mut() {
                if let Some(ps) = effect.pass_executor.particle_system.as_mut() {
                    ps.poll_counter_readback();
                    ps.poll_lattice_population();
                }
            }
        }

        let (source, pp) = crate::gpu::frame_graph::execute_and_composite(
            &self.layer_stack,
            &mut self.compositor,
            &self.device,
            &self.queue,
            &mut encoder,
        );
        if capture {
            self.post_process.render(
                &self.device,
                &self.queue,
                &mut encoder,
                source,
                &self.capture.view,
                self.uniforms.time,
                self.uniforms.rms,
                self.uniforms.onset,
                self.uniforms.flatness,
                &pp,
                false,
            );
            self.capture.copy_to_staging(&mut encoder);
        }
        self.queue.submit(std::iter::once(encoder.finish()));

        for layer in &self.layer_stack.layers {
            if let Some(effect) = layer.as_effect() {
                if let Some(ps) = &effect.pass_executor.particle_system {
                    ps.request_counter_readback();
                    ps.request_lattice_population_readback();
                }
            }
        }
        let _ = self.device.poll(wgpu::PollType::Poll);

        for layer in &mut self.layer_stack.layers {
            layer.flip();
        }
        self.frame_count += 1;
    }

    /// One hop of the scene: features in, frame out. Mirrors `App::update`'s
    /// ordering exactly — bindings evaluate BEFORE `feed_beat` reads
    /// `uniforms.beat` (a `uniform.beat` binding may overwrite it, and live it
    /// does), transport drains between the two, morph applies after tick.
    #[allow(clippy::too_many_arguments)]
    pub fn step(
        &mut self,
        ts: f64,
        dt: f32,
        out: &crate::audio::hop::HopOutput,
        waveform_peek: &[f32],
        capture: bool,
    ) {
        let features = out.frame.features;

        // Clock + globals, from the sample clock — no Instant anywhere.
        self.uniforms.time = ts as f32;
        self.uniforms.delta_time = dt;
        self.uniforms.resolution = [self.width as f32, self.height as f32];
        self.uniforms.feedback_decay = 0.88;
        self.uniforms.frame_index = self.frame_count as f32;
        crate::gpu::uniforms::mirror_audio_features(&mut self.uniforms, &features);

        // A17 textures. One mel column per tick means the scroll phase is
        // exactly 0 — the live path's phase only exists to interpolate between
        // commits that arrive slower than frames.
        self.audio_textures
            .upload_waveform(&self.queue, waveform_peek);
        self.audio_textures
            .upload_spectrum(&self.queue, &out.frame.spectrum);
        self.audio_textures
            .upload_spectrogram(&self.queue, std::slice::from_ref(&out.frame.mel));
        self.uniforms.scroll_phase = 0.0;

        // Bindings — the same dispatch the app runs, fed audio-only sources.
        let outs =
            self.binding_bus
                .evaluate_offline(Some(&features), &out.frame.mel, &out.frame.dmfcc);
        {
            let mut ctx = crate::bindings::apply::BindingTargetCtx {
                layer_stack: &mut self.layer_stack,
                effects: &self.effect_loader.effects,
                uniforms: &mut self.uniforms,
                pending_triggers: &mut self.binding_bus.pending_triggers,
            };
            for o in &outs {
                crate::bindings::apply::apply_binding_target(
                    &mut ctx, &o.target, o.value, o.rising,
                );
            }
        }

        // Transport triggers a binding fired this hop — mirror of main.rs's drain.
        let pending: Vec<String> = self.binding_bus.pending_triggers.drain(..).collect();
        for trigger in &pending {
            let event = match trigger.as_str() {
                "scene.transport.go" => self.timeline.go_next(),
                "scene.transport.prev" => self.timeline.go_prev(),
                "scene.transport.stop" => {
                    self.timeline.stop();
                    crate::scene::timeline::TimelineEvent::None
                }
                _ => crate::scene::timeline::TimelineEvent::None,
            };
            self.process_event(event);
        }

        // Timeline: tempo, beat, tick — same order as App::update.
        if self.timeline.active {
            self.timeline.set_beat_period(features.beat_period_secs());
            let beat_event = self.timeline.feed_beat(self.uniforms.beat > 0.5);
            self.process_event(beat_event);
            let tick_event = self.timeline.tick(dt);
            self.process_event(tick_event);
            if let crate::scene::timeline::PlaybackState::Transitioning {
                progress,
                transition_type: crate::scene::types::TransitionType::ParamMorph,
                ..
            } = &self.timeline.state
            {
                let progress = *progress;
                self.apply_morph_interpolation(progress);
            }
        }

        crate::gpu::frame_prep::prepare_effect_layers(
            &mut self.layer_stack.layers,
            &self.uniforms,
            &features,
            dt,
            &self.device,
            &self.queue,
            self.layer_stack.active_layer,
            self.volumetric_enabled,
            self.volumetric_params,
        );

        self.render_frame(capture);
    }

    /// Blocking readback of the last captured frame (tight RGBA rows).
    /// Fine offline: captures are sparse and throughput dominates latency.
    pub fn read_captured_frame(&mut self) -> Option<Vec<u8>> {
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .ok()?;
        self.capture.request_map();
        loop {
            self.device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .ok()?;
            if let Some(d) = self.capture.take_mapped_data(&self.device) {
                return Some(d);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::test_gpu::{gpu_guard, test_gpu};

    fn write_fixture(dir: &std::path::Path) {
        // Real effects, real params, so parity is against the shipped .pfx set.
        std::fs::write(
            dir.join("Look A.json"),
            r#"{"layers":[
                 {"effect_name":"Aurora","blend_mode":"Normal","opacity":1.0,
                  "params":{"curtain_speed":{"Float":0.8},"glow_width":{"Float":0.2}}},
                 {"effect_name":"Drift","blend_mode":"Add","opacity":0.7,
                  "params":{"warp_intensity":{"Float":0.4}}}],
               "active_layer":0}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("Look A.bindings.json"),
            r#"{"version":1,"bindings":[{"id":"b_000","name":"kick glow","enabled":true,
               "scope":"Preset","source":"audio.kick","target":"postfx.bloom_intensity",
               "transforms":[]}]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("_scene.json"),
            r#"{"version":1,"name":"Parity","advance_mode":"Timer","cues":[
                 {"preset_name":"Look A","hold_secs":8.0},
                 {"preset_name":"Look A","hold_secs":8.0,
                  "param_overrides":[{"curtain_speed":{"Float":0.15}}]}]}"#,
        )
        .unwrap();
    }

    fn float(store: &crate::params::ParamStore, name: &str) -> f32 {
        match store.values.get(name) {
            Some(crate::params::ParamValue::Float(v)) => *v,
            other => panic!("{name} = {other:?}"),
        }
    }

    /// The App-parity drift guard for the re-sequenced preset application: the
    /// observable result of loading a scene headless must match the preset
    /// JSON — params, blend, opacity — and the cue's overrides must land.
    /// Then one real frame renders without validation errors and is not black.
    #[test]
    #[ignore = "GPU"]
    fn parity_and_first_frame() {
        let _guard = gpu_guard();
        let (device, queue) = test_gpu();
        let dir = std::env::temp_dir().join("phosphor_headless_parity");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_fixture(&dir);

        // `assets_dir()` resolves CWD-relative and `cargo test` runs with
        // CWD = the crate dir, which has no assets/. The cached path is
        // relative, so pointing CWD at the repo root fixes resolution no
        // matter which test touched assets_dir() first.
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let mut sr = SceneRenderer::new(
            (*device).clone(),
            (*queue).clone(),
            320,
            180,
            ParticleQuality::Medium,
            dir.clone(),
        )
        .expect("renderer");
        let loaded = crate::headless::load::load_scene_dir(&dir).expect("scene loads");
        sr.install_scene(loaded);
        sr.start();

        // --- parity: cue 0 = the preset verbatim ---
        assert_eq!(sr.layer_stack.layers.len(), 2);
        let stores = sr.param_stores();
        assert!((float(stores[0], "curtain_speed") - 0.8).abs() < 1e-6);
        assert!((float(stores[0], "glow_width") - 0.2).abs() < 1e-6);
        assert!((float(stores[1], "warp_intensity") - 0.4).abs() < 1e-6);
        assert_eq!(
            sr.layer_stack.layers[1].blend_mode,
            crate::gpu::layer::BlendMode::Add
        );
        assert!((sr.layer_stack.layers[1].opacity - 0.7).abs() < 1e-6);
        // Sidecar came along.
        assert_eq!(sr.binding_bus.bindings.len(), 1);
        assert!(
            sr.warnings.is_empty(),
            "unexpected warnings: {:?}",
            sr.warnings
        );

        // --- cue 1: same preset + param_overrides ---
        let ev = sr.timeline.go_to_cue(1);
        sr.process_event(ev);
        let stores = sr.param_stores();
        assert!(
            (float(stores[0], "curtain_speed") - 0.15).abs() < 1e-6,
            "cue override did not land"
        );
        // Un-overridden params keep the preset value.
        assert!((float(stores[0], "glow_width") - 0.2).abs() < 1e-6);

        // --- one real frame through the full chain ---
        sr.uniforms.resolution = [320.0, 180.0];
        sr.uniforms.feedback_decay = 0.88;
        // Non-silent synthetic audio: a silent scene legitimately renders
        // dark (these are audio-reactive effects), so the not-black assertion
        // needs signal on the inputs, same as the loader previews.
        let audio = crate::audio::AudioFeatures {
            rms: 0.6,
            sub_bass: 0.7,
            bass: 0.8,
            low_mid: 0.6,
            mid: 0.5,
            upper_mid: 0.5,
            presence: 0.4,
            brilliance: 0.4,
            flux: 0.5,
            onset: 0.4,
            beat_strength: 0.7,
            kick: 0.9,
            ..Default::default()
        };

        // The sidecar's kick -> postfx.bloom_intensity binding, through the
        // offline bus and the SAME dispatch the app runs. Proves the whole
        // binding path works headless, not just that it loads.
        let outs = sr
            .binding_bus
            .evaluate_offline(Some(&audio), &[], &[0.0; 13]);
        assert_eq!(outs.len(), 1, "the sidecar binding should fire");
        {
            let mut ctx = crate::bindings::apply::BindingTargetCtx {
                layer_stack: &mut sr.layer_stack,
                effects: &sr.effect_loader.effects,
                uniforms: &mut sr.uniforms,
                pending_triggers: &mut sr.binding_bus.pending_triggers,
            };
            for out in &outs {
                crate::bindings::apply::apply_binding_target(
                    &mut ctx,
                    &out.target,
                    out.value,
                    out.rising,
                );
            }
        }
        let active = sr.layer_stack.active_layer;
        assert!(
            (sr.layer_stack.layers[active].postprocess.bloom_intensity - 0.9).abs() < 1e-5,
            "kick binding did not reach the active layer's postprocess"
        );

        for frame in 0..30u32 {
            sr.uniforms.time = frame as f32 / 60.0;
            sr.uniforms.delta_time = 1.0 / 60.0;
            sr.uniforms.frame_index = frame as f32;
            crate::gpu::uniforms::mirror_audio_features(&mut sr.uniforms, &audio);
            crate::gpu::frame_prep::prepare_effect_layers(
                &mut sr.layer_stack.layers,
                &sr.uniforms,
                &audio,
                1.0 / 60.0,
                &sr.device,
                &sr.queue,
                sr.layer_stack.active_layer,
                sr.volumetric_enabled,
                sr.volumetric_params,
            );
            sr.render_frame(frame == 29);
        }
        let rgba = sr.read_captured_frame().expect("readback");
        assert_eq!(rgba.len(), 320 * 180 * 4);
        let mean: f64 = rgba
            .chunks_exact(4)
            .map(|px| (px[0] as f64 + px[1] as f64 + px[2] as f64) / 3.0)
            .sum::<f64>()
            / (320.0 * 180.0);
        assert!(
            mean > 1.0,
            "frame is (near) black — mean luma {mean:.3}; the chain rendered nothing"
        );

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "validation error: {err:?}");
    }
}
