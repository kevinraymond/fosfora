//! The `.loop.json` spec (#2063, Phase 2): the serializable, versioned
//! description of a loop render. This file is the Phase 3 product artifact —
//! a few hundred bytes of text that reproduces a full clip — so it is a public
//! format: per-field serde defaults, explicit version check, actionable
//! validation errors.
//!
//! BPM ↔ frame snapping (P2.4) lives here too: a loop is seamless only if it
//! spans an integer number of frames, so the requested BPM is nudged to the
//! nearest tempo that closes exactly. `effective_bpm` is derived, never stored.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::effect::format::{LoopMode, PfxEffect};
use crate::params::ParamValue;

pub const LOOP_SPEC_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LoopCodec {
    Hap,
    #[default]
    HapAlpha,
    Prores4444,
    H264,
    Hevc,
}

impl LoopCodec {
    pub fn carries_alpha(self) -> bool {
        matches!(self, LoopCodec::HapAlpha | LoopCodec::Prores4444)
    }

    pub fn extension(self) -> &'static str {
        match self {
            LoopCodec::Hap | LoopCodec::HapAlpha | LoopCodec::Prores4444 => "mov",
            LoopCodec::H264 | LoopCodec::Hevc => "mp4",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LoopAudio {
    /// No audio analysis at all: the pinned neutral feature vector every frame.
    /// The golden path — bit-exact loop closure by construction.
    #[default]
    None,
    /// Synthetic kick at the effective BPM through the real analyzer (accent-
    /// grade; loop closure is perceptual, not bit-guaranteed).
    Synthetic,
    /// A real audio file via the offline decoder. Explicitly NOT loop-seamless;
    /// preview/one-shot use. Requires the `analyze` build.
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LoopBackground {
    /// Transparent clear + passthrough alpha — requires an `alpha: true`
    /// effect and an alpha-carrying codec.
    #[default]
    Transparent,
    Opaque,
}

/// The best-effort render modes (P2.7): second-class by design, never applied
/// implicitly, and never to phase-locked effects (whose loops already close
/// exactly). CLI flags, not spec fields — a spec describes the loop, not the
/// escape hatch used to approximate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BestEffort {
    /// Exact rendering only (the default; requires `loop: "phase_locked"`).
    None,
    /// `--allow-non-loop`: drive time over the window and hope the effect's
    /// time usage happens to be periodic. No seam guarantee at all.
    TimeWrapped,
    /// `--crossfade-bars T` (+ `--warmup-bars W`): render W discarded warmup
    /// bars, then loop + T extra bars, and crossfade the tail into the head.
    /// Perceptual seaming — the ceiling for stateful effects, by design.
    Crossfade { tail_bars: u32, warmup_bars: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopSpec {
    pub version: u32,
    /// Effect name as scanned (`PfxEffect::name`).
    pub effect: String,
    /// Param overrides by name, preset-style. Absent params keep .pfx defaults.
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
    /// Requested BPM; the effective BPM is derived (see [`Self::snap`]).
    pub bpm: f32,
    /// Loop length in bars (4/4 fixed in v1).
    pub bars: u32,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_resolution")]
    pub resolution: [u32; 2],
    #[serde(default)]
    pub codec: LoopCodec,
    #[serde(default)]
    pub audio: LoopAudio,
    #[serde(default)]
    pub audio_file: Option<String>,
    #[serde(default)]
    pub background: LoopBackground,
}

fn default_fps() -> u32 {
    60
}

fn default_resolution() -> [u32; 2] {
    [1920, 1080]
}

/// The result of BPM ↔ integer-frame snapping (P2.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopTiming {
    pub frames: u32,
    pub effective_bpm: f64,
    pub duration_secs: f64,
}

impl LoopSpec {
    pub fn from_json(json: &str) -> Result<Self, String> {
        let spec: LoopSpec =
            serde_json::from_str(json).map_err(|e| format!("invalid loop spec: {e}"))?;
        if spec.version != LOOP_SPEC_VERSION {
            return Err(format!(
                "loop spec version {} not supported (this build reads version {LOOP_SPEC_VERSION}); \
                 update Fosfora or re-export the spec",
                spec.version
            ));
        }
        Ok(spec)
    }

    /// Snap the requested BPM to the nearest tempo whose loop spans an integer
    /// number of frames. `frames = round(bars·4·60/bpm · fps)`;
    /// `effective_bpm = bars·4·60·fps / frames`. Loop closure becomes exact by
    /// construction; the caller surfaces requested → effective everywhere.
    pub fn snap(&self) -> Result<LoopTiming, String> {
        if !(20.0..=999.0).contains(&self.bpm) {
            return Err(format!("bpm {} out of range (20–999)", self.bpm));
        }
        if self.bars == 0 || self.bars > 64 {
            return Err(format!("bars {} out of range (1–64)", self.bars));
        }
        if !matches!(self.fps, 24 | 25 | 30 | 50 | 60 | 120) {
            return Err(format!(
                "fps {} unsupported (24, 25, 30, 50, 60 or 120)",
                self.fps
            ));
        }
        let beats = self.bars as f64 * 4.0;
        let frames = (beats * 60.0 / self.bpm as f64 * self.fps as f64).round();
        if frames < 2.0 {
            return Err("loop shorter than two frames — raise bars or lower bpm".into());
        }
        let effective_bpm = beats * 60.0 * self.fps as f64 / frames;
        Ok(LoopTiming {
            frames: frames as u32,
            effective_bpm,
            duration_secs: frames / self.fps as f64,
        })
    }

    /// Validate against the scanned effect library. Errors are user-facing and
    /// actionable — this is the wizard's error surface too (Phase 3).
    /// Exact-mode rules; the best-effort modes route through
    /// [`Self::validate_for`]. Unused internally today (the driver is
    /// mode-aware); this is Phase 3's wizard-facing entry.
    #[allow(dead_code)]
    pub fn validate(&self, effects: &[PfxEffect]) -> Result<(), String> {
        self.validate_for(effects, BestEffort::None)
    }

    /// Mode-aware validation (P2.7). Best-effort modes waive the phase-locked
    /// requirement — and only that; alpha/codec/backdrop rules always hold.
    pub fn validate_for(&self, effects: &[PfxEffect], mode: BestEffort) -> Result<(), String> {
        let effect = effects
            .iter()
            .find(|e| e.name == self.effect)
            .ok_or_else(|| {
                format!(
                    "effect '{}' not found — is it installed in assets/effects/?",
                    self.effect
                )
            })?;

        // Backdrop-reactive effects (errata #2062): a single-effect loop render
        // has no layers beneath, so `@backdrop` consumers would render nothing.
        let wants_backdrop = effect
            .normalized_passes()
            .iter()
            .any(|p| p.inputs.iter().any(|i| i == "@backdrop"));
        if wants_backdrop {
            return Err(format!(
                "'{}' reacts to the layers beneath it (@backdrop) and renders nothing solo — \
                 it cannot be exported as a single-effect loop",
                self.effect
            ));
        }

        if self.background == LoopBackground::Transparent {
            if !effect.alpha {
                return Err(format!(
                    "background 'transparent' needs an alpha-capable effect, but '{}' is not \
                     tagged alpha: true — use background 'opaque'",
                    self.effect
                ));
            }
            if !self.codec.carries_alpha() {
                return Err(format!(
                    "background 'transparent' needs an alpha-carrying codec (hap_alpha or \
                     prores4444), not {:?}",
                    self.codec
                ));
            }
        }
        if self.codec.carries_alpha() && !effect.alpha {
            return Err(format!(
                "codec {:?} carries alpha but '{}' is not tagged alpha: true — the alpha \
                 channel would be fully opaque; use hap/h264/hevc, or an overlay effect",
                self.codec, self.effect
            ));
        }

        match mode {
            BestEffort::None => {
                if effect.loop_mode != LoopMode::PhaseLocked {
                    return Err(format!(
                        "'{}' is not loop: \"phase_locked\" — its output is not guaranteed to \
                         close. Render anyway with --allow-non-loop (time-wrapped) or \
                         --crossfade-bars N",
                        self.effect
                    ));
                }
            }
            BestEffort::TimeWrapped | BestEffort::Crossfade { .. } => {
                if effect.loop_mode == LoopMode::PhaseLocked {
                    return Err(format!(
                        "'{}' is phase-locked — its loops already close exactly; drop the \
                         best-effort flag",
                        self.effect
                    ));
                }
            }
        }

        if self.audio == LoopAudio::File && self.audio_file.is_none() {
            return Err("audio 'file' needs audio_file".into());
        }

        let [w, h] = self.resolution;
        if !(16..=7680).contains(&w) || !(16..=4320).contains(&h) {
            return Err(format!(
                "resolution {w}x{h} out of range (16..7680 x 16..4320)"
            ));
        }
        for name in self.params.keys() {
            if !effect.inputs.iter().any(|p| p.name() == name) {
                return Err(format!(
                    "param '{name}' does not exist on '{}'",
                    self.effect
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(bpm: f32, bars: u32, fps: u32) -> LoopSpec {
        LoopSpec {
            version: 1,
            effect: "Tessera".into(),
            params: BTreeMap::new(),
            bpm,
            bars,
            fps,
            resolution: [1920, 1080],
            codec: LoopCodec::HapAlpha,
            audio: LoopAudio::None,
            audio_file: None,
            background: LoopBackground::Transparent,
        }
    }

    #[test]
    fn round_trips() {
        let s = spec(174.0, 8, 60);
        let json = serde_json::to_string_pretty(&s).unwrap();
        let back = LoopSpec::from_json(&json).unwrap();
        assert_eq!(back.effect, "Tessera");
        assert_eq!(back.bars, 8);
        assert_eq!(back.codec, LoopCodec::HapAlpha);
    }

    #[test]
    fn minimal_json_defaults() {
        let s =
            LoopSpec::from_json(r#"{"version":1,"effect":"Bezel","bpm":120.0,"bars":4}"#).unwrap();
        assert_eq!(s.fps, 60);
        assert_eq!(s.resolution, [1920, 1080]);
        assert_eq!(s.codec, LoopCodec::HapAlpha);
        assert_eq!(s.audio, LoopAudio::None);
        assert_eq!(s.background, LoopBackground::Transparent);
    }

    #[test]
    fn unknown_version_rejected() {
        let err = LoopSpec::from_json(r#"{"version":9,"effect":"Bezel","bpm":120.0,"bars":4}"#)
            .unwrap_err();
        assert!(err.contains("version 9"), "{err}");
    }

    /// 120 BPM at 30/60/120 fps snaps losslessly — why the market clusters there.
    #[test]
    fn lossless_snap_at_120() {
        for fps in [30, 60, 120] {
            let t = spec(120.0, 8, fps).snap().unwrap();
            assert_eq!(t.frames, 16 * fps);
            assert!((t.effective_bpm - 120.0).abs() < 1e-9);
        }
    }

    /// The handoff's CLI example: 174 BPM, 8 bars, 60 fps.
    #[test]
    fn snap_derives_effective_bpm() {
        let t = spec(174.0, 8, 60).snap().unwrap();
        // 8·4·60/174·60 = 662.07 → 662 frames.
        assert_eq!(t.frames, 662);
        assert!(
            (t.effective_bpm - 174.018).abs() < 1e-2,
            "{}",
            t.effective_bpm
        );
    }

    /// Sweep: effective is always within the theoretical half-frame bound and
    /// the frame count re-derives the effective BPM exactly.
    #[test]
    fn snap_bound_over_bpm_sweep() {
        for bpm10 in 600..=2000u32 {
            let bpm = bpm10 as f32 / 10.0;
            for fps in [30u32, 60, 120] {
                let t = spec(bpm, 4, fps).snap().unwrap();
                let beats = 16.0f64;
                // Half-frame bound on duration → BPM bound.
                let exact_frames = beats * 60.0 / bpm as f64 * fps as f64;
                assert!((t.frames as f64 - exact_frames).abs() <= 0.5 + 1e-9);
                let rederived = beats * 60.0 * fps as f64 / t.frames as f64;
                assert!((rederived - t.effective_bpm).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn degenerate_specs_rejected() {
        assert!(spec(0.0, 4, 60).snap().is_err());
        assert!(spec(120.0, 0, 60).snap().is_err());
        assert!(spec(120.0, 4, 61).snap().is_err());
    }
}
