use rustfft::FftPlanner;
use rustfft::num_complex::Complex;

use super::features::AudioFeatures;

/// FFT sizes for multi-resolution analysis.
const FFT_LARGE: usize = 4096; // 10.8 Hz/bin — sub_bass, bass, kick
const FFT_MED: usize = 1024; // 43 Hz/bin — low_mid, mid, upper_mid
const FFT_SMALL: usize = 512; // 86 Hz/bin — presence, brilliance

/// MFCC / chroma constants
const N_MELS: usize = 26;
const N_MFCC: usize = 13;
const N_CHROMA: usize = 12;

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

    /// Half-wave rectified spectral flux in a frequency range.
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

/// Multi-resolution FFT analyzer with 7 frequency bands and spectral features.
pub struct FftAnalyzer {
    large: FftResolution,  // 4096-pt for bass
    medium: FftResolution, // 1024-pt for mids
    small: FftResolution,  // 512-pt for highs

    time_domain: Vec<f32>, // Shared sample accumulator (FFT_LARGE length)
    sample_rate: f32,

    // Kick detection state
    prev_kick_flux: f32,
    kick_max: f32,

    // MFCC precomputed data
    mel_filters: MelFilter,              // N_MELS sparse triangular filters
    dct_matrix: [[f32; N_MELS]; N_MFCC], // DCT-II coefficients

    // Chroma precomputed data: (fft_bin_index, chroma_class 0-11)
    chroma_bins: Vec<(usize, usize)>,
}

impl FftAnalyzer {
    pub fn new(sample_rate: f32) -> Self {
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
        let mel_filters =
            Self::build_mel_filterbank(large.num_bins, large.bin_hz, 20.0, sample_rate * 0.5);

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

        // Precompute chroma bin map: for each FFT bin in 20–5000 Hz, map to pitch class
        let chroma_bins = Self::build_chroma_map(large.num_bins, large.bin_hz);

        Self {
            large,
            medium,
            small,
            time_domain: vec![0.0; FFT_LARGE],
            sample_rate,
            prev_kick_flux: 0.0,
            kick_max: 0.001,
            mel_filters,
            dct_matrix,
            chroma_bins,
        }
    }

    /// Build sparse mel filterbank: N_MELS triangular filters from lo_hz to hi_hz.
    fn build_mel_filterbank(num_bins: usize, bin_hz: f32, lo_hz: f32, hi_hz: f32) -> MelFilter {
        let lo_mel = hz_to_mel(lo_hz);
        let hi_mel = hz_to_mel(hi_hz);

        // N_MELS + 2 mel-spaced center frequencies (edges of the triangles)
        let n_points = N_MELS + 2;
        let mel_points: Vec<f32> = (0..n_points)
            .map(|i| lo_mel + (hi_mel - lo_mel) * i as f32 / (n_points - 1) as f32)
            .collect();
        let hz_points: Vec<f32> = mel_points.iter().map(|&m| mel_to_hz(m)).collect();
        let bin_points: Vec<usize> = hz_points
            .iter()
            .map(|&hz| (hz / bin_hz).round() as usize)
            .collect();

        let mut filters = Vec::with_capacity(N_MELS);
        for m in 0..N_MELS {
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

    /// Build chroma bin map: for each FFT bin in 20–5000 Hz, assign pitch class 0-11.
    fn build_chroma_map(num_bins: usize, bin_hz: f32) -> Vec<(usize, usize)> {
        let mut map = Vec::new();
        for k in 1..num_bins {
            let hz = k as f32 * bin_hz;
            if !(20.0..=5000.0).contains(&hz) {
                continue;
            }
            // Pitch class: 12 * log2(f / C0), mod 12, where C0 ~= 16.35 Hz
            let semitone = 12.0 * (hz / 16.3516).log2();
            let pitch_class = ((semitone % 12.0 + 12.0) % 12.0).round() as usize % 12;
            map.push((k, pitch_class));
        }
        map
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

    fn extract_features(&mut self) -> AudioFeatures {
        let mut out = AudioFeatures::default();

        // 7-band energy extraction:
        // Bass bands (linear RMS) — from large FFT
        out.sub_bass = self.large.band_energy_linear(20.0, 60.0);
        out.bass = self.large.band_energy_linear(60.0, 250.0);

        // Mid bands (linear) — from medium FFT
        out.low_mid = self.medium.band_energy_linear(250.0, 500.0);
        out.mid = self.medium.band_energy_linear(500.0, 2000.0);
        out.upper_mid = self.medium.band_energy_db(2000.0, 4000.0);

        // High bands (dB-scaled) — from small FFT
        out.presence = self.small.band_energy_db(4000.0, 6000.0);
        out.brilliance = self.small.band_energy_db(6000.0, 20000.0);

        // RMS from time domain (use last 2048 samples for reasonable window)
        let td_start = FFT_LARGE - 2048;
        let sum_sq: f32 = self.time_domain[td_start..].iter().map(|s| s * s).sum();
        out.rms = (sum_sq / 2048.0).sqrt();

        // Kick detection: half-wave rectified spectral flux in 30-120 Hz (from large FFT)
        let kick_flux = self.large.spectral_flux_range(30.0, 120.0);
        // Normalize by running max with decay
        self.kick_max = (self.kick_max * 0.999).max(kick_flux).max(0.001);
        out.kick = (kick_flux / self.kick_max).clamp(0.0, 1.0);
        self.prev_kick_flux = kick_flux;

        // Spectral features (from large FFT for best frequency resolution)
        let centroid_hz = self.spectral_centroid();
        out.centroid = centroid_hz / (self.sample_rate * 0.5);

        out.flux = self.spectral_flux();

        out.flatness = self.spectral_flatness();

        out.rolloff = self.spectral_rolloff() / (self.sample_rate * 0.5);

        out.bandwidth = (self.spectral_bandwidth(centroid_hz) / (self.sample_rate * 0.5)).min(1.0);

        out.zcr = self.zero_crossing_rate();

        // MFCC extraction (from large FFT magnitude)
        self.compute_mfccs(&mut out);

        // Chroma extraction (from large FFT magnitude)
        self.compute_chroma(&mut out);

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

    /// Compute 13 MFCCs from the large FFT magnitude spectrum.
    fn compute_mfccs(&self, out: &mut AudioFeatures) {
        let mag = &self.large.magnitude;

        // Apply mel filterbank → 26 mel energies
        let mut mel_energies = [0.0f32; N_MELS];
        for (m, filter) in self.mel_filters.iter().enumerate() {
            let mut energy = 0.0f32;
            for &(k, w) in filter {
                energy += mag[k] * mag[k] * w;
            }
            mel_energies[m] = energy;
        }

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

    /// Compute 12 chroma pitch-class energies from the large FFT magnitude spectrum.
    fn compute_chroma(&self, out: &mut AudioFeatures) {
        let mag = &self.large.magnitude;

        let mut chroma = [0.0f32; N_CHROMA];
        for &(k, pitch_class) in &self.chroma_bins {
            chroma[pitch_class] += mag[k] * mag[k];
        }

        // Normalize by max
        let max_val = chroma.iter().cloned().fold(0.0f32, f32::max);
        if max_val > 1e-10 {
            for c in &mut chroma {
                *c /= max_val;
            }
        }

        out.chroma = chroma;
    }

    fn spectral_centroid(&self) -> f32 {
        let mag = &self.large.magnitude;
        let bin_hz = self.large.bin_hz;
        let mut weighted_sum = 0.0f32;
        let mut mag_sum = 0.0f32;
        for (i, &m) in mag.iter().enumerate() {
            let freq = i as f32 * bin_hz;
            weighted_sum += freq * m;
            mag_sum += m;
        }
        if mag_sum > 1e-10 {
            weighted_sum / mag_sum
        } else {
            0.0
        }
    }

    fn spectral_flux(&self) -> f32 {
        let mag = &self.large.magnitude;
        let prev = &self.large.prev_magnitude;
        let mut flux = 0.0f32;
        for i in 0..mag.len() {
            let diff = mag[i] - prev[i];
            if diff > 0.0 {
                flux += diff;
            }
        }
        flux
    }

    fn spectral_flatness(&self) -> f32 {
        let mag = &self.large.magnitude;
        let mut log_sum = 0.0f64;
        let mut linear_sum = 0.0f64;
        let mut count = 0u32;
        for &m in &mag[1..] {
            let m = m as f64;
            if m > 1e-10 {
                log_sum += m.ln();
                linear_sum += m;
                count += 1;
            }
        }
        if count == 0 || linear_sum < 1e-10 {
            return 0.0;
        }
        let geometric_mean = (log_sum / count as f64).exp();
        let arithmetic_mean = linear_sum / count as f64;
        (geometric_mean / arithmetic_mean) as f32
    }

    fn spectral_rolloff(&self) -> f32 {
        let mag = &self.large.magnitude;
        let bin_hz = self.large.bin_hz;
        let total_energy: f32 = mag.iter().map(|m| m * m).sum();
        let threshold = total_energy * 0.85;
        let mut cumulative = 0.0f32;
        for (i, &m) in mag.iter().enumerate() {
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
