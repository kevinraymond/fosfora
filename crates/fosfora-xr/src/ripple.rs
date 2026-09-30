//! The rings' state (board #3317, the floor ripple until D1): rings of
//! light that expand from under the wearer on each beat, over a soft glow
//! that breathes with the bass. Plain numbers in, so it builds and tests
//! on the desktop as well. Since D1 (board #3472) the surfaces pass draws
//! them on every face whose behavior is the rings (`surface_fx.rs`): one
//! state, its rings written into each such face's rows, placed on it from
//! the smoothed head each ring was born under.
//!
//! The origin is the head projected onto the floor and low-passed (by time
//! constant, so 72 and 90 Hz behave alike): it needs no anchor, sits under
//! the chair and stays put when the wearer turns. Each ring keeps the
//! origin it was born at. A ring's radius grows at `speed`, its profile
//! across the radius is a gaussian `WIDTH_M` wide and its intensity decays
//! as `exp(-age / DECAY_S)`; it is cut at `CUT_S`. At most `MAX_RINGS` live
//! at once; a new beat replaces the oldest.
//!
//! The rings fire on the beat pulse, not a kick: there is no kick pulse,
//! and the beat is the pulse timed to the sound (board #3253). Their
//! amplitude is the low end at that beat.

use glam::{Vec2, Vec3};

pub const MAX_RINGS: usize = 8;
/// Ring speed across the floor (m/s), `debug.fosfora.ripplespeed`.
pub const SPEED_M_S: f32 = 2.5;
/// Width of a ring's gaussian profile (m).
pub const WIDTH_M: f32 = 0.15;
/// Intensity decay time constant (s).
pub const DECAY_S: f32 = 0.8;
/// Age at which a ring is dropped (s).
pub const CUT_S: f32 = 2.5;
/// Origin low-pass time constant (s).
pub const ORIGIN_TAU_S: f32 = 0.5;
/// The resting glow's gaussian radius (m) and its intensity at full bass.
pub const GLOW_RADIUS_M: f32 = 0.6;
pub const GLOW_LEVEL: f32 = 0.3;
/// Alpha at intensity 1: light on the floor, not paint over it.
pub const PEAK_ALPHA: f32 = 0.25;
/// Warm white of the ember palette.
pub const COLOR: [f32; 3] = [1.0, 0.86, 0.68];
/// A beat's ring amplitude at silent low end, rising to 1 with it: every
/// beat shows, loud low end shows brighter.
const AMP_FLOOR: f32 = 0.3;
/// Height off a face the surface quads sit at (m), plus a depth bias in
/// the pipeline: the surface's occluder writes depth at its face.
pub const LIFT_M: f32 = 0.02;

/// One ring: where and when it was born, and how bright.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ring {
    /// The smoothed head's floor projection at birth (x, z).
    pub origin: Vec2,
    /// The smoothed head at birth: the surfaces pass places the ring on
    /// any face from it (`surface_fx::rings_origin`).
    pub head: Vec3,
    pub born_s: f32,
    pub amp: f32,
}

impl Ring {
    /// Radius (m) and intensity at time `t`; `None` once cut (or before
    /// birth).
    pub fn at(&self, t: f32, speed: f32) -> Option<(f32, f32)> {
        let age = t - self.born_s;
        (0.0..CUT_S)
            .contains(&age)
            .then(|| (speed * age, self.amp * (-age / DECAY_S).exp()))
    }
}

#[derive(Debug, Clone)]
pub struct Ripple {
    pub speed: f32,
    pub gain: f32,
    /// The head low-passed; the origin is its floor projection.
    head: Option<Vec3>,
    rings: [Option<Ring>; MAX_RINGS],
    glow: f32,
}

impl Default for Ripple {
    fn default() -> Self {
        Self::new(SPEED_M_S, 1.0)
    }
}

impl Ripple {
    pub fn new(speed: f32, gain: f32) -> Self {
        Self {
            speed,
            gain,
            head: None,
            rings: [None; MAX_RINGS],
            glow: 0.0,
        }
    }

    /// Advance to time `t` (`dt` since the last call): follow the head's
    /// floor projection, add a ring when `beat` fired (amplitude from
    /// `low`, the low end 0..1), and set the glow from `bass`.
    pub fn update(&mut self, t: f32, dt: f32, head: Vec3, beat: bool, low: f32, bass: f32) {
        let blend = 1.0 - (-dt.max(0.0) / ORIGIN_TAU_S).exp();
        let smoothed = self.head.map_or(head, |h| h + (head - h) * blend);
        self.head = Some(smoothed);
        let origin = Vec2::new(smoothed.x, smoothed.z);
        self.glow = GLOW_LEVEL * bass.clamp(0.0, 1.0);
        for r in &mut self.rings {
            if r.is_some_and(|r| r.at(t, self.speed).is_none()) {
                *r = None;
            }
        }
        if beat {
            let ring = Ring {
                origin,
                head: smoothed,
                born_s: t,
                amp: AMP_FLOOR + (1.0 - AMP_FLOOR) * low.clamp(0.0, 1.0),
            };
            // A free slot, else the oldest ring.
            let slot = self
                .rings
                .iter()
                .position(Option::is_none)
                .unwrap_or_else(|| {
                    self.rings
                        .iter()
                        .enumerate()
                        .min_by(|a, b| {
                            let born = |r: &Option<Ring>| r.map_or(f32::MIN, |r| r.born_s);
                            born(a.1).total_cmp(&born(b.1))
                        })
                        .map_or(0, |(i, _)| i)
                });
            self.rings[slot] = Some(ring);
        }
    }

    /// The smoothed origin on the floor (x, z), once a head was seen.
    pub fn origin(&self) -> Option<Vec2> {
        self.head.map(|h| Vec2::new(h.x, h.z))
    }

    /// The smoothed head, once one was seen: the origin before its
    /// projection onto a face.
    pub fn head(&self) -> Option<Vec3> {
        self.head
    }

    /// The resting glow's intensity (0 to `GLOW_LEVEL`, with the bass).
    pub fn glow(&self) -> f32 {
        self.glow
    }

    pub fn rings(&self) -> impl Iterator<Item = &Ring> {
        self.rings.iter().flatten()
    }

    /// The ring slots in order, empty ones `None`: the surfaces pass
    /// writes one uniform row per slot.
    pub fn slots(&self) -> &[Option<Ring>; MAX_RINGS] {
        &self.rings
    }

    /// Light at floor point `p` (x, z) at time `t`, before the gain and
    /// the alpha scale: the rings plus the glow. The surfaces pass's
    /// fragment computes the same sum on a floor from the rows
    /// `surface_fx::rows` packs.
    pub fn intensity(&self, t: f32, p: Vec2) -> f32 {
        let rings: f32 = self
            .rings()
            .filter_map(|r| r.at(t, self.speed).map(|(radius, i)| (r.origin, radius, i)))
            .map(|(o, radius, i)| {
                let d = (p.distance(o) - radius) / WIDTH_M;
                i * (-d * d).exp()
            })
            .sum();
        let glow = self.origin().map_or(0.0, |o| {
            let d = p.distance(o) / GLOW_RADIUS_M;
            self.glow * (-d * d).exp()
        });
        rings + glow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: Vec3 = Vec3::new(0.4, 1.2, -0.3);

    fn settled(dt: f32) -> Ripple {
        let mut r = Ripple::default();
        r.update(0.0, dt, Vec3::ZERO, false, 0.0, 0.0);
        let mut t = 0.0;
        while t < 1.0 - 1e-4 {
            t += dt;
            r.update(t, dt, HEAD, false, 0.0, 0.0);
        }
        r
    }

    #[test]
    fn the_origin_settles_by_time_not_frame_count() {
        let (a, b) = (settled(1.0 / 72.0), settled(1.0 / 90.0));
        let (a, b) = (a.origin().unwrap(), b.origin().unwrap());
        assert!(a.distance(b) < 1e-3, "{a} vs {b}");
        // One second is two time constants: 1 - e^-2 = 86 % of the way.
        let target = Vec2::new(HEAD.x, HEAD.z);
        let frac = a.length() / target.length();
        assert!((frac - (1.0 - (-2.0f32).exp())).abs() < 0.01, "{frac}");
        // The head's height plays no part: the origin is on the floor.
        let mut r = Ripple::default();
        r.update(0.0, 0.01, Vec3::new(1.0, 1.7, 2.0), false, 0.0, 0.0);
        assert_eq!(r.origin(), Some(Vec2::new(1.0, 2.0)));
    }

    #[test]
    fn a_beat_adds_a_ring_at_the_origin() {
        let mut r = Ripple::default();
        r.update(0.0, 0.01, HEAD, false, 0.0, 0.0);
        assert_eq!(r.rings().count(), 0);
        r.update(0.5, 0.01, HEAD, true, 0.8, 0.0);
        let rings: Vec<_> = r.rings().copied().collect();
        assert_eq!(rings.len(), 1);
        assert_eq!(rings[0].origin, Vec2::new(HEAD.x, HEAD.z));
        // And the head it was born under, for a face other than a floor.
        assert_eq!(rings[0].head, HEAD);
        assert_eq!(r.head(), Some(HEAD));
        assert_close!(rings[0].born_s, 0.5);
        assert!((rings[0].amp - (AMP_FLOOR + (1.0 - AMP_FLOOR) * 0.8)).abs() < 1e-6);
        // A beat with no low end still shows, dimmer.
        r.update(0.6, 0.01, HEAD, true, 0.0, 0.0);
        assert!(r.rings().any(|g| (g.amp - AMP_FLOOR).abs() < 1e-6));
    }

    #[test]
    fn the_oldest_ring_is_replaced() {
        let mut r = Ripple::default();
        for i in 0..MAX_RINGS {
            r.update(i as f32 * 0.1, 0.1, HEAD, true, 0.5, 0.0);
        }
        assert_eq!(r.rings().count(), MAX_RINGS);
        r.update(0.85, 0.05, HEAD, true, 0.5, 0.0);
        assert_eq!(r.rings().count(), MAX_RINGS);
        let born: Vec<f32> = r.rings().map(|g| g.born_s).collect();
        assert!(
            !born.contains(&0.0),
            "the first ring should be gone: {born:?}"
        );
        assert!(born.contains(&0.85) && born.contains(&0.1), "{born:?}");
    }

    #[test]
    fn a_ring_peaks_at_its_radius_and_decays() {
        let mut r = Ripple::default();
        r.update(0.0, 0.01, Vec3::ZERO, true, 1.0, 0.0);
        let at = |t: f32, d: f32| r.intensity(t, Vec2::new(d, 0.0));
        // At 0.4 s the ring is 1 m out: brightest there, dark a width off
        // on either side.
        let radius = SPEED_M_S * 0.4;
        let peak = at(0.4, radius);
        assert!(peak > at(0.4, radius - 2.0 * WIDTH_M) * 10.0);
        assert!(peak > at(0.4, radius + 2.0 * WIDTH_M) * 10.0);
        assert!((peak - (-0.4 / DECAY_S).exp()).abs() < 1e-3, "{peak}");
        // Later and further, dimmer.
        let later = at(1.2, SPEED_M_S * 1.2);
        assert!(later < peak * 0.5 && later > 0.0, "{later} vs {peak}");
    }

    #[test]
    fn a_cut_ring_contributes_nothing() {
        let mut r = Ripple::default();
        r.update(0.0, 0.01, Vec3::ZERO, true, 1.0, 0.0);
        let t = CUT_S + 0.01;
        assert_close!(r.intensity(t, Vec2::new(SPEED_M_S * t, 0.0)), 0.0);
        assert!(
            r.slots()
                .iter()
                .flatten()
                .all(|g| g.at(t, r.speed).is_none())
        );
        // And the next update frees its slot.
        r.update(t, 0.01, Vec3::ZERO, false, 0.0, 0.0);
        assert_eq!(r.rings().count(), 0);
    }

    #[test]
    fn the_glow_breathes_with_the_bass() {
        let mut r = Ripple::default();
        r.update(0.0, 0.01, Vec3::ZERO, false, 0.0, 0.0);
        assert_close!(r.intensity(0.0, Vec2::ZERO), 0.0);
        r.update(0.01, 0.01, Vec3::ZERO, false, 0.0, 1.0);
        let center = r.intensity(0.01, Vec2::ZERO);
        assert!((center - GLOW_LEVEL).abs() < 1e-6);
        assert!(r.intensity(0.01, Vec2::new(GLOW_RADIUS_M * 2.0, 0.0)) < center * 0.05);
    }
}
