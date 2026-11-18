//! The flash measurement (board #5451): each theme drawn on the desktop GPU
//! as a seated wearer sees it, in a scanned room with the room black, at 72
//! frames a second, and the rule counted per region about 10 degrees
//! across ([`crate::flash::Counter`]).
//!
//! **What is drawn.** A 4.6 by 3.8 m room, 2.45 m high, with a table ahead,
//! a frame on the right wall and a chair on the left, each running its
//! theme's look (`themes::Theme::look`) as `app.rs` assembles a frame: the
//! surfaces pass's faces (`surface_fx::rows`), the ported Aurora as the sky
//! and its reflection (`surface_port::uniforms`, `sky::rows`), the wall
//! spectrum on the wall ahead, the floor's rings, and the edge bars in each
//! kind's mode with their history, all at immersion level 1 (the room
//! black, the passthrough's light gone: the worst contrast) and the music
//! playing. The light is drawn premultiplied over black, the faces first
//! and the edge bars over them, as the compositor stacks the layers. Not
//! drawn: the depth light (it needs a depth map), the world effect's
//! particles (no theme emits any), the hands and occluders. The depth
//! light's sweep (board #5511, `sonar.rs`) is counted apart, on the depth
//! light's Rust copy over flat surfaces, with no adapter
//! (`the_sweep_over_the_depth_light_stays_within_three_flashes`).
//!
//! **The view.** A cube around the seated eye, six faces of [`FACE_PX`]
//! pixels across 90 degrees, so every way the wearer can look is measured.
//! Each face is cut into regions [`CELL_DEG`] degrees across each way,
//! the rule's 10 degree field, every [`STEP_DEG`] degrees, so neighbors
//! overlap by half and a flash across a cut is still seen whole. The cuts
//! are at even angles, not even pixels: a face's pixels shrink in angle
//! toward its edges, and cells of even pixels would measure fields of 6
//! degrees there.
//! A compute pass averages each region's linear light per frame, clamped
//! to 1 as the display clamps it, and the counters read the averages back
//! in batches.
//!
//! **The audio.** A [`Source`] gives each frame what the app's frame loop
//! gets from the analysis: the features (between two hops, blended as the
//! core's render-side interpolation blends them), the newest mel column,
//! and whether a beat, downbeat or drop fired since the last frame. A
//! pulse train ([`Pulse`]) drives every level the looks read from 0 to 1
//! and back at once, with a beat on each rise; a dump ([`Dump`]) replays a
//! song's analysis, made outside the repo from the core's own analysis.
//!
//! `#[ignore]`d like the repo's other GPU tests: they need an adapter. Run
//! them with `cargo test -p fosfora-xr --lib flash_room -- --ignored
//! --nocapture`; `FOSFORA_FLASH_DUMPS=<dir>` adds the songs in it.

use std::f32::consts::FRAC_PI_2;

use fosfora_app::audio::features::{AudioFeatures, NUM_FEATURES};
use fosfora_app::effect::EffectLoader;
use glam::{Mat3, Quat, Vec3};

use crate::canvas::{Canvas, DEFAULT_BARS, WallFace, WallPick};
use crate::edge_bars::{self, Choice, History, Outline, Presence, Shape};
use crate::flash::{Counter, Limiter, Worst};
use crate::math::{self, Fov};
use crate::ripple::Ripple;
use crate::sky::{self, Map, Room, Sky};
use crate::surface_fx::{self, Audio, Clock, Slot, Spectrum};
use crate::surface_port::{self, Effect};
use crate::surfaces::{
    COLOR_KEY, Face, KIND_CEILING, KIND_FLOOR, KIND_FRAME, KIND_OTHER, KIND_TABLE, KIND_WALL,
    PLANE_HALF_THICKNESS_M, SurfaceBehavior, acting_face, key_tint, surface_color,
};
use crate::themes::{self, Theme};

/// Frames a second, the Quest 3's default display rate.
pub const FPS: f64 = 72.0;
/// A cube face's size (px) over its 90 degrees.
pub const FACE_PX: u32 = 160;
/// A region's size (px: 10 degrees at a face's center) and the step
/// between regions.
pub const CELL_DEG: u32 = 10;
pub const STEP_DEG: u32 = 5;
/// Regions along a face's side, and in the whole cube.
pub const CELLS: u32 = (90 - CELL_DEG) / STEP_DEG + 1;
pub const REGIONS: usize = (6 * CELLS * CELLS) as usize;
/// Frames read back at once.
const BATCH: usize = 256;
/// The immersion level the light is drawn at: 1, the room black.
const LEVEL: f32 = 1.0;
/// The mel bands the analysis makes.
pub const MELS: usize = 64;

/// The cube's faces: where each looks and what is up in it.
const FACES: [(&str, Vec3, Vec3); 6] = [
    ("ahead", Vec3::NEG_Z, Vec3::Y),
    ("behind", Vec3::Z, Vec3::Y),
    ("left", Vec3::NEG_X, Vec3::Y),
    ("right", Vec3::X, Vec3::Y),
    ("up", Vec3::Y, Vec3::Z),
    ("down", Vec3::NEG_Y, Vec3::NEG_Z),
];

/// The pixel at `deg` degrees off a face's center along one axis (-45 to
/// 45), rounded to the nearest pixel edge.
pub fn edge_px(deg: f32) -> u32 {
    ((deg.to_radians().tan() + 1.0) * 0.5 * FACE_PX as f32).round() as u32
}

/// Where region `r` looks: its face's name and its center's direction off
/// the face's center (degrees right and up).
pub fn region_name(r: usize) -> String {
    let per = (CELLS * CELLS) as usize;
    let (face, c) = (r / per, r % per);
    let (cx, cy) = ((c % CELLS as usize) as u32, (c / CELLS as usize) as u32);
    let mid = |k: u32| (k * STEP_DEG + CELL_DEG / 2) as i32 - 45;
    format!("{} {:+}° {:+}°", FACES[face].0, mid(cx), -mid(cy))
}

/// One frame's audio as the frame loop gets it.
#[derive(Debug, Clone, Copy)]
pub struct HopAt {
    pub features: AudioFeatures,
    pub mel: [f32; MELS],
    pub beat: bool,
    pub downbeat: bool,
    pub drop: bool,
}

/// Where a run's audio comes from.
pub trait Source {
    /// How long it plays (s).
    fn duration(&self) -> f64;
    /// The frame at `t` (s), `dt` after the last.
    fn at(&mut self, t: f64, dt: f64) -> HopAt;
}

/// Feature slots by name (the order of `AudioFeatures`, frozen).
pub mod slot {
    pub const BEAT: usize = 16;
    pub const BEAT_PHASE: usize = 17;
    pub const BPM: usize = 18;
    pub const LOUDNESS_M: usize = 46;
    pub const LOUDNESS_S: usize = 47;
    pub const KEY_CLASS: usize = 49;
    pub const KEY_IS_MINOR: usize = 50;
    pub const KEY_CONFIDENCE: usize = 51;
    pub const DOWNBEAT: usize = 52;
    pub const BAR_PHASE: usize = 53;
    pub const BEAT_IN_BAR: usize = 54;
    pub const DROP: usize = 60;
    pub const BAR_INDEX: usize = 81;
    pub const BEAT_INDEX: usize = 82;
    pub const TEMPO_CONFIDENCE: usize = 83;
    pub const BEAT_LOCKED: usize = 84;
}

/// A pulse train at `hz`, full depth: every level the looks read (the
/// bands, the rms, the kick, the onset, the beat's strength, the spectral
/// shape, the energies, every mel band) at 1 for the first half of each
/// cycle and 0 for the second, a beat on each rise and a downbeat on every
/// fourth, the bar clock at the pulse's rate up to 240 BPM. The key is C major, sure
/// of itself: the key's tint is red, which takes Aurora's low curtains to
/// a saturated red. The loudness stays up, so the music counts as playing.
/// [`Pulse::hits`] is the same train with each pulse short, as a drum's
/// hit is: the levels at 1 for [`HIT_S`] of each cycle.
pub struct Pulse {
    pub hz: f64,
    pub secs: f64,
    /// The share of each cycle the levels are at 1.
    on: f64,
    last: Option<u64>,
}

/// How long a hit of [`Pulse::hits`] lasts (s): two frames.
pub const HIT_S: f64 = 2.0 / FPS;

impl Pulse {
    pub fn new(hz: f64, secs: f64) -> Self {
        Self {
            hz,
            secs,
            on: 0.5,
            last: None,
        }
    }

    /// Short hits at `hz`: what lets a bar fall furthest between two
    /// rises, since a bar rises at once and falls by time.
    pub fn hits(hz: f64, secs: f64) -> Self {
        Self {
            on: (HIT_S * hz).min(0.5),
            ..Self::new(hz, secs)
        }
    }
}

/// Slots that are not levels: the triggers, the clocks, the tempo, the key
/// and the loudness, which a pulse holds.
fn held(i: usize) -> bool {
    use slot::*;
    matches!(
        i,
        BEAT | BEAT_PHASE
            | BPM
            | LOUDNESS_M
            | LOUDNESS_S
            | KEY_CLASS
            | KEY_IS_MINOR
            | KEY_CONFIDENCE
            | DOWNBEAT
            | BAR_PHASE
            | BEAT_IN_BAR
            | DROP
            | BAR_INDEX
            | BEAT_INDEX
            | TEMPO_CONFIDENCE
            | BEAT_LOCKED
    )
}

impl Source for Pulse {
    fn duration(&self) -> f64 {
        self.secs
    }

    fn at(&mut self, t: f64, _dt: f64) -> HopAt {
        let cycles = t * self.hz;
        let k = cycles.floor() as u64;
        let level = if cycles.fract() < self.on { 1.0 } else { 0.0 };
        let beat = self.last.is_none_or(|l| k > l);
        self.last = Some(k);
        let mut f = AudioFeatures::default();
        let s = f.as_slice_mut();
        for (i, v) in s.iter_mut().enumerate() {
            if !held(i) {
                *v = level;
            }
        }
        // The clocks at the pulse's rate up to 240 BPM, the fastest the
        // beat tracker reports a bar at (a bar a second); the beats at
        // every pulse, as onsets would fire them.
        let bar_hz = self.hz.min(4.0) / 4.0;
        {
            use slot::*;
            s[BEAT] = f32::from(u8::from(beat));
            s[DOWNBEAT] = f32::from(u8::from(beat && k.is_multiple_of(4)));
            s[BEAT_PHASE] = cycles.fract() as f32;
            s[BAR_PHASE] = (t * bar_hz).fract() as f32;
            s[BEAT_IN_BAR] = ((t * bar_hz * 4.0).floor() % 4.0) as f32 / 4.0;
            s[BEAT_INDEX] = (t * bar_hz * 4.0).floor() as f32;
            s[BAR_INDEX] = (t * bar_hz).floor() as f32;
            s[BPM] = (bar_hz * 240.0 / 300.0) as f32;
            s[TEMPO_CONFIDENCE] = 1.0;
            s[BEAT_LOCKED] = 1.0;
            s[LOUDNESS_M] = 0.8;
            s[LOUDNESS_S] = 0.8;
            s[KEY_CLASS] = 0.0;
            s[KEY_IS_MINOR] = 0.0;
            s[KEY_CONFIDENCE] = 1.0;
        }
        HopAt {
            features: f,
            mel: [level; MELS],
            beat,
            downbeat: beat && k.is_multiple_of(4),
            drop: false,
        }
    }
}

/// A steady song: every level held at `level`, no beat. What the looks do
/// with the music standing still is their own motion alone.
pub struct Still {
    pub level: f32,
    pub secs: f64,
}

impl Source for Still {
    fn duration(&self) -> f64 {
        self.secs
    }

    fn at(&mut self, t: f64, dt: f64) -> HopAt {
        let mut hop = Pulse::new(0.0, self.secs).at(t, dt);
        for (i, v) in hop.features.as_slice_mut().iter_mut().enumerate() {
            if !held(i) {
                *v = self.level;
            }
        }
        let s = hop.features.as_slice_mut();
        s[slot::BEAT] = 0.0;
        s[slot::DOWNBEAT] = 0.0;
        s[slot::BPM] = 0.0;
        s[slot::BEAT_LOCKED] = 0.0;
        hop.mel = [self.level; MELS];
        hop.beat = false;
        hop.downbeat = false;
        hop
    }
}

/// One hop of a dump: its time, its triggers (1 beat, 2 downbeat, 4 drop),
/// its features and its mel column.
#[derive(Debug, Clone, Copy)]
struct DumpHop {
    ts: f64,
    flags: u32,
    features: [f32; NUM_FEATURES],
    mel: [f32; MELS],
}

/// A song's analysis, hop by hop, as the dump tool writes it: `FLSH`,
/// version 1, the feature and mel counts and the hop rate, then per hop
/// the time (f64), the triggers (u32), the features and the mel bands
/// (f32), all little-endian.
pub struct Dump {
    hops: Vec<DumpHop>,
    cursor: usize,
}

/// Slots blended between two hops as the core's interpolation does; the
/// rest (triggers, clocks, counters, the key) take the earlier hop's.
fn blended(i: usize) -> bool {
    use slot::*;
    !matches!(
        i,
        BEAT | BEAT_PHASE
            | BPM
            | KEY_CLASS
            | KEY_IS_MINOR
            | DOWNBEAT
            | BAR_PHASE
            | BEAT_IN_BAR
            | DROP
            | BAR_INDEX
            | BEAT_INDEX
            | BEAT_LOCKED
    )
}

impl Dump {
    pub fn read(path: &std::path::Path) -> std::io::Result<Self> {
        use std::io::{Error, ErrorKind};
        let bytes = std::fs::read(path)?;
        let bad = |what: &str| Error::new(ErrorKind::InvalidData, what.to_owned());
        if bytes.len() < 20 || &bytes[..4] != b"FLSH" {
            return Err(bad("not a flash dump"));
        }
        let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().expect("4 bytes"));
        let f32_at = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().expect("4 bytes"));
        let (nf, nm) = (u32_at(8) as usize, u32_at(12) as usize);
        if u32_at(4) != 1 || nf != NUM_FEATURES || nm != MELS {
            return Err(bad("another version or layout"));
        }
        let rec = 12 + 4 * (nf + nm);
        let hops = bytes[20..]
            .chunks_exact(rec)
            .enumerate()
            .map(|(k, _)| {
                let o = 20 + k * rec;
                let ts = f64::from_le_bytes(bytes[o..o + 8].try_into().expect("8 bytes"));
                let mut features = [0.0; NUM_FEATURES];
                for (i, v) in features.iter_mut().enumerate() {
                    *v = f32_at(o + 12 + 4 * i);
                }
                let mut mel = [0.0; MELS];
                for (i, v) in mel.iter_mut().enumerate() {
                    *v = f32_at(o + 12 + 4 * (nf + i));
                }
                DumpHop {
                    ts,
                    flags: u32_at(o + 8),
                    features,
                    mel,
                }
            })
            .collect::<Vec<_>>();
        if hops.len() < 2 {
            return Err(bad("too short"));
        }
        Ok(Self { hops, cursor: 0 })
    }
}

impl Source for Dump {
    fn duration(&self) -> f64 {
        self.hops.last().map_or(0.0, |h| h.ts)
    }

    fn at(&mut self, t: f64, dt: f64) -> HopAt {
        // The triggers of every hop since the last frame.
        let mut flags = 0;
        while self.cursor + 1 < self.hops.len() && self.hops[self.cursor + 1].ts <= t {
            self.cursor += 1;
            if self.hops[self.cursor].ts > t - dt {
                flags |= self.hops[self.cursor].flags;
            }
        }
        let a = &self.hops[self.cursor];
        let b = &self.hops[(self.cursor + 1).min(self.hops.len() - 1)];
        let span = (b.ts - a.ts).max(1e-9);
        let w = ((t - a.ts) / span).clamp(0.0, 1.0) as f32;
        let mut f = AudioFeatures::default();
        for (i, v) in f.as_slice_mut().iter_mut().enumerate() {
            *v = if blended(i) {
                a.features[i] + (b.features[i] - a.features[i]) * w
            } else {
                a.features[i]
            };
        }
        let (beat, downbeat, drop) = (flags & 1 != 0, flags & 2 != 0, flags & 4 != 0);
        let s = f.as_slice_mut();
        s[slot::BEAT] = f32::from(u8::from(beat));
        s[slot::DOWNBEAT] = f32::from(u8::from(downbeat));
        s[slot::DROP] = f32::from(u8::from(drop));
        HopAt {
            features: f,
            mel: a.mel,
            beat,
            downbeat,
            drop,
        }
    }
}

/// One box of the room: its kind, center, rotation and half extents, as
/// the runtime reports a scanned surface.
#[derive(Debug, Clone, Copy)]
struct RoomBox {
    kind: u32,
    center: Vec3,
    rot: Quat,
    half: Vec3,
}

/// The room, the seated eye in it and the sky over it.
struct Scene {
    boxes: Vec<RoomBox>,
    eye: Vec3,
    forward: Vec3,
    sky: Sky,
}

fn scene() -> Scene {
    let (w, d, h) = (4.6, 3.8, 2.45);
    let t = PLANE_HALF_THICKNESS_M;
    let wall = |out: Vec3, turn: f32, len: f32| RoomBox {
        kind: KIND_WALL,
        center: out + Vec3::Y * 0.5 * h,
        rot: Quat::from_rotation_y(turn),
        half: Vec3::new(0.5 * len, 0.5 * h, t),
    };
    // A box the runtime reports with local +Z up.
    let upright = Quat::from_rotation_x(-FRAC_PI_2);
    let boxes = vec![
        wall(Vec3::X * 0.5 * w, -FRAC_PI_2, d),
        wall(Vec3::NEG_X * 0.5 * w, FRAC_PI_2, d),
        wall(Vec3::Z * 0.5 * d, std::f32::consts::PI, w),
        wall(Vec3::NEG_Z * 0.5 * d, 0.0, w),
        RoomBox {
            kind: KIND_FLOOR,
            center: Vec3::ZERO,
            rot: upright,
            half: Vec3::new(0.5 * w, 0.5 * d, t),
        },
        RoomBox {
            kind: KIND_CEILING,
            center: Vec3::Y * h,
            rot: Quat::from_rotation_x(FRAC_PI_2),
            half: Vec3::new(0.5 * w, 0.5 * d, t),
        },
        // A table 1 m ahead, its top at 0.75 m.
        RoomBox {
            kind: KIND_TABLE,
            center: Vec3::new(0.1, 0.375, -1.0),
            rot: upright,
            half: Vec3::new(0.6, 0.35, 0.375),
        },
        // A frame on the right wall at eye height.
        RoomBox {
            kind: KIND_FRAME,
            center: Vec3::new(0.5 * w - 0.03, 1.5, -0.4),
            rot: Quat::from_rotation_y(-FRAC_PI_2),
            half: Vec3::new(0.4, 0.3, t),
        },
        // A chair on the left.
        RoomBox {
            kind: KIND_OTHER,
            center: Vec3::new(-1.1, 0.45, -0.4),
            rot: upright,
            half: Vec3::new(0.25, 0.25, 0.45),
        },
    ];
    let room = Room::fit(
        boxes
            .iter()
            .filter(|b| matches!(b.kind, KIND_WALL | KIND_FLOOR | KIND_CEILING))
            .map(|b| (b.kind, b.center, b.rot, b.half)),
    )
    .expect("the room's walls");
    let sky = Sky::new(room, sky::facing(Quat::IDENTITY));
    Scene {
        boxes,
        // `FOSFORA_FLASH_EYE=<x>,<z>` seats the eye elsewhere (m from the
        // room's middle), to count with a wall close by.
        eye: sky.point
            + std::env::var("FOSFORA_FLASH_EYE")
                .ok()
                .and_then(|v| {
                    let (x, z) = v.split_once(',')?;
                    Some(Vec3::new(x.parse().ok()?, 0.0, z.parse().ok()?))
                })
                .unwrap_or(Vec3::new(0.1, 0.0, 0.2)),
        forward: Vec3::NEG_Z,
        sky,
    }
}

/// One frame's draws: the surfaces pass's faces, the ported faces (their
/// effect's uniform and their rows), and the edge bars' rows with the
/// strip and tunnel counts.
#[derive(Default)]
struct Draws {
    faces: Vec<[[f32; 4]; surface_fx::UNIFORM_ROWS]>,
    ports: Vec<(usize, Vec<u8>, [[f32; 4]; surface_port::UNIFORM_ROWS])>,
    edge_rows: Vec<[f32; 4]>,
    strips: usize,
    tunnels: usize,
}

/// What a run draws: every part of the theme, or one of them alone, to
/// find which part flashes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    All,
    /// The surfaces pass's faces running one behavior.
    Behavior(SurfaceBehavior),
    /// Aurora as the sky, on the walls and the ceiling.
    Sky,
    /// Aurora's reflection on the floor.
    Reflection,
    /// The edge bars on the faces or standing.
    Strips,
    /// The edge bars' tunnels.
    Tunnels,
}

impl Part {
    pub fn name(self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Behavior(b) => b.name().to_owned(),
            Self::Sky => "sky".to_owned(),
            Self::Reflection => "reflection".to_owned(),
            Self::Strips => "edge strips".to_owned(),
            Self::Tunnels => "edge tunnels".to_owned(),
        }
    }
}

/// A theme's show: the state `app.rs` keeps from frame to frame for the
/// looks, stepped as it steps it, with the flash limiter where it sits
/// there (on or off).
struct Show {
    theme: Theme,
    /// Instead of the theme's looks: this behavior on every surface whose
    /// kind offers it (`SurfaceBehavior::catalogue`), in its own colors and
    /// band, nothing on the rest, and no edge bars (a hand edit's worst).
    every: Option<SurfaceBehavior>,
    part: Part,
    /// The edge bars' mode: each kind in its theme's, or one forced on
    /// every kind as the Edges control forces it.
    edges: Choice,
    /// Whether the edge bars' heights follow the analysis
    /// (`edge_bars::Heights`, board #5532).
    bars_free: bool,
    /// The ceiling's strength as the wearer's Strength row set it, instead
    /// of the theme's (board #5533).
    ceiling: Option<f32>,
    /// The ceiling's facets drawn at the strength asked, the limiter on or
    /// not: what the cap is found against.
    uncapped: bool,
    scene: Scene,
    effects: Vec<Effect>,
    limiter: Limiter,
    ripple: Ripple,
    canvas: Canvas,
    edge_heights: edge_bars::Heights,
    pick: WallPick,
    history: History,
    presence: Presence,
    outline: Outline,
    clock: Clock,
    terrain: surface_fx::Terrain,
    beat_env: f32,
    downbeat_env: f32,
    frame: u64,
}

impl Show {
    fn new(
        theme: Theme,
        every: Option<SurfaceBehavior>,
        part: Part,
        limit: bool,
        effects: Vec<Effect>,
    ) -> Self {
        Self {
            theme,
            every,
            part,
            edges: Choice::Theme,
            bars_free: false,
            ceiling: None,
            uncapped: false,
            scene: scene(),
            effects,
            limiter: Limiter::new(limit),
            ripple: Ripple::default(),
            canvas: Canvas::new(DEFAULT_BARS, 1.0),
            edge_heights: edge_bars::Heights::new(DEFAULT_BARS),
            pick: WallPick::default(),
            history: History::default(),
            presence: Presence::default(),
            outline: Outline::default(),
            clock: Clock::default(),
            terrain: surface_fx::Terrain::default(),
            beat_env: 0.0,
            downbeat_env: 0.0,
            frame: 0,
        }
    }

    /// What every surface of `kind` runs.
    fn look(&self, kind: u32) -> themes::Look {
        match self.every {
            Some(behavior) => themes::Look {
                // A look only a theme gives is on no kind's list: on
                // every kind.
                behavior: if behavior.theme_only()
                    || SurfaceBehavior::catalogue(kind).contains(&behavior)
                {
                    behavior
                } else {
                    SurfaceBehavior::None
                },
                color: 0,
                band: behavior.default_band(),
                strength: 1.0,
                edges: edge_bars::Mode::Off,
                mono: false,
            },
            None => {
                let mut look = self.theme.look(kind).expect("a named kind");
                if let Some(strength) = self.ceiling.filter(|_| kind == KIND_CEILING) {
                    look.strength = strength;
                }
                look
            }
        }
    }

    /// The strength a box of `kind` draws `look` at: app.rs's, the
    /// ceiling's facets capped while the limiter is on (`uncapped`: not,
    /// to find the cap).
    fn drawn(&self, kind: u32, look: &themes::Look) -> f32 {
        look.behavior
            .drawn_strength(kind, look.strength, self.limiter.on() && !self.uncapped)
    }

    fn draws(&self, part: Part) -> bool {
        self.part == Part::All || self.part == part
    }

    /// The frame at `t`, `dt` after the last, on `hop`.
    fn step(&mut self, t: f32, dt: f32, hop: &HopAt) -> Draws {
        use crate::flash::{Triggers, follower};
        // The limiter first, on the analysis's output, as in app.rs: the
        // triggers it lets through, the features and the mel bands.
        let gate = self.limiter.begin(
            dt,
            Triggers {
                beat: hop.beat,
                downbeat: hop.downbeat,
                drop: hop.drop,
            },
        );
        let mut f = hop.features;
        self.limiter.features(f.as_slice_mut(), dt, gate);
        let mut f_shapes = f;
        self.limiter.glided(f_shapes.as_slice_mut());
        let mut mel = hop.mel;
        self.limiter.mel(&mut mel, dt);
        let head = self.scene.eye;
        // The envelopes, the clock, the rings and the bars, in app.rs's
        // order; what the looks read of them is what the limiter follows.
        if gate.beat {
            self.beat_env = 1.0;
        }
        self.beat_env *= (-dt * 6.0).exp();
        self.downbeat_env = surface_fx::downbeat_step(self.downbeat_env, gate.downbeat, dt);
        let beat_env = self.limiter.follow(follower::BEAT_ENV, self.beat_env);
        let downbeat_env = self
            .limiter
            .follow(follower::DOWNBEAT_ENV, self.downbeat_env);
        // `FOSFORA_FLASH_MOTION=<0..1>`: the looks' clocks at that share
        // of their speed (0 holds them), to tell their own motion from the
        // music's light.
        let motion: f32 = std::env::var("FOSFORA_FLASH_MOTION")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);
        let motion = motion * self.limiter.motion();
        self.clock.advance(dt * motion, beat_env);
        // The contours' land (board #5533), as in app.rs.
        let high = f.presence.max(f.brilliance);
        self.terrain
            .step(dt, motion, [f.rms, f.bass, f.mid, high], gate.downbeat);
        self.ripple
            .update(t, dt, head, gate.beat, f.bass.max(f.sub_bass), f.bass);
        self.canvas.update(dt, &mel);
        let mut heights = self.canvas.heights().to_vec();
        self.limiter.follow_all(follower::BARS, &mut heights);
        let glow = self.limiter.follow(follower::BAR_GLOW, self.canvas.glow());
        let looks: Vec<_> = self.scene.boxes.iter().map(|b| self.look(b.kind)).collect();
        // The wall spectrum: the wall ahead among those on the spectrum.
        let walls: Vec<(usize, WallFace)> = self
            .scene
            .boxes
            .iter()
            .zip(&looks)
            .enumerate()
            .filter(|(_, (b, l))| b.kind == KIND_WALL && l.behavior == SurfaceBehavior::Spectrum)
            .map(|(i, (b, _))| (i, WallFace::of(b.center, b.rot, b.half, head)))
            .collect();
        let faces: Vec<WallFace> = walls.iter().map(|w| w.1).collect();
        let spectrum_box = self
            .pick
            .update(head, self.scene.forward, &faces, dt)
            .map(|k| walls[k].0);
        let mut audio = Audio {
            rms: f.rms,
            bass: f.bass,
            beat: beat_env,
            clock: self.clock.seconds(),
            mid: f.mid,
            high: f.presence.max(f.brilliance),
            downbeat: downbeat_env,
            bar_phase: f.bar_phase,
            key_tint: [1.0; 3],
            land: self.terrain.land(),
        };
        if looks.iter().any(|l| l.color == COLOR_KEY) {
            let hue = fosfora_app::bindings::sources::collect_audio(&f)
                .get("audio.key_hue")
                .map_or(0.5, |v| v.0);
            audio.key_tint = key_tint(hue);
            self.limiter.follow_all(follower::TINT, &mut audio.key_tint);
        }
        for e in &mut self.effects {
            e.advance(dt * motion);
        }
        let mut slots = Vec::new();
        let mut ported = Vec::new();
        let mut sign = 1.0;
        for (k, (b, look)) in self.scene.boxes.iter().zip(&looks).enumerate() {
            let behavior = if spectrum_box == Some(k) {
                SurfaceBehavior::Spectrum
            } else {
                look.behavior
            };
            let spectrum = behavior == SurfaceBehavior::Spectrum;
            if spectrum != (spectrum_box == Some(k)) || (!spectrum && !behavior.draws_on_face()) {
                continue;
            }
            let mut face = acting_face(b.center, b.rot, b.half, head);
            if spectrum {
                (face, sign) = crate::canvas::spectrum_face(face);
            } else if b.kind == KIND_FLOOR {
                face = surface_fx::level_floor(face, None);
            }
            if let Some(i) = behavior.port() {
                ported.push((
                    i,
                    surface_port::port_face(face),
                    b.kind,
                    behavior,
                    self.drawn(b.kind, look),
                    look.color,
                    Map::of(true, behavior, b.kind),
                    look.mono,
                ));
                continue;
            }
            slots.push(Slot {
                face,
                kind: b.kind,
                behavior,
                strength: self.drawn(b.kind, look),
                color: look.color,
                band: look.band,
            });
        }
        let sky = &self.scene.sky;
        let sky_faces: Vec<Face> = ported
            .iter()
            .filter(|p| p.6 == Map::Sky)
            .map(|p| p.1)
            .collect();
        let mut joined = sky::join(&sky_faces).into_iter();
        let mut draws = Draws::default();
        for &(i, face, kind, behavior, strength, color, map, mono) in &ported {
            let effect = &self.effects[i];
            let (face, edges) = match map {
                Map::Sky => joined.next().unwrap_or((face, sky::Edges::OPEN)),
                _ => (face, sky::Edges::OPEN),
            };
            let part = match map {
                Map::Sky => Part::Sky,
                Map::Reflection => Part::Reflection,
                Map::Face => Part::Behavior(behavior),
            };
            if !self.draws(part) {
                continue;
            }
            let (resolution, params) = match map {
                Map::Face => (surface_port::resolution(face.half), effect.params_in(mono)),
                Map::Sky => (sky::RESOLUTION, effect.params()),
                Map::Reflection => (sky::RESOLUTION, effect.reflection_params()),
            };
            // As `app.rs`: a floor's reflection at the theme's share.
            let strength = match map {
                Map::Reflection => strength * self.theme.reflection(),
                _ => strength,
            };
            let rows = surface_port::rows(
                &face,
                strength,
                effect.overlay,
                behavior.id(),
                surface_port::tint(color, kind, audio.key_tint),
            );
            let features = if surface_port::PORTS[i].shapes {
                &f_shapes
            } else {
                &f
            };
            let u =
                surface_port::uniforms(features, audio.clock, dt, self.frame, resolution, params);
            draws.ports.push((
                i,
                bytemuck::bytes_of(&u).to_vec(),
                crate::immerse::lift_rows(sky::rows(rows, map, sky, edges), LEVEL),
            ));
        }
        let spectrum = Spectrum {
            heights: &heights,
            glow,
            gain: self.canvas.gain,
            sign,
        };
        draws.faces = slots
            .iter()
            .filter(|s| self.draws(Part::Behavior(s.behavior)))
            .map(|s| surface_fx::rows(s, audio, Some(&self.ripple), Some(spectrum), t))
            .map(|rows| crate::immerse::lift_rows(rows, LEVEL))
            .collect();
        // The edge bars, each kind in its theme's mode.
        // The edge bars' heights: the limit's, or the analysis's own
        // (board #5532), from the mel bands as they came.
        let limited = heights;
        let bars = self
            .edge_heights
            .step(dt, self.bars_free, &hop.mel, &limited);
        let heights = bars.now.to_vec();
        let presence = self.presence.step(dt, true, f.loudness_m);
        self.history.step(dt, bars.past);
        self.outline.clear();
        if presence > 0.0 {
            let shapes: Vec<Shape> = self
                .scene
                .boxes
                .iter()
                .zip(&looks)
                .map(|(b, look)| Shape {
                    center: b.center,
                    rot: b.rot,
                    half: b.half,
                    look: Some(edge_bars::Look {
                        color: surface_color(look.color, b.kind, audio.key_tint),
                        fade: 1.0,
                        mode: self.edges.mode(look.edges),
                    }),
                })
                .collect();
            edge_bars::outline(
                &shapes,
                head,
                edge_bars::HEIGHT_FRACTION,
                edge_bars::DEPTH_FRACTION,
                heights.len(),
                edge_bars::peak_alpha(LEVEL, 1.0) * presence,
                &mut self.outline,
            );
        }
        if !self.draws(Part::Strips) {
            self.outline.strips.clear();
        }
        if !self.draws(Part::Tunnels) {
            self.outline.tunnels.clear();
        }
        if !self.outline.strips.is_empty() || !self.outline.tunnels.is_empty() {
            edge_bars::rows(&heights, &self.history, &self.outline, &mut draws.edge_rows);
            draws.strips = self.outline.strips.len();
            draws.tunnels = self.outline.tunnels.len();
        }
        self.frame += 1;
        draws
    }

    /// The parts this theme draws, each alone.
    fn parts(theme: Theme) -> Vec<Part> {
        let mut parts = Vec::new();
        let mut add = |p: Part| {
            if !parts.contains(&p) {
                parts.push(p);
            }
        };
        for b in scene().boxes {
            let look = theme.look(b.kind).expect("a named kind");
            match Map::of(true, look.behavior, b.kind) {
                Map::Sky => add(Part::Sky),
                Map::Reflection => add(Part::Reflection),
                Map::Face if look.behavior.draws_on_face() => add(Part::Behavior(look.behavior)),
                Map::Face => {}
            }
            if look.behavior == SurfaceBehavior::Spectrum && b.kind == KIND_WALL {
                add(Part::Behavior(SurfaceBehavior::Spectrum));
            }
            match themes::edge_mode(Some(theme), b.kind) {
                edge_bars::Mode::Off => {}
                edge_bars::Mode::In => add(Part::Tunnels),
                _ => add(Part::Strips),
            }
        }
        parts
    }
}

/// The ports' effects, read from the core's assets as the app reads them.
fn effects() -> Vec<Effect> {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let _ = fosfora_app::effect::loader::set_assets_dir(assets);
    surface_port::load(&EffectLoader::new()).expect("the ports load")
}

/// Averages each region of the six faces' light, one frame per slot of the
/// output.
const REDUCE_WGSL: &str = r"
@group(0) @binding(0) var cube: texture_2d_array<f32>;
@group(0) @binding(1) var<storage, read_write> out: array<vec4<f32>>;
@group(0) @binding(2) var<uniform> slot: vec4<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    // The pixel edges at every step of the angle across a face.
    var edges = EDGES;
    let cells = CELLSu;
    let per = cells * cells;
    let r = id.x;
    if r >= 6u * per {
        return;
    }
    let face = r / per;
    let c = r % per;
    let cx = c % cells;
    let cy = c / cells;
    let x0 = edges[cx];
    let x1 = edges[cx + SPANu];
    let y0 = edges[cy];
    let y1 = edges[cy + SPANu];
    var sum = vec3<f32>(0.0);
    for (var y = y0; y < y1; y++) {
        for (var x = x0; x < x1; x++) {
            let p = textureLoad(cube, vec2<i32>(i32(x), i32(y)), i32(face), 0);
            sum += min(max(p.rgb, vec3<f32>(0.0)), vec3<f32>(1.0));
        }
    }
    out[slot.x * 6u * per + r] = vec4<f32>(sum / f32((x1 - x0) * (y1 - y0)), 0.0);
}
";

/// The GPU side: the cube's six layers, the pipelines the looks draw with,
/// their uniforms, and the regions' averages.
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    cube: wgpu::Texture,
    layers: Vec<wgpu::TextureView>,
    fx: wgpu::RenderPipeline,
    fx_eyes: Vec<wgpu::BindGroup>,
    fx_slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    /// Each port's pipeline, in `surface_port::PORTS`' order, on the
    /// ports' shared layouts.
    ports: Vec<wgpu::RenderPipeline>,
    port_slots: Vec<(
        wgpu::Buffer,
        wgpu::Buffer,
        wgpu::BindGroup,
        Vec<wgpu::BindGroup>,
    )>,
    strips: wgpu::RenderPipeline,
    tunnels: wgpu::RenderPipeline,
    edge: wgpu::Buffer,
    /// Per pipeline (the strips, the tunnels): the eyes' groups and the
    /// rows' group.
    edge_groups: [(Vec<wgpu::BindGroup>, wgpu::BindGroup); 2],
    reduce: wgpu::ComputePipeline,
    reduce_group: wgpu::BindGroup,
    slot: wgpu::Buffer,
    out: wgpu::Buffer,
    back: wgpu::Buffer,
}

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const MAX_FX: usize = 16;
const MAX_PORTS: usize = 12;

impl Gpu {
    fn new(effects: &[Effect]) -> Self {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::METAL,
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .expect("no wgpu adapter");
        eprintln!("adapter: {:?}", adapter.get_info().name);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("xr-flash"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .expect("no wgpu device");
        let cube = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cube"),
            size: wgpu::Extent3d {
                width: FACE_PX,
                height: FACE_PX,
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let layers = (0..6)
            .map(|i| {
                cube.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let buffer = |label: &str, size: usize, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage,
                mapped_at_creation: false,
            })
        };
        let uniform = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;
        // The six eyes: the camera, then the target's size (80 bytes, the
        // ports' eye block; the others read the camera alone).
        let scene = scene();
        let fov = Fov {
            left: -std::f32::consts::FRAC_PI_4,
            right: std::f32::consts::FRAC_PI_4,
            up: std::f32::consts::FRAC_PI_4,
            down: -std::f32::consts::FRAC_PI_4,
        };
        let eyes: Vec<wgpu::Buffer> = FACES
            .iter()
            .map(|&(_, fwd, up)| {
                let right = fwd.cross(up).normalize();
                let look = Quat::from_mat3(&Mat3::from_cols(right, right.cross(fwd), -fwd));
                let vp =
                    math::view_projection(look.to_array(), scene.eye.to_array(), fov, 0.05, 100.0);
                let mut block = [0.0f32; surface_port::EYE_BYTES / 4];
                block[..16].copy_from_slice(&vp.to_cols_array());
                block[16..18].copy_from_slice(&[FACE_PX as f32, FACE_PX as f32]);
                let b = buffer("eye", surface_port::EYE_BYTES, uniform);
                queue.write_buffer(&b, 0, bytemuck::cast_slice(&block));
                b
            })
            .collect();
        let pipeline = |label: &str, source: &str, entries: (&str, &str)| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(entries.0),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entries.1),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            })
        };
        let group = |layout: &wgpu::BindGroupLayout, buffers: &[&wgpu::Buffer]| {
            let entries: Vec<_> = buffers
                .iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: b.as_entire_binding(),
                })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout,
                entries: &entries,
            })
        };
        let fx = pipeline("fx", surface_fx::SURFACE_FX_WGSL, ("vs_main", "fs_main"));
        let fx_eyes = eyes
            .iter()
            .map(|e| group(&fx.get_bind_group_layout(0), &[e]))
            .collect();
        let fx_slots = (0..MAX_FX)
            .map(|_| {
                let b = buffer("fx", 16 * surface_fx::UNIFORM_ROWS, uniform);
                let g = group(&fx.get_bind_group_layout(1), &[&b]);
                (b, g)
            })
            .collect();
        let port_layouts = surface_port::layouts(&device);
        let ports = effects
            .iter()
            .zip(surface_port::PORTS)
            .map(|(e, p)| {
                surface_port::pipeline(
                    &device,
                    &port_layouts,
                    e.name,
                    &e.source,
                    p.entries(),
                    FORMAT,
                    None,
                )
            })
            .collect();
        let u_bytes = size_of::<fosfora_app::gpu::uniforms::ShaderUniforms>();
        let port_slots = (0..MAX_PORTS)
            .map(|_| {
                let u = buffer("port-u", u_bytes, uniform);
                let rows = buffer("port-rows", 16 * surface_port::UNIFORM_ROWS, uniform);
                let g0 = group(&port_layouts[0], &[&u]);
                let g1 = eyes
                    .iter()
                    .map(|e| group(&port_layouts[1], &[e, &rows]))
                    .collect();
                (u, rows, g0, g1)
            })
            .collect();
        let strips = pipeline("strips", edge_bars::EDGE_BARS_WGSL, ("vs_main", "fs_main"));
        let tunnels = pipeline(
            "tunnels",
            edge_bars::EDGE_BARS_WGSL,
            ("vs_tunnel", "fs_tunnel"),
        );
        let edge = buffer("edge", edge_bars::UNIFORM_BYTES, uniform);
        let edge_groups = [&strips, &tunnels].map(|p| {
            let eye_groups = eyes
                .iter()
                .map(|e| group(&p.get_bind_group_layout(0), &[e]))
                .collect();
            (eye_groups, group(&p.get_bind_group_layout(1), &[&edge]))
        });
        let source = REDUCE_WGSL
            .replace("CELLSu", &format!("{CELLS}u"))
            .replace("SPANu", &format!("{}u", CELL_DEG / STEP_DEG))
            .replace("EDGES", &{
                let n = 90 / STEP_DEG + 1;
                let px: Vec<String> = (0..n)
                    .map(|k| format!("{}u", edge_px((k * STEP_DEG) as f32 - 45.0)))
                    .collect();
                format!("array<u32, {n}>({})", px.join(", "))
            });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("reduce"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let reduce = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("reduce"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });
        let out_bytes = BATCH * REGIONS * 16;
        let out = buffer(
            "regions",
            out_bytes,
            wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        );
        let back = buffer(
            "regions-back",
            out_bytes,
            wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        );
        let slot = buffer("slot", 16, uniform);
        let cube_view = cube.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let reduce_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("reduce"),
            layout: &reduce.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&cube_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: out.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: slot.as_entire_binding(),
                },
            ],
        });
        Self {
            device,
            queue,
            cube,
            layers,
            fx,
            fx_eyes,
            fx_slots,
            ports,
            port_slots,
            strips,
            tunnels,
            edge,
            edge_groups,
            reduce,
            reduce_group,
            slot,
            out,
            back,
        }
    }

    /// Draw `draws` on the six faces and average the regions into batch
    /// slot `k`.
    fn draw(&self, draws: &Draws, k: usize) {
        assert!(draws.faces.len() <= MAX_FX && draws.ports.len() <= MAX_PORTS);
        for (rows, (b, _)) in draws.faces.iter().zip(&self.fx_slots) {
            self.queue.write_buffer(b, 0, bytemuck::cast_slice(rows));
        }
        for ((_, u, rows), (ub, rb, ..)) in draws.ports.iter().zip(&self.port_slots) {
            self.queue.write_buffer(ub, 0, u);
            self.queue.write_buffer(rb, 0, bytemuck::cast_slice(rows));
        }
        if !draws.edge_rows.is_empty() {
            self.queue
                .write_buffer(&self.edge, 0, bytemuck::cast_slice(&draws.edge_rows));
        }
        self.queue
            .write_buffer(&self.slot, 0, bytemuck::cast_slice(&[k as u32, 0, 0, 0]));
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        for (i, view) in self.layers.iter().enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cube"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.fx);
            pass.set_bind_group(0, &self.fx_eyes[i], &[]);
            for (_, g) in self.fx_slots.iter().take(draws.faces.len()) {
                pass.set_bind_group(1, g, &[]);
                pass.draw(0..6, 0..1);
            }
            for ((effect, ..), (.., g0, g1)) in draws.ports.iter().zip(&self.port_slots) {
                pass.set_pipeline(&self.ports[*effect]);
                pass.set_bind_group(0, g0, &[]);
                pass.set_bind_group(1, &g1[i], &[]);
                pass.draw(0..6, 0..1);
            }
            let edges = [(&self.strips, draws.strips), (&self.tunnels, draws.tunnels)];
            for ((pipeline, n), (eyes, rows)) in edges.into_iter().zip(&self.edge_groups) {
                if n > 0 {
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, &eyes[i], &[]);
                    pass.set_bind_group(1, rows, &[]);
                    pass.draw(0..6, 0..n as u32);
                }
            }
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&self.reduce);
            pass.set_bind_group(0, &self.reduce_group, &[]);
            pass.dispatch_workgroups(REGIONS.div_ceil(64) as u32, 1, 1);
        }
        self.queue.submit([encoder.finish()]);
    }

    /// The six faces as drawn last, side by side, as a PPM (light over
    /// black, sRGB), to look at.
    fn shot(&self, path: &std::path::Path) {
        let row = (FACE_PX * 8).next_multiple_of(256);
        let back = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shot"),
            size: u64::from(row * FACE_PX * 6),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.cube,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &back,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(FACE_PX),
                },
            },
            wgpu::Extent3d {
                width: FACE_PX,
                height: FACE_PX,
                depth_or_array_layers: 6,
            },
        );
        self.queue.submit([encoder.finish()]);
        back.slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("poll");
        let bytes = back.slice(..).get_mapped_range().to_vec();
        let half = |h: u16| {
            let (e, m) = (i32::from((h >> 10) & 0x1f), f32::from(h & 0x3ff));
            match e {
                0 => m * 2f32.powi(-24),
                31 => 1.0,
                _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
            }
        };
        let (w, h) = (FACE_PX as usize * 6, FACE_PX as usize);
        let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
        for y in 0..h {
            for face in 0..6 {
                for x in 0..FACE_PX as usize {
                    let o = (face * h + y) * row as usize + x * 8;
                    for c in 0..3 {
                        let v = half(u16::from_le_bytes([bytes[o + 2 * c], bytes[o + 2 * c + 1]]));
                        out.push((v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8);
                    }
                }
            }
        }
        std::fs::write(path, out).expect("writing the shot");
    }

    /// The first `n` slots' averages: per frame, per region, linear rgb.
    fn read(&self, n: usize) -> Vec<[f32; 4]> {
        let bytes = (n * REGIONS * 16) as u64;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_buffer_to_buffer(&self.out, 0, &self.back, 0, bytes);
        self.queue.submit([encoder.finish()]);
        let slice = self.back.slice(..bytes);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("poll");
        let out = bytemuck::pod_collect_to_vec::<u8, [f32; 4]>(&slice.get_mapped_range());
        self.back.unmap();
        out
    }
}

/// What one run found: the worst region's transitions and where.
#[derive(Debug, Clone, Copy, Default)]
pub struct Found {
    pub worst: Worst,
    pub general_region: usize,
    pub red_region: usize,
    /// Regions that ever failed.
    pub failing: usize,
}

/// `part` of `theme` (the limiter on or off) over `source`, every region
/// counted.
fn run(gpu: &Gpu, theme: Theme, part: Part, limit: bool, source: &mut dyn Source) -> Found {
    run_with(gpu, theme, None, part, limit, source)
}

/// [`run`], or with `every` surface on one behavior instead of the
/// theme's looks.
fn run_with(
    gpu: &Gpu,
    theme: Theme,
    every: Option<SurfaceBehavior>,
    part: Part,
    limit: bool,
    source: &mut dyn Source,
) -> Found {
    let show = Show::new(theme, every, part, limit, effects());
    run_show(gpu, show, source)
}

/// `part` of `theme` behind the limiter with the edge bars as `edges` has
/// them (the theme's modes, or one forced on every kind) and their heights
/// following the analysis when `bars_free` (board #5532).
fn run_edges(
    gpu: &Gpu,
    theme: Theme,
    part: Part,
    edges: Choice,
    bars_free: bool,
    source: &mut dyn Source,
) -> Found {
    let mut show = Show::new(theme, None, part, true, effects());
    (show.edges, show.bars_free) = (edges, bars_free);
    run_show(gpu, show, source)
}

/// `show` over `source`, every region counted.
fn run_show(gpu: &Gpu, mut show: Show, source: &mut dyn Source) -> Found {
    let (theme, part, limit) = (show.theme, show.part, show.limiter.on());
    let mut counters = vec![Counter::new(); REGIONS];
    let dt = 1.0 / FPS;
    let frames = (source.duration() * FPS).floor() as usize;
    let mut pending = Vec::with_capacity(BATCH);
    let flush = |pending: &mut Vec<f64>, counters: &mut [Counter]| {
        if pending.is_empty() {
            return;
        }
        let means = gpu.read(pending.len());
        for (k, t) in pending.iter().enumerate() {
            for (r, c) in counters.iter_mut().enumerate() {
                let m = means[k * REGIONS + r];
                c.push(*t, [m[0], m[1], m[2]]);
            }
        }
        pending.clear();
    };
    let shots = std::env::var_os("FOSFORA_FLASH_SHOTS");
    // `FOSFORA_FLASH_TRACE=<region>`: that region's light per frame, with
    // the limiter's direction, printed after the run.
    let trace: Option<usize> = std::env::var("FOSFORA_FLASH_TRACE")
        .ok()
        .and_then(|v| v.parse().ok());
    let mut phases = Vec::new();
    let mut traced = Vec::new();
    for n in 0..frames {
        let t = n as f64 / FPS;
        let hop = source.at(t, dt);
        let draws = show.step(t as f32, dt as f32, &hop);
        gpu.draw(&draws, pending.len());
        if let Some(dir) = &shots
            && [36, 72, 75, 144].contains(&n)
        {
            let on = if limit { "on" } else { "off" };
            let name = format!("{}_{}_{on}_{n}.ppm", theme.name(), part.name());
            gpu.shot(&std::path::Path::new(dir).join(name.replace(' ', "-")));
        }
        pending.push(t);
        phases.push(show.limiter.phase());
        if pending.len() == BATCH {
            if let Some(r) = trace {
                let means = gpu.read(pending.len());
                traced.extend((0..pending.len()).map(|k| means[k * REGIONS + r]));
            }
            flush(&mut pending, &mut counters);
        }
    }
    if let Some(r) = trace {
        let means = gpu.read(pending.len());
        traced.extend((0..pending.len()).map(|k| means[k * REGIONS + r]));
        eprintln!("trace of region {r} ({}):", region_name(r));
        let span = |name: &str, default: f64| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        };
        let (from, to) = (
            span("FOSFORA_FLASH_TRACE_FROM", 0.0),
            span("FOSFORA_FLASH_TRACE_TO", f64::MAX),
        );
        for (n, (m, p)) in traced.iter().zip(&phases).enumerate() {
            if !(from..=to).contains(&(n as f64 / FPS)) {
                continue;
            }
            eprintln!(
                "{:7.3} {:.4} {:?}",
                n as f64 / FPS,
                crate::flash::luminance([m[0], m[1], m[2]]),
                p
            );
        }
    }
    flush(&mut pending, &mut counters);
    let mut found = Found::default();
    for (r, c) in counters.iter().enumerate() {
        let w = c.worst();
        if w.general > found.worst.general {
            found.general_region = r;
        }
        if w.red > found.worst.red {
            found.red_region = r;
        }
        found.worst = found.worst.max(w);
        found.failing += usize::from(!w.passes());
    }
    found
}

/// One run's numbers for the table: the worst flashes in any second,
/// general and red, where and when, and how many regions failed.
fn cell(f: &Found) -> String {
    format!(
        "{:>4.1} / {:>4.1} ({:>4} failing; general {:>5.1} s {}, red {:>5.1} s {})",
        f.worst.general_flashes(),
        f.worst.red_flashes(),
        f.failing,
        f.worst.general_at,
        region_name(f.general_region),
        f.worst.red_at,
        region_name(f.red_region),
    )
}

/// `theme`'s `part` over the source `make` makes, the limiter off then on:
/// both lines printed, the run with the limiter on returned.
fn before_after(
    gpu: &Gpu,
    what: &str,
    theme: Theme,
    part: Part,
    make: &mut dyn FnMut() -> Box<dyn Source>,
) -> (Found, Found) {
    let off = run(gpu, theme, part, false, make().as_mut());
    let on = run(gpu, theme, part, true, make().as_mut());
    eprintln!(
        "| {what} | {} | {} | {:.1} / {:.1} ({}) | {:.1} / {:.1} ({}) | {} |",
        theme.name(),
        part.name(),
        off.worst.general_flashes(),
        off.worst.red_flashes(),
        off.failing,
        on.worst.general_flashes(),
        on.worst.red_flashes(),
        on.failing,
        cell(&on)
    );
    (off, on)
}

#[test]
fn the_regions_cover_each_face_with_overlapping_cells() {
    assert_eq!(CELLS, 17);
    assert_eq!(REGIONS, 6 * 17 * 17);
    // The cuts run from edge to edge, a cell 10 degrees (14 px) at the
    // face's center and wider in pixels toward its edges (24 px).
    assert_eq!(edge_px(-45.0), 0);
    assert_eq!(edge_px(45.0), FACE_PX);
    assert_eq!(edge_px(5.0) - edge_px(-5.0), 14);
    assert_eq!(edge_px(45.0) - edge_px(35.0), 24);
    assert_eq!(region_name(0), "ahead -40° +40°");
    assert!(region_name(REGIONS - 1).starts_with("down"));
}

/// The pulse trains at 4, 8, 12 and 16 Hz, full depth, on every theme,
/// the limiter off and on: with it on no region of any theme passes three
/// general or three red flashes in any second.
#[test]
#[ignore = "needs a GPU adapter"]
fn every_theme_stays_within_three_flashes_on_the_pulse_trains() {
    let gpu = Gpu::new(&effects());
    let mut failed = Vec::new();
    for hz in [4.0, 8.0, 12.0, 16.0] {
        for theme in Theme::ALL {
            let (_, on) = before_after(
                &gpu,
                &format!("pulse {hz} Hz"),
                theme,
                Part::All,
                &mut || Box::new(Pulse::new(hz, 6.0)),
            );
            if !on.worst.passes() {
                failed.push(format!("{hz} Hz {}", theme.name()));
            }
        }
    }
    assert!(failed.is_empty(), "over three flashes: {failed:?}");
}

/// Each part of each theme alone on the pulse trains, the limiter off and
/// on: which parts flash, and that each stays within the rule.
#[test]
#[ignore = "needs a GPU adapter"]
fn every_part_alone_stays_within_three_flashes_on_the_pulse_trains() {
    let gpu = Gpu::new(&effects());
    let mut failed = Vec::new();
    for theme in Theme::ALL {
        for part in Show::parts(theme) {
            for hz in [4.0, 8.0, 12.0, 16.0] {
                let (_, on) =
                    before_after(&gpu, &format!("pulse {hz} Hz"), theme, part, &mut || {
                        Box::new(Pulse::new(hz, 6.0))
                    });
                if !on.worst.passes() {
                    failed.push(format!("{hz} Hz {} {}", theme.name(), part.name()));
                }
            }
        }
    }
    assert!(failed.is_empty(), "over three flashes: {failed:?}");
}

/// Every behavior that lights a face, each on every surface that offers it
/// (a hand edit's worst: all eight ports, the rings, the flows, the pulse
/// and the wall spectrum), on the pulse trains, the limiter off and on:
/// with it on, none passes three flashes.
#[test]
#[ignore = "needs a GPU adapter"]
fn every_behavior_on_every_surface_stays_within_three_flashes() {
    let gpu = Gpu::new(&effects());
    let mut failed = Vec::new();
    let behaviors = SurfaceBehavior::ALL
        .into_iter()
        .filter(|b| b.draws_on_face() || *b == SurfaceBehavior::Spectrum);
    for behavior in behaviors {
        for hz in [4.0, 8.0, 12.0, 16.0] {
            let found = [false, true].map(|limit| {
                let mut source = Pulse::new(hz, 6.0);
                run_with(
                    &gpu,
                    Theme::Classic,
                    Some(behavior),
                    Part::All,
                    limit,
                    &mut source,
                )
            });
            let [off, on] = found;
            eprintln!(
                "| pulse {hz} Hz | every surface | {} | {:.1} / {:.1} ({}) | {:.1} / {:.1} ({}) | {} |",
                behavior.name(),
                off.worst.general_flashes(),
                off.worst.red_flashes(),
                off.failing,
                on.worst.general_flashes(),
                on.worst.red_flashes(),
                on.failing,
                cell(&on)
            );
            if !on.worst.passes() {
                failed.push(format!("{hz} Hz {}", behavior.name()));
            }
        }
    }
    assert!(failed.is_empty(), "over three flashes: {failed:?}");
}

/// The music standing still at two levels: the looks' own motion alone
/// (curtains drifting, streaks running, the bars' edges) never flashes,
/// the limiter off or on.
#[test]
#[ignore = "needs a GPU adapter"]
fn the_looks_own_motion_never_flashes() {
    let gpu = Gpu::new(&effects());
    for level in [0.3, 0.8] {
        for theme in Theme::ALL {
            let (off, on) = before_after(
                &gpu,
                &format!("still {level}"),
                theme,
                Part::All,
                &mut || Box::new(Still { level, secs: 20.0 }),
            );
            assert!(off.worst.passes() && on.worst.passes(), "{}", theme.name());
        }
    }
}

/// The songs dumped into `FOSFORA_FLASH_DUMPS` (`*.bin`), every theme (or
/// those `FOSFORA_FLASH_THEMES` names, comma separated), the limiter off
/// and on; with `FOSFORA_FLASH_PARTS` set, each part alone too; with
/// `FOSFORA_FLASH_EVERY` set, each behavior on every surface instead of
/// the themes. Nothing runs without the directory; no song is in the repo.
#[test]
#[ignore = "needs a GPU adapter and song dumps"]
fn the_dumped_songs_stay_within_three_flashes() {
    let Some(dir) = std::env::var_os("FOSFORA_FLASH_DUMPS") else {
        eprintln!("FOSFORA_FLASH_DUMPS unset: no songs");
        return;
    };
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("the dumps' directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .collect();
    paths.sort();
    let only: Option<Vec<String>> = std::env::var("FOSFORA_FLASH_THEMES")
        .ok()
        .map(|v| v.split(',').map(str::to_owned).collect());
    let by_part = std::env::var_os("FOSFORA_FLASH_PARTS").is_some();
    let every = std::env::var_os("FOSFORA_FLASH_EVERY").is_some();
    let gpu = Gpu::new(&effects());
    let mut failed = Vec::new();
    for path in &paths {
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        let name = name.unwrap_or_default();
        if every {
            let behaviors = SurfaceBehavior::ALL
                .into_iter()
                .filter(|b| b.draws_on_face() || *b == SurfaceBehavior::Spectrum);
            for behavior in behaviors {
                let [off, on] = [false, true].map(|limit| {
                    let mut dump = Dump::read(path).expect("a dump");
                    run_with(
                        &gpu,
                        Theme::Classic,
                        Some(behavior),
                        Part::All,
                        limit,
                        &mut dump,
                    )
                });
                eprintln!(
                    "| {name} | every surface | {} | {:.1} / {:.1} ({}) | {:.1} / {:.1} ({}) | {} |",
                    behavior.name(),
                    off.worst.general_flashes(),
                    off.worst.red_flashes(),
                    off.failing,
                    on.worst.general_flashes(),
                    on.worst.red_flashes(),
                    on.failing,
                    cell(&on)
                );
                if !on.worst.passes() {
                    failed.push(format!("{name} every {}", behavior.name()));
                }
            }
            continue;
        }
        for theme in Theme::ALL {
            if only
                .as_ref()
                .is_some_and(|o| !o.iter().any(|n| n == theme.name()))
            {
                continue;
            }
            let mut parts = vec![Part::All];
            if by_part {
                parts.extend(Show::parts(theme));
            }
            for part in parts {
                let (_, on) = before_after(&gpu, &name, theme, part, &mut || {
                    Box::new(Dump::read(path).expect("a dump"))
                });
                if !on.worst.passes() {
                    failed.push(format!("{name} {} {}", theme.name(), part.name()));
                }
            }
        }
    }
    assert!(failed.is_empty(), "over three flashes: {failed:?}");
}

/// One run picked by `FOSFORA_FLASH_ONE=<theme>:<part>:<source>:<0|1>`
/// (the theme, or `every-<behavior>` for one behavior on every surface;
/// the part's name as the tables print it, `all` for every part; the
/// source a pulse train's rate or a dump's path), for looking closer with
/// `FOSFORA_FLASH_TRACE` and `FOSFORA_FLASH_SHOTS`.
#[test]
#[ignore = "needs a GPU adapter"]
fn one_pulse_train_run() {
    let Ok(one) = std::env::var("FOSFORA_FLASH_ONE") else {
        return;
    };
    let v: Vec<&str> = one.split(':').collect();
    // `every-<behavior>` for one behavior on every surface.
    let every = v[0]
        .strip_prefix("every-")
        .map(|b| SurfaceBehavior::from_name(b).expect("a behavior"));
    let theme = Theme::from_name(v[0]).unwrap_or(Theme::Classic);
    let part = std::iter::once(Part::All)
        .chain(Show::parts(theme))
        .find(|p| p.name() == v[1])
        .expect("a part of the theme");
    let mut source: Box<dyn Source> = match v[2].parse::<f64>() {
        Ok(hz) => Box::new(Pulse::new(hz, 6.0)),
        Err(_) => Box::new(Dump::read(std::path::Path::new(v[2])).expect("a dump")),
    };
    let gpu = Gpu::new(&effects());
    let found = run_with(&gpu, theme, every, part, v[3] == "1", source.as_mut());
    eprintln!("{one}: {}", cell(&found));
}

/// A theme changing as a bundled track brings one (board #5533,
/// `themes::Bringing`): `from` plays for 3 s, then the room's light goes
/// through dark to `to`, as `app.rs` does it, the limiter on and the fade
/// keeping to its direction. The veil
/// scales each region's mean light, which is what scaling every lit
/// face's alpha comes to over a black room, but for pixels past full
/// white.
fn run_change(gpu: &Gpu, from: Theme, to: Theme, source: &mut dyn Source) -> Found {
    use crate::themes::{Bringing, Brought, Turn};
    let mut show = Show::new(from, None, Part::All, true, effects());
    let mut bringing = Bringing::default();
    let mut counters = vec![Counter::new(); REGIONS];
    let dt = 1.0 / FPS;
    let frames = (source.duration() * FPS).floor() as usize;
    for n in 0..frames {
        let t = n as f64 / FPS;
        if n == 3 * FPS as usize {
            bringing.start(Brought::Theme(to), false, 0);
        }
        let hop = source.at(t, dt);
        let draws = show.step(t as f32, dt as f32, &hop);
        gpu.draw(&draws, 0);
        // After the limiter's frame began, as in app.rs: the theme laid
        // now is in the next frame's draws.
        if bringing.step(dt as f32, Turn::of(&show.limiter)).is_some() {
            show.theme = to;
        }
        let veil = bringing.veil();
        for (c, m) in counters.iter_mut().zip(gpu.read(1)) {
            c.push(t, [m[0] * veil, m[1] * veil, m[2] * veil]);
        }
    }
    let mut found = Found::default();
    for (r, c) in counters.iter().enumerate() {
        let w = c.worst();
        if w.general > found.worst.general {
            found.general_region = r;
        }
        if w.red > found.worst.red {
            found.red_region = r;
        }
        found.worst = found.worst.max(w);
        found.failing += usize::from(!w.passes());
    }
    found
}

/// Each bundled track's theme giving way to the next one's through dark,
/// on the pulse trains with the limiter on: the change adds no flash past
/// the rule.
#[test]
#[ignore = "needs a GPU adapter"]
fn a_tracks_theme_coming_stays_within_three_flashes() {
    let gpu = Gpu::new(&effects());
    let themes: Vec<Theme> = crate::music::BUNDLED.iter().map(|b| b.theme).collect();
    let mut failed = Vec::new();
    for (i, &from) in themes.iter().enumerate() {
        let to = themes[(i + 1) % themes.len()];
        if to == from {
            continue;
        }
        for hz in [4.0, 8.0, 12.0, 16.0] {
            let found = run_change(&gpu, from, to, &mut Pulse::new(hz, 12.0));
            eprintln!(
                "| pulse {hz} Hz | {} to {} | {} |",
                from.name(),
                to.name(),
                cell(&found)
            );
            if !found.worst.passes() {
                failed.push(format!("{hz} Hz {} to {}", from.name(), to.name()));
            }
        }
    }
    assert!(failed.is_empty(), "over three flashes: {failed:?}");
}

impl Gpu {
    /// The frame drawn last ([`Self::draw`] wrote its uniforms), seen from
    /// the seated eye looking along `forward`, `width` by `height` pixels
    /// across `fov_deg` degrees, as a PPM (light over black, sRGB): a still
    /// to look at, larger than the cube's faces.
    fn still(
        &self,
        draws: &Draws,
        forward: Vec3,
        fov_deg: f32,
        (width, height): (u32, u32),
        path: &std::path::Path,
    ) {
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("still"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let half = (0.5 * fov_deg).to_radians();
        let up_down = (half.tan() * height as f32 / width as f32).atan();
        let fov = Fov {
            left: -half,
            right: half,
            up: up_down,
            down: -up_down,
        };
        let forward = forward.normalize();
        let right = forward.cross(Vec3::Y).normalize();
        let look = Quat::from_mat3(&Mat3::from_cols(right, right.cross(forward), -forward));
        let vp = math::view_projection(look.to_array(), scene().eye.to_array(), fov, 0.05, 100.0);
        let mut block = [0.0f32; surface_port::EYE_BYTES / 4];
        block[..16].copy_from_slice(&vp.to_cols_array());
        block[16..18].copy_from_slice(&[width as f32, height as f32]);
        let eye = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("still-eye"),
            size: surface_port::EYE_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&eye, 0, bytemuck::cast_slice(&block));
        let group = |layout: &wgpu::BindGroupLayout, buffers: &[&wgpu::Buffer]| {
            let entries: Vec<_> = buffers
                .iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource: b.as_entire_binding(),
                })
                .collect();
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout,
                entries: &entries,
            })
        };
        let fx_eye = group(&self.fx.get_bind_group_layout(0), &[&eye]);
        let port_eyes: Vec<wgpu::BindGroup> = draws
            .ports
            .iter()
            .zip(&self.port_slots)
            .map(|((effect, ..), (_, rows, ..))| {
                group(&self.ports[*effect].get_bind_group_layout(1), &[&eye, rows])
            })
            .collect();
        let edge_eyes =
            [&self.strips, &self.tunnels].map(|p| group(&p.get_bind_group_layout(0), &[&eye]));
        let row = (width * 8).next_multiple_of(256);
        let back = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("still-back"),
            size: u64::from(row * height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("still"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.fx);
            pass.set_bind_group(0, &fx_eye, &[]);
            for (_, g) in self.fx_slots.iter().take(draws.faces.len()) {
                pass.set_bind_group(1, g, &[]);
                pass.draw(0..6, 0..1);
            }
            for (((effect, ..), (.., g0, _)), g1) in
                draws.ports.iter().zip(&self.port_slots).zip(&port_eyes)
            {
                pass.set_pipeline(&self.ports[*effect]);
                pass.set_bind_group(0, g0, &[]);
                pass.set_bind_group(1, g1, &[]);
                pass.draw(0..6, 0..1);
            }
            let edges = [(&self.strips, draws.strips), (&self.tunnels, draws.tunnels)];
            for (((pipeline, n), (_, rows)), eye) in
                edges.into_iter().zip(&self.edge_groups).zip(&edge_eyes)
            {
                if n > 0 {
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, eye, &[]);
                    pass.set_bind_group(1, rows, &[]);
                    pass.draw(0..6, 0..n as u32);
                }
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &back,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        back.slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("poll");
        let bytes = back.slice(..).get_mapped_range().to_vec();
        let half_float = |h: u16| {
            let (e, m) = (i32::from((h >> 10) & 0x1f), f32::from(h & 0x3ff));
            match e {
                0 => m * 2f32.powi(-24),
                31 => 1.0,
                _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
            }
        };
        let mut out = format!("P6\n{width} {height}\n255\n").into_bytes();
        for y in 0..height as usize {
            for x in 0..width as usize {
                let o = y * row as usize + x * 8;
                for c in 0..3 {
                    let v =
                        half_float(u16::from_le_bytes([bytes[o + 2 * c], bytes[o + 2 * c + 1]]));
                    out.push((v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8);
                }
            }
        }
        std::fs::write(path, out).expect("writing the still");
    }
}

/// Flint with its ceiling's strength raised by hand (board #5533) over the
/// dumped songs, the limiter on: the ceiling's facets draw at their cap
/// (`surfaces::CEILING_SHARDS_LIMITED`), so no region passes three
/// flashes. Uncapped at full strength, one song reached 3.5 overhead.
/// `FOSFORA_FLASH_CEILING=<strength>` raises it to another value than 1,
/// and `FOSFORA_FLASH_UNCAPPED` draws it as asked, to find the cap;
/// `FOSFORA_FLASH_EYE` seats the eye near a wall. Nothing runs without
/// the dumps' directory.
#[test]
#[ignore = "needs a GPU adapter and song dumps"]
fn flints_ceiling_raised_by_hand_stays_within_three_flashes() {
    let songs = dumps();
    if songs.is_empty() {
        return;
    }
    let ceiling: f32 = std::env::var("FOSFORA_FLASH_CEILING")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    let uncapped = std::env::var_os("FOSFORA_FLASH_UNCAPPED").is_some();
    let gpu = Gpu::new(&effects());
    let mut failed = Vec::new();
    for (name, path) in &songs {
        let mut show = Show::new(Theme::Flint, None, Part::All, true, effects());
        show.ceiling = Some(ceiling);
        show.uncapped = uncapped;
        let mut dump = Dump::read(path).expect("a dump");
        let found = run_show(&gpu, show, &mut dump);
        eprintln!(
            "| {name} | flint, ceiling at {ceiling}{} | {} |",
            if uncapped { ", uncapped" } else { "" },
            cell(&found)
        );
        if !found.worst.passes() {
            failed.push(name.clone());
        }
    }
    assert!(failed.is_empty(), "over three flashes: {failed:?}");
}

/// Stills of a theme, to look at: `FOSFORA_STILL=<theme>:<dump>:<dir>:
/// <seconds>[,<seconds>…][:<part>]` plays the dump's song on the theme
/// (or on one part of it alone) with the
/// limiter on and writes, at each of those times, the view ahead, the
/// view to the right and down and the view to the left and down (the
/// chair) as `<theme>_<seconds>_<view>.ppm`, 1280 by
/// 720 across 100 degrees. The room is black and nothing hides anything:
/// the harness draws no occluder, so a wall shows through the table.
#[test]
#[ignore = "needs a GPU adapter and a song dump"]
fn stills_of_a_theme() {
    let Ok(spec) = std::env::var("FOSFORA_STILL") else {
        return;
    };
    let v: Vec<&str> = spec.split(':').collect();
    let theme = Theme::from_name(v[0]).expect("a theme");
    let mut source = Dump::read(std::path::Path::new(v[1])).expect("a dump");
    let dir = std::path::Path::new(v[2]);
    let mut times: Vec<f64> = v[3]
        .split(',')
        .map(|t| t.parse().expect("seconds"))
        .collect();
    times.sort_by(f64::total_cmp);
    // A fifth field names one part of the theme to draw alone, as the
    // tables print it ("sparks", "edge strips").
    let part = v.get(4).map_or(Part::All, |name| {
        Show::parts(theme)
            .into_iter()
            .find(|p| p.name() == *name)
            .expect("a part of the theme")
    });
    let gpu = Gpu::new(&effects());
    let mut show = Show::new(theme, None, part, true, effects());
    let dt = 1.0 / FPS;
    let views = [
        ("ahead", Vec3::new(0.0, -0.05, -1.0)),
        ("right", Vec3::new(0.8, -0.3, -0.6)),
        ("left", Vec3::new(-0.9, -0.45, -0.45)),
    ];
    let last = times.last().copied().unwrap_or(0.0);
    for n in 0..=(last * FPS).ceil() as usize {
        let t = n as f64 / FPS;
        let hop = source.at(t, dt);
        let draws = show.step(t as f32, dt as f32, &hop);
        if times.first().is_some_and(|at| t >= *at) {
            let at = times.remove(0);
            gpu.draw(&draws, 0);
            for (name, forward) in views {
                let file = format!("{}_{at:05.1}_{name}.ppm", theme.name());
                gpu.still(&draws, forward, 100.0, (1280, 720), &dir.join(file));
            }
            eprintln!("{} at {at} s: rms {:.2}", theme.name(), hop.features.rms);
        }
    }
}

// ---- The sweep over the depth light (board #5511, `sonar.rs`) -----------

/// A flat surface the sweep's count draws the depth light on: what to call
/// it, a point of it and its normal.
struct Lit {
    name: &'static str,
    at: Vec3,
    normal: Vec3,
}

/// Rays along a counted region's side: `SWEEP_SIDE` squared cover its 10
/// degrees.
const SWEEP_SIDE: usize = 12;
/// A pixel of the faces layer, where the depth light draws (rad): 840
/// across about 94 degrees.
const SWEEP_PIXEL_RAD: f32 = 0.001_95;

/// The region of the view `CELL_DEG` degrees across around the ray from
/// `eye` to `surface`'s point: where each of its rays meets the surface,
/// and the world's change across a pixel each way there.
fn sweep_region(eye: Vec3, surface: &Lit) -> Vec<(Vec3, [Vec3; 2])> {
    let c = (surface.at - eye).normalize();
    let u = c.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
    let v = u.cross(c);
    let hit = |a: f32, b: f32| {
        let ray = (c + u * a.tan() + v * b.tan()).normalize();
        eye + ray * ((surface.at - eye).dot(surface.normal) / ray.dot(surface.normal))
    };
    let half = (CELL_DEG as f32 / 2.0).to_radians();
    let at = |k: usize| -half + 2.0 * half * (k as f32 + 0.5) / SWEEP_SIDE as f32;
    (0..SWEEP_SIDE * SWEEP_SIDE)
        .map(|n| {
            let (a, b) = (at(n % SWEEP_SIDE), at(n / SWEEP_SIDE));
            let p = hit(a, b);
            (
                p,
                [
                    hit(a + SWEEP_PIXEL_RAD, b) - p,
                    hit(a, b + SWEEP_PIXEL_RAD) - p,
                ],
            )
        })
        .collect()
}

/// A source with a build and drops laid over it: `buildup` at `build`,
/// and a drop asked on every frame from `drops_from` on (the limiter lets
/// one through per its gap).
struct Building<S: Source> {
    under: S,
    build: f32,
    drops_from: f64,
}

impl<S: Source> Source for Building<S> {
    fn duration(&self) -> f64 {
        self.under.duration()
    }

    fn at(&mut self, t: f64, dt: f64) -> HopAt {
        let mut hop = self.under.at(t, dt);
        hop.features.buildup = self.build;
        hop.drop = t >= self.drops_from;
        hop.features.drop = f32::from(u8::from(hop.drop));
        hop
    }
}

/// The depth light with the sweep over `source`, as `app.rs` feeds it
/// behind the limiter, at immersion level 1 in `theme`'s colors, look and
/// band (board #5543), the regions of `surfaces` counted: the worst
/// region's transitions, the sweeps that started and the standing lines'
/// greatest strength.
fn sweep_run(
    theme: Theme,
    source: &mut dyn Source,
    sonar_opts: &crate::sonar::SonarOptions,
    light_gain: f32,
    surfaces: &[Lit],
) -> (Worst, &'static str, u32, f32) {
    use crate::depth_light::{self, DepthLightOptions, Drive};
    use crate::flash::{Triggers, follower};
    let eye = Vec3::new(0.0, 1.2, 0.0);
    let theme = Some(theme);
    let light_opts = DepthLightOptions {
        gain: light_gain,
        ..DepthLightOptions::default().under(theme)
    };
    let tile = depth_light::points_tile();
    let regions: Vec<_> = surfaces.iter().map(|s| sweep_region(eye, s)).collect();
    let mut counters = vec![Counter::new(); surfaces.len()];
    let mut limiter = Limiter::new(true);
    let mut drive = Drive::default();
    let mut sonar = crate::sonar::Sonar::default();
    let mut beat_env = 0.0f32;
    let (mut starts, mut lines) = (0, 0.0f32);
    let dt = 1.0 / FPS;
    for n in 0..(source.duration() * FPS).floor() as usize {
        let t = n as f64 / FPS;
        let hop = source.at(t, dt);
        let dt = dt as f32;
        let gate = limiter.begin(
            dt,
            Triggers {
                beat: hop.beat,
                downbeat: hop.downbeat,
                drop: hop.drop,
            },
        );
        let mut f = hop.features;
        limiter.features(f.as_slice_mut(), dt, gate);
        if gate.beat {
            beat_env = 1.0;
        }
        beat_env *= (-dt * 6.0).exp();
        let beat_shown = limiter.follow(follower::BEAT_ENV, beat_env);
        let mut audio = Audio {
            rms: f.rms,
            bass: f.bass,
            beat: beat_shown,
            mid: f.mid,
            high: f.presence.max(f.brilliance),
            key_tint: [1.0; 3],
            ..Audio::default()
        };
        let hue = fosfora_app::bindings::sources::collect_audio(&f)
            .get("audio.key_hue")
            .map_or(0.5, |v| v.0);
        audio.key_tint = key_tint(hue);
        limiter.follow_all(follower::TINT, &mut audio.key_tint);
        drive.step(
            dt,
            depth_light::band_level(&audio, depth_light::band(theme)),
            beat_shown,
        );
        drive.step_wind(dt, depth_light::band_level(&audio, surface_fx::BAND_RMS));
        let shown = drive.with_music(limiter.follow(follower::DEPTH_MUSIC, drive.music()));
        let started = sonar.step(
            sonar_opts,
            crate::sonar::Input {
                dt,
                downbeat: gate.downbeat,
                drop: gate.drop,
                beat_period_s: f.beat_period_secs(),
                buildup: f.buildup,
                quiet: false,
                head: eye,
                motion: limiter.motion(),
            },
        );
        starts += u32::from(started.is_some());
        lines = lines.max(sonar.contours().strength);
        let rows = depth_light::frame(
            &light_opts,
            &shown,
            theme,
            audio.key_tint,
            LEVEL,
            &sonar.rows(sonar_opts, LEVEL),
        )
        .rows;
        let swept = depth_light::swept(&rows);
        for ((surface, region), counter) in surfaces.iter().zip(&regions).zip(&mut counters) {
            let mut sum = [0.0f32; 3];
            for &(p, across) in region {
                let range = (p - eye).length();
                let bright = 1.0
                    + (depth_light::FAR_FLOOR - 1.0)
                        * depth_light::smoothstep(
                            depth_light::FADE_M[0],
                            depth_light::FADE_M[1],
                            range,
                        );
                let c = if swept {
                    depth_light::fragment_swept(
                        &rows,
                        &tile,
                        eye,
                        p,
                        surface.normal,
                        1.0,
                        bright,
                        across,
                    )
                } else {
                    let fw = across[0].abs() + across[1].abs();
                    depth_light::fragment(&rows, &tile, eye, p, surface.normal, 1.0, bright, fw)
                };
                for k in 0..3 {
                    sum[k] += c[k].clamp(0.0, 1.0);
                }
            }
            counter.push(t, sum.map(|v| v / region.len() as f32));
        }
    }
    let mut worst = Worst::default();
    let mut at = "";
    for (surface, c) in surfaces.iter().zip(&counters) {
        let w = c.worst();
        if w.general > worst.general || w.red > worst.red {
            at = surface.name;
        }
        worst = worst.max(w);
    }
    (worst, at, starts, lines)
}

/// What the depth light is counted on: a wall that faces the wearer and
/// lights all at once, the floor ahead (a top, in the tops' color), a far
/// wall and a wall seen at a slant.
fn depth_surfaces() -> [Lit; 4] {
    [
        Lit {
            name: "a wall 1.2 m ahead",
            at: Vec3::new(0.0, 1.2, -1.2),
            normal: Vec3::Z,
        },
        Lit {
            name: "the floor 1.5 m ahead",
            at: Vec3::new(0.0, 0.0, -1.5),
            normal: Vec3::Y,
        },
        Lit {
            name: "a wall 3.2 m ahead",
            at: Vec3::new(0.3, 1.4, -3.2),
            normal: Vec3::Z,
        },
        Lit {
            name: "a wall at a slant 2 m away",
            at: Vec3::new(1.4, 1.0, -1.4),
            normal: Vec3::new(-0.9, 0.0, 0.436).normalize(),
        },
    ]
}

/// The sweep and the standing lines over the depth light's points, behind
/// the limiter as `app.rs` runs them, on the depth light's Rust copy
/// (which the GPU test holds to the shader): a wall that faces the wearer
/// and lights all at once, the floor ahead in the key's tint (red, on the
/// pulse trains), a far wall and a wall seen at a slant. On the pulse
/// trains, with the test knob starting sweeps as fast as the gap allows
/// and both gains at their most, and through a build with drops asked on
/// every frame, no region passes three general or three red flashes in
/// any second.
#[test]
fn the_sweep_over_the_depth_light_stays_within_three_flashes() {
    use crate::sonar::{MIN_GAP_S, SonarOptions};
    let surfaces = depth_surfaces();
    let on = SonarOptions::default();
    let fastest = SonarOptions {
        test_s: Some(MIN_GAP_S),
        gain: crate::sonar::GAIN_MAX,
        ..on
    };
    let secs = 8.0;
    let mut failed = Vec::new();
    let mut run = |what: &str, source: &mut dyn Source, opts: &SonarOptions, gain: f32| {
        let (worst, at, starts, lines) = sweep_run(Theme::Aurora, source, opts, gain, &surfaces);
        eprintln!(
            "| {what} | {starts} | {lines:.2} | {:.1} | {:.1} | {at} |",
            worst.general_flashes(),
            worst.red_flashes()
        );
        if !worst.passes() {
            failed.push(what.to_owned());
        }
        (starts, lines)
    };
    eprintln!(
        "| source | sweeps in {secs} s | standing lines at most | general flashes | red flashes | worst region |"
    );
    eprintln!("|---|---|---|---|---|---|");
    for hz in [2.0, 4.0, 8.0, 16.0] {
        let what = format!("pulse {hz} Hz");
        let (starts, _) = run(&what, &mut Pulse::new(hz, secs), &on, 1.0);
        // A downbeat every fourth pulse, one let through per 0.75 s.
        assert!((4..=11).contains(&starts), "{what}: {starts}");
        let what = format!("pulse {hz} Hz, a sweep every {MIN_GAP_S} s, both gains at 4");
        let (starts, _) = run(&what, &mut Pulse::new(hz, secs), &fastest, 4.0);
        // One per 0.7 s, less the ones three alive held back.
        assert!((8..=12).contains(&starts), "{what}: {starts}");
    }
    // No beat at all: the test knob's sweeps alone.
    let mut still = Still { level: 0.6, secs };
    let (starts, _) = run("no beat, a sweep every 0.7 s", &mut still, &fastest, 4.0);
    assert!(starts >= 8, "{starts}");
    assert_eq!(
        sweep_run(Theme::Aurora, &mut still, &on, 1.0, &surfaces).2,
        0
    );
    // A full build, and drops asked on every frame from 5 s on.
    for hz in [2.0, 8.0] {
        let what = format!("pulse {hz} Hz, a full build, drops from 5 s");
        let mut source = Building {
            under: Pulse::new(hz, 12.0),
            build: 1.0,
            drops_from: 5.0,
        };
        let (_, lines) = run(&what, &mut source, &on, 1.0);
        assert!(lines > 0.9, "{what}: the standing lines reached {lines}");
    }
    assert!(failed.is_empty(), "over three flashes: {failed:?}");
}

/// The themes whose depth light draws a look of its own (board #5543),
/// each with that look: Aurora's beams, Flint's coals, Contour's grass
/// and Drift's bubbles, on the band the theme gives them.
const DEPTH_LOOKS: [(Theme, crate::themes::DepthLook); 4] = [
    (Theme::Aurora, crate::themes::DepthLook::Beams),
    (Theme::Flint, crate::themes::DepthLook::Coals),
    (Theme::Contour, crate::themes::DepthLook::Grass),
    (Theme::Drift, crate::themes::DepthLook::Bubbles),
];

/// The beams, the coals and the grass behind the limiter as `app.rs` runs
/// them, on the depth light's Rust copy (which the GPU test holds to the
/// shader), each in its theme's colors and on its band: on the pulse
/// trains, with the sweep as it ships and without it, at the light's gain
/// and at its most, no region passes three general or three red flashes
/// in any second. A kick launches the beams and flares the coals through
/// the music the limiter follows (`flash::follower::DEPTH_MUSIC`), so the
/// limiter holds them as it holds the points.
#[test]
fn the_depth_lights_looks_stay_within_three_flashes() {
    use crate::sonar::SonarOptions;
    let surfaces = depth_surfaces();
    let on = SonarOptions::default();
    let off = SonarOptions { mode: 0, ..on };
    let mut failed = Vec::new();
    eprintln!("| theme | look | source | general flashes | red flashes | worst region |");
    eprintln!("|---|---|---|---|---|---|");
    for (theme, look) in DEPTH_LOOKS {
        assert_eq!(crate::depth_light::look(Some(theme)), look);
        for hz in [2.0, 4.0, 8.0, 16.0] {
            for (sweep, opts, gain) in [
                ("no sweep", &off, 1.0),
                ("the sweep", &on, 1.0),
                (
                    "the sweep, the gain at 4",
                    &on,
                    crate::depth_light::GAIN_MAX,
                ),
            ] {
                let what = format!("pulse {hz} Hz, {sweep}");
                let (worst, at, ..) =
                    sweep_run(theme, &mut Pulse::new(hz, 8.0), opts, gain, &surfaces);
                eprintln!(
                    "| {} | {} | {what} | {:.1} | {:.1} | {at} |",
                    theme.name(),
                    look.name(),
                    worst.general_flashes(),
                    worst.red_flashes()
                );
                if !worst.passes() {
                    failed.push(format!("{} {what}", look.name()));
                }
            }
        }
    }
    assert!(failed.is_empty(), "over three flashes: {failed:?}");
}

/// The same looks over the songs dumped into `FOSFORA_FLASH_DUMPS`
/// (`*.bin`), the sweep as it ships. Nothing runs without the directory;
/// no song is in the repo.
#[test]
#[ignore = "needs song dumps"]
fn the_depth_lights_looks_on_the_dumped_songs_stay_within_three_flashes() {
    let Some(dir) = std::env::var_os("FOSFORA_FLASH_DUMPS") else {
        eprintln!("FOSFORA_FLASH_DUMPS unset: no songs");
        return;
    };
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("the dumps' directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .collect();
    paths.sort();
    let surfaces = depth_surfaces();
    let opts = crate::sonar::SonarOptions::default();
    let mut failed = Vec::new();
    eprintln!("| song | theme | look | general flashes | red flashes | worst region |");
    eprintln!("|---|---|---|---|---|---|");
    for path in &paths {
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        let name = name.unwrap_or_default();
        for (theme, look) in DEPTH_LOOKS {
            let mut dump = Dump::read(path).expect("a dump");
            let (worst, at, ..) = sweep_run(theme, &mut dump, &opts, 1.0, &surfaces);
            eprintln!(
                "| {name} | {} | {} | {:.1} | {:.1} | {at} |",
                theme.name(),
                look.name(),
                worst.general_flashes(),
                worst.red_flashes()
            );
            if !worst.passes() {
                failed.push(format!("{name} {}", look.name()));
            }
        }
    }
    assert!(failed.is_empty(), "over three flashes: {failed:?}");
}

// ---- The bars against the analysis (board #5532) -------------------------

/// The spectrum's bars over a source, frame by frame, three ways: as the
/// analysis makes them (the canvas on the mel bands as they come), as the
/// limiter shows them (the canvas on the limited mel bands, followed, in
/// `app.rs`'s order), and as the edge bars draw them free
/// (`edge_bars::Heights`).
struct BarRun {
    raw: Vec<[f32; DEFAULT_BARS]>,
    shown: Vec<[f32; DEFAULT_BARS]>,
    free: Vec<[f32; DEFAULT_BARS]>,
    /// Beats the analysis fired, and those the limiter let through.
    beats: (u32, u32),
    /// The direction's turns.
    turns: u32,
}

fn bar_run(source: &mut dyn Source) -> BarRun {
    use crate::flash::{Triggers, follower};
    let mut limiter = Limiter::new(true);
    let mut analyzed = Canvas::new(DEFAULT_BARS, 1.0);
    let mut limited = Canvas::new(DEFAULT_BARS, 1.0);
    let mut heights = edge_bars::Heights::new(DEFAULT_BARS);
    let (mut beat_env, mut downbeat_env) = (0.0f32, 0.0f32);
    let mut run = BarRun {
        raw: Vec::new(),
        shown: Vec::new(),
        free: Vec::new(),
        beats: (0, 0),
        turns: 0,
    };
    let mut phase = limiter.phase();
    let dt = 1.0 / FPS;
    for n in 0..(source.duration() * FPS).floor() as usize {
        let hop = source.at(n as f64 / FPS, dt);
        let dt = dt as f32;
        let gate = limiter.begin(
            dt,
            Triggers {
                beat: hop.beat,
                downbeat: hop.downbeat,
                drop: hop.drop,
            },
        );
        run.beats.0 += u32::from(hop.beat);
        run.beats.1 += u32::from(gate.beat);
        run.turns += u32::from(limiter.phase() != phase);
        phase = limiter.phase();
        let mut f = hop.features;
        limiter.features(f.as_slice_mut(), dt, gate);
        let mut mel = hop.mel;
        limiter.mel(&mut mel, dt);
        if gate.beat {
            beat_env = 1.0;
        }
        beat_env *= (-dt * 6.0).exp();
        downbeat_env = surface_fx::downbeat_step(downbeat_env, gate.downbeat, dt);
        limiter.follow(follower::BEAT_ENV, beat_env);
        limiter.follow(follower::DOWNBEAT_ENV, downbeat_env);
        analyzed.update(dt, &hop.mel);
        limited.update(dt, &mel);
        let mut shown = [0.0; DEFAULT_BARS];
        shown.copy_from_slice(limited.heights());
        limiter.follow_all(follower::BARS, &mut shown);
        limiter.follow(follower::BAR_GLOW, limited.glow());
        let take = |v: &[f32]| {
            let mut out = [0.0; DEFAULT_BARS];
            out.copy_from_slice(v);
            out
        };
        run.raw.push(take(analyzed.heights()));
        run.free
            .push(take(heights.step(dt, true, &hop.mel, &shown).now));
        run.shown.push(shown);
    }
    run
}

/// The frames at which a series has risen `delta` from its last low: a
/// zigzag with that threshold, as the limiter counts a level's hits.
fn rises(series: impl Iterator<Item = f32>, delta: f32) -> Vec<usize> {
    let mut out = Vec::new();
    let (mut rising, mut ext) = (true, f32::MAX);
    for (n, x) in series.enumerate() {
        if n == 0 {
            ext = x;
        } else if rising {
            if x > ext {
                ext = x;
            } else if ext - x >= delta {
                (rising, ext) = (false, x);
            }
        } else if x < ext {
            ext = x;
        } else if x - ext >= delta {
            (rising, ext) = (true, x);
            out.push(n);
        }
    }
    out
}

/// Pearson's correlation of two series; 0 where either stands still.
fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len()) as f64;
    let mean = |v: &[f32]| v.iter().map(|&x| f64::from(x)).sum::<f64>() / n;
    let (ma, mb) = (mean(a), mean(b));
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (&x, &y) in a.iter().zip(b) {
        let (x, y) = (f64::from(x) - ma, f64::from(y) - mb);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa < 1e-12 || bb < 1e-12 {
        0.0
    } else {
        ab / (aa * bb).sqrt()
    }
}

/// What a run's bars say (the fields' docs).
#[derive(Debug, Clone, Copy, Default)]
struct BarStats {
    /// Rises of the raw bars over the threshold.
    rises: usize,
    /// The delay from a raw bar's rise to the shown bar's (ms): the median
    /// and the 95th percentile over the rises the shown bar made.
    median_ms: f64,
    p95_ms: f64,
    /// The share of raw rises the shown bar made more than 50 ms late or
    /// not at all before the raw bar's next rise (or a second), and the
    /// share it never made.
    late: f64,
    lost: f64,
    /// The share of raw rises on whose own frame the shown bar is at the
    /// raw bar's height or over it.
    there: f64,
    /// The mean correlation between two different bars' heights, raw and
    /// shown, and between their changes from frame to frame.
    alike: (f64, f64),
    alike_moves: (f64, f64),
    /// The mean correlation between a bar as the analysis makes it and the
    /// same bar shown.
    tracks: f64,
    /// The share of a raw bar's frames at the top of its strip (0.98 and
    /// up) and at the bottom (0.02 and under).
    pinned: (f64, f64),
}

fn bar_stats(raw: &[[f32; DEFAULT_BARS]], shown: &[[f32; DEFAULT_BARS]], delta: f32) -> BarStats {
    let bar = |v: &[[f32; DEFAULT_BARS]], b: usize| v.iter().map(|f| f[b]).collect::<Vec<f32>>();
    let moves = |v: &[f32]| v.windows(2).map(|w| w[1] - w[0]).collect::<Vec<f32>>();
    let raws: Vec<Vec<f32>> = (0..DEFAULT_BARS).map(|b| bar(raw, b)).collect();
    let showns: Vec<Vec<f32>> = (0..DEFAULT_BARS).map(|b| bar(shown, b)).collect();
    let mut delays = Vec::new();
    let (mut all, mut late, mut lost, mut there) = (0usize, 0usize, 0usize, 0usize);
    let second = FPS as usize;
    for b in 0..DEFAULT_BARS {
        let a = rises(raws[b].iter().copied(), delta);
        let s = rises(showns[b].iter().copied(), delta);
        for (k, &at) in a.iter().enumerate() {
            let until = a.get(k + 1).map_or(at + second, |&n| n.min(at + second));
            all += 1;
            there += usize::from(showns[b][at] >= raws[b][at] - 1e-4);
            match s.iter().find(|&&n| n + 2 >= at && n < until) {
                Some(&n) => {
                    let ms = n.saturating_sub(at) as f64 / FPS * 1000.0;
                    late += usize::from(ms > 50.0);
                    delays.push(ms);
                }
                None => {
                    late += 1;
                    lost += 1;
                }
            }
        }
    }
    delays.sort_by(f64::total_cmp);
    let pct = |p: f64| {
        delays
            .get(((delays.len() as f64 - 1.0) * p).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    let pairs = |v: &[Vec<f32>]| {
        let (mut sum, mut n) = (0.0, 0);
        for i in 0..v.len() {
            for j in i + 1..v.len() {
                sum += correlation(&v[i], &v[j]);
                n += 1;
            }
        }
        sum / f64::from(n)
    };
    let (raw_moves, shown_moves): (Vec<_>, Vec<_>) = (
        raws.iter().map(|v| moves(v)).collect(),
        showns.iter().map(|v| moves(v)).collect(),
    );
    let cells = (raw.len() * DEFAULT_BARS) as f64;
    let share =
        |test: fn(f32) -> bool| raw.iter().flatten().filter(|&&h| test(h)).count() as f64 / cells;
    let of_all = |n: usize| n as f64 / all.max(1) as f64;
    BarStats {
        rises: all,
        median_ms: pct(0.5),
        p95_ms: pct(0.95),
        late: of_all(late),
        lost: of_all(lost),
        there: of_all(there),
        alike: (pairs(&raws), pairs(&showns)),
        alike_moves: (pairs(&raw_moves), pairs(&shown_moves)),
        tracks: raws
            .iter()
            .zip(&showns)
            .map(|(a, b)| correlation(a, b))
            .sum::<f64>()
            / DEFAULT_BARS as f64,
        pinned: (share(|h| h >= 0.98), share(|h| h <= 0.02)),
    }
}

/// The dumps in `FOSFORA_FLASH_DUMPS` (`*.bin`), sorted, by name.
fn dumps() -> Vec<(String, std::path::PathBuf)> {
    let Some(dir) = std::env::var_os("FOSFORA_FLASH_DUMPS") else {
        eprintln!("FOSFORA_FLASH_DUMPS unset: no songs");
        return Vec::new();
    };
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("the dumps' directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "bin"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let name = p.file_stem().map(|s| s.to_string_lossy().into_owned());
            (name.unwrap_or_default(), p)
        })
        .collect()
}

/// The bars of the dumped songs as the analysis makes them against the
/// bars the limiter shows and the edge bars' free ones: the tables of
/// board #5532. No GPU; nothing runs without the dumps.
#[test]
#[ignore = "needs song dumps"]
fn the_bars_of_the_dumped_songs_against_the_analysis() {
    eprintln!(
        "| song | beats fired | let through | turns a second | raw rises | limited: delay median ms | p95 ms | over 50 ms or never | never | there on the frame | free: delay median ms | p95 ms | over 50 ms or never | never | there on the frame |"
    );
    eprintln!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    let mut alike = Vec::new();
    for (name, path) in dumps() {
        let mut dump = Dump::read(&path).expect("a dump");
        let secs = dump.duration();
        let run = bar_run(&mut dump);
        for delta in [0.1, 0.3] {
            let s = bar_stats(&run.raw, &run.shown, delta);
            let f = bar_stats(&run.raw, &run.free, delta);
            eprintln!(
                "| {name} ({delta}) | {} | {} | {:.1} | {} | {:.0} | {:.0} | {:.0} % | {:.0} % | {:.0} % | {:.0} | {:.0} | {:.0} % | {:.0} % | {:.0} % |",
                run.beats.0,
                run.beats.1,
                f64::from(run.turns) / secs,
                s.rises,
                s.median_ms,
                s.p95_ms,
                100.0 * s.late,
                100.0 * s.lost,
                100.0 * s.there,
                f.median_ms,
                f.p95_ms,
                100.0 * f.late,
                100.0 * f.lost,
                100.0 * f.there,
            );
            if delta < 0.2 {
                alike.push(format!(
                    "| {name} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.2} | {:.0} % | {:.0} % |",
                    s.alike.0,
                    s.alike.1,
                    f.alike.1,
                    s.alike_moves.0,
                    s.alike_moves.1,
                    f.alike_moves.1,
                    s.tracks,
                    f.tracks,
                    100.0 * s.pinned.0,
                    100.0 * s.pinned.1,
                ));
            }
        }
    }
    eprintln!(
        "\n| song | bars alike: raw | limited | free | moves alike: raw | limited | free | tracks its raw bar: limited | free | raw at the top | at the bottom |"
    );
    eprintln!("|---|---|---|---|---|---|---|---|---|---|---|");
    for row in alike {
        eprintln!("{row}");
    }
}

/// A song made up for the tests: a steady kick at 124 BPM on every level
/// the looks read (a pulse train), under mel bands that each take hits of
/// their own, three bands to a bar, from 1 to 6 a second, 40 ms long,
/// 28 dB over what lies between them.
struct OwnHits {
    kick: Pulse,
}

impl OwnHits {
    fn new(secs: f64) -> Self {
        Self {
            kick: Pulse::hits(124.0 / 60.0, secs),
        }
    }
}

impl Source for OwnHits {
    fn duration(&self) -> f64 {
        self.kick.duration()
    }

    fn at(&mut self, t: f64, dt: f64) -> HopAt {
        let mut hop = self.kick.at(t, dt);
        for (k, m) in hop.mel.iter_mut().enumerate() {
            let group = (k / 3) as f64;
            let hz = 1.0 + (group * 7.0) % 11.0 * 0.5;
            let hit = (t * hz + group * 0.37).fract() < 0.04 * hz;
            *m = if hit { 0.85 } else { 0.5 };
        }
        hop
    }
}

/// The reason for the knob, and what it buys (board #5532): behind the
/// limiter a bar shows a good share of its own hits late or not at all;
/// free, it is at the analysis's height on the frame of every hit, and
/// where it shows the rise it shows it on that frame.
#[test]
fn free_bars_rise_on_the_frame_of_their_own_hits() {
    let run = bar_run(&mut OwnHits::new(30.0));
    let limited = bar_stats(&run.raw, &run.shown, 0.1);
    let free = bar_stats(&run.raw, &run.free, 0.1);
    eprintln!("limited: {limited:?}\nfree: {free:?}");
    assert!(limited.rises > 1000, "{}", limited.rises);
    // The kick is 124 BPM: every other beat goes through.
    assert!(run.beats.1 * 3 < run.beats.0 * 2, "{:?}", run.beats);
    // Limited: more than a third of the rises come over 50 ms late or
    // never, and the bars move more alike than the music has them.
    assert!(limited.late > 0.33, "{}", limited.late);
    assert!(limited.there < 0.9, "{}", limited.there);
    assert!(limited.alike_moves.1 > limited.alike_moves.0 + 0.1);
    // Free: at the hit's height on its frame, every time, and the rises
    // it shows it shows on that frame.
    assert!(free.there > 0.9999, "{}", free.there);
    assert!(free.p95_ms < 1e-9, "{free:?}");
    // A free bar falls at half the speed, so a hit that lands before the
    // bar is a tenth under it shows a smaller rise or none: about one in
    // ten at these rates.
    assert!(free.late < 0.15 && free.lost < 0.15, "{free:?}");
    assert!((free.alike_moves.1 - free.alike_moves.0).abs() < 0.1);
    assert!(free.tracks > 0.9 && free.tracks > limited.tracks + 0.1);
}

// ---- The edge bars' own light, counted on their Rust copy ----------------

/// Where a ray of a counted region meets the edge bars: a strip at a point
/// along its edge and off it (m), or a tunnel's quad at a world point, with
/// a ray's worth of the tunnel's depth there (m).
#[derive(Debug, Clone, Copy)]
enum BarHit {
    Strip(usize, f32, f32),
    Tunnel(usize, Vec3, f32),
}

/// Where the ray from `eye` along `ray` meets `strip`: m along its edge
/// from the low end and m off it, as the vertex stage hands them to the
/// fragment; `None` off the strip.
fn strip_hit(strip: &edge_bars::Strip, eye: Vec3, ray: Vec3) -> Option<(f32, f32)> {
    let [c0, c1, _, c3] = strip.corners;
    let along = (c1 - c0).try_normalize()?;
    let side = c3 - c0;
    let off = (side - along * side.dot(along)).try_normalize()?;
    let normal = along.cross(off);
    let t = (c0 - eye).dot(normal) / ray.dot(normal);
    if !t.is_finite() || t <= 0.0 {
        return None;
    }
    let p = eye + ray * t - c0;
    let far = strip.height * edge_bars::REACH;
    let (s, d) = (p.dot(along), p.dot(off));
    let miter = strip.inset * d / far.max(1e-6);
    ((0.0..=far).contains(&d) && s >= miter && s <= strip.length - miter).then_some((s, d))
}

/// Where the ray from `eye` along `ray` meets `tunnel`'s quad (lifted off
/// its face as the vertex stage lifts it), and how deep in the tunnel the
/// ray lands; `None` off the quad.
fn tunnel_point(tunnel: &edge_bars::Tunnel, eye: Vec3, ray: Vec3) -> Option<(Vec3, f32)> {
    let on = tunnel.center + tunnel.normal * edge_bars::LIFT_M;
    let t = (on - eye).dot(tunnel.normal) / ray.dot(tunnel.normal);
    if !t.is_finite() || t <= 0.0 {
        return None;
    }
    let p = eye + ray * t;
    let frame = |q: Vec3| {
        let q = q - tunnel.center;
        Vec3::new(
            q.dot(tunnel.axes[0]),
            q.dot(tunnel.axes[1]),
            q.dot(tunnel.normal),
        )
    };
    let local = frame(p);
    if local.x.abs() > tunnel.half[0] || local.y.abs() > tunnel.half[1] {
        return None;
    }
    let low = if tunnel.low { 1.0 } else { -1.0 };
    let hit = edge_bars::tunnel_hit(tunnel.half, tunnel.depth, low, frame(eye), local);
    Some((p, hit.depth))
}

/// Rays along a counted region's side for the bars' count.
const BAR_SIDE: usize = 8;
/// The regions the bars' count keeps: those the bars light most.
const BAR_REGIONS: usize = 32;

/// The harness's room outlined in `mode` on every kind, in white (brighter
/// than any color a bar takes) at immersion level 1, where the bars are
/// most opaque, and the regions of the cube's view the bars light most,
/// each ray's hits in the order they draw. Nothing hides a bar here: a
/// strip behind the table counts as if the table were not there.
fn bar_view(mode: edge_bars::Mode) -> BarView {
    let scene = scene();
    let shapes: Vec<Shape> = scene
        .boxes
        .iter()
        .map(|b| Shape {
            center: b.center,
            rot: b.rot,
            half: b.half,
            look: Some(edge_bars::Look {
                color: [1.0; 3],
                fade: 1.0,
                mode,
            }),
        })
        .collect();
    let mut outline = Outline::default();
    edge_bars::outline(
        &shapes,
        scene.eye,
        edge_bars::HEIGHT_FRACTION,
        edge_bars::DEPTH_FRACTION,
        DEFAULT_BARS,
        edge_bars::peak_alpha(LEVEL, 1.0),
        &mut outline,
    );
    // Every bar at full height, to find the regions they light most.
    let mut rows = Vec::new();
    let full = [1.0; DEFAULT_BARS];
    let mut history = History::default();
    for _ in 0..edge_bars::RUNGS * 8 {
        history.step(edge_bars::HISTORY_STEP_S, &full);
    }
    edge_bars::rows(&full, &history, &outline, &mut rows);
    let half = (CELL_DEG as f32 / 2.0).to_radians();
    let step = 2.0 * half / BAR_SIDE as f32;
    let at = |k: usize| -half + step * (k as f32 + 0.5);
    let mut regions = Vec::new();
    for r in 0..REGIONS {
        let per = (CELLS * CELLS) as usize;
        let (_, forward, up) = FACES[r / per];
        let (cx, cy) = ((r % per) % CELLS as usize, (r % per) / CELLS as usize);
        let mid = |k: usize| ((k as u32 * STEP_DEG + CELL_DEG / 2) as f32 - 45.0).to_radians();
        let right = forward.cross(up);
        let c = (forward + right * mid(cx).tan() - up * mid(cy).tan()).normalize();
        let u = c.cross(up).try_normalize().unwrap_or(right);
        let v = u.cross(c);
        let dir = |a: f32, b: f32| (c + u * a.tan() + v * b.tan()).normalize();
        let rays: Vec<Vec<BarHit>> = (0..BAR_SIDE * BAR_SIDE)
            .map(|n| {
                let (a, b) = (at(n % BAR_SIDE), at(n / BAR_SIDE));
                let ray = dir(a, b);
                let strips = outline.strips.iter().enumerate().filter_map(|(k, s)| {
                    strip_hit(s, scene.eye, ray).map(|(s, d)| BarHit::Strip(k, s, d))
                });
                let tunnels = outline.tunnels.iter().enumerate().filter_map(|(k, t)| {
                    let (p, z) = tunnel_point(t, scene.eye, ray)?;
                    // The next ray over lands this much deeper or
                    // shallower: the ray's footprint along the depth.
                    let px = tunnel_point(t, scene.eye, dir(a + step, b + step))
                        .map_or(0.01, |(_, z1)| (z1 - z).abs().max(1e-4));
                    Some(BarHit::Tunnel(k, p, px))
                });
                strips.chain(tunnels).collect()
            })
            .collect();
        let lit = bar_light(&rows, &outline, scene.eye, &rays);
        regions.push((lit, r, rays));
    }
    regions.sort_by(|a, b| b.0.total_cmp(&a.0));
    regions.truncate(BAR_REGIONS);
    (
        outline,
        regions.into_iter().map(|(_, r, rays)| (r, rays)).collect(),
    )
}

/// A region's mean light from the edge bars alone, over black: each ray's
/// strips and tunnels blended in the order they draw, premultiplied, on
/// the shaders' Rust copies (`edge_bars::alpha_at`, `tunnel_alpha_at`).
fn bar_light(rows: &[[f32; 4]], outline: &Outline, eye: Vec3, rays: &[Vec<BarHit>]) -> f32 {
    let mut sum = 0.0;
    for hits in rays {
        let mut light = 0.0;
        for hit in hits {
            let (alpha, color) = match *hit {
                BarHit::Strip(k, s, d) => {
                    (edge_bars::alpha_at(rows, k, s, d), outline.strips[k].color)
                }
                BarHit::Tunnel(k, p, px) => (
                    edge_bars::tunnel_alpha_at(rows, k, eye, p, px),
                    outline.tunnels[k].color,
                ),
            };
            light = light * (1.0 - alpha) + crate::flash::luminance(color) * alpha;
        }
        sum += light.clamp(0.0, 1.0);
    }
    sum / rays.len() as f32
}

/// A mode's outline and the regions its bars light most ([`bar_view`]).
type BarView = (Outline, Vec<(usize, Vec<Vec<BarHit>>)>);

/// The edge bars alone as `view` has them over `source`, their heights
/// free and the strips' falling `release` heights a second: the worst
/// region's transitions.
fn bar_count(view: &BarView, release: f32, source: &mut dyn Source) -> Worst {
    let (outline, regions) = view;
    let eye = scene().eye;
    let mut heights = edge_bars::Heights::falling(DEFAULT_BARS, release);
    let mut history = History::default();
    let mut counters = vec![Counter::new(); regions.len()];
    let mut rows = Vec::new();
    let dt = 1.0 / FPS;
    for n in 0..(source.duration() * FPS).floor() as usize {
        let t = n as f64 / FPS;
        let hop = source.at(t, dt);
        let bars = heights.step(dt as f32, true, &hop.mel, &[]);
        history.step(dt as f32, bars.past);
        edge_bars::rows(bars.now, &history, outline, &mut rows);
        for ((_, rays), counter) in regions.iter().zip(&mut counters) {
            counter.push(t, [bar_light(&rows, outline, eye, rays); 3]);
        }
    }
    counters
        .iter()
        .fold(Worst::default(), |w, c| w.max(c.worst()))
}

/// The edge bars' own light with their heights free (board #5532), in
/// each mode on every surface of the room, in white at immersion level 1,
/// on the pulse trains and on hits from 1.5 to 6 a second, on the
/// shaders' Rust copies: no region the bars light passes three flashes a
/// second on the face or standing, or one in. In white the strips reach
/// the three on hits 2.25 to 2.5 times a second, which is the hits' own
/// rate; faster hits leave a bar too little time to fall for a region to
/// count them. The whole picture in the themes' colors, with the themes
/// behind it, is counted on the GPU
/// (`the_edge_bars_with_free_heights_stay_within_three_flashes`).
#[test]
fn free_edge_bars_alone_stay_within_three_flashes() {
    eprintln!("| source | face | standing | in |");
    eprintln!("|---|---|---|---|");
    let mut failed = Vec::new();
    let views = BAR_MODES.map(bar_view);
    for (what, make, hz) in bar_trains() {
        let worst =
            [0, 1, 2].map(|k| bar_count(&views[k], edge_bars::FREE_RELEASE_PER_S, &mut make(hz)));
        eprintln!(
            "| {what} | {:.1} | {:.1} | {:.1} |",
            worst[0].general_flashes(),
            worst[1].general_flashes(),
            worst[2].general_flashes()
        );
        for (mode, w) in BAR_MODES.iter().zip(worst) {
            let most = if *mode == edge_bars::Mode::In {
                2
            } else {
                crate::flash::MAX_TRANSITIONS
            };
            if w.general > most || w.red > 0 {
                failed.push(format!("{what} {}: {w:?}", mode.name()));
            }
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
}

/// Why the strips' free bars fall at half the wall spectrum's speed: at
/// its speed, hits three or more times a second on every band take a
/// region beside a strip past three flashes a second.
#[test]
fn free_strips_falling_as_fast_as_the_wall_spectrum_pass_three_flashes() {
    let fast = crate::canvas::RELEASE_PER_S;
    let view = bar_view(edge_bars::Mode::Face);
    let worst = [3.0, 3.25, 3.5]
        .map(|hz| bar_count(&view, fast, &mut Pulse::hits(hz, 6.0)))
        .into_iter()
        .fold(Worst::default(), Worst::max);
    assert!(!worst.passes(), "{worst:?}");
}

/// A train of [`bar_trains`]: its name, what makes it and its rate.
type Train = (String, fn(f64) -> Pulse, f64);

/// The trains the edge bars' count runs on: the pulse trains every look is
/// counted on, and hits at the rates where a free bar swings furthest
/// between three and six times a second.
fn bar_trains() -> Vec<Train> {
    let mut out: Vec<Train> = Vec::new();
    for hz in [4.0, 8.0, 12.0, 16.0] {
        out.push((format!("pulse {hz} Hz"), |hz| Pulse::new(hz, 6.0), hz));
    }
    for hz in [
        1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0, 3.25, 3.5, 4.0, 5.0, 6.0,
    ] {
        out.push((format!("hits {hz} Hz"), |hz| Pulse::hits(hz, 6.0), hz));
    }
    out
}

/// The edge bars' modes a count forces on every kind, as the Edges control
/// does.
const BAR_MODES: [edge_bars::Mode; 3] = [
    edge_bars::Mode::Face,
    edge_bars::Mode::Standing,
    edge_bars::Mode::In,
];

/// What a count's edges are called in its table and in
/// `FOSFORA_FLASH_BARS`: `theme`, or the forced mode's name.
fn edges_name(edges: Choice) -> &'static str {
    match edges {
        Choice::Theme => "theme",
        Choice::Force(mode) => mode.name(),
    }
}

/// `theme` whole over the source `make` makes, behind the limiter, the
/// edge bars as `edges` has them, their heights limited then free: the row
/// printed, both runs returned.
fn limited_and_free(
    gpu: &Gpu,
    what: &str,
    theme: Theme,
    part: Part,
    edges: Choice,
    make: &mut dyn FnMut() -> Box<dyn Source>,
) -> [Found; 2] {
    let found = [false, true]
        .map(|bars_free| run_edges(gpu, theme, part, edges, bars_free, make().as_mut()));
    let [limited, free] = found;
    eprintln!(
        "| {what} | {} | {} | {} | {:.1} / {:.1} ({}) | {:.1} / {:.1} ({}) | {} |",
        theme.name(),
        edges_name(edges),
        part.name(),
        limited.worst.general_flashes(),
        limited.worst.red_flashes(),
        limited.failing,
        free.worst.general_flashes(),
        free.worst.red_flashes(),
        free.failing,
        cell(&free)
    );
    found
}

/// The edge bars alone, and each theme whole, with each mode forced on
/// every kind and the bars' heights following the analysis (board #5532),
/// behind the limiter, on the pulse trains and the hit trains: the bars
/// limited then free. `FOSFORA_FLASH_BARS=<mode>` runs one mode.
#[test]
#[ignore = "needs a GPU adapter"]
fn the_edge_bars_with_free_heights_stay_within_three_flashes() {
    let gpu = Gpu::new(&effects());
    let only = std::env::var("FOSFORA_FLASH_BARS").ok();
    let mut failed = Vec::new();
    eprintln!("| source | theme | edges | part | limited | free | free, worst |");
    eprintln!("|---|---|---|---|---|---|---|");
    for mode in BAR_MODES {
        if only.as_ref().is_some_and(|o| o != mode.name()) {
            continue;
        }
        let alone = if mode == edge_bars::Mode::In {
            Part::Tunnels
        } else {
            Part::Strips
        };
        for (what, make, hz) in bar_trains() {
            for theme in Theme::ALL {
                for part in [alone, Part::All] {
                    let [_, free] = limited_and_free(
                        &gpu,
                        &what,
                        theme,
                        part,
                        Choice::Force(mode),
                        &mut || Box::new(make(hz)),
                    );
                    if !free.worst.passes() {
                        failed.push(format!(
                            "{what} {} {} {}",
                            theme.name(),
                            mode.name(),
                            part.name()
                        ));
                    }
                }
            }
        }
    }
    assert!(failed.is_empty(), "over three flashes: {failed:?}");
}

/// The songs dumped into `FOSFORA_FLASH_DUMPS`, every theme whole (or those
/// `FOSFORA_FLASH_THEMES` names), behind the limiter, the edge bars in the
/// theme's modes and in each mode forced on every kind (or the one
/// `FOSFORA_FLASH_BARS` names: `theme`, `face`, `stand`, `in`), their
/// heights limited then free (board #5532). Nothing runs without the
/// directory.
#[test]
#[ignore = "needs a GPU adapter and song dumps"]
fn the_dumped_songs_with_free_edge_bars_stay_within_three_flashes() {
    let songs = dumps();
    if songs.is_empty() {
        return;
    }
    let themes: Option<Vec<String>> = std::env::var("FOSFORA_FLASH_THEMES")
        .ok()
        .map(|v| v.split(',').map(str::to_owned).collect());
    let only = std::env::var("FOSFORA_FLASH_BARS").ok();
    let gpu = Gpu::new(&effects());
    let mut failed = Vec::new();
    eprintln!("| source | theme | edges | part | limited | free | free, worst |");
    eprintln!("|---|---|---|---|---|---|---|");
    let all = std::iter::once(Choice::Theme).chain(BAR_MODES.map(Choice::Force));
    for edges in all.filter(|&e| only.as_ref().is_none_or(|o| o == edges_name(e))) {
        for (name, path) in &songs {
            for theme in Theme::ALL {
                if themes
                    .as_ref()
                    .is_some_and(|o| !o.iter().any(|n| n == theme.name()))
                {
                    continue;
                }
                let [_, free] = limited_and_free(&gpu, name, theme, Part::All, edges, &mut || {
                    Box::new(Dump::read(path).expect("a dump"))
                });
                if !free.worst.passes() {
                    failed.push(format!("{name} {} {}", theme.name(), edges_name(edges)));
                }
            }
        }
    }
    assert!(failed.is_empty(), "over three flashes: {failed:?}");
}
