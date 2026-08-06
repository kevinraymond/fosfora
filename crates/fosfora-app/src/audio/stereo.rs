//! A13 (#1464): stereo-field analysis — pan, mid/side width, and L/R correlation.
//! A13b (#1801): per-band pan — where each frequency band sits in the stereo image.
//!
//! The capture ring carries interleaved `L,R,L,R…` (see [`super::capture`]); the analysis thread
//! feeds each hop's stereo frames here. Metrics integrate over a rolling [`WINDOW`]-frame window
//! (~46 ms at 44.1 kHz) so they track the stereo *field* rather than instantaneous samples.
//!
//! All outputs are producer-normalized to 0..1 (schema policy `Passthrough`, so they survive
//! `normalize`/`smooth` unrescaled): the bipolar pan and correlation are remapped `0.5 + 0.5*x`;
//! width is a mid/side energy ratio already in 0..1.
//!
//! # Per-band pan
//!
//! The broadband `pan` collapses the whole mix to one number, so a centred kick under wide hats
//! reads the same as a mono track. Per-band pan resolves the stereo *image*: `band_pan[i]`
//! describes the same frequency range as `bands()[i]` in [`super::analyzer`], using the same edges
//! and the same 4096-point resolution, so the two always describe the same thing.
//!
//! Both channel spectra come from a **single** FFT. For two real sequences, packing one into the
//! real part and the other into the imaginary part (`z[n] = l[n] + i·r[n]`) makes the transform
//! recoverable: `L[k] = (Z[k] + conj(Z[N−k]))/2` and `R[k] = (Z[k] − conj(Z[N−k]))/(2i)`. Only
//! magnitudes are needed here and dividing by `2i` merely rotates, so no complex division appears
//! below — just `|Z[k] ± conj(Z[N−k])|/2`. Stereo therefore costs the same as one mono FFT.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

/// Rolling window length in stereo frames. 2048 @ 44.1 kHz ≈ 46 ms.
const WINDOW: usize = 2048;

/// Number of frequency bands, matching `analyzer::FftAnalyzer::bands`.
pub const NUM_BANDS: usize = 7;

/// FFT length for the per-band split, in stereo frames. Matches the analyzer's `FFT_LARGE`, so
/// `band_pan[i]` and `bands()[i]` see the same resolution: 10.8 Hz/bin at 44.1 kHz, which leaves
/// even the 20–60 Hz sub-bass band ~4 bins to average over.
const FFT_SIZE: usize = 4096;

/// Band edges in Hz, identical to the table in `analyzer::FftAnalyzer::bands`. The two must not
/// drift apart: `band_pan[i]` is only meaningful as "where `bands()[i]` is sitting".
const BAND_EDGES: [(f32, f32); NUM_BANDS] = [
    (20.0, 60.0),      // sub_bass
    (60.0, 250.0),     // bass
    (250.0, 500.0),    // low_mid
    (500.0, 2000.0),   // mid
    (2000.0, 4000.0),  // upper_mid
    (4000.0, 6000.0),  // presence
    (6000.0, 20000.0), // brilliance
];

/// The stereo-field features, each already mapped to 0..1 for the shader ABI.
#[derive(Debug, Clone, Copy)]
pub struct StereoFeatures {
    /// Stereo balance. 0.5 = centered, <0.5 = left-heavy, >0.5 = right-heavy.
    pub pan: f32,
    /// Mid/side width: `Es/(Em+Es)`. 0 = mono, →1 = fully decorrelated / anti-phase.
    pub stereo_width: f32,
    /// L/R correlation. 0.5 = decorrelated, 1 = mono/in-phase, 0 = anti-phase.
    pub stereo_corr: f32,
    /// A13b: per-band stereo balance, same convention as [`Self::pan`] and the same band order as
    /// `analyzer::FftAnalyzer::bands` (`sub_bass … brilliance`). A band carrying no meaningful
    /// energy reads 0.5 rather than chasing numerical noise.
    pub band_pan: [f32; NUM_BANDS],
}

impl StereoFeatures {
    /// Neutral field, emitted on silence where the energy denominators are undefined.
    pub const NEUTRAL: Self = Self {
        pan: 0.5,
        stereo_width: 0.0,
        stereo_corr: 0.5,
        band_pan: [0.5; NUM_BANDS],
    };
}

/// Integrates the stereo field over the last [`WINDOW`] frames.
///
/// The per-band split keeps its own longer ring ([`FFT_SIZE`]) rather than widening the broadband
/// window: `pan`/`stereo_width`/`stereo_corr` are shipped features whose ~46 ms integration is
/// part of their behaviour, and the low bands need a longer transform than that to resolve.
pub struct StereoAnalyzer {
    left: Box<[f32]>,
    right: Box<[f32]>,
    pos: usize,
    filled: usize,

    // --- A13b per-band pan ---
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    /// Time-domain ring for the packed transform, [`FFT_SIZE`] frames.
    fft_l: Box<[f32]>,
    fft_r: Box<[f32]>,
    fft_pos: usize,
    fft_filled: usize,
    /// Hann window, applied to both channels. The same window on both leaves the pan *ratio*
    /// untouched, but stops a loud low band leaking into a quiet high one and corrupting its pan.
    window: Box<[f32]>,
    /// Scratch for the packed `l + i·r` transform.
    scratch: Vec<Complex<f32>>,
    /// Per-band `[first_bin, last_bin]` (inclusive), precomputed from [`BAND_EDGES`].
    band_bins: [(usize, usize); NUM_BANDS],
}

impl StereoAnalyzer {
    pub fn new() -> Self {
        Self::with_sample_rate(44_100.0)
    }

    pub fn with_sample_rate(sample_rate: f32) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT_SIZE);
        let bin_hz = sample_rate / FFT_SIZE as f32;
        let nyquist_bin = FFT_SIZE / 2;

        // Bin 0 is DC and belongs to no band; every edge is clamped into 1..=N/2 so a band that
        // runs past Nyquist (brilliance at low sample rates) still yields a valid, non-empty range.
        let band_bins = BAND_EDGES.map(|(lo, hi)| {
            let first = ((lo / bin_hz).ceil() as usize).clamp(1, nyquist_bin);
            let last = ((hi / bin_hz).floor() as usize).clamp(first, nyquist_bin);
            (first, last)
        });

        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| {
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
            })
            .collect();

        Self {
            left: vec![0.0; WINDOW].into_boxed_slice(),
            right: vec![0.0; WINDOW].into_boxed_slice(),
            pos: 0,
            filled: 0,
            fft,
            fft_l: vec![0.0; FFT_SIZE].into_boxed_slice(),
            fft_r: vec![0.0; FFT_SIZE].into_boxed_slice(),
            fft_pos: 0,
            fft_filled: 0,
            window: window.into_boxed_slice(),
            scratch: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            band_bins,
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

            self.fft_l[self.fft_pos] = frame[0];
            self.fft_r[self.fft_pos] = frame[1];
            self.fft_pos = (self.fft_pos + 1) % FFT_SIZE;
            if self.fft_filled < FFT_SIZE {
                self.fft_filled += 1;
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
            band_pan: self.band_pan(),
        }
    }

    /// Per-band stereo balance from one packed `l + i·r` transform.
    ///
    /// Returns all-neutral until the transform ring has filled (~93 ms at 44.1 kHz) — a partial
    /// window would be a zero-padded, mis-windowed spectrum, which is worse than admitting we do
    /// not know yet.
    fn band_pan(&mut self) -> [f32; NUM_BANDS] {
        if self.fft_filled < FFT_SIZE {
            return [0.5; NUM_BANDS];
        }

        // Unwrap the ring into time order: the write head is the oldest sample.
        for i in 0..FFT_SIZE {
            let src = (self.fft_pos + i) % FFT_SIZE;
            let w = self.window[i];
            self.scratch[i] = Complex::new(self.fft_l[src] * w, self.fft_r[src] * w);
        }
        self.fft.process(&mut self.scratch);

        // |L[k]| = |Z[k] + conj(Z[N−k])|/2, |R[k]| = |Z[k] − conj(Z[N−k])|/2. The /(2i) on R is a
        // rotation, so it drops out of the magnitude. k and N−k are both taken mod N, which makes
        // the k=0 and k=N/2 cases fall out of the general form rather than needing special casing.
        let mut out = [0.5f32; NUM_BANDS];
        let mut band_energy = [0.0f64; NUM_BANDS];
        let mut pan_num = [0.0f64; NUM_BANDS];

        for (b, &(first, last)) in self.band_bins.iter().enumerate() {
            let (mut el, mut er) = (0.0f64, 0.0f64);
            for k in first..=last {
                let zk = self.scratch[k];
                let zc = self.scratch[(FFT_SIZE - k) % FFT_SIZE].conj();
                let l = zk + zc;
                let r = zk - zc;
                // norm_sqr of the un-halved sums; the shared factor 1/4 cancels in the ratio.
                el += l.norm_sqr() as f64;
                er += r.norm_sqr() as f64;
            }
            band_energy[b] = el + er;
            pan_num[b] = er - el;
        }

        // A band 60 dB below the loudest one carries nothing a viewer can see, and its pan is
        // numerical noise — hold those neutral rather than letting them jitter the image.
        let peak = band_energy.iter().cloned().fold(0.0f64, f64::max);
        const BAND_FLOOR: f64 = 1e-6;
        const EPS: f64 = 1e-12;
        for b in 0..NUM_BANDS {
            if band_energy[b] > peak * BAND_FLOOR {
                out[b] = (0.5 + 0.5 * (pan_num[b] / (band_energy[b] + EPS))) as f32;
            }
        }
        out
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

    // ---- A13b per-band pan ----

    /// Feed one full FFT window so the per-band path is live.
    fn run_bands(l: &[f32], r: &[f32]) -> StereoFeatures {
        StereoAnalyzer::new().process(&interleave(l, r))
    }

    fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    /// The whole point of the feature: a centred kick under hard-left hats must read as two
    /// different positions in the same frame. The broadband `pan` cannot express this.
    #[test]
    fn per_band_pan_separates_a_centered_low_from_a_panned_high() {
        let low = sine(50.0, FFT_SIZE); // sub_bass, 20–60 Hz
        let high = sine(8000.0, FFT_SIZE); // brilliance, 6–20 kHz
        let silent = vec![0.0; FFT_SIZE];

        // Kick centred (both channels), hats hard left (L only).
        let f = run_bands(&mix(&low, &high), &mix(&low, &silent));

        assert!(
            (f.band_pan[0] - 0.5).abs() < 0.05,
            "sub_bass should read centred, got {}",
            f.band_pan[0]
        );
        assert!(
            f.band_pan[6] < 0.05,
            "brilliance should read hard left, got {}",
            f.band_pan[6]
        );
    }

    /// Mirror of the above — guards against a sign flip that would mirror the whole stereo image,
    /// the class of bug that shipped in Splat's camera basis for two releases.
    #[test]
    fn per_band_pan_right_is_right() {
        let high = sine(8000.0, FFT_SIZE);
        let silent = vec![0.0; FFT_SIZE];
        let f = run_bands(&silent, &high);
        assert!(
            f.band_pan[6] > 0.95,
            "brilliance should read hard right, got {}",
            f.band_pan[6]
        );
    }

    #[test]
    fn mono_input_is_centered_in_every_band() {
        // Broadband content in one channel-identical signal: every band must read dead centre.
        let s = mix(
            &mix(&sine(50.0, FFT_SIZE), &sine(800.0, FFT_SIZE)),
            &sine(8000.0, FFT_SIZE),
        );
        let f = run_bands(&s, &s);
        for (b, &p) in f.band_pan.iter().enumerate() {
            assert!((p - 0.5).abs() < 0.02, "band {b} pan {p}, expected centred");
        }
    }

    #[test]
    fn silent_bands_hold_neutral_rather_than_chasing_noise() {
        // Only brilliance carries energy; the six silent bands must not report a position.
        let high = sine(8000.0, FFT_SIZE);
        let silent = vec![0.0; FFT_SIZE];
        let f = run_bands(&high, &silent);
        for b in 0..6 {
            assert_eq!(
                f.band_pan[b], 0.5,
                "empty band {b} should be exactly neutral, got {}",
                f.band_pan[b]
            );
        }
        assert!(f.band_pan[6] < 0.05, "brilliance {}", f.band_pan[6]);
    }

    #[test]
    fn per_band_pan_is_neutral_before_the_window_fills() {
        // One short hop: not enough for a transform, so it must admit that rather than guess.
        let f = StereoAnalyzer::new().process(&interleave(&sine(8000.0, 512), &vec![0.0; 512]));
        assert_eq!(f.band_pan, [0.5; NUM_BANDS]);
    }

    #[test]
    fn near_silence_is_neutral_in_every_band() {
        let q: Vec<f32> = sine(440.0, FFT_SIZE).iter().map(|x| x * 1e-4).collect();
        assert_eq!(run_bands(&q, &q).band_pan, [0.5; NUM_BANDS]);
    }

    /// Band edges must stay pinned to the analyzer's, or `band_pan[i]` stops describing `bands()[i]`.
    #[test]
    fn band_edges_match_the_analyzer_band_table() {
        assert_eq!(BAND_EDGES.len(), NUM_BANDS);
        assert_eq!(BAND_EDGES[0], (20.0, 60.0));
        assert_eq!(BAND_EDGES[6], (6000.0, 20000.0));
        // Contiguous: each band starts where the previous ended.
        for w in BAND_EDGES.windows(2) {
            assert_eq!(w[0].1, w[1].0, "band edges must be contiguous");
        }
    }

    /// A band running past Nyquist must still produce a usable bin range, not an empty or
    /// inverted one (brilliance ends at 20 kHz, above Nyquist for a 32 kHz device).
    #[test]
    fn band_bins_stay_valid_below_nyquist() {
        let a = StereoAnalyzer::with_sample_rate(32_000.0);
        for (b, &(first, last)) in a.band_bins.iter().enumerate() {
            assert!(first >= 1, "band {b} must not include DC");
            assert!(last >= first, "band {b} range inverted: {first}..{last}");
            assert!(last <= FFT_SIZE / 2, "band {b} past Nyquist bin");
        }
    }
}
