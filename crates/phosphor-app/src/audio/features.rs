use bytemuck::{Pod, Zeroable};

/// 45 audio features, all normalized to 0.0-1.0 range.
/// Multi-resolution FFT bands + spectral shape + beat detection + MFCC + chroma.
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
}

pub const NUM_FEATURES: usize = 46;

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
        for &v in s.iter() {
            assert_eq!(v, 0.0);
        }
    }

    #[test]
    fn as_slice_len() {
        let f = AudioFeatures::default();
        assert_eq!(f.as_slice().len(), 46);
    }

    #[test]
    fn as_slice_mut_write_through() {
        let mut f = AudioFeatures::default();
        f.as_slice_mut()[0] = 0.42;
        assert!((f.sub_bass - 0.42).abs() < 1e-6);
    }

    #[test]
    fn field_order_first_and_last() {
        let mut f = AudioFeatures::default();
        f.sub_bass = 0.11;
        f.dominant_chroma = 0.99;
        let s = f.as_slice();
        assert!((s[0] - 0.11).abs() < 1e-6);
        assert!((s[45] - 0.99).abs() < 1e-6);
    }

    #[test]
    fn size_is_180_bytes() {
        assert_eq!(std::mem::size_of::<AudioFeatures>(), 184);
    }
}
