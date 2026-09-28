//! Seated reach: Go-Go arm extension (Poupyrev et al., 1996). The app is
//! stationary and a seated arm reaches about 0.7 m, so beyond a comfortable
//! reach the virtual hand travels further than the real one. Plain numbers
//! in, so it builds and tests on the desktop as well.
//!
//! The shoulder is estimated from the head (below it and out to the hand's
//! side, turned with the head's yaw). Within `threshold` of the shoulder the
//! virtual hand is the real hand; beyond it the distance grows
//! quadratically, `d + gain * (d - threshold)^2`, along the shoulder-to-palm
//! line, and the whole joint set moves by the difference. What uses the
//! result decides what follows the far hand (the sim's obstacle spheres)
//! and what stays on the real one (occlusion, pinch gestures, the panel).
//!
//! The quadratic multiplies the palm's motion by `1 + 2 * gain * (d -
//! threshold)` (about 9x near a full stretch at the defaults), tracking
//! jitter included, so the shoulder and the extension are low-passed, by
//! time constant rather than per frame so 72 and 90 Hz behave alike.

use glam::{Quat, Vec3};

/// Distance from the shoulder (meters) within which the hand is 1:1.
/// The wearer's tuning (Sep 28, seated, worn): a stretched palm sits only
/// 0.52-0.62 m from the estimated shoulder, so the first pass's 0.45 m and
/// gain 20.8 (designed for 0.70 m) reached about 1 m.
pub const THRESHOLD_M: f32 = 0.30;
/// Quadratic gain (1/m): with the defaults a palm 0.45 m from the shoulder
/// acts at 1.35 m, 0.52 m at 2.46 m, and 0.55 m reaches the 3 m cap.
pub const GAIN: f32 = 40.0;
/// The virtual hand never goes further than this from the shoulder.
const MAX_REACH_M: f32 = 3.0;
/// Shoulder estimate relative to the head: out to the hand's side, down.
const SHOULDER_OUT_M: f32 = 0.15;
const SHOULDER_DOWN_M: f32 = 0.20;
/// Low-pass time constants (seconds): the shoulder follows the head slowly
/// (the torso does not turn with every glance), the extension quickly.
const SHOULDER_TAU_S: f32 = 0.15;
const EXTENSION_TAU_S: f32 = 0.05;

/// The Go-Go mapping from the real shoulder-to-palm distance to the
/// virtual one.
pub fn extend(d: f32, threshold: f32, gain: f32) -> f32 {
    if d <= threshold {
        d
    } else {
        (d + gain * (d - threshold).powi(2)).min(MAX_REACH_M.max(d))
    }
}

/// A shoulder `out` meters to the `side` (-1 left, +1 right) of the head
/// and `down` below it, turned with the head's yaw only.
pub fn shoulder(head: Vec3, head_rot: Quat, side: f32, out: f32, down: f32) -> Vec3 {
    let ahead = head_rot * Vec3::NEG_Z;
    let flat = Vec3::new(ahead.x, 0.0, ahead.z).normalize_or(Vec3::NEG_Z);
    let right = flat.cross(Vec3::Y);
    head + right * (out * side) - Vec3::Y * down
}

/// One hand's reach this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HandReach {
    /// Add to every joint of the hand to get the virtual hand.
    pub offset: Vec3,
    /// Real shoulder-to-palm distance (meters).
    pub real_m: f32,
    /// Virtual shoulder-to-palm distance (meters).
    pub virtual_m: f32,
}

#[derive(Debug, Clone)]
pub struct Reach {
    pub threshold: f32,
    pub gain: f32,
    shoulder: [Option<Vec3>; 2],
    /// Smoothed extra distance per hand (virtual minus real).
    extra: [f32; 2],
}

impl Reach {
    pub fn new(threshold: f32, gain: f32) -> Self {
        Self {
            threshold,
            gain,
            shoulder: [None; 2],
            extra: [0.0; 2],
        }
    }

    /// Hand `h` (0 left, 1 right) with its palm at `palm` (`None` when not
    /// tracked), the head at `head` turned by `head_rot`, `dt` seconds after
    /// the last update.
    pub fn update(
        &mut self,
        h: usize,
        palm: Option<Vec3>,
        head: Vec3,
        head_rot: Quat,
        dt: f32,
    ) -> Option<HandReach> {
        let side = if h == 0 { -1.0 } else { 1.0 };
        let target = shoulder(head, head_rot, side, SHOULDER_OUT_M, SHOULDER_DOWN_M);
        let blend = |tau: f32| 1.0 - (-dt.max(0.0) / tau).exp();
        let s = match self.shoulder[h] {
            Some(s) => s.lerp(target, blend(SHOULDER_TAU_S)),
            None => target,
        };
        self.shoulder[h] = Some(s);
        let Some(palm) = palm else {
            // A hand that comes back starts at its real place.
            self.extra[h] = 0.0;
            return None;
        };
        let to_palm = palm - s;
        let d = to_palm.length();
        let extra = extend(d, self.threshold, self.gain) - d;
        self.extra[h] += (extra - self.extra[h]) * blend(EXTENSION_TAU_S);
        let dir = to_palm.normalize_or_zero();
        Some(HandReach {
            offset: dir * self.extra[h],
            real_m: d,
            virtual_m: d + self.extra[h],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 72.0;
    const HEAD: Vec3 = Vec3::new(0.0, 1.2, 0.0);

    #[test]
    fn within_the_threshold_the_hand_is_the_real_hand() {
        for d in [0.0, 0.2, THRESHOLD_M - 0.01, THRESHOLD_M] {
            assert_eq!(extend(d, THRESHOLD_M, GAIN), d);
        }
    }

    #[test]
    fn the_defaults_reach_two_and_a_half_meters_from_a_seated_stretch() {
        // 0.52 m: the wearer's 90th-percentile stretch from the shoulder.
        let at = |d: f32| extend(d, THRESHOLD_M, GAIN);
        assert!((at(0.52) - 2.456).abs() < 0.01, "{}", at(0.52));
        assert!((at(0.45) - 1.35).abs() < 0.01, "{}", at(0.45));
        assert_eq!(at(0.60), MAX_REACH_M);
    }

    #[test]
    fn the_mapping_is_continuous_monotonic_and_capped() {
        let at = |d: f32| extend(d, 0.45, 20.8);
        assert!((at(0.70) - 2.0).abs() < 0.01, "{}", at(0.70));
        assert!((at(0.60) - 1.068).abs() < 0.01, "{}", at(0.60));
        // Continuous at the threshold, monotonic, capped.
        assert!((at(THRESHOLD_M + 1e-4) - THRESHOLD_M).abs() < 1e-3);
        let mut last = 0.0;
        for i in 0..200 {
            let v = at(i as f32 * 0.01);
            assert!(v >= last, "not monotonic at {i}");
            last = v;
        }
        assert!(at(1.5) <= MAX_REACH_M);
    }

    #[test]
    fn the_right_shoulder_is_right_of_and_below_the_head() {
        let s = shoulder(HEAD, Quat::IDENTITY, 1.0, 0.15, 0.2);
        assert!((s - Vec3::new(0.15, 1.0, 0.0)).length() < 1e-5, "{s}");
        // Turned 90 degrees left (facing -X), right is -Z.
        let turned = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let s = shoulder(HEAD, turned, 1.0, 0.15, 0.2);
        assert!((s - Vec3::new(0.0, 1.0, -0.15)).length() < 1e-5, "{s}");
    }

    #[test]
    fn an_outstretched_hand_acts_far_along_the_arm() {
        let mut r = Reach::new(0.45, 20.8);
        let shoulder = Vec3::new(0.15, 1.0, 0.0);
        // Palm 0.7 m straight ahead of the right shoulder, held for 1 s.
        let palm = shoulder + Vec3::new(0.0, 0.0, -0.7);
        let mut last = None;
        for _ in 0..72 {
            last = r.update(1, Some(palm), HEAD, Quat::IDENTITY, DT);
        }
        let hand = last.expect("tracked");
        assert!((hand.real_m - 0.7).abs() < 1e-4);
        assert!((hand.virtual_m - 2.0).abs() < 0.02, "{}", hand.virtual_m);
        // Along the arm: straight ahead of the right shoulder.
        let v = palm + hand.offset;
        assert!((v.x - 0.15).abs() < 1e-3 && (v.z + 2.0).abs() < 0.02, "{v}");
    }

    #[test]
    fn a_relaxed_hand_has_no_offset() {
        let mut r = Reach::new(THRESHOLD_M, GAIN);
        // 0.23 m from the right shoulder: resting near the body.
        let palm = Vec3::new(0.2, 0.9, -0.2);
        for _ in 0..72 {
            let hand = r
                .update(1, Some(palm), HEAD, Quat::IDENTITY, DT)
                .expect("tracked");
            assert_eq!(hand.offset, Vec3::ZERO);
        }
    }

    #[test]
    fn the_extension_settles_by_time_not_by_frame_count() {
        // 1/18 s of a full stretch (4 frames at 72 Hz, 5 at 90) lands at
        // the same place.
        let run = |hz: f32, frames: usize| {
            let mut r = Reach::new(0.45, 20.8);
            let palm = Vec3::new(0.15, 1.0, -0.7);
            let mut v = 0.0;
            for _ in 0..frames {
                v = r
                    .update(1, Some(palm), HEAD, Quat::IDENTITY, 1.0 / hz)
                    .expect("t")
                    .virtual_m;
            }
            v
        };
        let (a, b) = (run(72.0, 4), run(90.0, 5));
        // Part way there, and the same at both rates.
        assert!(a > 1.0 && a < 1.9, "{a}");
        assert!((a - b).abs() < 1e-3, "{a} vs {b}");
    }

    #[test]
    fn a_hand_that_returns_starts_at_its_real_place() {
        let mut r = Reach::new(0.45, 20.8);
        let far = Vec3::new(0.15, 1.0, -0.7);
        for _ in 0..72 {
            r.update(1, Some(far), HEAD, Quat::IDENTITY, DT);
        }
        assert!(r.update(1, None, HEAD, Quat::IDENTITY, DT).is_none());
        let back = r.update(1, Some(far), HEAD, Quat::IDENTITY, DT).expect("t");
        // One frame after coming back it is still near the real hand.
        assert!(back.virtual_m < 1.2, "{}", back.virtual_m);
    }
}
