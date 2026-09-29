//! The floor ripple (board #3317): rings of light that expand across the
//! real floor from under the wearer on each beat, over a soft glow that
//! breathes with the bass. Plain numbers in, so it builds and tests on the
//! desktop as well; `gfx.rs` draws it with [`RIPPLE_WGSL`] from the rows
//! [`Ripple::uniform`] packs.
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
/// Height above the highest floor top the quad sits at (m), plus a depth
/// bias in the pipeline: the floor occluders write depth at the floor.
pub const LIFT_M: f32 = 0.02;
/// Edge of the quad over the synthetic stage floor (m), centered on the
/// origin.
pub const SYNTHETIC_QUAD_M: f32 = 6.0;
/// Rows of [`Ripple::uniform`], `struct Ripple` in [`RIPPLE_WGSL`].
pub const UNIFORM_ROWS: usize = 8 + MAX_RINGS;

/// One ring: where and when it was born, and how bright.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ring {
    pub origin: Vec2,
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
    origin: Option<Vec2>,
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
            origin: None,
            rings: [None; MAX_RINGS],
            glow: 0.0,
        }
    }

    /// Advance to time `t` (`dt` since the last call): follow the head's
    /// floor projection, add a ring when `beat` fired (amplitude from
    /// `low`, the low end 0..1), and set the glow from `bass`.
    pub fn update(&mut self, t: f32, dt: f32, head: Vec3, beat: bool, low: f32, bass: f32) {
        let target = Vec2::new(head.x, head.z);
        let blend = 1.0 - (-dt.max(0.0) / ORIGIN_TAU_S).exp();
        let origin = self.origin.map_or(target, |o| o + (target - o) * blend);
        self.origin = Some(origin);
        self.glow = GLOW_LEVEL * bass.clamp(0.0, 1.0);
        for r in &mut self.rings {
            if r.is_some_and(|r| r.at(t, self.speed).is_none()) {
                *r = None;
            }
        }
        if beat {
            let ring = Ring {
                origin,
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
        self.origin
    }

    pub fn rings(&self) -> impl Iterator<Item = &Ring> {
        self.rings.iter().flatten()
    }

    /// Light at floor point `p` (x, z) at time `t`, before the gain and
    /// the alpha scale: the rings plus the glow. The fragment shader
    /// computes the same sum from [`Self::uniform`].
    pub fn intensity(&self, t: f32, p: Vec2) -> f32 {
        let rings: f32 = self
            .rings()
            .filter_map(|r| r.at(t, self.speed).map(|(radius, i)| (r.origin, radius, i)))
            .map(|(o, radius, i)| {
                let d = (p.distance(o) - radius) / WIDTH_M;
                i * (-d * d).exp()
            })
            .sum();
        let glow = self.origin.map_or(0.0, |o| {
            let d = p.distance(o) / GLOW_RADIUS_M;
            self.glow * (-d * d).exp()
        });
        rings + glow
    }

    /// The quad's four corners (world), in order around it: the scene
    /// floor's top face when the room has one, else a
    /// `SYNTHETIC_QUAD_M` square centered on the origin; lifted
    /// `LIFT_M` above the highest floor top (`synthetic_top`: the stage
    /// floor's, when it is an obstacle). `None` with no floor at all or no
    /// origin yet.
    pub fn quad(
        &self,
        scene_floor: Option<crate::surfaces::TopFace>,
        synthetic_top: Option<f32>,
    ) -> Option<[Vec3; 4]> {
        let top = match (scene_floor, synthetic_top) {
            (Some(f), s) => s.map_or(f.center.y, |s| s.max(f.center.y)),
            (None, Some(s)) => s,
            (None, None) => return None,
        };
        let y = top + LIFT_M;
        let (center, a, b) = match scene_floor {
            Some(f) => (
                Vec3::new(f.center.x, y, f.center.z),
                f.axes[0] * f.half[0],
                f.axes[1] * f.half[1],
            ),
            None => {
                let o = self.origin?;
                let h = SYNTHETIC_QUAD_M * 0.5;
                (Vec3::new(o.x, y, o.y), Vec3::X * h, Vec3::Z * h)
            }
        };
        Some(level_quad(center, a, b))
    }

    /// Diagnostic (`debug.fosfora.rippletest ceiling`): the quad under a
    /// box's downward face, `LIFT_M` below it, for a screencap from a
    /// headset lying face up. The mirror of the floor case: the box's
    /// occluder writes depth at that face, so the lift and the depth bias
    /// are what keep the light in front of it. `top` is the box's
    /// [`crate::surfaces::TopFace`], `center` its center.
    pub fn quad_under(top: crate::surfaces::TopFace, center: Vec3) -> [Vec3; 4] {
        let under = 2.0 * center.y - top.center.y - LIFT_M;
        level_quad(
            Vec3::new(top.center.x, under, top.center.z),
            top.axes[0] * top.half[0],
            top.axes[1] * top.half[1],
        )
    }

    /// The uniform rows for time `t`, `struct Ripple` in [`RIPPLE_WGSL`]:
    /// the four corners, the shape (ring width, glow radius, glow, peak
    /// alpha x gain), the color, the origin, then one row per ring slot
    /// (origin x, z, radius, intensity; 0 intensity for an empty or cut
    /// slot).
    pub fn uniform(&self, t: f32, corners: [Vec3; 4]) -> [[f32; 4]; UNIFORM_ROWS] {
        let mut rows = [[0.0f32; 4]; UNIFORM_ROWS];
        for (row, c) in rows.iter_mut().zip(corners) {
            *row = [c.x, c.y, c.z, 1.0];
        }
        rows[4] = [WIDTH_M, GLOW_RADIUS_M, self.glow, PEAK_ALPHA * self.gain];
        rows[5] = [COLOR[0], COLOR[1], COLOR[2], 0.0];
        let o = self.origin.unwrap_or(Vec2::ZERO);
        rows[6] = [o.x, o.y, 0.0, 0.0];
        for (row, r) in rows[8..].iter_mut().zip(&self.rings) {
            if let Some((ring, (radius, i))) = r.and_then(|r| r.at(t, self.speed).map(|a| (r, a))) {
                *row = [ring.origin.x, ring.origin.y, radius, i];
            }
        }
        rows
    }
}

/// A level quad at `center` spanning `a` and `b` (half edges) flattened
/// onto the horizontal: a slightly tilted scene floor still gets a level
/// quad at its lifted center height. Corners in order around it.
fn level_quad(center: Vec3, a: Vec3, b: Vec3) -> [Vec3; 4] {
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
    let (a, b) = (flat(a), flat(b));
    [
        center - a - b,
        center + a - b,
        center + a + b,
        center - a + b,
    ]
}

/// The ripple's shader: group 0 is the eye pass's shared camera
/// (`view_proj`), group 1 the rows of [`Ripple::uniform`]. The vertex stage
/// places the quad from its corners and hands the fragment the world
/// (x, z); the fragment sums the rings and the glow and writes
/// premultiplied warm white, at most the peak alpha.
pub const RIPPLE_WGSL: &str = r"
struct Eye { view_proj: mat4x4<f32> }
struct Ripple {
    corners: array<vec4<f32>, 4>,
    // x ring width, y glow radius, z glow, w peak alpha (gain applied)
    shape: vec4<f32>,
    color: vec4<f32>,
    origin: vec4<f32>,
    _pad: vec4<f32>,
    // x, z origin, radius, intensity
    rings: array<vec4<f32>, 8>,
}
@group(0) @binding(0) var<uniform> eye: Eye;
@group(1) @binding(0) var<uniform> ripple: Ripple;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) xz: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var order = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let c = ripple.corners[order[i]].xyz;
    var out: VsOut;
    out.pos = eye.view_proj * vec4<f32>(c, 1.0);
    out.xz = c.xz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let width = ripple.shape.x;
    var light = 0.0;
    for (var k = 0u; k < 8u; k++) {
        let r = ripple.rings[k];
        // Empty and cut slots are 0: skip them (uniform across the draw).
        if r.w <= 0.0 {
            continue;
        }
        let d = (distance(in.xz, r.xy) - r.z) / width;
        light += r.w * exp(-d * d);
    }
    let g = distance(in.xz, ripple.origin.xy) / ripple.shape.y;
    light += ripple.shape.z * exp(-g * g);
    let a = ripple.shape.w * clamp(light, 0.0, 1.0);
    return vec4<f32>(ripple.color.rgb * a, a);
}
";

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
        let rows = r.uniform(t, [Vec3::ZERO; 4]);
        assert!(rows[8..].iter().all(|row| row[3] == 0.0));
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

    #[test]
    fn the_uniform_carries_live_rings_and_the_quad() {
        let mut r = Ripple::new(2.0, 0.5);
        r.update(0.0, 0.01, HEAD, true, 1.0, 0.5);
        let corners = r.quad(None, Some(0.0)).expect("synthetic floor quad");
        let rows = r.uniform(0.5, corners);
        assert_close!(rows[0][1], LIFT_M);
        assert_close!(
            rows[4],
            [WIDTH_M, GLOW_RADIUS_M, GLOW_LEVEL * 0.5, PEAK_ALPHA * 0.5]
        );
        let live: Vec<_> = rows[8..].iter().filter(|row| row[3] > 0.0).collect();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0][..3], [HEAD.x, HEAD.z, 1.0]);
        assert!((live[0][3] - (-0.5 / DECAY_S).exp()).abs() < 1e-6);
    }

    #[test]
    fn the_quad_covers_the_scene_floor_above_the_highest_top() {
        let mut r = Ripple::default();
        r.update(0.0, 0.01, HEAD, false, 0.0, 0.0);
        // No floor at all: nothing to draw on.
        assert!(r.quad(None, None).is_none());
        // Synthetic floor: a 6 m square on the origin, 2 cm up.
        let q = r.quad(None, Some(0.0)).unwrap();
        assert!(q.iter().all(|c| (c.y - LIFT_M).abs() < 1e-6));
        let (min, max) = (q[0], q[2]);
        assert!((max.x - min.x - SYNTHETIC_QUAD_M).abs() < 1e-5);
        assert!(((min.x + max.x) * 0.5 - HEAD.x).abs() < 1e-5);
        // A scene floor plane (local +Z up, top 2 cm above y = 0): its top
        // face, lifted above it and the stage floor alike.
        let rot = glam::Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let face =
            crate::surfaces::TopFace::of(Vec3::new(1.0, 0.0, -1.0), rot, Vec3::new(2.0, 1.5, 0.02));
        let q = r.quad(Some(face), Some(0.0)).unwrap();
        assert!(q.iter().all(|c| (c.y - 0.04).abs() < 1e-5), "{q:?}");
        let xs: Vec<f32> = q.iter().map(|c| c.x).collect();
        let zs: Vec<f32> = q.iter().map(|c| c.z).collect();
        let span = |v: &[f32]| {
            v.iter().copied().fold(f32::MIN, f32::max) - v.iter().copied().fold(f32::MAX, f32::min)
        };
        assert!((span(&xs) - 4.0).abs() < 1e-4 && (span(&zs) - 3.0).abs() < 1e-4);
    }

    #[test]
    fn the_diagnostic_quad_sits_just_under_a_ceiling() {
        // A ceiling plane at 2.66 m, local +Z down (as measured).
        let rot = glam::Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let center = Vec3::new(1.5, 2.66, -1.8);
        let face = crate::surfaces::TopFace::of(center, rot, Vec3::new(3.0, 3.2, 0.02));
        let q = Ripple::quad_under(face, center);
        assert!(
            q.iter().all(|c| (c.y - (2.64 - LIFT_M)).abs() < 1e-5),
            "{q:?}"
        );
    }

    #[test]
    fn the_shader_validates_and_matches_the_rows() {
        let module = naga::front::wgsl::parse_str(RIPPLE_WGSL).expect("ripple WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("ripple WGSL validates");
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("Ripple"))
            .expect("struct Ripple");
        let naga::TypeInner::Struct { span, .. } = ty.inner else {
            panic!("Ripple is not a struct");
        };
        assert_eq!(span as usize, UNIFORM_ROWS * 16);
    }
}
