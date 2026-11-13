//! Themes (board #5415): the whole room's look in one press. A theme is a
//! name and, for each kind of surface (table, floor, wall, ceiling, frame,
//! other), a behavior, a color index, an audio band and a strength
//! ([`Look`]). Applying one (`lanes::RoomLanes::apply_theme`) sets every
//! kind default and every anchor of the room to its kind's look, hand
//! assignments included: the theme wins. Its name is saved with the room
//! (`room_file.rs`); an edit by hand afterwards stays, and the hand menu's
//! Theme control then shows the theme as edited ("Drift*", [`Shown`]).
//!
//! Plain data, so it builds and tests on the desktop: the table, lookup by
//! name, the cycle, what a room file shows, and the launch rule of the
//! `debug.fosfora.theme` knob ([`Knob`]). No theme adds a behavior or a
//! shader: each is built from the catalogue (`SurfaceBehavior::catalogue`),
//! every pairing on its kind's own list.
//!
//! The four, each with one rule for its colors: the room's planes (walls,
//! floor, ceiling) one color, what stands in it (tables, frames, other)
//! the other, so a table reads against the floor and a frame against its
//! wall.
//!
//! - **Classic**: today's defaults ([`SurfaceBehavior::default_for`]),
//!   each kind's own color, the behavior's band, full strength. Applying it
//!   gives exactly a fresh room. Only Classic runs the wall spectrum: the
//!   edge bars carry the spectrum on every edge in every theme.
//! - **Drift**, the calm one: tables streamlines, the floor rings, walls
//!   streamlines, the ceiling rings, frames the pulse, other surfaces the
//!   curls. The planes blue, the rest teal. The frames' pulse, the one look
//!   in it that jumps on the downbeat, at 0.6.
//! - **Tide**: walls the pulse, the floor curls, tables curls, the ceiling
//!   the pulse, frames the rings, other surfaces the pulse. The planes
//!   violet, the rest rose.
//! - **Aurora**: walls and the ceiling the desktop's Aurora, the floor and
//!   tables streamlines, frames the pulse, other surfaces the curls. The
//!   walls the key's tint, the floor and the ceiling green, the rest teal.
//!
//! Every band is the behavior's own ([`SurfaceBehavior::default_band`]):
//! the rms for the flows, the bass for the rings and the pulse, as Drift
//! asks; a band per theme waits for signal dumps of real songs to choose it
//! by. Every strength is 1 but Drift's frames.

use crate::room_file::RoomFile;
use crate::surface_fx::{BAND_BASS, BAND_RMS};
use crate::surfaces::{
    COLOR_KEY, KIND_CEILING, KIND_FLOOR, KIND_FRAME, KIND_NAMES, KIND_TABLE, KIND_WALL,
    SurfaceBehavior,
};

/// Color indices (`surfaces::SURFACE_PALETTE`, from 1) the themes use.
const BLUE: u32 = 1;
const VIOLET: u32 = 2;
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
}

impl Look {
    const fn new(behavior: SurfaceBehavior, color: u32, band: u32, strength: f32) -> Self {
        Self {
            behavior,
            color,
            band,
            strength,
        }
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
}

impl Theme {
    /// Every theme, in the cycle's order.
    pub const ALL: [Self; 4] = [Self::Classic, Self::Drift, Self::Tide, Self::Aurora];

    /// The name the room file, the knob and the log use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Drift => "drift",
            Self::Tide => "tide",
            Self::Aurora => "aurora",
        }
    }

    /// The name the hand menu and the voice's replies show.
    pub fn title(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Drift => "Drift",
            Self::Tide => "Tide",
            Self::Aurora => "Aurora",
        }
    }

    /// The theme called `name` (any case, spaces around it ignored).
    pub fn from_name(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::ALL
            .into_iter()
            .find(|t| t.name().eq_ignore_ascii_case(name))
    }

    /// The next theme in the cycle, wrapping: Classic, Drift, Tide,
    /// Aurora, Classic.
    #[must_use]
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|&t| t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    /// What every surface of `kind` runs under this theme; for a kind
    /// without a name (an unlabeled anchor) nothing: a theme leaves those
    /// as they are, as All: none does.
    pub fn look(self, kind: u32) -> Option<Look> {
        use SurfaceBehavior as B;
        KIND_NAMES.iter().find(|(k, _)| *k == kind)?;
        let look = Look::new;
        Some(match self {
            Self::Classic => {
                let behavior = B::default_for(kind);
                look(behavior, 0, behavior.default_band(), 1.0)
            }
            Self::Drift => match kind {
                KIND_TABLE => look(B::Streamlines, TEAL, BAND_RMS, 1.0),
                KIND_FLOOR => look(B::Rings, BLUE, BAND_BASS, 1.0),
                KIND_WALL => look(B::Streamlines, BLUE, BAND_RMS, 1.0),
                KIND_CEILING => look(B::Rings, BLUE, BAND_BASS, 1.0),
                KIND_FRAME => look(B::Pulse, TEAL, BAND_BASS, 0.6),
                _ => look(B::Curls, TEAL, BAND_RMS, 1.0),
            },
            Self::Tide => match kind {
                KIND_TABLE => look(B::Curls, ROSE, BAND_RMS, 1.0),
                KIND_FLOOR => look(B::Curls, VIOLET, BAND_RMS, 1.0),
                KIND_WALL => look(B::Pulse, VIOLET, BAND_BASS, 1.0),
                KIND_CEILING => look(B::Pulse, VIOLET, BAND_BASS, 1.0),
                KIND_FRAME => look(B::Rings, ROSE, BAND_BASS, 1.0),
                _ => look(B::Pulse, ROSE, BAND_BASS, 1.0),
            },
            Self::Aurora => match kind {
                KIND_TABLE => look(B::Streamlines, TEAL, BAND_RMS, 1.0),
                KIND_FLOOR => look(B::Streamlines, GREEN, BAND_RMS, 1.0),
                KIND_WALL => look(B::Aurora, COLOR_KEY, BAND_RMS, 1.0),
                KIND_CEILING => look(B::Aurora, GREEN, BAND_RMS, 1.0),
                KIND_FRAME => look(B::Pulse, TEAL, BAND_BASS, 1.0),
                _ => look(B::Curls, TEAL, BAND_RMS, 1.0),
            },
        })
    }

    /// The assignments the theme makes: each named kind
    /// ([`KIND_NAMES`]' order) with its look.
    pub fn looks(self) -> impl Iterator<Item = (u32, Look)> {
        KIND_NAMES
            .into_iter()
            .filter_map(move |(kind, _)| self.look(kind).map(|l| (kind, l)))
    }

    /// The looks in one line for the log, kind by kind: "table
    /// streamlines@1.00 teal rms · floor rings@1.00 blue bass · …".
    pub fn summary(self) -> String {
        self.looks()
            .map(|(kind, l)| {
                format!(
                    "{} {}@{:.2} {} {}",
                    crate::surfaces::kind_name(kind),
                    l.behavior.name(),
                    l.strength,
                    crate::surfaces::color_name(l.color),
                    crate::surface_fx::band_name(l.band)
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

/// What the hand menu's Theme control shows for a room file ([`Self::of`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    /// The room runs this theme: the one saved with it, or Classic for a
    /// room with none saved that runs the defaults (a fresh room).
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

/// The themes' names for a reply or a warning: "classic, drift, tide or
/// aurora" ([`Theme::name`]) or with `titles` "Classic, Drift, Tide or
/// Aurora".
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
        // No departures from the catalogue: each pairing is on its kind's
        // own list (`SurfaceBehavior::catalogue`), so none needed listing.
        for theme in Theme::ALL {
            assert_eq!(theme.looks().count(), KIND_NAMES.len(), "{theme:?}");
            for (kind, look) in theme.looks() {
                let what = format!("{} {}", theme.name(), kind_name(kind));
                assert!(
                    B::catalogue(kind).contains(&look.behavior),
                    "{what}: {} is not on the list",
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
            assert_eq!(
                look,
                Look::new(behavior, 0, behavior.default_band(), 1.0),
                "{}",
                kind_name(kind)
            );
            // Nothing a fresh room lacks: no params.
            assert_eq!(look.params(), None);
        }
        // A fresh room runs Classic.
        assert!(Theme::Classic.runs_in(&RoomFile::default()));
        for theme in [Theme::Drift, Theme::Tide, Theme::Aurora] {
            assert!(!theme.runs_in(&RoomFile::default()), "{theme:?}");
        }
    }

    #[test]
    fn the_four_as_the_brief_names_them() {
        let behaviors =
            |t: Theme| -> Vec<SurfaceBehavior> { t.looks().map(|(_, l)| l.behavior).collect() };
        // In KIND_NAMES' order: table, floor, wall, ceiling, frame, other.
        assert_eq!(
            behaviors(Theme::Classic),
            [
                B::Streamlines,
                B::Rings,
                B::Spectrum,
                B::None,
                B::Pulse,
                B::Curls
            ]
        );
        assert_eq!(
            behaviors(Theme::Drift),
            [
                B::Streamlines,
                B::Rings,
                B::Streamlines,
                B::Rings,
                B::Pulse,
                B::Curls
            ]
        );
        assert_eq!(
            behaviors(Theme::Tide),
            [B::Curls, B::Curls, B::Pulse, B::Pulse, B::Rings, B::Pulse]
        );
        assert_eq!(
            behaviors(Theme::Aurora),
            [
                B::Streamlines,
                B::Streamlines,
                B::Aurora,
                B::Aurora,
                B::Pulse,
                B::Curls
            ]
        );
        // Only Classic runs the wall spectrum.
        for theme in Theme::ALL {
            let spectrum = theme.looks().any(|(_, l)| l.behavior == B::Spectrum);
            assert_eq!(spectrum, theme == Theme::Classic, "{theme:?}");
        }
        // The colors: Drift blue and teal, Tide violet and rose, Aurora
        // green and teal with the key's tint on the walls.
        let colors = |t: Theme| -> Vec<u32> { t.looks().map(|(_, l)| l.color).collect() };
        assert!(
            colors(Theme::Drift)
                .iter()
                .all(|c| [BLUE, TEAL].contains(c))
        );
        assert!(
            colors(Theme::Tide)
                .iter()
                .all(|c| [VIOLET, ROSE].contains(c))
        );
        assert!(
            colors(Theme::Aurora)
                .iter()
                .all(|c| [GREEN, TEAL, COLOR_KEY].contains(c))
        );
        assert_eq!(Theme::Aurora.look(KIND_WALL).unwrap().color, COLOR_KEY);
        // Every band the behavior's own: Drift's rms, but the bass where
        // the behavior's default is the bass.
        for theme in Theme::ALL {
            for (_, l) in theme.looks() {
                assert_eq!(l.band, l.behavior.default_band(), "{theme:?}");
            }
        }
        assert!(
            Theme::Drift
                .looks()
                .all(|(_, l)| l.band == BAND_RMS || l.behavior.default_band() == BAND_BASS)
        );
    }

    #[test]
    fn names_and_the_cycle() {
        assert_eq!(
            Theme::ALL.map(Theme::name),
            ["classic", "drift", "tide", "aurora"]
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
            [Theme::Drift, Theme::Tide, Theme::Aurora, Theme::Classic]
        );
        // The control's press: the next after the one shown, edited or
        // not; custom starts the cycle.
        assert_eq!(Shown::Theme(Theme::Drift).next(), Theme::Tide);
        assert_eq!(Shown::Edited(Theme::Aurora).next(), Theme::Classic);
        assert_eq!(Shown::Custom.next(), Theme::Classic);
        // The words.
        assert_eq!(Shown::Theme(Theme::Drift).label(), "Theme: Drift");
        assert_eq!(Shown::Edited(Theme::Drift).label(), "Theme: Drift*");
        assert_eq!(Shown::Custom.label(), "Theme: custom");
        assert_eq!(Shown::Edited(Theme::Classic).value(), "Classic*");
        assert_eq!(listed(true), "Classic, Drift, Tide or Aurora");
        assert_eq!(listed(false), "classic, drift, tide or aurora");
    }

    #[test]
    fn a_look_runs_what_resolves_to_it() {
        let drift_floor = Theme::Drift.look(KIND_FLOOR).unwrap();
        assert_eq!(drift_floor.params(), Some([BLUE as f32, BAND_BASS as f32]));
        assert!(drift_floor.runs((B::Rings, 1.0, [1.0, 1.0])));
        assert!(!drift_floor.runs((B::Rings, 0.9, [1.0, 1.0])));
        assert!(!drift_floor.runs((B::Rings, 1.0, [6.0, 1.0])));
        assert!(!drift_floor.runs((B::Curls, 1.0, [1.0, 1.0])));
        let frame = Theme::Drift.look(KIND_FRAME).unwrap();
        assert!(frame.runs((B::Pulse, 0.6, [TEAL as f32, BAND_BASS as f32])));
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
}
