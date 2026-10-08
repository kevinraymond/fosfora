//! The pinch detector: thumb tip to index tip distance in, a pinch out.
//! Plain numbers, so it builds and tests on the desktop; `input.rs` feeds
//! it the located joints.
//!
//! A pinch is the tips closing under [`PINCH_ON_M`] and holding until they
//! open past [`PINCH_OFF_M`] (hysteresis, so a held pinch does not flicker
//! at the boundary). Board #3336: the wearer's resting hand, thumb and
//! index merely close, drifted under the threshold and fired the pinch,
//! and with it the hold that cycles the world effect. So a crossing counts
//! only when the tips **closed** to get there: at least [`CLOSE_DROP_M`]
//! nearer than they were within the last [`CLOSE_WINDOW_S`]. A deliberate
//! pinch from a relaxed hand (tips 20 to 30 mm apart) closes 15 mm or more
//! in a tenth of a second, and even a slow, careful one over a whole second
//! closes 10 mm in half of it; a hand settling on a desk drifts a
//! millimeter or two a second and never qualifies. The check runs on every
//! frame the tips are under the threshold, so a hand already resting close
//! that then pinches hard still fires.

use std::collections::VecDeque;

/// Thumb tip to index tip distance (meters) under which the tips are
/// pinching, and over which a held pinch releases.
pub const PINCH_ON_M: f32 = 0.015;
pub const PINCH_OFF_M: f32 = 0.030;
/// How much nearer (meters) the tips must be than at their widest within
/// the last [`CLOSE_WINDOW_S`] for a crossing to count as a pinch.
pub const CLOSE_DROP_M: f32 = 0.008;
/// The window (seconds) the closing is judged over.
pub const CLOSE_WINDOW_S: f32 = 0.50;
// A pinch opens past where it closed, the closing fits inside the release
// gap, and the window holds several frames at 72 Hz.
const _: () =
    assert!(PINCH_ON_M < PINCH_OFF_M && CLOSE_DROP_M < PINCH_OFF_M && CLOSE_WINDOW_S > 5.0 / 72.0);

/// What changed this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinchEdge {
    None,
    /// The tips closed under the threshold fast enough: a pinch.
    Began,
    /// A held pinch opened past the release threshold, or the hand was lost.
    Released,
    /// The tips are under the threshold but got there slowly; reported once
    /// per approach (the log), nothing fires.
    Rejected,
}

/// One hand's detector.
#[derive(Debug, Clone, Default)]
pub struct PinchDetector {
    pinching: bool,
    /// Recent tip distances with their age in seconds, newest last, trimmed
    /// to [`CLOSE_WINDOW_S`].
    recent: VecDeque<(f32, f32)>,
    /// A slow approach was already reported while the tips stay close.
    rejected: bool,
}

impl PinchDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the pinch is held.
    pub fn pinching(&self) -> bool {
        self.pinching
    }

    /// The hand was lost: nothing is held and the next approach is judged
    /// afresh.
    pub fn reset(&mut self) {
        self.pinching = false;
        self.recent.clear();
        self.rejected = false;
    }

    /// The runtime owns this hand's pinch this frame (its system gesture
    /// is armed or in progress, board #3336): a held pinch releases, and
    /// nothing begins until the hand is ours again, judged afresh.
    pub fn suppress(&mut self) -> PinchEdge {
        let was = self.pinching;
        self.reset();
        if was {
            PinchEdge::Released
        } else {
            PinchEdge::None
        }
    }

    /// How far (meters) the tips closed within the window: their widest
    /// recent distance minus the latest.
    pub fn recent_drop(&self) -> f32 {
        let Some(&(_, latest)) = self.recent.back() else {
            return 0.0;
        };
        self.recent.iter().map(|&(_, d)| d).fold(latest, f32::max) - latest
    }

    /// Feed this frame's tip distance (`None` when either tip is not
    /// located) and the frame's length.
    pub fn step(&mut self, d: Option<f32>, dt: f32) -> PinchEdge {
        let Some(d) = d else {
            let was = self.pinching;
            self.reset();
            return if was {
                PinchEdge::Released
            } else {
                PinchEdge::None
            };
        };
        for e in &mut self.recent {
            e.0 += dt;
        }
        while self.recent.front().is_some_and(|e| e.0 > CLOSE_WINDOW_S) {
            self.recent.pop_front();
        }
        self.recent.push_back((0.0, d));
        if self.pinching {
            if d > PINCH_OFF_M {
                self.pinching = false;
                return PinchEdge::Released;
            }
            return PinchEdge::None;
        }
        if d < PINCH_ON_M {
            if self.recent_drop() >= CLOSE_DROP_M {
                self.pinching = true;
                self.rejected = false;
                return PinchEdge::Began;
            }
            if !self.rejected {
                self.rejected = true;
                return PinchEdge::Rejected;
            }
            return PinchEdge::None;
        }
        // The tips are apart again: the next approach is judged afresh.
        self.rejected = false;
        PinchEdge::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 72.0;

    /// Run the tips from `from` to `to` meters over `seconds`, returning the
    /// edges that fired in order.
    fn sweep(p: &mut PinchDetector, from: f32, to: f32, seconds: f32) -> Vec<PinchEdge> {
        let n = (seconds / DT).round().max(1.0) as usize;
        (1..=n)
            .map(|i| {
                let d = from + (to - from) * i as f32 / n as f32;
                p.step(Some(d), DT)
            })
            .filter(|e| *e != PinchEdge::None)
            .collect()
    }

    #[test]
    fn a_quick_close_from_a_relaxed_hand_is_a_pinch() {
        let mut p = PinchDetector::new();
        assert!(sweep(&mut p, 0.025, 0.025, 0.5).is_empty());
        assert_eq!(sweep(&mut p, 0.025, 0.008, 0.1), vec![PinchEdge::Began]);
        assert!(p.pinching());
        // Held under the release threshold: nothing more fires.
        assert!(sweep(&mut p, 0.008, 0.025, 0.5).is_empty());
        assert!(p.pinching());
        assert_eq!(sweep(&mut p, 0.025, 0.040, 0.1), vec![PinchEdge::Released]);
        assert!(!p.pinching());
    }

    #[test]
    fn a_slow_drift_under_the_threshold_is_not_a_pinch() {
        let mut p = PinchDetector::new();
        // 25 mm to 10 mm over two seconds: a hand settling, not a pinch.
        let edges = sweep(&mut p, 0.025, 0.010, 2.0);
        assert_eq!(edges, vec![PinchEdge::Rejected], "{edges:?}");
        assert!(!p.pinching());
        // Resting there for a while reports nothing more.
        assert!(sweep(&mut p, 0.010, 0.010, 1.0).is_empty());
        assert!(!p.pinching());
    }

    #[test]
    fn a_hand_already_resting_close_can_still_pinch_hard() {
        let mut p = PinchDetector::new();
        assert_eq!(sweep(&mut p, 0.025, 0.014, 2.0), vec![PinchEdge::Rejected]);
        // 14 mm to 1 mm in a tenth of a second: closed 13 mm, a pinch.
        assert_eq!(sweep(&mut p, 0.014, 0.001, 0.1), vec![PinchEdge::Began]);
    }

    #[test]
    fn a_slow_careful_pinch_over_a_second_still_counts() {
        let mut p = PinchDetector::new();
        assert!(sweep(&mut p, 0.030, 0.030, 0.3).is_empty());
        // 30 mm to 3 mm over a full second: 13 mm in any half second.
        let edges = sweep(&mut p, 0.030, 0.003, 1.0);
        assert_eq!(edges, vec![PinchEdge::Began], "{edges:?}");
    }

    #[test]
    fn opening_again_judges_the_next_approach_afresh() {
        let mut p = PinchDetector::new();
        assert_eq!(sweep(&mut p, 0.025, 0.012, 2.0), vec![PinchEdge::Rejected]);
        assert!(sweep(&mut p, 0.012, 0.030, 0.3).is_empty());
        // A second slow approach is reported again, once.
        assert_eq!(sweep(&mut p, 0.030, 0.012, 2.0), vec![PinchEdge::Rejected]);
        // A quick one fires.
        assert!(sweep(&mut p, 0.012, 0.030, 0.3).is_empty());
        assert_eq!(sweep(&mut p, 0.030, 0.005, 0.08), vec![PinchEdge::Began]);
    }

    #[test]
    fn losing_the_hand_releases_and_forgets() {
        let mut p = PinchDetector::new();
        assert_eq!(sweep(&mut p, 0.030, 0.005, 0.08), vec![PinchEdge::Began]);
        assert_eq!(p.step(None, DT), PinchEdge::Released);
        assert!(!p.pinching());
        assert_eq!(p.step(None, DT), PinchEdge::None);
        assert!(p.recent_drop() <= 0.0);
        // Back in view already under the threshold: nothing is known about
        // how it got there, so it is a slow approach until it closes more.
        assert_eq!(p.step(Some(0.010), DT), PinchEdge::Rejected);
    }

    #[test]
    fn the_runtimes_gesture_takes_the_pinch_and_gives_it_back_afresh() {
        let mut p = PinchDetector::new();
        assert_eq!(sweep(&mut p, 0.030, 0.005, 0.08), vec![PinchEdge::Began]);
        assert_eq!(p.suppress(), PinchEdge::Released);
        assert!(!p.pinching());
        assert_eq!(p.suppress(), PinchEdge::None);
        // Still closed when the hand comes back: not a pinch until it
        // closes again.
        assert_eq!(p.step(Some(0.005), DT), PinchEdge::Rejected);
        assert!(sweep(&mut p, 0.005, 0.030, 0.2).is_empty());
        assert_eq!(sweep(&mut p, 0.030, 0.005, 0.08), vec![PinchEdge::Began]);
    }

    #[test]
    fn the_window_forgets_an_old_wide_sample() {
        let mut p = PinchDetector::new();
        // Wide once, then hold just over the threshold longer than the
        // window, then creep under it: the wide sample must not count.
        p.step(Some(0.060), DT);
        assert!(sweep(&mut p, 0.016, 0.016, 0.7).is_empty());
        assert_eq!(sweep(&mut p, 0.016, 0.014, 0.1), vec![PinchEdge::Rejected]);
        assert!(p.recent_drop() < CLOSE_DROP_M);
    }
}
