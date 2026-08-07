//! A monotonic musical-time accumulator: counts bars from `bar_index`/`bar_phase`
//! when the beat tracker is locked, and falls back to a 2 s/bar time equivalent
//! when it is not (silence, ambient material) so bar-denominated dwell times keep
//! working. Owned per consumer — section estimator and phrase tracker each keep
//! their own, fed the same frames.

use crate::audio::features::AudioFeatures;

/// How long `bar_index` may stand still before musical time falls back to the
/// wall-clock equivalent.
const STALL_SECS: f64 = 4.0;
/// The fallback tempo: 2 seconds per bar (a 120 BPM 4/4 bar).
const FALLBACK_SECS_PER_BAR: f64 = 2.0;

pub struct BarClock {
    bars: f64,
    prev_ts: Option<f64>,
    prev_bar_index: f32,
    last_advance_ts: f64,
}

impl BarClock {
    pub fn new() -> Self {
        Self {
            bars: 0.0,
            prev_ts: None,
            prev_bar_index: 0.0,
            last_advance_ts: 0.0,
        }
    }

    /// Advance with this hop's frame; returns the accumulated bar count.
    /// Monotonic by construction (device switches reset `bar_index`, but the
    /// accumulator only ever adds).
    pub fn advance(&mut self, f: &AudioFeatures, ts: f64) -> f64 {
        let Some(prev_ts) = self.prev_ts else {
            self.prev_ts = Some(ts);
            self.prev_bar_index = f.bar_index;
            self.last_advance_ts = ts;
            return self.bars;
        };
        let dt = (ts - prev_ts).max(0.0);
        self.prev_ts = Some(ts);

        let delta = f.bar_index - self.prev_bar_index;
        if delta > 0.0 {
            // Real musical time. Cap the step: a device switch resets bar_index and
            // an uncapped negative/huge delta would corrupt the accumulator.
            self.bars += f64::from(delta.min(4.0));
            self.last_advance_ts = ts;
        } else if ts - self.last_advance_ts > STALL_SECS {
            self.bars += dt / FALLBACK_SECS_PER_BAR;
        }
        self.prev_bar_index = f.bar_index;
        self.bars
    }

    pub fn bars(&self) -> f64 {
        self.bars
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    fn frame(bar_index: f32) -> AudioFeatures {
        let mut f = AudioFeatures::zeroed();
        f.bar_index = bar_index;
        f
    }

    #[test]
    fn counts_real_bar_advances() {
        let mut c = BarClock::new();
        let mut ts = 0.0;
        for hop in 0..400 {
            // one bar every 100 hops (~1.16 s at 86 Hz)
            let bar = (hop / 100) as f32;
            c.advance(&frame(bar), ts);
            ts += 1.0 / 86.0;
        }
        assert!((c.bars() - 3.0).abs() < 1e-6);
    }

    #[test]
    fn falls_back_to_time_equivalent_when_stalled() {
        let mut c = BarClock::new();
        let mut ts = 0.0;
        // 10 s of frames with bar_index frozen: first 4 s are the stall grace,
        // the remaining 6 s accrue at 2 s/bar = 3 bars.
        for _ in 0..860 {
            c.advance(&frame(0.0), ts);
            ts += 1.0 / 86.0;
        }
        assert!((c.bars() - 3.0).abs() < 0.1, "got {}", c.bars());
    }

    #[test]
    fn device_switch_reset_does_not_rewind() {
        let mut c = BarClock::new();
        c.advance(&frame(10.0), 0.0);
        c.advance(&frame(11.0), 1.0);
        let before = c.bars();
        // bar_index resets to 0 (new device); the accumulator must not go backward.
        c.advance(&frame(0.0), 1.5);
        assert!(c.bars() >= before);
    }
}
