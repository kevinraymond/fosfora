//! Single source of truth for the audio feature-vector layout and per-feature
//! processing policies.
//!
//! [`AudioFeatures`](super::features::AudioFeatures) is a `#[repr(C)]` POD that
//! is cast to `[f32; NUM_FEATURES]`. Several stages treat that slice
//! *positionally* — the adaptive normalizer decides which features to pass
//! through, the smoother picks attack/release constants per slot, and the
//! stale-feature decay exempts the tempo estimate. Historically each of those
//! sites hardcoded its own index literals (`15..=19`, `BPM_INDEX = 18`, a
//! hand-ordered 46-entry table), so inserting a feature anywhere shifted the
//! indices out from under them and silently corrupted the wrong channels.
//!
//! [`FEATURES`] is the one ordered table those stages read from. Its order MUST
//! match the field order of `AudioFeatures`; the `schema_matches_struct_layout`
//! test pins the boundary fields so a reorder or a missed row fails the build's
//! test run, and a `const` assertion pins the length.

use super::features::NUM_FEATURES;

/// How the adaptive normalizer treats a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormPolicy {
    /// Auto-level via running min/max (energy-like features).
    Adaptive,
    /// Already normalized (or binary) by its producer — pass through untouched.
    /// Used for the beat-detector-owned fields, which adaptive min/max scaling
    /// would only distort.
    Passthrough,
}

/// How the stale-feature decay treats a feature when the capture device stalls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecayPolicy {
    /// Multiply toward silence by the decay factor (energy levels).
    Scale,
    /// Hold the last value — a tempo estimate, not an energy level.
    Hold,
    /// Force to zero every frame — a 1-frame trigger must never latch high on
    /// a stalled device.
    ForceZero,
}

/// Per-feature asymmetric attack/release EMA constants (seconds).
#[derive(Debug, Clone, Copy)]
pub struct SmoothParams {
    pub attack: f32,
    pub release: f32,
    /// Pass through without smoothing (binary triggers / phase sawtooths).
    pub bypass: bool,
}

impl SmoothParams {
    const fn ar(attack: f32, release: f32) -> Self {
        Self {
            attack,
            release,
            bypass: false,
        }
    }
    const fn bypass() -> Self {
        Self {
            attack: 0.0,
            release: 0.0,
            bypass: true,
        }
    }
}

/// The full policy set for one feature slot. Order MUST match the `#[repr(C)]`
/// field order of [`AudioFeatures`](super::features::AudioFeatures).
#[derive(Debug, Clone, Copy)]
pub struct FeatureDef {
    /// Canonical name (the struct field for scalars; `mfcc.0` / `chroma.0` for
    /// array members). Consumed by the layout-guard test today; reserved for
    /// deriving binding source ids (see `bindings::sources`) in a later pass.
    #[allow(dead_code)]
    pub name: &'static str,
    pub norm: NormPolicy,
    pub smooth: SmoothParams,
    pub decay: DecayPolicy,
}

use DecayPolicy::{ForceZero, Hold, Scale};
use NormPolicy::{Adaptive, Passthrough};

/// The ordered feature table — the single source of truth for positional
/// per-feature policy. Row `i` describes `AudioFeatures::as_slice()[i]`.
pub const FEATURES: [FeatureDef; NUM_FEATURES] = [
    // Frequency bands (7) — multi-resolution FFT
    def("sub_bass", Adaptive, SmoothParams::ar(0.02, 0.15), Scale),
    def("bass", Adaptive, SmoothParams::ar(0.02, 0.15), Scale),
    def("low_mid", Adaptive, SmoothParams::ar(0.01, 0.10), Scale),
    def("mid", Adaptive, SmoothParams::ar(0.01, 0.10), Scale),
    def("upper_mid", Adaptive, SmoothParams::ar(0.005, 0.08), Scale),
    def("presence", Adaptive, SmoothParams::ar(0.005, 0.08), Scale),
    def("brilliance", Adaptive, SmoothParams::ar(0.005, 0.08), Scale),
    // Aggregates (2)
    def("rms", Adaptive, SmoothParams::ar(0.01, 0.12), Scale),
    def("kick", Adaptive, SmoothParams::ar(0.002, 0.06), Scale), // fast attack
    // Spectral shape (6)
    def("centroid", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("flux", Adaptive, SmoothParams::ar(0.005, 0.06), Scale),
    def("flatness", Adaptive, SmoothParams::ar(0.05, 0.20), Scale),
    def("rolloff", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("bandwidth", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("zcr", Adaptive, SmoothParams::ar(0.02, 0.10), Scale),
    // Beat detection (5) — detector-owned: already normalized, so pass through
    // the normalizer. `beat` is a 1-frame trigger; `bpm` is a tempo estimate.
    def("onset", Passthrough, SmoothParams::ar(0.001, 0.05), Scale), // very fast
    def("beat", Passthrough, SmoothParams::bypass(), ForceZero),
    def("beat_phase", Passthrough, SmoothParams::bypass(), Scale),
    def("bpm", Passthrough, SmoothParams::ar(0.5, 1.0), Hold), // very slow
    def(
        "beat_strength",
        Passthrough,
        SmoothParams::ar(0.001, 0.08),
        Scale,
    ),
    // MFCC (13) — timbral content, moderate smoothing
    def("mfcc.0", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.1", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.2", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.3", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.4", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.5", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.6", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.7", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.8", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.9", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.10", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.11", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.12", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    // Chroma (12) — pitch class energies, moderate smoothing
    def("chroma.0", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.1", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.2", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.3", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.4", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.5", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.6", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.7", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.8", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.9", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.10", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.11", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    // Derived
    def(
        "dominant_chroma",
        Adaptive,
        SmoothParams::ar(0.05, 0.2),
        Scale,
    ),
    // ---- Reserved tail (batched ABI bump #1505) — 0.0 until each detector lands ----
    // These rows describe the reserved slots so the positional stages stay aligned.
    // Policies are conservative placeholders (Adaptive/Scale like the timbral block);
    // each detector's follow-up task sets the final trigger/phase/hold policy when it
    // wires real data (CPU-side only — no ABI churn).
    // A10 loudness (#1461)
    def("loudness_m", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("loudness_s", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def(
        "loudness_trend",
        Adaptive,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    // A11 key (#1462) — detector-owned. `key_class`/`key_is_minor` are categorical
    // (a pitch-class index and a 0/1 flag), so they pass through the normalizer (no
    // percentile rescale), bypass the smoother (no EMA blend across key changes), and
    // Hold on silence (no sweep toward C). `key_confidence` is already 0..1: gently
    // smoothed and Scales toward 0 when the signal drops out.
    def("key_class", Passthrough, SmoothParams::bypass(), Hold),
    def("key_is_minor", Passthrough, SmoothParams::bypass(), Hold),
    def(
        "key_confidence",
        Passthrough,
        SmoothParams::ar(0.1, 0.3),
        Scale,
    ),
    // A12 downbeat (#1463)
    def("downbeat", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("bar_phase", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("beat_in_bar", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    // A13 stereo (#1464)
    def("pan", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def(
        "stereo_width",
        Adaptive,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def("stereo_corr", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    // A18 structure (#1469)
    def(
        "section_novelty",
        Adaptive,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def("buildup", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
    def("drop", Adaptive, SmoothParams::ar(0.03, 0.15), Scale),
];

/// Terse constructor so the table above reads as one row per feature.
const fn def(
    name: &'static str,
    norm: NormPolicy,
    smooth: SmoothParams,
    decay: DecayPolicy,
) -> FeatureDef {
    FeatureDef {
        name,
        norm,
        smooth,
        decay,
    }
}

// The table must describe every slot of the feature vector, exactly once.
const _: () = assert!(FEATURES.len() == NUM_FEATURES);

#[cfg(test)]
mod tests {
    use super::super::features::AudioFeatures;
    use super::*;

    /// Pins the schema's positional order to the struct's `#[repr(C)]` layout:
    /// writing each slot's index into the flat slice must read back through the
    /// matching named field, and the schema must name those same indices. If a
    /// field is inserted/reordered without updating [`FEATURES`] in lockstep,
    /// this fails.
    #[test]
    fn schema_matches_struct_layout() {
        let mut f = AudioFeatures::default();
        for (i, v) in f.as_slice_mut().iter_mut().enumerate() {
            *v = i as f32;
        }
        assert_eq!(f.sub_bass, 0.0);
        assert_eq!(f.zcr, 14.0);
        assert_eq!(f.onset, 15.0);
        assert_eq!(f.beat, 16.0);
        assert_eq!(f.beat_phase, 17.0);
        assert_eq!(f.bpm, 18.0);
        assert_eq!(f.beat_strength, 19.0);
        assert_eq!(f.mfcc[0], 20.0);
        assert_eq!(f.mfcc[12], 32.0);
        assert_eq!(f.chroma[0], 33.0);
        assert_eq!(f.chroma[11], 44.0);
        assert_eq!(f.dominant_chroma, 45.0);
        // Reserved tail (#1505): boundary pins for the appended block.
        assert_eq!(f.loudness_m, 46.0);
        assert_eq!(f.downbeat, 52.0);
        assert_eq!(f.bar_phase, 53.0);
        assert_eq!(f.pan, 55.0);
        assert_eq!(f.drop, 60.0);

        assert_eq!(FEATURES[0].name, "sub_bass");
        assert_eq!(FEATURES[14].name, "zcr");
        assert_eq!(FEATURES[18].name, "bpm");
        assert_eq!(FEATURES[20].name, "mfcc.0");
        assert_eq!(FEATURES[33].name, "chroma.0");
        assert_eq!(FEATURES[45].name, "dominant_chroma");
        assert_eq!(FEATURES[46].name, "loudness_m");
        assert_eq!(FEATURES[52].name, "downbeat");
        assert_eq!(FEATURES[53].name, "bar_phase");
        assert_eq!(FEATURES[55].name, "pan");
        assert_eq!(FEATURES[60].name, "drop");
    }

    /// The detector-owned fields are exactly the set the normalizer passes through:
    /// the beat block (onset..beat_strength, 15..=19) plus the categorical key fields
    /// (key_class/key_is_minor/key_confidence, 49..=51).
    #[test]
    fn passthrough_is_detector_owned() {
        for (i, def) in FEATURES.iter().enumerate() {
            let expected = if (15..=19).contains(&i) || (49..=51).contains(&i) {
                Passthrough
            } else {
                Adaptive
            };
            assert_eq!(
                def.norm, expected,
                "norm policy for slot {i} ({})",
                def.name
            );
        }
    }

    /// Decay exemptions: `bpm` and the categorical key fields hold their last value on
    /// silence, only `beat` is forced to zero, everything else scales toward silence.
    #[test]
    fn decay_exemptions() {
        for (i, def) in FEATURES.iter().enumerate() {
            let expected = match def.name {
                "bpm" | "key_class" | "key_is_minor" => Hold,
                "beat" => ForceZero,
                _ => Scale,
            };
            assert_eq!(
                def.decay, expected,
                "decay policy for slot {i} ({})",
                def.name
            );
        }
    }
}
