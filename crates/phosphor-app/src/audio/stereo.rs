//! A13 (#1464): stereo-field analysis — pan, mid/side width, and L/R correlation.
//!
//! The capture ring carries interleaved `L,R,L,R…` (see [`super::capture`]); the analysis thread
//! feeds each hop's stereo frames here. Metrics integrate over a rolling [`WINDOW`]-frame window
//! (~46 ms at 44.1 kHz) so they track the stereo *field* rather than instantaneous samples.
//!
//! All three outputs are producer-normalized to 0..1 (schema policy `Passthrough`, so they survive
//! `normalize`/`smooth` unrescaled): the bipolar pan and correlation are remapped `0.5 + 0.5*x`;
//! width is a mid/side energy ratio already in 0..1.

/// Rolling window length in stereo frames. 2048 @ 44.1 kHz ≈ 46 ms.
const WINDOW: usize = 2048;

/// The three stereo-field features, each already mapped to 0..1 for the shader ABI.
#[derive(Debug, Clone, Copy)]
pub struct StereoFeatures {
    /// Stereo balance. 0.5 = centered, <0.5 = left-heavy, >0.5 = right-heavy.
    pub pan: f32,
    /// Mid/side width: `Es/(Em+Es)`. 0 = mono, →1 = fully decorrelated / anti-phase.
    pub stereo_width: f32,
    /// L/R correlation. 0.5 = decorrelated, 1 = mono/in-phase, 0 = anti-phase.
    pub stereo_corr: f32,
}

impl StereoFeatures {
    /// Neutral field, emitted on silence where the energy denominators are undefined.
    pub const NEUTRAL: Self = Self {
        pan: 0.5,
        stereo_width: 0.0,
        stereo_corr: 0.5,
    };
}

/// Integrates the stereo field over the last [`WINDOW`] frames.
pub struct StereoAnalyzer {
    left: Box<[f32]>,
    right: Box<[f32]>,
    pos: usize,
    filled: usize,
}

impl StereoAnalyzer {
    pub fn new() -> Self {
        Self {
            left: vec![0.0; WINDOW].into_boxed_slice(),
            right: vec![0.0; WINDOW].into_boxed_slice(),
            pos: 0,
            filled: 0,
        }
    }

    /// Push one hop of interleaved `L,R,L,R…` frames and return the field over the current window.
    ///
    /// A trailing odd sample (never produced by the even-length capture ring) is ignored. Before the
    /// window has filled, the metrics are computed over just the frames seen so far.
    pub fn process(&mut self, interleaved: &[f32]) -> StereoFeatures {
        for frame in interleaved.chunks_exact(2) {
            self.left[self.pos] = frame[0];
            self.right[self.pos] = frame[1];
            self.pos = (self.pos + 1) % WINDOW;
            if self.filled < WINDOW {
                self.filled += 1;
            }
        }

        let n = self.filled;
        if n == 0 {
            return StereoFeatures::NEUTRAL;
        }

        // f64 accumulators: the window is short but energies span a wide dynamic range.
        let (mut sum_l2, mut sum_r2, mut sum_lr) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..n {
            let l = self.left[i] as f64;
            let r = self.right[i] as f64;
            sum_l2 += l * l;
            sum_r2 += r * r;
            sum_lr += l * r;
        }

        // Gate on the total *stereo* energy, not the mono mix: a fully anti-phase signal cancels to
        // mono silence yet carries full L/R energy and is maximally wide — gating it on mono loudness
        // would suppress exactly the anti-phase field this detects. Below the floor is the noise
        // floor / digital silence, where pan/width/corr are undefined.
        const ENERGY_FLOOR: f64 = 1e-6; // mean square per channel-sample ≈ -60 dBFS
        if (sum_l2 + sum_r2) / (2.0 * n as f64) < ENERGY_FLOOR {
            return StereoFeatures::NEUTRAL;
        }

        const EPS: f64 = 1e-12;
        // Pan from channel energies: (Er-El)/(Er+El) ∈ -1..1, remapped to 0..1.
        let pan_b = (sum_r2 - sum_l2) / (sum_r2 + sum_l2 + EPS);
        let pan = 0.5 + 0.5 * pan_b;

        // Mid/side energies from the three sums: Em+Es = (ΣL²+ΣR²)/2, Es = (ΣL²+ΣR²-2ΣLR)/4.
        let es = (sum_l2 + sum_r2 - 2.0 * sum_lr) * 0.25;
        let em_plus_es = (sum_l2 + sum_r2) * 0.5;
        let stereo_width = (es / (em_plus_es + EPS)).clamp(0.0, 1.0);

        // Pearson correlation (audio ≈ zero-mean): ΣLR / √(ΣL²·ΣR²) ∈ -1..1, remapped to 0..1.
        let corr = (sum_lr / (sum_l2.sqrt() * sum_r2.sqrt() + EPS)).clamp(-1.0, 1.0);
        let stereo_corr = 0.5 + 0.5 * corr;

        StereoFeatures {
            pan: pan as f32,
            stereo_width: stereo_width as f32,
            stereo_corr: stereo_corr as f32,
        }
    }
}

impl Default for StereoAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn interleave(l: &[f32], r: &[f32]) -> Vec<f32> {
        l.iter().zip(r).flat_map(|(&a, &b)| [a, b]).collect()
    }

    fn sine(freq: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (TAU * freq * i as f32 / 44_100.0).sin())
            .collect()
    }

    /// Feed exactly one full window in a single hop.
    fn run(l: &[f32], r: &[f32]) -> StereoFeatures {
        StereoAnalyzer::new().process(&interleave(l, r))
    }

    #[test]
    fn centered_mono_is_center_pan_zero_width_full_corr() {
        let s = sine(440.0, WINDOW);
        let f = run(&s, &s);
        assert!((f.pan - 0.5).abs() < 0.02, "pan {}", f.pan);
        assert!(f.stereo_width < 0.02, "width {}", f.stereo_width);
        assert!(f.stereo_corr > 0.98, "corr {}", f.stereo_corr);
    }

    #[test]
    fn hard_left_pans_to_zero() {
        let s = sine(440.0, WINDOW);
        let z = vec![0.0; WINDOW];
        assert!(run(&s, &z).pan < 0.02);
    }

    #[test]
    fn hard_right_pans_to_one() {
        let s = sine(440.0, WINDOW);
        let z = vec![0.0; WINDOW];
        assert!(run(&z, &s).pan > 0.98);
    }

    #[test]
    fn anti_phase_is_decorrelated_and_wide() {
        let s = sine(440.0, WINDOW);
        let neg: Vec<f32> = s.iter().map(|x| -x).collect();
        let f = run(&s, &neg);
        assert!(f.stereo_corr < 0.02, "corr {}", f.stereo_corr); // r = -1 → 0
        assert!(f.stereo_width > 0.98, "width {}", f.stereo_width);
        assert!((f.pan - 0.5).abs() < 0.02, "pan {}", f.pan); // equal power both sides
    }

    #[test]
    fn independent_channels_are_half_wide_half_corr() {
        // Two unrelated frequencies are ≈ uncorrelated over the window.
        let f = run(&sine(440.0, WINDOW), &sine(557.0, WINDOW));
        assert!(
            (f.stereo_width - 0.5).abs() < 0.12,
            "width {}",
            f.stereo_width
        );
        assert!((f.stereo_corr - 0.5).abs() < 0.12, "corr {}", f.stereo_corr);
    }

    #[test]
    fn near_silence_is_neutral() {
        // Full window of noise-floor content (≈ -80 dBFS) must gate to neutral, not chase noise.
        let q: Vec<f32> = sine(440.0, WINDOW).iter().map(|x| x * 1e-4).collect();
        let f = run(&q, &q);
        assert_eq!(f.pan, 0.5);
        assert_eq!(f.stereo_width, 0.0);
        assert_eq!(f.stereo_corr, 0.5);
    }

    #[test]
    fn empty_input_is_neutral() {
        let f = StereoAnalyzer::new().process(&[]);
        assert_eq!(f.pan, 0.5);
        assert_eq!(f.stereo_width, 0.0);
        assert_eq!(f.stereo_corr, 0.5);
    }

    #[test]
    fn odd_trailing_sample_ignored() {
        // 3 floats = 1 full L/R frame + 1 dangling; must not panic.
        let f = StereoAnalyzer::new().process(&[0.5, 0.5, 0.9]);
        assert!((f.pan - 0.5).abs() < 1e-6);
    }
}
