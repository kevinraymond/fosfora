//! Hands as instruments (board #3327), on the seated reach and the poses:
//! a pinch-release throws a burst that flies to where the hand points and
//! bursts on the surface it hits, and an open palm held still, facing down,
//! lifts embers toward it. Plain numbers in, so it builds and tests on the
//! desktop as well. Both act on the Flux world sims through the instrument
//! rows of the aux block ([`rows`], `flux_xr_sim.wgsl`); Murmur ignores
//! them.
//!
//! **The throw.** On a tap that is not the hand menu's, a ray from the head
//! through the far hand's pinch point (the reach applied) is cast from that
//! point against the room's boxes ([`cast`]: a slab test per oriented box,
//! the nearest entry; a box the pinch point is inside is skipped, and the
//! stage floor only counts while the room has no floor of its own). A miss
//! ends [`MISS_M`] along the ray. The burst travels from the pinch point to
//! the hit at [`THROW_SPEED_M_S`] (at least [`MIN_FLIGHT_S`]), shedding a
//! small streak every frame, then bursts [`BURST_COUNT`] particles over
//! [`BURST_FRAMES`] frames, [`BURST_RADIUS_M`] in radius, centered one
//! radius plus [`HIT_LIFT_M`] off the surface along its normal: the sphere
//! is born whole in front of the surface, and the half flying at it
//! bounces off it. One flight per hand; a new tap replaces the hand's
//! flight. The rows carry one burst a frame, so with both hands in flight
//! they take turns, frame by frame (an impact still delivers its whole
//! count: its frames count only when it is packed).
//!
//! **The lift.** A far hand that is open (`pose::Pose::Open`), palm down
//! (the palm normal against world up under [`LIFT_DOWN`]) and still (far
//! palm speed under [`LIFT_STILL_M_S`], low-passed) for [`LIFT_STILL_S`]
//! turns the lift on at the far palm, [`LIFT_RADIUS_M`] wide. It stays on
//! while the hand stays open and palm down and moves slower than
//! [`LIFT_MOVE_M_S`], so the embers can be led; its strength ramps 0 -> 1
//! over [`LIFT_RAMP_S`] and back, by time. The hand menu's left hand never
//! lifts while the menu is up. There is one lift lane: with both hands
//! lifting, the one that started later wins.
//!
//! **The pitcher** (board #3402). Turned on from the hand menu, the right
//! far palm (the left holds the menu) pours a steady stream along its palm
//! normal: [`Pitcher::step`] yields a [`Pour`] of `rate x dt` particles a
//! frame (the fraction carried over), born within [`POUR_NOZZLE_M`] of the
//! far palm and flying at the pitcher's speed within [`POUR_SPREAD_DEG`] of
//! the normal, with the cloud's full lifetime, so the stream reaches the
//! floor, lands and slides. It rides the burst rows ([`Pour::as_burst`])
//! plus one row of its own ([`pour_row`]); a throw's burst wins the rows,
//! and the pour skips that frame.

use glam::{Quat, Vec3};

use crate::pose::Pose;

/// Rows of the instrument block after Murmur's hand lanes.
pub const INSTRUMENT_ROWS: usize = 3;

/// Throw: flight speed (m/s), shortest flight (s), how far the ray looks
/// for a surface (m: a room's far wall), and where a miss ends (m along
/// the ray from the pinch point).
pub const THROW_SPEED_M_S: f32 = 6.0;
pub const MIN_FLIGHT_S: f32 = 0.12;
pub const RANGE_M: f32 = 12.0;
pub const MISS_M: f32 = 3.0;
/// The streak: particles per frame in flight, its radius (m), and how
/// bright it is by the end of the flight against the start (1): the
/// streak shrinks and dims as the throw recedes, and the impact bursts at
/// full brightness (Kevin, worn, Sep 28).
pub const STREAK_PER_FRAME: u32 = 300;
pub const STREAK_RADIUS_M: f32 = 0.03;
pub const STREAK_END_BRIGHTNESS: f32 = 0.3;
/// The impact: particles (`debug.fosfora.burstcount`), frames it is spread
/// over, radius (m), and the clearance off the surface (m).
pub const BURST_COUNT: u32 = 6000;
pub const BURST_FRAMES: u32 = 4;
pub const BURST_RADIUS_M: f32 = 0.12;
pub const HIT_LIFT_M: f32 = 0.02;

/// Lift: radius (m, `debug.fosfora.liftradius`), the palm normal's cosine
/// with world up below which the palm faces down, the far palm's speed
/// (m/s) under which it is still and over which a lift ends, how long it
/// must be still to start (s), and the strength ramp (s).
pub const LIFT_RADIUS_M: f32 = 0.35;
pub const LIFT_DOWN: f32 = -0.6;
pub const LIFT_STILL_M_S: f32 = 0.35;
pub const LIFT_MOVE_M_S: f32 = 0.7;
pub const LIFT_STILL_S: f32 = 0.25;
pub const LIFT_RAMP_S: f32 = 0.3;
/// Far palm speed low-pass (s): the reach multiplies the palm's jitter.
const SPEED_TAU_S: f32 = 0.1;
/// Speed a newly seen palm starts from (m/s), so it has to settle first.
const SPEED_UNSEEN_M_S: f32 = 1.0;

/// Pitcher: its rate (particles per second, `debug.fosfora.pitcherrate`,
/// and the panel's range), speed (m/s, `debug.fosfora.pitcherspeed`), the
/// cone's half-angle around the palm normal (degrees; `XR_POUR_SPREAD` in
/// `flux_xr_sim.wgsl`) and the nozzle's radius (m). The rate is capped at
/// 10000/s: into a full cloud the pour's particles come from the living
/// through the steal path every frame, and 20000/s measured 12.2 ms App
/// GPU against 9.6 ms off (MEASURED.md); the intended use is with the
/// cloud density lowered.
pub const PITCHER_RATE: f32 = 4000.0;
pub const PITCHER_RATE_MIN: f32 = 500.0;
pub const PITCHER_RATE_MAX: f32 = 10_000.0;
pub const PITCHER_SPEED_M_S: f32 = 1.5;
pub const POUR_SPREAD_DEG: f32 = 6.0;
pub const POUR_NOZZLE_M: f32 = 0.02;

/// At most this fraction of the living particles is taken for a burst
/// ([`steal_fraction`]).
pub const MAX_STEAL: f32 = 0.05;

/// A box the throw can hit, in the reference space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayBox {
    pub center: Vec3,
    /// Box -> world.
    pub rot: Quat,
    pub half: Vec3,
    pub kind: u32,
}

/// Where a ray entered a box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Index into the boxes cast against.
    pub index: usize,
    /// Distance along the ray (m).
    pub distance: f32,
    pub point: Vec3,
    /// The entered face's outward normal.
    pub normal: Vec3,
}

/// The nearest box `origin + t * dir` (`dir` unit, `0 < t <= max_m`)
/// enters, by a slab test in each box's frame. A box containing `origin`
/// is skipped: the ray leaves it, it does not hit it.
pub fn cast(origin: Vec3, dir: Vec3, boxes: &[RayBox], max_m: f32) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for (index, b) in boxes.iter().enumerate() {
        let inv = b.rot.conjugate();
        let o = (inv * (origin - b.center)).to_array();
        let d = (inv * dir).to_array();
        let h = b.half.to_array();
        let (mut near, mut far) = (f32::MIN, f32::MAX);
        let mut axis = None;
        let mut missed = false;
        for i in 0..3 {
            if d[i].abs() < 1e-8 {
                if o[i].abs() > h[i] {
                    missed = true;
                    break;
                }
                continue;
            }
            let (t1, t2) = ((-h[i] - o[i]) / d[i], (h[i] - o[i]) / d[i]);
            let (lo, hi) = (t1.min(t2), t1.max(t2));
            if lo > near {
                near = lo;
                axis = Some(i);
            }
            far = far.min(hi);
        }
        let Some(i) = axis else { continue };
        if missed || near > far || near <= 0.0 || near > max_m {
            continue;
        }
        if best.is_some_and(|b| b.distance <= near) {
            continue;
        }
        let mut n = [0.0f32; 3];
        n[i] = -d[i].signum();
        best = Some(Hit {
            index,
            distance: near,
            point: origin + dir * near,
            normal: b.rot * Vec3::from_array(n),
        });
    }
    best
}

/// One throw in flight: from the pinch point to the surface, then the
/// burst.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Flight {
    pub from: Vec3,
    /// The hit point, or the miss's end.
    pub to: Vec3,
    /// The burst's center: off the surface along its normal (a miss: `to`).
    pub center: Vec3,
    pub duration_s: f32,
    pub hit: Option<Hit>,
    age_s: f32,
    /// Impact frames delivered so far.
    impact_frames: u32,
}

impl Flight {
    /// A throw from the far pinch point `pinch`, aimed along the ray from
    /// `head` through it, against `boxes`. `None` when the two coincide.
    pub fn aim(head: Vec3, pinch: Vec3, boxes: &[RayBox]) -> Option<Self> {
        let dir = (pinch - head).try_normalize()?;
        let hit = cast(pinch, dir, boxes, RANGE_M);
        let (to, center) = match hit {
            Some(h) => (h.point, h.point + h.normal * (HIT_LIFT_M + BURST_RADIUS_M)),
            None => {
                let end = pinch + dir * MISS_M;
                (end, end)
            }
        };
        Some(Self {
            from: pinch,
            to,
            center,
            duration_s: (pinch.distance(to) / THROW_SPEED_M_S).max(MIN_FLIGHT_S),
            hit,
            age_s: 0.0,
            impact_frames: 0,
        })
    }
}

/// This frame's burst for the rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Burst {
    pub center: Vec3,
    pub radius: f32,
    pub count: u32,
    /// The hand whose flight it is.
    pub hand: usize,
    /// Part of the impact, not the streak.
    pub impact: bool,
    /// Size and opacity against a fresh burst's, 0..1: the streak fades
    /// from 1 to [`STREAK_END_BRIGHTNESS`] over the flight; the impact is 1.
    pub brightness: f32,
}

/// Each hand's flight, packed into the rows one a frame.
#[derive(Debug, Clone, Copy)]
pub struct Thrower {
    /// Impact particles (`burstcount`).
    pub burst_count: u32,
    flights: [Option<Flight>; 2],
    turn: usize,
}

impl Default for Thrower {
    fn default() -> Self {
        Self::new(BURST_COUNT)
    }
}

impl Thrower {
    pub fn new(burst_count: u32) -> Self {
        Self {
            burst_count,
            flights: [None; 2],
            turn: 0,
        }
    }

    /// Start `hand`'s flight, replacing the one it had.
    pub fn throw(&mut self, hand: usize, flight: Flight) {
        self.flights[hand] = Some(flight);
    }

    pub fn flight(&self, hand: usize) -> Option<&Flight> {
        self.flights[hand].as_ref()
    }

    /// Advance every flight by `dt` and return this frame's burst: the
    /// streak at the flight's position while it travels, then the impact's
    /// share. With both hands in flight they alternate frames.
    pub fn step(&mut self, dt: f32) -> Option<Burst> {
        for f in self.flights.iter_mut().flatten() {
            f.age_s += dt.max(0.0);
        }
        let hand = match (self.flights[0].is_some(), self.flights[1].is_some()) {
            (true, true) => {
                self.turn ^= 1;
                self.turn
            }
            (true, false) => 0,
            (false, true) => 1,
            (false, false) => return None,
        };
        let frames = BURST_FRAMES.max(1);
        let slot = &mut self.flights[hand];
        let f = slot.as_mut()?;
        let burst = if f.age_s < f.duration_s {
            let k = (f.age_s / f.duration_s).clamp(0.0, 1.0);
            Burst {
                center: f.from.lerp(f.to, k),
                radius: STREAK_RADIUS_M,
                count: STREAK_PER_FRAME,
                hand,
                impact: false,
                brightness: 1.0 - k * (1.0 - STREAK_END_BRIGHTNESS),
            }
        } else {
            // The remainder on the first frame, so the frames sum to the
            // count exactly.
            let share = self.burst_count / frames;
            let first = self.burst_count - share * (frames - 1);
            let count = if f.impact_frames == 0 { first } else { share };
            f.impact_frames += 1;
            let b = Burst {
                center: f.center,
                radius: BURST_RADIUS_M,
                count,
                hand,
                impact: true,
                brightness: 1.0,
            };
            if f.impact_frames >= frames {
                *slot = None;
            }
            b
        };
        Some(burst)
    }
}

/// One hand as the lift reads it this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiftHand {
    /// The pose read on the real hand (`None`: not tracked).
    pub pose: Option<Pose>,
    /// The palm at the far hand (the reach applied).
    pub far_palm: Option<Vec3>,
    /// The palm normal (`pose::HandInput::normal`).
    pub normal: Option<Vec3>,
    /// The hand is taken (the hand menu's left hand while it is up).
    pub blocked: bool,
}

#[derive(Debug, Clone, Copy, Default)]
struct HandLift {
    last: Option<Vec3>,
    speed: f32,
    still_s: f32,
    on: bool,
    /// When it turned on (the lifter's clock).
    since: f32,
    strength: f32,
    at: Vec3,
}

/// This frame's lift for the rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lift {
    pub at: Vec3,
    pub strength: f32,
    pub radius: f32,
    pub hand: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct Lifter {
    /// `liftradius`.
    pub radius: f32,
    hands: [HandLift; 2],
    clock: f32,
}

impl Default for Lifter {
    fn default() -> Self {
        Self::new(LIFT_RADIUS_M)
    }
}

impl Lifter {
    pub fn new(radius: f32) -> Self {
        Self {
            radius,
            hands: [HandLift::default(); 2],
            clock: 0.0,
        }
    }

    /// Whether `hand`'s lift is on (its strength may still be ramping).
    pub fn is_on(&self, hand: usize) -> bool {
        self.hands[hand].on
    }

    /// Advance by `dt` with both hands (left, right); this frame's lift, if
    /// any hand's strength is above 0.
    pub fn step(&mut self, hands: [LiftHand; 2], dt: f32) -> Option<Lift> {
        let dt = dt.max(0.0);
        self.clock += dt;
        let blend = 1.0 - (-dt / SPEED_TAU_S).exp();
        for (s, h) in self.hands.iter_mut().zip(hands) {
            match h.far_palm {
                Some(p) => {
                    s.speed = match s.last {
                        Some(last) if dt > 0.0 => {
                            s.speed + (p.distance(last) / dt - s.speed) * blend
                        }
                        Some(_) => s.speed,
                        None => SPEED_UNSEEN_M_S,
                    };
                    s.last = Some(p);
                }
                None => {
                    s.last = None;
                    s.speed = SPEED_UNSEEN_M_S;
                }
            }
            let down = h.normal.is_some_and(|n| n.dot(Vec3::Y) < LIFT_DOWN);
            let shaped = !h.blocked && h.far_palm.is_some() && h.pose == Some(Pose::Open) && down;
            s.still_s = if shaped && s.speed < LIFT_STILL_M_S {
                s.still_s + dt
            } else {
                0.0
            };
            if s.on {
                s.on = shaped && s.speed < LIFT_MOVE_M_S;
            } else if s.still_s >= LIFT_STILL_S - 1e-6 {
                s.on = true;
                s.since = self.clock;
            }
            if s.on
                && let Some(p) = h.far_palm
            {
                s.at = p;
            }
            let ramp = dt / LIFT_RAMP_S.max(1e-3);
            s.strength = if s.on {
                (s.strength + ramp).min(1.0)
            } else {
                (s.strength - ramp).max(0.0)
            };
        }
        // The later lift wins; with none on, the one still ramping down.
        let hand = (0..2)
            .filter(|&h| self.hands[h].on)
            .max_by(|&a, &b| self.hands[a].since.total_cmp(&self.hands[b].since))
            .or_else(|| {
                (0..2)
                    .filter(|&h| self.hands[h].strength > 0.0)
                    .max_by(|&a, &b| self.hands[a].strength.total_cmp(&self.hands[b].strength))
            })?;
        let s = self.hands[hand];
        (s.strength > 0.0).then_some(Lift {
            at: s.at,
            strength: s.strength,
            radius: self.radius,
            hand,
        })
    }
}

/// This frame's pour for the rows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pour {
    /// The far palm.
    pub center: Vec3,
    /// The palm normal (unit).
    pub dir: Vec3,
    pub count: u32,
    /// m/s.
    pub speed: f32,
    /// The nozzle (m).
    pub radius: f32,
}

impl Pour {
    /// The pour as the burst rows carry it: the count, the nozzle, full
    /// brightness (the burst look, so the stream reads through the cloud).
    pub fn as_burst(&self) -> Burst {
        Burst {
            center: self.center,
            radius: self.radius,
            count: self.count,
            hand: 1,
            impact: false,
            brightness: 1.0,
        }
    }
}

/// What the pitcher did since the last [`Pitcher::take_stats`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PourStats {
    /// Frames a pour was packed.
    pub frames: u32,
    /// Particles asked of the sim in those frames.
    pub asked: u64,
    /// Frames the pour gave way to a throw's burst.
    pub skipped: u32,
}

/// The particle pitcher on the right far palm.
#[derive(Debug, Clone, Copy)]
pub struct Pitcher {
    pub on: bool,
    /// Particles per second.
    pub rate: f32,
    /// m/s.
    pub speed: f32,
    /// The fraction of a particle not yet poured.
    carry: f32,
    stats: PourStats,
}

impl Default for Pitcher {
    fn default() -> Self {
        Self::new(PITCHER_RATE, PITCHER_SPEED_M_S)
    }
}

impl Pitcher {
    /// Off, at `rate` particles per second and `speed` m/s.
    pub fn new(rate: f32, speed: f32) -> Self {
        Self {
            on: false,
            rate,
            speed,
            carry: 0.0,
            stats: PourStats::default(),
        }
    }

    /// Advance by `dt` with the right far palm (`None`: untracked) and its
    /// normal; this frame's pour. Off or untracked, nothing (and nothing
    /// carried over). `throw_busy`: a throw has the rows this frame, so the
    /// frame's particles are dropped and the frame counted as skipped.
    pub fn step(&mut self, palm: Option<(Vec3, Vec3)>, throw_busy: bool, dt: f32) -> Option<Pour> {
        let Some((center, normal)) = palm.filter(|_| self.on) else {
            self.carry = 0.0;
            return None;
        };
        let dir = normal.try_normalize()?;
        self.carry += self.rate.max(0.0) * dt.max(0.0);
        let whole = self.carry.floor();
        self.carry -= whole;
        if throw_busy {
            self.stats.skipped += 1;
            return None;
        }
        let count = whole as u32;
        if count == 0 {
            return None;
        }
        self.stats.frames += 1;
        self.stats.asked += u64::from(count);
        Some(Pour {
            center,
            dir,
            count,
            speed: self.speed,
            radius: POUR_NOZZLE_M,
        })
    }

    /// The counts since the last call, reset.
    pub fn take_stats(&mut self) -> PourStats {
        std::mem::take(&mut self.stats)
    }
}

/// The pour row after the depth rows (`XR_AUX_POUR` in `flux_xr_sim.wgsl`):
/// xyz the pour's direction (unit; a direction is the same in the anchor's
/// frame, which only translates), w its speed (m/s). All zeros without a
/// pour: a burst in the burst rows is then a throw's, flying outward.
pub fn pour_row(pour: Option<Pour>) -> [f32; 4] {
    pour.filter(|p| p.count > 0)
        .map_or([0.0; 4], |p| [p.dir.x, p.dir.y, p.dir.z, p.speed.max(0.0)])
}

/// The instrument rows (layout in `flux_xr_sim.wgsl`, "Instrument rows"),
/// positions moved into the frame of `anchor`:
/// - row 0: x the burst count (u32 bits), y lift strength 0..1, z lift
///   radius (m), w the steal fraction ([`steal_fraction`], filled in where
///   the alive count is known; 0 here);
/// - row 1: the burst center xyz, w its radius (m);
/// - row 2: the lift point xyz (the far palm), w the burst's brightness
///   (0 with no burst; the sim reads 0 as 1).
///
/// No burst and no lift is all zeros, the rows' meaning before they existed.
pub fn rows(burst: Option<Burst>, lift: Option<Lift>, anchor: Vec3) -> [[f32; 4]; INSTRUMENT_ROWS] {
    let mut rows = [[0.0f32; 4]; INSTRUMENT_ROWS];
    if let Some(b) = burst.filter(|b| b.count > 0) {
        rows[0][0] = f32::from_bits(b.count);
        let c = b.center - anchor;
        rows[1] = [c.x, c.y, c.z, b.radius];
        rows[2][3] = b.brightness.clamp(0.0, 1.0);
    }
    if let Some(l) = lift.filter(|l| l.strength > 0.0) {
        rows[0][1] = l.strength.clamp(0.0, 1.0);
        rows[0][2] = l.radius;
        let p = l.at - anchor;
        rows[2][..3].copy_from_slice(&[p.x, p.y, p.z]);
    }
    rows
}

/// The fraction of living particles the sim respawns at a burst of `count`
/// this frame, besides the dead slots it claims first: a sim near its
/// particle count (Flux Cloud settles close to it) has only the few
/// hundred slots that die each frame. `alive` and `max` are the alive
/// count as last read back and the particle count. At most [`MAX_STEAL`].
pub fn steal_fraction(count: u32, alive: u32, max: u32) -> f32 {
    if count == 0 || alive == 0 {
        return 0.0;
    }
    let dead = max.saturating_sub(alive);
    (count.saturating_sub(dead) as f32 / alive as f32).clamp(0.0, MAX_STEAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surfaces::{KIND_FLOOR, KIND_TABLE, KIND_WALL};

    const DT: f32 = 1.0 / 72.0;
    const HEAD: Vec3 = Vec3::new(0.0, 1.2, 0.0);

    fn wall_ahead(z: f32) -> RayBox {
        // A wall plane (local +Z its normal) facing +Z, 4 cm thick.
        RayBox {
            center: Vec3::new(0.0, 1.25, z),
            rot: Quat::IDENTITY,
            half: Vec3::new(2.0, 1.25, 0.02),
            kind: KIND_WALL,
        }
    }

    fn table() -> RayBox {
        // A runtime table: local +Z up, top at y = 0.75.
        RayBox {
            center: Vec3::new(0.0, 0.4, -0.8),
            rot: Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2),
            half: Vec3::new(0.6, 0.4, 0.35),
            kind: KIND_TABLE,
        }
    }

    #[test]
    fn the_ray_hits_the_nearest_face_and_returns_its_normal() {
        let boxes = [wall_ahead(-3.0), wall_ahead(-2.0)];
        let hit = cast(Vec3::new(0.0, 1.2, -0.5), Vec3::NEG_Z, &boxes, 5.0).expect("hit");
        assert_eq!(hit.index, 1);
        assert!((hit.distance - 1.48).abs() < 1e-5, "{hit:?}");
        assert!(hit.point.abs_diff_eq(Vec3::new(0.0, 1.2, -1.98), 1e-5));
        assert!(hit.normal.abs_diff_eq(Vec3::Z, 1e-5), "{:?}", hit.normal);
        // Down onto a rotated table: its top, normal up.
        let down = (Vec3::new(0.1, 0.75, -0.8) - HEAD).normalize();
        let hit = cast(HEAD, down, &[table()], 5.0).expect("table");
        assert!(hit.normal.abs_diff_eq(Vec3::Y, 1e-5), "{:?}", hit.normal);
        assert!((hit.point.y - 0.75).abs() < 1e-4, "{hit:?}");
        // Out of range, behind the origin, or beside the box: no hit.
        assert!(cast(Vec3::new(0.0, 1.2, -0.5), Vec3::NEG_Z, &boxes, 1.0).is_none());
        assert!(cast(Vec3::new(0.0, 1.2, -0.5), Vec3::Z, &boxes, 5.0).is_none());
        assert!(cast(Vec3::new(3.0, 1.2, -0.5), Vec3::NEG_Z, &boxes, 5.0).is_none());
        // From inside a box the ray passes out of it to the next one.
        let inside = cast(Vec3::new(0.0, 1.2, -2.0), Vec3::NEG_Z, &boxes, 5.0).unwrap();
        assert_eq!(inside.index, 0);
    }

    #[test]
    fn a_throw_aims_from_the_head_through_the_pinch() {
        let boxes = [wall_ahead(-2.0)];
        let pinch = Vec3::new(0.0, 1.2, -0.6);
        let f = Flight::aim(HEAD, pinch, &boxes).expect("flight");
        let hit = f.hit.expect("the wall");
        assert_eq!(f.to, hit.point);
        assert!(
            f.center
                .abs_diff_eq(hit.point + Vec3::Z * (HIT_LIFT_M + BURST_RADIUS_M), 1e-5)
        );
        assert!(
            (f.duration_s - 1.38 / THROW_SPEED_M_S).abs() < 1e-4,
            "{f:?}"
        );
        // A miss ends 3 m along the ray, bursting in the air there.
        let up = Flight::aim(HEAD, HEAD + Vec3::Y * 0.5, &boxes).unwrap();
        assert!(up.hit.is_none());
        assert!(up.to.abs_diff_eq(HEAD + Vec3::Y * (0.5 + MISS_M), 1e-5));
        assert_eq!(up.center, up.to);
        // A pinch at the head has no direction.
        assert!(Flight::aim(HEAD, HEAD, &boxes).is_none());
        // A short throw still flies for the minimum time.
        let near = Flight::aim(HEAD, Vec3::new(0.0, 1.2, -1.9), &boxes).unwrap();
        assert_close!(near.duration_s, MIN_FLIGHT_S);
    }

    /// Frames of `thrower.step` until it has nothing to pack, with each
    /// frame's burst.
    fn fly(thrower: &mut Thrower, dt: f32) -> Vec<Burst> {
        let mut out = Vec::new();
        while let Some(b) = thrower.step(dt) {
            out.push(b);
            assert!(out.len() < 10_000, "the flight never ended");
        }
        out
    }

    #[test]
    fn a_projectile_reaches_the_hit_in_time_and_bursts_the_count() {
        let floor = RayBox {
            center: Vec3::new(0.0, -0.05, 0.0),
            rot: Quat::IDENTITY,
            half: Vec3::new(10.0, 0.05, 10.0),
            kind: KIND_FLOOR,
        };
        for dt in [1.0 / 72.0, 1.0 / 90.0] {
            let mut t = Thrower::default();
            let f =
                Flight::aim(HEAD, Vec3::new(0.0, 1.0, -0.4), &[wall_ahead(-3.0), floor]).unwrap();
            let hit = f.hit.unwrap();
            assert_eq!(hit.index, 1, "down and ahead: the floor first");
            t.throw(1, f);
            let bursts = fly(&mut t, dt);
            let streak: Vec<_> = bursts.iter().filter(|b| !b.impact).collect();
            let impact: Vec<_> = bursts.iter().filter(|b| b.impact).collect();
            // In flight for the distance at 6 m/s, by time.
            let flown = streak.len() as f32 * dt;
            assert!(
                (flown - f.duration_s).abs() <= dt + 1e-4,
                "{flown} vs {}",
                f.duration_s
            );
            assert!(streak.iter().all(|b| b.count == STREAK_PER_FRAME));
            // Bright at the hand, dimming to the end value at the hit; the
            // impact at full brightness.
            assert!((streak[0].brightness - 1.0).abs() < 0.05, "{:?}", streak[0]);
            let last_b = streak.last().unwrap().brightness;
            assert!(
                (last_b - STREAK_END_BRIGHTNESS).abs() < 0.05 && last_b >= STREAK_END_BRIGHTNESS,
                "{last_b}"
            );
            assert!(
                streak
                    .windows(2)
                    .all(|w| w[1].brightness <= w[0].brightness),
                "the streak never brightens"
            );
            assert!(
                impact
                    .iter()
                    .all(|b| crate::test_util::close(&b.brightness, &1.0))
            );
            // The streak runs from the pinch toward the hit.
            assert!(streak[0].center.abs_diff_eq(f.from, 0.1));
            let last = streak.last().unwrap().center;
            assert!(
                last.distance(hit.point) < THROW_SPEED_M_S * dt * 1.5,
                "{last}"
            );
            // Then the impact: four frames summing to the count, off the
            // floor by the radius and the lift.
            assert_eq!(impact.len(), BURST_FRAMES as usize);
            assert_eq!(impact.iter().map(|b| b.count).sum::<u32>(), BURST_COUNT);
            assert!(impact.iter().all(|b| b.radius == BURST_RADIUS_M));
            assert!((impact[0].center.y - (HIT_LIFT_M + BURST_RADIUS_M)).abs() < 1e-4);
            assert!(t.flight(1).is_none());
        }
        // An odd count still sums exactly.
        let mut t = Thrower::new(6001);
        t.throw(0, Flight::aim(HEAD, HEAD + Vec3::NEG_Z, &[]).unwrap());
        let total: u32 = fly(&mut t, DT)
            .iter()
            .filter(|b| b.impact)
            .map(|b| b.count)
            .sum();
        assert_eq!(total, 6001);
    }

    #[test]
    fn two_flights_alternate_and_a_new_tap_replaces_a_hand_s() {
        let boxes = [wall_ahead(-2.0)];
        let mut t = Thrower::default();
        t.throw(
            0,
            Flight::aim(HEAD, Vec3::new(-0.2, 1.2, -0.6), &boxes).unwrap(),
        );
        t.throw(
            1,
            Flight::aim(HEAD, Vec3::new(0.2, 1.2, -0.6), &boxes).unwrap(),
        );
        let bursts = fly(&mut t, DT);
        let hands: Vec<usize> = bursts.iter().map(|b| b.hand).collect();
        assert!(hands.windows(2).take(10).all(|w| w[0] != w[1]), "{hands:?}");
        // Each impact is delivered whole.
        for h in 0..2 {
            let sum: u32 = bursts
                .iter()
                .filter(|b| b.hand == h && b.impact)
                .map(|b| b.count)
                .sum();
            assert_eq!(sum, BURST_COUNT, "hand {h}");
        }
        // A second tap mid-flight replaces the first.
        let mut t = Thrower::default();
        let first = Flight::aim(HEAD, Vec3::new(0.0, 1.2, -0.6), &boxes).unwrap();
        t.throw(1, first);
        t.step(DT);
        let second = Flight::aim(HEAD, HEAD + Vec3::Y * 0.3, &boxes).unwrap();
        t.throw(1, second);
        let bursts = fly(&mut t, DT);
        assert!(
            bursts
                .iter()
                .filter(|b| b.impact)
                .all(|b| b.center == second.center)
        );
    }

    fn open_down(at: Vec3) -> LiftHand {
        LiftHand {
            pose: Some(Pose::Open),
            far_palm: Some(at),
            normal: Some(Vec3::NEG_Y),
            blocked: false,
        }
    }

    /// `s` seconds of the lifter with the right hand as `hand(t)` and no
    /// left hand; the last frame's lift.
    fn hold(l: &mut Lifter, s: f32, dt: f32, hand: impl Fn(f32) -> LiftHand) -> Option<Lift> {
        let mut out = None;
        for i in 0..(s / dt).round() as usize {
            out = l.step([LiftHand::default(), hand(i as f32 * dt)], dt);
        }
        out
    }

    const PALM: Vec3 = Vec3::new(0.3, 1.0, -1.5);

    #[test]
    fn the_lift_needs_a_still_open_palm_down_and_ramps_by_time() {
        for dt in [1.0 / 72.0, 1.0 / 90.0] {
            let mut l = Lifter::default();
            // Settling (the palm's speed starts high) plus the still time:
            // nothing yet at 0.3 s...
            assert!(hold(&mut l, 0.3, dt, |_| open_down(PALM)).is_none());
            // ...on by 0.5 s, ramping up.
            let on = hold(&mut l, 0.2, dt, |_| open_down(PALM)).expect("on");
            assert!(on.strength > 0.0 && on.strength < 1.0, "{on:?}");
            assert_eq!((on.at, on.radius, on.hand), (PALM, LIFT_RADIUS_M, 1));
            let full = hold(&mut l, LIFT_RAMP_S, dt, |_| open_down(PALM)).unwrap();
            assert_close!(full.strength, 1.0);
            // A fist: off, ramping down over the ramp time, by time.
            let fist = |_| LiftHand {
                pose: Some(Pose::Fist),
                ..open_down(PALM)
            };
            let half = hold(&mut l, LIFT_RAMP_S * 0.5, dt, fist).expect("ramping down");
            assert!((half.strength - 0.5).abs() < 0.05, "{half:?}");
            assert!(!l.is_on(1));
            assert!(hold(&mut l, LIFT_RAMP_S, dt, fist).is_none());
        }
    }

    #[test]
    fn a_fist_a_palm_up_a_moving_hand_or_the_menu_hand_never_lifts() {
        let never = |hand: &dyn Fn(f32) -> LiftHand| {
            let mut l = Lifter::default();
            hold(&mut l, 2.0, DT, hand).is_none()
        };
        assert!(never(&|_| LiftHand {
            pose: Some(Pose::Fist),
            ..open_down(PALM)
        }));
        assert!(never(&|_| LiftHand {
            pose: Some(Pose::PalmUp),
            normal: Some(Vec3::Y),
            ..open_down(PALM)
        }));
        // Open but the palm facing the wall, not down.
        assert!(never(&|_| LiftHand {
            normal: Some(Vec3::NEG_Z),
            ..open_down(PALM)
        }));
        // Open, palm down, sweeping at 1 m/s.
        assert!(never(&|t| open_down(PALM + Vec3::X * t)));
        // The menu's hand.
        assert!(never(&|_| LiftHand {
            blocked: true,
            ..open_down(PALM)
        }));
        // Untracked.
        assert!(never(&|_| LiftHand::default()));
    }

    #[test]
    fn an_on_lift_follows_a_slow_hand_and_ends_on_a_fast_one() {
        let mut l = Lifter::default();
        hold(&mut l, 1.0, DT, |_| open_down(PALM)).unwrap();
        // Led at 0.5 m/s (over the still threshold, under the move one).
        let led = hold(&mut l, 0.5, DT, |t| open_down(PALM + Vec3::X * 0.5 * t)).unwrap();
        assert!(l.is_on(1));
        assert!(led.at.x > PALM.x + 0.2, "{led:?}");
        // At 1.5 m/s, on from where it was led, it lets go.
        let from = led.at;
        hold(&mut l, 0.3, DT, |t| open_down(from + Vec3::X * 1.5 * t));
        assert!(!l.is_on(1));
    }

    #[test]
    fn with_both_hands_lifting_the_later_one_wins() {
        let mut l = Lifter::default();
        let right = PALM + Vec3::X * 0.4;
        for _ in 0..72 {
            l.step([open_down(PALM), LiftHand::default()], DT);
        }
        let mut last = None;
        for _ in 0..72 {
            last = l.step([open_down(PALM), open_down(right)], DT);
        }
        let last = last.unwrap();
        assert!(l.is_on(0) && l.is_on(1));
        assert_eq!((last.hand, last.at), (1, right));
    }

    #[test]
    fn nothing_playing_is_all_zero_rows() {
        assert_eq!(rows(None, None, Vec3::ONE), [[0.0; 4]; INSTRUMENT_ROWS]);
        // A spent burst or a lift at 0 strength is nothing too.
        let burst = Burst {
            center: Vec3::ONE,
            radius: 0.1,
            count: 0,
            hand: 0,
            impact: true,
            brightness: 1.0,
        };
        let lift = Lift {
            at: Vec3::ONE,
            strength: 0.0,
            radius: 0.35,
            hand: 0,
        };
        assert_eq!(
            rows(Some(burst), Some(lift), Vec3::ZERO),
            [[0.0; 4]; INSTRUMENT_ROWS]
        );
    }

    #[test]
    fn the_rows_carry_the_burst_and_the_lift_in_the_anchor_frame() {
        let anchor = Vec3::new(0.0, 1.0, -1.0);
        let burst = Burst {
            center: Vec3::new(0.5, 1.5, -3.0),
            radius: BURST_RADIUS_M,
            count: 1500,
            hand: 1,
            impact: true,
            brightness: 0.5,
        };
        let lift = Lift {
            at: Vec3::new(0.2, 1.1, -1.4),
            strength: 0.5,
            radius: 0.35,
            hand: 0,
        };
        let r = rows(Some(burst), Some(lift), anchor);
        assert_eq!(r[0][0].to_bits(), 1500);
        assert_eq!(r[0][1..], [0.5, 0.35, 0.0]);
        assert_close!(r[1], [0.5, 0.5, -2.0, BURST_RADIUS_M]);
        let p = Vec3::from_slice(&r[2][..3]);
        assert!(p.abs_diff_eq(Vec3::new(0.2, 0.1, -0.4), 1e-6));
        assert_close!(r[2][3], 0.5);
        // A burst alone still carries its brightness; a lift alone leaves 0.
        assert_close!(rows(Some(burst), None, anchor)[2], [0.0, 0.0, 0.0, 0.5]);
        assert_close!(rows(None, Some(lift), anchor)[2][3], 0.0);
    }

    const RIGHT_PALM: (Vec3, Vec3) = (Vec3::new(0.3, 1.0, -1.5), Vec3::new(0.0, -1.0, 0.0));

    #[test]
    fn a_pitcher_pours_its_rate_with_the_fraction_carried_over() {
        for (dt, hz) in [(1.0 / 72.0, 72.0), (1.0 / 90.0, 90.0)] {
            let mut p = Pitcher {
                on: true,
                ..Pitcher::default()
            };
            let pours: Vec<Pour> = (0..hz as usize)
                .filter_map(|_| p.step(Some(RIGHT_PALM), false, dt))
                .collect();
            // One second: the rate, to the particle, in frames of the
            // whole part of rate x dt (55 or 56 at 72 Hz, not 55 each).
            let total: u32 = pours.iter().map(|p| p.count).sum();
            assert!(
                (total as f32 - PITCHER_RATE).abs() <= 1.0,
                "{total} at {hz} Hz"
            );
            let per = PITCHER_RATE / hz;
            assert!(
                pours
                    .iter()
                    .all(|p| p.count == per.floor() as u32 || p.count == per.ceil() as u32),
                "{pours:?}"
            );
            let first = pours[0];
            assert_eq!((first.center, first.dir), RIGHT_PALM);
            assert_eq!(
                (first.speed, first.radius),
                (PITCHER_SPEED_M_S, POUR_NOZZLE_M)
            );
            let stats = p.take_stats();
            assert_eq!(
                (stats.frames, stats.asked, stats.skipped),
                (hz as u32, u64::from(total), 0)
            );
            assert_eq!(p.take_stats(), PourStats::default());
        }
        // A slow pitcher pours on some frames only, still at its rate.
        let mut p = Pitcher {
            on: true,
            ..Pitcher::new(PITCHER_RATE_MIN, PITCHER_SPEED_M_S)
        };
        let total: u32 = (0..720)
            .filter_map(|_| p.step(Some(RIGHT_PALM), false, DT))
            .map(|p| p.count)
            .sum();
        assert!(
            (total as f32 - 10.0 * PITCHER_RATE_MIN).abs() <= 1.0,
            "{total}"
        );
    }

    #[test]
    fn off_or_untracked_nothing_pours_and_a_throw_takes_the_frame() {
        let mut p = Pitcher::default();
        assert!(p.step(Some(RIGHT_PALM), false, DT).is_none(), "off");
        p.on = true;
        assert!(p.step(None, false, DT).is_none(), "untracked");
        assert_eq!(p.take_stats(), PourStats::default());
        // A throw's frames are dropped, not saved up for later.
        for _ in 0..10 {
            assert!(p.step(Some(RIGHT_PALM), true, DT).is_none());
        }
        let after = p.step(Some(RIGHT_PALM), false, DT).expect("pours again");
        assert!(
            after.count <= (PITCHER_RATE * DT).ceil() as u32,
            "{after:?}"
        );
        let stats = p.take_stats();
        assert_eq!((stats.frames, stats.skipped), (1, 10));
        // The palm normal is made unit; a zero normal pours nothing.
        let long = p
            .step(Some((RIGHT_PALM.0, Vec3::new(0.0, 0.0, -3.0))), false, DT)
            .unwrap();
        assert_eq!(long.dir, Vec3::NEG_Z);
        assert!(
            p.step(Some((RIGHT_PALM.0, Vec3::ZERO)), false, DT)
                .is_none()
        );
    }

    #[test]
    fn a_pour_rides_the_burst_rows_and_its_own() {
        let anchor = Vec3::new(0.0, 1.0, -1.0);
        let mut p = Pitcher {
            on: true,
            ..Pitcher::default()
        };
        let pour = p.step(Some(RIGHT_PALM), false, DT).unwrap();
        let r = rows(Some(pour.as_burst()), None, anchor);
        assert_eq!(r[0][0].to_bits(), pour.count);
        assert_close!(r[1], [0.3, 0.0, -0.5, POUR_NOZZLE_M]);
        assert_close!(r[2][3], 1.0);
        assert_close!(pour_row(Some(pour)), [0.0, -1.0, 0.0, PITCHER_SPEED_M_S]);
        // No pour, or an empty one: zeros, and a burst in the rows is a
        // throw's.
        assert_close!(pour_row(None), [0.0; 4]);
        assert_close!(pour_row(Some(Pour { count: 0, ..pour })), [0.0; 4]);
    }

    #[test]
    fn a_full_sim_steals_what_the_dead_slots_cannot_give() {
        // Plenty of dead slots: nothing taken.
        assert_close!(steal_fraction(1500, 380_000, 400_000), 0.0);
        // A sim at its count: the burst's share of the living.
        let f = steal_fraction(1500, 399_700, 400_000);
        assert!((f - 1200.0 / 399_700.0).abs() < 1e-7, "{f}");
        // Never more than the cap, and nothing without a burst.
        assert_close!(steal_fraction(1_000_000, 1000, 1000), MAX_STEAL);
        assert_close!(steal_fraction(0, 1000, 1000), 0.0);
        assert_close!(steal_fraction(1500, 0, 1000), 0.0);
    }
}
