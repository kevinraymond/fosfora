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
        if spec.audio != LoopAudio::None {
            return Err(format!(
                "audio '{:?}' is not wired yet — 'none' (the golden path) is the v1 mode",
                spec.audio
            ));
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
        })
    }

    /// Render one frame (blocking readback) and return its tightly-packed
    /// RGBA8 bytes.
    pub fn render_frame_at(&mut self, frame: u32) -> Result<Vec<u8>, String> {
        let dt = 1.0 / self.fps as f32;
        let features = synth_features(frame, self.fps, self.timing.effective_bpm);
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
    /// Run: cargo test -p phosphor-app -- --ignored golden_loop
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
            checked += 1;
        }
        assert!(
            checked >= 5,
            "expected the phase-locked family, got {checked}"
        );
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
    /// Run: cargo test -p phosphor-app -- --ignored crossfade_reduces
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
