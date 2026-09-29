//! Streaming linear-interpolation resampler for the recording mirror (#79).
//!
//! An in-progress recording encodes audio at the rate it started with, and the recording ring
//! outlives device switches. When the new device runs at another rate, the analysis thread
//! converts its mono mix to the ring's rate with this before pushing. Linear interpolation, no
//! anti-alias filter: the conversions it sees are between common device rates (44.1/48/96 kHz),
//! where the folded band sits at the top of the audible range.

/// Converts a mono stream from `from` Hz to `to` Hz, one block at a time, with no allocation
/// beyond what the caller's output `Vec` already has.
pub struct LinearResampler {
    /// Input samples advanced per output sample (`from / to`).
    step: f64,
    /// Position of the next output sample, in input samples, measured from `prev`.
    pos: f64,
    /// Last input sample of the previous block, so interpolation spans block edges.
    prev: f32,
}

impl LinearResampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self {
            step: f64::from(from.max(1)) / f64::from(to.max(1)),
            pos: 1.0,
            prev: 0.0,
        }
    }

    /// Append the resampled `input` to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let n = input.len();
        if n == 0 {
            return;
        }
        // The block extended by the carried sample: e[0] = prev, e[k] = input[k-1]. An output at
        // `pos` needs e[floor(pos)] and e[floor(pos) + 1], so it is ready while pos < n.
        let at = |k: usize| if k == 0 { self.prev } else { input[k - 1] };
        while self.pos < n as f64 {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            let (a, b) = (at(i), at(i + 1));
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        self.pos -= n as f64;
        self.prev = input[n - 1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    fn run(from: u32, to: u32, input: &[f32], block: usize) -> Vec<f32> {
        let mut r = LinearResampler::new(from, to);
        let mut out = Vec::new();
        for chunk in input.chunks(block) {
            r.process(chunk, &mut out);
        }
        out
    }

    #[test]
    fn same_rate_is_identity() {
        // Exact samples, with the newest one held back until the next block arrives (it is the
        // right-hand end of the next interpolation span).
        let x: Vec<f32> = (0..1000).map(|i| (i as f32 * 0.37).sin()).collect();
        assert_eq!(run(48_000, 48_000, &x, 333), x[..x.len() - 1]);
    }

    #[test]
    fn output_length_tracks_the_rate_ratio() {
        let x = vec![0.25f32; 48_000];
        let out = run(48_000, 44_100, &x, 480);
        assert!((out.len() as i64 - 44_100).abs() <= 1, "{}", out.len());
        let up = run(44_100, 48_000, &x[..44_100], 441);
        assert!((up.len() as i64 - 48_000).abs() <= 1, "{}", up.len());
    }

    #[test]
    fn a_tone_keeps_its_pitch_across_block_edges() {
        // 1 kHz at 48 kHz → 44.1 kHz must still be 1 kHz at 44.1 kHz, sample for sample within the
        // interpolation error, whatever the block size the capture thread happens to read.
        let x: Vec<f32> = (0..48_000)
            .map(|i| (TAU * 1000.0 * i as f32 / 48_000.0).sin())
            .collect();
        for block in [1, 7, 512, 4096] {
            let out = run(48_000, 44_100, &x, block);
            for (i, &y) in out.iter().enumerate().take(44_000) {
                let want = (TAU * 1000.0 * i as f32 / 44_100.0).sin();
                assert!(
                    (y - want).abs() < 0.01,
                    "block {block}, sample {i}: {y} vs {want}"
                );
            }
        }
    }
}
