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
        let render =
            |effect: &str, param: &str, audio: LoopAudio, fps: u32, decay: f32| -> Vec<u8> {
                let spec = LoopSpec {
                    version: 1,
                    effect: effect.to_string(),
                    params: [(param.to_string(), crate::params::ParamValue::Float(decay))]
                        .into_iter()
                        .collect(),
                    bpm: 120.0,
                    bars: 8,
                    fps,
                    resolution: [320, 180],
                    codec: crate::headless::loop_spec::LoopCodec::H264,
                    audio,
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
        // Mean absolute per-pixel difference against the 60 fps frame, in 8-bit
        // levels. THIS, not the mean-luma spread above, is the primary guard.
        //
        // Mean luma is a scalar summary of a whole image, so two error modes of
        // opposite sign cancel in it and a wrong render scores flat. Two sites
        // here prove it: Array's joint fix makes its luma spread WORSE (6.8% ->
        // 7.0%) while the image moves closer (1.34 -> 0.74 levels at 120 fps),
        // and Tide read a near-perfect 0.3% luma spread for two releases while
        // its 120 fps image sat 7.2 levels away from its 60 fps one. The contract
        // is "the same effect at another frame rate lands on the same image", and
        // a difference measures that directly.
        let mad = |a: &[u8], b: &[u8]| -> f64 {
            a.iter()
                .zip(b)
                .map(|(&x, &y)| (x as f64 - y as f64).abs())
                .sum::<f64>()
                / a.len() as f64
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
        // #2376 added the remaining in-shader source sites and, with them, the
        // per-pixel difference guard. Measured bugged -> fixed, max over the
        // 30-vs-60 and 120-vs-60 differences, in 8-bit levels:
        //   Tesla    12.14 -> 0.51   the largest frame-rate error in the family,
        //                            and it had never been measured. Its field is
        //                            a continuous source; its beat flash is a
        //                            per-EVENT impulse and is deliberately left
        //                            alone — wrapping that too reads 10.08.
        //   Cascade   4.06 -> 2.24   source gain and advection are ONE fix here.
        //                            Either alone is a regression (19.2% and
        //                            21.9% luma spread against 5.8% shipped);
        //                            the shipped number was two opposing errors
        //                            cancelling.
        //   Tide      7.20 -> 2.77   advection only. Its curtain source scales by
        //                            u.harmonic_energy, which no harness mode
        //                            raises above 0, so the source-gain half is
        //                            unmeasurable here and was not applied.
        //   Array     1.34 -> 1.03   joint, like Cascade. Guarded on the
        //                            difference ONLY: its luma spread gets worse
        //                            when the image gets better.
        //   Vessel    1.50 -> 0.21   joint. Runs under Synthetic, not None: its
        //                            floor glow scales by u.buildup, which is 0
        //                            in the neutral vector, so under None the
        //                            whole source term is identically zero and
        //                            the trail never accumulates (ratio 1.06).
        //                            That vacuity, not the param name, is why it
        //                            used to be excluded.
        //
        // Deliberately NOT in this list:
        //   Cymatics — measured and NOT fixed. Three real bugs (an input clamp
        //              and a 0.85 cap that bind in series, and a vignette that
        //              multiplies the stored trail every frame, so it is a
        //              spatially-varying per-frame retention rather than a
        //              display mask). Correcting the vignette collapses its luma
        //              spread 36.9% -> 7.7% but leaves the image no closer
        //              (18.70/10.53 -> 16.17/12.91), so nothing was landed. Its
        //              control difference is 5.8 levels, the highest here, which
        //              says the dominant term is its own sim content (#2380).
        //   Cleave   — measured and unmoved: 4.76/4.11 shipped against 4.61/4.54
        //              with both corrections. Neither is its mechanism.
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
        // (effect, trail param, audio, control ceiling, luma-spread ceiling,
        //  image-difference ceiling). A `None` luma ceiling means that statistic
        //  is not usable at this site — see Array above.
        struct Site {
            effect: &'static str,
            /// The effect input holding this site's feedback retention.
            param: &'static str,
            audio: LoopAudio,
            /// Cap on the decay-0 control's own spread: how rate-dependent this
            /// effect's content is, and so how much the reading below can claim.
            control: f64,
            /// Cap on the mean-luma spread, or `None` where that statistic is
            /// not usable at this site (Array, Tide — see above).
            luma: Option<f64>,
            /// Cap on the per-pixel image difference. The primary guard.
            mad: f64,
            /// How far above the control the trailed mean must sit for a pass to
            /// mean anything. `None` where there is no zero-trail control to
            /// compare against — see Vessel.
            vacuity: Option<f64>,
        }
        const N: LoopAudio = LoopAudio::None;
        const S: LoopAudio = LoopAudio::Synthetic;
        #[rustfmt::skip]
        let sites = [
            Site { effect: "Cascade",  param: "trail_decay", audio: N, control: 0.05, luma: Some(0.15), mad: 3.00, vacuity: Some(1.3) },
            Site { effect: "Ascend",   param: "trail_decay", audio: N, control: 0.05, luma: Some(0.06), mad: 1.60, vacuity: Some(1.3) },
            Site { effect: "Tide",     param: "trail_decay", audio: N, control: 0.12, luma: None,       mad: 4.00, vacuity: Some(1.3) },
            Site { effect: "Panorama", param: "trail_decay", audio: N, control: 0.05, luma: Some(0.05), mad: 0.50, vacuity: Some(1.3) },
            Site { effect: "Tesla",    param: "trail_decay", audio: N, control: 0.05, luma: Some(0.15), mad: 2.00, vacuity: Some(1.3) },
            Site { effect: "Array",    param: "trail_decay", audio: N, control: 0.05, luma: None,       mad: 1.15, vacuity: Some(1.3) },
            // Vessel's retention is `glow` remapped onto [0.78, 0.93], so glow=0
            // is still a decay of 0.78 — a SHORTER trail, not no trail. Its luma
            // "control" is therefore not a control, and the 1.3x ratio above it
            // is unreachable by design (measured 1.35x, which would leave every
            // pass one rounding away from a spurious failure). Non-vacuity comes
            // from the image difference instead, where the separation is wide:
            // control 0.09, fixed 0.21, and 1.50 with the bug reintroduced.
            Site { effect: "Vessel",   param: "glow",        audio: S, control: 0.05, luma: Some(0.10), mad: 0.60, vacuity: None },
        ];
        for Site {
            effect,
            param,
            audio,
            control: control_ceiling,
            luma: ceiling,
            mad: mad_ceiling,
            vacuity,
        } in sites
        {
            let sweep = |decay: f32| -> (Vec<f64>, [f64; 2]) {
                let frames: Vec<Vec<u8>> = [30u32, 60, 120]
                    .iter()
                    .map(|&fps| render(effect, param, audio, fps, decay))
                    .collect();
                let means: Vec<f64> = frames.iter().map(|px| mean_luma(px)).collect();
                for (i, fps) in [30u32, 60, 120].iter().enumerate() {
                    println!(
                        "PCOMP {effect} decay={decay:.2} fps={fps:<4} mean={:.6}",
                        means[i]
                    );
                }
                (
                    means,
                    [mad(&frames[0], &frames[1]), mad(&frames[2], &frames[1])],
                )
            };
            // CONTROL (#2348's rule): at decay 0 nothing accumulates, so there is
            // no a/(1-k) steady state and the gain is exactly 1 at every rate. Any
            // spread here is the effect's own content being rate-dependent, and
            // bounds what the measurement below can claim.
            let (control, control_mad) = sweep(0.0);
            assert!(
                spread(&control) < control_ceiling,
                "{effect}: control is not frame-rate flat ({:.1}%, ceiling {:.0}%) — its \
                 own content has become rate-dependent, so this probe can no longer \
                 isolate the composite gain",
                spread(&control) * 100.0,
                control_ceiling * 100.0,
            );

            let (trailed, trailed_mad) = sweep(0.95);
            if let Some(vacuity) = vacuity {
                assert!(
                    trailed.iter().all(|&m| m > control[1] * vacuity),
                    "{effect}: trail=0.95 ({trailed:?}) is barely above the no-trail \
                     control ({control:?}) — nothing is accumulating, so a pass is vacuous",
                );
            } else {
                // The image difference has to carry the non-vacuity argument on
                // its own here, so require the trail to move the picture well
                // clear of what the control already moves it.
                assert!(
                    trailed_mad[0].max(trailed_mad[1]) > control_mad[0].max(control_mad[1]) * 1.5,
                    "{effect}: the trail barely changes the image relative to the control \
                     ({trailed_mad:?} vs {control_mad:?}) — a pass would be vacuous",
                );
            }
            let worst_mad = trailed_mad[0].max(trailed_mad[1]);
            println!(
                "PCOMP {effect} SUMMARY control_spread={:.4} trailed_spread={:.4} \
                 mad30={:.2} mad120={:.2} (control {:.2}/{:.2}) ceiling={mad_ceiling:.2}",
                spread(&control),
                spread(&trailed),
                trailed_mad[0],
                trailed_mad[1],
                control_mad[0],
                control_mad[1],
            );
            if let Some(ceiling) = ceiling {
                assert!(
                    spread(&trailed) < ceiling,
                    "{effect}: brightness depends on frame rate — means {trailed:?} \
                     across 30/60/120 fps spread {:.1}%, ceiling {:.0}%. Several bugs \
                     reach this assertion: the additive composite's source gain is not \
                     tracking the background's retention (#2349), an in-shader source \
                     term's gain is still per-frame (#2376), or the shader resamples \
                     its feedback at a per-frame uv offset instead of a per-second one \
                     (#2378). The control above is flat, so it is one of those and not \
                     the effect's own content.",
                    spread(&trailed) * 100.0,
                    ceiling * 100.0,
                );
            }
            // The primary guard. Unlike the spread above it cannot be satisfied
            // by two errors cancelling, so it is the one to trust when the two
            // disagree — and at Array and Tide they do.
            assert!(
                worst_mad < mad_ceiling,
                "{effect}: the 30 and 120 fps renders do not land on the same image as \
                 the 60 fps one — mean per-pixel difference {:.2}/{:.2} levels of 255 \
                 (ceiling {mad_ceiling:.2}), against {:.2}/{:.2} for the no-trail \
                 control. Something in this effect's feedback loop is still measured \
                 in frames rather than seconds (#2376/#2378).",
                trailed_mad[0],
                trailed_mad[1],
                control_mad[0],
                control_mad[1],
            );
        }
    }

    /// #2380, the sixth and last category of the frame-rate family: per-frame
    /// temporal constants inside the particle SIM shaders.
    ///
    /// Every probe before this one measures a TRAIL. A sim bug does not change
    /// the trail, it changes the CONTENT the trail is drawn from, so it shows up
    /// in the sibling probe's decay-0 *control* reading and nowhere in its
    /// headline number. That is why 27 `*_sim.wgsl` went unswept while five
    /// rounds combed the `*_bg` shaders: nothing here ever failed.
    ///
    /// Three things this probe does differently, each of which cost an
    /// experiment to learn:
    ///
    /// 1. WINDOW_SECS = 4.0, not the sibling's 1.2. These are growth sims —
    ///    Mycelium builds a network, Chaos fills an attractor, Accretion forms a
    ///    disc — and at 1.2 s they have barely started. Their contribution there
    ///    is 1.2e-5..5e-4 of full scale, i.e. at or under the 8-bit readback
    ///    floor, so every ratio computed in that regime is two noise numbers
    ///    divided by each other. Two experiments returned null at 1.2 s and were
    ///    contradicted at 4.0 (#2379).
    ///
    /// 2. The trail is turned OFF, not turned up. Zeroing the retention removes
    ///    the `*_bg` feedback the sibling probe already guards and leaves the sim
    ///    as the only thing that can move the image. Four effects here expose no
    ///    such param and run at shipped defaults instead — their reading is
    ///    therefore sim AND background, and is a ceiling on the sim rather than a
    ///    measurement of it.
    ///
    /// 3. Per-site audio mode. A per-frame velocity kick scaled by `u.onset` or
    ///    `u.flux` is identically zero under `LoopAudio::None`, so the whole term
    ///    vanishes and the site measures nothing — the same vacuity that kept
    ///    Vessel out of the sibling probe for two releases. Sites whose bug is
    ///    level-gated run Synthetic.
    ///
    /// The statistic is the mean absolute per-pixel difference against the 60 fps
    /// render, in 8-bit levels — NOT a mean-luma spread. A scalar summary of an
    /// image cancels two opposite-sign errors and scores a wrong render flat;
    /// Tide passed at 0.29% for two releases with its 120 fps image 7.2 levels
    /// off (#2381). "The same effect at another frame rate lands on the same
    /// image" is the contract, and a difference measures it directly.
    ///
    /// Run: cargo test -p fosfora-app --release -- --ignored frame_rate_independent_particle_sim
    #[test]
    #[ignore = "GPU"]
    fn frame_rate_independent_particle_sim() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }

        const WINDOW_SECS: f32 = 4.0;
        let render =
            |effect: &str, trail_param: Option<&str>, audio: LoopAudio, fps: u32| -> Vec<u8> {
                let spec = LoopSpec {
                    version: 1,
                    effect: effect.to_string(),
                    params: trail_param
                        .map(|p| (p.to_string(), crate::params::ParamValue::Float(0.0)))
                        .into_iter()
                        .collect(),
                    bpm: 120.0,
                    bars: 8,
                    fps,
                    resolution: [320, 180],
                    codec: crate::headless::loop_spec::LoopCodec::H264,
                    audio,
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
        let mad = |a: &[u8], b: &[u8]| -> f64 {
            a.iter()
                .zip(b)
                .map(|(&x, &y)| (x as f64 - y as f64).abs())
                .sum::<f64>()
                / a.len() as f64
        };
        let mean_luma = |px: &[u8]| -> f64 {
            px.chunks_exact(4)
                .map(|p| (p[0] as f64 + p[1] as f64 + p[2] as f64) / 765.0)
                .sum::<f64>()
                / (px.len() / 4) as f64
        };

        struct SimSite {
            effect: &'static str,
            /// The input holding this effect's feedback retention, zeroed so the
            /// `*_bg` trail cannot contribute to the reading. `None` where the
            /// effect exposes no such param — those run at shipped defaults, so
            /// their number bounds the sim rather than isolating it.
            trail_param: Option<&'static str>,
            /// Synthetic wherever the suspect term is scaled by an audio LEVEL,
            /// or the term is identically zero and the site is vacuous.
            audio: LoopAudio,
            /// Cap on the BLOCK-averaged difference against the 60 fps render.
            /// The per-pixel figure is reported but not asserted: on stochastic
            /// content it is dominated by which particles landed where, not by
            /// whether the effect is right (Flux reads 27.6 per-pixel and 5.4
            /// blocked, and the emitter's atomic claim accounts for the gap).
            mad: f64,
            /// What this site is here to witness. `None` marks a floor reference:
            /// a sim already correct throughout, whose reading IS the noise floor
            /// for the rest of the list.
            witness: Option<&'static str>,
        }
        const N: LoopAudio = LoopAudio::None;
        const S: LoopAudio = LoopAudio::Synthetic;
        const T: Option<&'static str> = Some("trail_decay");
        // THREE SITES ARE GUARDED — a ceiling set between a measured bugged and a
        // measured fixed reading, with the bug reintroduced afterwards to watch
        // this test fail. The rest are WATCHED: a loose ceiling that catches a
        // gross regression but proves nothing, because no fix was landed there
        // and there is no bugged/fixed pair to bracket. The distinction is in the
        // comment on each row; do not tighten a watched ceiling to its measured
        // value, because a ceiling nobody has seen fail is not a guard.
        #[rustfmt::skip]
        let sites = [
            // FLOOR REFERENCES. Every damping in these three is already
            // pow(k, dt*60) or 1-exp(-k*dt), so whatever they read is stochastic
            // spawn scatter, not a bug. Nothing below their level is a claim.
            // Morph is the true zero: it places particles from a target image and
            // never touches u.seed, so it reads 0.00 on every statistic.
            SimSite { effect: "Pegboard",     trail_param: None, audio: N, mad: 0.60, witness: None },
            SimSite { effect: "Morph",        trail_param: None, audio: N, mad: 0.10, witness: None },
            SimSite { effect: "Cleave",       trail_param: T,    audio: N, mad: 0.40, witness: None },
            // GUARDED. Sub-stepping the RK4 integrator: 0.75 -> 0.06 blocked, and
            // the brightness ramp across 30/60/120 went 4.35/3.77/3.48 (-20%) to
            // 3.81/3.77/3.74 (-1.8%). 60 fps is unchanged at 3.77.
            SimSite { effect: "Chaos",        trail_param: T,    audio: N, mad: 0.30, witness: Some("chaos_sim:317 fixed RK4 step") },
            // GUARDED, and deliberately run under Synthetic. Under None its two
            // velocity terms are INVISIBLE: u.bass and u.mid are 0, so `desired`
            // barely moves and the velocity converges to it whatever the blend
            // weight is — reverting both retention fixes there reads 0.83 against
            // 0.83, a perfectly vacuous guard. Drive the target and the same two
            // terms separate cleanly, 0.37 fixed against 0.49 bugged with a
            // self-difference of 0.00. The branch gate shows up in either mode.
            // Landed here: tick-accurate branch gate, plus frame_diffuse on the
            // velocity blend and frame_decay on the damping.
            //
            // PARTIAL, and the residual is characterised rather than hidden: with
            // branching disabled the sim is EXACTLY rate-independent at every
            // rate (3.32/3.32/3.32), so what is left is entirely branch-driven
            // growth. See the board before assuming it is one of these terms.
            SimSite { effect: "Mycelium",     trail_param: T,    audio: S, mad: 0.44, witness: Some("mycelium_sim:109,161 + branch gate :248") },
            // WATCHED. Its vel *= 0.90 is a real per-frame retention by reading,
            // but at 1.06 against a 0.47 floor it is 2.2x noise and no fix was
            // attempted; see the board.
            SimSite { effect: "Reliquary",    trail_param: T,    audio: N, mad: 1.60, witness: Some("reliquary_sim:177 vel *= 0.90") },
            // GUARDED. frame_diffuse on the heading low-pass: 3.87 -> 1.51
            // blocked, with mean brightness flat throughout — the flock is the
            // same size and was simply in the wrong places.
            SimSite { effect: "Murmur",       trail_param: None, audio: N, mad: 2.50, witness: Some("murmur_sim:338 angular low-pass") },
            // WATCHED, AND THIS ROW CANNOT ISOLATE ITS SIM. trail_decay has
            // min 0.70 and polycephalum_bg floors it again at 0.5, so `trail_param`
            // does NOT turn the trail off here and this reading is sim plus
            // feedback. Forcing the bg decay to 0 in a scratch build drops it to
            // 0.19 against a 0.19 floor — the sim is clean, the ramp is the
            // feedback path, and two sim "fixes" measured exactly zero and were
            // reverted. Do not treat this number as a sim measurement.
            SimSite { effect: "Polycephalum", trail_param: T,    audio: N, mad: 2.40, witness: Some("polycephalum_sim: NOT isolatable, see comment") },
            // WATCHED. The linearised drag at these three is real by reading and
            // measured at NOTHING: swapping 1-(1-k)*n for pow(k,n) moved Flux
            // 27.52 -> 27.26 per-pixel, because the shipped drag values are
            // 0.995/0.97/0.997 where the two forms differ by ~2.5e-5 per frame.
            // Worth replacing one day for the drag < 0.5 case, where the linear
            // form goes NEGATIVE on a stalled frame and reverses velocity, but
            // that is a latent hazard and not a measured defect.
            SimSite { effect: "Cymatics",     trail_param: T,    audio: N, mad: 1.80, witness: Some("cymatics_sim:183 linearised drag") },
            // WATCHED, MECHANISM ESTABLISHED, NOT FIXED (#2383). Two things this
            // row used to claim are measured false, both by `psim_population`:
            //
            //  - It is NOT population. Flux's live count is 808466 / 805133 /
            //    803466 at 30 / 60 / 120 — flat to 0.6% and sloping the wrong way
            //    — while the level ramps +33%. "It is COUNT" was inferred from
            //    level ~ count x alpha x size^2, never measured.
            //  - `trail_param` does NOT turn Flux's trail off. `trail_decay` is
            //    declared min 0.8 in flux.pfx, so the 0.0 above clamps to 0.8 and
            //    this reading is sim AND feedback — the same trap already recorded
            //    at Polycephalum, one row down. It happens not to matter here
            //    (cutting the history pass to `col = bg` leaves the ramp fully
            //    intact) but the row cannot be read as a sim measurement.
            //
            // What it actually is: emission is quantised to the frame, so the
            // number of distinct spawn INSTANTS per second equals the frame rate.
            // A concentrating flow collapses each spawn cohort onto a filament, so
            // 30 fps draws 121 thick filaments where 120 fps draws 481 thin ones —
            // same particles, less screen covered (empty pixels 14.4% vs 5.7%) —
            // and the resolve's concave x/(1+x) turns that into mean level.
            // Forcing 30 emission phases per second at every rate collapses the
            // ramp from +33% to -4.4%; see the board for the full cell record.
            SimSite { effect: "Flux",         trail_param: T,    audio: N, mad: 7.00, witness: Some("flux_sim: frame-quantised emission phase, deferred") },
            // Same mechanism, ~a third the magnitude (+10% level, count flat).
            SimSite { effect: "Phosphor",     trail_param: T,    audio: N, mad: 1.60, witness: Some("phosphor_sim: frame-quantised emission phase") },
            // WATCHED. The u.onset/u.flux/u.zcr kicks are per-frame rates by
            // reading (onset is a held, decayed LEVEL — beat.rs:1918 — not a
            // one-frame pulse, so #2382 makes them rates). All four measure at or
            // under their own noise on both statistics, so nothing was changed.
            SimSite { effect: "Array",        trail_param: T,    audio: S, mad: 0.30, witness: Some("array_sim:206 u.onset kick") },
            SimSite { effect: "Cascade",      trail_param: T,    audio: S, mad: 0.50, witness: Some("cascade_sim:184 u.onset kick") },
            SimSite { effect: "Tesla",        trail_param: T,    audio: S, mad: 0.30, witness: Some("tesla_sim:394 u.onset kick") },
            SimSite { effect: "Accretion",    trail_param: T,    audio: S, mad: 0.30, witness: Some("accretion_sim:363 u.flux kick") },
            // WATCHED and UNMEASURABLE: its block self-difference (2.49) is its
            // whole reading (2.20). Nothing can be concluded here at any effort
            // without a different instrument.
            SimSite { effect: "Symbiosis",    trail_param: None, audio: S, mad: 4.00, witness: Some("symbiosis_sim: self-difference = reading") },
        ];

        // Bisecting one site means editing its shader and re-reading it, and
        // assets/ is read at RUNTIME, so a cell is a text edit plus a test run
        // with no rebuild. Running all sixteen sites per cell would make that
        // two minutes instead of five seconds. Unset, every site runs.
        let only = std::env::var("PSIM_ONLY").unwrap_or_default();
        // Audio mode override, for asking whether a site's verdict is a property
        // of the effect or of the regime it was measured in. A per-frame blend
        // weight toward an audio-driven target is invisible under None, because
        // the target barely moves and the velocity converges to it whatever the
        // weight is; the same term is live the moment the target changes every
        // frame. A null result under one mode is not a null result.
        let audio_override = match std::env::var("PSIM_AUDIO").unwrap_or_default().as_str() {
            "synth" => Some(LoopAudio::Synthetic),
            "none" => Some(LoopAudio::None),
            _ => None,
        };
        let mut failures = Vec::new();
        for SimSite {
            effect,
            trail_param,
            audio,
            mad: mad_ceiling,
            witness,
        } in sites
        {
            if !only.is_empty() && !only.split(',').any(|s| s.eq_ignore_ascii_case(effect)) {
                continue;
            }
            let audio = audio_override.unwrap_or(audio);
            // AVERAGE N RENDERS PER RATE, and compare the averages.
            //
            // `emit_claim()` is an atomicAdd (particle_lib.wgsl:587), so which
            // dead slot wins which emission ticket depends on GPU workgroup
            // scheduling and is not reproducible run to run. `emit_particle`
            // then seeds position, colour and lifetime from the SLOT INDEX, so a
            // different claim order is a different picture. Measured raw, that
            // noise reaches 7.98 levels at Symbiosis — larger than most of the
            // frame-rate differences this test is trying to detect.
            //
            // It is inherent to the emitter and cannot be switched off from a
            // spec, so it gets averaged down instead: the systematic part of a
            // frame-rate difference survives averaging, the scheduling noise
            // falls as 1/sqrt(N). The 60 fps reference is rendered 2N times and
            // split in half, so `self` below is the residual floor of exactly the
            // same statistic as the two readings beside it — not a different one.
            const N_RUNS: usize = 3;
            let avg = |runs: &[Vec<u8>]| -> Vec<f64> {
                let mut acc = vec![0.0f64; runs[0].len()];
                for r in runs {
                    for (a, &b) in acc.iter_mut().zip(r) {
                        *a += b as f64;
                    }
                }
                acc.iter().map(|a| a / runs.len() as f64).collect()
            };
            let mad_f = |a: &[f64], b: &[f64]| -> f64 {
                a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f64>() / a.len() as f64
            };
            let runs_at = |fps: u32, n: usize| -> Vec<Vec<u8>> {
                (0..n)
                    .map(|_| render(effect, trail_param, audio, fps))
                    .collect()
            };
            let r60 = runs_at(60, N_RUNS * 2);
            let a60 = avg(&r60[..N_RUNS]);
            let b60 = avg(&r60[N_RUNS..]);
            let a30 = avg(&runs_at(30, N_RUNS));
            let a120 = avg(&runs_at(120, N_RUNS));
            let frames = [r60[0].clone()];
            let self_mad = mad_f(&a60, &b60);
            let d30 = mad_f(&a30, &a60);
            let d120 = mad_f(&a120, &a60);
            let _ = &mad;

            // BLOCK-AVERAGED DIFFERENCE, on a 16x9 grid of 20x20 blocks.
            //
            // A per-pixel difference cannot tell "the same content, redistributed"
            // from "systematically different content", and both are frame-rate
            // dependent for entirely different reasons. Particles advected through
            // a spatially varying field on a first-order integrator take different
            // trajectories at different dt — real, but the DISTRIBUTION is
            // unchanged and no viewer could pick the two apart. A per-frame
            // damping constant or a per-frame growth gate moves the distribution
            // itself.
            //
            // Blocking separates them: a different draw from the same distribution
            // averages toward the same block means, a systematically wrong one does
            // not. This is the counterpart to #2381 — that finding says a scalar
            // summary of a whole image CANCELS real errors, and this one says a
            // per-pixel difference INVENTS them on stochastic content. Neither
            // statistic is sufficient alone, so both are reported, each against its
            // own same-rate floor.
            const BW: usize = 20;
            let blocks = |img: &[f64]| -> Vec<f64> {
                let (w, h) = (320usize, 180usize);
                let bx = w / BW;
                let mut out = vec![0.0f64; bx * (h / BW) * 4];
                for y in 0..h {
                    for x in 0..w {
                        let b = (y / BW) * bx + (x / BW);
                        for c in 0..4 {
                            out[b * 4 + c] += img[(y * w + x) * 4 + c];
                        }
                    }
                }
                let n = (BW * BW) as f64;
                out.iter().map(|v| v / n).collect()
            };
            let (k30, k60, k120, k60b) = (blocks(&a30), blocks(&a60), blocks(&a120), blocks(&b60));
            let b_self = mad_f(&k60, &k60b);
            let b_worst = mad_f(&k30, &k60).max(mad_f(&k120, &k60));

            // MEAN LEVEL PER RATE. The cheapest discriminator of the two, and it
            // needs no grid: if the three rates agree on average brightness but
            // differ per pixel, content moved around. If the mean itself moves,
            // something is accumulating per frame instead of per second — a RATE,
            // and genuinely wrong (#2382).
            let lvl = |img: &[f64]| -> f64 {
                img.chunks_exact(4)
                    .map(|p| (p[0] + p[1] + p[2]) / 3.0)
                    .sum::<f64>()
                    / (img.len() / 4) as f64
            };
            let (l30, l60, l120) = (lvl(&a30), lvl(&a60), lvl(&a120));
            let worst = d30.max(d120);
            let luma = mean_luma(&frames[0]);
            // Mean level of the 60 fps frame, so a difference can be read as a
            // fraction of the signal that produced it. 0.95 levels on a frame
            // averaging 3.8 is a quarter of the picture; 1.81 on one averaging 90
            // is nothing. Ranking on the absolute column alone inverts those two.
            let level = luma * 255.0;
            let rel = if level > 1e-6 { worst / level } else { 0.0 };
            let mode = match audio {
                LoopAudio::Synthetic => "synth",
                _ => "none",
            };
            println!(
                "PSIM {effect:<13} px={worst:6.2}/{self_mad:<5.2} blk={b_worst:6.2}/{b_self:<5.2} \
                 lvl={l30:6.2}/{l60:6.2}/{l120:6.2} rel={:4.0}% audio={mode} trail={} \
                 ceiling={mad_ceiling:.2} :: {}",
                rel * 100.0,
                if trail_param.is_some() { "off" } else { "dflt" },
                witness.unwrap_or("FLOOR REFERENCE"),
            );
            let _ = (d30, d120, level);

            // A dark frame makes any difference small for the wrong reason. This
            // is the vacuity guard the sibling probe learned to need at Vessel:
            // a site that renders nothing passes every ceiling forever.
            assert!(
                luma > 1e-4,
                "{effect}: the 60 fps frame is essentially black (mean luma {luma:.6}) — \
                 nothing is being drawn, so any reading below is vacuous. Check that this \
                 effect spawns under LoopAudio::{mode}.",
            );
            if b_worst >= mad_ceiling {
                failures.push(format!(
                    "{effect}: the 30 and 120 fps renders do not land on the same image as the \
                     60 fps one — block difference {b_worst:.2} levels of 255 against a same-rate \
                     floor of {b_self:.2}, ceiling {mad_ceiling:.2}. Mean level per rate was \
                     {l30:.2}/{l60:.2}/{l120:.2}: if that ramps monotonically, something in this \
                     sim accumulates per FRAME instead of per second. Witness: {}",
                    witness.unwrap_or("(floor reference — this should never fire)"),
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "a particle sim is measuring time in frames rather than seconds (#2380):\n  {}",
            failures.join("\n  "),
        );
    }

    /// DIAGNOSTIC INSTRUMENT, not a guard (#2383). It asserts nothing; it prints
    /// the numbers that decide which of several stories about a particle effect
    /// is true. Every other probe in this file measures an IMAGE, and an image
    /// statistic cannot tell "more particles" from "the same particles piled up".
    ///
    /// It reports, per frame rate: the live particle COUNT read off the GPU
    /// counter (`system.rs` `alive_count`), the mean level, the level with the
    /// resolve's `x/(1+x)` tonemap inverted per pixel, and a shape summary
    /// (sd / p50 / p99 / empty / saturated).
    ///
    /// WHAT IT ESTABLISHED, and why each column exists:
    ///
    /// 1. `alive` REFUTED #2383's headline. Flux reads 808466 / 805133 / 803466
    ///    at 30 / 60 / 120 fps — flat to 0.6%, and sloping the WRONG way (that
    ///    residual is the 1-frame counter lag, worth 6681 particles at 30 fps and
    ///    1670 at 120) — while the level ramps +33%. "Population scales with
    ///    frame rate" was inferred from level ~ count x alpha x size^2 and never
    ///    measured. The count is rate-independent.
    ///
    /// 2. `lin` separates redistribution from light. The resolve tonemaps with
    ///    `x/(1+x)`, which is CONCAVE, so a clustered field resolves dimmer than
    ///    a spread one carrying the same total. Inverting it recovers most of the
    ///    gap: linearising the resolve took Flux's ramp from +33% to +10.9%.
    ///
    /// 3. `zero` / `p50` / `p99` name the mechanism where the mean cannot. Flux
    ///    at 30 vs 120 fps: p50 15.0 -> 29.7 while p99 FALLS 145 -> 132, and
    ///    empty pixels drop 14.4% -> 5.7%. Same particles, clumped at 30 and
    ///    spread at 120. A mean alone reads that as "brighter".
    ///
    /// `PSIM_SECS` and `PSIM_FRAMES` are the two knobs that cracked it, because
    /// they separate variables no single-window probe can. Sweeping `PSIM_SECS`
    /// showed the ramp is not a per-frame constant at all: it is INVERTED at
    /// 0.25 s (-23%), crosses zero near 0.75 s, and grows without bound
    /// (+33% at 4 s, +53% at 8 s) — an accumulating divergence. `PSIM_FRAMES`
    /// pins the dispatch count equal across rates so wall-clock and frame count
    /// can be told apart; at a fixed 481 frames, 30 fps carries 2.5x MORE
    /// particles than 120 and still renders DIMMER (28.00 vs 40.06).
    ///
    /// Run: PSIM_ONLY=Flux cargo test -p fosfora-app --release -- --ignored psim_population
    #[test]
    #[ignore = "GPU"]
    fn psim_population() {
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let (device, queue) = crate::gpu::test_gpu::test_gpu();
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }
        let window_secs: f32 = std::env::var("PSIM_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4.0);
        let only = std::env::var("PSIM_ONLY").unwrap_or_else(|_| "Flux".into());
        let audio = match std::env::var("PSIM_AUDIO").unwrap_or_default().as_str() {
            "synth" => LoopAudio::Synthetic,
            _ => LoopAudio::None,
        };
        // PSIM_RES / PSIM_PNG exist to JUDGE rather than to measure: 320x180 is
        // fine for a statistic and useless for an eye. PSIM_TRAIL=default leaves
        // the effect's shipped trail alone, which is the only configuration
        // worth looking at when the question is what the effect looks like.
        let res: u32 = std::env::var("PSIM_RES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(320);
        let png_dir = std::env::var("PSIM_PNG").ok();
        let want_trail_off = std::env::var("PSIM_TRAIL").unwrap_or_default() != "default";
        if let Some(d) = &png_dir {
            std::fs::create_dir_all(d).unwrap();
        }
        for effect in only.split(',') {
            for fps in [30u32, 60, 120] {
                let spec_for = |trail: bool| LoopSpec {
                    version: 1,
                    effect: effect.to_string(),
                    params: (trail && want_trail_off)
                        .then(|| {
                            (
                                "trail_decay".to_string(),
                                crate::params::ParamValue::Float(0.0),
                            )
                        })
                        .into_iter()
                        .collect(),
                    bpm: 120.0,
                    bars: 8,
                    fps,
                    resolution: [res, res * 9 / 16],
                    codec: crate::headless::loop_spec::LoopCodec::H264,
                    audio,
                    audio_file: None,
                    background: LoopBackground::Opaque,
                };
                // Not every effect declares `trail_decay`, and one that does not
                // rejects the spec outright (Pegboard). Fall back rather than
                // panic, and say so, because a site running at shipped trail
                // defaults reports sim AND feedback rather than the sim alone.
                let mut session = match LoopSession::create_with(
                    &spec_for(true),
                    BestEffort::TimeWrapped,
                    (*device).clone(),
                    (*queue).clone(),
                ) {
                    Ok(s) => s,
                    Err(_) => {
                        if fps == 30 {
                            println!("POP {effect:<13} (no trail_decay param — shipped trail on)");
                        }
                        LoopSession::create_with(
                            &spec_for(false),
                            BestEffort::TimeWrapped,
                            (*device).clone(),
                            (*queue).clone(),
                        )
                        .unwrap_or_else(|e| panic!("{effect} @ {fps}fps: {e}"))
                    }
                };
                // PSIM_FRAMES pins the DISPATCH COUNT equal across rates instead
                // of the wall-clock window, which separates "per second" from
                // "per frame": if a reading tracks the frame count it is the
                // same at 30 and 120 fps here, and if it tracks seconds it is not.
                let last = match std::env::var("PSIM_FRAMES")
                    .ok()
                    .and_then(|v| v.parse().ok())
                {
                    Some(n) => n,
                    None => (window_secs * fps as f32).round() as u32,
                };
                let mut px = Vec::new();
                for f in 0..=last {
                    px = session.render_frame_at(f).unwrap();
                }
                // Drain the readback so alive_count reflects a recent frame
                // rather than whatever the last completed map happened to hold.
                for _ in 0..4 {
                    let _ = device.poll(wgpu::PollType::Poll);
                    for layer in &mut session.sr.layer_stack.layers {
                        if let Some(e) = layer.as_effect_mut() {
                            if let Some(ps) = &mut e.pass_executor.particle_system {
                                ps.poll_counter_readback();
                            }
                        }
                    }
                }
                let mut alive = 0u32;
                let mut maxp = 0u32;
                for layer in &session.sr.layer_stack.layers {
                    if let Some(e) = layer.as_effect() {
                        if let Some(ps) = &e.pass_executor.particle_system {
                            alive = ps.alive_count;
                            maxp = ps.max_particles;
                        }
                    }
                }
                if let Some(d) = &png_dir {
                    let (w, h) = (res, res * 9 / 16);
                    let img = image::RgbaImage::from_raw(w, h, px.clone()).unwrap();
                    let path = std::path::Path::new(d).join(format!("{effect}-{fps}fps.png"));
                    img.save(&path).unwrap();
                    println!("PNG {}", path.display());
                }
                let lum: Vec<f64> = px
                    .chunks_exact(4)
                    .map(|p| (p[0] as f64 + p[1] as f64 + p[2] as f64) / 3.0)
                    .collect();
                let n = lum.len() as f64;
                let level = lum.iter().sum::<f64>() / n;
                // Is it the SAME light redistributed, or MORE light? The resolve
                // tonemaps with x/(1+x), which is concave, so a clustered field
                // resolves dimmer than a spread one at equal total. Inverting it
                // per pixel recovers the pre-tonemap sum: if `lin` is flat across
                // rates and `lvl` is not, nothing is brighter — it moved.
                let lin: f64 = lum
                    .iter()
                    .map(|&v| {
                        let x = (v / 255.0).min(0.999);
                        x / (1.0 - x)
                    })
                    .sum::<f64>()
                    / n;
                let mut s = lum.clone();
                s.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let pct = |q: f64| s[((s.len() - 1) as f64 * q) as usize];
                let zero = lum.iter().filter(|&&v| v < 0.5).count() as f64 / n;
                let sat = lum.iter().filter(|&&v| v > 250.0).count() as f64 / n;
                let var = lum.iter().map(|v| (v - level).powi(2)).sum::<f64>() / n;
                let frames = last + 1;
                println!(
                    "POP {effect:<13} fps={fps:<4} alive={alive:<9} lvl={level:6.2} \
                     lin={lin:7.3} sd={:6.2} p50={:5.1} p99={:5.1} zero={:5.1}% sat={:5.2}% \
                     frames={frames}",
                    var.sqrt(),
                    pct(0.50),
                    pct(0.99),
                    zero * 100.0,
                    sat * 100.0,
                );
                let _ = maxp;
            }
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
