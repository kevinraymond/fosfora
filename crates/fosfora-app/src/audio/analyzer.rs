use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

use super::chroma::CqtChroma;
use super::features::AudioFeatures;
use super::ranging::PercentileWindow;
use crate::settings::BandScale;

/// FFT sizes for multi-resolution analysis.
const FFT_LARGE: usize = 4096; // 10.8 Hz/bin — sub_bass, bass, kick
const FFT_MED: usize = 1024; // 43 Hz/bin — low_mid, mid, upper_mid
const FFT_SMALL: usize = 512; // 86 Hz/bin — presence, brilliance

/// MFCC / chroma constants
const N_MELS: usize = 26;
const N_MFCC: usize = 13;
const N_CHROMA: usize = 12;

/// A3 (#1454): the kick envelope normalizes its log-flux against this many recent frames'
/// P95 (~10 s at the fixed 512-sample hop / 44.1 kHz). Long enough that the P95 tracks the
/// prevailing kick level rather than a single hit, and it freezes across silence (the
/// perceptual gate skips the push), so a kick after a quiet passage isn't over-scaled.
const KICK_WINDOW: usize = 860;

/// A4 (#1455): the `centroid` feature maps a power-weighted mean of log2(frequency) onto
/// 0..1 across this musical range, so it reads as a perceptual brightness fader instead
/// of hugging the top octave on a linear-Hz axis.
const CENTROID_F_MIN: f32 = 40.0;
const CENTROID_F_MAX: f32 = 18000.0;

/// A4 (#1455): fraction of spectral energy below the rolloff frequency. Configurable
/// (was a hardcoded 0.85); can be promoted to a user setting later with no ABI impact.
const ROLLOFF_PERCENTILE: f32 = 0.85;

/// A4 (#1455): magnitude floor (~−60 dB below a unit tone) applied before the log in the
/// spectral-flux feature. Per-bin log magnitude is wildly unstable near the noise floor —
/// without a floor, sub-signal bins dominate the flux and it tracks level again. Clamping
/// them to a common floor makes their frame-to-frame diff zero, so flux reads change only.
const FLUX_FLOOR: f32 = 1e-3;

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

        Self {
            fft,
            size,
            window,
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
        let scale = 2.0 / self.size as f32;
        for i in 0..self.num_bins {
            self.magnitude[i] = self.fft_buffer[i].norm() * scale;
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

    /// Half-wave rectified spectral flux in a frequency range, on **log** magnitude and
    /// per-bin-mean (A3 #1454). Same level-invariant form as the SuperFlux onset detector:
    /// a bass *change* registers regardless of absolute level, so the kick no longer
    /// saturates on loud material or vanishes on quiet material. Magnitudes are floored
    /// (`FLUX_FLOOR`) before the log so sub-signal bins — e.g. the near-silent 30–120 Hz
    /// bins under a bassless lead — read a constant zero instead of jittering into false
    /// kicks (a real, loud kick sits well above the floor and is unaffected).
    fn spectral_flux_range_log(&self, lo_hz: f32, hi_hz: f32) -> f32 {
        let (lo, hi) = self.bin_range(lo_hz, hi_hz);
        let hi = hi.min(self.num_bins);
        let count = hi.saturating_sub(lo).max(1);
        let mut flux = 0.0f32;
        for i in lo..hi {
            let diff = self.magnitude[i].max(FLUX_FLOOR).ln()
                - self.prev_magnitude[i].max(FLUX_FLOOR).ln();
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

/// Multi-resolution FFT analyzer with 7 frequency bands and spectral features.
pub struct FftAnalyzer {
    large: FftResolution,  // 4096-pt for bass
    medium: FftResolution, // 1024-pt for mids
    small: FftResolution,  // 512-pt for highs

    time_domain: Vec<f32>, // Shared sample accumulator (FFT_LARGE length)
    sample_rate: f32,

    // A1 (#1452): how the 7 bands are scaled (unified dB vs legacy linear/dB split).
    band_scale: BandScale,

    // A3 (#1454): kick detection. `kick_flux` is the latest 30-120 Hz log-flux from
    // `extract_features`; `kick_window` is the single detector-owned P95 normalizer,
    // applied (and gated) by `kick_envelope` once the perceptual silence flag is known.
    kick_flux: f32,
    kick_window: PercentileWindow,

    // MFCC precomputed data
    mel_filters: MelFilter,              // N_MELS sparse triangular filters
    dct_matrix: [[f32; N_MELS]; N_MFCC], // DCT-II coefficients

    // A17 spectrogram filterbank: SPECTROGRAM_MELS sparse triangular filters (#1468)
    spectrogram_mel: MelFilter,

    // A11 (#1462): CQT-lite constant-Q chroma with tuning compensation.
    cqt: CqtChroma,
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
        let cqt = CqtChroma::new(large.num_bins, large.bin_hz);

        Self {
            large,
            medium,
            small,
            time_domain: vec![0.0; FFT_LARGE],
            sample_rate,
            band_scale,
            kick_flux: 0.0,
            kick_window: PercentileWindow::new(KICK_WINDOW),
            mel_filters,
            dct_matrix,
            spectrogram_mel,
            cqt,
        }
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

    /// A3 (#1454): the kick envelope for the 30-120 Hz log-flux captured this frame,
    /// normalized once against its own long-term P95 (a single detector-owned AGC — no
    /// second pass in the feature normalizer, where `kick` is now Passthrough). Gated on
    /// the A10 perceptual silence flag: during silence it returns 0 and skips the window
    /// push, so noise-floor log-flux can't populate the P95 or manufacture kicks, and the
    /// P95 is frozen for the next active passage. Call once per hop after silence is known.
    pub fn kick_envelope(&mut self, loud_silent: bool) -> f32 {
        if loud_silent {
            return 0.0;
        }
        let flux = self.kick_flux;
        self.kick_window.push(flux);
        let p95 = self.kick_window.percentile(0.95);
        if p95 < 1e-6 {
            0.0
        } else {
            (flux / p95).clamp(0.0, 1.0)
        }
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

        // A3 (#1454): kick = 30-120 Hz log-magnitude half-wave flux. Only the raw flux is
        // captured here; the single detector-owned P95 normalization (and its silence gate)
        // runs in `kick_envelope` once the audio thread knows the perceptual silence flag,
        // so log-flux of the noise floor can't manufacture kicks. `kick` is left 0 in the
        // struct below and filled from `kick_envelope`.
        self.kick_flux = self.large.spectral_flux_range_log(30.0, 120.0);

        // Spectral features (from large FFT for best frequency resolution). `centroid_hz`
        // is the power-weighted arithmetic centroid in Hz, used as the centre for the
        // bandwidth spread; the `centroid` feature itself is on a log2 axis (A4 #1455).
        let centroid_hz = self.spectral_centroid_hz();

        let [
            sub_bass,
            bass,
            low_mid,
            mid,
            upper_mid,
            presence,
            brilliance,
        ] = self.bands();

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
            centroid: self.spectral_centroid_01(),
            flux: self.spectral_flux(),
            flatness: self.spectral_flatness(),
            rolloff: self.spectral_rolloff() / (self.sample_rate * 0.5),
            bandwidth: (self.spectral_bandwidth(centroid_hz) / (self.sample_rate * 0.5)).min(1.0),
            zcr: self.zero_crossing_rate(),
            ..Default::default()
        };

        // MFCC extraction (from large FFT magnitude)
        self.compute_mfccs(&mut out);

        // A11 (#1462): CQT-lite constant-Q chroma (also advances tuning estimation)
        out.chroma = self.cqt.compute(&self.large.magnitude);

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
    /// MFCC path and A4's mel-band flatness (#1455), so both see the same filterbank.
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

    /// Compute 13 MFCCs from the large FFT magnitude spectrum.
    fn compute_mfccs(&self, out: &mut AudioFeatures) {
        let mut mel_energies = self.mel_energies();

        // Log compression
        for e in &mut mel_energies {
            *e = (*e + 1e-10).ln();
        }

        // DCT-II → 13 MFCCs
        for i in 0..N_MFCC {
            let mut sum = 0.0f32;
            for j in 0..N_MELS {
                sum += self.dct_matrix[i][j] * mel_energies[j];
            }
            out.mfcc[i] = sum;
        }
    }

    /// Power-weighted spectral centroid in **Hz** (arithmetic, skipping the DC bin). The
    /// centre of mass used for the bandwidth spread; the `centroid` feature uses the log2
    /// form below.
    fn spectral_centroid_hz(&self) -> f32 {
        let mag = &self.large.magnitude;
        let bin_hz = self.large.bin_hz;
        let mut weighted_sum = 0.0f32;
        let mut power_sum = 0.0f32;
        for (i, &m) in mag.iter().enumerate().skip(1) {
            let p = m * m;
            weighted_sum += (i as f32 * bin_hz) * p;
            power_sum += p;
        }
        if power_sum > 1e-12 {
            weighted_sum / power_sum
        } else {
            0.0
        }
    }

    /// A4 (#1455): the `centroid` feature — a power-weighted mean of **log2(frequency)**
    /// (skipping DC) mapped onto 0..1 across `CENTROID_F_MIN..CENTROID_F_MAX`. On a log
    /// axis the centroid stops living in the top octave and becomes a usable brightness
    /// fader; the FixedRange policy (A2) holds it steady below the silence gate.
    fn spectral_centroid_01(&self) -> f32 {
        let mag = &self.large.magnitude;
        let bin_hz = self.large.bin_hz;
        let mut weighted_log2 = 0.0f32;
        let mut power_sum = 0.0f32;
        for (i, &m) in mag.iter().enumerate().skip(1) {
            let p = m * m;
            weighted_log2 += (i as f32 * bin_hz).log2() * p;
            power_sum += p;
        }
        if power_sum <= 1e-12 {
            return 0.0;
        }
        let lo = CENTROID_F_MIN.log2();
        let hi = CENTROID_F_MAX.log2();
        ((weighted_log2 / power_sum - lo) / (hi - lo)).clamp(0.0, 1.0)
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

    /// A4 (#1455): spectral flatness as **Wiener entropy over the 26 mel bands** — the
    /// ratio of their geometric to arithmetic mean, already in 0..1 (FixedRange). Computing
    /// it over mel bands (every band counted, with a tiny floor) removes the tiny-bin
    /// skipping bias and the raw-FFT HF dominance of the old version, so it cleanly
    /// separates tonal pads (low) from noise sweeps (high).
    fn spectral_flatness(&self) -> f32 {
        let mel = self.mel_energies();
        let mut log_sum = 0.0f64;
        let mut linear_sum = 0.0f64;
        for &e in &mel {
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
    pub fn spectral_contrast(&self, loud_silent: bool) -> [f32; 7] {
        if loud_silent {
            return [0.0; 7];
        }
        const LO_HZ: [f32; 6] = [200.0, 400.0, 800.0, 1600.0, 3200.0, 6400.0];
        let nyquist = self.sample_rate * 0.5;
        let mut out = [0.0f32; 7];
        let mut scratch: Vec<f32> = Vec::new(); // sorted per band; reused across the six bands
        let mut sum = 0.0f32;
        for b in 0..6 {
            let hi_hz = if b < 5 { LO_HZ[b + 1] } else { nyquist };
            let (lo, hi) = self.large.bin_range(LO_HZ[b], hi_hz);
            let c = band_contrast(&self.large.magnitude[lo..hi], &mut scratch);
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

    fn spectral_bandwidth(&self, centroid_hz: f32) -> f32 {
        let mag = &self.large.magnitude;
        let bin_hz = self.large.bin_hz;
        let mut weighted_sum = 0.0f32;
        let mut mag_sum = 0.0f32;
        for (i, &m) in mag.iter().enumerate() {
            let freq = i as f32 * bin_hz;
            let diff = freq - centroid_hz;
            weighted_sum += diff * diff * m;
            mag_sum += m;
        }
        if mag_sum > 1e-10 {
            (weighted_sum / mag_sum).sqrt()
        } else {
            0.0
        }
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

        // The real chroma should drive the key detector to C major.
        let mut det = KeyDetector::new(SR);
        let mut r = det.process(&c, 0.01);
        for _ in 0..4000 {
            r = det.process(&c, 0.01);
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

    /// A3 (#1454): the perceptual silence gate forces the kick to 0 even on a loud bass
    /// onset, and skips the P95 window push so the noise floor can't manufacture kicks.
    #[test]
    fn kick_gated_to_zero_on_silence() {
        let mut a = FftAnalyzer::new(SR, BandScale::Db);
        let mut phase = 0.0f32;
        assert_eq!(feed_kick_block(&mut a, 60.0, 0.8, &mut phase, true), 0.0);
        assert_eq!(feed_kick_block(&mut a, 60.0, 0.8, &mut phase, true), 0.0);
    }

    /// A3 (#1454): the kick fires on a bass *onset* but decays on a *sustained* tone —
    /// the level-invariant log-flux + single P95 normalizer no longer saturate on loud
    /// material (the old double-AGC did).
    #[test]
    fn kick_fires_on_onset_not_sustain() {
        let mut a = FftAnalyzer::new(SR, BandScale::Db);
        let mut phase = 0.0f32;
        // Prime with a few silent (zero) frames so the window starts from quiet.
        for _ in 0..3 {
            let z = vec![0.0f32; FFT_LARGE];
            a.analyze(&z);
            a.kick_envelope(false);
        }
        // Bass onset: jump from silence into a loud 60 Hz tone.
        let onset = feed_kick_block(&mut a, 60.0, 0.8, &mut phase, false);
        // Sustain the same tone; frame-to-frame change collapses → kick falls.
        let mut sustain = onset;
        for _ in 0..6 {
            sustain = feed_kick_block(&mut a, 60.0, 0.8, &mut phase, false);
        }
        assert!(
            onset > 0.5,
            "kick should fire on the bass onset, got {onset}"
        );
        assert!(
            sustain < onset,
            "kick should decay on a sustained tone (onset={onset}, sustain={sustain})"
        );
        assert!(
            sustain < 0.5,
            "sustained bass must not saturate the kick, got {sustain}"
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
