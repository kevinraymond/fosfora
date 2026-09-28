//! Hand poses for Murmur's hand behaviors (board #3314): a fist is a
//! predator, an open hand only pushes, and a palm turned to the ceiling or
//! two palms facing each other close together hold part of the flock. Poses
//! are read on the real hand's joints; the seated reach only moves where
//! the behavior acts. Plain numbers in, so it builds and tests on the
//! desktop as well.
//!
//! One hand's pose comes from two readings: the curl, the mean distance of
//! the four fingertips (index to little) from the palm joint, and the palm's
//! up-ness, the cosine between the palm normal (-Y of the palm joint) and
//! world up. Both thresholds have hysteresis, and a new pose takes over only
//! after its reading has lasted [`DWELL_S`], so a tracking glitch of a frame
//! or two changes nothing. Palm up wins over a fist: a cupped palm, the
//! natural posture for holding something, curls the fingers.
//!
//! Palm up is measured against world up, not toward the head. The hand menu
//! shows while the left palm faces the head (`palm_panel.rs`), and a palm
//! held low in front of the chest faces both, so the app hides the menu
//! while the left palm is up.
//!
//! Two hands hold together while both palms face each other (each normal
//! within [`FACE_ON`] of the direction to the other palm) and the palms are
//! closer than [`TOGETHER_ON_M`]: closing the hands past that distance
//! starts the hold, opening them past [`TOGETHER_OFF_M`] or turning a palm
//! away ends it. A fist on either hand rules it out.
//!
//! [`decide`] turns the poses into each hand's behavior at the far hand,
//! [`HoldTrack`] follows a hold across frames (its age and the velocity it
//! carries birds with), and [`lane_rows`] encodes the behaviors into the
//! per-hand lanes of Murmur's aux block (`murmur_xr_sim.wgsl`), where all
//! zeros is the behavior from before the lanes existed.

use glam::{Quat, Vec3};

/// Mean fingertip-to-palm distance (meters) below which a hand is a fist,
/// and above which a fist opens again. An open hand reads about 0.09 m, a
/// relaxed half-curl 0.07.
pub const FIST_ON_M: f32 = 0.055;
pub const FIST_OFF_M: f32 = 0.070;
/// Palm normal against world up (cosine): palm up above `PALM_UP_ON`
/// (about 37 degrees from vertical), no longer below `PALM_UP_OFF` (53).
pub const PALM_UP_ON: f32 = 0.8;
pub const PALM_UP_OFF: f32 = 0.6;
/// Two hands: each palm normal against the direction to the other palm
/// (cosine), and the palm-to-palm distance (meters).
pub const FACE_ON: f32 = 0.5;
pub const FACE_OFF: f32 = 0.25;
pub const TOGETHER_ON_M: f32 = 0.30;
pub const TOGETHER_OFF_M: f32 = 0.45;
/// Seconds a new reading has to last before the pose changes.
pub const DWELL_S: f32 = 0.1;
/// The hold's velocity (the carry) is low-passed with this time constant
/// (seconds): the far hand multiplies the palm's jitter.
const CARRY_TAU_S: f32 = 0.08;
/// The carry never exceeds this speed (m/s): a reach that jumps must not
/// throw the held birds.
pub const MAX_CARRY_M_S: f32 = 3.0;
/// Open hands push with a smaller pad and kick than a fist: the unworn
/// sweep (MEASURED.md, "Pose-driven hands") parts a clean channel from a
/// 0.05 m pad at far-hand speeds, while at a slow 0.5 m/s the 0.4 m/s kick
/// plows birds along (447 displaced > 0.5 m against 8 with no kick).
pub const OPEN_PAD_M: f32 = 0.05;
pub const OPEN_KICK_M_S: f32 = 0.1;
/// A palm-up hold's radius (meters), and how far above the palm its center
/// sits, in radii.
pub const HOLD_RADIUS_M: f32 = 0.25;
const HOLD_LIFT: f32 = 0.6;
/// A two-hand hold's radius is half the palms' distance, within these.
const TOGETHER_MIN_RADIUS_M: f32 = 0.10;
/// Rows of Murmur's per-hand lanes after the obstacle block: one shared
/// row, three per hand.
pub const HAND_LANE_ROWS: usize = 7;

/// What one hand is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pose {
    /// Relaxed or open: the hand only pushes.
    #[default]
    Open,
    /// Fingertips curled into the palm: a predator.
    Fist,
    /// Palm facing the ceiling: holds what is above it.
    PalmUp,
}

impl Pose {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Fist => "fist",
            Self::PalmUp => "palm up",
        }
    }
}

/// One hand's joints this frame, as far as they were located.
#[derive(Debug, Clone, Copy, Default)]
pub struct HandInput {
    /// Palm joint position and orientation.
    pub palm: Option<(Vec3, Quat)>,
    /// Index, middle, ring and little fingertips.
    pub tips: Option<[Vec3; 4]>,
}

impl HandInput {
    /// The palm normal: -Y of the palm joint (`XR_EXT_hand_tracking`: +Y
    /// points out of the back of the hand).
    pub fn normal(&self) -> Option<Vec3> {
        self.palm.map(|(_, q)| q * Vec3::NEG_Y)
    }

    /// Mean fingertip-to-palm distance (meters).
    pub fn curl(&self) -> Option<f32> {
        let ((palm, _), tips) = (self.palm?, self.tips?);
        Some(tips.iter().map(|t| t.distance(palm)).sum::<f32>() / 4.0)
    }
}

/// A reading that switches only after it has lasted [`DWELL_S`].
#[derive(Debug, Clone, Copy, Default)]
struct Debounced<T> {
    value: T,
    pending: Option<(T, f32)>,
}

impl<T: Copy + PartialEq> Debounced<T> {
    fn step(&mut self, raw: T, dt: f32) -> T {
        if raw == self.value {
            self.pending = None;
        } else {
            let held = match self.pending {
                Some((p, t)) if p == raw => t + dt,
                _ => dt,
            };
            if held >= DWELL_S - 1e-6 {
                self.value = raw;
                self.pending = None;
            } else {
                self.pending = Some((raw, held));
            }
        }
        self.value
    }

    fn reset(&mut self, value: T) {
        self.value = value;
        self.pending = None;
    }
}

/// The pose a hand's readings amount to, with hysteresis around `current`.
pub fn classify(current: Pose, curl: f32, up: f32) -> Pose {
    let palm_up = if current == Pose::PalmUp {
        up > PALM_UP_OFF
    } else {
        up > PALM_UP_ON
    };
    let fist = if current == Pose::Fist {
        curl < FIST_OFF_M
    } else {
        curl < FIST_ON_M
    };
    if palm_up {
        Pose::PalmUp
    } else if fist {
        Pose::Fist
    } else {
        Pose::Open
    }
}

/// This frame's poses.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PoseFrame {
    /// Each hand's pose (left, right); `None` while the hand is not tracked.
    pub pose: [Option<Pose>; 2],
    /// Both palms face each other close together.
    pub together: bool,
    /// The readings, for the log and the debug panel: curl (meters) and
    /// palm up-ness per hand, and the palm-to-palm distance.
    pub curl: [Option<f32>; 2],
    pub up: [Option<f32>; 2],
    pub apart_m: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct Poses {
    hands: [Debounced<Pose>; 2],
    together: Debounced<bool>,
}

impl Poses {
    /// Advance by one frame of `dt` seconds with both hands (left, right).
    pub fn step(&mut self, hands: [HandInput; 2], dt: f32) -> PoseFrame {
        let mut out = PoseFrame::default();
        for (h, hand) in hands.iter().enumerate() {
            let Some(normal) = hand.normal() else {
                // A lost hand comes back open.
                self.hands[h].reset(Pose::Open);
                continue;
            };
            let up = normal.dot(Vec3::Y);
            out.up[h] = Some(up);
            out.curl[h] = hand.curl();
            // Fingertips can drop out while the palm stays (curled fingers
            // hide from the cameras): hold the curl reading's pose then.
            let current = self.hands[h].value;
            let curl = out.curl[h].unwrap_or(match current {
                Pose::Fist => 0.0,
                _ => f32::MAX,
            });
            out.pose[h] = Some(self.hands[h].step(classify(current, curl, up), dt));
        }
        let facing = match (hands[0].palm, hands[1].palm) {
            (Some((pl, ql)), Some((pr, qr))) => {
                let across = (pr - pl).normalize_or_zero();
                let face = (ql * Vec3::NEG_Y)
                    .dot(across)
                    .min((qr * Vec3::NEG_Y).dot(-across));
                out.apart_m = Some(pl.distance(pr));
                Some((face, pl.distance(pr)))
            }
            _ => None,
        };
        let no_fist = out.pose.iter().all(|p| *p != Some(Pose::Fist));
        let raw = facing.is_some_and(|(face, d)| {
            no_fist
                && if self.together.value {
                    face > FACE_OFF && d < TOGETHER_OFF_M
                } else {
                    face > FACE_ON && d < TOGETHER_ON_M
                }
        });
        out.together = if facing.is_some() {
            self.together.step(raw, dt)
        } else {
            self.together.reset(false);
            false
        };
        out
    }
}

/// The velocity a hold carries its birds with: the hold center's motion,
/// low-passed by time constant (so 72 and 90 Hz behave alike) and capped.
/// A hold that starts, or a center that jumps to the other hand's hold,
/// starts from rest.
#[derive(Debug, Clone, Copy, Default)]
pub struct Carry {
    last: Option<Vec3>,
    vel: Vec3,
}

impl Carry {
    /// This frame's hold center (`None`: no hold), `dt` seconds after the
    /// last. Returns the carry velocity (m/s).
    pub fn update(&mut self, center: Option<Vec3>, dt: f32) -> Vec3 {
        let Some(c) = center else {
            *self = Self::default();
            return Vec3::ZERO;
        };
        if let Some(last) = self.last
            && dt > 0.0
        {
            let raw = ((c - last) / dt).clamp_length_max(MAX_CARRY_M_S);
            self.vel += (raw - self.vel) * (1.0 - (-dt / CARRY_TAU_S).exp());
        }
        self.last = Some(c);
        self.vel
    }
}

/// What made a hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldKind {
    PalmUp,
    /// Two palms facing each other; it rides in the left hand's lanes.
    Together,
}

/// How one hand acts on the flock this frame, before the hold is tracked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
    /// Predator strength 0..1 (times the global hand scare).
    pub scare: f32,
    /// The hand's joint spheres push birds.
    pub push: bool,
    /// Added to each joint's radius (meters).
    pub pad: f32,
    /// Outward speed (m/s) the hand's spheres give a bird they touch.
    pub kick: f32,
    /// A hold: its kind, center (reference space) and radius (meters).
    pub hold: Option<(HoldKind, Vec3, f32)>,
}

/// The pads, kicks and hold size the poses use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tuning {
    pub fist_pad: f32,
    pub fist_kick: f32,
    pub open_pad: f32,
    pub open_kick: f32,
    pub hold_radius: f32,
}

impl Plan {
    /// A hand before poses: a hawk that pushes with the fist's pad and kick.
    pub fn hawk(t: &Tuning) -> Self {
        Self {
            scare: 1.0,
            push: true,
            pad: t.fist_pad,
            kick: t.fist_kick,
            hold: None,
        }
    }

    fn open(t: &Tuning) -> Self {
        Self {
            scare: 0.0,
            pad: t.open_pad,
            kick: t.open_kick,
            ..Self::hawk(t)
        }
    }

    fn holding(t: &Tuning, hold: Option<(HoldKind, Vec3, f32)>) -> Self {
        Self {
            push: false,
            hold,
            ..Self::open(t)
        }
    }
}

/// Each hand's behavior from this frame's poses. `far_palm` is each palm's
/// position at the far hand (the seated reach applied) and its normal;
/// `menu_up` means the hand menu is showing, which owns the left hand (it
/// is an open hand meanwhile, and there is no two-hand hold).
pub fn decide(
    poses: &PoseFrame,
    far_palm: [Option<(Vec3, Vec3)>; 2],
    menu_up: bool,
    t: &Tuning,
) -> [Plan; 2] {
    if poses.together
        && !menu_up
        && let (Some((l, _)), Some((r, _)), Some(apart)) = (far_palm[0], far_palm[1], poses.apart_m)
    {
        let radius = (apart * 0.5).clamp(
            TOGETHER_MIN_RADIUS_M,
            t.hold_radius.max(TOGETHER_MIN_RADIUS_M),
        );
        let hold = (HoldKind::Together, (l + r) * 0.5, radius);
        return [Plan::holding(t, Some(hold)), Plan::holding(t, None)];
    }
    [0, 1].map(|h| {
        let pose = if h == 0 && menu_up {
            Pose::Open
        } else {
            poses.pose[h].unwrap_or_default()
        };
        match pose {
            Pose::Fist => Plan::hawk(t),
            Pose::Open => Plan::open(t),
            Pose::PalmUp => Plan::holding(
                t,
                far_palm[h].map(|(p, n)| {
                    (
                        HoldKind::PalmUp,
                        p + n * (t.hold_radius * HOLD_LIFT),
                        t.hold_radius,
                    )
                }),
            ),
        }
    })
}

/// One lane row's hold across frames: its age, the velocity it carries
/// birds with and how far it has traveled. A hold of another kind (a palm-up
/// hold becoming a two-hand one) starts over.
#[derive(Debug, Clone, Copy, Default)]
pub struct HoldTrack {
    kind: Option<HoldKind>,
    age: f32,
    carry: Carry,
    travel: f32,
    last: Option<Vec3>,
}

/// A tracked hold this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hold {
    pub center: Vec3,
    pub radius: f32,
    pub velocity: Vec3,
    /// Seconds since it began (Murmur latches birds only in its first
    /// moments).
    pub age_s: f32,
}

impl HoldTrack {
    /// This frame's hold for the row (`None`: none), `dt` seconds after the
    /// last. Returns the tracked hold, and the hold that just ended (kind,
    /// seconds, meters traveled) for the log.
    pub fn update(
        &mut self,
        hold: Option<(HoldKind, Vec3, f32)>,
        dt: f32,
    ) -> (Option<Hold>, Option<(HoldKind, f32, f32)>) {
        let kind = hold.map(|(k, ..)| k);
        let ended = match self.kind {
            Some(k) if kind != Some(k) => Some((k, self.age, self.travel)),
            _ => None,
        };
        if ended.is_some() || self.kind.is_none() {
            *self = Self {
                kind,
                ..Self::default()
            };
        } else {
            self.age += dt;
        }
        let Some((_, center, radius)) = hold else {
            return (None, ended);
        };
        if let Some(last) = self.last {
            self.travel += center.distance(last);
        }
        self.last = Some(center);
        let velocity = self.carry.update(Some(center), dt);
        let hold = Hold {
            center,
            radius,
            velocity,
            age_s: self.age,
        };
        (Some(hold), ended)
    }

    /// The hold in progress: kind and seconds so far.
    pub fn current(&self) -> Option<(HoldKind, f32)> {
        self.kind.map(|k| (k, self.age))
    }
}

/// One hand's behavior as the lanes carry it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Behavior {
    pub scare: f32,
    pub push: bool,
    pub kick: f32,
    pub hold: Option<Hold>,
}

impl Behavior {
    /// The hands before the lanes existed: full scare, pushing with the
    /// obstacle header's kick, no hold.
    pub fn before_lanes(header_kick: f32) -> Self {
        Self {
            scare: 1.0,
            push: true,
            kick: header_kick,
            hold: None,
        }
    }
}

/// Murmur's per-hand lanes (layout in `murmur_xr_sim.wgsl`) for the left
/// hand's `left_spheres` joint spheres (the obstacle set's first) and both
/// hands' behaviors, positions moved into the frame of `anchor`. Every lane
/// is stored so 0 keeps the behavior before it existed: calm = 1 - scare,
/// still = 1 - push, kick calm = 1 - kick / `header_kick` (the obstacle
/// header's kick, which a hand before the lanes used).
pub fn lane_rows(
    left_spheres: u32,
    hands: [Behavior; 2],
    header_kick: f32,
    anchor: Vec3,
) -> [[f32; 4]; HAND_LANE_ROWS] {
    let mut rows = [[0.0; 4]; HAND_LANE_ROWS];
    rows[0][0] = f32::from_bits(left_spheres);
    for (h, b) in hands.iter().enumerate() {
        let kick_calm = if header_kick > 0.0 {
            1.0 - b.kick / header_kick
        } else {
            0.0
        };
        let base = 1 + 3 * h;
        rows[base] = [
            1.0 - b.scare.clamp(0.0, 1.0),
            if b.push { 0.0 } else { 1.0 },
            kick_calm,
            if b.hold.is_some() { 1.0 } else { 0.0 },
        ];
        if let Some(hold) = b.hold {
            let c = hold.center - anchor;
            rows[base + 1] = [c.x, c.y, c.z, hold.radius];
            let v = hold.velocity;
            rows[base + 2] = [v.x, v.y, v.z, hold.age_s];
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 72.0;

    /// A hand at `at` with its palm normal along `normal` and its
    /// fingertips `curl` meters from the palm.
    fn hand(at: Vec3, normal: Vec3, curl: f32) -> HandInput {
        let rot = Quat::from_rotation_arc(Vec3::NEG_Y, normal.normalize());
        // Fingertips spread around the palm, all `curl` away.
        let tips =
            [0.0f32, 0.3, 0.6, 0.9].map(|a| at + rot * Vec3::new(a.sin(), 0.0, -a.cos()) * curl);
        HandInput {
            palm: Some((at, rot)),
            tips: Some(tips),
        }
    }

    const OPEN_M: f32 = 0.09;
    const FIST_M: f32 = 0.04;
    const DOWN: Vec3 = Vec3::NEG_Y;
    const L: Vec3 = Vec3::new(-0.2, 1.0, -0.4);
    const R: Vec3 = Vec3::new(0.2, 1.0, -0.4);

    fn run(p: &mut Poses, hands: [HandInput; 2], s: f32, dt: f32) -> PoseFrame {
        let mut out = PoseFrame::default();
        for _ in 0..(s / dt).round() as usize {
            out = p.step(hands, dt);
        }
        out
    }

    #[test]
    fn the_readings_classify_with_hysteresis() {
        assert_eq!(classify(Pose::Open, OPEN_M, 0.0), Pose::Open);
        assert_eq!(classify(Pose::Open, FIST_M, 0.0), Pose::Fist);
        // Between the fist thresholds a hand keeps what it was.
        assert_eq!(classify(Pose::Open, 0.062, 0.0), Pose::Open);
        assert_eq!(classify(Pose::Fist, 0.062, 0.0), Pose::Fist);
        assert_eq!(classify(Pose::Fist, 0.075, 0.0), Pose::Open);
        // Palm up, the same way.
        assert_eq!(classify(Pose::Open, OPEN_M, 0.9), Pose::PalmUp);
        assert_eq!(classify(Pose::Open, OPEN_M, 0.7), Pose::Open);
        assert_eq!(classify(Pose::PalmUp, OPEN_M, 0.7), Pose::PalmUp);
        assert_eq!(classify(Pose::PalmUp, OPEN_M, 0.5), Pose::Open);
        // A cupped palm up holds; it is not a fist.
        assert_eq!(classify(Pose::Open, FIST_M, 0.9), Pose::PalmUp);
    }

    #[test]
    fn a_fist_reads_as_a_fist_after_the_dwell() {
        let mut p = Poses::default();
        let open = hand(R, DOWN, OPEN_M);
        let fist = hand(R, DOWN, FIST_M);
        let f = run(&mut p, [HandInput::default(), open], 0.5, DT);
        assert_eq!(f.pose, [None, Some(Pose::Open)]);
        assert!((f.curl[1].unwrap() - OPEN_M).abs() < 1e-4);
        // Not yet at half the dwell, then a fist.
        let f = run(&mut p, [HandInput::default(), fist], DWELL_S * 0.5, DT);
        assert_eq!(f.pose[1], Some(Pose::Open));
        let f = run(&mut p, [HandInput::default(), fist], DWELL_S, DT);
        assert_eq!(f.pose[1], Some(Pose::Fist));
    }

    #[test]
    fn the_dwell_is_time_not_frames() {
        let fist = hand(R, DOWN, FIST_M);
        let open = hand(R, DOWN, OPEN_M);
        // Just past the dwell becomes a fist at 72 and 90 Hz alike; just
        // short of it does not.
        for hz in [72.0f32, 90.0] {
            let dt = 1.0 / hz;
            let frames = |s: f32| (s / dt).ceil() as usize;
            let mut p = Poses::default();
            run(&mut p, [HandInput::default(), open], 0.2, dt);
            let mut pose = None;
            for _ in 0..frames(DWELL_S) {
                pose = p.step([HandInput::default(), fist], dt).pose[1];
            }
            assert_eq!(pose, Some(Pose::Fist), "{hz} Hz");
            let mut p = Poses::default();
            run(&mut p, [HandInput::default(), open], 0.2, dt);
            for _ in 0..frames(DWELL_S) - 2 {
                pose = p.step([HandInput::default(), fist], dt).pose[1];
            }
            assert_eq!(pose, Some(Pose::Open), "{hz} Hz");
        }
    }

    #[test]
    fn a_one_frame_glitch_changes_nothing() {
        let mut p = Poses::default();
        let fist = hand(R, DOWN, FIST_M);
        let open = hand(R, DOWN, OPEN_M);
        run(&mut p, [HandInput::default(), fist], 0.5, DT);
        for i in 0..200 {
            // Every fifth frame reads open (a tracking glitch).
            let h = if i % 5 == 0 { open } else { fist };
            assert_eq!(
                p.step([HandInput::default(), h], DT).pose[1],
                Some(Pose::Fist),
                "frame {i}"
            );
        }
    }

    #[test]
    fn a_palm_to_the_ceiling_is_palm_up_and_a_tilt_keeps_it() {
        let mut p = Poses::default();
        let f = run(
            &mut p,
            [hand(L, Vec3::Y, OPEN_M), HandInput::default()],
            0.3,
            DT,
        );
        assert_eq!(f.pose[0], Some(Pose::PalmUp));
        // Tilted 45 degrees (cos 0.71): still up, though it would not start.
        let tilt = Vec3::new(1.0, 1.0, 0.0);
        let f = run(
            &mut p,
            [hand(L, tilt, OPEN_M), HandInput::default()],
            0.3,
            DT,
        );
        assert_eq!(f.pose[0], Some(Pose::PalmUp));
        let mut fresh = Poses::default();
        let f = run(
            &mut fresh,
            [hand(L, tilt, OPEN_M), HandInput::default()],
            0.3,
            DT,
        );
        assert_eq!(f.pose[0], Some(Pose::Open));
    }

    #[test]
    fn a_palm_toward_the_face_is_not_palm_up() {
        // The hand menu's posture: the left palm raised in front of the
        // face, turned toward the eyes (normal mostly horizontal, tilted up
        // a little). It faces the head (the menu shows) but not the ceiling.
        let head = Vec3::new(0.0, 1.25, 0.0);
        let palm = Vec3::new(-0.1, 1.1, -0.3);
        let to_head = (head - palm).normalize();
        assert!(to_head.dot(Vec3::Y) < PALM_UP_ON, "{to_head}");
        let mut p = Poses::default();
        let f = run(
            &mut p,
            [hand(palm, to_head, OPEN_M), HandInput::default()],
            0.5,
            DT,
        );
        assert_eq!(f.pose[0], Some(Pose::Open));
    }

    #[test]
    fn a_lost_hand_comes_back_open() {
        let mut p = Poses::default();
        run(
            &mut p,
            [HandInput::default(), hand(R, DOWN, FIST_M)],
            0.5,
            DT,
        );
        let f = p.step([HandInput::default(); 2], DT);
        assert_eq!(f.pose, [None, None]);
        let f = p.step([HandInput::default(), hand(R, DOWN, FIST_M)], DT);
        assert_eq!(f.pose[1], Some(Pose::Open));
    }

    #[test]
    fn fingertips_dropping_out_keep_the_pose() {
        let mut p = Poses::default();
        run(
            &mut p,
            [HandInput::default(), hand(R, DOWN, FIST_M)],
            0.5,
            DT,
        );
        let no_tips = HandInput {
            tips: None,
            ..hand(R, DOWN, FIST_M)
        };
        let f = run(&mut p, [HandInput::default(), no_tips], 0.5, DT);
        assert_eq!(f.pose[1], Some(Pose::Fist));
    }

    #[test]
    fn two_palms_facing_each_other_close_together_hold() {
        let mut p = Poses::default();
        // 0.4 m apart, palms facing: too far to start.
        let (l, r) = (Vec3::new(-0.2, 1.0, -0.4), Vec3::new(0.2, 1.0, -0.4));
        let f = run(
            &mut p,
            [hand(l, Vec3::X, OPEN_M), hand(r, Vec3::NEG_X, OPEN_M)],
            0.3,
            DT,
        );
        assert!(!f.together);
        assert!((f.apart_m.unwrap() - 0.4).abs() < 1e-4);
        // Closing to 0.2 m starts it; opening back to 0.4 m keeps it (under
        // the release distance); 0.5 m ends it.
        let at = |d: f32| {
            [
                hand(Vec3::new(-d / 2.0, 1.0, -0.4), Vec3::X, OPEN_M),
                hand(Vec3::new(d / 2.0, 1.0, -0.4), Vec3::NEG_X, OPEN_M),
            ]
        };
        assert!(run(&mut p, at(0.2), 0.3, DT).together);
        assert!(run(&mut p, at(0.4), 0.3, DT).together);
        assert!(!run(&mut p, at(0.5), 0.3, DT).together);
    }

    #[test]
    fn palms_turned_away_or_a_fist_do_not_hold_together() {
        let near = |nl: Vec3, nr: Vec3, curl_r: f32| {
            [
                hand(Vec3::new(-0.1, 1.0, -0.4), nl, OPEN_M),
                hand(Vec3::new(0.1, 1.0, -0.4), nr, curl_r),
            ]
        };
        // Both palms down (hands side by side on a desk).
        let mut p = Poses::default();
        assert!(!run(&mut p, near(DOWN, DOWN, OPEN_M), 0.5, DT).together);
        // Backs of the hands toward each other.
        let mut p = Poses::default();
        assert!(!run(&mut p, near(Vec3::NEG_X, Vec3::X, OPEN_M), 0.5, DT).together);
        // Facing, but the right hand is a fist.
        let mut p = Poses::default();
        assert!(!run(&mut p, near(Vec3::X, Vec3::NEG_X, FIST_M), 0.5, DT).together);
    }

    const TUNING: Tuning = Tuning {
        fist_pad: 0.10,
        fist_kick: 0.4,
        open_pad: OPEN_PAD_M,
        open_kick: OPEN_KICK_M_S,
        hold_radius: HOLD_RADIUS_M,
    };

    #[test]
    fn the_hands_before_the_lanes_encode_as_zeros() {
        let rows = lane_rows(
            0,
            [Behavior::before_lanes(0.4); 2],
            0.4,
            Vec3::new(1.0, 2.0, 3.0),
        );
        assert_eq!(rows, [[0.0; 4]; HAND_LANE_ROWS]);
        // No header kick: nothing to scale, the kick calm stays 0.
        let rows = lane_rows(0, [Behavior::before_lanes(0.0); 2], 0.0, Vec3::ZERO);
        assert_eq!(rows, [[0.0; 4]; HAND_LANE_ROWS]);
    }

    #[test]
    fn the_lanes_carry_each_hand_and_the_hold_relative_to_the_anchor() {
        let anchor = Vec3::new(0.0, 1.0, 0.0);
        let open = Behavior {
            scare: 0.0,
            push: true,
            kick: 0.1,
            hold: None,
        };
        let holding = Behavior {
            scare: 0.0,
            push: false,
            kick: 0.1,
            hold: Some(Hold {
                center: Vec3::new(0.3, 1.4, -1.0),
                radius: 0.25,
                velocity: Vec3::new(0.5, 0.0, 0.0),
                age_s: 0.05,
            }),
        };
        let rows = lane_rows(26, [open, holding], 0.4, anchor);
        assert_eq!(rows[0][0].to_bits(), 26);
        assert_eq!(rows[1], [1.0, 0.0, 0.75, 0.0]);
        assert_eq!(rows[2], [0.0; 4]);
        assert_eq!(rows[4], [1.0, 1.0, 0.75, 1.0]);
        let c = rows[5];
        assert!((Vec3::new(c[0], c[1], c[2]) - Vec3::new(0.3, 0.4, -1.0)).length() < 1e-6);
        assert_eq!(c[3], 0.25);
        assert_eq!(rows[6], [0.5, 0.0, 0.0, 0.05]);
    }

    #[test]
    fn a_fist_is_the_hawk_an_open_hand_only_pushes_a_palm_up_holds() {
        let frame = PoseFrame {
            pose: [Some(Pose::Fist), Some(Pose::Open)],
            ..PoseFrame::default()
        };
        let far = [Some((L, Vec3::Y)), Some((R, DOWN))];
        let [l, r] = decide(&frame, far, false, &TUNING);
        assert_eq!(l, Plan::hawk(&TUNING));
        assert_eq!(
            (r.scare, r.push, r.pad, r.kick),
            (0.0, true, OPEN_PAD_M, OPEN_KICK_M_S)
        );
        assert_eq!(r.hold, None);
        let frame = PoseFrame {
            pose: [Some(Pose::PalmUp), None],
            ..PoseFrame::default()
        };
        let [l, _] = decide(&frame, far, false, &TUNING);
        assert!(!l.push && l.scare == 0.0);
        let (kind, center, radius) = l.hold.expect("a palm-up hold");
        assert_eq!(kind, HoldKind::PalmUp);
        // Above the far palm, along its normal.
        assert!((center - (L + Vec3::Y * HOLD_RADIUS_M * HOLD_LIFT)).length() < 1e-6);
        assert_eq!(radius, HOLD_RADIUS_M);
    }

    #[test]
    fn the_menu_owns_the_left_hand() {
        let frame = PoseFrame {
            pose: [Some(Pose::PalmUp), Some(Pose::PalmUp)],
            together: true,
            apart_m: Some(0.2),
            ..PoseFrame::default()
        };
        let far = [Some((L, Vec3::Y)), Some((R, Vec3::Y))];
        let [l, r] = decide(&frame, far, true, &TUNING);
        assert_eq!(l, Plan::open(&TUNING));
        assert_eq!(r.hold.map(|h| h.0), Some(HoldKind::PalmUp));
    }

    #[test]
    fn two_hands_hold_between_the_far_palms_in_the_left_lanes() {
        let frame = PoseFrame {
            pose: [Some(Pose::Open); 2],
            together: true,
            apart_m: Some(0.24),
            ..PoseFrame::default()
        };
        let (fl, fr) = (Vec3::new(-0.1, 1.2, -2.0), Vec3::new(0.2, 1.2, -2.1));
        let far = [Some((fl, Vec3::X)), Some((fr, Vec3::NEG_X))];
        let [l, r] = decide(&frame, far, false, &TUNING);
        let (kind, center, radius) = l.hold.expect("a two-hand hold");
        assert_eq!(kind, HoldKind::Together);
        assert!((center - (fl + fr) * 0.5).length() < 1e-6);
        // Half the REAL palms' distance: the far palms can drift apart.
        assert!((radius - 0.12).abs() < 1e-6);
        assert!(!r.push && r.hold.is_none() && r.scare == 0.0);
    }

    #[test]
    fn a_hold_track_ages_carries_and_reports_its_end() {
        let mut t = HoldTrack::default();
        let (none, ended) = t.update(None, DT);
        assert!(none.is_none() && ended.is_none());
        let at = |x: f32| Some((HoldKind::PalmUp, Vec3::new(x, 1.0, -1.0), 0.25));
        let (h, _) = t.update(at(0.0), DT);
        assert_eq!(h.expect("hold").age_s, 0.0);
        let mut last = None;
        for i in 1..=36 {
            last = t.update(at(0.5 * i as f32 * DT), DT).0;
        }
        let h = last.expect("hold");
        assert!((h.age_s - 0.5).abs() < 1e-4, "{}", h.age_s);
        assert!(h.velocity.x > 0.4, "{}", h.velocity);
        // Becoming a two-hand hold starts over; the palm-up hold is reported.
        let (h, ended) = t.update(Some((HoldKind::Together, Vec3::ZERO, 0.15)), DT);
        let (kind, secs, travel) = ended.expect("ended");
        assert_eq!(kind, HoldKind::PalmUp);
        assert!(
            (secs - 0.5).abs() < 1e-4 && (travel - 0.25).abs() < 1e-3,
            "{secs} {travel}"
        );
        let h = h.expect("new hold");
        assert_eq!((h.age_s, h.velocity), (0.0, Vec3::ZERO));
        let (_, ended) = t.update(None, DT);
        assert_eq!(ended.map(|e| e.0), Some(HoldKind::Together));
        assert_eq!(t.current(), None);
    }

    #[test]
    fn the_carry_follows_the_hold_by_time_and_starts_at_rest() {
        for hz in [72.0f32, 90.0] {
            let dt = 1.0 / hz;
            let mut c = Carry::default();
            assert_eq!(c.update(Some(Vec3::ZERO), dt), Vec3::ZERO);
            // A hold moving at 0.5 m/s along +X for half a second.
            let mut v = Vec3::ZERO;
            let n = (0.5 / dt).round() as usize;
            for i in 1..=n {
                v = c.update(Some(Vec3::X * 0.5 * i as f32 * dt), dt);
            }
            assert!((v.x - 0.5).abs() < 0.01 && v.y.abs() < 1e-5, "{hz} Hz: {v}");
            // A jump is capped, and a released hold starts over at rest.
            let v = c.update(Some(Vec3::X * 10.0), dt);
            assert!(v.length() <= MAX_CARRY_M_S + 1e-4, "{v}");
            assert_eq!(c.update(None, dt), Vec3::ZERO);
            assert_eq!(c.update(Some(Vec3::X * 10.0), dt), Vec3::ZERO);
        }
    }
}
