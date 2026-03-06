//! 3-stage beat detection pipeline: OnsetDetector → TempoEstimator → BeatScheduler.
//! Ported from easey-glyph's Python implementation.

use rustfft::FftPlanner;
use rustfft::num_complex::Complex;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Circular buffer (fixed-size ring buffer with statistical methods)
// ---------------------------------------------------------------------------

struct CircularBuffer {
    buf: Vec<f64>,
    cap: usize,
    write: usize,
    count: usize,
}

impl CircularBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0.0; capacity],
            cap: capacity,
            write: 0,
            count: 0,
        }
    }

    fn push(&mut self, value: f64) {
        self.buf[self.write] = value;
        self.write = (self.write + 1) % self.cap;
        if self.count < self.cap {
            self.count += 1;
        }
    }

    fn len(&self) -> usize {
        self.count
    }

    fn values(&self) -> Vec<f64> {
        if self.count == 0 {
            return Vec::new();
        }
        if self.count < self.cap {
            self.buf[..self.count].to_vec()
        } else {
            let start = self.write;
            let mut v = Vec::with_capacity(self.cap);
            v.extend_from_slice(&self.buf[start..]);
            v.extend_from_slice(&self.buf[..start]);
            v
        }
    }

    fn median(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let mut vals = self.values();
        vals.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = vals.len() / 2;
        if vals.len().is_multiple_of(2) {
            f64::midpoint(vals[mid - 1], vals[mid])
        } else {
            vals[mid]
        }
    }

    fn mad(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        let med = self.median();
        let mut abs_devs: Vec<f64> = self.values().iter().map(|v| (v - med).abs()).collect();
        abs_devs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = abs_devs.len() / 2;
        if abs_devs.len().is_multiple_of(2) {
            f64::midpoint(abs_devs[mid - 1], abs_devs[mid])
        } else {
            abs_devs[mid]
        }
    }

    fn max(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        if self.count < self.cap {
            self.buf[..self.count]
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max)
        } else {
            self.buf.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
        }
    }

    fn mean(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        if self.count < self.cap {
            self.buf[..self.count].iter().sum::<f64>() / self.count as f64
        } else {
            self.buf.iter().sum::<f64>() / self.cap as f64
        }
    }
}

// ---------------------------------------------------------------------------
// Stage 1: Multi-band onset detection
// ---------------------------------------------------------------------------

/// Frequency band definition: (lo_hz, hi_hz, weight)
const ONSET_BANDS: [(f32, f32, f32); 4] = [
    (20.0, 80.0, 0.4),     // sub-bass (kick drums)
    (80.0, 250.0, 0.3),    // bass (bass guitar/synth)
    (500.0, 2000.0, 0.2),  // mid (snares/vocals)
    (2000.0, 4000.0, 0.1), // high-mid (hi-hats/cymbals)
];

struct OnsetDetector {
    sample_rate: f32,
    threshold_mult: f32,
    silence_threshold: f32,
    threshold_ceiling: f32,

    prev_mags: [Option<Vec<f64>>; 4],
    onset_history: CircularBuffer,
    long_term_history: CircularBuffer,
    silent_frames: u32,
}

impl OnsetDetector {
    fn new(sample_rate: f32, history_size: usize, long_term_size: usize) -> Self {
        Self {
            sample_rate,
            threshold_mult: 2.0,
            silence_threshold: 0.002,
            threshold_ceiling: 0.5,
            prev_mags: [None, None, None, None],
            onset_history: CircularBuffer::new(history_size),
            long_term_history: CircularBuffer::new(long_term_size),
            silent_frames: 0,
        }
    }

    fn bin_range(&self, lo_hz: f32, hi_hz: f32, fft_size: usize) -> (usize, usize) {
        let bin_width = self.sample_rate / fft_size as f32;
        let lo_bin = (lo_hz / bin_width).round().max(0.0) as usize;
        let hi_bin = ((hi_hz / bin_width).round() as usize).min(fft_size / 2);
        (lo_bin, hi_bin)
    }

    /// Process multi-resolution spectra and return (is_onset, onset_strength, combined_flux).
    fn process(
        &mut self,
        bass_spectrum: &[f32], // 4096-pt FFT magnitudes (num_bins)
        mid_spectrum: &[f32],  // 1024-pt FFT magnitudes
        high_spectrum: &[f32], // 512-pt FFT magnitudes
        rms: f32,
    ) -> (bool, f32, f64) {
        // Silence gate
        if rms < self.silence_threshold {
            self.silent_frames += 1;
            return (false, 0.0, 0.0);
        }
        self.silent_frames = 0;

        // Map bands to spectra: sub-bass & bass → 4096, mid → 1024, high-mid → 512
        let spectra: [&[f32]; 4] = [bass_spectrum, bass_spectrum, mid_spectrum, high_spectrum];
        // Reconstruct fft_size from num_bins: num_bins = fft_size/2 + 1
        let fft_sizes: [usize; 4] = [
            (bass_spectrum.len() - 1) * 2,
            (bass_spectrum.len() - 1) * 2,
            (mid_spectrum.len() - 1) * 2,
            (high_spectrum.len() - 1) * 2,
        ];

        let mut band_flux = [0.0f64; 4];

        for (i, &(lo_hz, hi_hz, _weight)) in ONSET_BANDS.iter().enumerate() {
            let spectrum = spectra[i];
            let fft_size = fft_sizes[i];
            let (lo_bin, hi_bin) = self.bin_range(lo_hz, hi_hz, fft_size);

            if hi_bin <= lo_bin || hi_bin > spectrum.len() {
                continue;
            }

            // Log-magnitude spectral flux: better models human loudness perception
            let current_mags: Vec<f64> = spectrum[lo_bin..hi_bin]
                .iter()
                .map(|&m| (m as f64 + 1e-10).ln())
                .collect();

            if let Some(ref prev) = self.prev_mags[i] {
                if prev.len() == current_mags.len() {
                    let count = current_mags.len().max(1) as f64;
                    let flux: f64 = current_mags
                        .iter()
                        .zip(prev.iter())
                        .map(|(&c, &p)| (c - p).max(0.0))
                        .sum();
                    band_flux[i] = flux / count;
                }
            }

            self.prev_mags[i] = Some(current_mags);
        }

        // Weighted combination
        let weight_sum: f32 = ONSET_BANDS.iter().map(|b| b.2).sum();
        let combined_flux: f64 = band_flux
            .iter()
            .zip(ONSET_BANDS.iter())
            .map(|(&flux, &(_, _, w))| flux * w as f64)
            .sum::<f64>()
            / weight_sum as f64;

        self.onset_history.push(combined_flux);
        self.long_term_history.push(combined_flux);

        // Adaptive threshold: median + k * MAD
        let threshold = self.compute_threshold();
        let is_onset = combined_flux > threshold;

        // Normalize onset strength to 0-1
        let recent_max = self.long_term_history.max();
        let onset_strength = (combined_flux / recent_max.max(1e-6)).min(1.0) as f32;

        (is_onset, onset_strength, combined_flux)
    }

    fn compute_threshold(&self) -> f64 {
        let median = self.onset_history.median();
        let mad = self.onset_history.mad();
        let base_threshold = median + self.threshold_mult as f64 * mad;

        let min_threshold = 0.001;

        // Cap at proportion of long-term max
        let mut max_threshold = f64::INFINITY;
        if self.long_term_history.len() > self.onset_history.len() {
            let lt_max = self.long_term_history.max();
            max_threshold = lt_max * self.threshold_ceiling as f64;
        }

        // Also cap at 80% of recent max
        let recent_max = self.onset_history.max();
        let recent_ceiling = recent_max * 0.8;

        let capped = base_threshold.min(max_threshold).min(recent_ceiling);
        capped.max(min_threshold)
    }

    fn is_sustained_silence(&self) -> bool {
        self.silent_frames >= 30
    }
}

// ---------------------------------------------------------------------------
// Kalman filter for BPM tracking in log2-BPM space
// ---------------------------------------------------------------------------

struct KalmanBpm {
    state: f64,         // log2(BPM)
    variance: f64,      // estimation uncertainty
    q: f64,             // process noise
    r: f64,             // measurement noise
    diverge_count: u32, // consecutive divergent frames
    snap_count: u32,    // consecutive octave-snapped frames
    initialized: bool,
}

impl KalmanBpm {
    fn new() -> Self {
        Self {
            state: 0.0,
            variance: 1.0,
            q: 0.001,
            r: 0.1,
            diverge_count: 0,
            snap_count: 0,
            initialized: false,
        }
    }

    /// Update with a raw BPM measurement and confidence. Returns filtered BPM.
    fn update(&mut self, raw_bpm: f64, confidence: f64) -> f64 {
        if raw_bpm <= 0.0 {
            return if self.initialized {
                2.0f64.powf(self.state)
            } else {
                0.0
            };
        }

        if !self.initialized {
            self.state = raw_bpm.log2();
            self.variance = 1.0;
            self.initialized = true;
            return raw_bpm;
        }

        let current_bpm = 2.0f64.powf(self.state);

        // Octave-aware preprocessing: snap only true octave errors (2:1, 1:2)
        let ratio = raw_bpm / current_bpm;
        let mut snapped_bpm = raw_bpm;
        let mut was_snapped = false;
        for &hr in &[0.5, 2.0] {
            if (ratio - hr).abs() / hr < 0.05 {
                snapped_bpm = current_bpm;
                was_snapped = true;
                break;
            }
        }

        // Track consecutive snaps — if snapping for too long, the tempo may
        // have genuinely changed to the half/double. Accept raw measurement.
        if was_snapped {
            self.snap_count += 1;
            if self.snap_count >= 30 {
                log::debug!(
                    "Kalman snap escape: accepting {:.1} BPM after {} consecutive snaps",
                    raw_bpm,
                    self.snap_count
                );
                snapped_bpm = raw_bpm;
                was_snapped = false;
                self.snap_count = 0;
            }
        } else {
            self.snap_count = 0;
        }
        let snapped_measurement = snapped_bpm.log2();

        // Divergence detection: 5 consecutive frames >10% deviation -> hard reset
        let bpm_deviation = (snapped_bpm - current_bpm).abs() / current_bpm.max(1.0);
        if bpm_deviation > 0.10 {
            self.diverge_count += 1;
        } else {
            self.diverge_count = 0;
        }

        if self.diverge_count >= 15 {
            log::debug!(
                "Kalman hard reset: {:.1} -> {:.1} BPM (diverged for {} frames)",
                current_bpm,
                raw_bpm,
                self.diverge_count
            );
            self.state = raw_bpm.log2();
            self.variance = 1.0;
            self.diverge_count = 0;
            return raw_bpm;
        }

        // Adaptive noise: R = f(confidence), Q = f(stability)
        self.r = 0.01 + (1.0 - confidence) * 0.5;
        self.q = if self.diverge_count > 0 { 0.1 } else { 0.001 };

        // Kalman predict (constant model: state unchanged)
        self.variance += self.q;

        // Kalman update
        let innovation = snapped_measurement - self.state;
        let s = self.variance + self.r;
        let k = self.variance / s;
        self.state += k * innovation;
        self.variance *= 1.0 - k;

        2.0f64.powf(self.state)
    }
}

// ---------------------------------------------------------------------------
// Stage 2: FFT-based tempo estimation with Kalman tracking
// ---------------------------------------------------------------------------

struct TempoEstimator {
    bpm_range: (f32, f32),

    onset_history: CircularBuffer,
    frame_time_history: CircularBuffer,
    frame_rate: f64,
    frame_time: f64,
    initial_frame_time: f64,
    last_time: f64,
    frame_count: u32,

    current_bpm: f64,
    current_confidence: f64,
    current_period_frames: f64,

    // FFT-based generalized autocorrelation
    fft_forward: Arc<dyn rustfft::Fft<f64>>,
    fft_inverse: Arc<dyn rustfft::Fft<f64>>,
    fft_size: usize,

    // Genre-aware tempo prior (log-Gaussian)
    prior_center_log2: f64,
    prior_sigma: f64,

    // Kalman filter replaces EMA + stability tracking
    kalman: KalmanBpm,
}

impl TempoEstimator {
    fn new(history_seconds: f64, frame_rate: f64, prior_center_bpm: f64) -> Self {
        let history_size = (history_seconds * frame_rate).ceil() as usize;
        let frame_time = 1.0 / frame_rate;

        let fft_size = (2 * history_size).next_power_of_two();
        let mut planner = FftPlanner::<f64>::new();
        let fft_forward = planner.plan_fft_forward(fft_size);
        let fft_inverse = planner.plan_fft_inverse(fft_size);

        Self {
            bpm_range: (40.0, 300.0),
            onset_history: CircularBuffer::new(history_size),
            frame_time_history: CircularBuffer::new(30),
            frame_rate,
            frame_time,
            initial_frame_time: frame_time,
            last_time: 0.0,
            frame_count: 0,
            current_bpm: 0.0,
            current_confidence: 0.0,
            current_period_frames: 0.0,
            fft_forward,
            fft_inverse,
            fft_size,
            prior_center_log2: prior_center_bpm.log2(),
            prior_sigma: 1.0,
            kalman: KalmanBpm::new(),
        }
    }

    /// Update tempo estimate. Returns (bpm, confidence, period_seconds).
    fn update(&mut self, onset_value: f64, timestamp: f64) -> (f64, f64, f64) {
        // Track frame timing
        if self.last_time > 0.0 {
            let dt = timestamp - self.last_time;
            if dt > 0.0 && dt < 0.1 {
                self.frame_time_history.push(dt);
                if self.frame_time_history.len() >= 10 {
                    let measured = self.frame_time_history.mean();
                    let lo = self.initial_frame_time * 0.5;
                    let hi = self.initial_frame_time * 2.0;
                    self.frame_time = measured.clamp(lo, hi);
                    self.frame_rate = 1.0 / self.frame_time;
                }
            }
        }
        self.last_time = timestamp;

        self.onset_history.push(onset_value);
        self.frame_count += 1;

        // Need enough history (at least 2s)
        let min_frames = (2.0 * self.frame_rate).ceil() as usize;
        if self.onset_history.len() < min_frames {
            return (0.0, 0.0, 0.0);
        }

        // Compute autocorrelation every ~6 frames (~16Hz update rate)
        if !self.frame_count.is_multiple_of(6) {
            let period_s = self.current_period_frames * self.frame_time;
            return (self.current_bpm, self.current_confidence, period_s);
        }

        let (raw_bpm, confidence, _raw_period_frames) = self.compute_tempo();

        // Confidence gate: don't feed low-confidence garbage to the Kalman.
        // During noisy sections (off-beats, transitions), the autocorrelation
        // is unreliable and would corrupt the filter state.
        if confidence < 0.15 && self.current_bpm > 0.0 {
            self.current_confidence = confidence;
            let period_s = self.current_period_frames * self.frame_time;
            return (self.current_bpm, self.current_confidence, period_s);
        }

        // Kalman filter update
        let filtered_bpm = self.kalman.update(raw_bpm, confidence);

        self.current_bpm = filtered_bpm;
        self.current_confidence = confidence;
        self.current_period_frames = if filtered_bpm > 0.0 {
            60.0 / (filtered_bpm * self.frame_time)
        } else {
            0.0
        };

        let period_s = self.current_period_frames * self.frame_time;
        (self.current_bpm, self.current_confidence, period_s)
    }

    fn compute_tempo(&mut self) -> (f64, f64, f64) {
        let history = self.onset_history.values();
        let n = history.len();

        // Convert BPM range to lag range (frames)
        let max_lag = ((60.0 / (self.bpm_range.0 as f64 * self.frame_time)) as usize).min(n / 2);
        let min_lag = ((60.0 / (self.bpm_range.1 as f64 * self.frame_time)) as usize).max(1);

        if max_lag <= min_lag {
            return (0.0, 0.0, 0.0);
        }

        // FFT-based autocorrelation via Wiener-Khinchin: zero-pad -> FFT -> |X|^2 -> IFFT
        // Using power spectrum (exponent 2) instead of amplitude (exponent 1) because
        // the power spectrum gives the fundamental period a clear height advantage over
        // subharmonics, reducing octave ambiguity.
        // Mean-subtract to remove DC offset — critical for autocorrelation contrast.
        let mean = history.iter().sum::<f64>() / n as f64;
        let mut buffer: Vec<Complex<f64>> = vec![Complex::new(0.0, 0.0); self.fft_size];
        for (i, &v) in history.iter().enumerate() {
            buffer[i] = Complex::new(v - mean, 0.0);
        }

        self.fft_forward.process(&mut buffer);

        // Power spectrum |X|^2 — standard autocorrelation (Wiener-Khinchin)
        for c in &mut buffer {
            let power = c.norm_sqr();
            *c = Complex::new(power, 0.0);
        }

        self.fft_inverse.process(&mut buffer);

        // Normalize by fft_size (rustfft doesn't normalize) and by zero-lag
        let scale = 1.0 / self.fft_size as f64;
        let zero_lag = buffer[0].re * scale;
        if zero_lag <= 0.0 {
            return (0.0, 0.0, 0.0);
        }

        // Extract autocorrelation up to 4*max_lag for harmonic scoring
        let acr_len = (4 * max_lag + 1).min(n).min(self.fft_size);
        let autocorr: Vec<f64> = buffer[..acr_len]
            .iter()
            .map(|c| c.re * scale / zero_lag)
            .collect();
        let acr_max = autocorr.len() - 1;

        // Find initial peak using raw autocorrelation (no prior bias)
        // Prior is applied only in the multi-ratio correction step
        let mut best_lag = min_lag;
        let mut best_value = f64::NEG_INFINITY;

        for lag in min_lag..=max_lag.min(acr_max) {
            if autocorr[lag] > best_value {
                best_value = autocorr[lag];
                best_lag = lag;
            }
        }

        // Multi-ratio octave correction
        // Check metrical ratios: for each, compute harmonic score weighted by tempo prior.
        // 1:3 and 1:4 are needed because the initial peak can land on the 3rd or 4th
        // subharmonic (e.g., lag 66 for a true period of 22), and without these ratios
        // the correction can only step down to 2T, never reaching T directly.
        let ratios: [(f64, f64); 9] = [
            (1.0, 4.0), // quarter lag -> 4x BPM
            (1.0, 3.0), // third lag -> 3x BPM
            (1.0, 2.0), // half lag -> double BPM
            (2.0, 3.0),
            (3.0, 4.0),
            (1.0, 1.0), // same
            (4.0, 3.0),
            (3.0, 2.0),
            (2.0, 1.0), // double lag -> half BPM
        ];

        let mut best_candidate_lag = best_lag;
        let mut best_score = f64::NEG_INFINITY;

        for &(num, den) in &ratios {
            let candidate_lag = ((best_lag as f64 * num / den).round() as usize).max(1);
            if candidate_lag < min_lag || candidate_lag > max_lag || candidate_lag > acr_max {
                continue;
            }

            // Harmonic score: base + sum(autocorr[h*lag] / h) for h=2,3,4
            let mut harmonic_score = autocorr[candidate_lag];
            for h in 2..=4usize {
                let h_lag = candidate_lag * h;
                if h_lag <= acr_max {
                    harmonic_score += autocorr[h_lag] / h as f64;
                }
            }

            let bpm = 60.0 / (candidate_lag as f64 * self.frame_time);
            let weight = self.tempo_prior_weight(bpm);
            let weighted_score = harmonic_score * weight;

            if weighted_score > best_score {
                best_score = weighted_score;
                best_candidate_lag = candidate_lag;
            }
        }

        if best_candidate_lag != best_lag {
            let old_bpm = 60.0 / (best_lag as f64 * self.frame_time);
            let new_bpm = 60.0 / (best_candidate_lag as f64 * self.frame_time);
            log::debug!(
                "Multi-ratio correction: lag {} ({:.1} BPM) -> lag {} ({:.1} BPM)",
                best_lag,
                old_bpm,
                best_candidate_lag,
                new_bpm
            );
        }

        best_lag = best_candidate_lag;

        // Parabolic interpolation for sub-frame precision
        let mut refined_lag = best_lag as f64;
        if best_lag > min_lag && best_lag < max_lag.min(acr_max) {
            let alpha = autocorr[best_lag - 1];
            let beta = autocorr[best_lag];
            let gamma = autocorr[best_lag + 1];
            let denom = 2.0 * (2.0 * beta - alpha - gamma);
            if denom.abs() > 1e-10 {
                let p = (alpha - gamma) / denom;
                refined_lag = best_lag as f64 + p.clamp(-0.5, 0.5);
            }
        }

        // Convert to BPM
        let period_s = refined_lag * self.frame_time;
        let bpm = if period_s > 0.0 { 60.0 / period_s } else { 0.0 };

        // Confidence: peak height relative to noise floor in the BPM range
        // Generalized autocorrelation (|FFT|^1) gives lower absolute values than
        // standard autocorrelation, so use relative measure instead
        let confidence = if best_lag <= acr_max {
            let range_end = max_lag.min(acr_max);
            let mut sorted_vals: Vec<f64> = autocorr[min_lag..=range_end].to_vec();
            sorted_vals
                .sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let noise_floor = sorted_vals[sorted_vals.len() / 2]; // median
            let peak = autocorr[best_lag];
            ((peak - noise_floor) / (1.0 - noise_floor).max(1e-6))
                .max(0.0)
                .min(1.0)
        } else {
            0.0
        };

        if bpm < self.bpm_range.0 as f64 || bpm > self.bpm_range.1 as f64 {
            return (0.0, 0.0, 0.0);
        }

        log::debug!(
            "Tempo estimate: {:.1} BPM (confidence {:.2}, lag {:.1})",
            bpm,
            confidence,
            refined_lag
        );

        (bpm, confidence, refined_lag)
    }

    /// Log-Gaussian tempo prior weight centered at prior_center_bpm.
    fn tempo_prior_weight(&self, bpm: f64) -> f64 {
        if bpm <= 0.0 {
            return 0.0;
        }
        let log2_bpm = bpm.log2();
        let diff = log2_bpm - self.prior_center_log2;
        (-0.5 * (diff / self.prior_sigma).powi(2)).exp()
    }
}

// ---------------------------------------------------------------------------
// Stage 3: Beat scheduler (prediction + confirmation)
// ---------------------------------------------------------------------------

#[derive(PartialEq)]
enum BeatState {
    Waiting,
    Expecting,
    Confirmed,
    Missed,
}

struct BeatScheduler {
    beat_window: f64,
    refractory: f64,
    min_confidence: f64,
    phase_correction: f64,
    max_misses: u32,
    beat_timeout: f64,

    state: BeatState,
    last_beat_time: f64,
    next_predicted: f64,
    phase: f64,

    bpm: f64,
    period: f64,
    tempo_confidence: f64,

    beat_strength: f64,
    tracking_confidence: f64,
    consecutive_misses: u32,
    last_fired_time: f64,
    stored_period: f64,
}

impl BeatScheduler {
    fn new() -> Self {
        Self {
            beat_window: 0.08,
            refractory: 0.15,
            min_confidence: 0.4,
            phase_correction: 0.3,
            max_misses: 4,
            beat_timeout: 3.0,

            state: BeatState::Waiting,
            last_beat_time: 0.0,
            next_predicted: 0.0,
            phase: 0.0,

            bpm: 0.0,
            period: 0.0,
            tempo_confidence: 0.0,

            beat_strength: 0.0,
            tracking_confidence: 0.0,
            consecutive_misses: 0,
            last_fired_time: 0.0,
            stored_period: 0.0,
        }
    }

    fn update_tempo(&mut self, bpm: f64, period: f64, confidence: f64) {
        self.bpm = bpm;
        self.period = period;
        self.tempo_confidence = confidence;
        if confidence >= self.min_confidence && period > 0.0 {
            self.stored_period = period;
        }
    }

    /// Main beat decision. Returns (is_beat, beat_phase, smoothed_bpm).
    fn process(
        &mut self,
        is_onset: bool,
        onset_strength: f32,
        timestamp: f64,
        is_silence: bool,
    ) -> (bool, f64, f64) {
        let time_since_beat = if self.last_beat_time > 0.0 {
            timestamp - self.last_beat_time
        } else {
            f64::INFINITY
        };
        let in_refractory = time_since_beat < self.refractory;

        if is_onset {
            self.beat_strength = onset_strength as f64;
        }

        // Update phase and count missed predicted beats
        let missed = if self.period > 0.0 {
            self.update_phase(timestamp)
        } else {
            0
        };

        // Beat timeout recovery
        let time_since_fired = if self.last_fired_time > 0.0 {
            timestamp - self.last_fired_time
        } else {
            0.0
        };
        if time_since_fired > self.beat_timeout && self.last_fired_time > 0.0 {
            self.tracking_confidence *= 0.3;
            self.consecutive_misses = 0;
            self.state = BeatState::Waiting;
            self.last_fired_time = timestamp;
        }

        // Silence → pause beat detection
        if is_silence {
            if self.state != BeatState::Waiting {
                self.state = BeatState::Waiting;
                self.consecutive_misses = 0;
            }
            return (false, 0.0, self.bpm);
        }

        // Determine if we should fire a beat
        let mut is_beat = false;

        if missed > 0 && self.tempo_confidence >= self.min_confidence {
            is_beat = true;
            self.beat_strength = 0.5;
            self.state = BeatState::Missed;
        } else if self.tempo_confidence < self.min_confidence || self.period == 0.0 {
            // Low confidence: onset-only mode
            is_beat = self.onset_only(is_onset, in_refractory);

            // Backup prediction
            if !is_beat && self.stored_period > 0.0 && self.last_fired_time > 0.0 {
                if time_since_fired >= self.stored_period * 0.9 && !in_refractory {
                    is_beat = true;
                    self.beat_strength = 0.5;
                }
            }
        } else {
            // High confidence: predictive mode
            is_beat = self.predictive(is_onset, timestamp, in_refractory);
        }

        if is_beat {
            self.last_beat_time = timestamp;
            self.last_fired_time = timestamp;
            self.phase = 0.0;
            self.consecutive_misses = 0;
            if self.period > 0.0 {
                self.next_predicted = timestamp + self.period;
            }
            self.tracking_confidence = (self.tracking_confidence + 0.15).min(1.0);
        } else {
            self.tracking_confidence = (self.tracking_confidence - 0.0005).max(0.0);
        }

        (is_beat, self.phase, self.bpm)
    }

    fn onset_only(&mut self, is_onset: bool, in_refractory: bool) -> bool {
        self.state = BeatState::Waiting;
        if is_onset && !in_refractory {
            self.state = BeatState::Confirmed;
            return true;
        }
        false
    }

    fn predictive(&mut self, is_onset: bool, timestamp: f64, in_refractory: bool) -> bool {
        let dist = if self.next_predicted > 0.0 {
            timestamp - self.next_predicted
        } else {
            f64::INFINITY
        };
        let in_window = dist.abs() <= self.beat_window;
        let window_expired = dist > self.beat_window;

        match self.state {
            BeatState::Waiting => {
                if self.next_predicted == 0.0 {
                    if is_onset && !in_refractory {
                        self.next_predicted = timestamp + self.period;
                        self.state = BeatState::Confirmed;
                        return true;
                    }
                } else {
                    self.state = BeatState::Expecting;
                }
                false
            }
            BeatState::Expecting => {
                if is_onset && in_window && !in_refractory {
                    self.state = BeatState::Confirmed;
                    if self.period > 0.0 {
                        let phase_error = dist / self.period;
                        self.apply_phase_correction(phase_error);
                    }
                    return true;
                } else if window_expired {
                    self.state = BeatState::Missed;
                    return true;
                }
                false
            }
            BeatState::Confirmed | BeatState::Missed => {
                self.state = BeatState::Expecting;
                if is_onset && in_window && !in_refractory {
                    self.state = BeatState::Confirmed;
                    return true;
                }
                false
            }
        }
    }

    fn update_phase(&mut self, timestamp: f64) -> u32 {
        if self.last_beat_time == 0.0 || self.period == 0.0 {
            return 0;
        }

        let elapsed = timestamp - self.last_beat_time;
        self.phase = (elapsed / self.period) % 1.0;

        let mut missed = 0u32;
        while self.next_predicted > 0.0 && timestamp > self.next_predicted + self.beat_window {
            self.consecutive_misses += 1;
            missed += 1;
            self.next_predicted += self.period;
            if self.consecutive_misses == self.max_misses {
                self.tracking_confidence *= 0.7;
            }
        }
        missed
    }

    fn apply_phase_correction(&mut self, phase_error: f64) {
        let clamped = phase_error.clamp(-0.5, 0.5);
        let correction = clamped * self.period * self.phase_correction;
        self.next_predicted += correction;
    }
}

// ---------------------------------------------------------------------------
// Main BeatDetector facade
// ---------------------------------------------------------------------------

/// Result from beat detection for one frame.
pub struct BeatResult {
    pub onset_strength: f32,
    pub beat: f32,
    pub beat_phase: f32,
    pub bpm: f32,
    pub beat_strength: f32,
}

/// 3-stage beat detection pipeline.
pub struct BeatDetector {
    onset_detector: OnsetDetector,
    tempo_estimator: TempoEstimator,
    beat_scheduler: BeatScheduler,

    // Onset hold+decay
    held_onset: f32,
    onset_decay_tau: f32,
    last_timestamp: f64,

    // Onset cooldown
    onset_cooldown: f64,
    last_onset_time: f64,
}

impl BeatDetector {
    pub fn new(sample_rate: f32) -> Self {
        // Approximate frame rate (~100 Hz for 10ms audio loop)
        let frame_rate = 100.0;

        let history_size = (0.5 * frame_rate) as usize; // ~0.5s
        let long_term_size = (4.0 * frame_rate) as usize; // ~4s

        Self {
            onset_detector: OnsetDetector::new(sample_rate, history_size, long_term_size),
            tempo_estimator: TempoEstimator::new(8.0, frame_rate, 150.0),
            beat_scheduler: BeatScheduler::new(),
            held_onset: 0.0,
            onset_decay_tau: 0.20,
            last_timestamp: 0.0,
            onset_cooldown: 0.05,
            last_onset_time: 0.0,
        }
    }

    /// Process one frame of audio data.
    ///
    /// Arguments:
    /// - bass_spectrum: magnitude spectrum from 4096-pt FFT (num_bins)
    /// - mid_spectrum: magnitude spectrum from 1024-pt FFT
    /// - high_spectrum: magnitude spectrum from 512-pt FFT
    /// - rms: current RMS energy
    /// - timestamp: current time in seconds
    pub fn process(
        &mut self,
        bass_spectrum: &[f32],
        mid_spectrum: &[f32],
        high_spectrum: &[f32],
        rms: f32,
        timestamp: f64,
    ) -> BeatResult {
        let dt = if self.last_timestamp > 0.0 {
            (timestamp - self.last_timestamp).max(0.0)
        } else {
            0.0
        };
        self.last_timestamp = timestamp;

        // Stage 1: Onset detection
        let (is_onset, onset_strength, combined_flux) =
            self.onset_detector
                .process(bass_spectrum, mid_spectrum, high_spectrum, rms);

        // Apply onset cooldown
        let mut onset_gated = is_onset;
        if is_onset && (timestamp - self.last_onset_time) < self.onset_cooldown {
            onset_gated = false;
        }
        if onset_gated {
            self.last_onset_time = timestamp;
        }

        // Stage 2: Tempo estimation
        let (bpm, confidence, period_s) = self.tempo_estimator.update(combined_flux, timestamp);

        // Stage 3: Beat scheduling
        self.beat_scheduler.update_tempo(bpm, period_s, confidence);
        let (is_beat, beat_phase, smoothed_bpm) = self.beat_scheduler.process(
            onset_gated,
            onset_strength,
            timestamp,
            self.onset_detector.is_sustained_silence(),
        );

        // Onset hold+decay (instant attack, exponential release)
        if onset_strength > self.held_onset {
            self.held_onset = onset_strength;
        } else if dt > 0.0 {
            self.held_onset *= (-dt as f32 / self.onset_decay_tau).exp();
        }

        // Freeze phase at 0 during silence
        let phase = if rms < 1e-4 { 0.0 } else { beat_phase as f32 };

        BeatResult {
            onset_strength: self.held_onset,
            beat: if is_beat { 1.0 } else { 0.0 },
            beat_phase: phase,
            bpm: if smoothed_bpm > 0.0 {
                smoothed_bpm as f32
            } else {
                0.0
            },
            beat_strength: if is_beat {
                self.beat_scheduler.beat_strength as f32
            } else {
                0.0
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }
    fn approx_eq_f64(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    // ---- CircularBuffer tests ----

    #[test]
    fn circular_buffer_new_empty() {
        let buf = CircularBuffer::new(10);
        assert_eq!(buf.len(), 0);
        assert!(buf.values().is_empty());
    }

    #[test]
    fn circular_buffer_push_under_capacity() {
        let mut buf = CircularBuffer::new(5);
        buf.push(1.0);
        buf.push(2.0);
        buf.push(3.0);
        assert_eq!(buf.len(), 3);
        assert_eq!(buf.values(), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn circular_buffer_push_wrap_around() {
        let mut buf = CircularBuffer::new(3);
        buf.push(1.0);
        buf.push(2.0);
        buf.push(3.0);
        buf.push(4.0); // wraps, oldest (1.0) overwritten
        assert_eq!(buf.len(), 3);
        let vals = buf.values();
        assert_eq!(vals, vec![2.0, 3.0, 4.0]);
    }

    #[test]
    fn circular_buffer_median_odd() {
        let mut buf = CircularBuffer::new(5);
        for v in [3.0, 1.0, 4.0, 1.0, 5.0] {
            buf.push(v);
        }
        // sorted: [1.0, 1.0, 3.0, 4.0, 5.0], median = 3.0
        assert!(approx_eq_f64(buf.median(), 3.0, 1e-10));
    }

    #[test]
    fn circular_buffer_median_even() {
        let mut buf = CircularBuffer::new(4);
        for v in [1.0, 2.0, 3.0, 4.0] {
            buf.push(v);
        }
        // sorted: [1.0, 2.0, 3.0, 4.0], median = (2.0+3.0)/2 = 2.5
        assert!(approx_eq_f64(buf.median(), 2.5, 1e-10));
    }

    #[test]
    fn circular_buffer_median_empty() {
        let buf = CircularBuffer::new(5);
        assert_eq!(buf.median(), 0.0);
    }

    #[test]
    fn circular_buffer_mad() {
        let mut buf = CircularBuffer::new(5);
        for v in [1.0, 2.0, 3.0, 4.0, 5.0] {
            buf.push(v);
        }
        // median=3.0, deviations=[2,1,0,1,2], sorted=[0,1,1,2,2], mad=1.0
        assert!(approx_eq_f64(buf.mad(), 1.0, 1e-10));
    }

    #[test]
    fn circular_buffer_max() {
        let mut buf = CircularBuffer::new(5);
        for v in [1.0, 5.0, 3.0] {
            buf.push(v);
        }
        assert!(approx_eq_f64(buf.max(), 5.0, 1e-10));
    }

    #[test]
    fn circular_buffer_max_empty() {
        let buf = CircularBuffer::new(5);
        assert_eq!(buf.max(), 0.0);
    }

    #[test]
    fn circular_buffer_mean() {
        let mut buf = CircularBuffer::new(5);
        for v in [2.0, 4.0, 6.0] {
            buf.push(v);
        }
        assert!(approx_eq_f64(buf.mean(), 4.0, 1e-10));
    }

    // ---- KalmanBpm tests ----

    #[test]
    fn kalman_first_measurement_returns_raw() {
        let mut k = KalmanBpm::new();
        let bpm = k.update(120.0, 0.5);
        assert!(approx_eq_f64(bpm, 120.0, 1e-6));
    }

    #[test]
    fn kalman_stable_input_stays_near() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8);
        for _ in 0..50 {
            let bpm = k.update(120.0, 0.8);
            assert!((bpm - 120.0).abs() < 5.0, "got {}", bpm);
        }
    }

    #[test]
    fn kalman_octave_snap() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8);
        for _ in 0..10 {
            k.update(120.0, 0.8);
        }
        // Now feed 240 (octave double) — should snap back to 120
        let bpm = k.update(240.0, 0.8);
        assert!((bpm - 120.0).abs() < 10.0, "expected near 120, got {}", bpm);
    }

    #[test]
    fn kalman_octave_escape() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8);
        // Feed 240 for 60 frames — should eventually escape snap
        for _ in 0..60 {
            k.update(240.0, 0.8);
        }
        let bpm = k.update(240.0, 0.8);
        // After escape, should be near 240
        assert!((bpm - 240.0).abs() < 30.0, "expected near 240, got {}", bpm);
    }

    #[test]
    fn kalman_divergence_reset() {
        let mut k = KalmanBpm::new();
        k.update(120.0, 0.8);
        for _ in 0..5 {
            k.update(120.0, 0.8);
        }
        // Feed completely different BPM — should reset after 15 frames
        for _ in 0..20 {
            k.update(80.0, 0.8);
        }
        let bpm = k.update(80.0, 0.8);
        assert!((bpm - 80.0).abs() < 15.0, "expected near 80, got {}", bpm);
    }

    // ---- OnsetDetector tests ----

    #[test]
    fn onset_silence_gate() {
        let mut od = OnsetDetector::new(44100.0, 50, 400);
        let bass = vec![0.0; 2049]; // 4096-pt fft
        let mid = vec![0.0; 513]; // 1024-pt fft
        let high = vec![0.0; 257]; // 512-pt fft
        let (is_onset, strength, _) = od.process(&bass, &mid, &high, 0.0);
        assert!(!is_onset);
        assert!(approx_eq(strength, 0.0, 1e-6));
    }

    #[test]
    fn onset_sustained_silence() {
        let mut od = OnsetDetector::new(44100.0, 50, 400);
        let bass = vec![0.0; 2049];
        let mid = vec![0.0; 513];
        let high = vec![0.0; 257];
        for _ in 0..40 {
            od.process(&bass, &mid, &high, 0.0);
        }
        assert!(od.is_sustained_silence());
    }

    #[test]
    fn onset_bin_range() {
        let od = OnsetDetector::new(44100.0, 50, 400);
        let (lo, hi) = od.bin_range(20.0, 80.0, 4096);
        // bin_width = 44100/4096 ≈ 10.77 Hz
        // lo = round(20/10.77) = 2, hi = round(80/10.77) = 7
        assert!(lo >= 1 && lo <= 3, "lo={}", lo);
        assert!(hi >= 6 && hi <= 8, "hi={}", hi);
    }

    // ---- BeatScheduler tests ----

    #[test]
    fn scheduler_zero_confidence_onset_fires() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(0.0, 0.0, 0.0);
        let (is_beat, _, _) = bs.process(true, 0.8, 1.0, false);
        assert!(is_beat);
    }

    #[test]
    fn scheduler_silence_no_beat() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(120.0, 0.5, 0.8);
        let (is_beat, phase, _) = bs.process(false, 0.0, 1.0, true);
        assert!(!is_beat);
        assert!(approx_eq_f64(phase, 0.0, 1e-6));
    }

    #[test]
    fn scheduler_phase_in_range() {
        let mut bs = BeatScheduler::new();
        bs.update_tempo(120.0, 0.5, 0.8);
        // Trigger a beat
        bs.process(true, 0.8, 1.0, false);
        // Advance time
        for i in 1..100 {
            let t = 1.0 + (i as f64) * 0.01;
            let (_, phase, _) = bs.process(false, 0.0, t, false);
            assert!(phase >= 0.0 && phase <= 1.0, "phase={} at t={}", phase, t);
        }
    }

    // ---- Integration test ----

    /// Run a BPM convergence test with synthetic kicks at the given tempo.
    /// Returns the detected BPM after `duration_secs` seconds.
    fn run_bpm_convergence_test(target_bpm: f64, duration_secs: f64) -> f32 {
        let sample_rate = 44100.0;
        let mut detector = BeatDetector::new(sample_rate);

        let bass_len = 2049; // 4096/2 + 1
        let mid_len = 513; // 1024/2 + 1
        let high_len = 257; // 512/2 + 1

        let dt = 0.01; // 100 Hz frame rate
        let kick_interval = 60.0 / target_bpm;
        let mut last_kick = -1.0f64;
        let num_frames = (duration_secs / dt) as usize;

        let mut last_bpm = 0.0f32;

        for frame in 0..num_frames {
            let t = frame as f64 * dt;

            let is_kick_frame = (t - last_kick) >= kick_interval - dt * 0.5;
            let mut bass = vec![0.001f32; bass_len];
            let mid = vec![0.001f32; mid_len];
            let high = vec![0.001f32; high_len];

            if is_kick_frame && t >= kick_interval {
                for bin in 1..12 {
                    bass[bin] = 2.0;
                }
                last_kick = t;
            }

            let rms = if is_kick_frame { 0.5 } else { 0.05 };
            let result = detector.process(&bass, &mid, &high, rms, t);
            last_bpm = result.bpm;
        }

        eprintln!(
            "BPM convergence: target={target_bpm}, detected={last_bpm}, duration={duration_secs}s"
        );
        last_bpm
    }

    #[test]
    fn bpm_converges_120() {
        let bpm = run_bpm_convergence_test(120.0, 8.0);
        assert!(
            bpm > 102.0 && bpm < 138.0,
            "120 BPM: expected 102-138, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_90() {
        let bpm = run_bpm_convergence_test(90.0, 10.0);
        assert!(
            bpm > 72.0 && bpm < 108.0,
            "90 BPM: expected 72-108, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_140() {
        let bpm = run_bpm_convergence_test(140.0, 10.0);
        assert!(
            bpm > 112.0 && bpm < 168.0,
            "140 BPM: expected 112-168, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_170() {
        let bpm = run_bpm_convergence_test(170.0, 10.0);
        assert!(
            bpm > 136.0 && bpm < 204.0,
            "170 BPM: expected 136-204, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_200() {
        let bpm = run_bpm_convergence_test(200.0, 10.0);
        assert!(
            bpm > 160.0 && bpm < 240.0,
            "200 BPM: expected 160-240, got {bpm}"
        );
    }

    #[test]
    fn bpm_converges_230() {
        let bpm = run_bpm_convergence_test(230.0, 10.0);
        // Accept 230 BPM or half-tempo 115 BPM (prior centered at 150 favors lower octave)
        let in_range = (bpm > 92.0 && bpm < 138.0) || (bpm > 184.0 && bpm < 276.0);
        assert!(in_range,
            "230 BPM: expected 92-138 or 184-276, got {bpm}");
    }

    #[test]
    fn bpm_no_octave_double_145() {
        let bpm = run_bpm_convergence_test(145.0, 10.0);
        assert!(bpm > 116.0 && bpm < 174.0,
            "145 BPM: expected 116-174 (not 290 octave double), got {bpm}");
    }
}
