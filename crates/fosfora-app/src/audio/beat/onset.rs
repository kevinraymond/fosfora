//! Stage 1: SuperFlux multi-band onset detection.

use super::buffer::CircularBuffer;

// ---------------------------------------------------------------------------
// Stage 1: Multi-band onset detection
// ---------------------------------------------------------------------------

/// SuperFlux onset detection (A6 #1457): a log-magnitude filterbank spectral flux with a
/// frequency **maximum filter** applied to the reference frame (Böck & Widmer, DAFx 2013).
/// The max filter lets a partial drift ±`SUPERFLUX_MAX_BINS` bands between frames without
/// registering flux, which suppresses the phantom onsets that plain flux fires on vibrato
/// and pitch slides. The bands are contiguous and log-spaced, so they cover 250–500 Hz —
/// the snare/tom/male-vocal gap the old four-band detector left open.
const N_ONSET_BANDS: usize = 64;
const ONSET_F_MIN: f32 = 20.0;
const ONSET_F_MAX: f32 = 16000.0;
/// Frequency max-filter half-width, in bands, on the reference frame.
const SUPERFLUX_MAX_BINS: usize = 1;
/// Partition edges (Hz) splitting the bands into low / mid / high, and the weights that
/// combine their mean flux. Preserves the old kick/bass-vs-snare-vs-hat balance; `mid` now
/// also spans the reclaimed 250–500 Hz. Weights sum to 1.
const ONSET_LOW_HZ: f32 = 250.0;
const ONSET_HIGH_HZ: f32 = 2000.0;
const ONSET_W_LOW: f64 = 0.60;
const ONSET_W_MID: f64 = 0.28;
const ONSET_W_HIGH: f64 = 0.12;
/// Column names of [`BeatDetector::raw_onset_flux`]: the SuperFlux value the tempo
/// estimator reads, then the low / mid / high partition means it is the weighted sum of.
pub const ONSET_FLUX_NAMES: [&str; 4] = ["combined", "low", "mid", "high"];

pub(super) struct OnsetDetector {
    sample_rate: f32,
    threshold_mult: f32,
    threshold_ceiling: f32,

    /// `[lo_bin, hi_bin)` in the 4096-pt spectrum for each log band (computed once the
    /// spectrum length is known), the partition (0=low,1=mid,2=high) each band belongs to,
    /// and the band count per partition (for mean-flux normalization).
    band_bins: Vec<(usize, usize)>,
    band_partition: Vec<u8>,
    partition_counts: [usize; 3],
    /// Previous frame's per-band log-magnitude (the μ=1 reference the max filter runs over).
    prev_log: Vec<f64>,

    onset_history: CircularBuffer,
    long_term_history: CircularBuffer,
    silent_frames: u32,
    /// This hop's SuperFlux: combined, then the low / mid / high partition means it is
    /// the weighted sum of; zeros under silence. Read by the #4163 hop sidecar.
    pub(super) last_flux: [f64; 4],
}

impl OnsetDetector {
    pub(super) fn new(sample_rate: f32, history_size: usize, long_term_size: usize) -> Self {
        Self {
            sample_rate,
            threshold_mult: 2.0,
            threshold_ceiling: 0.5,
            band_bins: Vec::new(),
            band_partition: Vec::new(),
            partition_counts: [0; 3],
            prev_log: Vec::new(),
            onset_history: CircularBuffer::new(history_size),
            long_term_history: CircularBuffer::new(long_term_size),
            silent_frames: 0,
            last_flux: [0.0; 4],
        }
    }

    /// Build the log-spaced filterbank the first time we see the spectrum length. Each band
    /// spans at least one FFT bin (adjacent low bands may overlap a shared bin, which is
    /// harmless); its partition is decided by the band's geometric-centre frequency.
    fn ensure_bands(&mut self, num_bins: usize) {
        if self.band_bins.len() == N_ONSET_BANDS {
            return;
        }
        let bin_hz = self.sample_rate / ((num_bins - 1) * 2) as f32;
        let ratio = (ONSET_F_MAX / ONSET_F_MIN).powf(1.0 / N_ONSET_BANDS as f32);
        self.band_bins.clear();
        self.band_partition.clear();
        self.partition_counts = [0; 3];
        for b in 0..N_ONSET_BANDS {
            let f_lo = ONSET_F_MIN * ratio.powi(b as i32);
            let f_hi = ONSET_F_MIN * ratio.powi(b as i32 + 1);
            let lo = (f_lo / bin_hz).floor() as usize;
            let hi = ((f_hi / bin_hz).ceil() as usize).max(lo + 1).min(num_bins);
            self.band_bins
                .push((lo.min(num_bins.saturating_sub(1)), hi));
            let centre = (f_lo * f_hi).sqrt();
            let part = if centre < ONSET_LOW_HZ {
                0u8
            } else if centre < ONSET_HIGH_HZ {
                1u8
            } else {
                2u8
            };
            self.band_partition.push(part);
            self.partition_counts[part as usize] += 1;
        }
        self.prev_log.clear();
    }

    /// Process the multi-resolution spectra and return (is_onset, onset_strength,
    /// combined_flux). SuperFlux runs on the 4096-pt `bass_spectrum` (its fine, consistent
    /// frequency resolution is what the max filter needs); `mid_spectrum`/`high_spectrum`
    /// are part of the detector's interface but not consumed here. `loud_silent` is the
    /// perceptual silence gate from the A10 loudness meter (replaces the old raw-RMS gate).
    pub(super) fn process(
        &mut self,
        bass_spectrum: &[f32],  // 4096-pt FFT magnitudes (num_bins)
        _mid_spectrum: &[f32],  // 1024-pt FFT magnitudes (unused by SuperFlux)
        _high_spectrum: &[f32], // 512-pt FFT magnitudes (unused by SuperFlux)
        loud_silent: bool,
    ) -> (bool, f32, f64) {
        // Unified perceptual silence gate (A10 #1461).
        if loud_silent {
            self.silent_frames += 1;
            self.last_flux = [0.0; 4];
            return (false, 0.0, 0.0);
        }
        self.silent_frames = 0;

        self.ensure_bands(bass_spectrum.len());

        // Current per-band log magnitude.
        let mut cur = [0.0f64; N_ONSET_BANDS];
        for (b, &(lo, hi)) in self.band_bins.iter().enumerate() {
            let hi = hi.min(bass_spectrum.len());
            let mut e = 0.0f64;
            for &m in &bass_spectrum[lo..hi] {
                e += m as f64;
            }
            cur[b] = (e + 1e-10).ln();
        }

        // SuperFlux: half-wave-rectified difference against the frequency-max-filtered
        // reference frame, accumulated per partition.
        let mut part = [0.0f64; 3];
        if self.prev_log.len() == N_ONSET_BANDS {
            for b in 0..N_ONSET_BANDS {
                let lo = b.saturating_sub(SUPERFLUX_MAX_BINS);
                let hi = (b + SUPERFLUX_MAX_BINS).min(N_ONSET_BANDS - 1);
                let mut reference = f64::MIN;
                for &v in &self.prev_log[lo..=hi] {
                    reference = reference.max(v);
                }
                let flux = (cur[b] - reference).max(0.0);
                part[self.band_partition[b] as usize] += flux;
            }
        }
        self.prev_log.clear();
        self.prev_log.extend_from_slice(&cur);

        // Mean flux per partition, weighted into one onset value (weights sum to 1).
        let mean = |sum: f64, count: usize| if count > 0 { sum / count as f64 } else { 0.0 };
        let low = mean(part[0], self.partition_counts[0]);
        let mid = mean(part[1], self.partition_counts[1]);
        let high = mean(part[2], self.partition_counts[2]);
        let combined_flux = ONSET_W_LOW * low + ONSET_W_MID * mid + ONSET_W_HIGH * high;
        self.last_flux = [combined_flux, low, mid, high];

        self.onset_history.push(combined_flux);
        self.long_term_history.push(combined_flux);

        // Adaptive threshold: median + k * MAD (unchanged).
        let threshold = self.compute_threshold();
        let is_onset = combined_flux > threshold;

        // Normalize onset strength to 0-1.
        let recent_max = self.long_term_history.max();
        let onset_strength = (combined_flux / recent_max.max(1e-6)).min(1.0) as f32;

        (is_onset, onset_strength, combined_flux)
    }

    fn compute_threshold(&mut self) -> f64 {
        let (median, mad) = self.onset_history.median_mad();
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

    pub(super) fn is_sustained_silence(&self) -> bool {
        self.silent_frames >= 30
    }
}
