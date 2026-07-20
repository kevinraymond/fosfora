//! A14 (#1465): harmonic/percussive source separation (HPSS) energies.
//!
//! Fitzgerald-2010 median-filter HPSS on the 1024-pt medium magnitude spectrum (513 bins),
//! run causally per analysis hop so it adds zero latency:
//! - **Harmonic** estimate `H[f]` = trailing **time**-median over the last [`TIME_FRAMES`]
//!   frames at each bin — sustained tones survive a median across time, transients don't.
//! - **Percussive** estimate `P[f]` = **frequency**-median over ±[`FREQ_RADIUS`] bins of the
//!   current frame — broadband transients survive a median across frequency, tonal peaks don't.
//!
//! Fitzgerald soft (Wiener) masks split each bin's power between the two: `Mp = P²/(P²+H²)`,
//! `Mh = H²/(P²+H²)` (so `Mp + Mh = 1`). Summing the masked power over the spectrum gives the
//! percussive / harmonic energies; their normalized balance gives `harmonic_ratio`.
//!
//! The two energies are **dB-mapped** (RMS of the masked power → 20·log10 → −120..0 dB → 0..1)
//! before the caller hands them to `normalize()`, where the A2 adaptive normalizer
//! percentile-ranges and silence-gates them like the frequency bands (schema policy `Adaptive`).
//! The dB map is load-bearing, not cosmetic: the raw masked powers at real listening levels sit
//! around 1e-7, below the normalizer's absolute span-collapse epsilon (1e-6), which hard-zeroed
//! `percussive_energy` on live music while full-scale synthetic test signals sailed through
//! (live finding — the Cleave starburst never fired on real DnB). In dB space a ±10 dB musical
//! swing is the same span at any volume, so the epsilon can never swallow it. The window is
//! wider than the bands' −80 dB because the mean over all 513 bins dilutes single-source content
//! by up to ~27 dB relative to a band-limited RMS. `harmonic_ratio` is computed from the raw
//! powers (level-invariant 0..1 balance), producer-owned (`Passthrough`) and neutral-gated here.

/// Time-median window in frames (~0.2 s at the 512-sample hop). Odd for a defined median.
const TIME_FRAMES: usize = 17;
/// Frequency-median half-width in bins (±8 → a 17-bin window on the 1024-pt spectrum).
const FREQ_RADIUS: usize = 8;

/// The three HPSS features.
#[derive(Debug, Clone, Copy)]
pub struct HpssFeatures {
    /// Percussive (transient) energy — dB-mapped 0..1, adaptively normalized downstream.
    pub percussive_energy: f32,
    /// Harmonic (sustained) energy — dB-mapped 0..1, adaptively normalized downstream.
    pub harmonic_energy: f32,
    /// Harmonic vs percussive balance `E_h/(E_h+E_p)`, 0..1 (0.5 = balanced), level-invariant.
    pub harmonic_ratio: f32,
}

impl HpssFeatures {
    /// Neutral field, emitted on silence / before any signal, where the split is undefined.
    pub const NEUTRAL: Self = Self {
        percussive_energy: 0.0,
        harmonic_energy: 0.0,
        harmonic_ratio: 0.5,
    };
}

/// Rolling median-filter HPSS over the medium magnitude spectrum.
pub struct HpssAnalyzer {
    /// Bin count of the spectrum; the ring is sized to it on the first frame.
    bins: usize,
    /// `TIME_FRAMES × bins` flat ring of recent magnitude frames.
    ring: Vec<f32>,
    /// Next slot to overwrite (oldest frame).
    pos: usize,
    /// Frames seen so far, capped at `TIME_FRAMES`.
    filled: usize,
}

impl HpssAnalyzer {
    pub fn new() -> Self {
        Self {
            bins: 0,
            ring: Vec::new(),
            pos: 0,
            filled: 0,
        }
    }

    /// Push the current magnitude frame and return the HPSS split over the causal window.
    ///
    /// `loud_silent` is the A10 perceptual-silence flag: the two energies are Adaptive
    /// (silence-gated by the normalizer regardless), but `harmonic_ratio` is Passthrough, so
    /// the whole field is returned neutral here to keep the ratio from chasing the noise floor.
    pub fn process(&mut self, mag: &[f32], loud_silent: bool) -> HpssFeatures {
        if mag.is_empty() {
            return HpssFeatures::NEUTRAL;
        }
        // Lazily size the ring to the spectrum width on the first frame (or a resolution change).
        if self.bins != mag.len() {
            self.bins = mag.len();
            self.ring = vec![0.0; TIME_FRAMES * self.bins];
            self.pos = 0;
            self.filled = 0;
        }

        // Overwrite the oldest slot with the current frame; the median then includes it.
        let bins = self.bins;
        self.ring[self.pos * bins..(self.pos + 1) * bins].copy_from_slice(mag);
        self.pos = (self.pos + 1) % TIME_FRAMES;
        if self.filled < TIME_FRAMES {
            self.filled += 1;
        }

        if loud_silent {
            return HpssFeatures::NEUTRAL;
        }

        let mut e_p = 0.0f64;
        let mut e_h = 0.0f64;
        let mut time_scratch = [0.0f32; TIME_FRAMES];
        let mut freq_scratch = [0.0f32; 2 * FREQ_RADIUS + 1];
        for f in 0..bins {
            // Harmonic estimate: trailing time-median at this bin (suppresses transients).
            for (k, slot) in time_scratch[..self.filled].iter_mut().enumerate() {
                *slot = self.ring[k * bins + f];
            }
            let h = median(&mut time_scratch[..self.filled]);
            // Percussive estimate: frequency-median over ±FREQ_RADIUS of the current frame
            // (suppresses tonal peaks); the window shrinks at the spectrum edges.
            let lo = f.saturating_sub(FREQ_RADIUS);
            let hi = (f + FREQ_RADIUS + 1).min(bins);
            let m = hi - lo;
            freq_scratch[..m].copy_from_slice(&mag[lo..hi]);
            let p = median(&mut freq_scratch[..m]);

            // Soft (Wiener) masks on power — Mp + Mh = 1, so they partition the bin's power.
            let (p2, h2) = ((p * p) as f64, (h * h) as f64);
            let denom = p2 + h2 + 1e-12;
            let x2 = (mag[f] * mag[f]) as f64;
            e_p += (p2 / denom) * x2;
            e_h += (h2 / denom) * x2;
        }

        let n = bins as f64;
        let e_p = e_p / n; // mean percussive power
        let e_h = e_h / n; // mean harmonic power
        let ratio = (e_h / (e_h + e_p + 1e-12)) as f32;
        HpssFeatures {
            percussive_energy: db_map(e_p),
            harmonic_energy: db_map(e_h),
            harmonic_ratio: ratio,
        }
    }
}

/// Mean masked power → RMS amplitude → dB, mapped −120..0 dB → 0..1 (see module doc for why
/// the map exists and why the window is wider than the bands' −80 dB).
fn db_map(power: f64) -> f32 {
    let rms = power.sqrt();
    if rms < 1e-10 {
        return 0.0;
    }
    let db = 20.0 * rms.log10() as f32;
    ((db + 120.0) / 120.0).clamp(0.0, 1.0)
}

impl Default for HpssAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Median of a scratch slice (sorted in place). Magnitudes are non-negative and finite, so the
/// partial comparison never hits the `Equal` fallback in practice.
fn median(vals: &mut [f32]) -> f32 {
    if vals.is_empty() {
        return 0.0;
    }
    vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = vals.len() / 2;
    if vals.len() % 2 == 1 {
        vals[mid]
    } else {
        0.5 * (vals[mid - 1] + vals[mid])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINS: usize = 513; // medium 1024-pt spectrum: 1024/2 + 1

    fn spike(bin: usize, amp: f32) -> Vec<f32> {
        let mut v = vec![0.0; BINS];
        v[bin] = amp;
        v
    }

    fn flat(amp: f32) -> Vec<f32> {
        vec![amp; BINS]
    }

    #[test]
    fn empty_input_is_neutral() {
        let f = HpssAnalyzer::new().process(&[], false);
        assert_eq!(f.harmonic_ratio, 0.5);
        assert_eq!(f.percussive_energy, 0.0);
        assert_eq!(f.harmonic_energy, 0.0);
    }

    #[test]
    fn silence_is_neutral() {
        // A loud frame flagged silent must still gate to neutral (ratio is Passthrough).
        let mut h = HpssAnalyzer::new();
        let f = h.process(&flat(1.0), true);
        assert_eq!(f.harmonic_ratio, 0.5);
        assert_eq!(f.percussive_energy, 0.0);
        assert_eq!(f.harmonic_energy, 0.0);
    }

    #[test]
    fn steady_tone_is_harmonic() {
        // A single-bin tone held steady: time-median keeps it, frequency-median (mostly zeros)
        // kills it → harmonic-dominant.
        let mut h = HpssAnalyzer::new();
        let tone = spike(100, 1.0);
        let mut f = HpssFeatures::NEUTRAL;
        for _ in 0..TIME_FRAMES {
            f = h.process(&tone, false);
        }
        assert!(f.harmonic_ratio > 0.9, "ratio {}", f.harmonic_ratio);
        assert!(
            f.harmonic_energy > f.percussive_energy,
            "h {} p {}",
            f.harmonic_energy,
            f.percussive_energy
        );
    }

    #[test]
    fn broadband_transient_is_percussive() {
        // A flat burst after a quiet history: time-median (mostly zeros) kills it, frequency-median
        // (flat) keeps it → percussive-dominant.
        let mut h = HpssAnalyzer::new();
        let zeros = vec![0.0; BINS];
        for _ in 0..TIME_FRAMES - 1 {
            h.process(&zeros, false);
        }
        let f = h.process(&flat(1.0), false);
        assert!(f.harmonic_ratio < 0.1, "ratio {}", f.harmonic_ratio);
        assert!(
            f.percussive_energy > f.harmonic_energy,
            "p {} h {}",
            f.percussive_energy,
            f.harmonic_energy
        );
    }

    #[test]
    fn quiet_signal_still_registers() {
        // Regression (Cleave starburst, 2026-07-20): at real listening levels the raw masked
        // powers are ~1e-7..1e-9 — below the normalizer's absolute span epsilon — so without the
        // dB map the energies normalized to a hard 0 on live music. A quiet tone (amp 1e-3, i.e.
        // −60 dBFS) must still produce a clearly nonzero energy for the ranger to work with.
        let mut h = HpssAnalyzer::new();
        let tone = spike(100, 1e-3);
        let mut f = HpssFeatures::NEUTRAL;
        for _ in 0..TIME_FRAMES {
            f = h.process(&tone, false);
        }
        assert!(f.harmonic_energy > 0.1, "h {}", f.harmonic_energy);
        assert!(f.harmonic_ratio > 0.9, "ratio {}", f.harmonic_ratio);
    }

    #[test]
    fn steady_broadband_is_balanced() {
        // Flat noise held steady: both medians land on the same level → ~50/50.
        let mut h = HpssAnalyzer::new();
        let noise = flat(0.5);
        let mut f = HpssFeatures::NEUTRAL;
        for _ in 0..TIME_FRAMES {
            f = h.process(&noise, false);
        }
        assert!(
            (f.harmonic_ratio - 0.5).abs() < 0.05,
            "ratio {}",
            f.harmonic_ratio
        );
    }
}
