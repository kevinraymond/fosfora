//! Kick detection by a small tree ensemble (#3624).
//!
//! The previous kick was 30-120 Hz log-flux divided by its own 10 s P95. Bass notes are
//! rises in that band too, and the self-normalization rescaled whatever was left in it
//! once the kick stopped, so basslines read as kicks: precision .35-.49 on synthetic
//! tracks with exact labels, .38 on 23 real recordings with human labels (MDB Drums),
//! and a stream of false kicks through kickless passages.
//!
//! This detector scores every analysis hop with a gradient-boosted tree ensemble over
//! features the analyzer already has: per-band log-flux and relative level in eleven
//! bands from 30 Hz to 10 kHz (the large FFT below 250 Hz, the medium one above), the
//! flatness and peakiness of the 30-250 Hz spectrum (a kick is spread out there, a bass
//! note is tonal), for this hop and the two before it. No lookahead, so no added latency.
//!
//! The ensemble was trained on Creative Commons electronic music from the Free Music
//! Archive, CC BY and CC0 only (the track list is in `kick_model_tracks.md`), labeled by
//! separating each track with Demucs and detecting kicks on the drum stem while rejecting
//! hits whose low end is mostly bass bleed. The training pipeline is in
//! `scripts/kick_model/`. Held out, through the app's own smoothing and thresholds: MDB
//! Drums F1 .71 (precision .62, recall .82) against the old kick's .51, batida renders
//! .77 against .54. In a blind A/B on four of Kevin's own tracks an earlier version won
//! all four, including a long kickless stretch where the old kick fired 57 times and this
//! one not at all.
//!
//! `kick` keeps its old contract for every consumer: 0..1, with 0.5 meaning "a kick".
//! [`kick_from_probability`] maps the ensemble's threshold to 0.5.

use std::sync::OnceLock;

use serde::Deserialize;

/// Band edges in Hz: eleven bands, the first five on the large FFT, the rest on the medium.
const EDGES: [f64; 12] = [
    30.0, 60.0, 90.0, 130.0, 180.0, 250.0, 400.0, 700.0, 1200.0, 2500.0, 5000.0, 10000.0,
];
const N_BANDS: usize = EDGES.len() - 1;
/// Bands whose upper edge is at or below this are read off the large FFT.
const LARGE_FFT_MAX_HZ: f64 = 250.0;
/// The flatness / peakiness span on the large FFT.
const LOW_SPAN_HZ: (f64, f64) = (30.0, 250.0);
/// Features per hop: band flux, band level, low-span flatness, low-span peakiness.
const N_FRAME: usize = 2 * N_BANDS + 2;
/// This hop and the two before it.
const CONTEXT: usize = 3;
/// Ensemble inputs.
pub const N_INPUT: usize = N_FRAME * CONTEXT;

/// The ensemble as exported by `scripts/kick_model/export_model.py`: flat node arrays,
/// one root per tree. An internal node goes left when `x[feature] <= threshold`.
#[derive(Deserialize)]
struct Ensemble {
    format: u32,
    baseline: f32,
    n_features: usize,
    /// The probability the model was tuned to call a kick; mapped to `kick` = 0.5.
    kick_threshold: f32,
    roots: Vec<u32>,
    feature: Vec<u16>,
    threshold: Vec<f32>,
    left: Vec<u32>,
    right: Vec<u32>,
    value: Vec<f32>,
    leaf: Vec<u8>,
}

impl Ensemble {
    fn probability(&self, x: &[f32; N_INPUT]) -> f32 {
        let mut raw = self.baseline;
        for &root in &self.roots {
            let mut i = root as usize;
            while self.leaf[i] == 0 {
                i = if x[self.feature[i] as usize] <= self.threshold[i] {
                    self.left[i]
                } else {
                    self.right[i]
                } as usize;
            }
            raw += self.value[i];
        }
        1.0 / (1.0 + (-raw).exp())
    }
}

fn ensemble() -> &'static Ensemble {
    static ENSEMBLE: OnceLock<Ensemble> = OnceLock::new();
    ENSEMBLE.get_or_init(|| {
        let e: Ensemble = serde_json::from_str(include_str!("kick_model.json"))
            .expect("embedded kick_model.json parses");
        assert_eq!(e.format, 1, "kick_model.json format");
        assert_eq!(
            e.n_features, N_INPUT,
            "kick_model.json was trained on other features"
        );
        e
    })
}

/// Map an ensemble probability onto the `kick` scale: the tuned threshold lands on 0.5,
/// linear on either side, so the consumers' 0.5 / 0.3 hysteresis keeps its meaning.
pub fn kick_from_probability(p: f32) -> f32 {
    let t = ensemble().kick_threshold;
    if p < t {
        0.5 * p / t
    } else {
        0.5 + 0.5 * (p - t) / (1.0 - t)
    }
    .clamp(0.0, 1.0)
}

/// A band as a bin range `[lo, hi)` on one of the two spectra.
#[derive(Clone, Copy)]
struct Band {
    large: bool,
    lo: usize,
    hi: usize,
}

/// Bin index of `hz`, rounding half to even like the Python pipeline that trained it.
fn bin(hz: f64, bin_hz: f64) -> usize {
    (hz / bin_hz).round_ties_even() as usize
}

/// Per-hop kick probability. Owns the band layout for one sample rate and the two
/// previous hops' features.
pub struct KickDetector {
    bands: [Band; N_BANDS],
    low_span: (usize, usize),
    history: [[f32; N_FRAME]; CONTEXT - 1],
}

impl KickDetector {
    /// `large_fft` and `medium_fft` are the two transform sizes the analyzer runs.
    pub fn new(sample_rate: f32, large_fft: usize, medium_fft: usize) -> Self {
        let sr = f64::from(sample_rate);
        let (bin_l, bin_m) = (sr / large_fft as f64, sr / medium_fft as f64);
        let bands = std::array::from_fn(|i| {
            let (lo_hz, hi_hz) = (EDGES[i], EDGES[i + 1]);
            let large = hi_hz <= LARGE_FFT_MAX_HZ;
            let bin_hz = if large { bin_l } else { bin_m };
            let lo = bin(lo_hz, bin_hz);
            Band {
                large,
                lo: lo.max(1),
                hi: bin(hi_hz, bin_hz).max(lo + 1),
            }
        });
        Self {
            bands,
            low_span: (bin(LOW_SPAN_HZ.0, bin_l), bin(LOW_SPAN_HZ.1, bin_l)),
            history: [[0.0; N_FRAME]; CONTEXT - 1],
        }
    }

    /// Score this hop from the large and medium magnitude spectra and their previous
    /// frames. Call exactly once per hop, silent or not, so the history stays aligned.
    pub fn process(
        &mut self,
        large: &[f32],
        large_prev: &[f32],
        medium: &[f32],
        medium_prev: &[f32],
        flux_floor: f32,
    ) -> f32 {
        let frame = self.frame_features(large, large_prev, medium, medium_prev, flux_floor);
        let mut x = [0.0f32; N_INPUT];
        x[..N_FRAME].copy_from_slice(&frame);
        for (k, h) in self.history.iter().enumerate() {
            x[N_FRAME * (k + 1)..N_FRAME * (k + 2)].copy_from_slice(h);
        }
        self.history.rotate_right(1);
        self.history[0] = frame;
        ensemble().probability(&x)
    }

    fn frame_features(
        &self,
        large: &[f32],
        large_prev: &[f32],
        medium: &[f32],
        medium_prev: &[f32],
        flux_floor: f32,
    ) -> [f32; N_FRAME] {
        let mut f = [0.0f32; N_FRAME];
        let floor = f64::from(flux_floor);
        let mut levels = [0.0f64; N_BANDS];
        for (i, b) in self.bands.iter().enumerate() {
            let (cur, prev) = if b.large {
                (large, large_prev)
            } else {
                (medium, medium_prev)
            };
            let (lo, hi) = (b.lo.min(cur.len()), b.hi.min(cur.len()));
            let n = hi.saturating_sub(lo).max(1) as f64;
            let mut flux = 0.0f64;
            let mut power = 0.0f64;
            for k in lo..hi {
                let c = f64::from(cur[k]);
                let d = c.max(floor).ln() - f64::from(prev[k]).max(floor).ln();
                if d > 0.0 {
                    flux += d;
                }
                power += c * c;
            }
            f[i] = (flux / n) as f32;
            levels[i] = (power / n).max(1e-12).ln();
        }
        // Levels relative to the hop's own mean, so overall loudness drops out.
        let mean = levels.iter().sum::<f64>() / N_BANDS as f64;
        for (i, l) in levels.iter().enumerate() {
            f[N_BANDS + i] = (l - mean) as f32;
        }
        let (lo, hi) = (
            self.low_span.0.min(large.len()),
            self.low_span.1.min(large.len()),
        );
        let span = &large[lo..hi];
        let n = span.len().max(1) as f64;
        let mean_mag = span.iter().map(|&m| f64::from(m)).sum::<f64>() / n;
        let mean_log = span
            .iter()
            .map(|&m| f64::from(m).max(1e-9).ln())
            .sum::<f64>()
            / n;
        let peak = span.iter().fold(0.0f64, |a, &m| a.max(f64::from(m)));
        f[2 * N_BANDS] = (mean_log.exp() / mean_mag.max(1e-12)) as f32;
        f[2 * N_BANDS + 1] = (peak / mean_mag.max(1e-12)) as f32;
        f
    }
}
