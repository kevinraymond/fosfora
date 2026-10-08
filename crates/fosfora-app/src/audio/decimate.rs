//! Power-of-two decimation of high-rate input down to the analysis rate (#53).
//!
//! The FFT sizes, the onset bands and the kick model are all fixed in samples, so at 96 kHz a
//! 4096-point frame spans half the time and each bin twice the bandwidth it does at 48 kHz:
//! `sub_bass` gets about two bins. Rather than retune every stage per rate, input above
//! 88.2 kHz-ish is halved (and halved again) until it sits in 44.1–88.2 kHz, so the chain only
//! ever sees the rates it was tuned at. 88.2/96 kHz become 44.1/48 kHz; 176.4/192 kHz the same.
//!
//! Each halving is a windowed-sinc half-band low-pass followed by dropping every other frame.
//! The filter is linear phase, so it delays the analysis by [`TAPS`]`/2` input frames per stage
//! (~0.3 ms at 96 kHz) and nothing else.

use std::f64::consts::PI;

/// Lowest rate decimation stops at: a source is halved only while the result stays at or above
/// this, so 44.1 and 48 kHz devices pass straight through.
const MIN_ANALYSIS_RATE: u32 = 44_100;

/// Taps of each half-band stage. Blackman-windowed, so its transition band is ~5.5/63 of the
/// input rate wide, centred on the output Nyquist: at 96 → 48 kHz it passes up to ~20 kHz and
/// stops by ~28 kHz, so only the 24–28 kHz sliver folds back, and only into the top octave the
/// analysis barely reads.
const TAPS: usize = 63;

/// How many times `rate` is halved before analysis: the largest power of two that keeps the
/// result at or above [`MIN_ANALYSIS_RATE`]. 1 for anything up to 88.2 kHz.
pub fn factor(rate: u32) -> usize {
    let mut factor = 1usize;
    while rate / (factor as u32 * 2) >= MIN_ANALYSIS_RATE {
        factor *= 2;
    }
    factor
}

/// The rate the analysis chain runs at for an input at `rate`.
pub fn analysis_rate(rate: u32) -> u32 {
    rate / factor(rate) as u32
}

/// Half-band low-pass taps, normalised to unit DC gain.
fn halfband_taps() -> [f32; TAPS] {
    let mid = (TAPS - 1) as f64 / 2.0;
    let mut h = [0.0f64; TAPS];
    for (i, tap) in h.iter_mut().enumerate() {
        let x = i as f64 - mid;
        // sinc at a quarter of the input rate (the output Nyquist).
        let sinc = if x == 0.0 {
            0.5
        } else {
            (0.5 * PI * x).sin() / (PI * x)
        };
        let n = i as f64 / (TAPS - 1) as f64;
        let blackman = 0.42 - 0.5 * (2.0 * PI * n).cos() + 0.08 * (4.0 * PI * n).cos();
        *tap = sinc * blackman;
    }
    let sum: f64 = h.iter().sum();
    let mut out = [0.0f32; TAPS];
    for (o, v) in out.iter_mut().zip(h) {
        *o = (v / sum) as f32;
    }
    out
}

/// One ÷2 stage over interleaved L,R.
struct Stage {
    /// Interleaved history: the last `TAPS - 1` input frames, then this block's frames.
    hist: Vec<f32>,
    /// Whether the next input frame is one whose output is kept.
    emit: bool,
}

impl Stage {
    fn new() -> Self {
        Self {
            hist: vec![0.0; (TAPS - 1) * 2],
            emit: true,
        }
    }

    fn process(&mut self, taps: &[f32; TAPS], input: &[f32], out: &mut Vec<f32>) {
        for frame in input.chunks_exact(2) {
            self.hist.extend_from_slice(frame);
            if self.emit {
                let window = &self.hist[self.hist.len() - TAPS * 2..];
                let (mut l, mut r) = (0.0f32, 0.0f32);
                for (t, f) in taps.iter().zip(window.chunks_exact(2)) {
                    l += t * f[0];
                    r += t * f[1];
                }
                out.push(l);
                out.push(r);
            }
            self.emit = !self.emit;
        }
        // Keep only what the next block's first output reaches back into.
        let keep = (TAPS - 1) * 2;
        let excess = self.hist.len() - keep;
        self.hist.drain(..excess);
    }
}

/// Streams interleaved L,R at the device rate down to [`analysis_rate`]. With a factor of 1 it
/// copies straight through.
pub struct StereoDecimator {
    taps: [f32; TAPS],
    stages: Vec<Stage>,
    /// Output of every stage but the last.
    scratch: Vec<f32>,
}

impl StereoDecimator {
    pub fn new(rate: u32) -> Self {
        let stages = factor(rate).trailing_zeros() as usize;
        Self {
            taps: halfband_taps(),
            stages: (0..stages).map(|_| Stage::new()).collect(),
            scratch: Vec::new(),
        }
    }

    /// Whether this decimator changes the rate at all.
    pub fn is_active(&self) -> bool {
        !self.stages.is_empty()
    }

    /// Replace `out` with `input` decimated. `input` must be whole L,R frames.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        let Some((last, rest)) = self.stages.split_last_mut() else {
            out.extend_from_slice(input);
            return;
        };
        if rest.is_empty() {
            last.process(&self.taps, input, out);
            return;
        }
        // Ping-pong between `scratch` and `out`, finishing in `out`.
        let mut src = std::mem::take(&mut self.scratch);
        src.clear();
        src.extend_from_slice(input);
        for stage in rest {
            out.clear();
            stage.process(&self.taps, &src, out);
            std::mem::swap(&mut src, out);
        }
        out.clear();
        last.process(&self.taps, &src, out);
        self.scratch = src;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factor_keeps_the_analysis_rate_between_44_1_and_88_2_khz() {
        for (rate, f) in [
            (16_000, 1),
            (44_100, 1),
            (48_000, 1),
            (64_000, 1),
            (88_200, 2),
            (96_000, 2),
            (176_400, 4),
            (192_000, 4),
            (384_000, 8),
        ] {
            assert_eq!(factor(rate), f, "{rate} Hz");
        }
        assert_eq!(analysis_rate(96_000), 48_000);
        assert_eq!(analysis_rate(176_400), 44_100);
    }

    #[test]
    fn passes_through_when_no_decimation_is_needed() {
        let mut d = StereoDecimator::new(48_000);
        assert!(!d.is_active());
        let input = [0.1, -0.2, 0.3, -0.4];
        let mut out = Vec::new();
        d.process(&input, &mut out);
        assert_eq!(out, input);
    }

    /// Amplitude of `tone` Hz in channel `ch` of the steady-state output, by correlation.
    fn tone_gain(rate: u32, tone: f64, ch: usize) -> f64 {
        let mut d = StereoDecimator::new(rate);
        let n = rate as usize; // one second
        let input: Vec<f32> = (0..n)
            .flat_map(|i| {
                let s = (2.0 * PI * tone * i as f64 / f64::from(rate)).sin() as f32;
                // Left carries the tone, right its negation, so channel crosstalk would show.
                [s, -s]
            })
            .collect();
        // Odd block sizes, to exercise the carried phase and history.
        let mut out = Vec::new();
        let mut all = Vec::new();
        for block in input.chunks(2 * 1001) {
            d.process(block, &mut out);
            all.extend_from_slice(&out);
        }
        let out_rate = f64::from(analysis_rate(rate));
        assert_eq!(all.len() / 2, n / factor(rate));
        // Skip the filter's start-up.
        let frames: Vec<f64> = all.chunks_exact(2).map(|f| f64::from(f[ch])).collect();
        let start = 200;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in frames.iter().enumerate().skip(start) {
            let ph = 2.0 * PI * tone * i as f64 / out_rate;
            re += v * ph.cos();
            im += v * ph.sin();
        }
        let len = (frames.len() - start) as f64;
        2.0 * (re * re + im * im).sqrt() / len
    }

    #[test]
    fn keeps_the_audible_band_and_rejects_what_would_alias() {
        for rate in [96_000, 192_000] {
            for tone in [60.0, 1_000.0, 15_000.0] {
                let g = tone_gain(rate, tone, 0);
                assert!((g - 1.0).abs() < 0.01, "{rate} Hz: {tone} Hz passed at {g}");
                let g = tone_gain(rate, tone, 1);
                assert!((g - 1.0).abs() < 0.01, "{rate} Hz right: {tone} Hz at {g}");
            }
        }
        // 40 kHz at 96 kHz would fold to 8 kHz at 48 kHz; it must be gone.
        let folded = {
            let mut d = StereoDecimator::new(96_000);
            let input: Vec<f32> = (0..96_000)
                .flat_map(|i| {
                    let s = (2.0 * PI * 40_000.0 * f64::from(i) / 96_000.0).sin() as f32;
                    [s, s]
                })
                .collect();
            let mut out = Vec::new();
            d.process(&input, &mut out);
            let tail = &out[400..];
            (tail.iter().map(|v| f64::from(v * v)).sum::<f64>() / tail.len() as f64).sqrt()
        };
        assert!(folded < 1e-3, "40 kHz leaked through at RMS {folded}");
    }
}
