//! The leaf names of the `postfx.*`, `particle.*`, `uniform.*` and
//! `scene.transport.*` binding targets, as enums.
//!
//! These were bare `String`s matched on every frame, with a `_ => {}` arm that
//! swallowed anything misspelled: `uniform.kik` loaded, showed in the matrix and
//! drove nothing. Parsing into an enum at load moves that check to the one place
//! a name enters, so an unrecognised leaf becomes `BindingTarget::Unknown` (kept
//! verbatim, warned about at load) and the per-frame dispatch is an exhaustive
//! match with no fallthrough. Each list is the single source of truth for its
//! wire names, the catalog the offline validator reads, and the UI picker.

/// One enum per target family: variants, their wire names, `ALL`, `as_str` and
/// `parse`, from a single list so the three cannot disagree.
macro_rules! leaf_enum {
    (
        $(#[$meta:meta])*
        $name:ident { $($variant:ident => $wire:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The name as written in a target string.
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $wire),+
                }
            }

            /// `None` for a name no variant carries.
            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($wire => Some($name::$variant),)+
                    _ => None,
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

leaf_enum! {
    /// `postfx.{field}` — Master's post-processing, which belongs to the preset
    /// rather than to any one layer (#3147).
    PostFxField {
        BloomThreshold => "bloom_threshold",
        BloomIntensity => "bloom_intensity",
        Vignette => "vignette",
        CaIntensity => "ca_intensity",
        GrainIntensity => "grain_intensity",
        GrainRate => "grain_rate",
    }
}

leaf_enum! {
    /// `particle.{field}` — applies to every layer that runs a particle system.
    ParticleField {
        EmitRate => "emit_rate",
        BurstOnBeat => "burst_on_beat",
        Lifetime => "lifetime",
        Speed => "speed",
        Size => "size",
        Drag => "drag",
        Turbulence => "turbulence",
        GravityX => "gravity_x",
        GravityY => "gravity_y",
        VortexStrength => "vortex_strength",
        ObstacleEnabled => "obstacle_enabled",
        ObstacleMode => "obstacle_mode",
        ObstacleThreshold => "obstacle_threshold",
        ObstacleElasticity => "obstacle_elasticity",
    }
}

leaf_enum! {
    /// `uniform.{field}` — direct override of one shader uniform field.
    UniformField {
        SubBass => "sub_bass",
        Bass => "bass",
        LowMid => "low_mid",
        Mid => "mid",
        UpperMid => "upper_mid",
        Presence => "presence",
        Brilliance => "brilliance",
        Rms => "rms",
        Kick => "kick",
        Centroid => "centroid",
        Flux => "flux",
        Flatness => "flatness",
        Rolloff => "rolloff",
        Bandwidth => "bandwidth",
        Zcr => "zcr",
        Onset => "onset",
        Beat => "beat",
        BeatPhase => "beat_phase",
        Bpm => "bpm",
        BeatStrength => "beat_strength",
        DominantChroma => "dominant_chroma",
        // Batched ABI bump #1505 ("v2").
        LoudnessM => "loudness_m",
        LoudnessS => "loudness_s",
        LoudnessTrend => "loudness_trend",
        KeyClass => "key_class",
        KeyIsMinor => "key_is_minor",
        KeyConfidence => "key_confidence",
        Downbeat => "downbeat",
        BarPhase => "bar_phase",
        BeatInBar => "beat_in_bar",
        Pan => "pan",
        StereoWidth => "stereo_width",
        StereoCorr => "stereo_corr",
        // A13b per-band pan (#1801).
        BandPanSubBass => "band_pan_sub_bass",
        BandPanBass => "band_pan_bass",
        BandPanLowMid => "band_pan_low_mid",
        BandPanMid => "band_pan_mid",
        BandPanUpperMid => "band_pan_upper_mid",
        BandPanPresence => "band_pan_presence",
        BandPanBrilliance => "band_pan_brilliance",
        SectionNovelty => "section_novelty",
        Buildup => "buildup",
        Drop => "drop",
        // Batched ABI bump #1629 ("v3").
        PercussiveEnergy => "percussive_energy",
        HarmonicEnergy => "harmonic_energy",
        HarmonicRatio => "harmonic_ratio",
        Pitch => "pitch",
        PitchConfidence => "pitch_confidence",
        Contrast0 => "contrast_0",
        Contrast1 => "contrast_1",
        Contrast2 => "contrast_2",
        Contrast3 => "contrast_3",
        Contrast4 => "contrast_4",
        Contrast5 => "contrast_5",
        ContrastMean => "contrast_mean",
        TimbreFlux => "timbre_flux",
        // Tempo trust (#81).
        TempoConfidence => "tempo_confidence",
        BeatLocked => "beat_locked",
        FeedbackDecay => "feedback_decay",
        Time => "time",
    }
}

leaf_enum! {
    /// `scene.transport.{action}` — edge-triggered timeline control, drained by
    /// the event loop (and the headless renderer) after the bus applies.
    TransportAction {
        Go => "go",
        Prev => "prev",
        Stop => "stop",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every wire name parses back to its own variant, and no two variants
    /// share a name — a duplicate would make one of them unreachable.
    #[test]
    fn every_leaf_round_trips_and_is_unique() {
        fn check<T: Copy + PartialEq + std::fmt::Debug>(
            all: &[T],
            as_str: fn(T) -> &'static str,
            parse: fn(&str) -> Option<T>,
        ) {
            let mut seen = HashSet::new();
            for &v in all {
                let s = as_str(v);
                assert!(seen.insert(s), "duplicate wire name {s}");
                assert!(
                    !s.contains('.'),
                    "{s} would re-parse as another target form"
                );
                assert_eq!(parse(s), Some(v), "{s} did not round-trip");
            }
        }
        check(PostFxField::ALL, PostFxField::as_str, PostFxField::parse);
        check(
            ParticleField::ALL,
            ParticleField::as_str,
            ParticleField::parse,
        );
        check(UniformField::ALL, UniformField::as_str, UniformField::parse);
        check(
            TransportAction::ALL,
            TransportAction::as_str,
            TransportAction::parse,
        );
    }

    #[test]
    fn a_misspelled_leaf_does_not_parse() {
        assert_eq!(UniformField::parse("kik"), None);
        assert_eq!(PostFxField::parse("Vignette"), None);
        assert_eq!(ParticleField::parse(""), None);
        assert_eq!(TransportAction::parse("next"), None);
    }
}
