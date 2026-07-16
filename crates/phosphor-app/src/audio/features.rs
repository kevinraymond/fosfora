use bytemuck::{Pod, Zeroable};

/// 61 audio features, all normalized to 0.0-1.0 range.
/// Multi-resolution FFT bands + spectral shape + beat detection + MFCC + chroma,
/// plus a reserved tail (loudness / key / downbeat / stereo / structure) laid out
/// by the batched shader-ABI bump (#1505). The reserved fields read 0.0 until
/// their detectors land; wiring them now means the DSP fills already-reserved
/// slots with zero further ABI churn.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct AudioFeatures {
    // Frequency bands (7) — multi-resolution FFT
    pub sub_bass: f32,   // 20-60 Hz (kick fundamentals)
    pub bass: f32,       // 60-250 Hz (bass guitar/synth)
    pub low_mid: f32,    // 250-500 Hz (lower vocals/snare body)
    pub mid: f32,        // 500-2000 Hz (vocal/snare presence)
    pub upper_mid: f32,  // 2000-4000 Hz (harmonic presence)
    pub presence: f32,   // 4000-6000 Hz (hi-hat attack)
    pub brilliance: f32, // 6000-20000 Hz (cymbal shimmer)

    // Aggregates (2)
    pub rms: f32,  // Overall amplitude
    pub kick: f32, // Dedicated kick drum detection (30-120Hz spectral flux)

    // Spectral shape (6)
    pub centroid: f32,  // Brightness/timbre
    pub flux: f32,      // Spectral change rate
    pub flatness: f32,  // Noise vs tone (Wiener entropy)
    pub rolloff: f32,   // 85% energy frequency
    pub bandwidth: f32, // Spectral spread
    pub zcr: f32,       // Zero crossing rate

    // Beat detection (5)
    pub onset: f32,         // Onset strength (continuous 0-1, for envelope effects)
    pub beat: f32,          // 1.0 on beat frame, 0.0 otherwise (trigger)
    pub beat_phase: f32,    // 0-1 sawtooth cycling at detected tempo
    pub bpm: f32,           // BPM / 300 (normalized 0-1)
    pub beat_strength: f32, // How strong the detected beat was

    // Mel-frequency cepstral coefficients (13)
    pub mfcc: [f32; 13],

    // Pitch class energies (12): C, C#, D, D#, E, F, F#, G, G#, A, A#, B
    pub chroma: [f32; 12],

    // Derived: dominant pitch class (argmax of chroma), normalized 0-1
    pub dominant_chroma: f32,

    // ---- Reserved tail (batched ABI bump #1505) — 0.0 until each detector lands ----
    // A10 loudness (#1461): perceptual loudness envelope
    pub loudness_m: f32,     // momentary loudness (LUFS-like, normalized)
    pub loudness_s: f32,     // short-term loudness
    pub loudness_trend: f32, // loudness slope/direction (rising vs falling)

    // A11 key (#1462): musical key estimate
    pub key_class: f32,      // detected key root pitch class / 11
    pub key_is_minor: f32,   // 0.0 = major, 1.0 = minor
    pub key_confidence: f32, // key estimate confidence

    // A12 downbeat (#1463): bar-level clock
    pub downbeat: f32,    // 1.0 on bar-start frame (trigger)
    pub bar_phase: f32,   // 0-1 sawtooth over the current bar
    pub beat_in_bar: f32, // beat index within the bar, normalized 0-1

    // A13 stereo (#1464): stereo field
    pub pan: f32,          // stereo balance, -1..1 remapped to 0..1
    pub stereo_width: f32, // mid/side width
    pub stereo_corr: f32,  // L/R correlation, -1..1 remapped to 0..1

    // A18 structure (#1469): song-structure cues
    pub section_novelty: f32, // self-similarity novelty curve
    pub buildup: f32,         // riser/tension estimate
    pub drop: f32,            // drop/impact detection
}

pub const NUM_FEATURES: usize = 61;

impl AudioFeatures {
    pub fn as_slice(&self) -> &[f32; NUM_FEATURES] {
        bytemuck::cast_ref(self)
    }

    pub fn as_slice_mut(&mut self) -> &mut [f32; NUM_FEATURES] {
        bytemuck::cast_mut(self)
    }
}

impl Default for AudioFeatures {
    fn default() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_zeroed() {
        let f = AudioFeatures::default();
        let s = f.as_slice();
        assert_eq!(s.len(), NUM_FEATURES);
        for &v in s {
            assert_eq!(v, 0.0);
        }
    }

    #[test]
    fn as_slice_len() {
        let f = AudioFeatures::default();
        assert_eq!(f.as_slice().len(), 61);
    }

    #[test]
    fn as_slice_mut_write_through() {
        let mut f = AudioFeatures::default();
        f.as_slice_mut()[0] = 0.42;
        assert!((f.sub_bass - 0.42).abs() < 1e-6);
    }

    #[test]
    fn field_order_first_and_last() {
        let f = AudioFeatures {
            sub_bass: 0.11,
            dominant_chroma: 0.55,
            drop: 0.99,
            ..Default::default()
        };
        let s = f.as_slice();
        assert!((s[0] - 0.11).abs() < 1e-6);
        // dominant_chroma keeps its original index (reserved tail appends after it)
        assert!((s[45] - 0.55).abs() < 1e-6);
        // `drop` is the new last slot (index 60)
        assert!((s[60] - 0.99).abs() < 1e-6);
    }

    #[test]
    fn size_is_244_bytes() {
        // 61 f32 features (was 184 bytes / 46 before the #1505 batched ABI bump)
        assert_eq!(std::mem::size_of::<AudioFeatures>(), 244);
    }
}
