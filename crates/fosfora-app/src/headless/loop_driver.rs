//! The loop-export driver (#2063, Phase 2): renders a [`LoopSpec`] to raw
//! frames with beat uniforms synthesized from pure arithmetic — no PCM, no
//! `HopAnalyzer`, no PLL, no wall clock.
//!
//! Semantics replicate the live path exactly (errata #2062, P2 Q1): `beat` and
//! `downbeat` are one-frame pulses on the frame where the corresponding
//! monotonic index steps; `beat_in_bar` is a held step ratio (`pos / 4`, 4/4
//! fixed in v1); phases are exact sawtooths (no PLL smoothing — correct for
//! export, noted in docs/loop-spec.md). The synthesized features flow through
//! `mirror_audio_features` — the same single mirror path live audio uses, so a
//! new uniform field can never silently diverge between live and export.

use crate::audio::features::{AudioFeatures, BPM_NORM};
use crate::gpu::layer_builder::LayerBuildCtx;
use crate::headless::loop_spec::{BestEffort, LoopAudio, LoopBackground, LoopSpec, LoopTiming};
use crate::headless::scene_renderer::SceneRenderer;
use crate::settings::{AlphaOutputMode, ParticleQuality};

/// The pinned neutral audio-feature vector for `audio: "none"` renders
/// (P2 Q2): mid-scale energies and a stable key, so palette warm/cool and
/// audio-accent paths sit at a sane resting point instead of degenerate zeros.
/// This is the SAME vector the phase-locked determinism probe renders with —
/// one definition, two consumers; golden loops are golden against exactly this.
pub fn neutral_features() -> AudioFeatures {
    let mut f = AudioFeatures {
        rms: 0.5,
        bass: 0.4,
        onset: 0.3,
        centroid: 0.45,
        flatness: 0.2,
        beat_strength: 0.7,
        key_class: 5.0 / 11.0,
        key_is_minor: 1.0,
        ..Default::default()
    };
    f.chroma = [0.5; 12];
    f
}

/// Synthesize the frame-`n` feature vector for a loop at `effective_bpm`:
/// the neutral vector plus arithmetic beat/bar clocks.
pub fn synth_features(frame: u32, fps: u32, effective_bpm: f64) -> AudioFeatures {
    let beats_at = |n: u32| n as f64 / fps as f64 * effective_bpm / 60.0;
    let beats = beats_at(frame);
    let bars = beats / 4.0;

    // One-frame pulses: fire on the frame where the monotonic index steps.
    // Frame 0 is the loop's "one" — both fire, matching a live downbeat.
    let (beat_fire, downbeat_fire) = if frame == 0 {
        (1.0, 1.0)
    } else {
        let prev = beats_at(frame - 1);
        (
            if beats.floor() > prev.floor() {
                1.0
            } else {
                0.0
            },
            if bars.floor() > (prev / 4.0).floor() {
                1.0
            } else {
                0.0
            },
        )
    };

    let mut f = neutral_features();
    f.beat = beat_fire;
    f.beat_phase = beats.fract() as f32;
    f.bpm = (effective_bpm / BPM_NORM as f64) as f32;
    f.downbeat = downbeat_fire;
    f.bar_phase = bars.fract() as f32;
    f.beat_in_bar = ((beats.floor() as u64 % 4) as f32) / 4.0;
    f.bar_index = bars.floor() as f32;
    f.beat_index = beats.floor() as f32;
    f
}

/// `audio: "synthetic"` (P2.2): the accent features themselves, synthesized
/// from phase arithmetic — a kick-shaped energy envelope on every beat, a
/// sharper onset transient, a bar-periodic loudness swell, and a buildup that
/// rises through each cycle. Deliberately NOT a real analyzer run: analyzer
/// state (AGC windows, EMAs, trackers) is not exactly periodic, and routing it
/// in would trade the bit-exact closure this pipeline is built on for noise.
/// Every value below is a pure, cycle-periodic function of the frame index, so
/// the golden guarantee extends to synthetic mode unchanged.
pub fn synth_features_accented(
    frame: u32,
    fps: u32,
    effective_bpm: f64,
    loop_bars: u32,
) -> AudioFeatures {
    let mut f = synth_features(frame, fps, effective_bpm);
    let beat_env = (-f.beat_phase * 5.0).exp();
    let onset_env = (-f.beat_phase * 9.0).exp();
    let bar_swell = 0.5 - 0.5 * (f.bar_phase * std::f32::consts::TAU).cos();
    // Cycle phase over the whole loop: buildup rises through each half-cycle
    // and falls through the second, mirroring the family's breathing cycles.
    let bars_f = loop_bars.max(1) as f32;
    let cyc = ((f.bar_index + f.bar_phase) / bars_f).fract();
    let breath = 1.0 - (1.0 - 2.0 * cyc).abs();

    f.rms = 0.35 + 0.45 * beat_env;
    f.sub_bass = 0.3 + 0.55 * beat_env;
    f.bass = 0.3 + 0.5 * beat_env;
    f.kick = 0.85 * onset_env;
    f.onset = 0.9 * onset_env;
    f.percussive_energy = 0.2 + 0.6 * onset_env;
    f.beat_strength = 0.9;
    f.loudness_m = 0.35 + 0.3 * bar_swell;
    f.buildup = 0.6 * breath;
    f
}

/// A loaded, ready-to-render loop: the effect on layer 0 of a headless
/// renderer with the spec's params applied. Frames are pure functions of the
/// frame index for phase-locked effects, so callers may render any subset in
/// any order (the golden-loop gate renders exactly frame 0 and frame N).
pub struct LoopSession {
    sr: SceneRenderer,
    pub timing: LoopTiming,
    fps: u32,
    width: u32,
    height: u32,
    audio: LoopAudio,
    loop_bars: u32,
}

/// Mode-aware render (P2.7). Exact and time-wrapped emit frames as rendered;
/// crossfade renders warmup + loop + tail SEQUENTIALLY (stateful effects
/// evolve across every call) and streams with bounded memory: the output loop
/// starts at rendered frame W+T, so only the head window (T frames) is
/// buffered, the middle streams as rendered, and the tail blends into the
/// buffered head as it arrives. The blend is a plain per-channel lerp — legal
/// precisely because frames are PREMULTIPLIED (premultiplied colors compose
/// linearly; straight alpha would need unpremult/re-premult).
pub fn render_loop_with(
    spec: &LoopSpec,
    mode: BestEffort,
    mut on_frame: impl FnMut(u32, &[u8]) -> Result<(), String>,
) -> Result<LoopTiming, String> {
    let mut session = LoopSession::create(spec, mode)?;
    let n = session.timing.frames;
    match mode {
        BestEffort::None | BestEffort::TimeWrapped => {
            if matches!(mode, BestEffort::TimeWrapped) {
                log::warn!(
                    "[loop] time-wrapped best-effort render: '{}' has no closure guarantee",
                    spec.effect
                );
            }
            for frame in 0..n {
                let rgba = session.render_frame_at(frame)?;
                on_frame(frame, &rgba)?;
            }
        }
        BestEffort::Crossfade {
            tail_bars,
            warmup_bars,
        } => {
            let bar_frames = (n / (spec.bars.max(1))).max(1);
            let t = (tail_bars.max(1) * bar_frames).min(n / 2);
            let w = warmup_bars * bar_frames;
            log::warn!(
                "[loop] crossfade best-effort render: {w} warmup frames discarded,                  {t}-frame seam blend — perceptual, not exact"
            );
            let mut head: Vec<Vec<u8>> = Vec::with_capacity(t as usize);
            for frame in 0..(w + n + t) {
                let rgba = session.render_frame_at(frame)?;
                if frame < w {
                    continue; // warmup: rendered to settle state, discarded
                }
                let k = frame - w;
                if k < t {
                    head.push(rgba); // head window: buffered, emitted blended at the end
                } else if k < n {
                    on_frame(k - t, &rgba)?; // middle: streams as rendered
                } else {
                    // Tail: blend toward the buffered head. Output frame
                    // (n - t + i) fades rendered continuity into head[i], so
                    // the file's wrap lands on head[t] == the first middle
                    // frame — continuous by construction.
                    let i = k - n;
                    let weight = (i + 1) as f32 / (t + 1) as f32;
                    let blended = blend_premultiplied(&rgba, &head[i as usize], weight);
                    on_frame(n - t + i, &blended)?;
                }
            }
        }
    }
    Ok(session.timing)
}

/// Per-channel lerp of two premultiplied RGBA8 frames: `w` = 0 → all `a`,
/// 1 → all `b`.
fn blend_premultiplied(a: &[u8], b: &[u8], w: f32) -> Vec<u8> {
    debug_assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b.iter())
        .map(|(&x, &y)| (x as f32 + (y as f32 - x as f32) * w).round() as u8)
        .collect()
}

impl LoopSession {
    /// Acquire a headless device and load the spec. One session per render —
    /// GPU state is not reused across specs.
    pub fn create(spec: &LoopSpec, mode: BestEffort) -> Result<Self, String> {
        let (device, queue, adapter) =
            crate::headless::gpu::create().map_err(|e| format!("headless GPU init: {e}"))?;
        log::info!("[loop] adapter: {adapter}");
        Self::create_with(spec, mode, device, queue)
    }

    /// Exact-mode session on an existing device (the GPU probes' entry).
    #[cfg(test)]
    pub fn create_on(
        spec: &LoopSpec,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Result<Self, String> {
        Self::create_with(spec, BestEffort::None, device, queue)
    }

    fn create_with(
        spec: &LoopSpec,
        mode: BestEffort,
        device: wgpu::Device,
        queue: wgpu::Queue,
    ) -> Result<Self, String> {
        let timing = spec.snap()?;
        if spec.audio == LoopAudio::File {
            return Err(
                "audio 'file' is not wired yet — use 'none' (neutral) or 'synthetic' \
                 (beat-locked accent envelopes)"
                    .into(),
            );
        }

        let [w, h] = spec.resolution;
        let scratch = std::env::temp_dir().join("fosfora-loop-scene");
        let _ = std::fs::create_dir_all(&scratch);
        let mut sr = SceneRenderer::new(device, queue, w, h, ParticleQuality::High, scratch)
            .map_err(|e| format!("renderer init: {e}"))?;
        spec.validate_for(&sr.effect_loader.effects, mode)?;

        sr.output_alpha = match spec.background {
            LoopBackground::Transparent => AlphaOutputMode::Passthrough,
            LoopBackground::Opaque => AlphaOutputMode::Opaque,
        };

        // Load the effect onto layer 0, exactly as the preset path does.
        let idx = sr
            .effect_loader
            .effects
            .iter()
            .position(|e| e.name == spec.effect)
            .expect("validated above");
        let effect = sr.effect_loader.effects[idx].clone();
        // A fresh LayerStack is empty — create layer 0 the same way the app's
        // add-layer path does before loading the effect into it.
        if sr.layer_stack.layers.is_empty() {
            let ctx = LayerBuildCtx {
                device: &sr.device,
                queue: &sr.queue,
                pipeline_cache: None,
                width: w,
                height: h,
                placeholder: &sr.placeholder,
                audio_textures: &sr.audio_textures,
                particle_quality: sr.particle_quality,
                backdrop: Some((
                    &sr.compositor.backdrop.view,
                    &sr.compositor.backdrop.sampler,
                )),
            };
            let layer = crate::gpu::layer_builder::new_default_layer(&ctx, "Loop".into())
                .ok_or("could not create the loop layer")?;
            sr.layer_stack.layers.push(layer);
        }
        let particle_system = {
            let ctx = LayerBuildCtx {
                device: &sr.device,
                queue: &sr.queue,
                pipeline_cache: None,
                width: w,
                height: h,
                placeholder: &sr.placeholder,
                audio_textures: &sr.audio_textures,
                particle_quality: sr.particle_quality,
                backdrop: Some((
                    &sr.compositor.backdrop.view,
                    &sr.compositor.backdrop.sampler,
                )),
            };
            crate::gpu::layer_builder::prepare_particles(&ctx, &mut sr.effect_loader, &effect)
        };
        {
            let ctx = LayerBuildCtx {
                device: &sr.device,
                queue: &sr.queue,
                pipeline_cache: None,
                width: w,
                height: h,
                placeholder: &sr.placeholder,
                audio_textures: &sr.audio_textures,
                particle_quality: sr.particle_quality,
                backdrop: Some((
                    &sr.compositor.backdrop.view,
                    &sr.compositor.backdrop.sampler,
                )),
            };
            crate::gpu::layer_builder::load_effect_into_layer(
                &ctx,
                &sr.effect_loader,
                &mut sr.layer_stack.layers[0],
                0,
                &effect,
                idx,
                particle_system,
            )
            .map_err(|e| format!("loading '{}': {e}", spec.effect))?;
        }
        sr.layer_stack.layers[0]
            .param_store
            .load_from_defs(&effect.inputs);
        for (name, value) in &spec.params {
            sr.layer_stack.layers[0]
                .param_store
                .set(name, value.clone());
        }

        Ok(Self {
            sr,
            timing,
            fps: spec.fps,
            width: w,
            height: h,
            audio: spec.audio,
            loop_bars: spec.bars,
        })
    }

    /// Render one frame (blocking readback) and return its tightly-packed
    /// RGBA8 bytes.
    pub fn render_frame_at(&mut self, frame: u32) -> Result<Vec<u8>, String> {
        let dt = 1.0 / self.fps as f32;
        let features = match self.audio {
            LoopAudio::Synthetic => {
                synth_features_accented(frame, self.fps, self.timing.effective_bpm, self.loop_bars)
            }
            _ => synth_features(frame, self.fps, self.timing.effective_bpm),
        };
        let sr = &mut self.sr;
        sr.uniforms.resolution = [self.width as f32, self.height as f32];
        sr.uniforms.feedback_decay = 0.88;
        sr.uniforms.time = frame as f32 * dt;
        sr.uniforms.delta_time = dt;
        sr.uniforms.frame_index = frame as f32;
        crate::gpu::uniforms::mirror_audio_features(&mut sr.uniforms, &features);
        crate::gpu::frame_prep::prepare_effect_layers(
            &mut sr.layer_stack.layers,
            &sr.uniforms,
            &features,
            dt,
            &sr.device,
            &sr.queue,
            sr.layer_stack.active_layer,
            sr.volumetric_enabled,
            sr.volumetric_params,
        );
        sr.render_frame(true);
        sr.read_captured_frame()
            .ok_or_else(|| format!("readback failed at frame {frame}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE golden-loop gate (P2.8, decision #2066: dev-run release probe):
    /// for every shipped `loop: "phase_locked"` effect, frame 0 and frame N of
    /// a fixed spec render BIT-IDENTICALLY — the loop closes exactly, by
    /// construction, at the pixel level. Also re-renders frame 0 to pin
    /// intra-session determinism. Any edit that sneaks wall-clock or stateful
    /// behavior into a phase-locked effect fails here.
    /// Run: cargo test -p fosfora-app -- --ignored golden_loop
    #[test]
    #[ignore = "GPU"]
    fn golden_loop_frame_zero_equals_frame_n() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }

        let mut checked = 0usize;
        for effect in crate::effect::loader::shipped_effects_for_test() {
            if effect.loop_mode != crate::effect::format::LoopMode::PhaseLocked {
                continue;
            }
            // 120 BPM / 8 bars / 30 fps snaps losslessly to 480 frames. Eight
            // bars = the LCM of the family's bars_per_cycle defaults (4 and 8);
            // a loop closes only over whole effect cycles, so a new effect with
            // a non-power-of-two default needs this bumped.
            let spec = LoopSpec {
                version: 1,
                effect: effect.name.clone(),
                params: Default::default(),
                bpm: 120.0,
                bars: 8,
                fps: 30,
                resolution: [320, 180],
                codec: crate::headless::loop_spec::LoopCodec::HapAlpha,
                audio: LoopAudio::None,
                audio_file: None,
                background: LoopBackground::Transparent,
            };
            let mut session = LoopSession::create_on(&spec, (*device).clone(), (*queue).clone())
                .unwrap_or_else(|e| panic!("{}: {e}", effect.name));
            assert_eq!(session.timing.frames, 480);
            let f0 = session.render_frame_at(0).unwrap();
            let f0_again = session.render_frame_at(0).unwrap();
            assert_eq!(
                f0, f0_again,
                "{}: frame 0 not deterministic within a session",
                effect.name
            );
            let fn_ = session.render_frame_at(session.timing.frames).unwrap();
            assert_eq!(
                f0, fn_,
                "{}: frame N != frame 0 — the loop does not close",
                effect.name
            );
            // The render must not be empty — a black/transparent output would
            // pass equality vacuously. Checked MID-cycle: a loop's start can
            // legitimately be its quietest frame (Fenestra opens released).
            // frames/2 of an 8-bar loop is a cycle BOUNDARY (quiet again for a
            // 4-bar cycle) — probe one bar in, where every family member is live.
            let f_mid = session.render_frame_at(session.timing.frames / 8).unwrap();
            assert!(
                f_mid.chunks_exact(4).any(|px| px[3] > 32),
                "{}: mid-cycle frame is empty",
                effect.name
            );

            // WRAP SMOOTHNESS (the jolt Kevin caught in a real player): bit-
            // exact closure says nothing about the SIZE of the visual step
            // across the wrap — a sawtooth cycle (reveal 0→1 then snap back)
            // closes exactly and still jolts every cycle. Require the wrap
            // step (frame N-1 → frame 0) to be comparable to ordinary frame
            // steps sampled mid-loop.
            let mad = |a: &[u8], b: &[u8]| -> f64 {
                a.iter()
                    .zip(b)
                    .map(|(&x, &y)| (x as f64 - y as f64).abs())
                    .sum::<f64>()
                    / a.len() as f64
            };
            let last = session.render_frame_at(session.timing.frames - 1).unwrap();
            let seam_step = mad(&last, &f0);
            // Compare against the LARGEST ordinary step, sampled to include
            // bar boundaries: effects may legitimately jump per bar (Reticle
            // teleports targets on the "one"), and the wrap IS a bar boundary
            // — it just must not be a bigger event than any other one.
            let mut typical_max = 0.0f64;
            for probe in [59u32, 119, 300] {
                let a = session.render_frame_at(probe).unwrap();
                let b = session.render_frame_at(probe + 1).unwrap();
                typical_max = typical_max.max(mad(&a, &b));
            }
            assert!(
                seam_step <= typical_max * 1.5 + 1.0,
                "{}: wrap step {seam_step:.3} vs largest ordinary step {typical_max:.3} — \
                 the cycle snaps at its boundary (sawtooth); make it breathe",
                effect.name
            );
            checked += 1;
        }
        assert!(
            checked >= 5,
            "expected the phase-locked family, got {checked}"
        );
    }

    /// #1986 end to end, on the real shaders rather than a Rust mirror of the
    /// formula: a trail must be a function of WALL-CLOCK time, so the same effect
    /// driven over the same 1.2 s window at 30 fps and at 120 fps must land on
    /// the same image. With a per-frame decay constant the 30 fps run retains
    /// k^36 of its history where the 120 fps run retains k^144 — for Pulse's
    /// k = 0.82 that is 8e-4 against 4e-13, i.e. one has trails and the other
    /// has effectively none.
    ///
    /// 1.2 s at 120 BPM is beat 2.4 — deliberately off any beat boundary, since
    /// `beat` is a one-frame pulse and so lasts 1/30 s in one run and 1/120 s in
    /// the other. Effects are picked from the three with no particle system
    /// (`pulse.pfx`, `iris.pfx`), whose sources live entirely in the shader, so
    /// the comparison isn't polluted by the Rust-side particle composite.
    ///
    /// Run: cargo test -p fosfora-app -- --ignored frame_rate_independent_trails
    #[test]
    #[ignore = "GPU"]
    fn frame_rate_independent_trails() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }

        const WINDOW_SECS: f32 = 1.2;
        let spec_at = |effect: &str, fps: u32, trail: f32| LoopSpec {
            version: 1,
            effect: effect.to_string(),
            params: [(
                "trail_length".to_string(),
                crate::params::ParamValue::Float(trail),
            )]
            .into_iter()
            .collect(),
            bpm: 120.0,
            bars: 8,
            fps,
            resolution: [320, 180],
            codec: crate::headless::loop_spec::LoopCodec::H264,
            audio: LoopAudio::None,
            audio_file: None,
            background: LoopBackground::Opaque,
        };

        // Accumulate history frame by frame — the trail IS the state under test,
        // so the window has to be walked, not jumped to.
        let render_window = |effect: &str, fps: u32, trail: f32| -> Vec<u8> {
            let spec = spec_at(effect, fps, trail);
            let mut session = LoopSession::create_with(
                &spec,
                BestEffort::TimeWrapped,
                (*device).clone(),
                (*queue).clone(),
            )
            .unwrap_or_else(|e| panic!("{effect} @ {fps}fps: {e}"));
            let last = (WINDOW_SECS * fps as f32).round() as u32;
            let mut frame_bytes = Vec::new();
            for f in 0..=last {
                frame_bytes = session.render_frame_at(f).unwrap();
            }
            frame_bytes
        };

        let mean_luma = |px: &[u8]| -> f64 {
            px.chunks_exact(4)
                .map(|p| (p[0] as f64 + p[1] as f64 + p[2] as f64) / 765.0)
                .sum::<f64>()
                / (px.len() / 4) as f64
        };

        let sweep = |trail: f32| -> Vec<f64> {
            [30u32, 60, 120]
                .iter()
                .map(|&fps| {
                    let m = mean_luma(&render_window("Iris", fps, trail));
                    println!("FPSINDEP Iris trail={trail:.2} fps={fps:<4} mean={m:.6}");
                    m
                })
                .collect()
        };
        let spread = |v: &[f64]| -> f64 {
            let (lo, hi) = v
                .iter()
                .fold((f64::MAX, 0.0f64), |(l, h), &x| (l.min(x), h.max(x)));
            (hi - lo) / hi.max(1e-9)
        };

        // CONTROL: with no trail at all, only the current dot is drawn, so the
        // three frame rates must agree exactly. This is what separates a decay
        // bug from the OTHER frame-rate dependency shipped effects have — an
        // effect that draws moving content leaves one sample per frame along its
        // path, so its coverage grows with frame rate no matter what decay does.
        // (Pulse is unusable for this test for exactly that reason: its expanding
        // rings are ~2x denser at 120 fps than at 30, fix or no fix.)
        let control = sweep(0.0);
        assert!(
            spread(&control) < 1e-6,
            "control is not frame-rate flat ({control:?}) — Iris's own content has \
             become rate-dependent, so this probe can no longer isolate the decay"
        );

        // The measurement: a long trail must reach the same brightness at every
        // frame rate. With a per-frame decay constant this spreads ~40%.
        let trailed = sweep(0.95);
        assert!(
            trailed.iter().all(|&m| m > control[0] * 1.3),
            "trail=0.95 ({trailed:?}) is barely above the no-trail control \
             ({control:?}) — no trail is actually accumulating, so a pass is vacuous"
        );
        assert!(
            spread(&trailed) < 0.05,
            "trail brightness depends on frame rate: means {trailed:?} across \
             30/60/120 fps spread {:.1}%. Trail length is being measured in frames, \
             not seconds (#1986).",
            spread(&trailed) * 100.0
        );
    }

    /// #2349 end to end, the particle half of the story `frame_rate_independent_trails`
    /// could not reach.
    ///
    /// That probe deliberately picks effects with no particle system, because
    /// particles composite additively in Rust *after* the fragment passes, so
    /// #1986 could only correct the retention `k` and not the source gain `a`.
    /// Steady state is `a/(1-k)`, so the 15-odd `*_bg` effects settled at roughly
    /// half their 60 fps brightness at 30 fps and double at 120. This walks a
    /// member of that family at all three rates and asserts the plateau holds.
    ///
    /// Run: cargo test -p fosfora-app -- --ignored frame_rate_independent_particle_composite
    #[test]
    #[ignore = "GPU"]
    fn frame_rate_independent_particle_composite() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }

        const WINDOW_SECS: f32 = 1.2;
        let render = |effect: &str, fps: u32, decay: f32| -> Vec<u8> {
            let spec = LoopSpec {
                version: 1,
                effect: effect.to_string(),
                params: [(
                    "trail_decay".to_string(),
                    crate::params::ParamValue::Float(decay),
                )]
                .into_iter()
                .collect(),
                bpm: 120.0,
                bars: 8,
                fps,
                resolution: [320, 180],
                codec: crate::headless::loop_spec::LoopCodec::H264,
                audio: LoopAudio::None,
                audio_file: None,
                background: LoopBackground::Opaque,
            };
            let mut session = LoopSession::create_with(
                &spec,
                BestEffort::TimeWrapped,
                (*device).clone(),
                (*queue).clone(),
            )
            .unwrap_or_else(|e| panic!("{effect} @ {fps}fps: {e}"));
            let last = (WINDOW_SECS * fps as f32).round() as u32;
            let mut frame_bytes = Vec::new();
            for f in 0..=last {
                frame_bytes = session.render_frame_at(f).unwrap();
            }
            frame_bytes
        };
        let mean_luma = |px: &[u8]| -> f64 {
            px.chunks_exact(4)
                .map(|p| (p[0] as f64 + p[1] as f64 + p[2] as f64) / 765.0)
                .sum::<f64>()
                / (px.len() / 4) as f64
        };
        let spread = |v: &[f64]| -> f64 {
            let (lo, hi) = v
                .iter()
                .fold((f64::MAX, 0.0f64), |(l, h), &x| (l.min(x), h.max(x)));
            (hi - lo) / hi.max(1e-9)
        };

        // Cascade exercises the compute-raster resolve, Ascend the billboard
        // renderer — the gain is applied in two different shaders and a wiring
        // bug in either would show only here.
        //
        // Tide additionally witnesses #2351: its ribbon ring used to advance one
        // slot per FRAME, making a 16-point trail 533 ms at 30 fps and 133 ms at
        // 120. That swamped everything else — its control read 50.6% and its
        // trailed spread 19.6%. On the 60 Hz ring they are 7.4% and 0.3%.
        //
        // Ascend and Panorama additionally witness #2378, feedback ADVECTION in
        // per-frame uv units: both resample the trail at an offset applied once
        // per frame with no delta_time term, so the echo travelled twice as far
        // per wall-clock second at 120 fps as at 60. Ascend is the family's clean
        // case (no in-shader source term at all) and went 13.7% -> 1.2-3.4%.
        // Panorama needed BOTH fixes and is the whole family in one effect:
        // 58.5% as shipped, 32.9% with the source gain corrected (#2376), 0.7-1.0%
        // with the advection corrected too. Its control is 0.1-0.3%, the flattest
        // here, which is what makes it the best witness in the list.
        //
        // Deliberately NOT in this list:
        //   Vessel   — its retention is param `glow`, not `trail_decay`, so the
        //              knob this probe turns does not reach it and the trail
        //              never accumulates. Cleave and Tide already cover ribbons.
        //   Chaos, Accretion, Mycelium — all three are clean advection sites on
        //              paper (pure `trail = prev * frame_decay3(..)`, no in-shader
        //              source) and all three FAIL the vacuity guard here: each
        //              reads control ~0.0130 and trailed ~0.0133, so no trail
        //              accumulates and any pass would be meaningless. Their sims
        //              need audio to spawn and this harness runs LoopAudio::None.
        //              Measuring their advection needs a driven harness, not this
        //              one — do not add them back without fixing that first.
        //   Storm, Frost, Drift — the other three #1986 frame_gain sites, so both
        //              their terms are already correct and they would isolate
        //              advection perfectly. None exposes a `trail_decay` param,
        //              so this probe cannot turn the knob it needs.
        //
        // Thresholds are from measurement with headroom, not theory. Cascade
        // reads 47.7% with the composite gain forced to 1.0 and 6.2% with it
        // live; Tide reads 19.6% on a frame-counted ring and 0.3% on the timed
        // one. Each ceiling sits between the two, well above the 0.1-0.2%
        // run-to-run noise on the controls.
        //
        // Ascend's 0.06 and Panorama's 0.05 were set the same way, by reverting
        // the fix and measuring both sides: Ascend 13.16-14.20% bugged (n=4)
        // against 1.15-3.77% fixed (n=9), Panorama 32.9% against 0.70-0.96%.
        // Ascend's fixed reading is the noisiest number in this test because its
        // trailed mean is only ~0.04, so absolute noise is large relative to it;
        // 0.06 is ~1.6x its worst fixed reading and less than half its best
        // bugged one. Neither residual is zero and neither should be: a corrected
        // offset is sub-texel at 120 fps, so a higher frame rate means more
        // bilinear taps along the same path, which is blur the correction cannot
        // remove.
        //
        // Tide's control does not reach the others' flatness because at 30 fps
        // the ring advances two slots and the writer backfills both with the
        // same current position rather than interpolating, which shortens the
        // ribbon's effective path slightly. Correct and stale-free, but not free.
        for (effect, control_ceiling, ceiling) in [
            ("Cascade", 0.05, 0.15),
            ("Ascend", 0.05, 0.06),
            ("Tide", 0.12, 0.05),
            ("Panorama", 0.05, 0.05),
        ] {
            let sweep = |decay: f32| -> Vec<f64> {
                [30u32, 60, 120]
                    .iter()
                    .map(|&fps| {
                        let m = mean_luma(&render(effect, fps, decay));
                        println!("PCOMP {effect} decay={decay:.2} fps={fps:<4} mean={m:.6}");
                        m
                    })
                    .collect()
            };
            // CONTROL (#2348's rule): at decay 0 nothing accumulates, so there is
            // no a/(1-k) steady state and the gain is exactly 1 at every rate. Any
            // spread here is the effect's own content being rate-dependent, and
            // bounds what the measurement below can claim.
            let control = sweep(0.0);
            assert!(
                spread(&control) < control_ceiling,
                "{effect}: control is not frame-rate flat ({:.1}%, ceiling {:.0}%) — its \
                 own content has become rate-dependent, so this probe can no longer \
                 isolate the composite gain",
                spread(&control) * 100.0,
                control_ceiling * 100.0,
            );

            let trailed = sweep(0.95);
            assert!(
                trailed.iter().all(|&m| m > control[1] * 1.3),
                "{effect}: trail=0.95 ({trailed:?}) is barely above the no-trail \
                 control ({control:?}) — nothing is accumulating, so a pass is vacuous",
            );
            println!(
                "PCOMP {effect} SUMMARY control_spread={:.4} trailed_spread={:.4}",
                spread(&control),
                spread(&trailed),
            );
            assert!(
                spread(&trailed) < ceiling,
                "{effect}: brightness depends on frame rate — means {trailed:?} \
                 across 30/60/120 fps spread {:.1}%, ceiling {:.0}%. Two different \
                 bugs reach this assertion: the additive composite's source gain is \
                 not tracking the background's retention (#2349), or the shader \
                 resamples its feedback at a per-frame uv offset instead of a \
                 per-second one (#2378). The control above is flat, so it is one of \
                 those and not the effect's own content.",
                spread(&trailed) * 100.0,
                ceiling * 100.0,
            );
        }
    }

    /// Synthetic accents are cycle-periodic (the golden guarantee extends to
    /// synthetic mode) and actually pulse on the beat.
    #[test]
    fn synthetic_accents_pulse_and_close() {
        let (fps, bpm, bars) = (60u32, 120.0f64, 8u32);
        let frames = 960u32; // 8 bars @ 120/60, lossless
        let f0 = synth_features_accented(0, fps, bpm, bars);
        let fn_ = synth_features_accented(frames, fps, bpm, bars);
        assert!((f0.rms - fn_.rms).abs() < 1e-5);
        assert!((f0.buildup - fn_.buildup).abs() < 1e-4);
        assert!((f0.loudness_m - fn_.loudness_m).abs() < 1e-5);
        // On-beat energy beats mid-beat energy.
        let on_beat = synth_features_accented(0, fps, bpm, bars);
        let mid_beat = synth_features_accented(15, fps, bpm, bars);
        assert!(on_beat.rms > mid_beat.rms + 0.2);
        assert!(on_beat.onset > mid_beat.onset + 0.4);
        // Buildup peaks mid-cycle (bar 4 of 8) and rests at the wrap.
        let mid_cycle = synth_features_accented(480, fps, bpm, bars);
        assert!(mid_cycle.buildup > 0.55);
        assert!(f0.buildup < 0.05);
    }

    #[test]
    fn blend_premultiplied_endpoints() {
        let a = vec![10u8, 20, 30, 40];
        let b = vec![110u8, 120, 130, 240];
        assert_eq!(blend_premultiplied(&a, &b, 0.0), a);
        assert_eq!(blend_premultiplied(&a, &b, 1.0), b);
        assert_eq!(blend_premultiplied(&a, &b, 0.5), vec![60, 70, 80, 140]);
    }

    /// P2.7 acceptance: a crossfade render of a stateful effect completes with
    /// the exact spec frame count, and its wrap seam is no worse than the
    /// unblended (time-wrapped) seam of the same spec — a sanity check on the
    /// mechanism, not a quality judgment.
    /// Run: cargo test -p fosfora-app -- --ignored crossfade_reduces
    #[test]
    #[ignore = "GPU"]
    fn crossfade_reduces_the_seam() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }
        let spec = LoopSpec::from_json(
            r#"{"version":1,"effect":"Drift","bpm":120.0,"bars":1,"fps":30,
                "resolution":[128,72],"codec":"h264","background":"opaque"}"#,
        )
        .unwrap();
        let mad = |a: &[u8], b: &[u8]| -> f64 {
            a.iter()
                .zip(b)
                .map(|(&x, &y)| (x as f64 - y as f64).abs())
                .sum::<f64>()
                / a.len() as f64
        };
        // (first frame, last frame, emitted count) for a mode, using the same
        // emission scheme render_loop_with implements.
        let run = |mode: BestEffort| -> (Vec<u8>, Vec<u8>, u32) {
            let mut session =
                LoopSession::create_with(&spec, mode, (*device).clone(), (*queue).clone()).unwrap();
            let n = session.timing.frames;
            let (mut first, mut last, mut count) = (Vec::new(), Vec::new(), 0u32);
            match mode {
                BestEffort::None => unreachable!("exact mode not under test"),
                BestEffort::TimeWrapped => {
                    for k in 0..n {
                        let rgba = session.render_frame_at(k).unwrap();
                        if k == 0 {
                            first = rgba.clone();
                        }
                        if k == n - 1 {
                            last = rgba;
                        }
                        count += 1;
                    }
                }
                BestEffort::Crossfade {
                    tail_bars,
                    warmup_bars,
                } => {
                    let bar_frames = (n / spec.bars.max(1)).max(1);
                    let t = (tail_bars.max(1) * bar_frames).min(n / 2);
                    let w = warmup_bars * bar_frames;
                    let mut head: Vec<Vec<u8>> = Vec::new();
                    for frame in 0..(w + n + t) {
                        let rgba = session.render_frame_at(frame).unwrap();
                        if frame < w {
                            continue;
                        }
                        let k = frame - w;
                        if k < t {
                            head.push(rgba);
                        } else if k < n {
                            if k == t {
                                first = rgba;
                            }
                            count += 1;
                        } else {
                            let i = k - n;
                            let weight = (i + 1) as f32 / (t + 1) as f32;
                            let blended = blend_premultiplied(&rgba, &head[i as usize], weight);
                            if n - t + i == n - 1 {
                                last = blended;
                            }
                            count += 1;
                        }
                    }
                }
            }
            (first, last, count)
        };

        let (tw_first, tw_last, tw_count) = run(BestEffort::TimeWrapped);
        let (x_first, x_last, x_count) = run(BestEffort::Crossfade {
            tail_bars: 1,
            warmup_bars: 1,
        });
        assert_eq!(tw_count, 60);
        assert_eq!(x_count, 60);
        let seam_raw = mad(&tw_last, &tw_first);
        let seam_x = mad(&x_last, &x_first);
        eprintln!("seam raw {seam_raw:.3} vs crossfade {seam_x:.3}");
        assert!(
            seam_x <= seam_raw * 1.05 + 0.5,
            "crossfade seam ({seam_x:.3}) worse than raw seam ({seam_raw:.3})"
        );
    }

    /// The handoff's acceptance numbers: at 120 BPM / 60 fps, beat_phase at
    /// frame 30 is exactly 0.0 (a beat boundary) and the bar index increments
    /// every 120 frames.
    #[test]
    fn synth_arithmetic_is_exact() {
        let f30 = synth_features(30, 60, 120.0);
        assert_eq!(f30.beat_phase, 0.0);
        assert_eq!(f30.beat, 1.0, "frame 30 lands exactly on a beat");
        assert_eq!(synth_features(0, 60, 120.0).downbeat, 1.0);
        assert_eq!(synth_features(0, 60, 120.0).bar_index, 0.0);
        assert_eq!(synth_features(120, 60, 120.0).bar_index, 1.0);
        assert_eq!(synth_features(120, 60, 120.0).downbeat, 1.0);
        assert_eq!(synth_features(121, 60, 120.0).downbeat, 0.0);
        // beat_in_bar steps through {0, .25, .5, .75} across a bar.
        assert_eq!(synth_features(45, 60, 120.0).beat_in_bar, 0.25);
        assert_eq!(synth_features(75, 60, 120.0).beat_in_bar, 0.5);
    }

    /// Loop closure: frame N's phase state equals frame 0's (the golden-loop
    /// premise, checked at the arithmetic level; the GPU probe checks pixels).
    #[test]
    fn phase_state_closes_at_loop_length() {
        let spec = crate::headless::loop_spec::LoopSpec::from_json(
            r#"{"version":1,"effect":"Tessera","bpm":174.0,"bars":8}"#,
        )
        .unwrap();
        let t = spec.snap().unwrap();
        let f0 = synth_features(0, spec.fps, t.effective_bpm);
        let fn_ = synth_features(t.frames, spec.fps, t.effective_bpm);
        assert!((fn_.beat_phase - f0.beat_phase).abs() < 1e-5);
        assert!((fn_.bar_phase - f0.bar_phase).abs() < 1e-5);
        assert_eq!(fn_.beat_in_bar, f0.beat_in_bar);
        assert_eq!(fn_.downbeat, 1.0, "the wrap frame is a downbeat again");
        // Counters differ (monotonic) — phases and pulses are what must close.
    }

    /// Beat pulses are one frame wide and the count over a window is right.
    #[test]
    fn pulses_are_one_frame_wide() {
        for (bpm, fps) in [(120.0, 60u32), (174.0, 60), (98.5, 30), (140.0, 120)] {
            let frames = fps * 8;
            let mut fired = 0i32;
            let mut prev_fired = false;
            for n in 0..frames {
                let f = synth_features(n, fps, bpm);
                if f.beat > 0.5 {
                    fired += 1;
                    assert!(
                        n == 0 || !prev_fired,
                        "consecutive beat pulses at {bpm} bpm {fps} fps frame {n}"
                    );
                    prev_fired = true;
                } else {
                    prev_fired = false;
                }
            }
            let expected = (frames as f64 / fps as f64 * bpm / 60.0).floor() as i32;
            assert!(
                (fired - 1 - expected).abs() <= 1,
                "{bpm} bpm {fps} fps: {fired} pulses vs ~{expected}"
            );
        }
    }
}
