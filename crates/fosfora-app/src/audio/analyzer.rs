use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

use super::chroma::CqtChroma;
use super::features::AudioFeatures;
use super::kick_model::{KickDetector, kick_from_probability};
use crate::settings::BandScale;

/// FFT sizes for multi-resolution analysis.
const FFT_LARGE: usize = 4096; // 10.8 Hz/bin — sub_bass, bass, kick
const FFT_MED: usize = 1024; // 43 Hz/bin — low_mid, mid, upper_mid
const FFT_SMALL: usize = 512; // 86 Hz/bin — presence, brilliance

/// MFCC / chroma constants
const N_MELS: usize = 26;
const N_MFCC: usize = 13;
const N_CHROMA: usize = 12;

/// A4 (#1455): the `centroid` feature maps a power-weighted mean of log2(frequency) onto
/// 0..1 across this musical range, so it reads as a perceptual brightness fader instead
/// of hugging the top octave on a linear-Hz axis. `rolloff` and `zcr` share the axis (#62).
const CENTROID_F_MIN: f32 = 40.0;
const CENTROID_F_MAX: f32 = 18000.0;

/// `bandwidth` is the power-weighted spread of log2(frequency) around the centroid, in
/// octaves; this many octaves reads 1.0 (#62). A pure tone reads ~0, pink noise ~0.75.
const BANDWIDTH_OCTAVES: f32 = 4.0;

/// A4 (#1455): fraction of spectral energy below the rolloff frequency. Configurable
/// (was a hardcoded 0.85); can be promoted to a user setting later with no ABI impact.
const ROLLOFF_PERCENTILE: f32 = 0.85;

/// A4 (#1455): magnitude floor (~−54 dB below a full-scale tone) applied before the log in
/// the spectral-flux feature. Per-bin log magnitude is wildly unstable near the noise floor —
/// without a floor, sub-signal bins dominate the flux and it tracks level again. Clamping
/// them to a common floor makes their frame-to-frame diff zero, so flux reads change only.
/// Tuned while the spectrum read 6 dB low; doubled with the coherent-gain correction (#54)
/// so it sits at the same signal level.
const FLUX_FLOOR: f32 = 2e-3;

/// Mel bands for the A17 scrolling spectrogram texture (#1468). Independent of the
/// MFCC filterbank (N_MELS) — a higher band count gives finer vertical detail in the
/// waterfall (Strata #1479) without touching MFCC output. Also the height of the
/// R8Unorm spectrogram texture (consumed by `gpu::audio_textures`).
pub const SPECTROGRAM_MELS: usize = 64;

/// Bins in the A17 log-frequency spectrum texture (#1468). Matches the R16Float 512x1
/// texture width declared in the shader ABI.
pub const SPECTRUM_BINS: usize = 512;

/// A single FFT resolution with its own window and buffers.
struct FftResolution {
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    size: usize,
    window: Vec<f32>,
    /// `2 / Σw`: one-sided amplitude scaling with the window's coherent gain divided out,
    /// so a full-scale sine reads 1.0 (0 dB) at its peak bin (#54).
    scale: f32,
    fft_buffer: Vec<Complex<f32>>,
    magnitude: Vec<f32>,
    prev_magnitude: Vec<f32>,
    num_bins: usize,
    bin_hz: f32,
}

impl FftResolution {
    fn new(planner: &mut FftPlanner<f32>, size: usize, sample_rate: f32) -> Self {
        let fft = planner.plan_fft_forward(size);
        let num_bins = size / 2 + 1;
        let bin_hz = sample_rate / size as f32;

        // Hann window
        let window: Vec<f32> = (0..size)
            .map(|i| {
                0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (size - 1) as f32).cos())
            })
            .collect();
        let scale = 2.0 / window.iter().sum::<f32>();

        Self {
            fft,
            size,
            window,
            scale,
            fft_buffer: vec![Complex::new(0.0, 0.0); size],
            magnitude: vec![0.0; num_bins],
            prev_magnitude: vec![0.0; num_bins],
            num_bins,
            bin_hz,
        }
    }

    /// Compute FFT from the tail of the time-domain buffer.
    fn compute(&mut self, time_domain: &[f32]) {
        let td_len = time_domain.len();
        let offset = td_len.saturating_sub(self.size);
        let samples = &time_domain[offset..];
        let n = samples.len().min(self.size);

        // Apply window
        for i in 0..self.size {
            let s = if i < n { samples[i] } else { 0.0 };
            let w = self.window[i];
            self.fft_buffer[i] = Complex::new(s * w, 0.0);
        }

        self.fft.process(&mut self.fft_buffer);

        // Save previous magnitude
        std::mem::swap(&mut self.magnitude, &mut self.prev_magnitude);

        // Compute magnitude spectrum
        for i in 0..self.num_bins {
            self.magnitude[i] = self.fft_buffer[i].norm() * self.scale;
        }
    }

    fn bin_range(&self, lo_hz: f32, hi_hz: f32) -> (usize, usize) {
        let lo = (lo_hz / self.bin_hz).round() as usize;
        let hi = ((hi_hz / self.bin_hz).round() as usize).min(self.num_bins);
        (lo, hi)
    }

    /// RMS band energy (linear).
    fn band_energy_linear(&self, lo_hz: f32, hi_hz: f32) -> f32 {
        let (lo, hi) = self.bin_range(lo_hz, hi_hz);
        let hi = hi.min(self.num_bins);
        let count = hi.saturating_sub(lo).max(1);
        let sum: f32 = self.magnitude[lo..hi].iter().map(|m| m * m).sum();
        (sum / count as f32).sqrt()
    }

    /// dB-scaled band energy over 80dB range, normalized to 0-1.
    fn band_energy_db(&self, lo_hz: f32, hi_hz: f32) -> f32 {
        let linear = self.band_energy_linear(lo_hz, hi_hz);
        if linear < 1e-10 {
            return 0.0;
        }
        let db = 20.0 * linear.log10();
        // Map -80dB..0dB → 0..1
        ((db + 80.0) / 80.0).clamp(0.0, 1.0)
    }

    /// dB band energy over a tighter `floor_db..0` window with an equal-loudness `tilt_db`
    /// added before mapping (A1 #1452). Used by the unified `BandScale::Db` path so all seven
    /// bands share one comparable scale.
    fn band_energy_db_window(&self, lo_hz: f32, hi_hz: f32, floor_db: f32, tilt_db: f32) -> f32 {
        let linear = self.band_energy_linear(lo_hz, hi_hz);
        if linear < 1e-10 {
            return 0.0;
        }
        let db = 20.0 * linear.log10() + tilt_db;
        ((db - floor_db) / -floor_db).clamp(0.0, 1.0)
    }

    /// Half-wave rectified spectral flux in a frequency range (linear magnitude).
    fn spectral_flux_range(&self, lo_hz: f32, hi_hz: f32) -> f32 {
        let (lo, hi) = self.bin_range(lo_hz, hi_hz);
        let hi = hi.min(self.num_bins);
        let count = hi.saturating_sub(lo).max(1);
        let mut flux = 0.0f32;
        for i in lo..hi {
            let diff = self.magnitude[i] - self.prev_magnitude[i];
            if diff > 0.0 {
                flux += diff;
            }
        }
        flux / count as f32
    }
}

/// Sparse mel filterbank: for each mel band, stores (bin_index, weight) pairs.
type MelFilter = Vec<Vec<(usize, f32)>>;

fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10.0f32.powf(mel / 2595.0) - 1.0)
}

/// Map a linear amplitude to 0..1 over an 80 dB range (−80..0 dB → 0..1). Shared by the
/// A17 spectrum/spectrogram textures so their brightness matches the dB-scaled bands.
fn amp_to_db01(amp: f32) -> f32 {
    if amp < 1e-10 {
        return 0.0;
    }
    let db = 20.0 * amp.log10();
    ((db + 80.0) / 80.0).clamp(0.0, 1.0)
}

/// A16 (#1467): peak-vs-valley contrast of one band's linear magnitudes, mapped 0-60 dB → 0..1.
/// Takes the mean of the top and bottom 2% of bins (≥1 each, librosa's `alpha`); `scratch` is
/// sorted in place and `band` is left untouched. An empty band or a near-silent one (both means at
/// the floor) yields 0; a lone peak over a silent floor saturates to 1.
fn band_contrast(band: &[f32], scratch: &mut Vec<f32>) -> f32 {
    let n = band.len();
    if n == 0 {
        return 0.0;
    }
    scratch.clear();
    scratch.extend_from_slice(band);
    scratch.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let k = ((n as f32 * 0.02).ceil() as usize).clamp(1, n);
    let valley = scratch[..k].iter().sum::<f32>() / k as f32;
    let peak = scratch[n - k..].iter().sum::<f32>() / k as f32;
    // Floor the means so a near-silent band (both ≈ 0) gives a ~0 dB gap rather than NaN/±inf.
    const EPS: f32 = 1e-10;
    let peak_db = 20.0 * (peak + EPS).log10();
    let valley_db = 20.0 * (valley + EPS).log10();
    ((peak_db - valley_db) / 60.0).clamp(0.0, 1.0)
}

/// A4 (#1455): spectral flatness as **Wiener entropy over the 26 mel bands** — the
/// ratio of their geometric to arithmetic mean, already in 0..1 (FixedRange). Computing
/// it over mel bands (every band counted, with a tiny floor) removes the tiny-bin
/// skipping bias and the raw-FFT HF dominance of the old version, so it cleanly
/// separates tonal pads (low) from noise sweeps (high).
fn spectral_flatness(mel: &[f32; N_MELS]) -> f32 {
    let mut log_sum = 0.0f64;
    let mut linear_sum = 0.0f64;
    for &e in mel {
        let e = e as f64 + 1e-12;
        log_sum += e.ln();
        linear_sum += e;
    }
    let n = N_MELS as f64;
    let arithmetic_mean = linear_sum / n;
    if arithmetic_mean < 1e-12 {
        return 0.0;
    }
    let geometric_mean = (log_sum / n).exp();
    (geometric_mean / arithmetic_mean).clamp(0.0, 1.0) as f32
}

/// Multi-resolution FFT analyzer with 7 frequency bands and spectral features.
pub struct FftAnalyzer {
    large: FftResolution,  // 4096-pt for bass
    medium: FftResolution, // 1024-pt for mids
    small: FftResolution,  // 512-pt for highs

    time_domain: Vec<f32>, // Shared sample accumulator (FFT_LARGE length)
    sample_rate: f32,

    // A1 (#1452): how the 7 bands are scaled (unified dB vs legacy linear/dB split).
    band_scale: BandScale,

    // Kick detection (#3624): the tree ensemble scores every hop in `extract_features`;
    // `kick_envelope` maps the score onto `kick` once the silence flag is known.
    kick_detector: KickDetector,
    kick_probability: f32,

    // MFCC precomputed data
    mel_filters: MelFilter,              // N_MELS sparse triangular filters
    dct_matrix: [[f32; N_MELS]; N_MFCC], // DCT-II coefficients

    // A17 spectrogram filterbank: SPECTROGRAM_MELS sparse triangular filters (#1468)
    spectrogram_mel: MelFilter,

    // A11 (#1462): CQT-lite constant-Q chroma with tuning compensation.
    cqt: CqtChroma,
    // #2079: this hop's pure-fold unnormalized chroma, read by the key detector.
    key_chroma: [f32; N_CHROMA],
    // #2079: this hop's accepted bass root observation, for the key sidecar.
    key_bass: Option<super::chroma::BassObs>,

    // log2 of each large-spectrum bin's frequency, for the log centroid (DC holds 0 and
    // is never read). Precomputed so a hop doesn't take ~2k logs (#76).
    log2_bin_hz: Vec<f32>,
    // Reused sort scratch for `spectral_contrast` (#75).
    contrast_scratch: Vec<f32>,
}

impl FftAnalyzer {
    pub fn new(sample_rate: f32, band_scale: BandScale) -> Self {
        let mut planner = FftPlanner::new();

        let large = FftResolution::new(&mut planner, FFT_LARGE, sample_rate);
        let medium = FftResolution::new(&mut planner, FFT_MED, sample_rate);
        let small = FftResolution::new(&mut planner, FFT_SMALL, sample_rate);

        log::info!(
            "Multi-resolution FFT: {FFT_LARGE}/{FFT_MED}/{FFT_SMALL}-pt, {:.1}/{:.1}/{:.1} Hz/bin",
            large.bin_hz,
            medium.bin_hz,
            small.bin_hz
        );

        // Precompute mel filterbank (26 triangular filters, 20 Hz – Nyquist)
        let mel_filters = Self::build_mel_filterbank(
            large.num_bins,
            large.bin_hz,
            20.0,
            sample_rate * 0.5,
            N_MELS,
        );

        // Precompute the A17 spectrogram filterbank (64 bands, 20 Hz – Nyquist) — a
        // higher-resolution bank dedicated to the scrolling mel-spectrogram texture (#1468).
        let spectrogram_mel = Self::build_mel_filterbank(
            large.num_bins,
            large.bin_hz,
            20.0,
            sample_rate * 0.5,
            SPECTROGRAM_MELS,
        );

        // Precompute DCT-II matrix: dct[i][j] = cos(PI * i * (j + 0.5) / N_MELS) * sqrt(2/N_MELS)
        let scale = (2.0 / N_MELS as f32).sqrt();
        let mut dct_matrix = [[0.0f32; N_MELS]; N_MFCC];
        for i in 0..N_MFCC {
            for j in 0..N_MELS {
                dct_matrix[i][j] =
                    (std::f32::consts::PI * i as f32 * (j as f32 + 0.5) / N_MELS as f32).cos()
                        * scale;
            }
        }

        // A11 (#1462): CQT-lite constant-Q chroma over the large (4096-pt) spectrum.
        let cqt = CqtChroma::new(large.num_bins, large.bin_hz, super::hop_rate(sample_rate));

        let log2_bin_hz = (0..large.num_bins)
            .map(|i| {
                if i == 0 {
                    0.0
                } else {
                    (i as f32 * large.bin_hz).log2()
                }
            })
            .collect();

        Self {
            large,
            medium,
            small,
            time_domain: vec![0.0; FFT_LARGE],
            sample_rate,
            band_scale,
            kick_detector: KickDetector::new(sample_rate, FFT_LARGE, FFT_MED),
            kick_probability: 0.0,
            mel_filters,
            dct_matrix,
            spectrogram_mel,
            cqt,
            key_chroma: [0.0; N_CHROMA],
            key_bass: None,
            log2_bin_hz,
            contrast_scratch: Vec::new(),
        }
    }

    /// This hop's pure-fold unnormalized chroma — the key detector's input (#2079).
    /// Valid after `analyze()`; the visual `AudioFeatures::chroma` stays templated
    /// and L-∞ normalized for the feature bus.
    pub fn key_chroma(&self) -> &[f32; N_CHROMA] {
        &self.key_chroma
    }

    /// This hop's pre-fold per-semitone energies, for the key-sidecar dump (#2079).
    pub fn key_e61(&self) -> &[f32; super::chroma::N_SEMITONES] {
        self.cqt.e61()
    }

    /// This hop's accepted bass root observation, for the key-sidecar dump (#2079).
    pub fn key_bass(&self) -> Option<super::chroma::BassObs> {
        self.key_bass
    }

    /// Build sparse mel filterbank: `n_mels` triangular filters from lo_hz to hi_hz.
    fn build_mel_filterbank(
        num_bins: usize,
        bin_hz: f32,
        lo_hz: f32,
        hi_hz: f32,
        n_mels: usize,
    ) -> MelFilter {
        let lo_mel = hz_to_mel(lo_hz);
        let hi_mel = hz_to_mel(hi_hz);

        // n_mels + 2 mel-spaced center frequencies (edges of the triangles)
        let n_points = n_mels + 2;
        let mel_points: Vec<f32> = (0..n_points)
            .map(|i| lo_mel + (hi_mel - lo_mel) * i as f32 / (n_points - 1) as f32)
            .collect();
        let hz_points: Vec<f32> = mel_points.iter().map(|&m| mel_to_hz(m)).collect();
        let bin_points: Vec<usize> = hz_points
            .iter()
            .map(|&hz| (hz / bin_hz).round() as usize)
            .collect();

        let mut filters = Vec::with_capacity(n_mels);
        for m in 0..n_mels {
            let left = bin_points[m];
            let center = bin_points[m + 1];
            let right = bin_points[m + 2];

            let mut filter = Vec::new();
            // Rising slope
            if center > left {
                for k in left..=center {
                    if k < num_bins {
                        let w = (k - left) as f32 / (center - left) as f32;
                        if w > 0.0 {
                            filter.push((k, w));
                        }
                    }
                }
            }
            // Falling slope
            if right > center {
                for k in (center + 1)..=right {
                    if k < num_bins {
                        let w = (right - k) as f32 / (right - center) as f32;
                        if w > 0.0 {
                            filter.push((k, w));
                        }
                    }
                }
            }
            filters.push(filter);
        }
        filters
    }

    /// Feed new samples and compute all features.
    pub fn analyze(&mut self, samples: &[f32]) -> AudioFeatures {
        // Shift time-domain buffer left, append new samples
        let shift = samples.len().min(FFT_LARGE);
        if shift < FFT_LARGE {
            self.time_domain.copy_within(shift.., 0);
        }
        let src_offset = if samples.len() > FFT_LARGE {
            samples.len() - FFT_LARGE
        } else {
            0
        };
        self.time_domain[FFT_LARGE - shift..]
            .copy_from_slice(&samples[src_offset..src_offset + shift]);

        // Run all three FFTs
        self.large.compute(&self.time_domain);
        self.medium.compute(&self.time_domain);
        self.small.compute(&self.time_domain);

        self.extract_features()
    }

    /// Expose the large (4096-pt) magnitude spectrum for beat detection.
    pub fn bass_magnitude(&self) -> &[f32] {
        &self.large.magnitude
    }

    /// Expose the medium (1024-pt) magnitude spectrum for beat detection.
    pub fn mid_magnitude(&self) -> &[f32] {
        &self.medium.magnitude
    }

    /// Expose the small (512-pt) magnitude spectrum for beat detection.
    pub fn high_magnitude(&self) -> &[f32] {
        &self.small.magnitude
    }

    /// Expose the raw (un-windowed) 4096-sample time-domain window for A15 pitch (#1466). YIN wants
    /// the un-windowed samples — the Hann window is applied only inside each `FftResolution::compute`.
    pub fn time_domain(&self) -> &[f32] {
        &self.time_domain
    }

    /// Per-band half-wave-rectified spectral flux for the A12 downbeat tracker (#1463):
    /// low (20-150 Hz), mid (150-2000 Hz), high (2000-20000 Hz), each from the resolution
    /// that best covers it. Reuses the same `spectral_flux_range` the kick detector uses;
    /// read-only (does not touch `prev_magnitude`), so it is safe to call once per frame
    /// alongside feature extraction without disturbing the kick flux.
    pub fn band_flux_3(&self) -> [f32; 3] {
        [
            self.large.spectral_flux_range(20.0, 150.0),
            self.medium.spectral_flux_range(150.0, 2000.0),
            self.small.spectral_flux_range(2000.0, 20000.0),
        ]
    }

    /// The kick envelope for this hop (#3624): the ensemble's probability, scored in
    /// `extract_features`, mapped so 0.5 means a kick. Gated on the A10 perceptual silence
    /// flag, so the noise floor can't read as a kick. Call once per hop after silence is
    /// known.
    pub fn kick_envelope(&mut self, loud_silent: bool) -> f32 {
        if loud_silent {
            return 0.0;
        }
        kick_from_probability(self.kick_probability)
    }

    /// A17 (#1468): log-frequency-resampled magnitude spectrum for the `audio_spectrum`
    /// texture (R16Float 512x1). Each output bin takes the peak magnitude in its
    /// log-spaced frequency slice of the large (4096-pt) spectrum, dB-normalized to 0..1
    /// (−80..0 dB → 0..1, matching `band_energy_db`). Peak (not mean) keeps narrow tones
    /// visible in the bars.
    pub fn log_spectrum_512(&self) -> [f32; SPECTRUM_BINS] {
        let mag = &self.large.magnitude;
        let bin_hz = self.large.bin_hz;
        let nyquist = self.sample_rate * 0.5;
        let lo_hz = 30.0f32;
        let hi_hz = nyquist.max(lo_hz * 2.0);
        let ratio = (hi_hz / lo_hz).ln();

        let mut out = [0.0f32; SPECTRUM_BINS];
        for (j, o) in out.iter_mut().enumerate() {
            // Log-spaced frequency edges for this output bin.
            let f0 = lo_hz * (ratio * j as f32 / SPECTRUM_BINS as f32).exp();
            let f1 = lo_hz * (ratio * (j + 1) as f32 / SPECTRUM_BINS as f32).exp();
            let k0 = (f0 / bin_hz).floor() as usize;
            let k1 = ((f1 / bin_hz).ceil() as usize).max(k0 + 1).min(mag.len());
            let k0 = k0.min(mag.len().saturating_sub(1));

            let mut peak = 0.0f32;
            for &m in &mag[k0..k1] {
                if m > peak {
                    peak = m;
                }
            }
            *o = amp_to_db01(peak);
        }
        out
    }

    /// A17 (#1468): one column of the scrolling mel-spectrogram for the
    /// `audio_spectrogram` texture (R8Unorm width=frames × height=SPECTROGRAM_MELS).
    /// Applies the dedicated 64-band mel filterbank to the large magnitude spectrum and
    /// dB-normalizes each band to 0..1.
    pub fn spectrogram_column(&self) -> [f32; SPECTROGRAM_MELS] {
        let mag = &self.large.magnitude;
        let mut out = [0.0f32; SPECTROGRAM_MELS];
        for (m, filter) in self.spectrogram_mel.iter().enumerate() {
            // Weighted power sum over the triangular filter, back to an amplitude.
            let mut power = 0.0f32;
            for &(k, w) in filter {
                power += mag[k] * mag[k] * w;
            }
            out[m] = amp_to_db01(power.sqrt());
        }
        out
    }

    /// The 7 band energies `[sub_bass, bass, low_mid, mid, upper_mid, presence, brilliance]`,
    /// scaled per `band_scale` (A1 #1452). Each band keeps the FFT resolution best suited to
    /// its range (large for the two lowest, medium for the three mids, small for the two
    /// highs). `Legacy` reproduces the pre-A1 split (low four linear RMS, high three
    /// dB(−80..0)); `Db` puts all seven in one dB(−60..0) domain with a +3 dB/oct
    /// equal-loudness tilt above 2 kHz, so the adaptive normalizer sees one comparable family.
    fn bands(&self) -> [f32; 7] {
        match self.band_scale {
            BandScale::Legacy => [
                self.large.band_energy_linear(20.0, 60.0),
                self.large.band_energy_linear(60.0, 250.0),
                self.medium.band_energy_linear(250.0, 500.0),
                self.medium.band_energy_linear(500.0, 2000.0),
                self.medium.band_energy_db(2000.0, 4000.0),
                self.small.band_energy_db(4000.0, 6000.0),
                self.small.band_energy_db(6000.0, 20000.0),
            ],
            BandScale::Db => {
                const FLOOR: f32 = -60.0;
                // +3 dB/oct above 2 kHz, keyed on the band's geometric-centre frequency.
                let tilt = |lo: f32, hi: f32| {
                    let centre = (lo * hi).sqrt();
                    if centre > 2000.0 {
                        3.0 * (centre / 2000.0).log2()
                    } else {
                        0.0
                    }
                };
                let db = |res: &FftResolution, lo: f32, hi: f32| {
                    res.band_energy_db_window(lo, hi, FLOOR, tilt(lo, hi))
                };
                [
                    db(&self.large, 20.0, 60.0),
                    db(&self.large, 60.0, 250.0),
                    db(&self.medium, 250.0, 500.0),
                    db(&self.medium, 500.0, 2000.0),
                    db(&self.medium, 2000.0, 4000.0),
                    db(&self.small, 4000.0, 6000.0),
                    db(&self.small, 6000.0, 20000.0),
                ]
            }
        }
    }

    fn extract_features(&mut self) -> AudioFeatures {
        // RMS from time domain (use last 2048 samples for reasonable window)
        let td_start = FFT_LARGE - 2048;
        let sum_sq: f32 = self.time_domain[td_start..].iter().map(|s| s * s).sum();
        let rms = (sum_sq / 2048.0).sqrt();

        // Kick (#3624): score this hop now, every hop, so the detector's history stays
        // aligned; the silence gate and the mapping onto `kick` run in `kick_envelope`.
        // `kick` is left 0 in the struct below and filled from `kick_envelope`.
        self.kick_probability = self.kick_detector.process(
            &self.large.magnitude,
            &self.large.prev_magnitude,
            &self.medium.magnitude,
            &self.medium.prev_magnitude,
            FLUX_FLOOR,
        );

        // Spectral features (from large FFT for best frequency resolution). Centroid and
        // bandwidth are the mean and spread of the same power-weighted log2-frequency
        // distribution (A4 #1455, #62).
        let (centroid, bandwidth) = self.spectral_shape();

        let [
            sub_bass,
            bass,
            low_mid,
            mid,
            upper_mid,
            presence,
            brilliance,
        ] = self.bands();

        // Shared by flatness and the MFCCs (#76).
        let mel = self.mel_energies();

        let mut out = AudioFeatures {
            // 7-band energy extraction (A1 #1452: scaling per `band_scale`).
            sub_bass,
            bass,
            low_mid,
            mid,
            upper_mid,
            presence,
            brilliance,
            rms,
            kick: 0.0, // A3 (#1454): filled by `kick_envelope` after the silence gate
            centroid,
            flux: self.spectral_flux(),
            flatness: spectral_flatness(&mel),
            rolloff: log_freq_01(self.spectral_rolloff()),
            bandwidth,
            // A zero-crossing rate is the frequency of the sine that would cross as often:
            // f = rate · sr / 2. On the centroid's axis it spans 0..1 and no longer depends
            // on the sample rate (#62).
            zcr: log_freq_01(self.zero_crossing_rate() * self.sample_rate * 0.5),
            ..Default::default()
        };

        // MFCC extraction (from large FFT magnitude)
        self.compute_mfccs(&mel, &mut out);

        // A11 (#1462): CQT-lite constant-Q chroma (also advances tuning estimation).
        // The visual form fills the feature frame; the pure-fold energy form is held
        // for the key detector's per-hop read (#2079), like `kick`/`band_flux_3`.
        let chroma_frame = self.cqt.compute(&self.large.magnitude);
        out.chroma = chroma_frame.chroma;
        self.key_chroma = chroma_frame.e12;
        self.key_bass = chroma_frame.bass;

        // Dominant chroma: argmax of chroma bins, normalized to 0-1
        let mut max_idx = 0usize;
        let mut max_val = out.chroma[0];
        for i in 1..N_CHROMA {
            if out.chroma[i] > max_val {
                max_val = out.chroma[i];
                max_idx = i;
            }
        }
        out.dominant_chroma = max_idx as f32 / 11.0;

        // Beat fields left at 0.0 — filled by beat detector in audio thread
        out
    }

    /// The 26 mel-band **power** energies from the large magnitude spectrum. Shared by the
    /// MFCC path and A4's mel-band flatness (#1455), so both see the same filterbank;
    /// `extract_features` computes it once per hop and hands it to both (#76).
    fn mel_energies(&self) -> [f32; N_MELS] {
        let mag = &self.large.magnitude;
        let mut mel = [0.0f32; N_MELS];
        for (m, filter) in self.mel_filters.iter().enumerate() {
            let mut energy = 0.0f32;
            for &(k, w) in filter {
                energy += mag[k] * mag[k] * w;
            }
            mel[m] = energy;
        }
        mel
    }

    /// Compute 13 MFCCs from this hop's mel-band power energies.
    fn compute_mfccs(&self, mel: &[f32; N_MELS], out: &mut AudioFeatures) {
        // Log compression
        let mel_energies = mel.map(|e| (e + 1e-10).ln());

        // DCT-II → 13 MFCCs
        for i in 0..N_MFCC {
            let mut sum = 0.0f32;
            for j in 0..N_MELS {
                sum += self.dct_matrix[i][j] * mel_energies[j];
            }
            out.mfcc[i] = sum;
        }
    }

    /// The `(centroid, bandwidth)` features: mean and spread of the power-weighted
    /// distribution of **log2(frequency)**, skipping DC.
    ///
    /// A4 (#1455): the centroid is that mean mapped onto 0..1 across
    /// `CENTROID_F_MIN..CENTROID_F_MAX`. On a log axis it stops living in the top octave
    /// and becomes a usable brightness fader; the FixedRange policy (A2) holds it steady
    /// below the silence gate. #62: the bandwidth is the standard deviation around that
    /// same centroid, in octaves over [`BANDWIDTH_OCTAVES`]. It used to be a
    /// magnitude-weighted spread in Hz (DC included) around a separate linear centroid,
    /// divided by Nyquist, so it measured neither this centroid's spread nor more than a
    /// sliver of 0..1.
    fn spectral_shape(&self) -> (f32, f32) {
        let mag = &self.large.magnitude;
        let mut power_sum = 0.0f64;
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        for (&m, &log2_hz) in mag.iter().zip(&self.log2_bin_hz).skip(1) {
            let p = f64::from(m * m);
            let x = f64::from(log2_hz);
            power_sum += p;
            sum += x * p;
            sum_sq += x * x * p;
        }
        if power_sum <= 1e-12 {
            return (0.0, 0.0);
        }
        let mean = sum / power_sum;
        let spread = (sum_sq / power_sum - mean * mean).max(0.0).sqrt() as f32;
        let lo = CENTROID_F_MIN.log2();
        let hi = CENTROID_F_MAX.log2();
        let centroid = ((mean as f32 - lo) / (hi - lo)).clamp(0.0, 1.0);
        (centroid, (spread / BANDWIDTH_OCTAVES).min(1.0))
    }

    /// A4 (#1455): spectral flux as a **level-invariant** rate of change — half-wave
    /// rectified per-bin log-magnitude difference (skipping DC), averaged over bins. The
    /// old version summed linear-magnitude differences over the whole spectrum, so it
    /// doubled when the volume doubled (a second RMS); this measures *change*, not level,
    /// and the Adaptive percentile policy (A2) ranges it downstream.
    fn spectral_flux(&self) -> f32 {
        let mag = &self.large.magnitude;
        let prev = &self.large.prev_magnitude;
        let n = mag.len();
        if n <= 1 {
            return 0.0;
        }
        let mut flux = 0.0f32;
        for i in 1..n {
            let diff = mag[i].max(FLUX_FLOOR).ln() - prev[i].max(FLUX_FLOOR).ln();
            if diff > 0.0 {
                flux += diff;
            }
        }
        flux / (n - 1) as f32
    }

    /// A16 (#1467): spectral contrast — per-octave peak-vs-valley tonality (Jiang 2002 /
    /// librosa). For each of six octave bands (200-400, 400-800, … 6400-Nyquist Hz) on the large
    /// (4096-pt) magnitude, `contrast = dB(mean top 2%) − dB(mean bottom 2%)`, mapped 0-60 dB →
    /// 0..1: a sharp harmonic (sine, voiced vowel) reads high, flat noise reads low. Returns
    /// `[contrast_0..5, contrast_mean]`.
    ///
    /// The large spectrum (10.8 Hz/bin) is used for every band so even the 200-400 Hz octave has
    /// ~18 bins to take a 2% quantile over (the medium spectrum gives only ~4). Contrast is a
    /// tonality measure, not a transient, so the 93 ms window is fine.
    ///
    /// `loud_silent` (A10) returns all-zero: the fields are Passthrough — the normalizer won't gate
    /// them, so the producer must, since the noise floor has its own spurious peak/valley structure
    /// (mirrors A13/A14 self-gating).
    pub fn spectral_contrast(&mut self, loud_silent: bool) -> [f32; 7] {
        if loud_silent {
            return [0.0; 7];
        }
        const LO_HZ: [f32; 6] = [200.0, 400.0, 800.0, 1600.0, 3200.0, 6400.0];
        let nyquist = self.sample_rate * 0.5;
        let mut out = [0.0f32; 7];
        let mut sum = 0.0f32;
        for b in 0..6 {
            let hi_hz = if b < 5 { LO_HZ[b + 1] } else { nyquist };
            let (lo, hi) = self.large.bin_range(LO_HZ[b], hi_hz);
            let c = band_contrast(&self.large.magnitude[lo..hi], &mut self.contrast_scratch);
            out[b] = c;
            sum += c;
        }
        out[6] = sum / 6.0;
        out
    }

    /// A4 (#1455): frequency below which `ROLLOFF_PERCENTILE` of the spectral power lies
    /// (skipping DC; percentage now a named const rather than a hardcoded 0.85).
    fn spectral_rolloff(&self) -> f32 {
        let mag = &self.large.magnitude;
        let bin_hz = self.large.bin_hz;
        let total_energy: f32 = mag.iter().skip(1).map(|m| m * m).sum();
        if total_energy < 1e-12 {
            return 0.0;
        }
        let threshold = total_energy * ROLLOFF_PERCENTILE;
        let mut cumulative = 0.0f32;
        for (i, &m) in mag.iter().enumerate().skip(1) {
            cumulative += m * m;
            if cumulative >= threshold {
                return i as f32 * bin_hz;
            }
        }
        (mag.len() - 1) as f32 * bin_hz
    }

    fn zero_crossing_rate(&self) -> f32 {
        let td_start = FFT_LARGE - 2048;
        let td = &self.time_domain[td_start..];
        let mut crossings = 0u32;
        for i in 1..td.len() {
            if (td[i] >= 0.0) != (td[i - 1] >= 0.0) {
                crossings += 1;
            }
        }
        crossings as f32 / (td.len() - 1) as f32
    }
}

/// `hz` on the centroid's log2 axis: `CENTROID_F_MIN..CENTROID_F_MAX` → 0..1, clamped
/// (0 Hz reads 0).
fn log_freq_01(hz: f32) -> f32 {
    if hz <= 0.0 {
        return 0.0;
    }
    let lo = CENTROID_F_MIN.log2();
    let hi = CENTROID_F_MAX.log2();
    ((hz.log2() - lo) / (hi - lo)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;

    /// Feed a pure sine of `freq` Hz through the analyzer for a few FFT windows.
    fn analyze_sine(analyzer: &mut FftAnalyzer, freq: f32) {
        let mut phase = 0.0f32;
        let step = 2.0 * std::f32::consts::PI * freq / SR;
        // Several FFT_LARGE-sized blocks so the sliding time-domain buffer fills fully.
        for _ in 0..4 {
            let block: Vec<f32> = (0..FFT_LARGE)
                .map(|_| {
                    let s = phase.sin();
                    phase += step;
                    s
                })
                .collect();
            analyzer.analyze(&block);
        }
    }

    /// Feed a sine and return the resulting features (final of a few windows).
    fn features_for_sine(band_scale: BandScale, freq: f32) -> AudioFeatures {
        let mut a = FftAnalyzer::new(SR, band_scale);
        let mut phase = 0.0f32;
        let step = 2.0 * std::f32::consts::PI * freq / SR;
        let mut feats = AudioFeatures::default();
        for _ in 0..5 {
            let block: Vec<f32> = (0..FFT_LARGE)
                .map(|_| {
                    let s = 0.5 * phase.sin();
                    phase += step;
                    s
                })
                .collect();
            feats = a.analyze(&block);
        }
        feats
    }

    #[test]
    fn band_scale_db_vs_legacy_differ_and_bounded() {
        // A 40 Hz tone lands in sub_bass. Both scalings must stay in 0..1, and unified dB
        // (A1 #1452) must differ from the legacy linear-RMS scaling for that low band.
        let db = features_for_sine(BandScale::Db, 40.0);
        let legacy = features_for_sine(BandScale::Legacy, 40.0);
        for v in [
            db.sub_bass,
            db.bass,
            db.low_mid,
            db.mid,
            db.upper_mid,
            db.presence,
            db.brilliance,
        ] {
            assert!((0.0..=1.0).contains(&v), "dB band out of range: {v}");
        }
        assert!(
            db.sub_bass > 0.0,
            "40 Hz tone should light sub_bass in dB mode"
        );
        assert!(
            (db.sub_bass - legacy.sub_bass).abs() > 1e-3,
            "dB ({}) and legacy ({}) sub_bass should differ",
            db.sub_bass,
            legacy.sub_bass
        );
    }

    #[test]
    fn log_spectrum_512_shape_and_bounds() {
        let mut a = FftAnalyzer::new(SR, BandScale::Db);
        analyze_sine(&mut a, 1000.0);
        let spec = a.log_spectrum_512();
        assert_eq!(spec.len(), SPECTRUM_BINS);
        assert!(
            spec.iter().all(|&v| (0.0..=1.0).contains(&v)),
            "spectrum values must be normalized to 0..1"
        );
        // A 1 kHz tone should light up at least one bin above silence.
        assert!(
            spec.iter().cloned().fold(0.0f32, f32::max) > 0.1,
            "1 kHz tone should produce a visible spectrum peak"
        );
    }

    #[test]
    fn spectrogram_column_shape_and_bounds() {
        let mut a = FftAnalyzer::new(SR, BandScale::Db);
        analyze_sine(&mut a, 440.0);
        let col = a.spectrogram_column();
        assert_eq!(col.len(), SPECTROGRAM_MELS);
        assert!(
            col.iter().all(|&v| (0.0..=1.0).contains(&v)),
            "mel-column values must be normalized to 0..1"
        );
        assert!(
            col.iter().cloned().fold(0.0f32, f32::max) > 0.1,
            "440 Hz tone should light a mel band"
        );
    }

    #[test]
    fn silence_produces_zero_textures() {
        let a = FftAnalyzer::new(SR, BandScale::Db);
        // No samples fed: magnitude is all zero → both textures read 0.0.
        assert!(a.log_spectrum_512().iter().all(|&v| v == 0.0));
        assert!(a.spectrogram_column().iter().all(|&v| v == 0.0));
    }

    #[test]
    fn mfcc_filterbank_unchanged_size() {
        // MFCC path still uses the 26-band bank; the spectrogram bank is separate.
        let a = FftAnalyzer::new(SR, BandScale::Db);
        assert_eq!(a.mel_filters.len(), N_MELS);
        assert_eq!(a.spectrogram_mel.len(), SPECTROGRAM_MELS);
    }

    /// A11 (#1462): real audio → CQT chroma → key detection, end to end. A C-major
    /// triad (C4/E4/G4) must light exactly those three pitch classes and resolve to
    /// C major.
    #[test]
    fn c_major_triad_chroma_and_key() {
        use super::super::key::KeyDetector;

        let mut a = FftAnalyzer::new(SR, BandScale::Db);
        let freqs = [261.63f32, 329.63, 392.00]; // C4, E4, G4
        let mut phase = [0.0f32; 3];
        let mut feats = AudioFeatures::default();
        for _ in 0..8 {
            let block: Vec<f32> = (0..FFT_LARGE)
                .map(|_| {
                    let mut s = 0.0f32;
                    for (p, &f) in phase.iter_mut().zip(&freqs) {
                        s += p.sin();
                        *p += 2.0 * std::f32::consts::PI * f / SR;
                    }
                    s / 3.0
                })
                .collect();
            feats = a.analyze(&block);
        }

        // The three chord tones (C=0, E=4, G=7) must be the top three pitch classes.
        let c = feats.chroma;
        let mut ranked: Vec<usize> = (0..12).collect();
        ranked.sort_by(|&x, &y| c[y].partial_cmp(&c[x]).unwrap());
        let mut top3 = ranked[..3].to_vec();
        top3.sort_unstable();
        assert_eq!(
            top3,
            vec![0, 4, 7],
            "top-3 chroma should be C/E/G; chroma={c:?}"
        );

        // The key-path chroma (pure fold, unnormalized — what hop.rs feeds) should
        // drive the key detector to C major.
        let kc = *a.key_chroma();
        let mut det = KeyDetector::new(SR);
        let mut r = det.process(&kc, 0.01);
        for _ in 0..4000 {
            r = det.process(&kc, 0.01);
        }
        assert_eq!(r.key_class, 0.0, "expected C tonic; chroma={c:?}");
        assert_eq!(r.is_minor, 0.0, "C-major triad should read major");
        assert!(r.confidence > 0.7, "confidence {}", r.confidence);
    }

    /// Feed one FFT_LARGE block of a `freq` sine at `amp`, advancing `phase`, then read the
    /// kick envelope with the given silence flag.
    fn feed_kick_block(
        a: &mut FftAnalyzer,
        freq: f32,
        amp: f32,
        phase: &mut f32,
        loud_silent: bool,
    ) -> f32 {
        let step = 2.0 * std::f32::consts::PI * freq / SR;
        let block: Vec<f32> = (0..FFT_LARGE)
            .map(|_| {
                let s = amp * phase.sin();
                *phase += step;
                s
            })
            .collect();
        a.analyze(&block);
        a.kick_envelope(loud_silent)
    }

    /// The perceptual silence gate forces the kick to 0 whatever the detector scores, so
    /// the noise floor can't read as a kick.
    #[test]
    fn kick_gated_to_zero_on_silence() {
        let mut a = FftAnalyzer::new(SR, BandScale::Db);
        let mut phase = 0.0f32;
        assert_eq!(feed_kick_block(&mut a, 60.0, 0.8, &mut phase, true), 0.0);
        assert_eq!(feed_kick_block(&mut a, 60.0, 0.8, &mut phase, true), 0.0);
    }

    /// A synthetic kick drum: a 155 -> 45 Hz pitch drop decaying over ~180 ms, plus a 4 ms
    /// click (a deterministic 4 -> 1 kHz chirp stands in for the beater noise).
    fn synth_kick(n: usize) -> Vec<f32> {
        let (mut body_phase, mut click_phase) = (0.0f32, 0.0f32);
        (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                body_phase += 2.0 * std::f32::consts::PI * (45.0 + 110.0 * (-t / 0.03).exp()) / SR;
                click_phase +=
                    2.0 * std::f32::consts::PI * (1000.0 + 3000.0 * (-t / 0.004).exp()) / SR;
                0.8 * (body_phase.sin() * (-t / 0.18).exp()
                    + 0.3 * click_phase.sin() * (-t / 0.004).exp())
            })
            .collect()
    }

    /// A bass note: 10 ms attack into a sustained fundamental plus second harmonic.
    fn synth_bass_note(n: usize, freq: f32) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / SR;
                let w = 2.0 * std::f32::consts::PI * freq * t;
                0.6 * (t / 0.01).min(1.0) * (w.sin() + 0.3 * (2.0 * w).sin())
            })
            .collect()
    }

    /// Feed `signal` one 512-sample hop at a time, as the audio thread does, and count the
    /// kicks a consumer would see through the 0.5 / 0.3 hysteresis.
    fn count_kicks(signal: &[f32]) -> usize {
        let mut a = FftAnalyzer::new(SR, BandScale::Db);
        let (mut armed, mut kicks) = (true, 0);
        for hop in signal.chunks_exact(512) {
            a.analyze(hop);
            let k = a.kick_envelope(false);
            if k < 0.3 {
                armed = true;
            } else if armed && k >= 0.5 {
                armed = false;
                kicks += 1;
            }
        }
        kicks
    }

    /// Sixteen events half a second apart, each `make(i)` samples long, after 0.5 s quiet.
    fn pattern(make: impl Fn(usize, usize) -> Vec<f32>) -> Vec<f32> {
        let period = (0.5 * SR) as usize;
        let mut x = vec![0.0f32; period * 17];
        for i in 0..16 {
            let start = period * (i + 1);
            x[start..start + period].copy_from_slice(&make(i, period));
        }
        x
    }

    /// #3624: the kick fires on kick drums and not on bass notes. The old 30-120 Hz flux
    /// fired on both (a bass note is a rise in that band too), which is the bug.
    #[test]
    fn kick_fires_on_kick_drums_not_on_bass_notes() {
        let drums = count_kicks(&pattern(|_, n| synth_kick(n)));
        let notes = [55.0, 41.2, 49.0, 61.7];
        let bass = count_kicks(&pattern(|i, n| synth_bass_note(n, notes[i % 4])));
        assert!(
            drums >= 13,
            "16 kick drums should read as kicks, got {drums}"
        );
        assert!(
            bass <= 1,
            "16 bass notes must not read as kicks, got {bass}"
        );
    }

    /// A4 (#1455): the centroid feature tracks brightness — a high tone reads much higher
    /// than a low tone — on its log2 axis, and stays in 0..1.
    #[test]
    fn centroid_rises_with_frequency() {
        let low = features_for_sine(BandScale::Db, 200.0).centroid;
        let high = features_for_sine(BandScale::Db, 6000.0).centroid;
        assert!((0.0..=1.0).contains(&low) && (0.0..=1.0).contains(&high));
        assert!(
            high > low + 0.2,
            "centroid should track brightness: low={low}, high={high}"
        );
    }

    /// A4 (#1455): flux measures *change*, not level — a steady tone reads ~0 flux at any
    /// amplitude (the old linear-sum flux scaled with volume, a second RMS).
    #[test]
    fn flux_measures_change_not_level() {
        fn steady_flux(freq: f32, amp: f32) -> f32 {
            let mut a = FftAnalyzer::new(SR, BandScale::Db);
            let mut phase = 0.0f32;
            let step = 2.0 * std::f32::consts::PI * freq / SR;
            let mut flux = 0.0;
            for _ in 0..6 {
                let block: Vec<f32> = (0..FFT_LARGE)
                    .map(|_| {
                        let s = amp * phase.sin();
                        phase += step;
                        s
                    })
                    .collect();
                flux = a.analyze(&block).flux;
            }
            flux
        }
        let quiet = steady_flux(1000.0, 0.2);
        let loud = steady_flux(1000.0, 0.9);
        assert!(
            quiet < 0.02 && loud < 0.02,
            "steady flux should be ~0: quiet={quiet}, loud={loud}"
        );
        assert!(
            (quiet - loud).abs() < 0.02,
            "flux must not scale with level: {quiet} vs {loud}"
        );
    }

    /// A4 (#1455): mel-band Wiener-entropy flatness separates a tone (low) from white
    /// noise (high).
    #[test]
    fn flatness_separates_tone_from_noise() {
        let tone = features_for_sine(BandScale::Db, 1000.0).flatness;
        let mut a = FftAnalyzer::new(SR, BandScale::Db);
        let mut state: u32 = 0x1234_5678;
        let mut feats = AudioFeatures::default();
        for _ in 0..6 {
            let block: Vec<f32> = (0..FFT_LARGE)
                .map(|_| {
                    // Deterministic LCG white noise in −1..1.
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
                })
                .collect();
            feats = a.analyze(&block);
        }
        let noise = feats.flatness;
        assert!((0.0..=1.0).contains(&tone) && (0.0..=1.0).contains(&noise));
        assert!(
            noise > tone + 0.2,
            "noise flatness ({noise}) should exceed tone flatness ({tone})"
        );
    }

    /// A4 (#1455): rolloff rises with the frequency content, stays in 0..1.
    #[test]
    fn rolloff_rises_with_frequency() {
        let low = features_for_sine(BandScale::Db, 400.0).rolloff;
        let high = features_for_sine(BandScale::Db, 8000.0).rolloff;
        assert!((0.0..=1.0).contains(&low) && (0.0..=1.0).contains(&high));
        assert!(
            high > low,
            "rolloff should rise with tone frequency: low={low}, high={high}"
        );
    }

    /// #62: rolloff, zcr and the centroid share one log-frequency axis, so for a pure tone
    /// all three read the tone's own position on it, and an octave is the same step anywhere.
    #[test]
    fn rolloff_zcr_and_centroid_share_the_log_axis() {
        for hz in [220.0, 1000.0, 5000.0] {
            let f = features_for_sine(BandScale::Db, hz);
            let expect = log_freq_01(hz);
            for (name, v) in [
                ("rolloff", f.rolloff),
                ("zcr", f.zcr),
                ("centroid", f.centroid),
            ] {
                assert!((v - expect).abs() < 0.02, "{hz} Hz {name}: {v} vs {expect}");
            }
        }
        let octave = log_freq_01(880.0) - log_freq_01(440.0);
        assert!((octave - (log_freq_01(8000.0) - log_freq_01(4000.0))).abs() < 1e-5);
    }

    /// #62: bandwidth is the spread around the reported centroid. A pure tone has none;
    /// two tones three octaves apart at equal power sit 1.5 octaves either side of it.
    #[test]
    fn bandwidth_is_the_octave_spread_around_the_centroid() {
        assert!(features_for_sine(BandScale::Db, 1000.0).bandwidth < 0.02);

        let mut a = FftAnalyzer::new(SR, BandScale::Db);
        let tau = std::f32::consts::TAU;
        let mut f = AudioFeatures::default();
        for blk in 0..5 {
            let block: Vec<f32> = (0..FFT_LARGE)
                .map(|i| {
                    let t = (blk * FFT_LARGE + i) as f32 / SR;
                    0.3 * (tau * 250.0 * t).sin() + 0.3 * (tau * 2000.0 * t).sin()
                })
                .collect();
            f = a.analyze(&block);
        }
        let octaves = f.bandwidth * BANDWIDTH_OCTAVES;
        assert!((octaves - 1.5).abs() < 0.1, "spread {octaves} octaves");
        let mid = log_freq_01((250.0f32 * 2000.0).sqrt());
        assert!(
            (f.centroid - mid).abs() < 0.02,
            "centroid {} vs {mid}",
            f.centroid
        );
    }

    /// A16 (#1467): spectral contrast reads high for a sharp harmonic (a tonal peak over a quiet
    /// floor) and lower for broadband noise, in the band carrying the energy. Every band stays 0..1.
    #[test]
    fn spectral_contrast_tone_vs_noise() {
        // A 1000 Hz sine lands in band 2 (800-1600 Hz).
        let mut a = FftAnalyzer::new(SR, BandScale::Db);
        analyze_sine(&mut a, 1000.0);
        let tone = a.spectral_contrast(false);
        for (b, &c) in tone.iter().enumerate() {
            assert!((0.0..=1.0).contains(&c), "contrast[{b}] out of range: {c}");
        }

        let mut n = FftAnalyzer::new(SR, BandScale::Db);
        let mut state: u32 = 0x9e37_79b9;
        for _ in 0..4 {
            let block: Vec<f32> = (0..FFT_LARGE)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (state >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
                })
                .collect();
            n.analyze(&block);
        }
        let noise = n.spectral_contrast(false);
        assert!(
            tone[2] > 0.5,
            "a sharp tonal peak should read high contrast, got {}",
            tone[2]
        );
        assert!(
            tone[2] > noise[2],
            "tone contrast ({}) should exceed noise contrast ({}) in the shared band",
            tone[2],
            noise[2]
        );
    }

    /// A16 (#1467): the perceptual-silence gate zeroes every contrast band — they are Passthrough,
    /// so the normalizer won't gate them and the producer must.
    #[test]
    fn spectral_contrast_silence_gate_is_zero() {
        let mut a = FftAnalyzer::new(SR, BandScale::Db);
        analyze_sine(&mut a, 1000.0);
        assert_eq!(a.spectral_contrast(true), [0.0; 7]);
    }
}
