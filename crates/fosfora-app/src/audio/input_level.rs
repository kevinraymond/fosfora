//! Input trim and level metering (#84), shared between the UI and the analysis thread.
//!
//! The trim is a gain in dB applied to what the analysis sees, so a quiet line or mic
//! source can be lifted off the silence gate. It does not touch the recording mirror:
//! a recording keeps the source as it arrived. The meter reads the raw input, before the
//! trim, because clipping is a property of the source — a trim cannot undo it.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Trim range in dB.
pub const TRIM_MIN_DB: f32 = -24.0;
pub const TRIM_MAX_DB: f32 = 24.0;

/// A sample at or above this magnitude counts as clipped. Just under full scale, so a
/// limiter parked at 0 dBFS still registers but a hot, clean master does not.
pub const CLIP_LEVEL: f32 = 0.999;

/// Lock-free trim and meter state. One analysis thread writes the meter; any thread may set
/// the trim or read the meter.
#[derive(Debug, Default)]
pub struct InputLevel {
    /// Trim in dB, as `f32` bits (0 bits = 0.0 dB).
    trim_db: AtomicU32,
    /// Largest |sample| since the last [`take_peak`](Self::take_peak), as `f32` bits.
    /// Non-negative floats order the same as their bit patterns, so `fetch_max` on the bits
    /// is a float max.
    peak: AtomicU32,
    /// Clipped samples since the pipeline started.
    clips: AtomicU64,
}

impl InputLevel {
    pub fn set_trim_db(&self, db: f32) {
        // `+ 0.0` turns −0.0 into 0.0, so "no trim" has exactly one bit pattern.
        let db = if db.is_finite() {
            db.clamp(TRIM_MIN_DB, TRIM_MAX_DB) + 0.0
        } else {
            0.0
        };
        self.trim_db.store(db.to_bits(), Ordering::Relaxed);
    }

    pub fn trim_db(&self) -> f32 {
        f32::from_bits(self.trim_db.load(Ordering::Relaxed))
    }

    /// The trim as a linear gain, or `None` at 0 dB so the caller can skip the multiply.
    pub fn gain(&self) -> Option<f32> {
        let bits = self.trim_db.load(Ordering::Relaxed);
        (bits != 0).then(|| 10f32.powf(f32::from_bits(bits) / 20.0))
    }

    /// Meter one block of raw input (any channel layout).
    pub fn record(&self, samples: &[f32]) {
        let mut peak = 0.0f32;
        let mut clipped = 0u64;
        for &s in samples {
            let a = s.abs();
            peak = peak.max(a);
            if a >= CLIP_LEVEL {
                clipped += 1;
            }
        }
        self.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
        if clipped > 0 {
            self.clips.fetch_add(clipped, Ordering::Relaxed);
        }
    }

    /// The largest |sample| since the previous call, and reset it.
    pub fn take_peak(&self) -> f32 {
        f32::from_bits(self.peak.swap(0, Ordering::Relaxed))
    }

    /// Clipped samples counted so far.
    pub fn clips(&self) -> u64 {
        self.clips.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_is_clamped_and_converts_to_gain() {
        let l = InputLevel::default();
        assert_eq!(l.trim_db(), 0.0);
        assert_eq!(l.gain(), None);
        l.set_trim_db(6.0);
        assert!((l.gain().unwrap() - 1.9953).abs() < 1e-3);
        l.set_trim_db(-0.0);
        assert_eq!(l.gain(), None);
        l.set_trim_db(99.0);
        assert_eq!(l.trim_db(), TRIM_MAX_DB);
        l.set_trim_db(f32::NAN);
        assert_eq!(l.trim_db(), 0.0);
    }

    #[test]
    fn meter_holds_the_peak_until_taken_and_counts_clips() {
        let l = InputLevel::default();
        l.record(&[0.1, -0.5, 0.2]);
        l.record(&[0.3, -0.2]);
        assert_eq!(l.take_peak(), 0.5);
        assert_eq!(l.take_peak(), 0.0);
        assert_eq!(l.clips(), 0);
        l.record(&[1.0, -1.0, 0.9985, 0.999]);
        assert_eq!(l.clips(), 3);
        assert_eq!(l.take_peak(), 1.0);
    }
}
