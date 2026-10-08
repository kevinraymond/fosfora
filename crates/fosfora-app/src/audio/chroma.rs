//! CQT-lite constant-Q chroma with tuning compensation (A11 #1462).
//!
//! Replaces the old FFT-bin → pitch-class histogram (which hard-rounded every bin
//! to the nearest of 12 classes) with sparse per-semitone Gaussian kernels over the
//! 4096-pt magnitude spectrum. The 61 semitone energies (MIDI 36–96) are octave-folded
//! to 12 pitch classes.
//!
//! Each frame is published in two forms (#2079): the **visual chroma** — harmonic
//! template applied, L-∞ normalized to 0..1 — feeds the feature bus, shaders and the
//! downbeat tracker exactly as before; the **key chroma** (`e12`) is the pure
//! unnormalized fold of the well-resolved kernels only (MIDI ≥ `KEY_FOLD_LO_MIDI`),
//! because the key detector needs energy weighting (a quiet frame must not vote like a
//! drop frame), no manufactured harmonics (the template's only cross-term deposits a
//! third of every note's energy on its subdominant, which measurably drags key
//! estimates a fifth flat), and no aliased bottom-octave garbage.
//!
//! A slow tuning estimator tracks the global A-reference offset (±50 cents) from
//! parabola-refined spectral peaks and shifts the kernel centers so a 432 Hz-tuned
//! track no longer smears across pitch classes. The offset is circular — +50 and −50
//! cents are the same tuning, a semitone apart in name only — so the histogram, its
//! mode and the smoothing all wrap (#61).

pub const N_CHROMA: usize = 12;

/// Constant-Q kernel note range: MIDI 36 (C2, ~65 Hz) .. MIDI 96 (C7, ~2093 Hz).
pub const MIDI_LO: i32 = 36;
const MIDI_HI: i32 = 96;
pub const N_SEMITONES: usize = (MIDI_HI - MIDI_LO + 1) as usize; // 61

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

/// The key path folds only kernels from MIDI 54 (F#3, ~185 Hz) upward (#2079). Below
/// that, semitone spacing at the 4096-pt resolution is narrower than one FFT bin, so
/// adjacent kernels collapse onto the SAME bin (C2 and C#2 both read bin 6): the bottom
/// octaves cannot attribute pitch class, and their single-bin kernels collect the
/// spectral skirts of sub-floor kick/bass fundamentals. Under energy weighting that
/// garbage dominated the key mean — measured on the bench fixture, the kick's skirt
/// made the whole bottom octave read C/C♯ and the detector called C minor; flooring the
/// fold reads A minor at 0.79 correlation. The visual chroma keeps the full fold so the
/// wheel still lights for bass.
const KEY_FOLD_LO_MIDI: i32 = 54;

/// Bass root tracker (#2079): the 32–183 Hz zone the floored key fold excludes is
/// where EDM keeps its root line, and it is essentially monophonic there — so a
/// single QIFFT-refined dominant peak, gated on being a clean local max, close to an
/// equal-tempered semitone (tuning-compensated), and stable for a few hops, is an
/// honest pitch-class observation where the aliased kernels were not. The accepted
/// observation deposits `W_BASS ×` its magnitude into the key fold. Kick transients
/// fail the persistence and cents gates: a swept kick never parks on one semitone
/// for [`BASS_PERSIST_HOPS`] at 86 fps.
///
/// The zone in Hz; [`CqtChroma::new`] converts it to bins at the running rate (#53). At
/// 44.1 kHz / 4096 it is bins 3..=17 (32.3–183 Hz).
const BASS_HZ_LO: f32 = 32.0;
/// Just under the MIDI 54 fold floor (185 Hz).
const BASS_HZ_HI: f32 = 184.0;
/// Bound on the zone's width in bins, for the median scratch. 15 at 44.1 kHz; it only grows
/// at device rates far below that.
const BASS_BAND_MAX: usize = 64;
/// Candidate must beat the local median by this factor (peak vs bass-band floor)…
const BASS_MEDIAN_FACTOR: f32 = 2.0;
/// …and an absolute magnitude floor (≈ −74 dBFS). Tuned while the spectrum read 6 dB low;
/// doubled with the coherent-gain correction (#54) so it sits at the same signal level.
const BASS_ABS_FLOOR: f32 = 2e-4;
/// Reject peaks further than this from an equal-tempered semitone (fraction of one).
const BASS_MAX_CENTS_OFF: f32 = 0.35;
/// Same pitch class for this many consecutive frames before it counts.
const BASS_PERSIST_HOPS: u8 = 3;
/// Deposit weight into the key fold, from the tune-half sweep (bench/sweep_key.py):
/// clean peak at 1.0 (.5524 vs .4840 without; exact 99→132, dominant-fifth errors
/// halved); ≥ 4 over-trusts the bass line and subdominant errors grow.
const W_BASS: f32 = 1.0;

/// Tuning histogram: 1 bin per cent over ±50 cents, circular (bin 0 neighbours bin 99).
const TUNING_BINS: usize = 100;
/// The mode is the heaviest window of `2 × TUNING_MODE_HALF + 1` bins, so a peak split
/// across neighbouring bins — or across the ±50 wrap — counts as one peak.
const TUNING_MODE_HALF: usize = 2;
/// How far past ±50 cents the estimate may run before it is renamed by a semitone.
/// Material tuned a quarter tone off would otherwise flip its pitch-class names every
/// time the estimate wobbles across the boundary.
const TUNING_WRAP_HYSTERESIS: f32 = 5.0;
/// Per-frame histogram decay at the reference hop rate: a 1,000-hop time constant, ≈ 11.6 s.
/// Rescaled to the same time at other rates (#53).
const TUNING_DECAY: f32 = 0.999;
/// EMA rate for the smoothed cents offset (slow — tuning is near-constant per track).
const TUNING_EMA: f32 = 0.02;
/// Rebuild kernels once the tuning estimate has drifted this far from the built value…
const KERNEL_REGEN_CENTS: f32 = 2.0;
/// …and no more often than this many frames at the reference hop rate (~3.5 s; rescaled to
/// the same time at other rates, #53) to avoid per-frame kernel churn.
const KERNEL_REGEN_MIN_FRAMES: u32 = 300;

/// One sparse constant-Q kernel per semitone: (fft_bin, weight) pairs.
type Kernel = Vec<(usize, f32)>;

/// A persistent low-band pitch observation from the bass root tracker (#2079).
#[derive(Clone, Copy)]
pub struct BassObs {
    /// Pitch class, 0 = C.
    pub pc: usize,
    /// Peak magnitude on the 2/N-scaled spectrum.
    pub mag: f32,
}

/// One analysis frame's chroma, in both published forms (#2079).
pub struct ChromaFrame {
    /// Harmonic-templated, L-∞ normalized to 0..1 — the visual/feature-bus chroma.
    pub chroma: [f32; N_CHROMA],
    /// Pure octave fold of the well-resolved kernels (MIDI ≥ `KEY_FOLD_LO_MIDI`),
    /// unnormalized magnitudes, plus the accepted bass deposit — the key detector's
    /// input.
    pub e12: [f32; N_CHROMA],
    /// This frame's accepted bass observation, if the tracker's gates all passed.
    pub bass: Option<BassObs>,
}

pub struct CqtChroma {
    num_bins: usize,
    bin_hz: f32,
    kernels: Vec<Kernel>, // N_SEMITONES entries

    // Rate-dependent constants, resolved once (#53).
    bass_bin_lo: usize,
    bass_bin_hi: usize,
    tuning_decay: f32,
    kernel_regen_min_frames: u32,

    // Tuning estimator
    tuning_hist: Vec<f32>, // TUNING_BINS, cents histogram (magnitude-weighted, decaying)
    tuning_cents: f32,     // EMA'd global offset from A440, in cents (±(50 + hysteresis))
    kernel_cents: f32,     // offset the current kernels were built for
    frames_since_regen: u32,

    /// This frame's per-semitone energies, pre-fold — the key-sidecar dump reads them
    /// (#2079) so offline sweeps can re-fold with any floor without a replica front-end.
    e61: [f32; N_SEMITONES],

    // Bass root tracker persistence state (#2079).
    bass_last_pc: Option<usize>,
    bass_run: u8,
}

impl CqtChroma {
    /// `hop_rate` is hops per second.
    pub fn new(num_bins: usize, bin_hz: f32, hop_rate: f32) -> Self {
        let bass_bin_lo = ((BASS_HZ_LO / bin_hz).ceil() as usize).max(1);
        let bass_bin_hi = ((BASS_HZ_HI / bin_hz).floor() as usize)
            .min(bass_bin_lo + BASS_BAND_MAX - 1)
            .max(bass_bin_lo);
        Self {
            num_bins,
            bin_hz,
            kernels: Self::build_kernels(num_bins, bin_hz, 0.0),
            bass_bin_lo,
            bass_bin_hi,
            tuning_decay: super::rescale_decay(TUNING_DECAY, hop_rate),
            kernel_regen_min_frames: super::rescale_hops(KERNEL_REGEN_MIN_FRAMES as usize, hop_rate)
                as u32,
            tuning_hist: vec![0.0; TUNING_BINS],
            tuning_cents: 0.0,
            kernel_cents: 0.0,
            frames_since_regen: 0,
            e61: [0.0; N_SEMITONES],
            bass_last_pc: None,
            bass_run: 0,
        }
    }

    /// This frame's per-semitone energies (see the `e61` field doc). Valid after
    /// `compute()`.
    pub fn e61(&self) -> &[f32; N_SEMITONES] {
        &self.e61
    }

    /// Compute one magnitude frame's chroma in both published forms (see module doc).
    /// Also advances the tuning estimator and lazily rebuilds kernels when tuning drifts.
    pub fn compute(&mut self, mag: &[f32]) -> ChromaFrame {
        self.update_tuning(mag);
        self.maybe_regen_kernels();

        // Weighted-magnitude energy per semitone, octave-folded into 12 pitch classes.
        // Two folds: the full range feeds the visual template; the key fold starts at
        // `KEY_FOLD_LO_MIDI`, above the pairwise-aliased bottom octaves (see its doc).
        let mut e12_vis = [0.0f32; N_CHROMA];
        let mut e12_key = [0.0f32; N_CHROMA];
        for (s, kernel) in self.kernels.iter().enumerate() {
            let mut e = 0.0f32;
            for &(k, w) in kernel {
                e += mag[k] * w;
            }
            self.e61[s] = e;
            let midi = MIDI_LO + s as i32;
            let pc = ((midi % 12 + 12) % 12) as usize;
            e12_vis[pc] += e;
            if midi >= KEY_FOLD_LO_MIDI {
                e12_key[pc] += e;
            }
        }
        let e12 = e12_vis;

        // Harmonic reinforcement (visual chroma only): gather each fundamental's
        // harmonics with 1/h weight. Post-fold, the octave terms collapse onto the
        // fundamental, so the sole cross-term is the +7 gather — equivalently, every
        // sounding note deposits 1/3 of its energy on its subdominant. That reads
        // fine on a chroma wheel but is poison for key profiles, so `e12` stays pure.
        let mut chroma = [0.0f32; N_CHROMA];
        for (p, c) in chroma.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for (h, &off) in HARM_OFFSET_UP.iter().enumerate() {
                let src = (p as i32 + off).rem_euclid(12) as usize;
                acc += HARM_WEIGHT[h] * e12[src];
            }
            *c = acc;
        }

        // L-∞ normalize the visual chroma.
        let max = chroma.iter().cloned().fold(0.0f32, f32::max);
        if max > 1e-10 {
            for c in &mut chroma {
                *c /= max;
            }
        }

        // Bass root tracker: deposit the accepted low-band observation into the key
        // fold (key path only; the visual chroma never sees it).
        let bass = self.track_bass(mag);
        if let Some(obs) = bass {
            e12_key[obs.pc] += W_BASS * obs.mag;
        }

        ChromaFrame {
            chroma,
            e12: e12_key,
            bass,
        }
    }

    /// One frame of the bass root tracker (see the constants' doc): dominant local-max
    /// peak in 32–183 Hz, QIFFT-refined, tuning-compensated, near-semitone, persistent
    /// for [`BASS_PERSIST_HOPS`] frames.
    fn track_bass(&mut self, mag: &[f32]) -> Option<BassObs> {
        let lo = self.bass_bin_lo;
        let hi = self.bass_bin_hi.min(self.num_bins.saturating_sub(2));
        if hi < lo {
            return None;
        }
        let mut best: Option<(usize, f32)> = None;
        let mut band = [0.0f32; BASS_BAND_MAX];
        for (i, k) in (lo..=hi).enumerate() {
            band[i] = mag[k];
            if mag[k] > mag[k - 1] && mag[k] >= mag[k + 1] && best.is_none_or(|(_, m)| mag[k] > m) {
                best = Some((k, mag[k]));
            }
        }
        let n_band = hi + 1 - lo;
        let median = {
            let b = &mut band[..n_band];
            b.sort_by(|a, c| a.total_cmp(c));
            b[n_band / 2]
        };

        let accepted = best.and_then(|(k, m)| {
            if m < BASS_ABS_FLOOR || m < BASS_MEDIAN_FACTOR * median {
                return None;
            }
            // QIFFT vertex refinement on LOG magnitude — the linear-magnitude variant
            // the tuning estimator uses is biased by ~0.05 bins on wide low-frequency
            // mainlobes, which at 33 Hz is ±40 cents: enough to fail the semitone gate
            // on a perfectly steady sub. Log magnitude is parabola-exact there.
            let (alpha, gamma) = (mag[k - 1].max(1e-12).ln(), mag[k + 1].max(1e-12).ln());
            let beta = m.max(1e-12).ln();
            let denom = alpha - 2.0 * beta + gamma;
            let p = if denom.abs() > 1e-12 {
                (0.5 * (alpha - gamma) / denom).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            let freq = (k as f32 + p) * self.bin_hz;
            let ref_hz = 440.0 * 2.0f32.powf(self.tuning_cents / 1200.0);
            let midi = 69.0 + 12.0 * (freq / ref_hz).log2();
            if (midi - midi.round()).abs() >= BASS_MAX_CENTS_OFF {
                return None;
            }
            let pc = (midi.round() as i32).rem_euclid(12) as usize;
            Some((pc, m))
        });

        match accepted {
            Some((pc, m)) => {
                if self.bass_last_pc == Some(pc) {
                    self.bass_run = self.bass_run.saturating_add(1);
                } else {
                    self.bass_last_pc = Some(pc);
                    self.bass_run = 1;
                }
                (self.bass_run >= BASS_PERSIST_HOPS).then_some(BassObs { pc, mag: m })
            }
            None => {
                self.bass_last_pc = None;
                self.bass_run = 0;
                None
            }
        }
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
            *h *= self.tuning_decay;
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
            let cents = (semitone - semitone.round()) * 100.0; // [−50, 50]
            // +50 and −50 are the same tuning: both land in bin 0.
            let bin = ((cents + 50.0) / 100.0 * TUNING_BINS as f32) as usize % TUNING_BINS;
            self.tuning_hist[bin] += b;
        }

        // Mode of the circular histogram — only trust it when clearly concentrated.
        let Some(mode_cents) = circular_mode_cents(&self.tuning_hist) else {
            return;
        };
        // Step the shorter way round the circle, so an estimate near +50 heading for a
        // mode near −50 moves a few cents instead of sweeping through 0.
        self.tuning_cents += TUNING_EMA * wrap_cents(mode_cents - self.tuning_cents);
        let limit = 50.0 + TUNING_WRAP_HYSTERESIS;
        if self.tuning_cents > limit {
            self.tuning_cents -= 100.0;
        } else if self.tuning_cents < -limit {
            self.tuning_cents += 100.0;
        }
    }

    fn maybe_regen_kernels(&mut self) {
        self.frames_since_regen = self.frames_since_regen.saturating_add(1);
        if (self.tuning_cents - self.kernel_cents).abs() > KERNEL_REGEN_CENTS
            && self.frames_since_regen > self.kernel_regen_min_frames
        {
            self.kernels = Self::build_kernels(self.num_bins, self.bin_hz, self.tuning_cents);
            self.kernel_cents = self.tuning_cents;
            self.frames_since_regen = 0;
        }
    }
}

/// Wrap a cents difference into [−50, 50).
fn wrap_cents(c: f32) -> f32 {
    (c + 50.0).rem_euclid(100.0) - 50.0
}

/// The histogram's mode in cents, or `None` when no window stands out. The heaviest
/// circular window of `2 × TUNING_MODE_HALF + 1` bins wins, and the mode is that
/// window's weighted centre; it is trusted only when the window's tallest bin clears
/// 3× the mean bin.
fn circular_mode_cents(hist: &[f32]) -> Option<f32> {
    let n = hist.len();
    let half = TUNING_MODE_HALF as isize;
    let at = |i: usize, d: isize| hist[(i as isize + d).rem_euclid(n as isize) as usize];

    let mut best = 0usize;
    let mut best_mass = 0.0f32;
    for i in 0..n {
        let mass: f32 = (-half..=half).map(|d| at(i, d)).sum();
        if mass > best_mass {
            best_mass = mass;
            best = i;
        }
    }
    let peak = (-half..=half).map(|d| at(best, d)).fold(0.0f32, f32::max);
    let mean = hist.iter().sum::<f32>() / n as f32;
    if peak <= 1e-6 || peak <= 3.0 * mean {
        return None;
    }

    // Offset of the window's centre of mass from its middle bin, in bins.
    let offset = (-half..=half).map(|d| d as f32 * at(best, d)).sum::<f32>() / best_mass;
    let bins_per_cent = n as f32 / 100.0;
    Some(wrap_cents(
        (best as f32 + 0.5 + offset) / bins_per_cent - 50.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bass zone is fixed in Hz, so 48 kHz neither shifts it nor lets it reach the key
    /// fold's floor at 185 Hz (#53).
    #[test]
    fn bass_zone_stays_in_hz_across_rates() {
        let at_44 = CqtChroma::new(
            num_bins(),
            44_100.0 / FFT as f32,
            crate::audio::REFERENCE_HOP_RATE,
        );
        assert_eq!((at_44.bass_bin_lo, at_44.bass_bin_hi), (3, 17));
        let bh = bin_hz();
        let at_48 = CqtChroma::new(num_bins(), bh, crate::audio::hop_rate(SR));
        assert!(at_48.bass_bin_lo as f32 * bh >= BASS_HZ_LO);
        assert!(at_48.bass_bin_hi as f32 * bh < 185.0);
        // Same tuning time constant in seconds: decay per second matches.
        let per_sec = |c: &CqtChroma, rate: f32| c.tuning_decay.powf(crate::audio::hop_rate(rate));
        assert!((per_sec(&at_44, 44_100.0) - per_sec(&at_48, SR)).abs() < 1e-4);
    }

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
        let mut cqt = CqtChroma::new(num_bins(), bin_hz(), crate::audio::REFERENCE_HOP_RATE);
        let mag = sine_mag(440.0);
        let mut chroma = [0.0f32; N_CHROMA];
        for _ in 0..2000 {
            chroma = cqt.compute(&mag).chroma;
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
        let mut cqt = CqtChroma::new(num_bins(), bin_hz(), crate::audio::REFERENCE_HOP_RATE);
        let mag = sine_mag(432.0);
        let mut chroma = [0.0f32; N_CHROMA];
        for _ in 0..3000 {
            chroma = cqt.compute(&mag).chroma;
        }
        assert_eq!(dominant(&chroma), 9, "chroma = {chroma:?}");
        // 432 Hz is ~−31.8 cents flat of A440.
        assert!(
            (cqt.tuning_cents - (-31.8)).abs() < 6.0,
            "tuning estimate {} cents, expected ≈ −32",
            cqt.tuning_cents
        );
    }

    /// Circular distance in cents between two tuning offsets.
    fn cents_apart(a: f32, b: f32) -> f32 {
        wrap_cents(a - b).abs()
    }

    /// #61: the tuning estimate moves the short way round the circle. Material settled
    /// just sharp of a quarter tone (+46.5 measured) followed by material just flat of one
    /// (−42.5) is an 11-cent move across the ±50 wrap; stepping linearly instead swept
    /// the estimate through 0, a quarter tone from both, for hundreds of hops.
    #[test]
    fn tuning_crosses_the_wrap_the_short_way() {
        let mut cqt = CqtChroma::new(num_bins(), bin_hz(), crate::audio::REFERENCE_HOP_RATE);
        // Measured by the QIFFT on these synthetic peaks as +46.5 and −42.5 cents.
        let sharp = sine_mag(392.0 * 2.0f32.powf(49.0 / 1200.0));
        let flat = sine_mag(440.0 * 2.0f32.powf(-45.0 / 1200.0));
        for _ in 0..1500 {
            cqt.compute(&sharp);
        }
        assert!(
            cents_apart(cqt.tuning_cents, 46.5) < 2.0,
            "settled at {}",
            cqt.tuning_cents
        );
        let mut worst = 0.0f32;
        for _ in 0..4000 {
            cqt.compute(&flat);
            worst = worst.max(cents_apart(cqt.tuning_cents, 50.0));
        }
        assert!(
            worst < 9.0,
            "estimate strayed {worst} cents from the boundary"
        );
        assert!(
            (cqt.tuning_cents - (-42.5)).abs() < 2.0,
            "ended at {} cents, expected ≈ −42.5 (renamed past the wrap)",
            cqt.tuning_cents
        );
    }

    /// A single peak split across the wrap is one peak, centred on the boundary.
    #[test]
    fn mode_of_a_peak_split_across_the_wrap() {
        let mut hist = vec![0.1f32; TUNING_BINS];
        hist[TUNING_BINS - 1] = 5.0;
        hist[0] = 5.0;
        hist[40] = 7.0; // taller single bin, but less mass than the split peak
        let mode = circular_mode_cents(&hist).expect("concentrated");
        assert!(cents_apart(mode, 50.0) < 0.5, "mode {mode}");
    }

    #[test]
    fn flat_histogram_has_no_mode() {
        assert_eq!(circular_mode_cents(&[1.0f32; TUNING_BINS]), None);
    }

    #[test]
    fn wrap_cents_takes_the_short_way_round() {
        assert_eq!(wrap_cents(98.0), -2.0);
        assert_eq!(wrap_cents(-98.0), 2.0);
        assert_eq!(wrap_cents(10.0), 10.0);
    }

    #[test]
    fn silence_is_finite_and_untuned() {
        let mut cqt = CqtChroma::new(num_bins(), bin_hz(), crate::audio::REFERENCE_HOP_RATE);
        let mag = vec![0.0f32; num_bins()];
        for _ in 0..100 {
            let frame = cqt.compute(&mag);
            for v in frame.chroma.iter().chain(frame.e12.iter()) {
                assert!(v.is_finite());
            }
        }
        assert_eq!(cqt.tuning_cents, 0.0);
    }

    /// #2079: a persistent sub-kernel-range fundamental must be observed by the bass
    /// tracker with the correct pitch class — this is the only key-path
    /// representation of the 32–183 Hz zone the floored fold excludes.
    #[test]
    fn bass_tracker_attributes_sub_fundamentals() {
        for (hz, want_pc, name) in [(41.2, 4, "E1"), (32.7, 0, "C1"), (55.0, 9, "A1")] {
            let mut cqt = CqtChroma::new(num_bins(), bin_hz(), crate::audio::REFERENCE_HOP_RATE);
            let mag = sine_mag(hz);
            let mut frame = cqt.compute(&mag);
            for _ in 0..5 {
                frame = cqt.compute(&mag);
            }
            let obs = frame.bass.unwrap_or_else(|| panic!("{name} not observed"));
            assert_eq!(obs.pc, want_pc, "{name} pitch class");
        }
    }

    /// A kick-style frequency sweep never parks on one semitone long enough to pass
    /// the persistence gate — the tracker must stay silent on it.
    #[test]
    fn bass_tracker_rejects_swept_kick() {
        let mut cqt = CqtChroma::new(num_bins(), bin_hz(), crate::audio::REFERENCE_HOP_RATE);
        for i in 0..30 {
            let frame = cqt.compute(&sine_mag(80.0 - 4.0 * i as f32 % 50.0));
            assert!(frame.bass.is_none(), "sweep accepted at step {i}");
        }
    }

    /// #2079: the key path's `e12` is a pure fold — a single note deposits energy
    /// only on its own class (± kernel bleed), while the visual chroma's harmonic
    /// template manufactures a subdominant at 1/3 weight. This test documents WHY
    /// the two outputs exist; it fails if the template ever leaks back into `e12`.
    #[test]
    fn pure_fold_has_no_subdominant_deposit() {
        let mut cqt = CqtChroma::new(num_bins(), bin_hz(), crate::audio::REFERENCE_HOP_RATE);
        let mag = sine_mag(440.0); // A, pitch class 9; its subdominant is D, class 2.
        let mut frame = cqt.compute(&mag);
        for _ in 0..10 {
            frame = cqt.compute(&mag);
        }

        // Pure fold: nothing lands on D beyond numerical dust.
        assert!(
            frame.e12[2] < frame.e12[9] * 1e-3,
            "e12 subdominant contamination: {:?}",
            frame.e12
        );
        // Visual chroma: the template gathers e12[p+7] at 1/3 against the 1.75×
        // self-term, so D reads at ≈ 0.19 of A.
        let ratio = frame.chroma[2] / frame.chroma[9];
        assert!(
            (ratio - (1.0 / 3.0) / 1.75).abs() < 0.02,
            "templated subdominant ratio {ratio}, chroma = {:?}",
            frame.chroma
        );
    }
}
