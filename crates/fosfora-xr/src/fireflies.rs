//! Fireflies (board #5470): the one world effect, its caps and its knobs.
//! Plain numbers, so they build and test on the desktop; `scene.rs` uploads
//! the rows and `app.rs` polls the knobs.
//!
//! The room is the show: the lit surfaces, the sky and the depth points.
//! What the world effect adds is one sparse, calm layer, the fireflies, and
//! the particles a hand makes (a throw's streak and burst, the pitcher's
//! pour). Both live in one pool and one sim,
//! `assets/xr/shaders/fireflies_xr_sim.wgsl`, whose header says how each
//! moves.
//!
//! **The caps.** [`FIREFLY_CAP`] is the most fireflies alive at once, and
//! [`POOL_CAP`] the most particles alive at once, fireflies and hand
//! particles together. Neither is a target: the fireflies' count is a knob
//! under its cap ([`Knobs::count`]), and the pool is the preset's particle
//! count held under its cap ([`pool`]). The sim keeps both by where a
//! particle may be born. The pool's slots are all there is, a firefly is
//! born only in a slot under the count ([`Knobs::fireflies`], at most the
//! cap), and a hand particle only in a slot at or past it, so no room,
//! song, theme or gesture can put more of either alive.
//!
//! **The knobs** ([`Knobs`]) are read live, once a second, for a worn
//! tuning pass: the fireflies' count, size, brightness, drift speed and how
//! strongly they answer the music, and the hand particles' gravity and
//! edge-flow strength. Each is clamped to its range, and a value that does
//! not parse keeps the default.
//!
//! **The air per theme** (board #5543, [`Look`]). A theme makes the
//! fireflies something else: stars, ash, spray, plankton, a mirror ball's
//! specks. It is the same pool, the same slots under the same count and
//! cap, the same sprites and the same draw. What the theme gives
//! (`themes::Air`) is a color, the band their light follows, a count and a
//! motion, and the sim's header says how each motion goes. The count is the
//! theme's unless `debug.fosfora.fireflies` is set, which wins
//! ([`Knobs::under`]). A room with no theme has the fireflies as they
//! were. When the theme changes, the particles of the old air fade out
//! where they are, in their own motion and color, and the new ones are
//! born into theirs, so nothing jumps. When a track brings its theme
//! through dark (`themes::Bringing`), `app.rs` dims the air's brightness
//! by the same veil as the surfaces' light.

use crate::surface_fx::{BAND_BASS, BAND_HIGH, BAND_MID, BAND_RMS, BANDS};
use crate::themes::{AirColor, Motion, Theme};

/// The world effect's name, as its preset has it.
pub const EFFECT: &str = "Fireflies";

/// The most fireflies alive at once (`XR_FLY_CAP` in the sim).
pub const FIREFLY_CAP: u32 = 3_000;
/// The most particles alive at once, fireflies and hand particles
/// together: the most slots the pool may have.
pub const POOL_CAP: u32 = 20_000;
// The hands always have slots of their own.
const _: () = assert!(FIREFLY_CAP < POOL_CAP);

/// The rows the sim reads after the box cell list (`XR_AUX_FIREFLIES`,
/// `XR_AUX_FIREFLY_ROWS`).
pub const ROWS: usize = 4;

/// The fireflies' count (`debug.fosfora.fireflies`), 0 to [`FIREFLY_CAP`].
/// In the preset's 6 m volume, 2,000 is about one every 0.36 m.
pub const COUNT: u32 = 2_000;
/// Their sprite radius (m, `debug.fosfora.fireflysize`): each firefly is
/// 0.7 to 1.3 of it. Twice the sprite a thrown burst has: in the headset's
/// captures (2026-11-14, Aurora and a black room) a firefly at the burst's
/// 0.01 was a pinprick beside the depth light's points, 0.02 a soft glow
/// and 0.03 a blob near the head.
pub const SIZE_M: f32 = 0.02;
pub const SIZE_RANGE_M: [f32; 2] = [0.002, 0.04];
/// Their brightness (`debug.fosfora.fireflygain`): the opacity at the top
/// of a firefly's rhythm with the music silent. Over 1 the top saturates
/// and the dim part of the rhythm comes up.
pub const BRIGHTNESS: f32 = 0.8;
pub const BRIGHTNESS_MAX: f32 = 2.0;
/// Their drift speed (m/s, `debug.fosfora.fireflydrift`): the typical
/// speed, each firefly 0.6 to 1.4 of it.
pub const DRIFT_M_S: f32 = 0.06;
pub const DRIFT_MAX_M_S: f32 = 0.5;
/// How strongly they answer the music (`debug.fosfora.fireflymusic`), 0 to
/// 1: at 1 full loudness lifts the light by 60% and the radius by 35%.
pub const MUSIC: f32 = 0.5;
/// The hand particles' gravity (m/s^2, `debug.fosfora.handgravity`).
pub const HAND_GRAVITY: f32 = 2.5;
pub const HAND_GRAVITY_MAX: f32 = 9.81;
/// The hand particles' edge-flow strength (`debug.fosfora.edgeflow`): 0
/// off, 1 the default, up to [`EDGE_FLOW_MAX`]. It scales how near a second
/// surface must be ([`EDGE_BAND_M`]) and how fast a particle turns to run
/// along the edge.
pub const EDGE_FLOW: f32 = 1.0;
pub const EDGE_FLOW_MAX: f32 = 2.0;
/// The edge band at strength 1 (m, `XR_EDGE_BAND_M` in the sim).
pub const EDGE_BAND_M: f32 = 0.05;
/// The widest the edge band gets (m): what the box cell list's masks are
/// dilated by, so a face within the band is in the particle's cell.
pub const EDGE_BAND_MAX_M: f32 = EDGE_BAND_M * EDGE_FLOW_MAX;

pub const KNOB_COUNT: &str = "debug.fosfora.fireflies";
pub const KNOB_SIZE: &str = "debug.fosfora.fireflysize";
pub const KNOB_BRIGHTNESS: &str = "debug.fosfora.fireflygain";
pub const KNOB_DRIFT: &str = "debug.fosfora.fireflydrift";
pub const KNOB_MUSIC: &str = "debug.fosfora.fireflymusic";
pub const KNOB_GRAVITY: &str = "debug.fosfora.handgravity";
pub const KNOB_EDGE_FLOW: &str = "debug.fosfora.edgeflow";

/// By band (`themes::AirColor::ByBand`): the colors of the particles that
/// follow the bass, the mids and the highs. They are the colors of the
/// parts of Classic that follow those bands: the floor's own, whose rings
/// the bass spreads, the tables' own, which the mids move, and the walls'
/// own, the spectrum's. The app has no other colors per band: the edge
/// bars and the spectrum draw every band in the surface's one color.
pub const BAND_COLORS: [[f32; 3]; 3] = [
    crate::surfaces::VIOLET,
    crate::surfaces::BLUE,
    crate::surfaces::WARM_WHITE,
];

/// The orbit's turn rate (rad/s) per m/s of the drift speed: at the
/// default drift one turn in about 42 s.
pub const ORBIT_RAD_PER_M: f32 = 2.5;

/// The orbit's phase a frame of `dt` seconds later at the drift speed
/// `drift` (m/s), kept in 0..TAU. The XR app steps it and hands it to the
/// sim ([`Knobs::rows`]), so anything else that should turn with the
/// particles can read the same number.
pub fn orbit_step(phase: f32, dt: f32, drift: f32) -> f32 {
    if !(phase.is_finite() && dt.is_finite() && drift.is_finite()) {
        return 0.0;
    }
    (phase + dt.max(0.0) * drift.max(0.0) * ORBIT_RAD_PER_M).rem_euclid(std::f32::consts::TAU)
}

/// Where an orbiting particle is at `phase`, in meters from the anchor
/// (the sim's `xr_orbit_place`): `radius` of the way out (0..1, its own),
/// `angle` its own (radians) and `height` its own share of the slab, in a
/// volume of half extent `half` whose slab runs from `slab[0]` to
/// `slab[1]` of it and whose particles span `span` of it.
pub fn orbit_place(
    phase: f32,
    (radius, angle, height): (f32, f32, f32),
    half: f32,
    span: f32,
    slab: [f32; 2],
) -> [f32; 3] {
    let r = radius.sqrt() * half * span;
    let a = phase + angle;
    [
        r * a.cos(),
        (slab[0] + (slab[1] - slab[0]) * height) * half,
        r * a.sin(),
    ]
}

/// How a particle of the air is tinted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tint {
    /// The fireflies' own two tints, a warm yellow and a yellow green.
    Own,
    /// One color (linear RGB), each particle a little toward white.
    Color([f32; 3]),
    /// The color of the band it follows ([`BAND_COLORS`]).
    ByBand,
}

/// The motion's number in the sim's row (`XR_AIR_*`).
pub fn motion_id(motion: Motion) -> u32 {
    match motion {
        Motion::Wander => 0,
        Motion::Twinkle => 1,
        Motion::Rise => 2,
        Motion::Drift => 3,
        Motion::Orbit => 4,
    }
}

/// What the air's particles are (board #5543): the fireflies, or what the
/// room's theme makes of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// Which air this is: 0 with no theme, else the theme's place in the
    /// cycle from 1. A particle carries the stamp it was born with, and
    /// the sim fades out the ones whose stamp is not the air's.
    pub stamp: u32,
    pub tint: Tint,
    /// The audio band the light and the size follow (`surface_fx::BAND_*`).
    pub band: u32,
    pub motion: Motion,
}

impl Default for Look {
    /// The fireflies as they were: their own tints, the loudness, the
    /// wander.
    fn default() -> Self {
        Self {
            stamp: 0,
            tint: Tint::Own,
            band: BAND_RMS,
            motion: Motion::Wander,
        }
    }
}

impl Look {
    /// The air in a room whose saved theme is `theme`: the theme's
    /// (`themes::Theme::air`), or the fireflies with none. A color index
    /// is the surface palette's; the key's tint is not the air's to
    /// follow, and reads as white.
    pub fn of(theme: Option<Theme>) -> Self {
        let Some(theme) = theme else {
            return Self::default();
        };
        let air = theme.air();
        let place = Theme::ALL.iter().position(|t| *t == theme).unwrap_or(0);
        Self {
            stamp: place as u32 + 1,
            tint: match air.color {
                AirColor::Index(i) => Tint::Color(crate::surfaces::surface_color(
                    i,
                    crate::surfaces::KIND_OTHER,
                    [1.0; 3],
                )),
                AirColor::ByBand => Tint::ByBand,
            },
            band: air.band.min(BANDS - 1),
            motion: air.motion,
        }
    }

    /// The sim's two rows for the air (the layout is in the sim's header):
    /// the color and how a particle is tinted, then the stamp, the motion,
    /// the band and the orbit's `phase`.
    pub fn rows(&self, phase: f32) -> [[f32; 4]; 2] {
        let (rgb, tint) = match self.tint {
            Tint::Own => ([0.0; 3], 0.0),
            Tint::Color(c) => (c, 1.0),
            Tint::ByBand => ([0.0; 3], 2.0),
        };
        let phase = if phase.is_finite() { phase } else { 0.0 };
        [
            [rgb[0], rgb[1], rgb[2], tint],
            [
                self.stamp as f32,
                motion_id(self.motion) as f32,
                self.band as f32,
                phase,
            ],
        ]
    }

    /// The band's level in the four the surfaces read (`rms`, `bass`,
    /// `mid`, `high`), 0..1: what the size swells with, from the levels
    /// gliding. Tinted by band, every particle has a band of its own, and
    /// the size follows the loudness.
    pub fn level(&self, [rms, bass, mid, high]: [f32; 4]) -> f32 {
        let level = match (self.tint, self.band) {
            (Tint::ByBand, _) => rms,
            (_, BAND_BASS) => bass,
            (_, BAND_MID) => mid,
            (_, BAND_HIGH) => high,
            _ => rms,
        };
        if level.is_finite() {
            level.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The air in a few words, for the log: "air: warm white, high,
    /// twinkle (theme aurora)", "air: the fireflies (no theme saved)".
    pub fn line(&self, theme: Option<Theme>) -> String {
        let Some(theme) = theme else {
            return "air: the fireflies (no theme saved)".to_owned();
        };
        let color = match theme.air().color {
            AirColor::Index(i) => crate::surfaces::color_name(i),
            AirColor::ByBand => "by band",
        };
        format!(
            "air: {color}, {}, {} (theme {})",
            crate::surface_fx::band_name(self.band),
            self.motion.name(),
            theme.name()
        )
    }
}

/// The live knobs, each inside its range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Knobs {
    /// The fireflies' count, at most [`FIREFLY_CAP`].
    pub count: u32,
    /// The count's knob is set to a number, so it decides and the room's
    /// theme does not ([`Self::under`]).
    pub count_forced: bool,
    /// Their sprite radius (m).
    pub size: f32,
    /// Their peak opacity with the music silent.
    pub brightness: f32,
    /// Their typical speed (m/s).
    pub drift: f32,
    /// How strongly they answer the music, 0..1.
    pub music: f32,
    /// The hand particles' gravity (m/s^2).
    pub gravity: f32,
    /// The hand particles' edge-flow strength.
    pub edge_flow: f32,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            count: COUNT,
            count_forced: false,
            size: SIZE_M,
            brightness: BRIGHTNESS,
            drift: DRIFT_M_S,
            music: MUSIC,
            gravity: HAND_GRAVITY,
            edge_flow: EDGE_FLOW,
        }
    }
}

impl Knobs {
    /// The knobs as set, each read through `prop` (a system property by
    /// name, `None` unset). A value that is not a finite number takes the
    /// default, and one out of range the nearest end of it. The count's
    /// knob at a number also takes the count from the room's theme
    /// ([`Self::under`]); unset, empty or unreadable leaves it the theme's.
    pub fn from_knobs(prop: impl Fn(&str) -> Option<String>) -> Self {
        let d = Self::default();
        let number = |name: &str| {
            prop(name)
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite())
        };
        Self {
            count: number(KNOB_COUNT).map_or(d.count, |v| v.clamp(0.0, FIREFLY_CAP as f32) as u32),
            count_forced: number(KNOB_COUNT).is_some(),
            size: number(KNOB_SIZE).map_or(d.size, |v| v.clamp(SIZE_RANGE_M[0], SIZE_RANGE_M[1])),
            brightness: number(KNOB_BRIGHTNESS)
                .map_or(d.brightness, |v| v.clamp(0.0, BRIGHTNESS_MAX)),
            drift: number(KNOB_DRIFT).map_or(d.drift, |v| v.clamp(0.0, DRIFT_MAX_M_S)),
            music: number(KNOB_MUSIC).map_or(d.music, |v| v.clamp(0.0, 1.0)),
            gravity: number(KNOB_GRAVITY).map_or(d.gravity, |v| v.clamp(0.0, HAND_GRAVITY_MAX)),
            edge_flow: number(KNOB_EDGE_FLOW).map_or(d.edge_flow, |v| v.clamp(0.0, EDGE_FLOW_MAX)),
        }
    }

    /// These knobs in a room whose saved theme is `theme` (board #5543):
    /// the count is the theme's (`themes::Air::count`, held to
    /// [`FIREFLY_CAP`]) unless the count's knob is set. With no theme the
    /// count is the knob's or the default.
    #[must_use]
    pub fn under(self, theme: Option<Theme>) -> Self {
        match theme {
            Some(t) if !self.count_forced => Self {
                count: t.air().count.min(FIREFLY_CAP),
                ..self
            },
            _ => self,
        }
    }

    /// The fireflies the sim is asked for at `density` (0..1, the debug
    /// panel's density and the thermal governor's under it): that share of
    /// the count, never past [`FIREFLY_CAP`].
    pub fn fireflies(&self, density: f32) -> u32 {
        let density = if density.is_finite() {
            density.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let count = self.count.min(FIREFLY_CAP);
        ((count as f32 * density).round() as u32).min(count)
    }

    /// The sim's four rows (the layout is in the sim's header): the count
    /// at `density`, the size, the brightness and the drift speed; the
    /// music knob, the hand particles' gravity and edge-flow strength, and
    /// `swell`, the air's band as the flash limit glides it, 0..1, which
    /// the size follows ([`Look::level`]); then the air's two
    /// ([`Look::rows`]) with the orbit's `phase` ([`orbit_step`]).
    pub fn rows(&self, density: f32, swell: f32, air: &Look, phase: f32) -> [[f32; 4]; ROWS] {
        let swell = if swell.is_finite() {
            swell.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let [color, air] = air.rows(phase);
        [
            [
                f32::from_bits(self.fireflies(density)),
                self.size,
                self.brightness,
                self.drift,
            ],
            [self.music, self.gravity, self.edge_flow, swell],
            color,
            air,
        ]
    }

    /// The state in a few words, for the log.
    pub fn describe(&self) -> String {
        format!(
            "{} fireflies (cap {FIREFLY_CAP}) · radius {} m · brightness {} · drift {} m/s · music {} · hand gravity {} m/s2 · edge flow {}",
            self.count,
            self.size,
            self.brightness,
            self.drift,
            self.music,
            self.gravity,
            self.edge_flow
        )
    }

    /// The launch line.
    pub fn line(&self) -> String {
        format!("fireflies: {}", self.describe())
    }

    /// The line for a change from `before`.
    pub fn change_line(&self, before: &Self) -> String {
        format!(
            "fireflies: {} (was: {})",
            self.describe(),
            before.describe()
        )
    }
}

/// The pool's particle count: the `debug.fosfora.count` knob's when it is
/// set, else the preset's, never past [`POOL_CAP`] and never under
/// `min_rows`, the aux rows the XR inputs need (the aux buffer holds one
/// row per particle).
pub fn pool(knob: Option<u32>, preset: u32, min_rows: u32) -> u32 {
    knob.unwrap_or(preset)
        .max(min_rows)
        .min(POOL_CAP.max(min_rows))
}

/// The most particles one throw's impact may ask for
/// (`debug.fosfora.burstcount`): the slots the fireflies never take.
pub const BURST_MAX: u32 = POOL_CAP - FIREFLY_CAP;

// The default burst fits them.
const _: () = assert!(crate::instruments::BURST_COUNT <= BURST_MAX);

/// A throw's impact count as asked, at most [`BURST_MAX`].
pub fn burst_count(asked: u32) -> u32 {
    asked.min(BURST_MAX)
}

/// The fraction of the living hand particles the sim takes for a burst of
/// `count` this frame, besides the dead hand slots it claims first
/// (`instruments::steal_fraction` on the hands' share of the pool).
/// `alive` is the alive count as last read back, `pool` the particle count
/// and `fireflies` the fireflies asked for, whose slots and living
/// particles are not the hands'.
pub fn hand_steal(count: u32, alive: u32, pool: u32, fireflies: u32) -> f32 {
    let fireflies = fireflies.min(pool);
    crate::instruments::steal_fraction(count, alive.saturating_sub(fireflies), pool - fireflies)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surfaces::{KIND_FLOOR, KIND_TABLE, KIND_WALL};

    const PRESET: &str = include_str!("../../../assets/xr/effects/fireflies_xr_world.pfx");
    const SIM: &str = include_str!("../../../assets/xr/shaders/fireflies_xr_sim.wgsl");
    /// What `EffectLoader::prepend_compute_libraries` puts in front of a
    /// compute shader: the shared libraries, then the particle library.
    const PREAMBLE: [&str; 7] = [
        include_str!("../../../assets/shaders/lib/noise.wgsl"),
        include_str!("../../../assets/shaders/lib/palette.wgsl"),
        include_str!("../../../assets/shaders/lib/sdf.wgsl"),
        include_str!("../../../assets/shaders/lib/tonemap.wgsl"),
        include_str!("../../../assets/shaders/lib/chronoflow.wgsl"),
        include_str!("../../../assets/shaders/lib/overlay_lib.wgsl"),
        include_str!("../../../assets/shaders/lib/particle_lib.wgsl"),
    ];

    fn sim_module() -> naga::Module {
        let source = format!("{}\n{SIM}", PREAMBLE.join("\n"));
        let module = naga::front::wgsl::parse_str(&source).expect("fireflies_xr_sim.wgsl parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("fireflies_xr_sim.wgsl validates");
        module
    }

    fn sim_literal(module: &naga::Module, name: &str) -> naga::Literal {
        let (_, c) = module
            .constants
            .iter()
            .find(|(_, c)| c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("const {name}"));
        match module.global_expressions[c.init] {
            naga::Expression::Literal(v) => v,
            ref e => panic!("const {name} is {e:?}"),
        }
    }

    fn sim_u32(module: &naga::Module, name: &str) -> u32 {
        match sim_literal(module, name) {
            naga::Literal::U32(v) => v,
            v => panic!("const {name} is {v:?}"),
        }
    }

    fn sim_f32(module: &naga::Module, name: &str) -> f32 {
        match sim_literal(module, name) {
            naga::Literal::F32(v) => v,
            v => panic!("const {name} is {v:?}"),
        }
    }

    fn preset() -> serde_json::Value {
        serde_json::from_str(PRESET).expect("fireflies_xr_world.pfx parses")
    }

    fn knobs_of(pairs: &[(&str, &str)]) -> Knobs {
        Knobs::from_knobs(|name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        })
    }

    /// Both caps, wherever a number could get past them: the constants,
    /// the preset's pool, the sim's own cap, the count knob at any value
    /// and density, the pool at any `count` knob, and a throw's burst.
    #[test]
    fn the_caps_hold() {
        assert_eq!(FIREFLY_CAP, 3_000);
        assert_eq!(POOL_CAP, 20_000);
        // The preset's pool is the cap, and the sim's cap is the constant.
        let max_count = preset()["particles"]["max_count"].as_u64().unwrap();
        assert_eq!(max_count, u64::from(POOL_CAP));
        let module = sim_module();
        assert_eq!(sim_u32(&module, "XR_FLY_CAP"), FIREFLY_CAP);
        assert!(SIM.contains("min(bitcast<u32>(aux[XR_AUX_FIREFLIES].home.x), XR_FLY_CAP)"));
        // The count knob: no value asks for more than the cap.
        assert!(Knobs::default().count <= FIREFLY_CAP);
        for value in [
            "0",
            "1",
            "2999",
            "3000",
            "3001",
            "20000",
            "4000000000",
            "1e30",
            "-5",
            "inf",
            "NaN",
            "many",
            "",
        ] {
            let k = knobs_of(&[(KNOB_COUNT, value)]);
            assert!(k.count <= FIREFLY_CAP, "{value}: {}", k.count);
            for density in [0.0, 0.05, 0.3, 1.0, 7.0, -1.0, f32::NAN, f32::INFINITY] {
                let n = k.fireflies(density);
                assert!(
                    n <= k.count && n <= FIREFLY_CAP,
                    "{value} at {density}: {n}"
                );
                let rows = k.rows(density, 0.5, &Look::default(), 0.0);
                assert_eq!(rows[0][0].to_bits(), n);
            }
        }
        assert_eq!(knobs_of(&[(KNOB_COUNT, "3001")]).count, FIREFLY_CAP);
        assert_eq!(knobs_of(&[(KNOB_COUNT, "-5")]).count, 0);
        // A count set by hand past the cap is still held to it.
        let over = Knobs {
            count: FIREFLY_CAP + 500,
            ..Knobs::default()
        };
        assert_eq!(over.fireflies(1.0), FIREFLY_CAP);
        // The pool: the preset's, or the count knob's, never past the cap.
        let rows = 1242;
        assert_eq!(pool(None, POOL_CAP, rows), POOL_CAP);
        for knob in [
            None,
            Some(1),
            Some(5_000),
            Some(20_000),
            Some(20_001),
            Some(u32::MAX),
        ] {
            for preset in [0, 2_000, 20_000, 300_000, u32::MAX] {
                let n = pool(knob, preset, rows);
                assert!((rows..=POOL_CAP).contains(&n), "{knob:?} {preset}: {n}");
            }
        }
        assert_eq!(pool(Some(5_000), POOL_CAP, rows), 5_000);
        assert_eq!(pool(Some(500_000), POOL_CAP, rows), POOL_CAP);
        assert_eq!(pool(Some(1), POOL_CAP, rows), rows);
        // A throw: at most the slots the fireflies never take.
        assert_eq!(BURST_MAX, 17_000);
        assert_eq!(burst_count(6_000), 6_000);
        assert_eq!(burst_count(1_000_000), BURST_MAX);
    }

    #[test]
    fn each_knob_keeps_its_range_and_a_bad_value_the_default() {
        let d = Knobs::default();
        assert_eq!(Knobs::from_knobs(|_| None), d);
        assert_eq!(
            (
                d.count,
                d.size,
                d.brightness,
                d.drift,
                d.music,
                d.gravity,
                d.edge_flow
            ),
            (2_000, 0.02, 0.8, 0.06, 0.5, 2.5, 1.0)
        );
        let set = knobs_of(&[
            (KNOB_COUNT, " 1500 "),
            (KNOB_SIZE, "0.03"),
            (KNOB_BRIGHTNESS, "1.2"),
            (KNOB_DRIFT, "0.1"),
            (KNOB_MUSIC, "0.25"),
            (KNOB_GRAVITY, "4"),
            (KNOB_EDGE_FLOW, "0.5"),
        ]);
        assert_eq!(
            set,
            Knobs {
                count: 1_500,
                count_forced: true,
                size: 0.03,
                brightness: 1.2,
                drift: 0.1,
                music: 0.25,
                gravity: 4.0,
                edge_flow: 0.5,
            }
        );
        let high = knobs_of(&[
            (KNOB_SIZE, "1"),
            (KNOB_BRIGHTNESS, "9"),
            (KNOB_DRIFT, "9"),
            (KNOB_MUSIC, "9"),
            (KNOB_GRAVITY, "99"),
            (KNOB_EDGE_FLOW, "9"),
        ]);
        assert_eq!(
            (
                high.size,
                high.brightness,
                high.drift,
                high.music,
                high.gravity,
                high.edge_flow
            ),
            (
                SIZE_RANGE_M[1],
                BRIGHTNESS_MAX,
                DRIFT_MAX_M_S,
                1.0,
                HAND_GRAVITY_MAX,
                EDGE_FLOW_MAX
            )
        );
        let low = knobs_of(&[
            (KNOB_SIZE, "-1"),
            (KNOB_BRIGHTNESS, "-1"),
            (KNOB_DRIFT, "-1"),
            (KNOB_MUSIC, "-1"),
            (KNOB_GRAVITY, "-1"),
            (KNOB_EDGE_FLOW, "-1"),
        ]);
        assert_eq!(
            (
                low.size,
                low.brightness,
                low.drift,
                low.music,
                low.gravity,
                low.edge_flow
            ),
            (SIZE_RANGE_M[0], 0.0, 0.0, 0.0, 0.0, 0.0)
        );
        for bad in ["", "fast", "NaN", "inf"] {
            let k = knobs_of(&[
                (KNOB_COUNT, bad),
                (KNOB_SIZE, bad),
                (KNOB_BRIGHTNESS, bad),
                (KNOB_DRIFT, bad),
                (KNOB_MUSIC, bad),
                (KNOB_GRAVITY, bad),
                (KNOB_EDGE_FLOW, bad),
            ]);
            assert_eq!(k, d, "{bad:?}");
        }
    }

    #[test]
    fn the_rows_carry_the_knobs_in_the_sims_lanes() {
        let k = Knobs {
            count: 1_000,
            count_forced: false,
            size: 0.012,
            brightness: 0.9,
            drift: 0.05,
            music: 0.4,
            gravity: 3.0,
            edge_flow: 1.5,
        };
        let fireflies = Look::default();
        let rows = k.rows(0.5, 0.25, &fireflies, 0.0);
        assert_eq!(rows[0][0].to_bits(), 500);
        for (got, want) in rows[0][1..].iter().zip([0.012, 0.9, 0.05]) {
            assert_close!(*got, want);
        }
        for (got, want) in rows[1].iter().zip([0.4, 3.0, 1.5, 0.25]) {
            assert_close!(*got, want);
        }
        // The swell is a level, 0..1, and a bad one is silence.
        assert_close!(k.rows(1.0, 3.0, &fireflies, 0.0)[1][3], 1.0);
        assert_close!(k.rows(1.0, -3.0, &fireflies, 0.0)[1][3], 0.0);
        assert_close!(k.rows(1.0, f32::NAN, &fireflies, 0.0)[1][3], 0.0);
        // With no theme the air's two rows are all zero: the fireflies as
        // they were, which is also what a sim reads before anything is
        // written.
        assert_eq!(rows[2..], [[0.0; 4]; 2]);
        // The log names every knob's value.
        assert_eq!(
            Knobs::default().line(),
            "fireflies: 2000 fireflies (cap 3000) · radius 0.02 m · brightness 0.8 · drift 0.06 m/s · music 0.5 · hand gravity 2.5 m/s2 · edge flow 1"
        );
        assert!(
            k.change_line(&Knobs::default())
                .contains("(was: 2000 fireflies")
        );
    }

    /// The sim's rows are the ones `scene.rs` uploads, and its constants
    /// the ones this module documents.
    #[test]
    fn the_sim_reads_the_rows_the_app_writes() {
        let module = sim_module();
        let get = |name: &str| sim_u32(&module, name);
        // The shared rows are where the Flux sim has them.
        assert_eq!(
            get("XR_AUX_SPHERES") + get("XR_MAX_SPHERES"),
            get("XR_AUX_BOX_CENTER")
        );
        assert_eq!(
            get("XR_AUX_BOX_CENTER") + get("XR_MAX_BOXES"),
            get("XR_AUX_BOX_ROT")
        );
        assert_eq!(
            get("XR_AUX_BOX_ROT") + get("XR_MAX_BOXES"),
            get("XR_AUX_BOX_HALF")
        );
        assert_eq!(get("XR_AUX_INSTRUMENTS"), 170);
        assert_eq!(
            get("XR_AUX_INSTRUMENT_ROWS") as usize,
            crate::instruments::INSTRUMENT_ROWS
        );
        assert_eq!(
            get("XR_AUX_INSTRUMENTS") + get("XR_AUX_INSTRUMENT_ROWS"),
            get("XR_AUX_DEPTH")
        );
        assert_eq!(
            get("XR_AUX_DEPTH_ROWS") as usize,
            crate::env_depth::COLLIDE_ROWS
        );
        assert_eq!(
            get("XR_AUX_DEPTH") + get("XR_AUX_DEPTH_ROWS"),
            get("XR_AUX_POUR")
        );
        assert_eq!(
            get("XR_AUX_POUR") as usize + 1 + crate::surfaces::SURFACE_LANE_ROWS,
            get("XR_AUX_CELLS") as usize
        );
        assert_eq!(get("XR_CELLS_MAX"), crate::box_cells::CELLS_MAX);
        assert_eq!(
            get("XR_AUX_CELL_ROWS") as usize,
            crate::box_cells::CELL_ROWS
        );
        // The fireflies' rows follow the box cell list.
        assert_eq!(
            get("XR_AUX_CELLS") as usize + crate::box_cells::ROWS,
            get("XR_AUX_FIREFLIES") as usize
        );
        assert_eq!(get("XR_AUX_FIREFLIES"), 1238);
        assert_eq!(get("XR_AUX_FIREFLY_ROWS") as usize, ROWS);
        // The air's two rows (board #5543) follow the knobs' two.
        assert_eq!(get("XR_AUX_AIR_COLOR"), get("XR_AUX_FIREFLIES") + 2);
        assert_eq!(get("XR_AUX_AIR"), get("XR_AUX_FIREFLIES") + 3);
        assert_eq!(
            get("XR_AUX_FIREFLIES") + get("XR_AUX_FIREFLY_ROWS"),
            get("XR_AUX_AIR") + 1
        );
        assert_close!(sim_f32(&module, "XR_EDGE_BAND_M"), EDGE_BAND_M);
        assert_close!(
            sim_f32(&module, "XR_POUR_SPREAD"),
            crate::instruments::POUR_SPREAD_DEG.to_radians()
        );
        // The sim-cut mask's bits are `sim_cut`'s.
        for (i, part) in crate::sim_cut::PARTS.iter().enumerate() {
            let name = format!("XR_CUT_{}", part.to_ascii_uppercase());
            assert_eq!(get(&name), 1 << i, "{name}");
        }
    }

    /// What the motion may not use: anything that changes direction from
    /// one frame to the next, a bounce, and the beat's phase, which the
    /// flash limit holds.
    #[test]
    fn the_sim_has_no_jitter_no_bounce_and_no_beat_phase() {
        let module = sim_module();
        assert_close!(sim_f32(&module, "XR_HAND_RESTITUTION"), 0.0);
        // The code, without its comments.
        let code: String = SIM
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        for banned in [
            "sample_flow_field",
            "fosfora_noise",
            "curl_noise",
            "u.beat_phase",
            "u.beat",
            "u.onset",
            "u.kick",
            "u.flux",
            "u.turbulence",
        ] {
            assert!(!code.contains(banned), "the sim uses {banned}");
        }
        // The light reads the air's band as the flash limit lets it
        // through, and nothing else of the music: the loudness, the bass,
        // the mids or the highs (board #5543), read in `xr_band_level`.
        assert!(code.contains("let light = 1.0 + music * XR_FLY_MUSIC_LIGHT * level;"));
        assert!(code.contains("let level = xr_band_level(xr_air_band(idx));"));
        assert!(code.contains("return clamp(u.rms, 0.0, 1.0);"));
    }

    /// A vector constant's lanes.
    fn sim_vec(module: &naga::Module, name: &str) -> Vec<f32> {
        let (_, c) = module
            .constants
            .iter()
            .find(|(_, c)| c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("const {name}"));
        let naga::Expression::Compose { ref components, .. } = module.global_expressions[c.init]
        else {
            panic!("const {name} is not a vector");
        };
        components
            .iter()
            .map(|&h| match module.global_expressions[h] {
                naga::Expression::Literal(naga::Literal::F32(v)) => v,
                ref e => panic!("const {name} holds {e:?}"),
            })
            .collect()
    }

    /// Board #5543: what a theme gives the air reaches the sim's rows, and
    /// a room with no theme has the fireflies as they were.
    #[test]
    fn the_air_is_the_themes_and_the_fireflies_with_none() {
        use crate::surfaces::{AMBER, GREEN, ROSE, TEAL, WARM_WHITE};
        let none = Look::of(None);
        assert_eq!(none, Look::default());
        assert_eq!(
            (none.stamp, none.tint, none.band, none.motion),
            (0, Tint::Own, BAND_RMS, Motion::Wander)
        );
        assert_eq!(none.rows(1.5), [[0.0; 4], [0.0, 0.0, 0.0, 1.5]]);
        assert_eq!(none.line(None), "air: the fireflies (no theme saved)");
        // Each theme its own stamp, never the fireflies' 0, so a change of
        // theme is a change of air.
        let mut stamps: Vec<u32> = Theme::ALL
            .into_iter()
            .map(|t| Look::of(Some(t)).stamp)
            .collect();
        assert!(stamps.iter().all(|s| *s > 0));
        stamps.sort_unstable();
        stamps.dedup();
        assert_eq!(stamps.len(), Theme::ALL.len());
        // The video's three, then the rest: the color, the band, the
        // motion.
        let air = |t: Theme| {
            let l = Look::of(Some(t));
            (l.tint, l.band, l.motion)
        };
        let color = Tint::Color;
        assert_eq!(
            air(Theme::Aurora),
            (color(WARM_WHITE), BAND_HIGH, Motion::Twinkle)
        );
        assert_eq!(air(Theme::Flint), (color(AMBER), BAND_BASS, Motion::Rise));
        assert_eq!(
            air(Theme::Contour),
            (color(GREEN), BAND_HIGH, Motion::Wander)
        );
        assert_eq!(air(Theme::Drift), (color(TEAL), BAND_HIGH, Motion::Drift));
        assert_eq!(air(Theme::Tide), (color(ROSE), BAND_BASS, Motion::Rise));
        assert_eq!(
            air(Theme::Mirror),
            (color(WARM_WHITE), BAND_HIGH, Motion::Orbit)
        );
        assert_eq!(
            air(Theme::Classic),
            (Tint::ByBand, BAND_HIGH, Motion::Wander)
        );
        // The rows: the color and the tint's number, then the stamp, the
        // motion, the band and the orbit's phase.
        let aurora = Look::of(Some(Theme::Aurora));
        let [c, a] = aurora.rows(0.25);
        assert_close!(c, [WARM_WHITE[0], WARM_WHITE[1], WARM_WHITE[2], 1.0]);
        assert_close!(a, [aurora.stamp as f32, 1.0, BAND_HIGH as f32, 0.25]);
        assert_close!(Look::of(Some(Theme::Classic)).rows(0.0)[0][3], 2.0);
        assert_close!(Look::of(Some(Theme::Flint)).rows(f32::NAN)[1][3], 0.0);
        let knobs = Knobs::default();
        assert_eq!(knobs.rows(1.0, 0.0, &aurora, 0.25)[2..], [c, a]);
        // The words.
        assert_eq!(
            aurora.line(Some(Theme::Aurora)),
            "air: warm white, high, twinkle (theme aurora)"
        );
        assert_eq!(
            Look::of(Some(Theme::Classic)).line(Some(Theme::Classic)),
            "air: by band, high, wander (theme classic)"
        );
        // The level the size swells with: the air's band, gliding; the
        // loudness for the fireflies and for an air tinted by band.
        let levels = [0.2, 0.7, 0.4, 0.9];
        assert_close!(none.level(levels), 0.2);
        assert_close!(Look::of(Some(Theme::Flint)).level(levels), 0.7);
        assert_close!(aurora.level(levels), 0.9);
        assert_close!(Look::of(Some(Theme::Classic)).level(levels), 0.2);
        assert_close!(aurora.level([0.0, 0.0, 0.0, 7.0]), 1.0);
        assert_close!(aurora.level([0.0, 0.0, 0.0, f32::NAN]), 0.0);
    }

    /// The count is the theme's, held to the cap, unless the count's knob
    /// is set; with no theme it is the knob's or the default.
    #[test]
    fn the_count_is_the_themes_unless_the_knob_is_set() {
        let d = Knobs::default();
        assert!(!d.count_forced);
        assert_eq!(d.under(None), d);
        for t in Theme::ALL {
            let k = d.under(Some(t));
            assert_eq!(k.count, t.air().count, "{t:?}");
            assert!(k.count <= FIREFLY_CAP);
            assert_eq!(
                Knobs {
                    count: d.count,
                    ..k
                },
                d,
                "{t:?}"
            );
        }
        assert_eq!(d.under(Some(Theme::Flint)).count, 1_500);
        assert_eq!(d.under(Some(Theme::Tide)).count, 1_000);
        // The knob at a number wins, 0 too; unset, empty or unreadable
        // leaves the count the theme's.
        for (value, forced, flint) in [
            ("800", true, 800),
            ("0", true, 0),
            ("9999", true, FIREFLY_CAP),
            ("", false, 1_500),
            (" ", false, 1_500),
            ("many", false, 1_500),
            ("NaN", false, 1_500),
        ] {
            let k = knobs_of(&[(KNOB_COUNT, value)]);
            assert_eq!(k.count_forced, forced, "{value:?}");
            assert_eq!(k.under(Some(Theme::Flint)).count, flint, "{value:?}");
        }
        assert!(!knobs_of(&[(KNOB_SIZE, "0.03")]).count_forced);
    }

    /// The sim's side of the air: its numbers for the motions and the
    /// bands are this crate's, and each motion's constants keep what the
    /// motion promises.
    #[test]
    fn the_sims_air_is_the_one_the_rows_name() {
        let module = sim_module();
        let u = |name: &str| sim_u32(&module, name);
        let f = |name: &str| sim_f32(&module, name);
        let v = |name: &str| sim_vec(&module, name);
        // The motions' numbers.
        for (motion, name) in [
            (Motion::Wander, "XR_AIR_WANDER"),
            (Motion::Twinkle, "XR_AIR_TWINKLE"),
            (Motion::Rise, "XR_AIR_RISE"),
            (Motion::Drift, "XR_AIR_DRIFT"),
            (Motion::Orbit, "XR_AIR_ORBIT"),
        ] {
            assert_eq!(u(name), motion_id(motion), "{name}");
        }
        assert_eq!(Motion::ALL.len(), 5);
        assert_close!(f("XR_TINT_COLOR"), 1.0);
        assert_close!(f("XR_TINT_BAND"), 2.0);
        // The bands' numbers are the surfaces' (`surface_fx::BAND_*`).
        assert_eq!((BAND_RMS, BAND_BASS, BAND_MID, BAND_HIGH), (0, 1, 2, 3));
        for (band, read) in [
            ("1u", "clamp(u.bass, 0.0, 1.0)"),
            ("2u", "clamp(u.mid, 0.0, 1.0)"),
            ("3u", "clamp(max(u.presence, u.brilliance), 0.0, 1.0)"),
        ] {
            let want = format!("if band == {band} {{\n        return {read};");
            assert!(SIM.contains(&want), "{want}");
        }
        // By band: the bass, the mids and the highs in Classic's colors.
        assert_close!(v("XR_BAND_BASS"), BAND_COLORS[0].to_vec());
        assert_close!(v("XR_BAND_MID"), BAND_COLORS[1].to_vec());
        assert_close!(v("XR_BAND_HIGH"), BAND_COLORS[2].to_vec());
        assert_close!(BAND_COLORS[0], crate::surfaces::palette(KIND_FLOOR));
        assert_close!(BAND_COLORS[1], crate::surfaces::palette(KIND_TABLE));
        assert_close!(BAND_COLORS[2], crate::surfaces::palette(KIND_WALL));
        // The fastest any particle goes against the drift speed.
        let top = 1.4 / 1.1;
        // A star is nearly still, high in the room, and its own flicker
        // turns about once a second at most: a third of the three a
        // second the flash rule allows, were one star a region.
        assert!(DRIFT_M_S * top * f("XR_STAR_DRIFT") < 0.01);
        assert!(f("XR_STAR_Y_LOW") > 0.0 && f("XR_STAR_Y_LOW") < f("XR_FLY_Y_HIGH"));
        let star = v("XR_STAR_PERIOD_S");
        assert!(1.0 / star[0] < 1.2 && star[0] < star[1]);
        assert!(f("XR_STAR_CALM") > f("XR_FLY_GLOW_LOW") && f("XR_STAR_CALM") < 1.0);
        // A rising one is born on the floor, on average climbs past a
        // seated eye (1.2 m), and the fastest over the longest life is
        // still inside the volume, so it ends by fading and not by being
        // born again. It fades over the last half of its life.
        let (low, high, out) = (f("XR_FLY_Y_LOW"), f("XR_FLY_Y_HIGH"), f("XR_FLY_OUT"));
        let half = 3.0;
        let life = v("XR_RISE_LIFE_S");
        let up = DRIFT_M_S * f("XR_RISE_SPEED");
        assert!(f("XR_RISE_BIRTH") <= 0.1);
        assert!(up / 1.1 * 0.5 * (life[0] + life[1]) > 1.2 + 0.3);
        assert!(up * top * life[1] < (high + (out - 1.0) - low) * half);
        assert!((0.3..=0.7).contains(&f("XR_RISE_FADE")));
        assert!(f("XR_RISE_IDLE") > 0.0 && f("XR_RISE_IDLE") < 0.2);
        assert!(f("XR_RISE_SWAY") < 0.5);
        // The drift's way is one level direction, and a particle comes
        // back in where it is unseen: past the bound and well short of
        // where one is taken as out.
        let dir = glam::Vec3::from_slice(&v("XR_DRIFT_DIR"));
        assert!((dir.length() - 1.0).abs() < 1e-3, "{dir}");
        assert_close!(dir.y, 0.0);
        assert!(f("XR_DRIFT_WRAP") > 1.0 && f("XR_DRIFT_WRAP") < out);
        assert!(f("XR_DRIFT_WOBBLE") < 0.5);
        // The old air fades out faster than a firefly thins.
        assert!(f("XR_AIR_SWAP_S") < f("XR_FLY_FADE_S"));
        // The wander is the fireflies' own, as it was.
        assert!(SIM.contains(
            "let dir = vec3f(cos(a) + XR_FLY_LOOP * cos(b), 0.5 * sin(c), sin(a) + XR_FLY_LOOP * sin(b));"
        ));
    }

    /// The orbit: one phase for every particle, stepped by the app, and a
    /// particle's radius and height its own, so it keeps both.
    #[test]
    fn an_orbit_keeps_its_radius_and_turns_with_the_phase() {
        use std::f32::consts::TAU;
        let module = sim_module();
        let slab = [
            sim_f32(&module, "XR_FLY_Y_LOW"),
            sim_f32(&module, "XR_FLY_Y_HIGH"),
        ];
        let span = sim_f32(&module, "XR_FLY_SPAN");
        let own = (0.36, 1.0, 0.5);
        let at = |phase: f32| glam::Vec3::from(orbit_place(phase, own, 3.0, span, slab));
        let level = |p: glam::Vec3| glam::Vec2::new(p.x, p.z).length();
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        let start = at(0.0);
        // The square root of its own share of the way out, so the
        // particles fill the disk evenly.
        assert!(near(level(start), 0.6 * 3.0 * span));
        assert!(near(start.y, (slab[0] + slab[1]) * 0.5 * 3.0));
        let mut phase = 0.0;
        for _ in 0..60 * 72 {
            phase = orbit_step(phase, 1.0 / 72.0, DRIFT_M_S);
            assert!((0.0..TAU).contains(&phase));
            let p = at(phase);
            assert!(near(level(p), level(start)), "{p}");
            assert!(near(p.y, start.y), "{p}");
        }
        // A minute at the default drift: 0.15 rad/s, a turn in about 42 s.
        assert_close!(DRIFT_M_S * ORBIT_RAD_PER_M, 0.15);
        assert!((phase - (60.0 * 0.15f32).rem_euclid(TAU)).abs() < 1e-2);
        // Half a turn puts it across the anchor.
        let across = at(std::f32::consts::PI);
        assert!(near(across.x, -start.x) && near(across.z, -start.z));
        // No drift, no turn; bad input starts over.
        assert_close!(orbit_step(1.0, 1.0, 0.0), 1.0);
        assert_close!(orbit_step(f32::NAN, 1.0, 0.1), 0.0);
        assert_close!(orbit_step(1.0, f32::INFINITY, 0.1), 0.0);
    }

    #[test]
    fn the_preset_is_the_hidden_world_effect() {
        let p = preset();
        assert_eq!(p["name"], EFFECT);
        assert_eq!(p["hidden"], true);
        let particles = &p["particles"];
        assert_eq!(
            particles["compute_shader"],
            "../xr/shaders/fireflies_xr_sim.wgsl"
        );
        // No flow field: the sim samples none.
        assert_eq!(particles["flow_field"], false);
        // The burst's sprite, half a firefly's.
        assert_close!(
            particles["initial_size"].as_f64().unwrap() as f32,
            SIZE_M / 2.0
        );
    }

    #[test]
    fn a_burst_steals_from_the_hands_share_alone() {
        // With 3,000 fireflies the pool has 17,000 slots for the hands.
        let (pool, flies) = (POOL_CAP, FIREFLY_CAP);
        // Only fireflies alive: every hand slot is dead, nothing to steal.
        assert_close!(hand_steal(6_000, flies, pool, flies), 0.0);
        // The hands' share full: the burst comes from the living, capped.
        assert_close!(
            hand_steal(1_500, pool, pool, flies),
            crate::instruments::MAX_STEAL
        );
        // 16,000 hand particles alive, 1,000 dead slots, a burst of 1,500:
        // 500 of the 16,000.
        assert_close!(
            hand_steal(1_500, flies + 16_000, pool, flies),
            500.0 / 16_000.0
        );
        // No burst, nothing taken.
        assert_close!(hand_steal(0, pool, pool, flies), 0.0);
    }
}
