//! Themes (board #5415): the whole room's look in one press. A theme is a
//! name and, for each kind of surface (table, floor, wall, ceiling, frame,
//! other), a behavior, a color index, an audio band, a strength and how the
//! edge bars draw its edges ([`Look`]). Applying one
//! (`lanes::RoomLanes::apply_theme`) sets every kind default and every
//! anchor of the room to its kind's look, hand assignments included: the
//! theme wins. Its name is saved with the room (`room_file.rs`); an edit by
//! hand afterwards stays, and the hand menu's Theme control then shows the
//! theme as edited ("Drift*", [`Shown`]).
//!
//! **Edges** (board #5431). Each look names an edge mode
//! (`edge_bars::Mode`: off, on the face, standing, in), and the edge bars
//! draw each shape in its kind's ([`edge_mode`]) unless the Edges control
//! or `debug.fosfora.edgebars` forces one on every kind. The mode comes from
//! the saved theme's name, edited or not, so nothing new is written to the
//! room file; a room with no theme has every kind on the face.
//!
//! Plain data, so it builds and tests on the desktop: the table, lookup by
//! name, the cycle, what a room file shows, and the launch rule of the
//! `debug.fosfora.theme` knob ([`Knob`]). Each is built from the catalogue
//! (`SurfaceBehavior::catalogue`), every pairing on its kind's own list.
//!
//! **Three more rows** (board #5543). A theme also says what the depth
//! light draws on what the depth map sees ([`Depth`]: a look, the sides'
//! and the tops' colors, a band, and whether it hides the surfaces' light
//! behind it), what the air carries ([`Air`]: the fireflies' pool as
//! stars, ash, spray or plankton, by a color, a band, a count and a
//! motion), and how much of the sky a floor reflects
//! ([`Theme::reflection`]). None of the three is saved: like the edges
//! they follow the saved theme's name.
//!
//! The seven, each one scene, as the owner's theme file has them
//! (`themes_new.csv`, Nov 18). A row whose effect is not built yet keeps
//! the look it had, and a depth look that is not built draws as the
//! points ([`DepthLook::built`]).
//!
//! - **Classic**, the hi-fi: the built-in defaults
//!   ([`SurfaceBehavior::default_for`]), each kind's own color, the
//!   behavior's band, full strength, so applying it gives exactly a room
//!   with no file. Only Classic runs the wall spectrum. The tables' edge
//!   bars stand on their rims as level meters. A fresh room on the headset
//!   starts on Aurora instead (board #5447,
//!   `lanes::RoomLanes::with_fresh_theme`). The file also moves its tables
//!   and other surfaces to the mids, which waits: a band that is not the
//!   behavior's own would end "a room with no file runs Classic".
//! - **Drift**, deep underwater: blue water on the walls (streamlines,
//!   currents) and the ceiling (rings, the surface from below), teal on
//!   the rest: curls round the tables and other things, sand ripples on
//!   the floor (the contours at 0.3), shafts of light in the frames (the
//!   pulse at 0.6). No bars but the frames'.
//! - **Tide**, a shore at dusk: a violet sky breathing with the bar on
//!   the walls and the ceiling (the pulse, the ceiling's at 0.6), rose on
//!   what stands in it: rings on the tables, tide pools, and curls on
//!   other things. The floor waits for the wash and keeps the violet curls.
//! - **Aurora** (board #5436), a night city under the lights: walls and
//!   the ceiling the desktop's Aurora, drawn as one sky around the room
//!   (`sky.rs`), the floor its reflection at 0.3 of what it was, all three
//!   in the key's tint and their edges off, so nothing cuts the sky at a
//!   corner. Under it the city: tables, frames and other surfaces run the
//!   windows (`surface_fx.rs`), warm white on a table, a low building,
//!   amber on other things, tall ones, and on a frame at 0.6, a far-off
//!   building's face. The mids light them and the highs flicker single
//!   ones. Their edges are off.
//! - **Mirror**, a club of infinity mirrors: tables and other surfaces run
//!   no behavior and their edges go in, each face a tunnel of light. The
//!   floor is tessera's lit tiles in one color, with the bars on its
//!   face. The ceiling waits for astrolabe and the frames for fenestra,
//!   held back for flashing, and keep their tunnels. The walls keep
//!   theirs too: the file gives them bezel, chrome round each tunnel, but
//!   in unworn captures on the headset the two together showed no chrome
//!   and a fainter tunnel.
//! - **Flint** (board #5533), a forge in a cave: the walls and the
//!   ceiling are shards, cells whose edges jump with the kick, drawn in
//!   one color ([`Look::mono`]), warm white, so they read as glowing
//!   fissures in stone and not the effect's own stained glass. The
//!   ceiling's are dim, 0.4, under the strength a ceiling's shards are
//!   capped to while the flash limit is on
//!   (`surfaces::CEILING_SHARDS_LIMITED`). The floor is tessera's
//!   flagstones in cold blue at 0.5, and the frames pulse amber on the
//!   bass, firelight through the cave's mouth. Tables are anvils, sparks
//!   struck on the kick with the bars on their face, and other surfaces
//!   banked coals, the embers on the mids, both amber on a heat ramp
//!   (`surface_fx.rs`).
//! - **Contour** (board #5533), a meadow drawn as a map: the floor and
//!   the walls run the contours (`surface_fx.rs`), one landscape's lines
//!   round the room, green on the low land and warm white on the high,
//!   their edge bars off as Aurora's are. The ceiling is streamlines at
//!   0.4, wind and thin cloud, and the frames pulse with the loudness, a
//!   sunrise, both warm white. Tables and other surfaces are hills in the
//!   same land: the terraces (`surface_fx.rs`), green, on the mids.
//!
//! Some pairings depart from the room editor's lists
//! (`SurfaceBehavior::catalogue`): Tide's tables run the rings, and the
//! windows and the terraces are on no kind's list, a theme's alone
//! (`SurfaceBehavior::theme_only`). Embers and sparks are on their kinds'
//! lists but not offered to the wearer; a theme gives them all the same.

use crate::edge_bars::Mode;
use crate::room_file::RoomFile;
use crate::surface_fx::{BAND_BASS, BAND_HIGH, BAND_MID, BAND_RMS};
use crate::surfaces::{
    COLOR_KEY, KIND_CEILING, KIND_FLOOR, KIND_FRAME, KIND_NAMES, KIND_TABLE, KIND_WALL,
    SurfaceBehavior,
};

/// Color indices (`surfaces::SURFACE_PALETTE`, from 1) the themes use.
const BLUE: u32 = 1;
const VIOLET: u32 = 2;
const WARM_WHITE: u32 = 3;
const AMBER: u32 = 4;
const GREEN: u32 = 5;
const TEAL: u32 = 6;
const ROSE: u32 = 7;

/// What every surface of one kind runs under a theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub behavior: SurfaceBehavior,
    /// The color index (`surfaces::surface_color`): 0 the kind's own, 1 to
    /// 7 the surface palette, [`COLOR_KEY`] the key's tint.
    pub color: u32,
    /// The audio band (`surface_fx::BAND_*`).
    pub band: u32,
    /// 0..1.
    pub strength: f32,
    /// How the edge bars draw this kind's edges (board #5431). Not saved:
    /// it follows the saved theme's name.
    pub edges: Mode,
    /// A ported effect drawn without its own hues, so the look's color
    /// alone tints it (board #5533, `surface_port::Port::mono`). Not
    /// saved: like the edges it follows the saved theme's name
    /// ([`mono`]), so a look picked by hand keeps its own colors.
    pub mono: bool,
}

impl Look {
    const fn new(
        behavior: SurfaceBehavior,
        color: u32,
        band: u32,
        strength: f32,
        edges: Mode,
    ) -> Self {
        Self {
            behavior,
            color,
            band,
            strength,
            edges,
            mono: false,
        }
    }

    /// This look in one color ([`Self::mono`]).
    const fn in_one_color(mut self) -> Self {
        self.mono = true;
        self
    }

    /// The lane's two spare values for this look as the room file keeps
    /// them (`room_file::AnchorEntry::params`): `None` when they are the
    /// behavior's defaults (the kind's own color and the behavior's band),
    /// so a look that keeps them writes nothing a fresh room lacks, and the
    /// band then follows a later change of behavior as it does in a fresh
    /// room.
    pub fn params(self) -> Option<[f32; 2]> {
        (self.color != 0 || self.band != self.behavior.default_band())
            .then(|| self.resolved_params())
    }

    /// The color index and band a surface running this look resolves to,
    /// as the room file's `resolve` reports them.
    pub fn resolved_params(self) -> [f32; 2] {
        [self.color as f32, self.band as f32]
    }

    /// Whether a surface resolving to `(behavior, strength, params)`
    /// (`RoomFile::resolve`) runs this look.
    pub fn runs(self, (behavior, strength, params): (SurfaceBehavior, f32, [f32; 2])) -> bool {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        let want = self.resolved_params();
        behavior == self.behavior
            && close(strength, self.strength)
            && close(params[0], want[0])
            && close(params[1], want[1])
    }
}

/// The themes, in the control's cycle order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Classic,
    Drift,
    Tide,
    Aurora,
    Mirror,
    Flint,
    Contour,
}

impl Theme {
    /// Every theme, in the cycle's order.
    pub const ALL: [Self; 7] = [
        Self::Classic,
        Self::Drift,
        Self::Tide,
        Self::Aurora,
        Self::Mirror,
        Self::Flint,
        Self::Contour,
    ];

    /// The name the room file, the knob and the log use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Drift => "drift",
            Self::Tide => "tide",
            Self::Aurora => "aurora",
            Self::Mirror => "mirror",
            Self::Flint => "flint",
            Self::Contour => "contour",
        }
    }

    /// The name the hand menu and the voice's replies show.
    pub fn title(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Drift => "Drift",
            Self::Tide => "Tide",
            Self::Aurora => "Aurora",
            Self::Mirror => "Mirror",
            Self::Flint => "Flint",
            Self::Contour => "Contour",
        }
    }

    /// The theme called `name` (any case, spaces around it ignored).
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .into_iter()
            .find(|t| t.name().eq_ignore_ascii_case(name))
    }

    /// The next theme in the cycle ([`Self::ALL`]'s order), wrapping from
    /// the last to Classic.
    #[must_use]
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// The theme before this one in the cycle, wrapping from Classic to
    /// the last.
    #[must_use]
    pub fn prev(self) -> Self {
        let n = Self::ALL.len();
        let i = Self::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Self::ALL[(i + n - 1) % n]
    }

    /// What every surface of `kind` runs under this theme; for a kind
    /// without a name (an unlabeled anchor) nothing: a theme leaves those
    /// as they are, as All: none does.
    pub fn look(self, kind: u32) -> Option<Look> {
        use SurfaceBehavior as B;
        KIND_NAMES.iter().find(|(k, _)| *k == kind)?;
        let look = Look::new;
        let face = Mode::Face;
        Some(match self {
            // The hi-fi. The tables' bars stand on their rims as level
            // meters (board #5543).
            Self::Classic => {
                let behavior = B::default_for(kind);
                let edges = if kind == KIND_TABLE {
                    Mode::Standing
                } else {
                    face
                };
                look(behavior, 0, behavior.default_band(), 1.0, edges)
            }
            // Board #5543: deep underwater. Blue water on the walls and
            // the ceiling, teal life on the rest; no bars but the frames'.
            Self::Drift => match kind {
                KIND_FLOOR => look(B::Contours, TEAL, BAND_BASS, 0.3, Mode::Off),
                KIND_WALL => look(B::Streamlines, BLUE, BAND_RMS, 1.0, Mode::Off),
                KIND_CEILING => look(B::Rings, BLUE, BAND_BASS, 1.0, Mode::Off),
                KIND_FRAME => look(B::Pulse, TEAL, BAND_RMS, 0.6, face),
                _ => look(B::Curls, TEAL, BAND_MID, 1.0, Mode::Off),
            },
            // Board #5543: a shore at dusk. The floor waits for the wash
            // and keeps the curls and its bars until then.
            Self::Tide => match kind {
                KIND_TABLE => look(B::Rings, ROSE, BAND_BASS, 1.0, Mode::Off),
                KIND_FLOOR => look(B::Curls, VIOLET, BAND_RMS, 1.0, face),
                KIND_WALL => look(B::Pulse, VIOLET, BAND_BASS, 1.0, Mode::Off),
                KIND_CEILING => look(B::Pulse, VIOLET, BAND_BASS, 0.6, Mode::Off),
                KIND_FRAME => look(B::Rings, ROSE, BAND_BASS, 1.0, face),
                _ => look(B::Curls, ROSE, BAND_MID, 1.0, Mode::Off),
            },
            // Board #5436: the sky on the walls and the ceiling, its
            // reflection on the floor, one tint over all three so the
            // sky has no seam of color at the ceiling's edge. Board
            // #5543: under it a city at night. Tables are low buildings
            // with warm white windows, other things tall ones in amber,
            // and a frame a far-off building's face.
            Self::Aurora => match kind {
                KIND_FLOOR | KIND_WALL | KIND_CEILING => {
                    look(B::Aurora, COLOR_KEY, BAND_RMS, 1.0, Mode::Off)
                }
                KIND_TABLE => look(B::Windows, WARM_WHITE, BAND_MID, 1.0, Mode::Off),
                KIND_FRAME => look(B::Windows, AMBER, BAND_MID, 0.6, Mode::Off),
                _ => look(B::Windows, AMBER, BAND_MID, 1.0, Mode::Off),
            },
            // Board #5543: a club of infinity mirrors. The ceiling waits
            // for astrolabe and the frames for fenestra, both held back
            // (`SurfaceBehavior::held_back`), and keep their tunnels. So
            // do the walls: with bezel on them the headset showed no
            // chrome and a fainter tunnel than the ceiling's (Nov 18).
            Self::Mirror => match kind {
                KIND_FLOOR => look(B::Tessera, BLUE, BAND_BASS, 1.0, face).in_one_color(),
                KIND_WALL => look(B::None, WARM_WHITE, BAND_RMS, 1.0, Mode::In),
                KIND_CEILING => look(B::None, WARM_WHITE, BAND_RMS, 1.0, Mode::In),
                KIND_FRAME => look(B::None, BLUE, BAND_RMS, 1.0, Mode::In),
                _ => look(B::None, BLUE, BAND_MID, 1.0, Mode::In),
            },
            // Boards #5533, #5543: a forge in a cave. Warm white fissures
            // on the walls and the ceiling, cold blue flagstones, amber
            // firelight in the frames. Tables are anvils, sparks struck
            // on the kick, and other surfaces banked coals.
            Self::Flint => match kind {
                KIND_TABLE => look(B::Sparks, AMBER, BAND_BASS, 1.0, face),
                KIND_FLOOR => look(B::Tessera, BLUE, BAND_BASS, 0.5, Mode::Off).in_one_color(),
                KIND_WALL => look(B::Shards, WARM_WHITE, BAND_RMS, 1.0, face).in_one_color(),
                KIND_CEILING => {
                    look(B::Shards, WARM_WHITE, BAND_RMS, 0.4, Mode::Off).in_one_color()
                }
                KIND_FRAME => look(B::Pulse, AMBER, BAND_BASS, 0.8, face),
                _ => look(B::Embers, AMBER, BAND_MID, 1.0, Mode::Off),
            },
            // Boards #5533, #5543: a meadow drawn as a map. One landscape
            // on the floor and the walls, its edges off so nothing cuts it
            // at a corner, wind on the ceiling and a sunrise in the
            // frames. Tables and other surfaces are hills in the same
            // land, terraced.
            Self::Contour => match kind {
                KIND_FLOOR | KIND_WALL => look(B::Contours, GREEN, BAND_RMS, 1.0, Mode::Off),
                KIND_CEILING => look(B::Streamlines, WARM_WHITE, BAND_RMS, 0.4, Mode::Off),
                KIND_FRAME => look(B::Pulse, WARM_WHITE, BAND_RMS, 0.6, face),
                _ => look(B::Terraces, GREEN, BAND_MID, 1.0, Mode::Off),
            },
        })
    }

    /// How the edge bars draw the edges of a surface of `kind` under this
    /// theme (board #5431); on the face for a kind without a name.
    pub fn edges(self, kind: u32) -> Mode {
        self.look(kind).map_or(Mode::Face, |l| l.edges)
    }

    /// The assignments the theme makes: each named kind
    /// ([`KIND_NAMES`]' order) with its look.
    pub fn looks(self) -> impl Iterator<Item = (u32, Look)> {
        KIND_NAMES
            .into_iter()
            .filter_map(move |(kind, _)| self.look(kind).map(|l| (kind, l)))
    }

    /// The looks in one line for the log, kind by kind: "table
    /// streamlines@1.00 teal rms · floor rings@1.00 blue bass · …", and the
    /// edge mode where it is not on the face: "wall none@1.00 warm white
    /// rms edges in".
    pub fn summary(self) -> String {
        self.looks()
            .map(|(kind, l)| {
                let edges = if l.edges == Mode::Face {
                    String::new()
                } else {
                    format!(" edges {}", l.edges.name())
                };
                format!(
                    "{} {}@{:.2} {} {}{edges}",
                    crate::surfaces::kind_name(kind),
                    l.behavior.name(),
                    l.strength,
                    crate::surfaces::color_name(l.color),
                    crate::surface_fx::band_name(l.band),
                )
            })
            .collect::<Vec<_>>()
            .join(" \u{b7} ")
    }

    /// Whether `file` runs this theme: every named kind's default, and
    /// every entry of a named kind, resolves to the kind's look (an
    /// unlabeled anchor's entry is the theme's to leave alone).
    pub fn runs_in(self, file: &RoomFile) -> bool {
        self.looks()
            .all(|(kind, look)| look.runs(file.resolve_kind(kind)))
            && file.anchors.iter().all(|e| {
                self.look(e.kind)
                    .is_none_or(|look| look.runs(file.resolve(&e.uuid, e.kind)))
            })
    }
}

/// How the depth light draws what the depth map sees (board #5543,
/// `depth_light.rs`). Every look stays on the thing's surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthLook {
    /// Points of light, the first look.
    Points,
    /// Tightly packed streaks running up the thing, launched on the kick.
    Beams,
    /// Points on a heat ramp that smolder with the band and flare on the
    /// kick.
    Coals,
    /// Short blades that sway with the band and gust with the loudness.
    Grass,
    /// Points let go with the band that slide up the thing and wobble.
    Bubbles,
    /// Dim points that flare briefly.
    Sparkle,
    /// Specks that sweep over the points together, round the room's
    /// center.
    Glints,
    /// Points drawn as level meters, each on one band.
    Meters,
}

impl DepthLook {
    /// Every look.
    pub const ALL: [Self; 8] = [
        Self::Points,
        Self::Beams,
        Self::Coals,
        Self::Grass,
        Self::Bubbles,
        Self::Sparkle,
        Self::Glints,
        Self::Meters,
    ];

    /// The name the log uses.
    pub fn name(self) -> &'static str {
        match self {
            Self::Points => "points",
            Self::Beams => "beams",
            Self::Coals => "coals",
            Self::Grass => "grass",
            Self::Bubbles => "bubbles",
            Self::Sparkle => "sparkle",
            Self::Glints => "glints",
            Self::Meters => "meters",
        }
    }

    /// Whether the depth light draws this look yet. One that is not built
    /// draws as the points, in the theme's colors and on its band.
    pub fn built(self) -> bool {
        matches!(self, Self::Points | Self::Beams | Self::Coals | Self::Grass)
    }
}

/// What a theme gives the depth light (board #5543).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Depth {
    pub look: DepthLook,
    /// The color indices (as [`Look::color`]) of the sides and of the
    /// tops.
    pub colors: [u32; 2],
    /// The audio band (`surface_fx::BAND_*`).
    pub band: u32,
    /// Whether what the depth map sees hides the surfaces' light behind it
    /// at every immersion level, as a black base under its own light. With
    /// it off that light shows between the points once the room is dark
    /// (`depth_light::veil`).
    pub occlude: bool,
}

/// How the air's particles move (board #5543, `fireflies.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// The fireflies' own loops.
    Wander,
    /// Nearly still and high in the room, the light flickering with the
    /// band: stars.
    Twinkle,
    /// Up from the floor and fading, let go with the band: ash, spray.
    Rise,
    /// Carried slowly one shared way: plankton.
    Drift,
    /// All round the room's center together: a mirror ball's specks.
    Orbit,
}

impl Motion {
    /// Every motion.
    pub const ALL: [Self; 5] = [
        Self::Wander,
        Self::Twinkle,
        Self::Rise,
        Self::Drift,
        Self::Orbit,
    ];

    /// The name the log uses.
    pub fn name(self) -> &'static str {
        match self {
            Self::Wander => "wander",
            Self::Twinkle => "twinkle",
            Self::Rise => "rise",
            Self::Drift => "drift",
            Self::Orbit => "orbit",
        }
    }
}

/// The color a theme gives the air's particles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirColor {
    /// A color index of the surface palette, from 1.
    Index(u32),
    /// Each particle the color of the band it follows.
    ByBand,
}

/// What a theme gives the air (board #5543): the fireflies' pool and
/// sprites as something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Air {
    pub color: AirColor,
    /// The audio band (`surface_fx::BAND_*`).
    pub band: u32,
    /// How many, under `fireflies::FIREFLY_CAP`.
    pub count: u32,
    pub motion: Motion,
}

impl Theme {
    /// What the depth light draws under this theme.
    pub fn depth(self) -> Depth {
        let depth = |look, colors, band| Depth {
            look,
            colors,
            band,
            occlude: true,
        };
        match self {
            // By band once the meters are built; other's own until then.
            Self::Classic => depth(DepthLook::Meters, [0, 0], BAND_RMS),
            Self::Drift => depth(DepthLook::Bubbles, [TEAL, WARM_WHITE], BAND_BASS),
            Self::Tide => depth(DepthLook::Sparkle, [ROSE, ROSE], BAND_HIGH),
            Self::Aurora => depth(DepthLook::Beams, [TEAL, COLOR_KEY], BAND_BASS),
            Self::Mirror => depth(DepthLook::Glints, [WARM_WHITE, WARM_WHITE], BAND_HIGH),
            // The heat ramp once the coals are built.
            Self::Flint => depth(DepthLook::Coals, [AMBER, WARM_WHITE], BAND_BASS),
            Self::Contour => depth(DepthLook::Grass, [GREEN, WARM_WHITE], BAND_MID),
        }
    }

    /// What the air carries under this theme.
    pub fn air(self) -> Air {
        let air = |color, band, count, motion| Air {
            color,
            band,
            count,
            motion,
        };
        let index = AirColor::Index;
        match self {
            Self::Classic => air(AirColor::ByBand, BAND_HIGH, 2000, Motion::Wander),
            Self::Drift => air(index(TEAL), BAND_HIGH, 1500, Motion::Drift),
            Self::Tide => air(index(ROSE), BAND_BASS, 1000, Motion::Rise),
            Self::Aurora => air(index(WARM_WHITE), BAND_HIGH, 2000, Motion::Twinkle),
            Self::Mirror => air(index(WARM_WHITE), BAND_HIGH, 1500, Motion::Orbit),
            Self::Flint => air(index(AMBER), BAND_BASS, 1500, Motion::Rise),
            Self::Contour => air(index(GREEN), BAND_HIGH, 2000, Motion::Wander),
        }
    }

    /// The share of the sky's reflection a floor shows under this theme
    /// (`sky::REFLECT_GAIN` is 1): Aurora's is dimmed, so the floor reads
    /// as dark water or ice. No other theme has a sky.
    pub fn reflection(self) -> f32 {
        match self {
            Self::Aurora => 0.3,
            _ => 1.0,
        }
    }
}

/// The share of the sky's reflection a floor shows in a room whose saved
/// theme is `theme` ([`Theme::reflection`]); all of it with none.
pub fn reflection(theme: Option<Theme>) -> f32 {
    theme.map_or(1.0, Theme::reflection)
}

/// How the edge bars draw the edges of a surface of `kind` in a room whose
/// saved theme is `theme` (board #5431): the theme's mode for the kind,
/// edited since or not; on the face with no theme (custom, or the built-in
/// defaults without a room). A fresh room runs Aurora before anything is
/// saved (board #5447), so it has one.
pub fn edge_mode(theme: Option<Theme>, kind: u32) -> Mode {
    theme.map_or(Mode::Face, |t| t.edges(kind))
}

/// Whether a surface of `kind` running `behavior` draws it in one color
/// (board #5533, [`Look::mono`]) in a room whose saved theme is `theme`:
/// the theme asks it of the kind, and the surface still runs the theme's
/// look for the kind. A surface edited to another look, a room with no
/// theme and every other theme draw each look in its own colors.
pub fn mono(theme: Option<Theme>, kind: u32, behavior: SurfaceBehavior) -> bool {
    theme
        .and_then(|t| t.look(kind))
        .is_some_and(|l| l.mono && l.behavior == behavior)
}

/// What the hand menu's Theme control shows for a room file ([`Self::of`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// The room runs this theme: the one saved with it, or Classic for a
    /// file with none that runs the built-in defaults. On the headset such
    /// a room is fresh and runs Aurora first (board #5447,
    /// `lanes::RoomLanes::with_fresh_theme`), so this shows Classic only
    /// without a room or a fresh theme.
    Theme(Theme),
    /// This theme was applied and something was edited by hand since: the
    /// edit stays, and so does the saved name.
    Edited(Theme),
    /// No theme saved and the room is not Classic: assigned by hand before
    /// themes existed.
    Custom,
}

impl Shown {
    /// The control's state for `file`: the saved theme, edited when the
    /// room no longer runs it; with none saved, Classic when the room runs
    /// Classic, else custom. Worked out from the file each time, so an
    /// edit marks the theme edited, an edit undone unmarks it, and a reload
    /// shows the same.
    pub fn of(file: &RoomFile) -> Self {
        match file.theme {
            Some(t) if t.runs_in(file) => Self::Theme(t),
            Some(t) => Self::Edited(t),
            None if Theme::Classic.runs_in(file) => Self::Theme(Theme::Classic),
            None => Self::Custom,
        }
    }

    /// The theme it names, if any.
    pub fn theme(self) -> Option<Theme> {
        match self {
            Self::Theme(t) | Self::Edited(t) => Some(t),
            Self::Custom => None,
        }
    }

    /// The theme a press of the control applies: the next in the cycle
    /// after the one it names (edited or not), Classic from custom, the
    /// start of the cycle.
    pub fn next(self) -> Theme {
        self.theme().map_or(Theme::Classic, Theme::next)
    }

    /// The theme a thumb swipe left applies: the one before the one it
    /// names (edited or not), the cycle's last from custom.
    pub fn prev(self) -> Theme {
        self.theme()
            .map_or_else(|| Theme::Classic.prev(), Theme::prev)
    }

    /// The theme one step along the cycle from the one it names: forward
    /// ([`Self::next`]) for a `step` over 0, back ([`Self::prev`]) otherwise.
    pub fn step(self, step: i32) -> Theme {
        if step > 0 { self.next() } else { self.prev() }
    }

    /// The control's value: "Drift", "Drift*" when edited, "custom".
    pub fn value(self) -> String {
        match self {
            Self::Theme(t) => t.title().to_owned(),
            Self::Edited(t) => format!("{}*", t.title()),
            Self::Custom => "custom".to_owned(),
        }
    }

    /// The control's words in one line: "Theme: Drift*".
    pub fn label(self) -> String {
        format!("Theme: {}", self.value())
    }
}

/// The `debug.fosfora.theme` knob (board #5415), polled about once a
/// second: a theme's name applies that theme when the value changes, once,
/// not at every poll. At launch (the first poll with the room's file
/// loaded) a value naming the theme the room shows applies nothing, so a
/// value left set from an earlier run neither rewrites the room nor wipes
/// an edit. An app cannot clear a `debug.` property: to apply the same
/// theme again, set the knob to "" first.
#[derive(Debug, Clone, Default)]
pub struct Knob {
    /// The value last seen (`None`: unset).
    last: Option<String>,
    /// Whether the launch poll has happened.
    started: bool,
}

/// What one poll of the [`Knob`] asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KnobPoll {
    /// Nothing changed (or the room's anchors are not there yet).
    Nothing,
    /// The launch value names the theme the room shows: nothing applied.
    Kept(Theme),
    /// A new value naming this theme: apply it.
    Apply(Theme),
    /// A new value naming no theme: nothing applied.
    Unknown(String),
}

impl Knob {
    /// This poll's value (`None` or empty: unset). While the room is on
    /// but has no anchors yet (`waiting`) it waits for them, as the
    /// `surface` knob does; `shown` is what the room's file shows now.
    pub fn poll(&mut self, value: Option<&str>, waiting: bool, shown: Shown) -> KnobPoll {
        if waiting {
            return KnobPoll::Nothing;
        }
        let value = value.map(str::trim).filter(|v| !v.is_empty());
        let launch = !self.started;
        self.started = true;
        if !launch && value == self.last.as_deref() {
            return KnobPoll::Nothing;
        }
        self.last = value.map(str::to_owned);
        let Some(value) = value else {
            return KnobPoll::Nothing;
        };
        match Theme::from_name(value) {
            Some(t) if launch && shown.theme() == Some(t) => KnobPoll::Kept(t),
            Some(t) => KnobPoll::Apply(t),
            None => KnobPoll::Unknown(value.to_owned()),
        }
    }
}

/// How long the room's light takes to go dark before a track's theme is
/// laid, how long it stays dark, and how long the new look takes to come
/// up, with the music still and while it rises on a hit (s).
pub const BRING_OUT_S: f32 = 1.0;
pub const BRING_DARK_S: f32 = 0.25;
pub const BRING_IN_S: f32 = 1.5;
pub const BRING_IN_HIT_S: f32 = 0.3;

/// What the flash limit lets the room's light do this frame
/// (`flash::Limiter`): everything it limits moves one shared way at a
/// time, and the fade of a theme's change moves that way too, so it adds
/// no turn of its own to the second the flash rule counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// The direction has not turned for a second (still or quiet music, or
    /// the limiter off): the fade runs either way.
    Free,
    /// The lights may rise this frame.
    Rise,
    /// The lights may fall this frame.
    Fall,
}

impl Turn {
    /// This frame's, from the limiter once its frame has begun.
    pub fn of(limiter: &crate::flash::Limiter) -> Self {
        if limiter.turns_last_second() == 0 {
            return Self::Free;
        }
        match limiter.phase() {
            crate::flash::Phase::Rise => Self::Rise,
            crate::flash::Phase::Fall => Self::Fall,
        }
    }
}

/// What a change of the room's look at a track's start comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Brought {
    /// The theme a bundled track brings (`music::Bundled::theme`).
    Theme(Theme),
    /// The room's own look again: the wearer's music took over.
    Own,
}

/// A change that is due: what to lay, and the count of the wearer's own
/// picks when the track started ([`Bringing::start`]'s `picks`). A pick
/// made since then stands, and the change is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Due {
    pub what: Brought,
    pub picks: u64,
}

/// A bundled track brings its theme (board #5533): the change of the
/// room's look when a track starts, as a fade through dark instead of a
/// cut. The room's light falls to nothing from the track's first frame,
/// the theme is laid while it is dark, and after [`BRING_DARK_S`] the new
/// look rises. So the song that ends keeps its look to its last note, and
/// the wearer sees the room go dark as the new song begins and come back
/// changed. A room that already shows the theme is laid at once with no
/// fade: nothing changes to the eye.
///
/// The fade keeps to the flash limit's direction ([`Turn`]). It falls only
/// in frames where the lights may fall, over [`BRING_OUT_S`] of them, and
/// waits through a rise. It rises only where they may rise: with a hit, so
/// the new look comes up on the first beats, [`BRING_IN_HIT_S`] of rising
/// frames in all; with the music still, evenly over [`BRING_IN_S`]. A
/// fade that ran on its own clock put one more change of light in a second
/// that the music had already filled: Aurora's red curtains on a 4 Hz
/// pulse went from 3 flashes to 3.5 while it rose.
///
/// Plain numbers; `app.rs` starts it at a track's start, steps it once a
/// frame, multiplies every lit surface's light by [`Self::veil`] and lays
/// what [`Self::step`] hands back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bringing {
    /// The light's share, 0 to 1.
    veil: f32,
    phase: BringPhase,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum BringPhase {
    Idle,
    /// Laid on the next step with the light as it is: the room shows the
    /// look already.
    Now(Due),
    /// Going dark, to lay this.
    Out(Due),
    /// Dark for this many seconds more.
    Dark(f32),
    /// Coming up.
    In,
}

impl Default for Bringing {
    fn default() -> Self {
        Self {
            veil: 1.0,
            phase: BringPhase::Idle,
        }
    }
}

impl Bringing {
    /// A track started and the room's look goes to `what`. `shows`: the
    /// room shows that look already, so with no change under way it is
    /// laid on the next step with no fade. `picks`: the count of the
    /// wearer's own picks now. A start while another change runs takes
    /// over from where the light is.
    pub fn start(&mut self, what: Brought, shows: bool, picks: u64) {
        let due = Due { what, picks };
        self.phase = if shows && self.phase == BringPhase::Idle {
            BringPhase::Now(due)
        } else {
            BringPhase::Out(due)
        };
    }

    /// `dt` seconds on, the lights free to move as `turn` says: the look to
    /// lay now, if one is due.
    pub fn step(&mut self, dt: f32, turn: Turn) -> Option<Due> {
        let dt = dt.max(0.0);
        match self.phase {
            BringPhase::Idle => None,
            BringPhase::Now(due) => {
                self.phase = BringPhase::Idle;
                Some(due)
            }
            BringPhase::Out(due) => {
                if turn != Turn::Rise {
                    self.veil = (self.veil - dt / BRING_OUT_S).max(0.0);
                }
                if self.veil > 0.0 {
                    return None;
                }
                self.phase = BringPhase::Dark(BRING_DARK_S);
                Some(due)
            }
            BringPhase::Dark(left) => {
                let left = left - dt;
                self.phase = if left > 0.0 {
                    BringPhase::Dark(left)
                } else {
                    BringPhase::In
                };
                None
            }
            BringPhase::In => {
                let secs = match turn {
                    Turn::Free => BRING_IN_S,
                    Turn::Rise => BRING_IN_HIT_S,
                    Turn::Fall => f32::INFINITY,
                };
                self.veil = (self.veil + dt / secs).min(1.0);
                if self.veil >= 1.0 {
                    self.phase = BringPhase::Idle;
                }
                None
            }
        }
    }

    /// The share of its light every lit surface draws at now, 0 to 1.
    pub fn veil(&self) -> f32 {
        self.veil
    }

    /// Whether a change is under way.
    pub fn busy(&self) -> bool {
        self.phase != BringPhase::Idle
    }
}

/// A lit face's uniform rows (`surface_fx::rows` or `surface_port::rows`,
/// after `immerse::lift_rows`) with its peak alpha at `veil` of what it
/// was ([`Bringing::veil`]).
pub fn veil_rows<const N: usize>(mut rows: [[f32; 4]; N], veil: f32) -> [[f32; 4]; N] {
    const { assert!(N > crate::immerse::PEAK_ROW) };
    rows[crate::immerse::PEAK_ROW][3] *= veil.clamp(0.0, 1.0);
    rows
}

/// The themes' names for a reply or a warning, in the cycle's order:
/// "classic, drift, … or contour" ([`Theme::name`]) or with `titles`
/// "Classic, Drift, … or Contour".
pub fn listed(titles: bool) -> String {
    let names: Vec<&str> = Theme::ALL
        .iter()
        .map(|t| if titles { t.title() } else { t.name() })
        .collect();
    match names.split_last() {
        Some((last, head)) if !head.is_empty() => format!("{} or {last}", head.join(", ")),
        Some((last, _)) => (*last).to_owned(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface_fx::BANDS;
    use crate::surfaces::{KIND_NONE, kind_name};
    use SurfaceBehavior as B;

    #[test]
    fn every_look_is_on_its_kinds_list_with_a_valid_color_and_band() {
        // The departures from the catalogue (`SurfaceBehavior::catalogue`):
        // Tide's tables run the rings, tide pools, which the room editor's
        // list for a table does not carry, and the windows and the
        // terraces are on no kind's list (`SurfaceBehavior::theme_only`).
        for theme in Theme::ALL {
            assert_eq!(theme.looks().count(), KIND_NAMES.len(), "{theme:?}");
            for (kind, look) in theme.looks() {
                let what = format!("{} {}", theme.name(), kind_name(kind));
                let departs = (theme, kind, look.behavior) == (Theme::Tide, KIND_TABLE, B::Rings)
                    || look.behavior.theme_only();
                assert_eq!(
                    B::catalogue(kind).contains(&look.behavior),
                    !departs,
                    "{what}: {}",
                    look.behavior.name()
                );
                assert!(look.color <= COLOR_KEY, "{what}");
                assert!(look.band < BANDS, "{what}");
                assert!((0.0..=1.0).contains(&look.strength), "{what}");
            }
            // An unlabeled anchor is left alone.
            assert_eq!(theme.look(KIND_NONE), None);
        }
    }

    #[test]
    fn classic_is_the_kind_defaults() {
        for (kind, _) in KIND_NAMES {
            let look = Theme::Classic.look(kind).expect("a named kind");
            let behavior = B::default_for(kind);
            // The tables' bars stand (board #5543); the rest on the face.
            let edges = if kind == KIND_TABLE {
                Mode::Standing
            } else {
                Mode::Face
            };
            assert_eq!(
                look,
                Look::new(behavior, 0, behavior.default_band(), 1.0, edges),
                "{}",
                kind_name(kind)
            );
            // Nothing a fresh room lacks: no params.
            assert_eq!(look.params(), None);
        }
        // The built-in defaults (a room file nobody wrote) run Classic.
        assert!(Theme::Classic.runs_in(&RoomFile::default()));
        for theme in Theme::ALL.into_iter().filter(|t| *t != Theme::Classic) {
            assert!(!theme.runs_in(&RoomFile::default()), "{theme:?}");
        }
    }

    /// One theme's rows in `KIND_NAMES`' order (table, floor, wall,
    /// ceiling, frame, other): the behavior, the color, the band, the
    /// strength, the edges and whether in one color.
    fn table(t: Theme) -> Vec<(B, u32, u32, f32, Mode, bool)> {
        t.looks()
            .map(|(_, l)| (l.behavior, l.color, l.band, l.strength, l.edges, l.mono))
            .collect()
    }

    #[test]
    fn the_seven_as_the_theme_file_has_them() {
        use crate::surface_fx::BAND_MID as MID;
        let (rms, bass) = (BAND_RMS, BAND_BASS);
        let (off, face, into) = (Mode::Off, Mode::Face, Mode::In);
        // Board #5543, themes_new.csv, a row per part. A row whose effect
        // is not built yet is as it was before.
        assert_eq!(
            table(Theme::Drift),
            [
                (B::Curls, TEAL, MID, 1.0, off, false),
                (B::Contours, TEAL, bass, 0.3, off, false),
                (B::Streamlines, BLUE, rms, 1.0, off, false),
                (B::Rings, BLUE, bass, 1.0, off, false),
                (B::Pulse, TEAL, rms, 0.6, face, false),
                (B::Curls, TEAL, MID, 1.0, off, false),
            ]
        );
        // The floor waits for the wash.
        assert_eq!(
            table(Theme::Tide),
            [
                (B::Rings, ROSE, bass, 1.0, off, false),
                (B::Curls, VIOLET, rms, 1.0, face, false),
                (B::Pulse, VIOLET, bass, 1.0, off, false),
                (B::Pulse, VIOLET, bass, 0.6, off, false),
                (B::Rings, ROSE, bass, 1.0, face, false),
                (B::Curls, ROSE, MID, 1.0, off, false),
            ]
        );
        // A city under the sky: windows on what stands in the room.
        let sky = (B::Aurora, COLOR_KEY, rms, 1.0, off, false);
        assert_eq!(
            table(Theme::Aurora),
            [
                (B::Windows, WARM_WHITE, MID, 1.0, off, false),
                sky,
                sky,
                sky,
                (B::Windows, AMBER, MID, 0.6, off, false),
                (B::Windows, AMBER, MID, 1.0, off, false),
            ]
        );
        // The ceiling waits for astrolabe, the frames for fenestra and
        // the walls for a bezel that draws with a tunnel.
        assert_eq!(
            table(Theme::Mirror),
            [
                (B::None, BLUE, MID, 1.0, into, false),
                (B::Tessera, BLUE, bass, 1.0, face, true),
                (B::None, WARM_WHITE, rms, 1.0, into, false),
                (B::None, WARM_WHITE, rms, 1.0, into, false),
                (B::None, BLUE, rms, 1.0, into, false),
                (B::None, BLUE, MID, 1.0, into, false),
            ]
        );
        // Sparks on the tables and embers on other surfaces, drawn on
        // the face. The ceiling's shards are under their cap for the
        // flash rule.
        assert_eq!(
            table(Theme::Flint),
            [
                (B::Sparks, AMBER, bass, 1.0, face, false),
                (B::Tessera, BLUE, bass, 0.5, off, true),
                (B::Shards, WARM_WHITE, rms, 1.0, face, true),
                (B::Shards, WARM_WHITE, rms, 0.4, off, true),
                (B::Pulse, AMBER, bass, 0.8, face, false),
                (B::Embers, AMBER, MID, 1.0, off, false),
            ]
        );
        let ceiling = Theme::Flint.look(KIND_CEILING).expect("ceiling");
        assert!(ceiling.strength <= crate::surfaces::CEILING_SHARDS_LIMITED);
        // Tables and other surfaces are terraced hills in the land.
        assert_eq!(
            table(Theme::Contour),
            [
                (B::Terraces, GREEN, MID, 1.0, off, false),
                (B::Contours, GREEN, rms, 1.0, off, false),
                (B::Contours, GREEN, rms, 1.0, off, false),
                (B::Streamlines, WARM_WHITE, rms, 0.4, off, false),
                (B::Pulse, WARM_WHITE, rms, 0.6, face, false),
                (B::Terraces, GREEN, MID, 1.0, off, false),
            ]
        );
        // Only Classic runs the wall spectrum, and no theme a look that
        // draws nothing: none is held back, and each is none, the
        // spectrum or a look the surfaces pass draws. Embers, sparks,
        // windows and terraces are a theme's to give though the wearer's
        // lists leave them out (`SurfaceBehavior::offered`).
        for theme in Theme::ALL {
            let spectrum = theme.looks().any(|(_, l)| l.behavior == B::Spectrum);
            assert_eq!(spectrum, theme == Theme::Classic, "{theme:?}");
            for (_, l) in theme.looks() {
                let b = l.behavior;
                assert!(!b.held_back(), "{theme:?} {b:?}");
                assert!(
                    b.draws_on_face() || matches!(b, B::None | B::Spectrum),
                    "{theme:?} {b:?}"
                );
            }
        }
        // One color follows the room's theme and the look the theme gave
        // the kind: a surface edited to another look, another theme and a
        // room with none draw every look in its own colors.
        let flint = Some(Theme::Flint);
        assert!(mono(flint, KIND_WALL, B::Shards));
        assert!(mono(flint, KIND_CEILING, B::Shards));
        assert!(mono(flint, KIND_FLOOR, B::Tessera));
        assert!(!mono(flint, KIND_TABLE, B::Shards));
        assert!(!mono(flint, KIND_WALL, B::Bezel));
        assert!(!mono(flint, KIND_FLOOR, B::Shards));
        assert!(!mono(flint, crate::surfaces::KIND_NONE, B::Shards));
        assert!(!mono(Some(Theme::Classic), KIND_TABLE, B::Shards));
        assert!(!mono(None, KIND_TABLE, B::Shards));
    }

    #[test]
    fn every_theme_has_a_depth_light_and_an_air_of_its_own() {
        use crate::surface_fx::{BAND_HIGH as HIGH, BAND_MID as MID};
        // Board #5543, themes_new.csv: the depth-detected row. The look,
        // the sides' and the tops' colors, the band; every one occludes.
        let depth = |t: Theme| {
            let d = t.depth();
            assert!(d.occlude, "{t:?}");
            assert!(d.colors.iter().all(|c| *c <= COLOR_KEY), "{t:?}");
            (d.look, d.colors, d.band)
        };
        use DepthLook as D;
        assert_eq!(depth(Theme::Classic), (D::Meters, [0, 0], BAND_RMS));
        assert_eq!(
            depth(Theme::Drift),
            (D::Bubbles, [TEAL, WARM_WHITE], BAND_BASS)
        );
        assert_eq!(depth(Theme::Tide), (D::Sparkle, [ROSE, ROSE], HIGH));
        assert_eq!(
            depth(Theme::Aurora),
            (D::Beams, [TEAL, COLOR_KEY], BAND_BASS)
        );
        assert_eq!(
            depth(Theme::Mirror),
            (D::Glints, [WARM_WHITE, WARM_WHITE], HIGH)
        );
        assert_eq!(
            depth(Theme::Flint),
            (D::Coals, [AMBER, WARM_WHITE], BAND_BASS)
        );
        assert_eq!(depth(Theme::Contour), (D::Grass, [GREEN, WARM_WHITE], MID));
        // The points and the three looks of the video's themes are
        // built: every other look draws as the points.
        for look in DepthLook::ALL {
            let built = [D::Points, D::Beams, D::Coals, D::Grass].contains(&look);
            assert_eq!(look.built(), built, "{}", look.name());
        }
        // The air row: the color, the band, the count and the motion.
        let air = |t: Theme| {
            let a = t.air();
            assert!(a.count <= crate::fireflies::FIREFLY_CAP, "{t:?}");
            (a.color, a.band, a.count, a.motion)
        };
        let index = AirColor::Index;
        use Motion as M;
        assert_eq!(
            air(Theme::Classic),
            (AirColor::ByBand, HIGH, 2000, M::Wander)
        );
        assert_eq!(air(Theme::Drift), (index(TEAL), HIGH, 1500, M::Drift));
        assert_eq!(air(Theme::Tide), (index(ROSE), BAND_BASS, 1000, M::Rise));
        assert_eq!(
            air(Theme::Aurora),
            (index(WARM_WHITE), HIGH, 2000, M::Twinkle)
        );
        assert_eq!(
            air(Theme::Mirror),
            (index(WARM_WHITE), HIGH, 1500, M::Orbit)
        );
        assert_eq!(air(Theme::Flint), (index(AMBER), BAND_BASS, 1500, M::Rise));
        assert_eq!(air(Theme::Contour), (index(GREEN), HIGH, 2000, M::Wander));
        // The floor's reflection: Aurora's dimmed, all of it elsewhere and
        // with no theme.
        assert_close!(Theme::Aurora.reflection(), 0.3);
        assert_close!(reflection(Some(Theme::Aurora)), 0.3);
        for t in Theme::ALL.into_iter().filter(|t| *t != Theme::Aurora) {
            assert_close!(t.reflection(), 1.0);
        }
        assert_close!(reflection(None), 1.0);
    }

    #[test]
    fn names_and_the_cycle() {
        assert_eq!(
            Theme::ALL.map(Theme::name),
            [
                "classic", "drift", "tide", "aurora", "mirror", "flint", "contour"
            ]
        );
        for t in Theme::ALL {
            assert_eq!(Theme::from_name(t.name()), Some(t));
            assert_eq!(Theme::from_name(t.title()), Some(t));
            assert_eq!(
                Theme::from_name(&format!(" {} ", t.name().to_uppercase())),
                Some(t)
            );
        }
        assert_eq!(Theme::from_name("sunset"), None);
        assert_eq!(Theme::from_name(""), None);
        // Round and back.
        let mut t = Theme::Classic;
        let mut seen = Vec::new();
        for _ in 0..Theme::ALL.len() {
            t = t.next();
            seen.push(t);
        }
        assert_eq!(
            seen,
            [
                Theme::Drift,
                Theme::Tide,
                Theme::Aurora,
                Theme::Mirror,
                Theme::Flint,
                Theme::Contour,
                Theme::Classic
            ]
        );
        // And back the other way, wrapping at Classic.
        let mut seen = Vec::new();
        for _ in 0..Theme::ALL.len() {
            t = t.prev();
            seen.push(t);
        }
        assert_eq!(
            seen,
            [
                Theme::Contour,
                Theme::Flint,
                Theme::Mirror,
                Theme::Aurora,
                Theme::Tide,
                Theme::Drift,
                Theme::Classic
            ]
        );
        for t in Theme::ALL {
            assert_eq!(t.next().prev(), t);
            assert_eq!(t.prev().next(), t);
        }
        // A thumb swipe: right is the control's press, left the theme
        // before, each from the one shown, edited or not. Custom steps to
        // the cycle's start or its end.
        assert_eq!(Shown::Theme(Theme::Drift).step(1), Theme::Tide);
        assert_eq!(Shown::Theme(Theme::Drift).step(-1), Theme::Classic);
        assert_eq!(Shown::Edited(Theme::Aurora).step(-1), Theme::Tide);
        let last = Theme::ALL[Theme::ALL.len() - 1];
        assert_eq!(last, Theme::Contour);
        assert_eq!(Shown::Theme(Theme::Classic).prev(), last);
        assert_eq!(Shown::Theme(last).step(1), Theme::Classic);
        assert_eq!(Shown::Custom.step(1), Theme::Classic);
        assert_eq!(Shown::Custom.step(-1), last);
        // The control's press: the next after the one shown, edited or
        // not; custom starts the cycle. Mirror comes after Aurora, Flint
        // after Mirror and Contour last.
        assert_eq!(Shown::Theme(Theme::Drift).next(), Theme::Tide);
        assert_eq!(Shown::Edited(Theme::Aurora).next(), Theme::Mirror);
        assert_eq!(Shown::Theme(Theme::Mirror).next(), Theme::Flint);
        assert_eq!(Shown::Theme(Theme::Flint).next(), Theme::Contour);
        assert_eq!(Shown::Theme(last).next(), Theme::Classic);
        assert_eq!(Shown::Custom.next(), Theme::Classic);
        // The words.
        assert_eq!(Shown::Theme(Theme::Drift).label(), "Theme: Drift");
        assert_eq!(Shown::Edited(Theme::Drift).label(), "Theme: Drift*");
        assert_eq!(Shown::Custom.label(), "Theme: custom");
        assert_eq!(Shown::Edited(Theme::Classic).value(), "Classic*");
        assert_eq!(
            listed(true),
            "Classic, Drift, Tide, Aurora, Mirror, Flint or Contour"
        );
        assert_eq!(
            listed(false),
            "classic, drift, tide, aurora, mirror, flint or contour"
        );
    }

    #[test]
    fn every_theme_names_an_edge_mode_per_kind() {
        for theme in Theme::ALL {
            for (kind, l) in theme.looks() {
                assert!(
                    Mode::ALL.contains(&l.edges),
                    "{theme:?} {}",
                    kind_name(kind)
                );
                assert_eq!(theme.edges(kind), l.edges);
            }
            // A kind without a name: on the face.
            assert_eq!(theme.edges(KIND_NONE), Mode::Face);
        }
        // The log names a mode only where it is not on the face.
        assert!(
            Theme::Aurora
                .summary()
                .contains("floor aurora@1.00 key rms edges off")
        );
        assert!(
            Theme::Aurora
                .summary()
                .contains("table windows@1.00 warm white mid edges off")
        );
        assert!(
            Theme::Flint
                .summary()
                .contains("table sparks@1.00 amber bass \u{b7}")
        );
        assert!(
            Theme::Contour
                .summary()
                .contains("wall contours@1.00 green rms edges off")
        );
        assert!(
            Theme::Contour
                .summary()
                .ends_with("other terraces@1.00 green mid edges off")
        );
        // The room's theme decides; with none saved (fresh or custom),
        // every kind on the face.
        for (kind, _) in KIND_NAMES {
            assert_eq!(edge_mode(None, kind), Mode::Face);
            assert_eq!(
                edge_mode(Some(Theme::Mirror), kind),
                Theme::Mirror.edges(kind)
            );
        }
        assert_eq!(edge_mode(Some(Theme::Mirror), KIND_WALL), Mode::In);
        assert!(
            Theme::Drift
                .summary()
                .contains("frame pulse@0.60 teal rms \u{b7}")
        );
        assert!(
            Theme::Mirror
                .summary()
                .contains("wall none@1.00 warm white rms edges in")
        );
        assert!(
            Theme::Mirror
                .summary()
                .contains("floor tessera@1.00 blue bass \u{b7}")
        );
    }

    #[test]
    fn a_look_runs_what_resolves_to_it() {
        // Drift's ceiling: the rings, blue, on the bass.
        let drift_floor = Theme::Drift.look(KIND_CEILING).unwrap();
        assert_eq!(drift_floor.params(), Some([BLUE as f32, BAND_BASS as f32]));
        assert!(drift_floor.runs((B::Rings, 1.0, [1.0, 1.0])));
        assert!(!drift_floor.runs((B::Rings, 0.9, [1.0, 1.0])));
        assert!(!drift_floor.runs((B::Rings, 1.0, [6.0, 1.0])));
        assert!(!drift_floor.runs((B::Curls, 1.0, [1.0, 1.0])));
        let frame = Theme::Drift.look(KIND_FRAME).unwrap();
        assert!(frame.runs((B::Pulse, 0.6, [TEAL as f32, BAND_RMS as f32])));
    }

    #[test]
    fn the_knob_applies_once_per_change_and_not_at_launch_on_the_rooms_theme() {
        let drift = Shown::Theme(Theme::Drift);
        // Waiting for the room: nothing, and the launch poll is still to
        // come.
        let mut knob = Knob::default();
        assert_eq!(knob.poll(Some("drift"), true, drift), KnobPoll::Nothing);
        // The launch poll: the room's own theme applies nothing, edited or
        // not.
        assert_eq!(
            knob.poll(Some("drift"), false, drift),
            KnobPoll::Kept(Theme::Drift)
        );
        assert_eq!(knob.poll(Some("drift"), false, drift), KnobPoll::Nothing);
        let mut edited = Knob::default();
        assert_eq!(
            edited.poll(Some(" Drift "), false, Shown::Edited(Theme::Drift)),
            KnobPoll::Kept(Theme::Drift)
        );
        // A change applies, once.
        assert_eq!(
            knob.poll(Some("tide"), false, drift),
            KnobPoll::Apply(Theme::Tide)
        );
        for _ in 0..3 {
            assert_eq!(
                knob.poll(Some("tide"), false, Shown::Theme(Theme::Tide)),
                KnobPoll::Nothing
            );
        }
        // Back to the launch value: a change, so it applies, now that the
        // launch is past.
        assert_eq!(
            knob.poll(Some("drift"), false, Shown::Theme(Theme::Tide)),
            KnobPoll::Apply(Theme::Drift)
        );
        // The same theme again needs "" first.
        assert_eq!(knob.poll(Some(""), false, drift), KnobPoll::Nothing);
        assert_eq!(knob.poll(None, false, drift), KnobPoll::Nothing);
        assert_eq!(
            knob.poll(Some("drift"), false, drift),
            KnobPoll::Apply(Theme::Drift)
        );
        // An unknown name: said once, nothing applied.
        assert_eq!(
            knob.poll(Some("sunset"), false, drift),
            KnobPoll::Unknown("sunset".to_owned())
        );
        assert_eq!(knob.poll(Some("sunset"), false, drift), KnobPoll::Nothing);
        // At launch on another theme than the room's: it applies.
        let mut other = Knob::default();
        assert_eq!(
            other.poll(Some("aurora"), false, drift),
            KnobPoll::Apply(Theme::Aurora)
        );
        // A custom room names no theme: any value applies at launch.
        let mut custom = Knob::default();
        assert_eq!(
            custom.poll(Some("classic"), false, Shown::Custom),
            KnobPoll::Apply(Theme::Classic)
        );
        // Unset at launch, then set: applies.
        let mut late = Knob::default();
        assert_eq!(late.poll(None, false, drift), KnobPoll::Nothing);
        assert_eq!(
            late.poll(Some("drift"), false, drift),
            KnobPoll::Apply(Theme::Drift)
        );
    }

    /// Board #5533: a track's theme comes through dark. The light falls
    /// for a second from the track's start, the look is laid once, dark,
    /// and the light is back after the hold and the rise.
    #[test]
    fn a_tracks_theme_is_laid_in_the_dark_and_once() {
        let dt = 1.0 / 72.0;
        let mut b = Bringing::default();
        assert_close!(b.veil(), 1.0);
        assert!(!b.busy());
        assert_eq!(b.step(dt, Turn::Free), None);
        let what = Brought::Theme(Theme::Flint);
        b.start(what, false, 3);
        let (mut laid, mut laid_at, mut t) = (Vec::new(), 0.0, 0.0f32);
        let mut dark = 0.0;
        let mut last = b.veil();
        let mut turned_up = false;
        while b.busy() {
            let due = b.step(dt, Turn::Free);
            t += dt;
            if let Some(due) = due {
                // Laid with no light on the room.
                assert_close!(b.veil(), 0.0);
                laid.push(due);
                laid_at = t;
            }
            if b.veil() <= 0.0 {
                dark += dt;
            }
            // Down, then up: never back down.
            turned_up |= b.veil() > last;
            assert!(!turned_up || b.veil() >= last);
            last = b.veil();
            assert!(t < 10.0);
        }
        assert_eq!(laid, [Due { what, picks: 3 }]);
        assert!((laid_at - BRING_OUT_S).abs() < 2.0 * dt, "{laid_at}");
        assert!(dark >= BRING_DARK_S, "{dark}");
        let whole = BRING_OUT_S + BRING_DARK_S + BRING_IN_S;
        assert!((t - whole).abs() < 4.0 * dt, "{t}");
        assert_close!(b.veil(), 1.0);
    }

    /// The fade keeps to the flash limit's direction: it falls only where
    /// the lights may fall and rises only where they may rise, faster on a
    /// hit, and with the direction still it runs on its own clock.
    #[test]
    fn the_fade_moves_only_the_way_the_lights_may() {
        let dt = 1.0 / 72.0;
        let mut b = Bringing::default();
        b.start(Brought::Theme(Theme::Flint), false, 0);
        // A rise: the fall waits.
        for _ in 0..72 {
            assert_eq!(b.step(dt, Turn::Rise), None);
        }
        assert_close!(b.veil(), 1.0);
        // Falls: a second of them, however they are spread.
        let mut falling = 0;
        let mut frame = 0;
        let due = loop {
            let turn = if frame % 3 == 0 {
                Turn::Fall
            } else {
                Turn::Rise
            };
            let before = b.veil();
            let due = b.step(dt, turn);
            assert!(b.veil() <= before);
            assert_eq!(b.veil() < before, turn == Turn::Fall);
            falling += usize::from(turn == Turn::Fall);
            frame += 1;
            if due.is_some() {
                break due;
            }
            assert!(frame < 1000);
        };
        assert!(due.is_some());
        assert!((falling as f32 * dt - BRING_OUT_S).abs() < 2.0 * dt);
        // Dark, whatever the direction.
        for _ in 0..(BRING_DARK_S / dt) as usize {
            b.step(dt, Turn::Rise);
            assert_close!(b.veil(), 0.0);
        }
        b.step(dt, Turn::Rise);
        // The rise: none in a fall, quick on a hit.
        let start = b.veil();
        for _ in 0..72 {
            b.step(dt, Turn::Fall);
        }
        assert_close!(b.veil(), start);
        let mut rising = 0;
        while b.busy() {
            let before = b.veil();
            b.step(dt, Turn::Rise);
            assert!(b.veil() > before);
            rising += 1;
            assert!(rising < 1000);
        }
        assert!(
            (rising as f32 * dt - BRING_IN_HIT_S).abs() < 3.0 * dt,
            "{rising}"
        );
        assert_close!(b.veil(), 1.0);
    }

    /// A look the room shows already is laid on the next step, the light
    /// as it is. A track that starts while a change runs takes over from
    /// where the light is, and only its look is laid.
    #[test]
    fn a_shown_look_is_laid_at_once_and_a_new_start_takes_over() {
        let dt = 1.0 / 72.0;
        let mut b = Bringing::default();
        let aurora = Brought::Theme(Theme::Aurora);
        b.start(aurora, true, 0);
        assert!(b.busy());
        assert_eq!(
            b.step(dt, Turn::Free),
            Some(Due {
                what: aurora,
                picks: 0
            })
        );
        assert_close!(b.veil(), 1.0);
        assert!(!b.busy());
        // Half way down for Flint, the next track starts: the room's own.
        b.start(Brought::Theme(Theme::Flint), false, 0);
        for _ in 0..36 {
            assert_eq!(b.step(dt, Turn::Free), None);
        }
        let mid = b.veil();
        assert!(mid > 0.4 && mid < 0.6, "{mid}");
        b.start(Brought::Own, false, 1);
        let mut laid = Vec::new();
        let mut steps = 0;
        while b.busy() {
            // The light goes on down from where it was.
            let before = b.veil();
            if let Some(due) = b.step(dt, Turn::Free) {
                laid.push(due);
            }
            if laid.is_empty() {
                assert!(b.veil() < before);
            }
            steps += 1;
            assert!(steps < 1000);
        }
        assert_eq!(
            laid,
            [Due {
                what: Brought::Own,
                picks: 1
            }]
        );
        // During the rise, a shown look does not skip the fade's end: the
        // new start goes down again from where the light is.
        b.start(Brought::Theme(Theme::Flint), false, 1);
        while b.step(dt, Turn::Free).is_none() {}
        for _ in 0..40 {
            b.step(dt, Turn::Free);
        }
        let rising = b.veil();
        assert!(rising > 0.0 && rising < 1.0, "{rising}");
        b.start(aurora, true, 1);
        b.step(dt, Turn::Free);
        assert!(b.veil() < rising);
    }
}
