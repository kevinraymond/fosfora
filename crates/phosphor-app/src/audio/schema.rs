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

/// How the feature normalizer (A2 #1453) treats a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormPolicy {
    /// Auto-level via gated percentile ranging — `(v − P5) / (P95 − P5)` over a
    /// windowed history, gated to 0 on perceptual silence. For energy-like features
    /// (bands, rms, flux) whose absolute scale is unknown and drifts.
    Adaptive,
    /// Already in a known physical 0..1 range (the analyzer maps it there): clamp,
    /// don't adapt, and hold the last value on silence rather than dance to room
    /// noise. For the spectral-shape features (centroid/flatness/rolloff/bandwidth/zcr).
    FixedRange,
    /// Signed, zero-centred features (MFCCs): standardize by a running mean/variance
    /// and map through a tanh to 0..1, so a coefficient's excursions read symmetrically
    /// instead of being min/max-stretched like an energy band.
    ZScore,
    /// Already normalized (or binary) by its producer — pass through untouched. Used
    /// for the detector-owned fields (beat/loudness/key/bar clock/structure) and the
    /// CQT chroma, which any rescaling would only distort.
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

/// How the A8 render-side interpolator (#1459) treats a feature between audio frames.
///
/// Analysis runs at a fixed 86.1 Hz hop while the render loop can poll at 144 Hz, so the
/// render thread blends the two frames bracketing its playhead. Only quantities with a
/// meaningful value *between* two samples may be blended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpPolicy {
    /// Linearly blend the bracketing frames — a continuous quantity.
    Lerp,
    /// Zero-order hold from the older bracketing frame. For a 1-frame trigger, a wrapping
    /// phase (whose wrap would lerp into a backwards sweep through 0.5), or a categorical
    /// index (a value between two pitch classes is not a pitch class).
    Hold,
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
    /// A8 (#1459). Orthogonal to `smooth.bypass`, which cannot stand in for it:
    /// `dominant_chroma` is an argmax index that must not be lerped yet is smoothed, and
    /// `bpm` Holds on decay yet lerps fine. Set via [`def_hold`]; [`def`] defaults to
    /// `Lerp` since 65 of the 74 slots are continuous.
    pub interp: InterpPolicy,
}

use DecayPolicy::{ForceZero, Hold, Scale};
use NormPolicy::{Adaptive, FixedRange, Passthrough, ZScore};

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
    // A3 (#1454): kick is now single-normalized by the detector (log-flux / long-term P95
    // in `FftAnalyzer::kick_envelope`), so it Passes through here — the old Adaptive slot
    // was a second AGC stacked on the analyzer's peak-hold. Fast attack retained.
    def("kick", Passthrough, SmoothParams::ar(0.002, 0.06), Scale),
    // Spectral shape (6) — A4 (#1455) puts each in a known physical 0..1 range, so
    // FixedRange clamps + holds on silence rather than adaptively rescaling. `flux` is
    // the exception: a level-invariant change rate with no fixed ceiling, so it stays
    // Adaptive (percentile-ranged like an energy band).
    def("centroid", FixedRange, SmoothParams::ar(0.03, 0.15), Scale),
    def("flux", Adaptive, SmoothParams::ar(0.005, 0.06), Scale),
    def("flatness", FixedRange, SmoothParams::ar(0.05, 0.20), Scale),
    def("rolloff", FixedRange, SmoothParams::ar(0.03, 0.15), Scale),
    def("bandwidth", FixedRange, SmoothParams::ar(0.03, 0.15), Scale),
    def("zcr", FixedRange, SmoothParams::ar(0.02, 0.10), Scale),
    // Beat detection (5) — detector-owned: already normalized, so pass through
    // the normalizer. `beat` is a 1-frame trigger; `bpm` is a tempo estimate.
    def("onset", Passthrough, SmoothParams::ar(0.001, 0.05), Scale), // very fast
    def_hold("beat", Passthrough, SmoothParams::bypass(), ForceZero),
    // A8 (#1459): a wrapping sawtooth — never lerp across the wrap. The render thread
    // replaces this slot outright with a locally-advanced phase anyway.
    def_hold("beat_phase", Passthrough, SmoothParams::bypass(), Scale),
    def("bpm", Passthrough, SmoothParams::ar(0.5, 1.0), Hold), // very slow
    def(
        "beat_strength",
        Passthrough,
        SmoothParams::ar(0.001, 0.08),
        Scale,
    ),
    // MFCC (13) — signed timbral coefficients: ZScore standardizes each so its
    // excursions read symmetrically instead of being min/max-stretched.
    def("mfcc.0", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.1", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.2", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.3", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.4", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.5", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.6", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.7", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.8", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.9", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.10", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.11", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    def("mfcc.12", ZScore, SmoothParams::ar(0.03, 0.15), Scale),
    // Chroma (12) — pitch class energies, already max-normed by the CQT (A11), so
    // Passthrough; adaptive rescaling would flatten the inter-class contrast.
    def("chroma.0", Passthrough, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.1", Passthrough, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.2", Passthrough, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.3", Passthrough, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.4", Passthrough, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.5", Passthrough, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.6", Passthrough, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.7", Passthrough, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.8", Passthrough, SmoothParams::ar(0.03, 0.15), Scale),
    def("chroma.9", Passthrough, SmoothParams::ar(0.03, 0.15), Scale),
    def(
        "chroma.10",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "chroma.11",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    // Derived: a pitch-class index / 11, not an energy level — Passthrough. A8 (#1459)
    // Holds it: lerping an argmax index reads out a pitch class that was never detected
    // (C → D# would sweep through D). Smoothed but not interpolated — the one row where
    // `smooth.bypass` and `interp` genuinely disagree.
    def_hold(
        "dominant_chroma",
        Passthrough,
        SmoothParams::ar(0.05, 0.2),
        Scale,
    ),
    // ---- Reserved tail (batched ABI bump #1505) — 0.0 until each detector lands ----
    // These rows describe the reserved slots so the positional stages stay aligned.
    // Policies are conservative placeholders (Adaptive/Scale like the timbral block);
    // each detector's follow-up task sets the final trigger/phase/hold policy when it
    // wires real data (CPU-side only — no ABI churn).
    // A10 loudness (#1461) — detector-owned: the meter emits absolute LUFS mapped to
    // 0..1, so pass through the normalizer (adaptive percentile rescaling would destroy
    // the perceptual/device-independent scale A18 depends on for its loudness-jump test).
    // Gently smoothed; Scale toward 0 on silence.
    def(
        "loudness_m",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "loudness_s",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "loudness_trend",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    // A11 key (#1462) — detector-owned. `key_class`/`key_is_minor` are categorical
    // (a pitch-class index and a 0/1 flag), so they pass through the normalizer (no
    // percentile rescale), bypass the smoother (no EMA blend across key changes), and
    // Hold on silence (no sweep toward C). `key_confidence` is already 0..1: gently
    // smoothed and Scales toward 0 when the signal drops out.
    def_hold("key_class", Passthrough, SmoothParams::bypass(), Hold),
    def_hold("key_is_minor", Passthrough, SmoothParams::bypass(), Hold),
    def(
        "key_confidence",
        Passthrough,
        SmoothParams::ar(0.1, 0.3),
        Scale,
    ),
    // A12 downbeat (#1463) — detector-owned, mirroring the beat block. `downbeat` is a
    // 1-frame trigger (like `beat`: pass through, no EMA, ForceZero on silence). `bar_phase`
    // is a 0-1 sawtooth (like `beat_phase`: pass through, no EMA, Scale). `beat_in_bar` is a
    // normalized index, not an energy level — pass through so the normalizer doesn't
    // percentile-rescale it.
    // A8 (#1459) Holds all three: a trigger, a wrapping sawtooth, and a stepwise index.
    // `bar_phase` Holds for the same reasons as `beat_phase`: lerping its wrap would sweep
    // backwards through 0.5, and since A8b (#1554) the render thread replaces the slot
    // outright with a locally-advanced phase anyway — for which this Held value is the PLL's
    // input, so it must not be a lerped wrap either.
    def_hold("downbeat", Passthrough, SmoothParams::bypass(), ForceZero),
    def_hold("bar_phase", Passthrough, SmoothParams::bypass(), Scale),
    def_hold("beat_in_bar", Passthrough, SmoothParams::bypass(), Scale),
    // A13 stereo (#1464) — detector-owned. `pan`/`stereo_corr` are bipolar values the
    // StereoAnalyzer remaps to 0..1, and `stereo_width` is a mid/side ratio already in 0..1, so all
    // three pass through the normalizer (percentile ranging would distort a producer-normalized
    // value). Smoothed to steady the field; Scale toward 0 on a stalled device.
    def("pan", Passthrough, SmoothParams::ar(0.03, 0.15), Scale),
    def(
        "stereo_width",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "stereo_corr",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    // A18 structure (#1469) — detector-owned. `section_novelty` is self-normalized 0..1 and
    // `buildup` is a logistic 0..1, so both pass through the normalizer; each is smoothed to
    // iron out the ~10 Hz decimation stairs, and Scales toward 0 on silence. `drop` is a
    // 1-frame trigger like `beat`/`downbeat`: pass through, no EMA, ForceZero on silence.
    def(
        "section_novelty",
        Passthrough,
        SmoothParams::ar(0.05, 0.2),
        Scale,
    ),
    def("buildup", Passthrough, SmoothParams::ar(0.08, 0.25), Scale),
    def_hold("drop", Passthrough, SmoothParams::bypass(), ForceZero),
    // ---- Reserved tail (batched ABI bump #1629, "v3") — 0.0 until each detector lands ----
    // Conservative placeholders (Passthrough/Scale/gently-smoothed) like the v2 tail above;
    // each DSP fill sets the final policy when it wires real data (CPU-side only — no ABI churn).
    // A14 HPSS (#1465): the two energies are raw masked-power levels — Adaptive, so the gated
    // percentile normalizer ranges them like the frequency bands (quiet music still fills 0..1).
    // `harmonic_ratio` is already a level-invariant 0..1 balance the analyzer neutral-gates on
    // silence, so it passes through. All three Scale toward 0 on a stalled device.
    def(
        "percussive_energy",
        Adaptive,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "harmonic_energy",
        Adaptive,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "harmonic_ratio",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    // A15 pitch (#1466) — detector-owned. `pitch` is a producer-normalized 0..1 log-frequency
    // (Passthrough; percentile ranging would distort it) that the YIN analyzer holds through
    // unvoiced gaps, so on a stalled device it **Holds** its last f0 (like `bpm`/`key_class`) rather
    // than sweeping toward the lowest note. `pitch_confidence` Scales toward 0. Both lightly smoothed.
    def("pitch", Passthrough, SmoothParams::ar(0.03, 0.15), Hold),
    def(
        "pitch_confidence",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    // A16 spectral contrast (#1467).
    def(
        "contrast_0",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "contrast_1",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "contrast_2",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "contrast_3",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "contrast_4",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "contrast_5",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "contrast_mean",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
    def(
        "timbre_flux",
        Passthrough,
        SmoothParams::ar(0.03, 0.15),
        Scale,
    ),
];

/// Terse constructor so the table above reads as one row per feature. Interpolates
/// (A8 #1459) — the common case; use [`def_hold`] for the triggers, wrapping phases and
/// categorical indices that must not be blended.
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
        interp: InterpPolicy::Lerp,
    }
}

/// As [`def`], but the A8 interpolator zero-order-holds this slot instead of blending it.
const fn def_hold(
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
        interp: InterpPolicy::Hold,
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
        // Reserved v2 tail (#1505): boundary pins for the appended block.
        assert_eq!(f.loudness_m, 46.0);
        assert_eq!(f.downbeat, 52.0);
        assert_eq!(f.bar_phase, 53.0);
        assert_eq!(f.pan, 55.0);
        assert_eq!(f.drop, 60.0);
        // Reserved v3 tail (#1629): boundary pins for the appended block.
        assert_eq!(f.percussive_energy, 61.0);
        assert_eq!(f.pitch, 64.0);
        assert_eq!(f.contrast_0, 66.0);
        assert_eq!(f.timbre_flux, 73.0);

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
        assert_eq!(FEATURES[61].name, "percussive_energy");
        assert_eq!(FEATURES[64].name, "pitch");
        assert_eq!(FEATURES[66].name, "contrast_0");
        assert_eq!(FEATURES[73].name, "timbre_flux");
    }

    /// Pins the A2 (#1453) per-feature normalization policy for every slot:
    /// - **FixedRange** (clamp + hold on silence): the spectral-shape features A4 (#1455)
    ///   puts in a known physical 0..1 range — centroid (9), flatness (11), rolloff (12),
    ///   bandwidth (13), zcr (14).
    /// - **ZScore** (standardized): the 13 signed MFCC coefficients (20..=32).
    /// - **Passthrough** (producer-owned): kick (8, single-normalized by the A3 #1454
    ///   detector); the beat block (15..=19); chroma + dominant_chroma + loudness + key +
    ///   bar clock, which happen to be contiguous (33..=54); the A13 stereo block (55..=57,
    ///   producer-remapped to 0..1); the A18 structure block (58..=60); and most of the v3 (#1629)
    ///   reserved tail — `harmonic_ratio` (63, a level-invariant balance), A15 pitch (64..=65),
    ///   A16 contrast (66..=73) — all producer-scaled to 0..1.
    /// - **Adaptive** (gated percentile ranging): the energy-like features — the 7 bands, rms (7),
    ///   flux (10), and the A14 HPSS energies `percussive_energy` / `harmonic_energy` (61, 62),
    ///   which are raw masked-power levels of unknown absolute scale.
    #[test]
    fn norm_policy_assignment() {
        for (i, def) in FEATURES.iter().enumerate() {
            let expected = match i {
                9 | 11 | 12 | 13 | 14 => FixedRange,
                20..=32 => ZScore,
                8 | 15..=19 | 33..=60 | 63..=73 => Passthrough,
                _ => Adaptive,
            };
            assert_eq!(
                def.norm, expected,
                "norm policy for slot {i} ({})",
                def.name
            );
        }
    }

    /// Decay exemptions: `bpm`, the categorical key fields, and the A15 `pitch` estimate hold
    /// their last value on a stalled device (no sweep toward the lowest note), the `beat` and
    /// `downbeat` triggers are forced to zero, everything else scales toward silence.
    #[test]
    fn decay_exemptions() {
        for (i, def) in FEATURES.iter().enumerate() {
            let expected = match def.name {
                "bpm" | "key_class" | "key_is_minor" | "pitch" => Hold,
                "beat" | "downbeat" | "drop" => ForceZero,
                _ => Scale,
            };
            assert_eq!(
                def.decay, expected,
                "decay policy for slot {i} ({})",
                def.name
            );
        }
    }

    /// A8 (#1459) interp exemptions: 9 of 74 slots must never be blended between audio
    /// frames — the 1-frame triggers, the two wrapping sawtooths, and the categorical
    /// indices. Everything else is a continuous quantity and lerps.
    ///
    /// Note this cannot be derived from `smooth.bypass`, and the two sets deliberately
    /// disagree in both directions: `dominant_chroma` is smoothed but must Hold (an argmax
    /// index), while `beat_strength` bypasses nothing and lerps fine.
    ///
    /// Same guard weakness as `decay_exemptions`: a newly appended row defaults to `Lerp`
    /// in both the table and this test's `_` arm, so it passes silently. Adding a feature
    /// means deciding its interp policy by hand.
    #[test]
    fn interp_policy_assignment() {
        for (i, def) in FEATURES.iter().enumerate() {
            let expected = match def.name {
                // 1-frame triggers (moot in practice — the render-side counter latch
                // overwrites all three — but correct on its own terms).
                "beat" | "downbeat" | "drop" => InterpPolicy::Hold,
                // Wrapping 0-1 sawtooths: lerping 0.98 → 0.02 sweeps backwards through 0.5.
                "beat_phase" | "bar_phase" => InterpPolicy::Hold,
                // Categorical: an index between two classes is not a class.
                "dominant_chroma" | "key_class" | "key_is_minor" | "beat_in_bar" => {
                    InterpPolicy::Hold
                }
                _ => InterpPolicy::Lerp,
            };
            assert_eq!(
                def.interp, expected,
                "interp policy for slot {i} ({})",
                def.name
            );
        }
    }
}
