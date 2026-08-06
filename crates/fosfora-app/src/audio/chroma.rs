//! CQT-lite constant-Q chroma with tuning compensation (A11 #1462).
//!
//! Replaces the old FFT-bin → pitch-class histogram (which hard-rounded every bin
//! to the nearest of 12 classes) with sparse per-semitone Gaussian kernels over the
//! 4096-pt magnitude spectrum. The 61 semitone energies (MIDI 36–96) are octave-folded
//! to 12 pitch classes, then harmonically summed with 1/h weighting so a note is
//! reinforced by its own upper harmonics — recovering low fundamentals the 4096-pt FFT
//! can't resolve directly (their harmonics live at higher, well-resolved frequencies).
//!
//! A slow tuning estimator tracks the global A-reference offset (±50 cents) from
//! parabola-refined spectral peaks and shifts the kernel centers so a 432 Hz-tuned
//! track no longer smears across pitch classes.

pub const N_CHROMA: usize = 12;

/// Constant-Q kernel note range: MIDI 36 (C2, ~65 Hz) .. MIDI 96 (C7, ~2093 Hz).
const MIDI_LO: i32 = 36;
const MIDI_HI: i32 = 96;
const N_SEMITONES: usize = (MIDI_HI - MIDI_LO + 1) as usize; // 61

/// Gaussian kernel width in **semitones** (log-frequency space). At 0.5 semitones an
/// adjacent semitone sits 2σ away (weight ≈ 0.14) and two semitones away is 4σ (≈ 0),
/// so a note bleeds only lightly into its neighbours — far tighter than a linear-Hz
/// Gaussian, which at these frequencies would span a whole semitone.
const SIGMA_SEMITONES: f32 = 0.5;
/// Kernel support: ±3σ of frequency bins around each semitone centre.
const KERNEL_HALF_WIDTH_SEMITONES: f32 = 3.0 * SIGMA_SEMITONES;

/// Harmonics summed to reinforce each fundamental. Offsets are the pitch-class of the
/// h-th harmonic relative to the fundamental: round(12·log2(h)) mod 12 for h = 1..4
/// → unison, octave, perfect-fifth, double-octave. Weights are 1/h.
const HARM_OFFSET_UP: [i32; 4] = [0, 0, 7, 0];
const HARM_WEIGHT: [f32; 4] = [1.0, 0.5, 1.0 / 3.0, 0.25];

/// Tuning histogram: 1 bin per cent over ±50 cents.
const TUNING_BINS: usize = 100;
/// Per-frame histogram decay ≈ 10 s memory at ~100 analysis frames/s.
const TUNING_DECAY: f32 = 0.999;
/// EMA rate for the smoothed cents offset (slow — tuning is near-constant per track).
const TUNING_EMA: f32 = 0.02;
/// Rebuild kernels once the tuning estimate has drifted this far from the built value…
const KERNEL_REGEN_CENTS: f32 = 2.0;
/// …and no more often than this (~3 s at 100 fps) to avoid per-frame kernel churn.
const KERNEL_REGEN_MIN_FRAMES: u32 = 300;

/// One sparse constant-Q kernel per semitone: (fft_bin, weight) pairs.
type Kernel = Vec<(usize, f32)>;

pub struct CqtChroma {
    num_bins: usize,
    bin_hz: f32,
    kernels: Vec<Kernel>, // N_SEMITONES entries

    // Tuning estimator
    tuning_hist: Vec<f32>, // TUNING_BINS, cents histogram (magnitude-weighted, decaying)
    tuning_cents: f32,     // EMA'd global offset from A440, in cents
    kernel_cents: f32,     // offset the current kernels were built for
    frames_since_regen: u32,
}

impl CqtChroma {
    pub fn new(num_bins: usize, bin_hz: f32) -> Self {
        Self {
            num_bins,
            bin_hz,
            kernels: Self::build_kernels(num_bins, bin_hz, 0.0),
            tuning_hist: vec![0.0; TUNING_BINS],
            tuning_cents: 0.0,
            kernel_cents: 0.0,
            frames_since_regen: 0,
        }
    }

    /// Compute the 12-bin chroma vector for one magnitude frame (L-∞ normalized to 0..1).
    /// Also advances the tuning estimator and lazily rebuilds kernels when tuning drifts.
    pub fn compute(&mut self, mag: &[f32]) -> [f32; N_CHROMA] {
        self.update_tuning(mag);
        self.maybe_regen_kernels();

        // Weighted-magnitude energy per semitone, octave-folded into 12 pitch classes.
        let mut e12 = [0.0f32; N_CHROMA];
        for (s, kernel) in self.kernels.iter().enumerate() {
            let mut e = 0.0f32;
            for &(k, w) in kernel {
                e += mag[k] * w;
            }
            let pc = ((MIDI_LO + s as i32) % 12 + 12) % 12;
            e12[pc as usize] += e;
        }

        // Harmonic reinforcement: gather each fundamental's harmonics with 1/h weight.
        let mut chroma = [0.0f32; N_CHROMA];
        for (p, c) in chroma.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for (h, &off) in HARM_OFFSET_UP.iter().enumerate() {
                let src = (p as i32 + off).rem_euclid(12) as usize;
                acc += HARM_WEIGHT[h] * e12[src];
            }
            *c = acc;
        }

        // L-∞ normalize.
        let max = chroma.iter().cloned().fold(0.0f32, f32::max);
        if max > 1e-10 {
            for c in &mut chroma {
                *c /= max;
            }
        }
        chroma
    }

    /// Build one Gaussian constant-Q kernel per semitone, centred for a given global
    /// tuning offset (cents). Each kernel is unit-sum so a flat spectrum yields equal
    /// semitone energies.
    fn build_kernels(num_bins: usize, bin_hz: f32, cents: f32) -> Vec<Kernel> {
        let ref_hz = 440.0 * 2.0f32.powf(cents / 1200.0);
        let mut kernels = Vec::with_capacity(N_SEMITONES);
        for s in 0..N_SEMITONES {
            let midi = MIDI_LO + s as i32;
            let f_c = ref_hz * 2.0f32.powf((midi as f32 - 69.0) / 12.0);

            // Frequency support at ±KERNEL_HALF_WIDTH_SEMITONES (geometric, i.e. log-space).
            let f_lo = f_c * 2.0f32.powf(-KERNEL_HALF_WIDTH_SEMITONES / 12.0);
            let f_hi = f_c * 2.0f32.powf(KERNEL_HALF_WIDTH_SEMITONES / 12.0);
            let lo = ((f_lo / bin_hz).floor() as i32).max(1) as usize;
            let hi = ((f_hi / bin_hz).ceil() as usize).min(num_bins - 1);

            let mut kernel = Kernel::new();
            let mut wsum = 0.0f32;
            for k in lo..=hi {
                let hz = k as f32 * bin_hz;
                if hz <= 0.0 {
                    continue;
                }
                // Distance in semitones (log-frequency), so bandwidth is constant-Q.
                let d = 12.0 * (hz / f_c).log2() / SIGMA_SEMITONES;
                let w = (-0.5 * d * d).exp();
                if w > 1e-3 {
                    kernel.push((k, w));
                    wsum += w;
                }
            }
            if wsum > 0.0 {
                for (_, w) in &mut kernel {
                    *w /= wsum;
                }
            } else {
                // Degenerate at very low frequencies (σ < bin width): use nearest bin.
                let k = ((f_c / bin_hz).round() as usize).clamp(1, num_bins - 1);
                kernel.push((k, 1.0));
            }
            kernels.push(kernel);
        }
        kernels
    }

    /// Accumulate parabola-refined spectral-peak deviations into the cents histogram and
    /// EMA the mode toward the smoothed tuning offset.
    fn update_tuning(&mut self, mag: &[f32]) {
        for h in &mut self.tuning_hist {
            *h *= TUNING_DECAY;
        }

        // Peaks are sharpest and most reliable in the low-mid range.
        let lo = ((100.0 / self.bin_hz) as usize).max(2);
        let hi = ((2000.0 / self.bin_hz) as usize).min(self.num_bins.saturating_sub(2));
        if hi <= lo {
            return;
        }
        let max_mag = mag[lo..hi].iter().cloned().fold(0.0f32, f32::max);
        if max_mag < 1e-6 {
            return;
        }
        let thresh = max_mag * 0.1;

        for k in lo..hi {
            let b = mag[k];
            if b <= thresh || b <= mag[k - 1] || b < mag[k + 1] {
                continue;
            }
            // Quadratic (QIFFT) peak-vertex refinement: x* = ½(α−γ)/(α−2β+γ).
            let (alpha, gamma) = (mag[k - 1], mag[k + 1]);
            let denom = alpha - 2.0 * b + gamma;
            let p = if denom.abs() > 1e-12 {
                (0.5 * (alpha - gamma) / denom).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            let freq = (k as f32 + p) * self.bin_hz;
            if freq <= 0.0 {
                continue;
            }
            // Fractional-semitone deviation from equal temperament (A4 = MIDI 69).
            let semitone = 12.0 * (freq / 440.0).log2() + 69.0;
            let cents = (semitone - semitone.round()) * 100.0; // (−50, 50]
            let bin = (((cents + 50.0) / 100.0) * TUNING_BINS as f32) as usize;
            self.tuning_hist[bin.min(TUNING_BINS - 1)] += b;
        }

        // Mode of the histogram — only trust it when clearly concentrated.
        let mut mode_bin = 0usize;
        let mut mode_val = 0.0f32;
        let mut total = 0.0f32;
        for (i, &v) in self.tuning_hist.iter().enumerate() {
            total += v;
            if v > mode_val {
                mode_val = v;
                mode_bin = i;
            }
        }
        let mean = total / TUNING_BINS as f32;
        if mode_val > 1e-6 && mode_val > 3.0 * mean {
            let mode_cents = (mode_bin as f32 + 0.5) / TUNING_BINS as f32 * 100.0 - 50.0;
            self.tuning_cents += TUNING_EMA * (mode_cents - self.tuning_cents);
            self.tuning_cents = self.tuning_cents.clamp(-50.0, 50.0);
        }
    }

    fn maybe_regen_kernels(&mut self) {
        self.frames_since_regen = self.frames_since_regen.saturating_add(1);
        if (self.tuning_cents - self.kernel_cents).abs() > KERNEL_REGEN_CENTS
            && self.frames_since_regen > KERNEL_REGEN_MIN_FRAMES
        {
            self.kernels = Self::build_kernels(self.num_bins, self.bin_hz, self.tuning_cents);
            self.kernel_cents = self.tuning_cents;
            self.frames_since_regen = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    const FFT: usize = 4096;

    fn num_bins() -> usize {
        FFT / 2 + 1
    }
    fn bin_hz() -> f32 {
        SR / FFT as f32
    }

    /// Synthesize a magnitude spectrum with a single sharp spectral line at `hz`
    /// (plus a narrow skirt so peak interpolation has neighbours to work with).
    fn sine_mag(hz: f32) -> Vec<f32> {
        let bh = bin_hz();
        let center = hz / bh;
        let mut mag = vec![0.0f32; num_bins()];
        for (k, m) in mag.iter_mut().enumerate() {
            let d = k as f32 - center;
            *m = (-0.5 * (d / 0.8).powi(2)).exp();
        }
        mag
    }

    fn dominant(chroma: &[f32; N_CHROMA]) -> usize {
        let mut idx = 0;
        for i in 1..N_CHROMA {
            if chroma[i] > chroma[idx] {
                idx = i;
            }
        }
        idx
    }

    #[test]
    fn a440_peaks_at_a_with_zero_tuning() {
        let mut cqt = CqtChroma::new(num_bins(), bin_hz());
        let mag = sine_mag(440.0);
        let mut chroma = [0.0f32; N_CHROMA];
        for _ in 0..2000 {
            chroma = cqt.compute(&mag);
        }
        // Pitch class 9 == A (C=0).
        assert_eq!(dominant(&chroma), 9, "chroma = {chroma:?}");
        assert!(
            cqt.tuning_cents.abs() < 3.0,
            "tuning drifted: {} cents",
            cqt.tuning_cents
        );
    }

    #[test]
    fn a432_still_peaks_at_a_and_estimates_flat_tuning() {
        let mut cqt = CqtChroma::new(num_bins(), bin_hz());
        let mag = sine_mag(432.0);
        let mut chroma = [0.0f32; N_CHROMA];
        for _ in 0..3000 {
            chroma = cqt.compute(&mag);
        }
        assert_eq!(dominant(&chroma), 9, "chroma = {chroma:?}");
        // 432 Hz is ~−31.8 cents flat of A440.
        assert!(
            (cqt.tuning_cents - (-31.8)).abs() < 6.0,
            "tuning estimate {} cents, expected ≈ −32",
            cqt.tuning_cents
        );
    }

    #[test]
    fn silence_is_finite_and_untuned() {
        let mut cqt = CqtChroma::new(num_bins(), bin_hz());
        let mag = vec![0.0f32; num_bins()];
        for _ in 0..100 {
            let chroma = cqt.compute(&mag);
            for v in chroma {
                assert!(v.is_finite());
            }
        }
        assert_eq!(cqt.tuning_cents, 0.0);
    }
}
