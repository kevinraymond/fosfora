//! The voice path, V2: the grammar (board #3751, `docs/xr/VOICE_DESIGN.md`,
//! "V2 as built"). The sentence V1 transcribes becomes one [`Intent`]: an
//! action the hand menu or the room editor already performs, reached by
//! voice. Nothing here is new behavior: every variant names its hand-driven
//! equivalent.
//!
//! **Data plus one matcher.** The words come from what the app holds at the
//! moment of the sentence ([`Vocabulary`]): the names of the three cloud
//! effects the app once had ([`CUT_EFFECTS`]), kept so that a sentence
//! asking for one is answered and not missed ([`EFFECTS_GONE`]), the
//! surface behaviors' names (`surfaces::SurfaceBehavior::name`), the room's
//! surfaces by their friendly names (`surfaces::friendly_name`), the color
//! and band names the hand menu shows (`surfaces::color_name`,
//! `surface_fx::band_name`) and the surface the editor's beam points at. A
//! small set of templates with slots is tried in order ([`parse`]); each
//! slot takes a whole run of words, so a name is never cut in two.
//!
//! **The normalizer** ([`normalize`]): lower case, apostrophes dropped,
//! every other mark a space, whitespace collapsed, and the fillers "the",
//! "a", "an", "please" and "to" dropped wherever they stand (in the names
//! too, so both sides compare alike). V4: number words are digits, `one`
//! to `twenty`, `thirty`, `forty`, `fifty` and their compounds ("twenty
//! one"), so "table fourteen" names "table 14" (whisper writes digits for
//! most spoken numbers, words for some); "one" after "this", "that",
//! "next" or "previous" stays a word.
//!
//! **Clauses, help and the hint** (V4, [`parse_clauses`], [`respond`]): a
//! sentence the grammar misses whole is split at " and ", " then ", ", ",
//! ";" and a sentence's end ([`split_clauses`], never inside a name the
//! room has), each clause parsed on its own, at most [`MAX_CLAUSES`];
//! "what can I say" shows five example phrases built from the room
//! ([`examples`], the next five on each ask); a sentence no template fits
//! gets the nearest example as a hint ([`hint`]).
//!
//! Pure and desktop-tested; `app.rs` builds the vocabulary from the frame,
//! maps the menu's intents onto its `Action` consumer and writes the
//! surface intents through [`apply`].
//!
//! **The agent's actions** (V3, `agent.rs`): one [`AgentAction`] of a
//! model's reply becomes an [`Intent`] through [`Intent::from_json`], by the
//! same slots and the same target resolution as a sentence, so a name the
//! room lacks misses as the grammar's would.

use crate::lanes::{LaneBox, Param, ParamEdit, RoomLanes, Target as LaneTarget};
use crate::surfaces::{
    COLOR_KEY, KIND_CEILING, KIND_FLOOR, KIND_FRAME, KIND_OTHER, KIND_TABLE, KIND_WALL,
    SurfaceBehavior, color_name, friendly_name, friendly_names, kind_name,
};

/// How long the label shows a reply, and a miss (s).
pub const REPLY_S: f32 = 1.5;
pub const MISS_S: f32 = 2.5;

/// One room surface as the grammar names it: its friendly name ("desk",
/// "table 2", "wall 5"), its kind and its index in the frame's lane boxes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surface {
    pub name: String,
    pub kind: u32,
    pub index: usize,
}

/// The frame's surfaces for the grammar: one per lane box, named as the
/// room editor's status row names it.
pub fn surfaces(boxes: &[LaneBox<'_>]) -> Vec<Surface> {
    boxes
        .iter()
        .zip(friendly_names(boxes))
        .enumerate()
        .map(|(k, (b, name))| Surface {
            name,
            kind: b.kind,
            index: k,
        })
        .collect()
}

/// The three cloud effects the app had until board #5470, by the names the
/// hand menu showed. The grammar still knows them ([`Vocabulary::effects`]),
/// so "switch to Flock" is answered [`EFFECTS_GONE`] and not missed.
pub const CUT_EFFECTS: [&str; 3] = ["Flux Cloud", "Embers", "Flock"];

/// What the label says to a sentence, or an agent's action, that asks for
/// a cloud effect, the next one or the one before (board #5470): there is
/// one world effect now, the fireflies, and nothing to switch.
pub const EFFECTS_GONE: &str = "The effects are gone";

/// Whether `intent` asks for a cloud effect, by name or by a step: it does
/// nothing and is answered [`EFFECTS_GONE`].
pub fn asks_for_effect(intent: &Intent) -> bool {
    matches!(
        intent,
        Intent::NextEffect | Intent::PrevEffect | Intent::Effect(_)
    )
}

/// [`CUT_EFFECTS`] as [`Vocabulary::effects`] takes them.
pub fn cut_effects() -> Vec<String> {
    CUT_EFFECTS.map(str::to_owned).to_vec()
}

/// What the grammar can name, built at the moment of the sentence.
#[derive(Debug, Clone)]
pub struct Vocabulary<'a> {
    /// The effect names the grammar recognizes: in the app the cut ones
    /// ([`cut_effects`]), which it answers [`EFFECTS_GONE`].
    pub effects: &'a [String],
    /// The behaviors a surface can be given (`SurfaceBehavior::OFFERED` in
    /// the app).
    pub behaviors: &'a [SurfaceBehavior],
    /// The room's surfaces, one per lane box.
    pub surfaces: Vec<Surface>,
    /// The lane box under the room editor's beam, if any.
    pub pointed: Option<usize>,
    /// The Music row's tracks (board #5385).
    pub tracks: Tracks<'a>,
}

/// The Music row's tracks as the grammar names them (board #5385,
/// `playlist.rs`; the wearer's, or the bundled ones while the folders
/// are empty, board #5528): their titles in the playlist's order, the one the
/// Music row shows (`None` on the test clip), whether the music plays,
/// and whether the shared `Music/` folder goes unread for want of the
/// storage permission (board #5461: a step with no songs names it). The
/// default is no tracks, for the agent's vocabularies, whose actions name
/// none.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tracks<'a> {
    pub titles: &'a [String],
    pub current: Option<usize>,
    pub playing: bool,
    pub blocked: bool,
}

impl Vocabulary<'_> {
    /// The surface of lane box `index`.
    fn surface(&self, index: usize) -> Option<&Surface> {
        self.surfaces.iter().find(|s| s.index == index)
    }
}

/// What a sentence names a surface by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The surface under the editor's beam: "this", "that", "here", or no
    /// name at all.
    Pointed,
    /// A surface by its friendly name, by lane box index.
    Surface(usize),
    /// Every surface of a kind and the kind's default: "every table",
    /// "walls".
    Kind(u32),
}

/// A strength step or value: the hand menu's Strength row steps a tenth;
/// half, full and off set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strength {
    Up,
    Down,
    Half,
    Full,
    Off,
}

/// The hand menu's Immersion control by voice (board #5384): a step along
/// off, low, full, or a depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImmersionStep {
    /// "darker": one step deeper, black at most.
    Darker,
    /// "lighter": one step back, off at least.
    Lighter,
    /// "lights on" is off, "lights off" and "full dark" full, "immersion
    /// low" low.
    To(crate::immerse::Depth),
}

impl ImmersionStep {
    /// The depth this asks for, from `now`.
    pub fn resolve(self, now: crate::immerse::Depth) -> crate::immerse::Depth {
        match self {
            Self::Darker => now.deeper(),
            Self::Lighter => now.lighter(),
            Self::To(depth) => depth,
        }
    }
}

/// The hand menu's Theme control by voice (board #5415): the next theme,
/// as a press, or one by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeStep {
    Next,
    To(crate::themes::Theme),
}

/// What a sentence asks for. Each variant does what a hand already does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// The next cloud effect, while there were three. It does nothing now
    /// and is answered [`EFFECTS_GONE`].
    NextEffect,
    /// The cloud effect before, likewise.
    PrevEffect,
    /// The cloud effect at this index of [`Vocabulary::effects`], likewise.
    Effect(usize),
    /// The hand menu's "Edit room" toggle.
    EditRoom(bool),
    /// The hand menu's "Particles" toggle (the cloud, `Controls::cloud`).
    Cloud(bool),
    /// The hand menu's "Pitcher" toggle.
    Pitcher(bool),
    /// The hand menu's Music row: play or stop.
    Music(bool),
    /// The Music row's `>`: the next track (board #5385).
    NextTrack,
    /// The Music row's `<`: the previous track.
    PrevTrack,
    /// The track at this index of the playlist, played: stepping the
    /// Music row to it and pressing play.
    Track(usize),
    /// What the Music row shows: the track, and whether it plays.
    NowPlaying,
    /// The music folders scanned again for files added since the launch.
    ScanMusic,
    /// The hand menu's "Rescan the room".
    Rescan,
    /// The hand menu's "Recenter".
    Recenter,
    /// The hand menu's "All: none".
    AllNone,
    /// The hand menu itself (board #5459): open, parked ahead of the
    /// wearer with no palm needed (as the palm summons it), or closed (as
    /// a pinch away from it closes it).
    Menu(bool),
    /// The hand menu's Immersion control (board #5384): how dark the room
    /// may get with the music.
    Immersion(ImmersionStep),
    /// The hand menu's Roam control (board #5393): the show follows the
    /// wearer, or stays where it is.
    Roam(bool),
    /// The hand menu's Theme control (board #5415): the whole room's look.
    Theme(ThemeStep),
    /// The editor's tap (one surface) or hold (the kind), set to this
    /// behavior instead of stepped.
    Behavior {
        target: Target,
        behavior: SurfaceBehavior,
    },
    /// The hand menu's Color row with Edit room on, set to this index.
    Color { target: Target, color: u32 },
    /// The hand menu's Band row, set to this band.
    Band { target: Target, band: u32 },
    /// The hand menu's Strength row: a step, or a value.
    Strength { target: Target, strength: Strength },
    /// The status cell and the label for the pointed surface: what it runs.
    Describe(Target),
    /// "What can I say": five example phrases on the label (V4), the way
    /// the hand menu's rows show what the hands can do.
    Help,
    /// The hand menu's Edges control set to this stop (board #5453).
    Edges(crate::edge_bars::Choice),
    /// The hand menu's Agent control (board #5453).
    Agent(AgentAsk),
    /// The hand menu's Voice toggle (board #5453): "voice off" turns it
    /// off as the press does, and only the menu turns it back on.
    Voice(bool),
    /// The pinch that begins the show (board #5452, `first_run.rs`):
    /// "begin" or "start" while the show waits for the wearer.
    Begin,
    /// "Credits" (board #4198): the short credits card ([`CREDITS`]) on the
    /// card "what can I say" uses, staying as long. The licenses in full
    /// are the APK's `NOTICE` (`docs/xr/RELEASE.md`).
    Credits,
}

/// The hand menu's Agent control by voice (board #5453): its stops on the
/// release surface, the on-device provider and off, and "on" for the
/// first provider the control has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAsk {
    On,
    Off,
    Local,
}

/// Why a sentence did nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// No template fits.
    NoMatch,
    /// The sentence means the pointed surface, and none is pointed at.
    NoSurface,
    /// A surface name the room does not have.
    UnknownSurface(String),
    /// A behavior name the catalogue does not have ("fire on the desk").
    UnknownBehavior(String),
    /// A look held back until it is bounded (board #5462,
    /// `SurfaceBehavior::held_back`): astrolabe and fenestra. Nothing
    /// changes, and the label says it is not available.
    NotAvailable(SurfaceBehavior),
    /// The behavior does not render on the surface's kind
    /// (`SurfaceBehavior::catalogue`).
    NotOnThisKind {
        behavior: SurfaceBehavior,
        kind: u32,
    },
    /// A name several surfaces share, none of them pointed at: their names.
    Ambiguous(Vec<String>),
    /// More clauses in one sentence than [`MAX_CLAUSES`]: how many (V4).
    TooMany(usize),
    /// A song name no track's title holds (board #5385).
    NoTrack(String),
    /// A song name several tracks' titles hold: their titles.
    AmbiguousTrack(Vec<String>),
    /// A theme name there is no theme by (board #5415).
    UnknownTheme(String),
}

impl Reason {
    /// Whether the miss was on a surface's name: one the room lacks, or
    /// one several surfaces share. The surface labels show for a while
    /// after one, so the wearer can read the names (board #5378).
    pub fn names_surfaces(&self) -> bool {
        matches!(self, Self::UnknownSurface(_) | Self::Ambiguous(_))
    }
}

/// A sentence that did nothing, and what was heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Miss {
    pub reason: Reason,
    pub heard: String,
}

/// Words dropped wherever they stand.
const FILLERS: [&str; 5] = ["the", "a", "an", "please", "to"];

/// The sentence as words: lower case, apostrophes dropped ("what's" is
/// "whats"), every other non-alphanumeric character a space, the
/// [`FILLERS`] dropped, number words as digits ([`numbers`]).
pub fn normalize(sentence: &str) -> Vec<String> {
    let cleaned: String = sentence
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '\'' | '\u{2019}'))
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    numbers(
        cleaned
            .split_whitespace()
            .filter(|w| !FILLERS.contains(w))
            .map(str::to_owned)
            .collect(),
    )
}

/// The number words `one` to `nineteen`.
const UNITS: [&str; 19] = [
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
/// The tens, `twenty` to `fifty`.
const TENS: [&str; 4] = ["twenty", "thirty", "forty", "fifty"];
/// The words after which "one" is a pronoun, not a number: "this one",
/// "next one".
const ONE_AFTER: [&str; 5] = ["this", "that", "next", "previous", "last"];

/// Number words as digits: `one` to `nineteen`, the tens to `fifty`, and a
/// ten followed by a unit word under ten as one number ("twenty one" is
/// 21). "One" after a word of [`ONE_AFTER`] stays a word.
fn numbers(words: Vec<String>) -> Vec<String> {
    let unit = |w: &str| UNITS.iter().position(|u| *u == w).map(|i| i + 1);
    let ten = |w: &str| TENS.iter().position(|t| *t == w).map(|i| 20 + 10 * i);
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        if let Some(t) = ten(w) {
            match words.get(i + 1).and_then(|n| unit(n)).filter(|u| *u < 10) {
                Some(u) => {
                    out.push((t + u).to_string());
                    i += 2;
                }
                None => {
                    out.push(t.to_string());
                    i += 1;
                }
            }
            continue;
        }
        let pronoun = w == "one" && out.last().is_some_and(|p| ONE_AFTER.contains(&p.as_str()));
        match unit(w) {
            Some(u) if !pronoun => out.push(u.to_string()),
            _ => out.push(w.to_owned()),
        }
        i += 1;
    }
    out
}

/// Whether `ts` is `phrase`'s words.
fn is(ts: &[&str], phrase: &str) -> bool {
    ts.iter().copied().eq(phrase.split(' '))
}

/// Whether `ts` is one of `phrases`.
fn any(ts: &[&str], phrases: &[&str]) -> bool {
    phrases.iter().any(|p| is(ts, p))
}

const NEXT: [&str; 3] = ["next effect", "next", "next one"];
const PREV: [&str; 4] = ["previous effect", "last effect", "previous", "previous one"];
const EDIT_ON: [&str; 4] = [
    "edit room",
    "edit room on",
    "start editing",
    "start editing room",
];
const EDIT_OFF: [&str; 6] = [
    "stop editing",
    "stop editing room",
    "done editing",
    "done editing room",
    "finish editing",
    "edit room off",
];
const RESCAN: [&str; 3] = ["rescan room", "scan room", "rescan"];
/// Board #5393: roam on and off, as [`normalize`] leaves them.
const ROAM_ON: [&str; 6] = [
    "roam",
    "roam mode",
    "follow me",
    "roam on",
    "roam mode on",
    "start roaming",
];
const ROAM_OFF: [&str; 6] = [
    "stay here",
    "seated",
    "stop roaming",
    "roam off",
    "roam mode off",
    "seated mode",
];
const RECENTER: [&str; 6] = [
    "recenter cloud",
    "recenter particles",
    "recenter",
    "re center cloud",
    "re center particles",
    "re center",
];
/// Board #5459: the hand menu opened ahead of the wearer and closed, as
/// [`normalize`] leaves them ("open the menu" is "open menu").
const MENU_OPEN: [&str; 8] = [
    "menu",
    "open menu",
    "show menu",
    "menu on",
    "bring up menu",
    "hand menu",
    "open hand menu",
    "show hand menu",
];
const MENU_CLOSE: [&str; 6] = [
    "close menu",
    "hide menu",
    "menu off",
    "dismiss menu",
    "close hand menu",
    "hide hand menu",
];
/// Board #5452: the show begun by voice, as [`normalize`] leaves the
/// sentences ("let's begin" is "lets begin", "start the show" is "start
/// show").
const BEGIN: [&str; 6] = [
    "begin",
    "start",
    "begin show",
    "start show",
    "lets begin",
    "lets start",
];
/// "Credits" (board #4198), as [`normalize`] leaves them ("show the
/// credits" is "show credits").
const CREDITS_SAID: [&str; 9] = [
    "credits",
    "show credits",
    "licenses",
    "license",
    "show licenses",
    "about",
    "about app",
    "about this app",
    "about fosfora",
];
/// The credits card (board #4198): who made what the app ships with, and
/// where the licenses are, a line each. At most `label::MAX_LINES`, and
/// no line wider than the help card's widest example, so the card reads
/// as large as the help does.
pub const CREDITS: [&str; 6] = [
    "Fosfora: MIT or Apache 2.0",
    "whisper.cpp, Whisper",
    "Voice model: from Ettin",
    "ONNX Runtime, OpenXR",
    "Music: made with YuE2",
    "Licenses: the NOTICE file",
];
/// "What can I say" (V4).
const HELP: [&str; 6] = [
    "what can i say",
    "what can i do",
    "what do i say",
    "help",
    "help me",
    "voice help",
];
const ALL_NONE: [&str; 6] = [
    "clear everything",
    "clear all",
    "nothing anywhere",
    "all none",
    "everything none",
    "everything off",
];
/// The Music row's steps by voice (board #5385), as [`normalize`] leaves
/// them ("skip to the next song" is "skip next song").
const NEXT_TRACK: [&str; 9] = [
    "next song",
    "next track",
    "skip",
    "skip song",
    "skip track",
    "skip this song",
    "skip next song",
    "play next song",
    "play next track",
];
const PREV_TRACK: [&str; 8] = [
    "previous song",
    "previous track",
    "last song",
    "last track",
    "play previous song",
    "play previous track",
    "play last song",
    "go back song",
];
/// "What is playing" (board #5385). "What is this" stays the pointed
/// surface's.
const NOW_PLAYING: [&str; 11] = [
    "what is playing",
    "whats playing",
    "what song is this",
    "what song is playing",
    "what track is this",
    "what track is playing",
    "whats this song",
    "what is this song",
    "which song is this",
    "what am i listening",
    "what music is playing",
];
/// The music folders scanned again (board #5385).
const SCAN_MUSIC: [&str; 9] = [
    "scan music",
    "rescan music",
    "scan my music",
    "rescan my music",
    "refresh music",
    "reload music",
    "update music",
    "look for new songs",
    "find new songs",
];
/// What "play" asks for when it names no song: the music on.
const PLAY_ANY: [&str; 8] = [
    "some music",
    "my music",
    "something",
    "anything",
    "song",
    "songs",
    "some songs",
    "my songs",
];
/// Board #5453: the music by one word, nothing else in the sentence.
const MUSIC_STOP: [&str; 2] = ["stop", "pause"];
const MUSIC_PLAY: [&str; 1] = ["play"];
/// Board #5453: the Agent control's on-device stop, as [`normalize`]
/// leaves it ("on-device" is "on device"), alone or after "use",
/// "switch", "set" or "turn" ([`agent_local`]).
const AGENT_LOCAL: [&str; 4] = [
    "agent local",
    "local agent",
    "agent on device",
    "on device agent",
];
/// What a sentence opens with to switch effects: "switch to embers",
/// "show flock".
const EFFECT_VERBS: [&str; 4] = ["switch", "show", "go", "change"];
/// A verb a surface sentence may open with, dropped: "make the desk
/// amber", "put embers on the table", "turn the desk off".
const SURFACE_VERBS: [&str; 7] = ["put", "set", "make", "turn", "give", "paint", "use"];
/// Words for the pointed surface.
const POINTED: [&str; 6] = ["this", "that", "here", "it", "this one", "that one"];
/// Words for "every".
const EVERY: [&str; 3] = ["all", "every", "each"];
/// Words that make a strength of "off" ("strength off").
const LEVEL: [&str; 3] = ["strength", "brightness", "intensity"];
/// Words for behavior none.
const NONE_WORDS: [&str; 4] = ["none", "nothing", "clear", "off"];
/// Board #5384: the room's immersion, a step at a time ("darker" alone is
/// the room's; a surface's dimmer names the surface, "the desk darker")...
const DARKER: [&str; 9] = [
    "darker",
    "room darker",
    "make room darker",
    "darken room",
    "dim room",
    "dim lights",
    "lights down",
    "more immersion",
    "immersion up",
];
const LIGHTER: [&str; 8] = [
    "lighter",
    "room lighter",
    "make room lighter",
    "brighten room",
    "room brighter",
    "lights up",
    "less immersion",
    "immersion down",
];
/// ...or to a depth: the lights on is the plain room, off the full depth.
const LIGHTS_ON: [&str; 4] = ["lights on", "light on", "on lights", "on light"];
const LIGHTS_OFF: [&str; 7] = [
    "lights off",
    "light off",
    "off lights",
    "off light",
    "lights out",
    "full dark",
    "full darkness",
];

/// Board #5415: the next theme, as [`normalize`] leaves them ("change the
/// theme" is "change theme").
const NEXT_THEME: [&str; 9] = [
    "next theme",
    "another theme",
    "other theme",
    "different theme",
    "new theme",
    "change theme",
    "switch theme",
    "cycle theme",
    "theme next",
];
/// Verbs a theme sentence may open with, dropped: "set the theme to
/// aurora", "use the drift theme", "switch to the tide theme".
const THEME_VERBS: [&str; 8] = [
    "set", "change", "switch", "use", "make", "go", "apply", "pick",
];
/// The longest run of words a theme's name is taken from: "set the theme
/// to blue sky" asks for an unknown theme, a longer tail is something else.
const MAX_THEME_WORDS: usize = 2;

/// A kind word, singular or plural (the plural is always the kind):
/// `table`, `desk`; `floor`, `ground`; `wall`; `ceiling`, `roof`;
/// `frame`, `window`, `door`; `other`, `couch`, `sofa`, `chair`.
fn kind_word(w: &str) -> Option<(u32, bool)> {
    let (kind, plural) = match w {
        "table" | "desk" => (KIND_TABLE, false),
        "tables" | "desks" => (KIND_TABLE, true),
        "floor" | "ground" => (KIND_FLOOR, false),
        "floors" => (KIND_FLOOR, true),
        "wall" => (KIND_WALL, false),
        "walls" => (KIND_WALL, true),
        "ceiling" | "roof" => (KIND_CEILING, false),
        "ceilings" => (KIND_CEILING, true),
        "frame" | "window" | "door" => (KIND_FRAME, false),
        "frames" | "windows" | "doors" => (KIND_FRAME, true),
        "other" | "couch" | "sofa" | "chair" => (KIND_OTHER, false),
        "others" | "couches" | "sofas" | "chairs" => (KIND_OTHER, true),
        _ => return None,
    };
    Some((kind, plural))
}

/// What a target slot came to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Aim {
    Hit(Target),
    /// A name several surfaces share (lane box indices), none pointed at.
    Several(Vec<usize>),
    Miss(Reason),
}

/// The longest run of words a surface name takes ("wall art 3").
const MAX_NAME_WORDS: usize = 3;

/// The target slot: `ts` whole names the pointed surface (no words, or
/// "this"), a kind ("every table", "walls"), a surface by its friendly
/// name ("table 2"), the surfaces sharing a name without its number
/// ("table"), or the surfaces of a kind word ("desk" in a room without
/// one). A shared name resolves to the pointed surface when it is one of
/// them, to every floor when they are all floors (the scene floor and the
/// stage floor are one floor to the eye), else to [`Aim::Several`]. Words
/// the room has no surface for are a name it lacks when one of them is a
/// kind word, a number or a word of a surface's name ("table 9", "lamp
/// 2", "shelf" alone);
/// several words with none ("shut that song" before "up") are no name,
/// and the sentence misses as [`Reason::NoMatch`] so the agent reads it
/// (board #3771).
fn aim(ts: &[&str], vocab: &Vocabulary<'_>) -> Aim {
    if ts.is_empty() || any(ts, &POINTED) {
        return match vocab.pointed {
            Some(_) => Aim::Hit(Target::Pointed),
            None => Aim::Miss(Reason::NoSurface),
        };
    }
    if ts.len() > MAX_NAME_WORDS {
        return Aim::Miss(Reason::NoMatch);
    }
    let rest = match ts {
        [q, "of", rest @ ..] | [q, rest @ ..] if EVERY.contains(q) => Some(rest),
        _ => None,
    };
    if let Some(&[w]) = rest
        && let Some((kind, _)) = kind_word(w)
    {
        return Aim::Hit(Target::Kind(kind));
    }
    if let &[w] = ts
        && let Some((kind, true)) = kind_word(w)
    {
        return Aim::Hit(Target::Kind(kind));
    }
    // A surface's name as words, and whether `ts` is all of it, or it
    // without its disambiguating number ("table" for "table 2").
    let words = |s: &Surface| normalize(&s.name);
    let exact = |s: &&Surface| words(s).iter().map(String::as_str).eq(ts.iter().copied());
    let base = |s: &&Surface| match words(s).split_last() {
        Some((last, base)) if last.bytes().all(|b| b.is_ascii_digit()) => {
            base.iter().map(String::as_str).eq(ts.iter().copied())
        }
        _ => false,
    };
    if let Some(s) = vocab.surfaces.iter().find(exact) {
        return Aim::Hit(Target::Surface(s.index));
    }
    let mut several: Vec<usize> = vocab
        .surfaces
        .iter()
        .filter(base)
        .map(|s| s.index)
        .collect();
    if several.is_empty()
        && let &[w] = ts
        && let Some((kind, _)) = kind_word(w)
    {
        several = vocab
            .surfaces
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| s.index)
            .collect();
    }
    match several.as_slice() {
        [] if ts.len() > 1 && !ts.iter().any(|w| surface_word(w, vocab)) => {
            Aim::Miss(Reason::NoMatch)
        }
        [] => Aim::Miss(Reason::UnknownSurface(ts.join(" "))),
        [k] => Aim::Hit(Target::Surface(*k)),
        many => {
            let kind_of = |k: &usize| vocab.surface(*k).map(|s| s.kind);
            if many.iter().all(|k| kind_of(k) == Some(KIND_FLOOR)) {
                Aim::Hit(Target::Kind(KIND_FLOOR))
            } else if vocab.pointed.is_some_and(|p| many.contains(&p)) {
                Aim::Hit(Target::Pointed)
            } else {
                Aim::Several(several)
            }
        }
    }
}

/// Whether `w` could be part of a surface's name: a kind word, a number
/// (names end in one: "table 2"), or a word of a name the room has.
fn surface_word(w: &str, vocab: &Vocabulary<'_>) -> bool {
    w.bytes().all(|b| b.is_ascii_digit())
        || kind_word(w).is_some()
        || vocab
            .surfaces
            .iter()
            .any(|s| normalize(&s.name).iter().any(|n| n == w))
}

/// The behavior slot: one word, a behavior's name in the vocabulary (a
/// trailing "s" forgiven: "streamline", "curl"), or none's words. A look
/// that is no longer offered (board #5462: embers, sparks, astrolabe,
/// fenestra) is heard too, so [`fill`] can give it its stand-in or say it
/// is not available, where it would otherwise miss as unknown.
fn behavior(ts: &[&str], vocab: &Vocabulary<'_>) -> Option<SurfaceBehavior> {
    let &[w] = ts else { return None };
    if NONE_WORDS.contains(&w) {
        return Some(SurfaceBehavior::None);
    }
    let known = |name: &str| {
        SurfaceBehavior::from_name(name).filter(|b| vocab.behaviors.contains(b) || !b.offered())
    };
    known(w).or_else(|| known(&format!("{w}s")))
}

/// The color slot: a color the hand menu names (`surfaces::color_name`,
/// "warm white" two words), "key" for the key's tint, a few everyday
/// names (purple, pink, white, orange), or the kind's own ("normal color",
/// "its own color", "default color"); "color" after a name is allowed.
fn color(ts: &[&str]) -> Option<u32> {
    let (ts, said_color) = match ts.split_last() {
        Some((&"color", rest)) if !rest.is_empty() => (rest, true),
        _ => (ts, false),
    };
    let joined = ts.join(" ");
    if let Some(i) = (1..=COLOR_KEY).find(|&i| color_name(i) == joined) {
        return Some(i);
    }
    match joined.as_str() {
        "keys" => Some(COLOR_KEY),
        "purple" => Some(2),
        "white" => Some(3),
        "orange" => Some(4),
        "pink" => Some(7),
        "normal" | "own" | "its own" | "default" | "original" if said_color => Some(0),
        _ => None,
    }
}

/// The band slot: bass (whisper hears "base"), low; mid, mids, middle;
/// high, highs, treble; rms, level, volume; "band" after it allowed.
fn band(ts: &[&str]) -> Option<u32> {
    let ts = match ts.split_last() {
        Some((&"band", rest)) if !rest.is_empty() => rest,
        _ => ts,
    };
    let &[w] = ts else { return None };
    match w {
        "rms" | "level" | "volume" => Some(crate::surface_fx::BAND_RMS),
        "bass" | "base" | "low" | "lows" => Some(crate::surface_fx::BAND_BASS),
        "mid" | "mids" | "middle" => Some(crate::surface_fx::BAND_MID),
        "high" | "highs" | "treble" => Some(crate::surface_fx::BAND_HIGH),
        _ => None,
    }
}

/// The strength slot: brighter, up, more; dimmer, down, less; half, full;
/// a [`LEVEL`] word before or after is allowed ("half strength",
/// "brightness up"), and with one, off or zero ("strength off").
fn strength(ts: &[&str]) -> Option<Strength> {
    let ts = ts.strip_prefix(&["at"]).unwrap_or(ts);
    let (ts, leveled) = match ts {
        [l, rest @ ..] | [rest @ .., l] if LEVEL.contains(l) && !rest.is_empty() => (rest, true),
        _ => (ts, false),
    };
    let &[w] = ts else { return None };
    match w {
        "brighter" | "up" | "more" | "stronger" => Some(Strength::Up),
        "dimmer" | "down" | "less" | "weaker" | "darker" => Some(Strength::Down),
        "half" => Some(Strength::Half),
        "full" | "max" | "maximum" => Some(Strength::Full),
        "off" | "zero" if leveled => Some(Strength::Off),
        _ => None,
    }
}

/// A slot that matched, waiting for its target.
#[derive(Debug, Clone, Copy)]
enum Slot {
    Behavior(SurfaceBehavior),
    Color(u32),
    Band(u32),
    Strength(Strength),
}

impl Slot {
    /// The intent for this slot on `target`.
    fn on(self, target: Target) -> Intent {
        match self {
            Self::Behavior(behavior) => Intent::Behavior { target, behavior },
            Self::Color(color) => Intent::Color { target, color },
            Self::Band(band) => Intent::Band { target, band },
            Self::Strength(strength) => Intent::Strength { target, strength },
        }
    }
}

/// The kind of what `target` names, for the catalogue check.
fn kind_of(target: Target, vocab: &Vocabulary<'_>) -> Option<u32> {
    match target {
        Target::Pointed => vocab.pointed.and_then(|p| vocab.surface(p)).map(|s| s.kind),
        Target::Surface(k) => vocab.surface(k).map(|s| s.kind),
        Target::Kind(kind) => Some(kind),
    }
}

/// `slot` on what `target_words` names: the intent, or why not. A
/// behavior is checked against the kind's catalogue, also for a shared
/// name whose surfaces are all of one kind (there the kind is the answer,
/// not the question which one). Board #5462: a look held back is
/// [`Reason::NotAvailable`] whatever the target, and embers or sparks,
/// whose particles are gone, become the kind's stand-in
/// (`SurfaceBehavior::stand_in`), so the intent and its reply name the
/// look the surface gets.
fn fill(slot: Slot, target_words: &[&str], vocab: &Vocabulary<'_>) -> Result<Intent, Reason> {
    if let Slot::Behavior(b) = slot
        && b.held_back()
    {
        return Err(Reason::NotAvailable(b));
    }
    let check = |kind: Option<u32>| match (slot, kind) {
        (Slot::Behavior(b), kind) if !b.offered() => Ok(Slot::Behavior(
            b.stand_in(kind.unwrap_or(crate::surfaces::KIND_NONE))
                .unwrap_or(SurfaceBehavior::None),
        )),
        (Slot::Behavior(b), Some(kind)) if !SurfaceBehavior::catalogue(kind).contains(&b) => {
            Err(Reason::NotOnThisKind { behavior: b, kind })
        }
        _ => Ok(slot),
    };
    match aim(target_words, vocab) {
        Aim::Hit(target) => {
            let slot = check(kind_of(target, vocab))?;
            Ok(slot.on(target))
        }
        Aim::Several(ks) => {
            let kinds: Vec<Option<u32>> = ks
                .iter()
                .map(|&k| vocab.surface(k).map(|s| s.kind))
                .collect();
            if kinds.windows(2).all(|w| w[0] == w[1]) {
                check(kinds.first().copied().flatten())?;
            }
            Err(Reason::Ambiguous(
                ks.iter()
                    .filter_map(|&k| vocab.surface(k).map(|s| s.name.clone()))
                    .collect(),
            ))
        }
        Aim::Miss(reason) => Err(reason),
    }
}

/// The menu's phrases: the toggles and the buttons, and the phrases that
/// stepped the cloud effects, which are still heard so they can be
/// answered.
fn menu(ts: &[&str], vocab: &Vocabulary<'_>) -> Option<Intent> {
    if any(ts, &HELP) {
        return Some(Intent::Help);
    }
    if any(ts, &CREDITS_SAID) {
        return Some(Intent::Credits);
    }
    if any(ts, &BEGIN) {
        return Some(Intent::Begin);
    }
    if any(ts, &MENU_OPEN) {
        return Some(Intent::Menu(true));
    }
    if any(ts, &MENU_CLOSE) {
        return Some(Intent::Menu(false));
    }
    if any(ts, &NEXT) {
        return Some(Intent::NextEffect);
    }
    if any(ts, &PREV) {
        return Some(Intent::PrevEffect);
    }
    if any(ts, &EDIT_ON) {
        return Some(Intent::EditRoom(true));
    }
    if any(ts, &EDIT_OFF) {
        return Some(Intent::EditRoom(false));
    }
    if any(ts, &RESCAN) {
        return Some(Intent::Rescan);
    }
    if any(ts, &RECENTER) {
        return Some(Intent::Recenter);
    }
    if any(ts, &ROAM_ON) {
        return Some(Intent::Roam(true));
    }
    if any(ts, &ROAM_OFF) {
        return Some(Intent::Roam(false));
    }
    if any(ts, &ALL_NONE) {
        return Some(Intent::AllNone);
    }
    if let Some(step) = immersion(ts) {
        return Some(Intent::Immersion(step));
    }
    if any(ts, &NEXT_TRACK) {
        return Some(Intent::NextTrack);
    }
    if any(ts, &PREV_TRACK) {
        return Some(Intent::PrevTrack);
    }
    if any(ts, &NOW_PLAYING) {
        return Some(Intent::NowPlaying);
    }
    if any(ts, &SCAN_MUSIC) {
        return Some(Intent::ScanMusic);
    }
    // Board #5453: the menu's last three controls, and the music by one
    // word.
    if let Some(choice) = edges(ts) {
        return Some(Intent::Edges(choice));
    }
    if agent_local(ts) {
        return Some(Intent::Agent(AgentAsk::Local));
    }
    if any(ts, &MUSIC_STOP) {
        return Some(Intent::Music(false));
    }
    if any(ts, &MUSIC_PLAY) {
        return Some(Intent::Music(true));
    }
    if let Some(t) = toggle(ts) {
        return Some(t);
    }
    effect(ts, vocab).map(Intent::Effect)
}

/// Board #5453: the Edges control's stops: "edges theme", "edges off",
/// "edges face", "edges stand", "edges in", the word before or after
/// "edges" (or "edge bars"), a leading "turn", "switch", "set" or "make"
/// dropped. "On" and "show" are the theme's (the control's first stop and
/// the launch's); "hide" and "no" are off; "standing" is stand; "inside"
/// and "tunnel" are in.
fn edges(ts: &[&str]) -> Option<crate::edge_bars::Choice> {
    use crate::edge_bars::{Choice, Mode};
    let ts = match ts {
        ["turn" | "switch" | "set" | "make", rest @ ..] => rest,
        _ => ts,
    };
    let word = match ts {
        ["edges" | "edge", w]
        | [w, "edges" | "edge"]
        | ["edge", "bars", w]
        | [w, "edge", "bars"] => *w,
        _ => return None,
    };
    Some(match word {
        "theme" | "on" | "show" => Choice::Theme,
        "off" | "hide" | "no" | "none" => Choice::Force(Mode::Off),
        "face" | "faces" => Choice::Force(Mode::Face),
        "stand" | "standing" => Choice::Force(Mode::Standing),
        "in" | "inside" | "tunnel" | "tunnels" => Choice::Force(Mode::In),
        _ => return None,
    })
}

/// Board #5453: the Agent control's on-device stop ([`AGENT_LOCAL`]).
/// "Local" alone is enough after a verb ("use local"), not without one.
fn agent_local(ts: &[&str]) -> bool {
    match ts {
        ["use" | "switch" | "set" | "turn", rest @ ..] => {
            any(rest, &AGENT_LOCAL) || is(rest, "local")
        }
        _ => any(ts, &AGENT_LOCAL),
    }
}

/// The Agent control's on and off as a toggle's ([`device`]).
fn agent_toggle(on: bool) -> Intent {
    Intent::Agent(if on { AgentAsk::On } else { AgentAsk::Off })
}

/// The toggles: "cloud on", "particles off", "pitcher on", "music play",
/// "play music", "turn the cloud off", "turn off the music", "show the
/// particles", "hide the cloud".
fn toggle(ts: &[&str]) -> Option<Intent> {
    let ts = match ts {
        ["turn" | "switch", rest @ ..] => rest,
        _ => ts,
    };
    let (dev, state) = match ts {
        [d, s] if device(d).is_some() => (*d, *s),
        [s, d] if device(d).is_some() => (*d, *s),
        _ => return None,
    };
    let on = match state {
        "on" | "show" | "start" => true,
        "off" | "hide" | "stop" => false,
        "play" if dev == "music" => true,
        "pause" if dev == "music" => false,
        _ => return None,
    };
    device(dev).map(|d| d(on))
}

/// A toggle's word: the cloud ("cloud", or "particles" as the menu's row
/// reads), the pitcher, the music; board #5453: the voice ("voice off")
/// and the agent ("agent on", "turn the agent off").
fn device(w: &str) -> Option<fn(bool) -> Intent> {
    match w {
        "cloud" | "particles" => Some(Intent::Cloud),
        "pitcher" => Some(Intent::Pitcher),
        "music" => Some(Intent::Music),
        "voice" => Some(Intent::Voice),
        "agent" => Some(agent_toggle),
        _ => None,
    }
}

/// Board #5384: the room's immersion. "darker" and "lighter" step it
/// ([`DARKER`], [`LIGHTER`]); "lights on" sets it off, "lights off" and
/// "full dark" full ([`LIGHTS_ON`], [`LIGHTS_OFF`]); "immersion" with off,
/// none, low, half, full, on or max, before or after it, sets that depth. A
/// leading "turn" or "switch" is dropped ("turn the lights off").
fn immersion(ts: &[&str]) -> Option<ImmersionStep> {
    use crate::immerse::Depth;
    let ts = match ts {
        ["turn" | "switch", rest @ ..] => rest,
        _ => ts,
    };
    if any(ts, &DARKER) {
        return Some(ImmersionStep::Darker);
    }
    if any(ts, &LIGHTER) {
        return Some(ImmersionStep::Lighter);
    }
    if any(ts, &LIGHTS_ON) {
        return Some(ImmersionStep::To(Depth::Off));
    }
    if any(ts, &LIGHTS_OFF) {
        return Some(ImmersionStep::To(Depth::Full));
    }
    let depth = match ts {
        ["immersion" | "immerse", w] | [w, "immersion"] => *w,
        _ => return None,
    };
    let depth = match depth {
        "off" | "none" => Depth::Off,
        "low" | "half" => Depth::Low,
        "full" | "on" | "max" => Depth::Full,
        "black" | "blackout" | "always" => Depth::Black,
        _ => return None,
    };
    Some(ImmersionStep::To(depth))
}

/// Board #5415: the whole room's theme. "next theme" (and "another theme",
/// "change the theme") steps it as the control's press does; "theme drift",
/// "drift theme", "set the theme to aurora", "use the tide theme" name one
/// (a leading verb of [`THEME_VERBS`] dropped). A name no theme has is
/// [`Reason::UnknownTheme`], whose reply lists the themes. `None` for a
/// sentence without the word "theme" at its start or end.
fn theme(ts: &[&str]) -> Option<Result<ThemeStep, Reason>> {
    if any(ts, &NEXT_THEME) {
        return Some(Ok(ThemeStep::Next));
    }
    let (ts, verb) = match ts {
        [v, rest @ ..] if THEME_VERBS.contains(v) && !rest.is_empty() => (rest, true),
        _ => (ts, false),
    };
    let name = match ts {
        ["theme"] if verb => return Some(Ok(ThemeStep::Next)),
        ["theme", rest @ ..] | [rest @ .., "theme"]
            if !rest.is_empty() && rest.len() <= MAX_THEME_WORDS =>
        {
            rest.join(" ")
        }
        _ => return None,
    };
    if [
        "next",
        "next one",
        "another",
        "another one",
        "other",
        "different",
        "new",
    ]
    .contains(&name.as_str())
    {
        return Some(Ok(ThemeStep::Next));
    }
    Some(
        crate::themes::Theme::from_name(&name)
            .map(ThemeStep::To)
            .ok_or(Reason::UnknownTheme(name)),
    )
}

/// "play {song}" (board #5385; also "play song …", "play track …"): the
/// track whose title the name finds (`playlist::find`), or why none.
/// "play some music" and the like name no song and turn the music on.
/// `None` for a sentence that does not open with "play" and a name.
fn track(ts: &[&str], vocab: &Vocabulary<'_>) -> Option<Result<Intent, Reason>> {
    let rest = match ts {
        ["play", "song" | "track", rest @ ..] | ["play", rest @ ..] if !rest.is_empty() => rest,
        _ => return None,
    };
    let name = rest.join(" ");
    if PLAY_ANY.contains(&name.as_str()) {
        return Some(Ok(Intent::Music(true)));
    }
    let titles = vocab.tracks.titles;
    Some(match crate::playlist::find(titles, &name) {
        crate::playlist::Found::One(i) => Ok(Intent::Track(i)),
        crate::playlist::Found::Several(is) => Err(Reason::AmbiguousTrack(
            is.iter().filter_map(|&i| titles.get(i).cloned()).collect(),
        )),
        crate::playlist::Found::Nothing => Err(Reason::NoTrack(name)),
    })
}

/// A cloud effect by name, alone or after a switching verb.
fn effect(ts: &[&str], vocab: &Vocabulary<'_>) -> Option<usize> {
    let named = |ts: &[&str]| {
        vocab.effects.iter().position(|e| {
            let words = normalize(e);
            !words.is_empty() && words.iter().map(String::as_str).eq(ts.iter().copied())
        })
    };
    named(ts).or_else(|| match ts {
        [v, rest @ ..] if EFFECT_VERBS.contains(v) => named(rest),
        _ => None,
    })
}

/// The surface templates, in order: "what is {target}" (and "what are",
/// "list", "what {target} exist", "which {target} are there"), "clear
/// {target}", `{x} on {target}` and `{target} on {band}`, `{target} in
/// {color}`, `{target} follows {band}`, then `{target} {x}` with the
/// target possibly absent (the pointed surface). The first that fits
/// whole is the intent; a template whose slot fits but whose target does
/// not gives the reason, the first such one being the answer when nothing
/// fits.
fn surface(ts: &[&str], vocab: &Vocabulary<'_>) -> Result<Intent, Reason> {
    // Board #5378: "what are the walls", "list the tables", "what walls
    // exist", "which tables are there" ask for a kind's list as "describe"
    // does.
    let asked = match ts {
        ["what", "is" | "s" | "are", rest @ ..] | ["whats" | "describe" | "list", rest @ ..] => {
            Some(rest)
        }
        ["what" | "which", rest @ .., "exist" | "exists"]
        | ["what" | "which", rest @ .., "are" | "is", "there"]
            if !rest.is_empty() =>
        {
            Some(rest)
        }
        _ => None,
    };
    if let Some(rest) = asked {
        let rest = rest.strip_prefix(&["on"]).unwrap_or(rest);
        return match aim(rest, vocab) {
            Aim::Hit(t) => Ok(Intent::Describe(t)),
            Aim::Several(ks) => Err(Reason::Ambiguous(names(&ks, vocab))),
            Aim::Miss(r) => Err(r),
        };
    }
    if let ["clear", rest @ ..] = ts
        && !rest.is_empty()
    {
        return fill(Slot::Behavior(SurfaceBehavior::None), rest, vocab);
    }
    let ts = match ts {
        [v, rest @ ..] if SURFACE_VERBS.contains(v) && !rest.is_empty() => rest,
        _ => ts,
    };
    let mut first_miss: Option<Reason> = None;
    // The first reason a slot that fit gave (a target too long to be a
    // name is no reason: the sentence is something else).
    let keep = |r: Reason, first: &mut Option<Reason>| {
        if r != Reason::NoMatch && first.is_none() {
            *first = Some(r);
        }
    };
    // `{x} on {target}` and `{target} on {band}`.
    for (i, w) in ts.iter().enumerate() {
        let (l, r) = (&ts[..i], &ts[i + 1..]);
        match *w {
            "on" | "onto" => {
                let left = behavior(l, vocab)
                    .map(Slot::Behavior)
                    .or_else(|| color(l).map(Slot::Color))
                    .or_else(|| strength(l).map(Slot::Strength));
                if let Some(slot) = left {
                    match fill(slot, r, vocab) {
                        Ok(intent) => return Ok(intent),
                        Err(e) => keep(e, &mut first_miss),
                    }
                } else if let Some(b) = band(r) {
                    match fill(Slot::Band(b), l, vocab) {
                        Ok(intent) => return Ok(intent),
                        Err(e) => keep(e, &mut first_miss),
                    }
                } else if (1..=2).contains(&l.len())
                    && matches!(aim(r, vocab), Aim::Hit(_) | Aim::Several(_))
                    && !r.is_empty()
                {
                    keep(Reason::UnknownBehavior(l.join(" ")), &mut first_miss);
                }
            }
            "in" => {
                if let Some(c) = color(r) {
                    match fill(Slot::Color(c), l, vocab) {
                        Ok(intent) => return Ok(intent),
                        Err(e) => keep(e, &mut first_miss),
                    }
                }
            }
            "follow" | "follows" | "following" => {
                if let Some(b) = band(r) {
                    match fill(Slot::Band(b), l, vocab) {
                        Ok(intent) => return Ok(intent),
                        Err(e) => keep(e, &mut first_miss),
                    }
                }
            }
            _ => {}
        }
    }
    // `{target} {x}`: the longest tail that is a slot.
    for j in 0..ts.len() {
        let (l, r) = (&ts[..j], &ts[j..]);
        let slot = behavior(r, vocab)
            .map(Slot::Behavior)
            .or_else(|| color(r).map(Slot::Color))
            .or_else(|| strength(r).map(Slot::Strength))
            .or_else(|| band(r).map(Slot::Band));
        if let Some(slot) = slot {
            match fill(slot, l, vocab) {
                Ok(intent) => return Ok(intent),
                Err(e) => keep(e, &mut first_miss),
            }
            break;
        }
    }
    Err(first_miss.unwrap_or(Reason::NoMatch))
}

/// The names of lane boxes `ks`.
fn names(ks: &[usize], vocab: &Vocabulary<'_>) -> Vec<String> {
    ks.iter()
        .filter_map(|&k| vocab.surface(k).map(|s| s.name.clone()))
        .collect()
}

/// The sentence as an intent, or why not: the menu's phrases first (the
/// effect row, the toggles, the buttons, a world effect by name), then a
/// theme (board #5415), a song by name, then the surface templates.
pub fn parse(sentence: &str, vocab: &Vocabulary<'_>) -> Result<Intent, Miss> {
    let miss = |reason| Miss {
        reason,
        heard: sentence.trim().to_owned(),
    };
    let words = normalize(sentence);
    let ts: Vec<&str> = words.iter().map(String::as_str).collect();
    if ts.is_empty() {
        return Err(miss(Reason::NoMatch));
    }
    if let Some(intent) = menu(&ts, vocab) {
        return Ok(intent);
    }
    if let Some(outcome) = theme(&ts) {
        return outcome.map(Intent::Theme).map_err(miss);
    }
    if let Some(outcome) = track(&ts, vocab) {
        return outcome.map_err(miss);
    }
    surface(&ts, vocab).map_err(miss)
}

/// The most clauses one sentence asks for (V4): "amber on the desk and
/// the walls on the bass" is two; more than this is [`Reason::TooMany`].
pub const MAX_CLAUSES: usize = 3;
/// How long the label shows a reply of several lines, a kind's list (s).
pub const HELP_S: f32 = 4.0;
/// How long "what can I say"'s card stays at most (s, board #5453): the
/// next fist or thumb tap opens a window, whose "Listening…" takes its
/// place, so a wearer reads it at their own pace and puts it away by
/// talking.
pub const CARD_S: f32 = 30.0;
/// How many examples one ask shows, and the longest one (characters).
pub const HELP_LINES: usize = 5;
const MAX_EXAMPLE_CHARS: usize = 29;
/// The label for [`Reason::TooMany`].
pub const TOO_MANY_LABEL: &str = "Three things at most in one sentence";
/// Where a sentence splits into clauses: the conjunctions, a comma, a
/// semicolon, and a sentence's end (whisper writes two commands said with
/// a pause as two sentences).
const SPLITTERS: [&str; 7] = [" and ", " then ", ", ", ";", ". ", "? ", "! "];
/// Words a clause may open with that join it to the last one, dropped.
const JOINERS: [&str; 3] = ["and", "then", "also"];

/// `sentence` cut into clauses at the [`SPLITTERS`], each trimmed and
/// without a leading joiner ("and then rings on the floor" is "rings on
/// the floor"); a clause of fillers alone ("please") is dropped. A
/// splitter inside a name the vocabulary has (an effect, a surface, a
/// behavior or a color whose name carries "and" or a comma) does not
/// split. A sentence without a splitter is one clause.
pub fn split_clauses(sentence: &str, vocab: &Vocabulary<'_>) -> Vec<String> {
    let s = sentence.trim();
    // ASCII lower case keeps every byte where it was, so the offsets found
    // in it cut `s`; the splitters are ASCII, so every cut is on a char
    // boundary.
    let lower = s.to_ascii_lowercase();
    let names = vocab
        .effects
        .iter()
        .cloned()
        .chain(vocab.surfaces.iter().map(|x| x.name.clone()))
        .chain(vocab.behaviors.iter().map(|b| b.name().to_owned()))
        .chain((1..=COLOR_KEY).map(|i| color_name(i).to_owned()))
        .chain(vocab.tracks.titles.iter().cloned());
    let mut guarded: Vec<(usize, usize)> = Vec::new();
    for name in names {
        let name = name.to_ascii_lowercase();
        if !SPLITTERS.iter().any(|sp| name.contains(sp)) {
            continue;
        }
        let mut from = 0;
        while let Some(i) = lower.get(from..).and_then(|rest| rest.find(&name)) {
            guarded.push((from + i, from + i + name.len()));
            from += i + 1;
        }
    }
    let bytes = lower.as_bytes();
    let mut cuts: Vec<&str> = Vec::new();
    let (mut start, mut i) = (0, 0);
    while i < bytes.len() {
        let hit = SPLITTERS
            .iter()
            .find(|sp| bytes[i..].starts_with(sp.as_bytes()))
            .filter(|sp| !guarded.iter().any(|&(a, b)| a <= i && i + sp.len() <= b));
        match hit {
            Some(sp) => {
                cuts.push(&s[start..i]);
                i += sp.len();
                start = i;
            }
            None => i += 1,
        }
    }
    cuts.push(&s[start..]);
    cuts.into_iter()
        .map(|c| {
            let mut c = c.trim();
            while let Some(rest) = JOINERS.iter().find_map(|j| {
                c.get(..j.len())
                    .filter(|head| head.eq_ignore_ascii_case(j))
                    .and_then(|_| c.get(j.len()..))
                    .filter(|rest| rest.starts_with(char::is_whitespace))
            }) {
                c = rest.trim_start();
            }
            c.to_owned()
        })
        .filter(|c| !normalize(c).is_empty())
        .collect()
}

/// One clause of a sentence and what the grammar made of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clause {
    pub text: String,
    pub outcome: Result<Intent, Miss>,
}

/// The sentence as clauses (V4). A sentence the grammar matches whole is
/// one clause, as in V2 (so a comma whisper put inside a command, "the
/// desk, in amber", splits nothing); else it is cut ([`split_clauses`])
/// and each clause parsed on its own, a clause's miss not stopping the
/// others. More than [`MAX_CLAUSES`] is one clause missing as
/// [`Reason::TooMany`]; a sentence that does not split keeps its own miss.
pub fn parse_clauses(sentence: &str, vocab: &Vocabulary<'_>) -> Vec<Clause> {
    let text = sentence.trim().to_owned();
    let whole = parse(sentence, vocab);
    if whole.is_ok() {
        return vec![Clause {
            text,
            outcome: whole,
        }];
    }
    let parts = split_clauses(sentence, vocab);
    match parts.len() {
        0 => vec![Clause {
            text,
            outcome: whole,
        }],
        1 => {
            let part = parse(&parts[0], vocab);
            vec![Clause {
                text,
                outcome: if part.is_ok() { part } else { whole },
            }]
        }
        n if n > MAX_CLAUSES => vec![Clause {
            outcome: Err(Miss {
                reason: Reason::TooMany(n),
                heard: text.clone(),
            }),
            text,
        }],
        _ => parts
            .into_iter()
            .map(|text| Clause {
                outcome: parse(&text, vocab),
                text,
            })
            .collect(),
    }
}

/// Every example the room gives, in the order the help pages through
/// them: a round robin over the menu's phrases, the room's surfaces (a
/// template each, turning through a behavior of the surface's kind, a
/// color, a band, a strength and "what is") and the kinds ("every wall
/// aurora"). Names come from the vocabulary, so each one is a surface of
/// this room. Board #5470: no example steps or names an effect, since
/// there is none to switch to, and a behavior the vocabulary lacks
/// (embers, sparks) is never an example.
fn pool(vocab: &Vocabulary<'_>) -> Vec<String> {
    // Board #5459: "open the menu" on the first page, for the wearer who
    // has one hand free.
    let menu: Vec<String> = [
        "next theme",
        "open the menu",
        "music play and next theme",
        "edit the room",
        "particles off",
        // Board #5453: every control the menu has.
        "edges off",
        "agent off",
        "voice off",
        "music stop",
        "recenter",
        "rescan the room",
        "next song",
        "what is playing",
        "theme aurora",
        "close the menu",
        "menu",
        // Board #4198: last, so every line before keeps its place.
        "credits",
    ]
    .map(str::to_owned)
    .to_vec();
    let runs = |kind: u32, k: usize| -> Option<&'static str> {
        let own: Vec<SurfaceBehavior> = SurfaceBehavior::catalogue(kind)
            .iter()
            .copied()
            .filter(|b| *b != SurfaceBehavior::None && vocab.behaviors.contains(b))
            .collect();
        (!own.is_empty()).then(|| own[k % own.len()].name())
    };
    let surfaces: Vec<String> = vocab
        .surfaces
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let name = &s.name;
            Some(match i % 5 {
                0 => format!("{} on {name}", runs(s.kind, i / 5)?),
                1 => format!("{name} in {}", color_name(1 + (i as u32 / 5) % 7)),
                2 => format!("{name} on the {}", ["bass", "mids", "highs"][(i / 5) % 3]),
                3 => format!("{name} {}", ["brighter", "dimmer", "half"][(i / 5) % 3]),
                _ => format!("what is {name}"),
            })
        })
        .collect();
    let mut kinds: Vec<u32> = Vec::new();
    for s in &vocab.surfaces {
        if [KIND_TABLE, KIND_WALL, KIND_FLOOR, KIND_CEILING, KIND_FRAME].contains(&s.kind)
            && !kinds.contains(&s.kind)
        {
            kinds.push(s.kind);
        }
    }
    let kinds: Vec<String> = kinds
        .iter()
        .enumerate()
        .filter_map(|(i, &kind)| Some(format!("every {} {}", kind_name(kind), runs(kind, i + 1)?)))
        .collect();
    let lists = [menu, surfaces, kinds];
    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
    (0..longest)
        .flat_map(|k| lists.iter().filter_map(move |l| l.get(k).cloned()))
        .collect()
}

/// The examples the help can show: [`pool`]'s, under 30 characters, and
/// only those the grammar matches on this vocabulary (every clause), so
/// the help never shows a phrase that would miss.
fn shown_pool(vocab: &Vocabulary<'_>) -> Vec<String> {
    pool(vocab)
        .into_iter()
        .filter(|e| {
            e.chars().count() <= MAX_EXAMPLE_CHARS
                && parse_clauses(e, vocab).iter().all(|c| c.outcome.is_ok())
        })
        .collect()
}

/// "What can I say", ask number `page` (from 0): the next [`HELP_LINES`]
/// examples of this room, wrapping round its pool.
pub fn examples(vocab: &Vocabulary<'_>, page: usize) -> Vec<String> {
    let pool = shown_pool(vocab);
    if pool.is_empty() {
        return Vec::new();
    }
    let start = page.wrapping_mul(HELP_LINES) % pool.len();
    (0..HELP_LINES.min(pool.len()))
        .map(|k| pool[(start + k) % pool.len()].clone())
        .collect()
}

/// Words that say little about which phrase was meant, left out of the
/// hint's shared words.
const GLUE: [&str; 5] = ["on", "in", "of", "and", "then"];
/// With no word shared, the shortest common prefix (characters) that
/// still makes an example near.
const MIN_PREFIX: usize = 4;

/// The hint under a [`Reason::NoMatch`]: the example nearest to what was
/// `heard`, by the words they share (glue words aside), then by how long
/// a prefix their words have in common; `Try "what can I say"` when
/// nothing is near.
pub fn hint(heard: &str, vocab: &Vocabulary<'_>) -> String {
    let said = normalize(heard);
    let said_line = said.join(" ");
    let best = shown_pool(vocab)
        .into_iter()
        .map(|e| {
            let words = normalize(&e);
            let shared = words
                .iter()
                .filter(|w| !GLUE.contains(&w.as_str()) && said.contains(w))
                .count();
            let prefix = words
                .join(" ")
                .chars()
                .zip(said_line.chars())
                .take_while(|(a, b)| a == b)
                .count();
            (shared, prefix, e)
        })
        .filter(|(shared, prefix, _)| *shared > 0 || *prefix >= MIN_PREFIX)
        // The first of the best: `max_by_key` keeps the last of equals.
        .fold(None::<(usize, usize, String)>, |best, c| match best {
            Some(b) if (b.0, b.1) >= (c.0, c.1) => Some(b),
            _ => Some(c),
        });
    format!(
        "Try \"{}\"",
        best.map_or_else(|| "what can I say".to_owned(), |(.., e)| e)
    )
}

/// What a sentence came to (V4, [`respond`]): the label and how long it
/// shows, and the words for the agent, if any go to it.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub label: String,
    pub seconds: f32,
    pub agent: Option<String>,
}

/// The sentence's clauses applied in order, and the label: each clause's
/// own text (its reply through `apply`, or its miss's text) joined with
/// " · "; then "what can I say"'s examples a line each, the page turned
/// on each ask; then, under a [`Reason::NoMatch`] the agent does not take,
/// the [`hint`]. With the agent on (`agent`), a clause that misses the way
/// the agent takes (`agent::forwards`) goes to it instead of showing its
/// miss, and "Thinking…" stands in its place: the whole sentence when
/// every clause goes or it had too many, else the clauses that go, joined
/// with " and ". The menu's and the surfaces' clauses that matched are
/// applied first, so the agent sees the room after them.
pub fn respond(
    sentence: &str,
    clauses: &[Clause],
    vocab: &Vocabulary<'_>,
    agent: bool,
    help_page: &mut usize,
    mut apply: impl FnMut(&Intent) -> String,
) -> Response {
    let goes = |c: &Clause| {
        agent
            && matches!(&c.outcome, Err(m) if crate::agent::forwards(&m.reason)
                || matches!(m.reason, Reason::TooMany(_)))
    };
    let mut parts: Vec<String> = Vec::new();
    let mut extra: Vec<String> = Vec::new();
    let mut to_agent: Vec<&str> = Vec::new();
    // Board #5378: a reply of several lines (a kind's list) shows as long
    // as the help, to be read.
    let (mut missed, mut helped, mut listed) = (false, false, false);
    for c in clauses {
        match &c.outcome {
            Ok(Intent::Help) => {
                extra.extend(examples(vocab, *help_page));
                *help_page = help_page.wrapping_add(1);
                helped = true;
            }
            // Board #4198: the credits on the same card, as long.
            Ok(Intent::Credits) => {
                extra.extend(CREDITS.map(str::to_owned));
                helped = true;
            }
            Ok(intent) => {
                let text = apply(intent);
                listed |= text.contains('\n');
                parts.push(text);
            }
            Err(_) if goes(c) => {
                to_agent.push(&c.text);
                if !parts.iter().any(|p| p == crate::agent::THINKING_LABEL) {
                    parts.push(crate::agent::THINKING_LABEL.to_owned());
                }
            }
            Err(miss) => {
                missed = true;
                parts.push(miss_text(miss));
                if miss.reason == Reason::NoMatch && !extra.iter().any(|l| l.starts_with("Try ")) {
                    extra.push(hint(&miss.heard, vocab));
                }
            }
        }
    }
    let agent_text = (!to_agent.is_empty()).then(|| {
        if to_agent.len() == clauses.len() {
            sentence.trim().to_owned()
        } else {
            to_agent.join(" and ")
        }
    });
    let label = parts
        .join(" \u{b7} ")
        .lines()
        .map(str::to_owned)
        .chain(extra)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let seconds = if agent_text.is_some() {
        crate::agent::THINKING_S
    } else if helped {
        CARD_S
    } else if listed {
        HELP_S
    } else if missed {
        MISS_S
    } else {
        REPLY_S
    };
    Response {
        label,
        seconds,
        agent: agent_text,
    }
}

/// The log line for a sentence of `clauses`: one clause as [`log_line`]
/// has it, several as `voice: heard "…" → [<intent or miss>, …] · label
/// "…"`; the label's lines joined with " | ".
pub fn clauses_log_line(sentence: &str, clauses: &[Clause], label: &str) -> String {
    let label = label.replace('\n', " | ");
    if let [one] = clauses {
        return log_line(sentence, &one.outcome, &label);
    }
    let what: Vec<String> = clauses
        .iter()
        .map(|c| match &c.outcome {
            Ok(intent) => format!("{intent:?}"),
            Err(miss) => format!("miss {:?}", miss.reason),
        })
        .collect();
    format!(
        "voice: heard \"{}\" \u{2192} [{}] \u{b7} label \"{label}\"",
        sentence.trim(),
        what.join(", ")
    )
}

/// One action of the agent's reply (`agent.rs`, the schema's `actions`
/// items): a kind from the fixed set, a target (a surface's name, a kind
/// word, `pointed`, or empty) and a value (an effect, `on`/`off`, a
/// behavior, a color, a band, a strength word, or empty). A missing field
/// reads as empty.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AgentAction {
    pub kind: String,
    pub target: String,
    pub value: String,
}

impl std::fmt::Display for AgentAction {
    /// `kind target value`, the empty parts left out: "color desk amber".
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts: Vec<&str> = [&self.kind, &self.target, &self.value]
            .into_iter()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .collect();
        f.write_str(&parts.join(" "))
    }
}

/// An agent action's target as the target slot's words: none for
/// `pointed` or empty (the pointed surface, as a sentence without a name);
/// a bare kind word that is no surface's own name as "every" and the word
/// (the agent's kind words mean the kind, where "table" alone in a
/// sentence asks which one); else the name's words.
fn agent_target(target: &str, vocab: &Vocabulary<'_>) -> Vec<String> {
    let target = target.trim();
    if target.is_empty() || target.eq_ignore_ascii_case("pointed") {
        return Vec::new();
    }
    let words = normalize(target);
    if let [w] = words.as_slice()
        && kind_word(w).is_some()
        && !vocab.surfaces.iter().any(|s| normalize(&s.name) == words)
    {
        return vec!["every".to_owned(), w.clone()];
    }
    words
}

impl Intent {
    /// One action of the agent's reply as an intent, through the grammar's
    /// own slots and target resolution: the same effect names, the same
    /// behavior lookup and catalogue check, the same color and band names,
    /// the same surface names. A surface the room does not have is the
    /// grammar's [`Reason::UnknownSurface`], a behavior the catalogue lacks
    /// its [`Reason::UnknownBehavior`], one the kind does not run its
    /// [`Reason::NotOnThisKind`], one held back its
    /// [`Reason::NotAvailable`]; embers and sparks become their stand-ins,
    /// as in a sentence (board #5462); an unknown kind, effect, color, band or
    /// strength, or a toggle that is neither on nor off, is
    /// [`Reason::NoMatch`]. The miss's `heard` is the action as text
    /// ("color desk orchid").
    pub fn from_json(action: &AgentAction, vocab: &Vocabulary<'_>) -> Result<Self, Miss> {
        let miss = |reason| Miss {
            reason,
            heard: action.to_string(),
        };
        let value = normalize(&action.value);
        let vs: Vec<&str> = value.iter().map(String::as_str).collect();
        let target = agent_target(&action.target, vocab);
        let ts: Vec<&str> = target.iter().map(String::as_str).collect();
        let on = || match vs.as_slice() {
            ["on" | "play" | "start" | "show" | "true"] => Ok(true),
            ["off" | "stop" | "pause" | "hide" | "false"] => Ok(false),
            _ => Err(miss(Reason::NoMatch)),
        };
        // The value with words around it, for a slot it alone does not
        // fit: the kind's own color ("own" as "own color"), strength off
        // ("off" as "strength off").
        let around = |pre: &[&'static str], post: &[&'static str]| -> Vec<&str> {
            pre.iter()
                .copied()
                .chain(vs.iter().copied())
                .chain(post.iter().copied())
                .collect()
        };
        let filled = |slot: Option<Slot>| match slot {
            Some(slot) => fill(slot, &ts, vocab).map_err(miss),
            None => Err(miss(Reason::NoMatch)),
        };
        match action.kind.trim() {
            "next_effect" => Ok(Self::NextEffect),
            // `previous_effect` is the on-device model's id for it (V5,
            // `local.rs`), the agent schema's `prev_effect` spelled out.
            "prev_effect" | "previous_effect" => Ok(Self::PrevEffect),
            "effect" => effect(&vs, vocab)
                .map(Self::Effect)
                .ok_or_else(|| miss(Reason::NoMatch)),
            "edit_room" => on().map(Self::EditRoom),
            "cloud" | "particles" => on().map(Self::Cloud),
            "pitcher" => on().map(Self::Pitcher),
            "music" => on().map(Self::Music),
            "rescan" => Ok(Self::Rescan),
            "recenter" => Ok(Self::Recenter),
            "all_none" => Ok(Self::AllNone),
            "behavior" => match behavior(&vs, vocab) {
                Some(b) => filled(Some(Slot::Behavior(b))),
                None => Err(miss(Reason::UnknownBehavior(
                    action.value.trim().to_owned(),
                ))),
            },
            "color" => filled(
                color(&vs)
                    .or_else(|| color(&around(&[], &["color"])))
                    .map(Slot::Color),
            ),
            "band" => filled(band(&vs).map(Slot::Band)),
            "strength" => filled(
                strength(&vs)
                    .or_else(|| strength(&around(&["strength"], &[])))
                    .map(Slot::Strength),
            ),
            "describe" => match aim(&ts, vocab) {
                Aim::Hit(t) => Ok(Self::Describe(t)),
                Aim::Several(ks) => Err(miss(Reason::Ambiguous(names(&ks, vocab)))),
                Aim::Miss(r) => Err(miss(r)),
            },
            _ => Err(miss(Reason::NoMatch)),
        }
    }
}

/// What the label calls `target`: the surface's name, "all tables" for a
/// kind (`label::kind_plural`, the hold's words), "this" for a pointed
/// surface the vocabulary lacks.
pub fn target_name(target: Target, vocab: &Vocabulary<'_>) -> String {
    let named = |k: usize| {
        vocab
            .surface(k)
            .map_or_else(|| "this".to_owned(), |s| s.name.clone())
    };
    match target {
        Target::Pointed => vocab.pointed.map_or_else(|| "this".to_owned(), named),
        Target::Surface(k) => named(k),
        Target::Kind(kind) => format!("all {}", crate::label::kind_plural(kind)),
    }
}

/// The label's confirmation, in the house style of the editor's labels
/// (`label::cycle_text`, `class_text`): "desk: streamlines", "all walls:
/// spectrum", "desk: amber", "next theme".
pub fn reply(intent: &Intent, vocab: &Vocabulary<'_>) -> String {
    let on_off = |on: bool| if on { "on" } else { "off" };
    let of = |t: Target, what: &str| format!("{}: {what}", target_name(t, vocab));
    match *intent {
        Intent::NextEffect | Intent::PrevEffect | Intent::Effect(_) => EFFECTS_GONE.to_owned(),
        Intent::EditRoom(on) => format!("edit room {}", on_off(on)),
        Intent::Cloud(on) => format!("particles {}", on_off(on)),
        Intent::Pitcher(on) => format!("pitcher {}", on_off(on)),
        Intent::Music(on) => format!("music {}", on_off(on)),
        Intent::NextTrack => track_step(vocab, 1, "next"),
        Intent::PrevTrack => track_step(vocab, -1, "previous"),
        Intent::Track(i) => format!(
            "playing {}",
            vocab.tracks.titles.get(i).map_or("", String::as_str)
        ),
        Intent::NowPlaying => {
            let t = vocab.tracks;
            match (t.current.and_then(|i| t.titles.get(i)), t.playing) {
                (Some(title), true) => format!("playing: {title}"),
                (Some(title), false) => format!("stopped: {title}"),
                (None, true) => "playing the test clip".to_owned(),
                (None, false) => "music off".to_owned(),
            }
        }
        Intent::ScanMusic => "looking for new music".to_owned(),
        Intent::Rescan => "rescanning the room".to_owned(),
        Intent::Recenter => "recentered".to_owned(),
        Intent::AllNone => "every surface: none".to_owned(),
        Intent::Menu(true) => "menu open".to_owned(),
        Intent::Menu(false) => "menu closed".to_owned(),
        // With the menu on the palm (`debug.fosfora.menustay 0`) the app
        // answers `MENU_ON_PALM` instead.
        Intent::Immersion(ImmersionStep::To(depth)) => immersion_reply(depth),
        Intent::Immersion(ImmersionStep::Darker) => "darker".to_owned(),
        Intent::Immersion(ImmersionStep::Lighter) => "lighter".to_owned(),
        Intent::Roam(on) => format!("roam {}", on_off(on)),
        Intent::Theme(ThemeStep::To(theme)) => theme_reply(theme),
        Intent::Theme(ThemeStep::Next) => "next theme".to_owned(),
        Intent::Behavior { target, behavior } => of(target, behavior.name()),
        Intent::Color { target, color: 0 } => of(target, "own color"),
        Intent::Color { target, color } => of(target, color_name(color)),
        Intent::Band { target, band } => of(target, crate::surface_fx::band_name(band)),
        Intent::Strength { target, strength } => of(
            target,
            match strength {
                Strength::Up => "brighter",
                Strength::Down => "dimmer",
                Strength::Half => "half strength",
                Strength::Full => "full strength",
                Strength::Off => "strength 0",
            },
        ),
        Intent::Describe(target) => target_name(target, vocab),
        Intent::Help => "what can I say".to_owned(),
        Intent::Credits => CREDITS.join("\n"),
        Intent::Edges(choice) => edges_reply(choice),
        // What the control comes to is the app's ([`agent_step`]).
        Intent::Agent(AgentAsk::On) => "agent on".to_owned(),
        Intent::Agent(AgentAsk::Off) => "agent: off".to_owned(),
        Intent::Agent(AgentAsk::Local) => "agent: local".to_owned(),
        Intent::Voice(true) => "voice on".to_owned(),
        Intent::Voice(false) => VOICE_OFF_REPLY.to_owned(),
        // Once the show is on the app answers `BEGUN_REPLY` instead.
        Intent::Begin => BEGIN_REPLY.to_owned(),
    }
}

/// Board #5452: the label for a spoken "begin" while the show waits, and
/// for one after it has begun, which changes nothing.
pub const BEGIN_REPLY: &str = "starting";
pub const BEGUN_REPLY: &str = "already started";

/// Whether `sentence` asks to begin the show (board #5452), whatever the
/// room holds: the `debug.fosfora.say` knob feeds such a sentence without
/// waiting for the room's anchors.
pub fn asks_to_begin(sentence: &str) -> bool {
    let words = normalize(sentence);
    let ts: Vec<&str> = words.iter().map(String::as_str).collect();
    any(&ts, &BEGIN)
}

/// Board #5459: the label for a spoken open or close while the menu lives
/// on the palm (`debug.fosfora.menustay 0`), where the voice leaves it
/// alone.
pub const MENU_ON_PALM: &str = "The menu is on your left palm";

/// The label once the Edges control is set by voice (board #5453): the
/// stop it came to, "edges: theme", "edges: off", "edges: stand".
pub fn edges_reply(choice: crate::edge_bars::Choice) -> String {
    match choice {
        crate::edge_bars::Choice::Theme => "edges: theme".to_owned(),
        crate::edge_bars::Choice::Force(mode) => format!("edges: {}", mode.name()),
    }
}

/// The label for "voice off" (board #5453): it is off, and only the hand
/// menu turns it back on, since no sentence is heard until then. Two
/// lines, so it stays as long as a list ([`HELP_S`]).
pub const VOICE_OFF_REPLY: &str = "voice off\nTurn it back on in the hand menu";
/// The labels for an Agent control the headset has nothing for ("Agent:
/// none"), and for the on-device stop when only a network provider is
/// configured.
pub const NO_AGENT_REPLY: &str = "No agent on this headset";
pub const NO_LOCAL_REPLY: &str = "No local agent on this headset";

/// The Agent control asked by voice (board #5453), as its presses would
/// set it: the pick `ask` comes to among `picks` (the control's cycle,
/// `agent::Available::picks`, empty when it reads "Agent: none") with
/// `now` showing, `None` when nothing changes, and the label in the
/// control's words, "agent: local". "On" keeps a provider that is on, and
/// from off takes the first in the cycle, as one press from off does.
pub fn agent_step(
    ask: AgentAsk,
    picks: &[crate::agent::Pick],
    now: crate::agent::Pick,
) -> (Option<crate::agent::Pick>, String) {
    use crate::agent::Pick;
    let first = picks.iter().copied().find(|&p| p != Pick::Off);
    let want = match (ask, first) {
        (_, None) => return (None, NO_AGENT_REPLY.to_owned()),
        (AgentAsk::Off, _) => Pick::Off,
        (AgentAsk::On, _) if now != Pick::Off => now,
        (AgentAsk::On, Some(first)) => first,
        (AgentAsk::Local, _) if picks.contains(&Pick::Local) => Pick::Local,
        (AgentAsk::Local, _) => return (None, NO_LOCAL_REPLY.to_owned()),
    };
    let word = want
        .label()
        .strip_prefix("Agent: ")
        .unwrap_or_else(|| want.name());
    ((want != now).then_some(want), format!("agent: {word}"))
}

/// The label once an immersion intent is applied, whatever asked for it:
/// the depth it came to, "immersion low" (a "darker" at full says "immersion
/// full", where "darker" would promise a change).
pub fn immersion_reply(depth: crate::immerse::Depth) -> String {
    format!("immersion {}", depth.name())
}

/// The label once a theme is applied, whatever asked for it (board
/// #5415): the theme it came to, "theme: Drift" (a "next theme" names the
/// one it stepped to).
pub fn theme_reply(theme: crate::themes::Theme) -> String {
    format!("theme: {}", theme.title())
}

/// The reply to a track step `by` (1 next, -1 previous): the track the
/// Music row steps to, "next: One More Time", as `playlist::stepped`
/// finds it; with no tracks, the Music row's own words for it
/// (`music::no_songs`, board #5461).
fn track_step(vocab: &Vocabulary<'_>, by: isize, word: &str) -> String {
    let t = vocab.tracks;
    match crate::playlist::stepped(t.titles.len(), t.current, by) {
        Some(i) => format!("{word}: {}", t.titles[i]),
        None => crate::music::no_songs(t.blocked).to_owned(),
    }
}

/// How many names an ambiguity lists before "or N more".
const MAX_LISTED: usize = 5;

/// "a, b or c"; past [`MAX_LISTED`], "a, b, c, d, e or 2 more".
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        _ if names.len() > MAX_LISTED => format!(
            "{} or {} more",
            names[..MAX_LISTED].join(", "),
            names.len() - MAX_LISTED
        ),
        [head @ .., last] => format!("{} or {last}", head.join(", ")),
    }
}

/// A kind as "a table", "a wall"; "that surface" for other and unlabeled.
fn a_kind(kind: u32) -> String {
    match kind_name(kind) {
        "other" | "none" => "that surface".to_owned(),
        name => format!("a {name}"),
    }
}

/// The label for a look held back (board #5462, [`Reason::NotAvailable`]):
/// one short sentence, "Astrolabe is not available".
pub fn not_available(behavior: SurfaceBehavior) -> String {
    let name = behavior.name();
    let mut first = name.chars();
    match first.next() {
        Some(c) => format!("{}{} is not available", c.to_uppercase(), first.as_str()),
        None => "Not available".to_owned(),
    }
}

/// Whether an agent's action asks for a look whose particles are gone
/// (embers, sparks) and so gets its stand-in (board #5462,
/// `SurfaceBehavior::stand_in`): the model's own sentence then names a
/// look the surface does not get, and the label shows what was applied
/// instead (`agent::reply_label`).
pub fn stands_in(action: &AgentAction) -> bool {
    let value = normalize(&action.value);
    let vs: Vec<&str> = value.iter().map(String::as_str).collect();
    action.kind.trim() == "behavior"
        && match vs.as_slice() {
            &[w] => [w.to_owned(), format!("{w}s")]
                .iter()
                .filter_map(|n| SurfaceBehavior::from_name(n))
                .any(|b| b.stand_in(crate::surfaces::KIND_NONE).is_some()),
            _ => false,
        }
}

/// The label for a miss: "Didn't catch that" with the sentence, or what
/// was missing.
pub fn miss_text(miss: &Miss) -> String {
    match &miss.reason {
        Reason::NoMatch => format!("{}: \"{}\"", crate::voice::NOTHING_LABEL, miss.heard),
        Reason::NoSurface => "Point at a surface, or name one".to_owned(),
        Reason::UnknownSurface(name) => format!("No surface called \"{name}\""),
        Reason::UnknownBehavior(name) => format!("No behavior called \"{name}\""),
        Reason::NotAvailable(behavior) => not_available(*behavior),
        Reason::NotOnThisKind { behavior, kind } => {
            format!("{} doesn't run on {}", behavior.name(), a_kind(*kind))
        }
        Reason::Ambiguous(names) => format!("Which one: {}?", listed(names)),
        Reason::TooMany(_) => TOO_MANY_LABEL.to_owned(),
        Reason::NoTrack(name) => format!("No song called \"{name}\""),
        Reason::AmbiguousTrack(titles) => format!("Which one: {}?", listed(titles)),
        Reason::UnknownTheme(name) => format!(
            "No theme called \"{name}\": {}",
            crate::themes::listed(true)
        ),
    }
}

/// The log line for a sentence: `voice: heard "…" → <intent or miss> ·
/// label "…"`.
pub fn log_line(sentence: &str, outcome: &Result<Intent, Miss>, label: &str) -> String {
    let what = match outcome {
        Ok(intent) => format!("{intent:?}"),
        Err(miss) => format!("miss {:?}", miss.reason),
    };
    format!(
        "voice: heard \"{}\" \u{2192} {what} \u{b7} label \"{label}\"",
        sentence.trim()
    )
}

/// The lane boxes `target` names: the pointed one, one by index, or every
/// box of a kind.
fn indices(target: Target, vocab: &Vocabulary<'_>, boxes: &[LaneBox<'_>]) -> Vec<usize> {
    match target {
        Target::Pointed => vocab.pointed.into_iter().collect(),
        Target::Surface(k) => vec![k],
        Target::Kind(kind) => boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.kind == kind)
            .map(|(k, _)| k)
            .collect(),
    }
}

/// Write a surface intent through the lanes' typed writers, the editor's
/// and the hand menu's own paths (each saves and logs as theirs do), and
/// return the label's text: [`reply`], or for [`Intent::Describe`] what
/// the surface runs (`label::param_text`, "desk: curls · amber · mid ·
/// 0.7"). `None` for the menu's intents, which the app maps onto its
/// `Action` consumer. A behavior on one surface keeps its strength, as
/// the editor's tap does; on a kind it goes through
/// [`RoomLanes::set_kind`]. A color, band or strength on a kind sets each
/// surface of it in turn. A theme (board #5415) goes through
/// [`RoomLanes::apply_theme`], the next one from what the control shows,
/// and the label names the theme it came to ([`theme_reply`]).
pub fn apply(
    intent: &Intent,
    vocab: &Vocabulary<'_>,
    lanes: &mut RoomLanes,
    boxes: &[LaneBox<'_>],
) -> Option<String> {
    if let Intent::Theme(step) = *intent {
        let theme = match step {
            ThemeStep::To(theme) => theme,
            ThemeStep::Next => lanes.theme().next(),
        };
        lanes.apply_theme(theme, boxes, "voice");
        return Some(theme_reply(theme));
    }
    let target = match *intent {
        Intent::Behavior { target, .. }
        | Intent::Color { target, .. }
        | Intent::Band { target, .. }
        | Intent::Strength { target, .. }
        | Intent::Describe(target) => target,
        _ => return None,
    };
    let ks = indices(target, vocab, boxes);
    if target == Target::Pointed && ks.is_empty() {
        return Some(miss_text(&Miss {
            reason: Reason::NoSurface,
            heard: String::new(),
        }));
    }
    match *intent {
        // Board #5462: the grammar never builds one ([`fill`]); a look
        // that is not offered is written by no voice path.
        Intent::Behavior { behavior, .. } if !behavior.offered() => {
            return Some(not_available(behavior));
        }
        Intent::Behavior {
            target: Target::Kind(kind),
            behavior,
        } => lanes.set_kind(kind, behavior, boxes),
        Intent::Behavior { behavior, .. } => {
            for &k in &ks {
                let strength = lanes.effective(k, boxes).map_or(1.0, |(_, s)| s);
                if let Err(e) = lanes.assign(&LaneTarget::Index(k), behavior, strength, boxes) {
                    log::warn!("voice: {e}; nothing changed");
                }
            }
        }
        Intent::Color { color, .. } => {
            for &k in &ks {
                lanes.set_params(
                    k,
                    boxes,
                    ParamEdit {
                        color: Some(color),
                        ..ParamEdit::default()
                    },
                );
            }
        }
        Intent::Band { band, .. } => {
            for &k in &ks {
                lanes.set_params(
                    k,
                    boxes,
                    ParamEdit {
                        band: Some(band),
                        ..ParamEdit::default()
                    },
                );
            }
        }
        Intent::Strength { strength, .. } => {
            for &k in &ks {
                let edit = match strength {
                    Strength::Up | Strength::Down => ParamEdit::step(
                        Param::Strength,
                        strength == Strength::Up,
                        lanes.params_of(k, boxes),
                    ),
                    Strength::Half | Strength::Full | Strength::Off => ParamEdit {
                        strength: Some(match strength {
                            Strength::Half => 0.5,
                            Strength::Full => 1.0,
                            _ => 0.0,
                        }),
                        ..ParamEdit::default()
                    },
                };
                lanes.set_params(k, boxes, edit);
            }
        }
        Intent::Describe(_) => return Some(describe(target, &ks, vocab, lanes, boxes)),
        _ => {}
    }
    Some(reply(intent, vocab))
}

/// The widest line of a kind's list on the voice label (characters): two
/// names with their behaviors after the kind's word. The label shrinks a
/// line this long from its 64 px to about 52 (`label.rs`), still well
/// above its 40 px floor.
const LIST_LINE_CHARS: usize = 36;

/// What `target` runs, for "what is this": one surface's behavior, color,
/// band and strength (`label::param_text`); a kind's surfaces by the
/// grammar's names, each with its behavior (`label::behavior_word`),
/// wrapped onto the label's lines ([`wrap_list`], board #5378): "walls:
/// wall 1 aurora · wall 6 aurora" over "wall 8 none · wall 14 none".
fn describe(
    target: Target,
    ks: &[usize],
    vocab: &Vocabulary<'_>,
    lanes: &RoomLanes,
    boxes: &[LaneBox<'_>],
) -> String {
    let name = target_name(target, vocab);
    match (target, ks) {
        (Target::Pointed | Target::Surface(_), &[k]) => {
            let behavior = lanes
                .effective(k, boxes)
                .map(|(b, _)| b)
                .unwrap_or_default();
            crate::label::param_text(&name, behavior, lanes.params_of(k, boxes))
        }
        _ => {
            let head = match target {
                Target::Kind(kind) => format!("{}:", crate::label::kind_plural(kind)),
                _ => format!("{name}:"),
            };
            let items: Vec<String> = ks
                .iter()
                .map(|&k| {
                    let name = vocab
                        .surface(k)
                        .map_or_else(|| friendly_name(k, boxes), |s| s.name.clone());
                    let runs = crate::label::behavior_word(lanes.effective(k, boxes));
                    format!("{name} {runs}")
                })
                .collect();
            if items.is_empty() {
                format!("{head} none in the room")
            } else {
                wrap_list(&head, &items, LIST_LINE_CHARS, crate::label::MAX_LINES)
            }
        }
    }
}

/// `head` and `items` on lines of at most `width` characters, as the
/// help puts its examples a line each: `head` starts the first line, the
/// items follow joined with " · ", never split (one wider than a line has
/// a line of its own). At most `max_lines`: the items past the last line
/// are counted at its end, "· 3 more".
fn wrap_list(head: &str, items: &[String], width: usize, max_lines: usize) -> String {
    // A line of items by index, the first after `head`, with `more` at
    // its end.
    let render = |k: usize, line: &[usize], more: usize| -> String {
        let mut words: Vec<String> = line.iter().map(|&i| items[i].clone()).collect();
        if more > 0 {
            words.push(format!("{more} more"));
        }
        let body = words.join(" \u{b7} ");
        if k == 0 {
            format!("{head} {body}")
        } else {
            body
        }
    };
    let fits =
        |k: usize, line: &[usize], more: usize| render(k, line, more).chars().count() <= width;
    let mut lines: Vec<Vec<usize>> = vec![Vec::new()];
    for i in 0..items.len() {
        let k = lines.len() - 1;
        let mut with = lines[k].clone();
        with.push(i);
        if lines[k].is_empty() || fits(k, &with, 0) {
            lines[k] = with;
        } else {
            lines.push(vec![i]);
        }
    }
    let max_lines = max_lines.max(1);
    let mut more = 0;
    if lines.len() > max_lines {
        more = lines[max_lines..].iter().map(Vec::len).sum();
        lines.truncate(max_lines);
        let k = max_lines - 1;
        while !lines[k].is_empty() && !fits(k, &lines[k], more) {
            lines[k].pop();
            more += 1;
        }
    }
    let last = lines.len() - 1;
    lines
        .iter()
        .enumerate()
        .map(|(k, line)| render(k, line, if k == last { more } else { 0 }))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `debug.fosfora.say` knob (board #3751): a sentence fed to the
/// grammar as if heard, for the unworn gate. A value is fed once, when it
/// differs from the last one fed; an app cannot clear a `debug.` property,
/// so to say the same sentence again, set it to "" first (or to another
/// sentence). While the room is on but has no anchors yet (`waiting`) the
/// value waits for them, as the `surface` knob's does. Board #5452: the
/// app does not hold back a sentence that begins a waiting show
/// ([`asks_to_begin`]), which names nothing in the room.
#[derive(Debug, Clone, Default)]
pub struct SayKnob {
    fed: Option<String>,
}

impl SayKnob {
    /// This poll's value (`None` or empty: unset): the sentence to feed
    /// now, if any.
    pub fn poll(&mut self, value: Option<&str>, waiting: bool) -> Option<String> {
        let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
            self.fed = None;
            return None;
        };
        if waiting || self.fed.as_deref() == Some(value) {
            return None;
        }
        self.fed = Some(value.to_owned());
        Some(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room_file::{STAGE_FLOOR_UUID, room_id};
    use SurfaceBehavior as B;

    const EFFECTS: [&str; 5] = ["Flux Cloud", "Embers", "Flock", "Tide Pool", "Night Sky"];

    fn effects() -> Vec<String> {
        EFFECTS.iter().map(|e| (*e).to_owned()).collect()
    }

    fn uuid(n: u8) -> [u8; 16] {
        std::array::from_fn(|i| if i == 0 { 0x40 + n } else { 0xb0 + i as u8 })
    }

    fn lane(n: u8, kind: u32, label: &'static str) -> LaneBox<'static> {
        LaneBox {
            uuid: uuid(n),
            kind,
            label,
        }
    }

    /// The fixture room: a desk, two tables, two walls, a floor, a
    /// ceiling and a lamp ("desk", "table 1", "table 2", "wall 3",
    /// "wall 4", "floor", "ceiling", "lamp").
    fn room() -> Vec<LaneBox<'static>> {
        vec![
            lane(0, KIND_TABLE, "DESK"),
            lane(1, KIND_TABLE, "TABLE"),
            lane(2, KIND_TABLE, "TABLE"),
            lane(3, KIND_WALL, "WALL_FACE"),
            lane(4, KIND_WALL, "WALL_FACE"),
            lane(5, KIND_FLOOR, "FLOOR"),
            lane(6, KIND_CEILING, "CEILING"),
            lane(7, KIND_OTHER, "LAMP"),
        ]
    }

    /// The fixture's vocabulary, the desk pointed at or `pointed`: the
    /// behaviors the app offers, as `app.rs` gives them.
    fn vocab<'a>(
        effects: &'a [String],
        boxes: &[LaneBox<'_>],
        pointed: Option<usize>,
    ) -> Vocabulary<'a> {
        Vocabulary {
            effects,
            behaviors: &SurfaceBehavior::OFFERED,
            surfaces: surfaces(boxes),
            pointed,
            tracks: Tracks::default(),
        }
    }

    /// Each sentence parses to its intent.
    fn check(v: &Vocabulary<'_>, cases: &[(&str, Intent)]) {
        for (sentence, want) in cases {
            assert_eq!(parse(sentence, v), Ok(*want), "\"{sentence}\"");
        }
    }

    /// Each sentence misses for its reason.
    fn missed(v: &Vocabulary<'_>, cases: &[(&str, Reason)]) {
        for (sentence, want) in cases {
            assert_eq!(
                parse(sentence, v).map_err(|m| m.reason),
                Err(want.clone()),
                "\"{sentence}\""
            );
        }
    }

    fn on(target: Target, behavior: SurfaceBehavior) -> Intent {
        Intent::Behavior { target, behavior }
    }

    fn color_of(target: Target, color: u32) -> Intent {
        Intent::Color { target, color }
    }

    fn band_of(target: Target, band: u32) -> Intent {
        Intent::Band { target, band }
    }

    fn strength_of(target: Target, strength: Strength) -> Intent {
        Intent::Strength { target, strength }
    }

    #[test]
    fn the_fixture_names_its_surfaces_as_the_editor_does() {
        let names: Vec<String> = surfaces(&room()).into_iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            [
                "desk", "table 1", "table 2", "wall 3", "wall 4", "floor", "ceiling", "lamp"
            ]
        );
    }

    #[test]
    fn the_menu_rows_and_buttons() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        check(
            &v,
            &[
                ("Next effect.", Intent::NextEffect),
                ("next", Intent::NextEffect),
                ("Previous effect", Intent::PrevEffect),
                ("last effect", Intent::PrevEffect),
                ("Embers.", Intent::Effect(1)),
                ("Switch to Flux Cloud.", Intent::Effect(0)),
                ("show flock", Intent::Effect(2)),
                ("Night sky", Intent::Effect(4)),
                ("Edit the room.", Intent::EditRoom(true)),
                ("Stop editing.", Intent::EditRoom(false)),
                ("Done editing", Intent::EditRoom(false)),
                ("Cloud off.", Intent::Cloud(false)),
                ("Particles on", Intent::Cloud(true)),
                ("hide the particles", Intent::Cloud(false)),
                ("Turn the cloud on.", Intent::Cloud(true)),
                ("pitcher on", Intent::Pitcher(true)),
                ("Music play.", Intent::Music(true)),
                ("music stop", Intent::Music(false)),
                ("Play the music.", Intent::Music(true)),
                ("Turn off the music.", Intent::Music(false)),
                ("Rescan the room.", Intent::Rescan),
                ("scan the room", Intent::Rescan),
                ("Recenter the cloud.", Intent::Recenter),
                ("Re-center the cloud", Intent::Recenter),
                ("Clear everything.", Intent::AllNone),
                ("Nothing anywhere", Intent::AllNone),
                ("all none", Intent::AllNone),
            ],
        );
    }

    /// Board #5393: roam on and off by voice, with the menu's other words
    /// unchanged around them.
    #[test]
    fn roam_on_and_off() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        check(
            &v,
            &[
                ("Roam.", Intent::Roam(true)),
                ("Roam mode", Intent::Roam(true)),
                ("Follow me.", Intent::Roam(true)),
                ("Roam on", Intent::Roam(true)),
                ("Start roaming.", Intent::Roam(true)),
                ("Stay here.", Intent::Roam(false)),
                ("Seated", Intent::Roam(false)),
                ("Stop roaming.", Intent::Roam(false)),
                ("Roam off", Intent::Roam(false)),
                ("Seated mode.", Intent::Roam(false)),
                ("Recenter the cloud.", Intent::Recenter),
                ("Rescan the room.", Intent::Rescan),
            ],
        );
        // In a sentence of clauses, beside the menu's.
        let clauses = parse_clauses("Follow me and lights off.", &v);
        let outcomes: Vec<_> = clauses.into_iter().map(|c| c.outcome).collect();
        assert_eq!(
            outcomes,
            [
                Ok(Intent::Roam(true)),
                Ok(Intent::Immersion(ImmersionStep::To(
                    crate::immerse::Depth::Full
                )))
            ]
        );
        assert_eq!(reply(&Intent::Roam(true), &v), "roam on");
        assert_eq!(reply(&Intent::Roam(false), &v), "roam off");
        // Not the agent's: its actions have no roam.
        let action = AgentAction {
            kind: "roam".to_owned(),
            target: String::new(),
            value: "on".to_owned(),
        };
        assert!(Intent::from_json(&action, &v).is_err());
    }

    /// Board #5452: the show begun by voice, beside the phrases that start
    /// with the same words; not the agent's.
    #[test]
    fn begin_the_show() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        check(
            &v,
            &[
                ("Begin.", Intent::Begin),
                ("Start", Intent::Begin),
                ("Begin the show.", Intent::Begin),
                ("Start the show, please.", Intent::Begin),
                ("Let's begin.", Intent::Begin),
                ("Let's start", Intent::Begin),
                // The phrases the grammar had keep their meaning.
                ("Start editing.", Intent::EditRoom(true)),
                ("Start roaming", Intent::Roam(true)),
                ("Play music.", Intent::Music(true)),
                ("Next.", Intent::NextEffect),
            ],
        );
        for said in ["begin", " Start. ", "Let's begin!", "start the show"] {
            assert!(asks_to_begin(said), "{said}");
        }
        for said in ["", "start editing", "begin the beguine", "next effect"] {
            assert!(!asks_to_begin(said), "{said}");
        }
        assert_eq!(reply(&Intent::Begin, &v), BEGIN_REPLY);
        assert_eq!(BEGIN_REPLY, "starting");
        assert_eq!(BEGUN_REPLY, "already started");
        // With another command in the sentence, both apply.
        let clauses = parse_clauses("Begin and theme aurora.", &v);
        let outcomes: Vec<_> = clauses.into_iter().map(|c| c.outcome).collect();
        assert_eq!(
            outcomes,
            [
                Ok(Intent::Begin),
                Ok(Intent::Theme(ThemeStep::To(crate::themes::Theme::Aurora)))
            ]
        );
        // Not the agent's: its actions have no begin.
        let action = AgentAction {
            kind: "begin".to_owned(),
            target: String::new(),
            value: String::new(),
        };
        assert!(Intent::from_json(&action, &v).is_err());
        // "What can I say" does not list it: by then the show is on.
        assert!(!shown_pool(&v).iter().any(|p| asks_to_begin(p)));
    }

    /// Board #5459: the hand menu opened ahead and closed by voice, beside
    /// the menu's other words; not the agent's.
    #[test]
    fn open_and_close_the_menu() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        check(
            &v,
            &[
                ("Menu.", Intent::Menu(true)),
                ("Open the menu.", Intent::Menu(true)),
                ("Show the menu", Intent::Menu(true)),
                ("Open the hand menu, please.", Intent::Menu(true)),
                ("Bring up the menu.", Intent::Menu(true)),
                ("Close the menu.", Intent::Menu(false)),
                ("Hide the menu", Intent::Menu(false)),
                ("Close the hand menu.", Intent::Menu(false)),
                ("Menu off.", Intent::Menu(false)),
                ("Next effect.", Intent::NextEffect),
                ("Show flock", Intent::Effect(2)),
                ("Recenter the cloud.", Intent::Recenter),
            ],
        );
        let clauses = parse_clauses("Open the menu and next song.", &v);
        let outcomes: Vec<_> = clauses.into_iter().map(|c| c.outcome).collect();
        assert_eq!(outcomes, [Ok(Intent::Menu(true)), Ok(Intent::NextTrack)]);
        assert_eq!(reply(&Intent::Menu(true), &v), "menu open");
        assert_eq!(reply(&Intent::Menu(false), &v), "menu closed");
        // "What can I say" lists them, the open on the first page.
        let pool = shown_pool(&v);
        for said in ["open the menu", "close the menu", "menu"] {
            assert!(pool.iter().any(|p| p == said), "{said}: {pool:?}");
        }
        assert!(examples(&v, 0).iter().any(|p| p == "open the menu"));
        let action = AgentAction {
            kind: "menu".to_owned(),
            target: String::new(),
            value: "on".to_owned(),
        };
        assert!(Intent::from_json(&action, &v).is_err());
    }

    /// Board #5453: the Edges control's every stop, the Agent control's
    /// stops on the release surface, the Voice toggle, and the music by
    /// one word, with the menu's other words unchanged around them.
    #[test]
    fn every_menu_control_can_be_said() {
        use crate::edge_bars::{Choice, Mode};
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        let edges = |m| Intent::Edges(Choice::Force(m));
        check(
            &v,
            &[
                ("Edges theme.", Intent::Edges(Choice::Theme)),
                ("edges on", Intent::Edges(Choice::Theme)),
                ("Show the edges.", Intent::Edges(Choice::Theme)),
                ("Edges off.", edges(Mode::Off)),
                ("Turn the edges off.", edges(Mode::Off)),
                ("turn off the edge bars", edges(Mode::Off)),
                ("Hide the edges", edges(Mode::Off)),
                ("No edges.", edges(Mode::Off)),
                ("Edges face.", edges(Mode::Face)),
                ("edges faces", edges(Mode::Face)),
                ("Edges stand.", edges(Mode::Standing)),
                ("Standing edges", edges(Mode::Standing)),
                ("Edges in.", edges(Mode::In)),
                ("edges inside", edges(Mode::In)),
                ("Set the edges to tunnel.", edges(Mode::In)),
                ("Agent on.", Intent::Agent(AgentAsk::On)),
                ("Turn on the agent.", Intent::Agent(AgentAsk::On)),
                ("Agent off.", Intent::Agent(AgentAsk::Off)),
                ("Turn the agent off.", Intent::Agent(AgentAsk::Off)),
                ("Stop the agent.", Intent::Agent(AgentAsk::Off)),
                ("Agent local.", Intent::Agent(AgentAsk::Local)),
                ("Local agent", Intent::Agent(AgentAsk::Local)),
                ("Use the local agent.", Intent::Agent(AgentAsk::Local)),
                ("use local", Intent::Agent(AgentAsk::Local)),
                ("Agent on-device.", Intent::Agent(AgentAsk::Local)),
                ("Voice off.", Intent::Voice(false)),
                ("Turn off the voice.", Intent::Voice(false)),
                ("voice on", Intent::Voice(true)),
                ("Stop.", Intent::Music(false)),
                ("Pause.", Intent::Music(false)),
                ("Play.", Intent::Music(true)),
                // Unchanged.
                ("music stop", Intent::Music(false)),
                ("play music", Intent::Music(true)),
                ("Stop editing.", Intent::EditRoom(false)),
                ("Voice help.", Intent::Help),
                ("Show flock.", Intent::Effect(2)),
                ("next effect", Intent::NextEffect),
                (
                    "theme aurora",
                    Intent::Theme(ThemeStep::To(crate::themes::Theme::Aurora)),
                ),
                (
                    "the desk in amber",
                    Intent::Color {
                        target: Target::Surface(0),
                        color: 4,
                    },
                ),
            ],
        );
        // A word that is no stop, and "local" without a verb, are not the
        // controls'.
        for said in ["edges sideways", "edges", "local"] {
            assert!(
                !matches!(
                    parse(said, &v),
                    Ok(Intent::Edges(_) | Intent::Agent(_) | Intent::Voice(_))
                ),
                "{said}"
            );
        }
        // Two clauses, as any other control.
        let clauses = parse_clauses("edges off and voice off", &v);
        assert_eq!(
            clauses
                .iter()
                .map(|c| c.outcome.clone().unwrap())
                .collect::<Vec<_>>(),
            [edges(Mode::Off), Intent::Voice(false)]
        );
    }

    #[test]
    fn the_new_controls_reply_what_they_did() {
        use crate::agent::{Pick, ProviderKind};
        use crate::edge_bars::{Choice, Mode};
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        for (intent, want) in [
            (Intent::Edges(Choice::Theme), "edges: theme"),
            (Intent::Edges(Choice::Force(Mode::Off)), "edges: off"),
            (Intent::Edges(Choice::Force(Mode::Face)), "edges: face"),
            (Intent::Edges(Choice::Force(Mode::Standing)), "edges: stand"),
            (Intent::Edges(Choice::Force(Mode::In)), "edges: in"),
            (Intent::Voice(true), "voice on"),
            (
                Intent::Voice(false),
                "voice off\nTurn it back on in the hand menu",
            ),
            (Intent::Music(false), "music off"),
            (Intent::Music(true), "music on"),
        ] {
            assert_eq!(reply(&intent, &v), want, "{intent:?}");
        }
        let ok = |pick: Option<Pick>, label: &str| (pick, label.to_owned());
        // The release surface's stops: local and off.
        let release = [Pick::Local, Pick::Off];
        assert_eq!(
            agent_step(AgentAsk::Off, &release, Pick::Local),
            ok(Some(Pick::Off), "agent: off")
        );
        assert_eq!(
            agent_step(AgentAsk::On, &release, Pick::Off),
            ok(Some(Pick::Local), "agent: local")
        );
        assert_eq!(
            agent_step(AgentAsk::Local, &release, Pick::Off),
            ok(Some(Pick::Local), "agent: local")
        );
        // Already there: nothing changes, and the label says where it is.
        assert_eq!(
            agent_step(AgentAsk::On, &release, Pick::Local),
            ok(None, "agent: local")
        );
        assert_eq!(
            agent_step(AgentAsk::Off, &release, Pick::Off),
            ok(None, "agent: off")
        );
        // A network provider configured too: "on" keeps it, "local" moves.
        let net = Pick::Network(ProviderKind::Anthropic);
        let both = [Pick::Local, net, Pick::Off];
        assert_eq!(
            agent_step(AgentAsk::On, &both, net),
            ok(None, "agent: Anthropic")
        );
        assert_eq!(
            agent_step(AgentAsk::Local, &both, net),
            ok(Some(Pick::Local), "agent: local")
        );
        // Only the network provider: "on" takes it, "local" cannot.
        let only = [net, Pick::Off];
        assert_eq!(
            agent_step(AgentAsk::On, &only, Pick::Off),
            ok(Some(net), "agent: Anthropic")
        );
        assert_eq!(
            agent_step(AgentAsk::Local, &only, Pick::Off),
            ok(None, NO_LOCAL_REPLY)
        );
        // "Agent: none": nothing to pick, whatever is asked.
        for ask in [AgentAsk::On, AgentAsk::Off, AgentAsk::Local] {
            assert_eq!(agent_step(ask, &[], Pick::Off), ok(None, NO_AGENT_REPLY));
        }
        // The cycle as the control builds it.
        let available = crate::agent::Available::new(Ok(None), Ok(()));
        assert_eq!(available.picks(), release);
    }

    /// Board #5453: "what can I say" lists the menu's last three controls
    /// among its examples, and paging through it shows each.
    #[test]
    fn the_help_lists_every_menu_control() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        let shown = shown_pool(&v);
        for said in ["edges off", "agent off", "voice off"] {
            assert!(shown.iter().any(|s| s == said), "{said}: {shown:?}");
        }
        let seen: Vec<String> = (0..shown.len()).flat_map(|p| examples(&v, p)).collect();
        for said in ["edges off", "agent off", "voice off"] {
            assert!(seen.iter().any(|s| s == said), "{said}");
        }
    }

    /// Board #4198: "credits" shows the credits card on the help's card,
    /// as long, six lines or fewer, each as short as an example; the help
    /// lists it last, so every line it had keeps its place.
    #[test]
    fn credits_show_on_the_card_and_the_help_lists_it_last() {
        let dir =
            std::env::temp_dir().join(format!("fosfora-intent-credits-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (e, boxes) = (effects(), room());
        let id = room_id(boxes.iter().map(|b| b.uuid));
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        let v = vocab(&e, &boxes, Some(0));
        for said in [
            "Credits.",
            "Show the credits",
            "licenses",
            "License.",
            "About.",
            "About the app",
            "about this app",
            "About Fosfora",
        ] {
            assert_eq!(parse(said, &v), Ok(Intent::Credits), "{said}");
        }
        let mut page = 0;
        let (r, line) = respond_on("Credits.", &v, &mut lanes, &boxes, false, &mut page);
        assert_eq!(r.label, CREDITS.join("\n"));
        assert_close!(r.seconds, CARD_S);
        assert_eq!(r.agent, None);
        assert_eq!(page, 0, "the help's page stays");
        assert!(line.contains("Credits"), "{line}");
        // Six lines or fewer, none wrapped, each fitting the card.
        assert!(CREDITS.len() <= crate::label::MAX_LINES);
        let laid = crate::label::layout(&r.label);
        assert_eq!(laid.lines, CREDITS.map(str::to_owned));
        assert!(laid.px >= crate::label::MIN_FONT_PX, "{}", laid.px);
        for l in CREDITS {
            assert!(l.chars().count() <= MAX_EXAMPLE_CHARS, "{l}");
        }
        // No wider than the widest line the help's card shows for this
        // room, so it reads at least as large as the help at its widest.
        let em = crate::text_width::em;
        let widest_help = shown_pool(&v).iter().map(|l| em(l)).fold(0.0, f32::max);
        let widest = CREDITS.iter().map(|l| em(l)).fold(0.0, f32::max);
        assert!(widest <= widest_help, "{widest} vs {widest_help}");
        // The help lists it, last of the menu's phrases: the pool is the
        // one it was with "credits" added, nothing dropped.
        let pool = shown_pool(&v);
        assert!(pool.iter().any(|p| p == "credits"), "{pool:?}");
        let seen: Vec<String> = (0..pool.len()).flat_map(|p| examples(&v, p)).collect();
        assert!(seen.iter().any(|s| s == "credits"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Board #5453: the card stays until the next window or 30 s; a list
    /// and the two-line "voice off" stay as long as before, a one-line
    /// reply 1.5 s.
    #[test]
    fn the_card_stays_until_the_next_window_or_30_s() {
        let dir = std::env::temp_dir().join(format!("fosfora-intent-card-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (e, boxes) = (effects(), room());
        let id = room_id(boxes.iter().map(|b| b.uuid));
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        let v = vocab(&e, &boxes, Some(0));
        let mut page = 0;
        let mut say = |sentence: &str| {
            let (r, _) = respond_on(sentence, &v, &mut lanes, &boxes, false, &mut page);
            r
        };
        let card = say("What can I say?");
        assert_eq!(card.label.lines().count(), HELP_LINES);
        assert_close!(card.seconds, CARD_S);
        // With another clause, still the card.
        let r = say("Next theme and what can I say?");
        assert!(r.label.starts_with("theme: Drift\n"), "{}", r.label);
        assert_close!(r.seconds, CARD_S);
        let r = say("what are the walls");
        assert_close!(r.seconds, HELP_S);
        let r = say("voice off");
        assert_eq!(r.label, VOICE_OFF_REPLY);
        assert_close!(r.seconds, HELP_S);
        let r = say("edges off");
        assert_eq!(r.label, "edges: off");
        assert_close!(r.seconds, REPLY_S);
        // On the label: up for 30 s, its last part fading; the next
        // window's "Listening…" takes its place at once.
        let dt = 1.0 / 72.0;
        let mut l = crate::label::Label::default();
        l.show_for(
            card.label.clone(),
            glam::Vec3::ZERO,
            glam::Vec3::Z,
            card.seconds,
        );
        for _ in 0..(29.5 / dt) as usize {
            l.step(dt);
        }
        assert_eq!(l.now().map(|f| f.text), Some(card.label.as_str()));
        for _ in 0..(0.6 / dt) as usize {
            l.step(dt);
        }
        assert_eq!(l.now(), None);
        l.show_for(
            card.label.clone(),
            glam::Vec3::ZERO,
            glam::Vec3::Z,
            card.seconds,
        );
        l.step(dt);
        l.show_for(
            crate::voice::LISTENING_LABEL.to_owned(),
            glam::Vec3::ZERO,
            glam::Vec3::Z,
            crate::voice::FIST_MAX_S + 1.0,
        );
        assert_eq!(l.now().map(|f| f.text), Some(crate::voice::LISTENING_LABEL));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Board #5415: a theme by name, the next one, and an unknown name,
    /// with the menu's and the surfaces' words unchanged around them.
    #[test]
    fn a_theme_by_name_the_next_one_and_an_unknown_name() {
        use crate::themes::Theme;
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        let to = |t| Intent::Theme(ThemeStep::To(t));
        let next = Intent::Theme(ThemeStep::Next);
        check(
            &v,
            &[
                ("Theme drift.", to(Theme::Drift)),
                ("theme: Aurora", to(Theme::Aurora)),
                ("Drift theme", to(Theme::Drift)),
                ("Set the theme to aurora.", to(Theme::Aurora)),
                ("Change the theme to Tide.", to(Theme::Tide)),
                ("Use the classic theme please", to(Theme::Classic)),
                ("Switch to the Tide theme.", to(Theme::Tide)),
                ("Next theme.", next),
                ("Another theme", next),
                ("Change the theme.", next),
                ("theme next", next),
                ("Set the theme to the next one.", next),
                // Board #5431: Mirror through the same grammar.
                ("Mirror theme.", to(Theme::Mirror)),
                ("Set the theme to mirror", to(Theme::Mirror)),
                // The words around them still mean what they did.
                ("Next effect.", Intent::NextEffect),
                ("next", Intent::NextEffect),
                ("Switch to Tide Pool.", Intent::Effect(3)),
                ("Every wall aurora.", on(Target::Kind(KIND_WALL), B::Aurora)),
            ],
        );
        missed(
            &v,
            &[
                ("Theme sunset.", Reason::UnknownTheme("sunset".to_owned())),
                (
                    "Set the theme to blue sky",
                    Reason::UnknownTheme("blue sky".to_owned()),
                ),
                (
                    "Tide pool theme",
                    Reason::UnknownTheme("tide pool".to_owned()),
                ),
            ],
        );
        // The unknown name's reply lists the themes.
        let Err(miss) = parse("Theme sunset.", &v) else {
            panic!("parsed");
        };
        assert_eq!(
            miss_text(&miss),
            "No theme called \"sunset\": Classic, Drift, Tide, Aurora or Mirror"
        );
        // Not the agent's to take: the reply stands.
        assert!(!crate::agent::forwards(&miss.reason));
        // In a sentence of clauses, beside the menu's.
        let clauses = parse_clauses("Theme tide and next effect.", &v);
        let outcomes: Vec<_> = clauses.into_iter().map(|c| c.outcome).collect();
        assert_eq!(outcomes, [Ok(to(Theme::Tide)), Ok(Intent::NextEffect)]);
        // The agent's actions have no theme: its kind misses.
        let action = AgentAction {
            kind: "theme".to_owned(),
            target: String::new(),
            value: "drift".to_owned(),
        };
        assert_eq!(
            Intent::from_json(&action, &v).map_err(|m| m.reason),
            Err(Reason::NoMatch)
        );
    }

    /// Board #5415: what a spoken theme does to the room, and the reply
    /// naming the theme it came to.
    #[test]
    fn a_spoken_theme_applies_and_the_reply_names_it() {
        use crate::themes::{Shown, Theme};
        let dir = std::env::temp_dir().join(format!("fosfora-intent-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (e, boxes) = (effects(), room());
        let id = room_id(boxes.iter().map(|b| b.uuid));
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        let v = vocab(&e, &boxes, Some(0));
        let mut page = 0;
        let mut say = |sentence: &str, lanes: &mut RoomLanes| {
            respond_on(sentence, &v, lanes, &boxes, false, &mut page).0
        };
        // A hand assignment first: the theme wins over it.
        assert_eq!(say("Amber on the desk.", &mut lanes).label, "desk: amber");
        let r = say("Set the theme to aurora.", &mut lanes);
        assert_eq!(r.label, "theme: Aurora");
        assert_close!(r.seconds, REPLY_S);
        assert_eq!(lanes.theme(), Shown::Theme(Theme::Aurora));
        let aurora = Theme::Aurora.look(KIND_TABLE).unwrap();
        assert_eq!(
            lanes.params_of(0, &boxes),
            (aurora.color, aurora.band, aurora.strength)
        );
        // "next theme" names the one it stepped to.
        assert_eq!(say("Next theme.", &mut lanes).label, "theme: Mirror");
        assert_eq!(say("next theme", &mut lanes).label, "theme: Classic");
        assert_eq!(say("next theme", &mut lanes).label, "theme: Drift");
        assert_eq!(lanes.theme(), Shown::Theme(Theme::Drift));
        // An edit by voice after it: edited, and the next press goes on.
        say("The desk in rose.", &mut lanes);
        assert_eq!(lanes.theme(), Shown::Edited(Theme::Drift));
        assert_eq!(say("Another theme.", &mut lanes).label, "theme: Tide");
        // An unknown name changes nothing and lists the themes.
        let r = say("Theme sunset.", &mut lanes);
        assert_eq!(
            r.label,
            "No theme called \"sunset\": Classic, Drift, Tide, Aurora or Mirror"
        );
        assert_close!(r.seconds, MISS_S);
        assert_eq!(lanes.theme(), Shown::Theme(Theme::Tide));
        // The replies without the room, as `reply` words them.
        assert_eq!(
            reply(&Intent::Theme(ThemeStep::To(Theme::Drift)), &v),
            "theme: Drift"
        );
        assert_eq!(theme_reply(Theme::Classic), "theme: Classic");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Board #5384: the room darker and lighter, the lights on and off;
    /// a surface's dimmer still needs the surface.
    #[test]
    fn the_room_darker_lighter_and_the_lights() {
        use crate::immerse::Depth::{Full, Low, Off};
        use ImmersionStep::{Darker, Lighter, To};
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        let im = Intent::Immersion;
        check(
            &v,
            &[
                ("Darker.", im(Darker)),
                ("Make the room darker.", im(Darker)),
                ("Dim the lights", im(Darker)),
                ("Turn the lights down.", im(Darker)),
                ("More immersion", im(Darker)),
                ("Lighter.", im(Lighter)),
                ("Make the room lighter", im(Lighter)),
                ("Lights up", im(Lighter)),
                ("Lights on.", im(To(Off))),
                ("Turn on the lights.", im(To(Off))),
                ("Turn the lights on", im(To(Off))),
                ("Lights off.", im(To(Full))),
                ("Turn off the lights.", im(To(Full))),
                ("Lights out", im(To(Full))),
                ("Full dark.", im(To(Full))),
                ("Immersion low.", im(To(Low))),
                ("immersion half", im(To(Low))),
                ("Immersion off", im(To(Off))),
                ("full immersion", im(To(Full))),
                ("Immersion on.", im(To(Full))),
                // A surface named, or "it": the surface's strength, as
                // before.
                (
                    "The desk darker.",
                    strength_of(Target::Surface(0), Strength::Down),
                ),
                (
                    "Make it darker",
                    strength_of(Target::Pointed, Strength::Down),
                ),
                ("dimmer", strength_of(Target::Pointed, Strength::Down)),
                ("brighter", strength_of(Target::Pointed, Strength::Up)),
            ],
        );
        // No depth word: not the room's (the surface templates take them
        // and find no surface by that name).
        for sentence in ["Immersion purple.", "Lights blue.", "Turn immersion"] {
            assert!(parse(sentence, &v).is_err(), "\"{sentence}\"");
        }
        // In a sentence of clauses, beside the menu's.
        let clauses = parse_clauses("Lights off and next effect.", &v);
        let outcomes: Vec<_> = clauses.into_iter().map(|c| c.outcome).collect();
        assert_eq!(outcomes, [Ok(im(To(Full))), Ok(Intent::NextEffect)]);
        // The steps from each depth, held at the ends.
        assert_eq!(
            [Off, Low, Full, crate::immerse::Depth::Black].map(|d| Darker.resolve(d)),
            [
                Low,
                Full,
                crate::immerse::Depth::Black,
                crate::immerse::Depth::Black
            ]
        );
        // Black by name.
        assert_eq!(
            parse("Immersion black.", &v),
            Ok(im(To(crate::immerse::Depth::Black)))
        );
        assert_eq!(
            [Off, Low, Full].map(|d| Lighter.resolve(d)),
            [Off, Off, Low]
        );
        assert_eq!(To(Low).resolve(Full), Low);
        // The replies: the depth it came to, in the menu's words.
        assert_eq!(reply(&im(To(Full)), &v), "immersion full");
        assert_eq!(reply(&im(To(Off)), &v), "immersion off");
        assert_eq!(reply(&im(Darker), &v), "darker");
        assert_eq!(immersion_reply(Low), "immersion low");
        // The agent's actions have no immersion (the on-device model's
        // kinds are a closed set): its kind misses.
        let action = AgentAction {
            kind: "immersion".to_owned(),
            target: String::new(),
            value: "full".to_owned(),
        };
        assert_eq!(
            Intent::from_json(&action, &v).map_err(|m| m.reason),
            Err(Reason::NoMatch)
        );
    }

    /// The music folders' titles of the track tests (board #5385).
    fn songs() -> Vec<String> {
        [
            "One More Time",
            "Harder, Better, Faster, Stronger",
            "Don't Stop Me Now",
            "Stop and Stare",
            "Rock and Roll",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    #[test]
    fn the_music_rows_steps_and_a_song_by_name() {
        let (e, boxes, titles) = (effects(), room(), songs());
        let mut v = vocab(&e, &boxes, Some(0));
        v.tracks = Tracks {
            titles: &titles,
            current: Some(0),
            playing: true,
            ..Tracks::default()
        };
        check(
            &v,
            &[
                ("Next song.", Intent::NextTrack),
                ("skip", Intent::NextTrack),
                ("Skip to the next song.", Intent::NextTrack),
                ("next track please", Intent::NextTrack),
                ("Previous song.", Intent::PrevTrack),
                ("last track", Intent::PrevTrack),
                ("Go back a song.", Intent::PrevTrack),
                ("What's playing?", Intent::NowPlaying),
                ("What is playing", Intent::NowPlaying),
                ("What song is this?", Intent::NowPlaying),
                ("What am I listening to?", Intent::NowPlaying),
                ("Scan my music.", Intent::ScanMusic),
                ("look for new songs", Intent::ScanMusic),
                ("Play One More Time.", Intent::Track(0)),
                ("play harder better faster stronger", Intent::Track(1)),
                ("Play the song Don't Stop Me Now", Intent::Track(2)),
                ("play stare", Intent::Track(3)),
                ("Play rock and roll.", Intent::Track(4)),
                // The toggle and "play" naming no song keep the music's.
                ("play music", Intent::Music(true)),
                ("Play some music.", Intent::Music(true)),
                ("play something", Intent::Music(true)),
                // The effect row and the surface question are unchanged.
                ("next", Intent::NextEffect),
                ("last effect", Intent::PrevEffect),
                ("what is this", Intent::Describe(Target::Pointed)),
            ],
        );
        missed(
            &v,
            &[
                ("play stairway", Reason::NoTrack("stairway".to_owned())),
                (
                    "play stop",
                    Reason::AmbiguousTrack(vec![
                        "Don't Stop Me Now".to_owned(),
                        "Stop and Stare".to_owned(),
                    ]),
                ),
            ],
        );
        // No tracks: a name finds none, and the steps still parse.
        v.tracks = Tracks::default();
        missed(
            &v,
            &[(
                "play one more time",
                Reason::NoTrack("1 more time".to_owned()),
            )],
        );
        check(&v, &[("next song", Intent::NextTrack)]);
    }

    #[test]
    fn the_music_replies_name_the_track() {
        let (e, boxes, titles) = (effects(), room(), songs());
        let mut v = vocab(&e, &boxes, Some(0));
        v.tracks = Tracks {
            titles: &titles,
            current: Some(0),
            playing: true,
            ..Tracks::default()
        };
        assert_eq!(
            reply(&Intent::NextTrack, &v),
            "next: Harder, Better, Faster, Stronger"
        );
        assert_eq!(reply(&Intent::PrevTrack, &v), "previous: Rock and Roll");
        assert_eq!(reply(&Intent::Track(2), &v), "playing Don't Stop Me Now");
        assert_eq!(reply(&Intent::NowPlaying, &v), "playing: One More Time");
        v.tracks.playing = false;
        assert_eq!(reply(&Intent::NowPlaying, &v), "stopped: One More Time");
        // On the test clip.
        v.tracks.current = None;
        assert_eq!(reply(&Intent::NowPlaying, &v), "music off");
        assert_eq!(reply(&Intent::NextTrack, &v), "next: One More Time");
        v.tracks.playing = true;
        assert_eq!(reply(&Intent::NowPlaying, &v), "playing the test clip");
        // With no music of the wearer's own the tracks are the bundled
        // ones (board #5528): a step names the one it goes to, whether or
        // not the shared folder is read, and never says "no songs".
        let bundled = ["Bundled One".to_owned(), "Bundled Two".to_owned()];
        for blocked in [false, true] {
            v.tracks = Tracks {
                titles: &bundled,
                current: Some(0),
                playing: true,
                blocked,
            };
            assert_eq!(reply(&Intent::NextTrack, &v), "next: Bundled Two");
            assert_eq!(reply(&Intent::PrevTrack, &v), "previous: Bundled Two");
            assert_eq!(reply(&Intent::NowPlaying, &v), "playing: Bundled One");
        }
        // No songs (board #5461): where to put them, or the permission
        // when the shared folder goes unread for want of it; the words of
        // the Music row's `<` `>`. Only with no bundled track either.
        v.tracks = Tracks::default();
        assert_eq!(
            reply(&Intent::NextTrack, &v),
            "No songs on the headset. Put them in its Music folder."
        );
        assert_eq!(reply(&Intent::PrevTrack, &v), crate::music::no_songs(false));
        v.tracks.blocked = true;
        assert_eq!(
            reply(&Intent::NextTrack, &v),
            "Can't read the Music folder. Allow music files in Settings."
        );
        assert_eq!(reply(&Intent::PrevTrack, &v), crate::music::no_songs(true));
        assert_eq!(
            miss_text(&Miss {
                reason: Reason::AmbiguousTrack(vec!["A".to_owned(), "B".to_owned()]),
                heard: "play x".to_owned(),
            }),
            "Which one: A or B?"
        );
        assert_eq!(
            miss_text(&Miss {
                reason: Reason::NoTrack("stairway".to_owned()),
                heard: "play stairway".to_owned(),
            }),
            "No song called \"stairway\""
        );
        // A song's miss is not a surface's: the surface labels stay down.
        assert!(!Reason::NoTrack(String::new()).names_surfaces());
        assert!(!Reason::AmbiguousTrack(Vec::new()).names_surfaces());
    }

    #[test]
    fn a_title_with_and_in_it_does_not_split_its_sentence() {
        let (e, boxes, titles) = (effects(), room(), songs());
        let mut v = vocab(&e, &boxes, Some(0));
        v.tracks = Tracks {
            titles: &titles,
            current: Some(0),
            playing: true,
            ..Tracks::default()
        };
        let clauses = parse_clauses("play Rock and Roll and next effect", &v);
        let outcomes: Vec<_> = clauses.into_iter().map(|c| c.outcome).collect();
        assert_eq!(outcomes, [Ok(Intent::Track(4)), Ok(Intent::NextEffect)]);
    }

    #[test]
    fn a_behavior_on_a_surface_a_kind_or_the_pointed_one() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        check(
            &v,
            &[
                (
                    "Streamlines on table 2.",
                    on(Target::Surface(2), B::Streamlines),
                ),
                ("the desk curls", on(Target::Surface(0), B::Curls)),
                ("Put curls on the lamp.", on(Target::Surface(7), B::Curls)),
                // Board #5462: embers' particles are gone; the curls stand
                // in, and sparks' the pulse.
                ("Embers on the desk.", on(Target::Surface(0), B::Curls)),
                ("Aurora on the ceiling", on(Target::Surface(6), B::Aurora)),
                (
                    "Every table streamlines.",
                    on(Target::Kind(KIND_TABLE), B::Streamlines),
                ),
                (
                    "all the walls spectrum",
                    on(Target::Kind(KIND_WALL), B::Spectrum),
                ),
                ("walls bezel", on(Target::Kind(KIND_WALL), B::Bezel)),
                ("Nothing on the ceiling.", on(Target::Surface(6), B::None)),
                ("Clear the walls.", on(Target::Kind(KIND_WALL), B::None)),
                ("the desk off", on(Target::Surface(0), B::None)),
                ("Pulse.", on(Target::Pointed, B::Pulse)),
                ("streamline on this", on(Target::Pointed, B::Streamlines)),
                ("that sparks", on(Target::Pointed, B::Pulse)),
                ("ripple on the floor", on(Target::Surface(5), B::Rings)),
            ],
        );
    }

    #[test]
    fn a_color_a_band_and_a_strength() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        check(
            &v,
            &[
                ("The desk in amber.", color_of(Target::Surface(0), 4)),
                ("Amber on the desk", color_of(Target::Surface(0), 4)),
                ("Make the lamp teal.", color_of(Target::Surface(7), 6)),
                ("every wall in rose", color_of(Target::Kind(KIND_WALL), 7)),
                ("In key.", color_of(Target::Pointed, COLOR_KEY)),
                ("the desk in its own color", color_of(Target::Surface(0), 0)),
                ("desk warm white", color_of(Target::Surface(0), 3)),
                ("table 1 purple", color_of(Target::Surface(1), 2)),
                ("Wall 3 on the bass.", band_of(Target::Surface(3), 1)),
                ("Table 2 follows the highs.", band_of(Target::Surface(2), 3)),
                ("On the mids.", band_of(Target::Pointed, 2)),
                ("the floor on the base", band_of(Target::Surface(5), 1)),
                ("lamp on the volume", band_of(Target::Surface(7), 0)),
                (
                    "The desk dimmer.",
                    strength_of(Target::Surface(0), Strength::Down),
                ),
                ("Brighter.", strength_of(Target::Pointed, Strength::Up)),
                (
                    "dimmer on the lamp",
                    strength_of(Target::Surface(7), Strength::Down),
                ),
                (
                    "table 1 half",
                    strength_of(Target::Surface(1), Strength::Half),
                ),
                (
                    "every table full",
                    strength_of(Target::Kind(KIND_TABLE), Strength::Full),
                ),
                (
                    "the desk strength off",
                    strength_of(Target::Surface(0), Strength::Off),
                ),
                (
                    "half strength",
                    strength_of(Target::Pointed, Strength::Half),
                ),
            ],
        );
    }

    #[test]
    fn describe_the_pointed_or_a_named_surface() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        check(
            &v,
            &[
                ("What is this?", Intent::Describe(Target::Pointed)),
                ("What's the lamp?", Intent::Describe(Target::Surface(7))),
                ("describe the ceiling", Intent::Describe(Target::Surface(6))),
                ("what is on wall 4", Intent::Describe(Target::Surface(4))),
                // Board #5378: a kind's list, asked the ways a wearer
                // asks it.
                (
                    "Describe the tables.",
                    Intent::Describe(Target::Kind(KIND_TABLE)),
                ),
                (
                    "What are the walls?",
                    Intent::Describe(Target::Kind(KIND_WALL)),
                ),
                (
                    "What walls exist?",
                    Intent::Describe(Target::Kind(KIND_WALL)),
                ),
                (
                    "which tables are there",
                    Intent::Describe(Target::Kind(KIND_TABLE)),
                ),
                ("List the walls.", Intent::Describe(Target::Kind(KIND_WALL))),
                (
                    "what is every table",
                    Intent::Describe(Target::Kind(KIND_TABLE)),
                ),
            ],
        );
        // "What exists" names nothing: no match, not the pointed surface.
        missed(
            &v,
            &[
                ("what exists", Reason::NoMatch),
                (
                    "What shelves are there?",
                    Reason::UnknownSurface("shelves".to_owned()),
                ),
            ],
        );
    }

    #[test]
    fn a_miss_on_a_name_is_one_the_surface_labels_answer() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, None);
        let reason = |sentence: &str| parse(sentence, &v).unwrap_err().reason;
        // A name the room lacks, and a name two tables share.
        assert!(reason("the shelf in blue").names_surfaces());
        assert!(reason("table in amber").names_surfaces());
        assert_eq!(
            reason("table in amber"),
            Reason::Ambiguous(vec!["table 1".to_owned(), "table 2".to_owned()])
        );
        // Not about a name.
        for sentence in [
            "flibber jabber",
            "in amber",
            "astrolabe on wall 3",
            "fire on the desk",
        ] {
            assert!(!reason(sentence).names_surfaces(), "{sentence}");
        }
    }

    #[test]
    fn a_list_wraps_onto_lines_and_counts_what_does_not_fit() {
        let items =
            |n: usize| -> Vec<String> { (0..n).map(|k| format!("wall {k} none")).collect() };
        assert_eq!(wrap_list("walls:", &items(1), 36, 6), "walls: wall 0 none");
        assert_eq!(
            wrap_list("walls:", &items(5), 36, 6),
            "walls: wall 0 none \u{b7} wall 1 none\nwall 2 none \u{b7} wall 3 none\nwall 4 none"
        );
        // Every line within the width.
        let wrapped = wrap_list("walls:", &items(9), 36, 6);
        assert!(
            wrapped.lines().all(|l| l.chars().count() <= 36),
            "{wrapped}"
        );
        // An item wider than a line has one of its own.
        let long = vec![
            "a very long name for a surface aurora".to_owned(),
            "b none".to_owned(),
        ];
        assert_eq!(
            wrap_list("others:", &long, 20, 6),
            "others: a very long name for a surface aurora\nb none"
        );
        // More than the lines hold: the rest counted at the end, within
        // the width.
        let wrapped = wrap_list("walls:", &items(40), 36, 3);
        let lines: Vec<&str> = wrapped.lines().collect();
        assert_eq!(lines.len(), 3, "{wrapped}");
        assert!(lines.iter().all(|l| l.chars().count() <= 36), "{wrapped}");
        assert!(lines[2].ends_with(" more"), "{wrapped}");
        let shown = wrapped.matches(" none").count();
        let more: usize = lines[2]
            .rsplit(" \u{b7} ")
            .next()
            .and_then(|m| m.strip_suffix(" more"))
            .and_then(|n| n.parse().ok())
            .unwrap();
        assert_eq!(shown + more, 40, "{wrapped}");
    }

    #[test]
    fn describe_a_kind_lists_its_surfaces_by_name() {
        let dir = std::env::temp_dir().join(format!("fosfora-intent-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Kevin's room on Nov 12, as far as its walls go: walls 1, 6, 8 and
        // 14 among a desk, tables, storage and a couch.
        let mut boxes: Vec<LaneBox<'static>> = (0..15u8)
            .map(|n| match n {
                0 => lane(n, KIND_TABLE, "DESK"),
                1 | 6 | 8 | 14 => lane(n, KIND_WALL, "WALL_FACE"),
                2..=4 => lane(n, KIND_TABLE, "TABLE"),
                5 => lane(n, KIND_OTHER, "COUCH"),
                7 => lane(n, KIND_FLOOR, "FLOOR"),
                9 => lane(n, KIND_CEILING, "CEILING"),
                _ => lane(n, KIND_OTHER, "STORAGE"),
            })
            .collect();
        boxes.push(LaneBox {
            uuid: STAGE_FLOOR_UUID,
            kind: KIND_FLOOR,
            label: "",
        });
        let e = effects();
        let id = room_id(boxes.iter().map(|b| b.uuid));
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        let v = vocab(&e, &boxes, None);
        let say = |lanes: &mut RoomLanes, sentence: &str| {
            let intent = parse(sentence, &v).unwrap();
            apply(&intent, &v, lanes, &boxes).unwrap()
        };
        say(&mut lanes, "every wall aurora");
        say(&mut lanes, "nothing on wall 8");
        say(&mut lanes, "wall 14 off");
        assert_eq!(
            say(&mut lanes, "What walls exist?"),
            "walls: wall 1 aurora \u{b7} wall 6 aurora\nwall 8 none \u{b7} wall 14 none"
        );
        // The names are the grammar's: each one names its wall.
        for name in ["wall 1", "wall 6", "wall 8", "wall 14"] {
            assert!(parse(&format!("what is {name}"), &v).is_ok(), "{name}");
        }
        // The scene floor and the stage floor, both named.
        assert_eq!(
            say(&mut lanes, "describe the floors"),
            "floors: floor 7 rings\nfloor 15 rings"
        );
        assert_eq!(
            say(&mut lanes, "list the tables"),
            "tables: desk streamlines\ntable 2 streamlines\ntable 3 streamlines\ntable 4 streamlines"
        );
        // A kind the room lacks.
        assert_eq!(
            say(&mut lanes, "describe the frames"),
            "frames: none in the room"
        );
        // A list shows as long as the help; one line as a reply.
        let mut page = 0;
        let (r, _) = respond_on(
            "what are the walls",
            &v,
            &mut lanes,
            &boxes,
            false,
            &mut page,
        );
        assert_eq!(r.label.lines().count(), 2, "{}", r.label);
        assert_close!(r.seconds, HELP_S);
        let (r, _) = respond_on(
            "describe the ceiling",
            &v,
            &mut lanes,
            &boxes,
            false,
            &mut page,
        );
        assert_close!(r.seconds, REPLY_S);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_normalizer_takes_capitals_punctuation_and_fillers() {
        assert_eq!(
            normalize("The Desk, in Amber, please."),
            ["desk", "in", "amber"]
        );
        assert_eq!(normalize("  Switch   to\tEMBERS! "), ["switch", "embers"]);
        assert_eq!(normalize("What's this?"), ["whats", "this"]);
        assert_eq!(normalize("table two"), ["table", "2"]);
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        check(
            &v,
            &[
                ("PLEASE, SWITCH TO THE FLUX CLOUD!", Intent::Effect(0)),
                (
                    "The desk in amber, please.",
                    color_of(Target::Surface(0), 4),
                ),
                (
                    "Put the streamlines on the table 1.",
                    on(Target::Surface(1), B::Streamlines),
                ),
            ],
        );
    }

    #[test]
    fn the_synonyms() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        check(
            &v,
            &[
                // The couch is the other kind: every one of it, or the
                // room's only one.
                ("All couches curls.", on(Target::Kind(KIND_OTHER), B::Curls)),
                ("the couch in blue", color_of(Target::Surface(7), 1)),
                ("the table 2 on the bass", band_of(Target::Surface(2), 1)),
                ("the lamp on the low band", band_of(Target::Surface(7), 1)),
                (
                    "Brighter on the desk.",
                    strength_of(Target::Surface(0), Strength::Up),
                ),
                (
                    "the desk more",
                    strength_of(Target::Surface(0), Strength::Up),
                ),
                ("Nothing on the roof.", on(Target::Surface(6), B::None)),
                ("the roof pulse", on(Target::Surface(6), B::Pulse)),
                ("every desk sparks", on(Target::Kind(KIND_TABLE), B::Pulse)),
                ("Nothing on the lamp.", on(Target::Surface(7), B::None)),
                ("none on the floor", on(Target::Surface(5), B::None)),
                ("windows pulse", on(Target::Kind(KIND_FRAME), B::Pulse)),
                ("the ground rings", on(Target::Surface(5), B::Rings)),
            ],
        );
    }

    #[test]
    fn a_shared_name_is_the_pointed_one_or_every_floor() {
        let e = effects();
        // "table" while table 2 is pointed at: table 2.
        let boxes = room();
        let v = vocab(&e, &boxes, Some(2));
        check(
            &v,
            &[(
                "the table dimmer",
                strength_of(Target::Pointed, Strength::Down),
            )],
        );
        // The scene floor and the stage floor share "floor": both.
        let mut boxes = room();
        boxes.push(LaneBox {
            uuid: STAGE_FLOOR_UUID,
            kind: KIND_FLOOR,
            label: "",
        });
        let v = vocab(&e, &boxes, None);
        assert_eq!(v.surfaces[5].name, "floor 5");
        assert_eq!(v.surfaces[8].name, "floor 8");
        check(
            &v,
            &[
                (
                    "Rings on the floor.",
                    on(Target::Kind(KIND_FLOOR), B::Rings),
                ),
                (
                    "floor 8 streamlines",
                    on(Target::Surface(8), B::Streamlines),
                ),
            ],
        );
    }

    #[test]
    fn the_misses() {
        let (e, boxes) = (effects(), room());
        let pointed = vocab(&e, &boxes, Some(0));
        missed(
            &pointed,
            &[
                ("Flibber jabber wocky.", Reason::NoMatch),
                ("", Reason::NoMatch),
                (
                    "Switch to the next effect and fade the colors to purple.",
                    Reason::NoMatch,
                ),
                // A wall's catalogue lacks the prism, whichever wall was
                // meant.
                (
                    "Prism on the wall.",
                    Reason::NotOnThisKind {
                        behavior: B::Prism,
                        kind: KIND_WALL,
                    },
                ),
                // Held back (board #5462), whatever the target.
                ("Fenestra on the wall.", Reason::NotAvailable(B::Fenestra)),
                (
                    "astrolabe on the ceiling",
                    Reason::NotAvailable(B::Astrolabe),
                ),
                (
                    "spectrum on the desk",
                    Reason::NotOnThisKind {
                        behavior: B::Spectrum,
                        kind: KIND_TABLE,
                    },
                ),
                (
                    "every ceiling curls",
                    Reason::NotOnThisKind {
                        behavior: B::Curls,
                        kind: KIND_CEILING,
                    },
                ),
                // The rings do run on a wall: then the question is which.
                (
                    "Rings on the wall.",
                    Reason::Ambiguous(vec!["wall 3".to_owned(), "wall 4".to_owned()]),
                ),
                (
                    "The shelf in blue.",
                    Reason::UnknownSurface("shelf".to_owned()),
                ),
                (
                    "table 9 in blue",
                    Reason::UnknownSurface("table 9".to_owned()),
                ),
                ("Lamp 2 up.", Reason::UnknownSurface("lamp 2".to_owned())),
                // Several words with no surface word before a slot word
                // are no name: the sentence is something else, for the
                // agent (board #3771).
                ("Shut that song up.", Reason::NoMatch),
                ("Turn my music down.", Reason::NoMatch),
                (
                    "Fire on the desk.",
                    Reason::UnknownBehavior("fire".to_owned()),
                ),
            ],
        );
        let nothing = vocab(&e, &boxes, None);
        missed(
            &nothing,
            &[
                (
                    "Table in amber.",
                    Reason::Ambiguous(vec!["table 1".to_owned(), "table 2".to_owned()]),
                ),
                ("In amber.", Reason::NoSurface),
                ("What is this?", Reason::NoSurface),
                ("brighter", Reason::NoSurface),
                ("streamlines", Reason::NoSurface),
            ],
        );
        // The miss keeps the sentence as heard.
        assert_eq!(
            parse(" Flibber jabber. ", &nothing).unwrap_err().heard,
            "Flibber jabber."
        );
    }

    #[test]
    fn reply_names_what_was_done() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        for (intent, want) in [
            (Intent::NextEffect, EFFECTS_GONE),
            (Intent::PrevEffect, EFFECTS_GONE),
            (Intent::Effect(1), EFFECTS_GONE),
            (Intent::EditRoom(true), "edit room on"),
            (Intent::EditRoom(false), "edit room off"),
            (Intent::Cloud(false), "particles off"),
            (Intent::Pitcher(true), "pitcher on"),
            (Intent::Music(true), "music on"),
            (Intent::Music(false), "music off"),
            (Intent::Rescan, "rescanning the room"),
            (Intent::Recenter, "recentered"),
            (Intent::AllNone, "every surface: none"),
            (on(Target::Surface(0), B::Streamlines), "desk: streamlines"),
            (on(Target::Pointed, B::Curls), "desk: curls"),
            (
                on(Target::Kind(KIND_WALL), B::Spectrum),
                "all walls: spectrum",
            ),
            (
                on(Target::Kind(KIND_OTHER), B::None),
                "all other surfaces: none",
            ),
            (color_of(Target::Surface(7), 4), "lamp: amber"),
            (color_of(Target::Surface(0), 0), "desk: own color"),
            (color_of(Target::Pointed, COLOR_KEY), "desk: key"),
            (band_of(Target::Surface(3), 1), "wall 3: bass"),
            (
                strength_of(Target::Surface(0), Strength::Up),
                "desk: brighter",
            ),
            (
                strength_of(Target::Surface(0), Strength::Down),
                "desk: dimmer",
            ),
            (
                strength_of(Target::Kind(KIND_TABLE), Strength::Half),
                "all tables: half strength",
            ),
            (
                strength_of(Target::Surface(1), Strength::Full),
                "table 1: full strength",
            ),
            (
                strength_of(Target::Surface(1), Strength::Off),
                "table 1: strength 0",
            ),
            (Intent::Describe(Target::Surface(6)), "ceiling"),
        ] {
            assert_eq!(reply(&intent, &v), want, "{intent:?}");
        }
    }

    #[test]
    fn miss_text_for_each_reason() {
        let miss = |reason| Miss {
            reason,
            heard: "Flibber jabber.".to_owned(),
        };
        for (reason, want) in [
            (Reason::NoMatch, "Didn't catch that: \"Flibber jabber.\""),
            (Reason::NoSurface, "Point at a surface, or name one"),
            (
                Reason::UnknownSurface("shelf".to_owned()),
                "No surface called \"shelf\"",
            ),
            (
                Reason::UnknownBehavior("fire".to_owned()),
                "No behavior called \"fire\"",
            ),
            (
                Reason::NotOnThisKind {
                    behavior: B::Prism,
                    kind: KIND_WALL,
                },
                "prism doesn't run on a wall",
            ),
            (
                Reason::NotAvailable(B::Astrolabe),
                "Astrolabe is not available",
            ),
            (
                Reason::NotAvailable(B::Fenestra),
                "Fenestra is not available",
            ),
            (
                Reason::NotOnThisKind {
                    behavior: B::Spectrum,
                    kind: KIND_OTHER,
                },
                "spectrum doesn't run on that surface",
            ),
            (
                Reason::Ambiguous(vec!["table 1".to_owned(), "table 2".to_owned()]),
                "Which one: table 1 or table 2?",
            ),
            (
                Reason::Ambiguous(
                    ["table 2", "table 7", "table 13"]
                        .map(str::to_owned)
                        .to_vec(),
                ),
                "Which one: table 2, table 7 or table 13?",
            ),
            (
                Reason::Ambiguous((1..=7).map(|k| format!("storage {k}")).collect()),
                "Which one: storage 1, storage 2, storage 3, storage 4, storage 5 or 2 more?",
            ),
        ] {
            assert_eq!(miss_text(&miss(reason.clone())), want, "{reason:?}");
        }
    }

    #[test]
    fn the_log_line_has_the_sentence_the_outcome_and_the_label() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        let ok = parse("The desk in amber.", &v);
        assert_eq!(
            log_line(" The desk in amber. ", &ok, "desk: amber"),
            "voice: heard \"The desk in amber.\" \u{2192} Color { target: Surface(0), color: 4 } \u{b7} label \"desk: amber\""
        );
        let miss = parse("The shelf in blue.", &v);
        assert_eq!(
            log_line("The shelf in blue.", &miss, "No surface called \"shelf\""),
            "voice: heard \"The shelf in blue.\" \u{2192} miss UnknownSurface(\"shelf\") \u{b7} label \"No surface called \"shelf\"\""
        );
    }

    #[test]
    fn apply_writes_through_the_lanes_and_describe_reads_them() {
        let dir = std::env::temp_dir().join(format!("fosfora-intent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (e, boxes) = (effects(), room());
        let id = room_id(boxes.iter().map(|b| b.uuid));
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        let v = vocab(&e, &boxes, Some(0));
        let say = |lanes: &mut RoomLanes, sentence: &str| {
            let intent = parse(sentence, &v).unwrap();
            apply(&intent, &v, lanes, &boxes)
        };
        // The menu's intents are the app's.
        assert_eq!(say(&mut lanes, "next effect"), None);
        assert_eq!(
            say(&mut lanes, "every table curls").as_deref(),
            Some("all tables: curls")
        );
        for k in 0..3 {
            assert_eq!(
                lanes.effective(k, &boxes),
                Some((B::Curls, 1.0)),
                "table {k}"
            );
        }
        assert_eq!(
            say(&mut lanes, "the desk in amber").as_deref(),
            Some("desk: amber")
        );
        assert_eq!(
            say(&mut lanes, "the desk on the bass").as_deref(),
            Some("desk: bass")
        );
        assert_eq!(say(&mut lanes, "dimmer").as_deref(), Some("desk: dimmer"));
        assert_eq!(lanes.params_of(0, &boxes), (4, 1, 0.9));
        assert_eq!(
            say(&mut lanes, "what is this").as_deref(),
            Some("desk: curls · amber · bass · 0.9")
        );
        // A behavior on one surface keeps its strength, as a tap does.
        assert_eq!(say(&mut lanes, "pulse").as_deref(), Some("desk: pulse"));
        assert_eq!(lanes.effective(0, &boxes), Some((B::Pulse, 0.9)));
        // A kind's color, band and strength: each surface of it.
        say(&mut lanes, "every wall in rose");
        say(&mut lanes, "walls half");
        for k in [3, 4] {
            assert_eq!(lanes.params_of(k, &boxes), (7, 0, 0.5), "wall {k}");
        }
        assert_eq!(
            say(&mut lanes, "what is every table").as_deref(),
            Some("tables: desk pulse \u{b7} table 1 curls\ntable 2 curls")
        );
        assert_eq!(
            say(&mut lanes, "lamp strength off").as_deref(),
            Some("lamp: strength 0")
        );
        assert_close!(lanes.params_of(7, &boxes).2, 0.0);
        assert_eq!(
            say(&mut lanes, "the lamp full").as_deref(),
            Some("lamp: full strength")
        );
        // Saved: a relaunch reads it back.
        let mut again = RoomLanes::new(dir.clone());
        again.update(id, &boxes);
        assert_eq!(again.params_of(0, &boxes), (4, 1, 0.9));
        assert_eq!(again.effective(1, &boxes), Some((B::Curls, 1.0)));
        assert_eq!(again.params_of(4, &boxes), (7, 0, 0.5));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `intent` as the agent would send it: the kind, the target by name
    /// (a kind by its word, the pointed surface as `pointed`), the value.
    fn to_json(intent: &Intent, v: &Vocabulary<'_>) -> AgentAction {
        let target = |t: Target| match t {
            Target::Pointed => "pointed".to_owned(),
            Target::Surface(k) => v.surface(k).unwrap().name.clone(),
            Target::Kind(kind) => kind_name(kind).to_owned(),
        };
        let on_off = |on: bool| if on { "on" } else { "off" }.to_owned();
        let (kind, t, value) = match *intent {
            Intent::NextEffect => ("next_effect", String::new(), String::new()),
            Intent::PrevEffect => ("prev_effect", String::new(), String::new()),
            Intent::Effect(i) => ("effect", String::new(), v.effects[i].clone()),
            Intent::EditRoom(on) => ("edit_room", String::new(), on_off(on)),
            Intent::Cloud(on) => ("cloud", String::new(), on_off(on)),
            Intent::Pitcher(on) => ("pitcher", String::new(), on_off(on)),
            Intent::Music(on) => ("music", String::new(), on_off(on)),
            Intent::Rescan => ("rescan", String::new(), String::new()),
            Intent::Recenter => ("recenter", String::new(), String::new()),
            Intent::AllNone => ("all_none", String::new(), String::new()),
            Intent::Behavior {
                target: t,
                behavior,
            } => ("behavior", target(t), behavior.name().to_owned()),
            Intent::Color {
                target: t,
                color: 0,
            } => ("color", target(t), "own".to_owned()),
            Intent::Color { target: t, color } => {
                ("color", target(t), color_name(color).to_owned())
            }
            Intent::Band { target: t, band } => (
                "band",
                target(t),
                crate::surface_fx::band_name(band).to_owned(),
            ),
            Intent::Strength {
                target: t,
                strength,
            } => (
                "strength",
                target(t),
                match strength {
                    Strength::Up => "up",
                    Strength::Down => "down",
                    Strength::Half => "half",
                    Strength::Full => "full",
                    Strength::Off => "off",
                }
                .to_owned(),
            ),
            Intent::Describe(t) => ("describe", target(t), String::new()),
            Intent::Help => unreachable!("the agent's actions have no help"),
            Intent::Immersion(_) => unreachable!("the agent's actions have no immersion"),
            Intent::Roam(_) => unreachable!("the agent's actions have no roam"),
            Intent::Theme(_) => unreachable!("the agent's actions have no theme"),
            Intent::Menu(_) => unreachable!("the agent's actions have no menu"),
            Intent::Begin => unreachable!("the agent's actions have no begin"),
            Intent::Credits => unreachable!("the agent's actions have no credits"),
            Intent::Edges(_) | Intent::Agent(_) | Intent::Voice(_) => {
                unreachable!("the agent's actions have no edges, agent or voice")
            }
            Intent::NextTrack
            | Intent::PrevTrack
            | Intent::Track(_)
            | Intent::NowPlaying
            | Intent::ScanMusic => unreachable!("the agent's actions name no tracks"),
        };
        AgentAction {
            kind: kind.to_owned(),
            target: t,
            value,
        }
    }

    /// Every variant sent as the agent's JSON (through serde, as a reply
    /// carries it) comes back as itself, with the same label.
    #[test]
    fn from_json_round_trips_the_reply_of_every_variant() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        let mut intents = vec![
            Intent::NextEffect,
            Intent::PrevEffect,
            Intent::Effect(0),
            Intent::Effect(3),
            Intent::EditRoom(true),
            Intent::EditRoom(false),
            Intent::Cloud(true),
            Intent::Cloud(false),
            Intent::Pitcher(true),
            Intent::Pitcher(false),
            Intent::Music(true),
            Intent::Music(false),
            Intent::Rescan,
            Intent::Recenter,
            Intent::AllNone,
            on(Target::Pointed, B::Pulse),
            on(Target::Surface(2), B::Streamlines),
            on(Target::Kind(KIND_WALL), B::Aurora),
            on(Target::Kind(KIND_TABLE), B::None),
            band_of(Target::Surface(3), 1),
            band_of(Target::Kind(KIND_WALL), 3),
            Intent::Describe(Target::Pointed),
            Intent::Describe(Target::Surface(7)),
            Intent::Describe(Target::Kind(KIND_TABLE)),
        ];
        intents.extend((0..=COLOR_KEY).map(|c| color_of(Target::Surface(7), c)));
        intents.extend(
            [
                Strength::Up,
                Strength::Down,
                Strength::Half,
                Strength::Full,
                Strength::Off,
            ]
            .map(|s| strength_of(Target::Surface(0), s)),
        );
        for intent in intents {
            let sent = serde_json::to_string(&to_json(&intent, &v)).unwrap();
            let action: AgentAction = serde_json::from_str(&sent).unwrap();
            let back = Intent::from_json(&action, &v);
            assert_eq!(back, Ok(intent), "{sent}");
            assert_eq!(reply(&back.unwrap(), &v), reply(&intent, &v), "{sent}");
        }
        // A missing field reads as empty.
        let bare: AgentAction = serde_json::from_str(r#"{"kind": "rescan"}"#).unwrap();
        assert_eq!(Intent::from_json(&bare, &v), Ok(Intent::Rescan));
    }

    #[test]
    fn the_say_knob_feeds_a_value_once() {
        let mut knob = SayKnob::default();
        assert_eq!(knob.poll(None, false), None);
        // Waiting for the room: held, then fed once it is in.
        assert_eq!(knob.poll(Some("next effect"), true), None);
        assert_eq!(
            knob.poll(Some(" next effect "), false).as_deref(),
            Some("next effect")
        );
        assert_eq!(knob.poll(Some("next effect"), false), None);
        // Another sentence: fed.
        assert_eq!(knob.poll(Some("embers"), false).as_deref(), Some("embers"));
        // Cleared, then the same again: fed again.
        assert_eq!(knob.poll(Some(""), false), None);
        assert_eq!(knob.poll(Some("embers"), false).as_deref(), Some("embers"));
    }

    /// The replayed room the reviewer runs the `say` knob on (17 anchors:
    /// 5 tables, 5 storage, 4 walls, a floor, a ceiling, a window frame,
    /// then the stage floor), the boxes at the indices the device's lane
    /// table logged (`MEASURED.md`: table 0 and 14, storage 1 and 9, wall
    /// 2, 5 and 16, floor 7, window 8, ceiling 10); the other seven's order
    /// is a guess that none of the ten sentences depends on.
    fn replayed_room() -> Vec<LaneBox<'static>> {
        let at = |k: usize| -> (u32, &'static str) {
            match k {
                0 | 3 | 6 | 12 | 14 => (KIND_TABLE, "TABLE"),
                1 | 4 | 9 | 11 | 13 => (KIND_OTHER, "STORAGE"),
                2 | 5 | 15 | 16 => (KIND_WALL, "WALL_FACE"),
                7 => (KIND_FLOOR, "FLOOR"),
                8 => (KIND_FRAME, "WINDOW_FRAME"),
                _ => (KIND_CEILING, "CEILING"),
            }
        };
        let mut boxes: Vec<LaneBox<'static>> = (0..17)
            .map(|k| {
                let (kind, label) = at(k);
                lane(k as u8, kind, label)
            })
            .collect();
        boxes.push(LaneBox {
            uuid: STAGE_FLOOR_UUID,
            kind: KIND_FLOOR,
            label: "",
        });
        boxes
    }

    #[test]
    fn the_ten_sentences_on_the_replayed_room() {
        let dir = std::env::temp_dir().join(format!("fosfora-intent-ten-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let e: Vec<String> = ["Flux Cloud", "Embers", "Flock"]
            .map(str::to_owned)
            .to_vec();
        let boxes = replayed_room();
        let id = room_id(boxes.iter().map(|b| b.uuid));
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        let v = vocab(&e, &boxes, None);
        let mut lines = Vec::new();
        for sentence in [
            "Next effect.",
            "Embers.",
            "Edit the room.",
            "Every table streamlines.",
            "Table 14 in amber.",
            "Wall 5 on the bass.",
            "Rings on the floor.",
            "What is table 14?",
            "Prism on wall 2.",
            "The shelf in blue.",
        ] {
            let outcome = parse(sentence, &v);
            let label = match &outcome {
                Ok(intent) => {
                    apply(intent, &v, &mut lanes, &boxes).unwrap_or_else(|| reply(intent, &v))
                }
                Err(miss) => miss_text(miss),
            };
            lines.push(log_line(sentence, &outcome, &label));
        }
        let want = [
            "voice: heard \"Next effect.\" → NextEffect · label \"The effects are gone\"",
            "voice: heard \"Embers.\" → Effect(1) · label \"The effects are gone\"",
            "voice: heard \"Edit the room.\" → EditRoom(true) · label \"edit room on\"",
            "voice: heard \"Every table streamlines.\" → Behavior { target: Kind(1), behavior: Streamlines } · label \"all tables: streamlines\"",
            "voice: heard \"Table 14 in amber.\" → Color { target: Surface(14), color: 4 } · label \"table 14: amber\"",
            "voice: heard \"Wall 5 on the bass.\" → Band { target: Surface(5), band: 1 } · label \"wall 5: bass\"",
            "voice: heard \"Rings on the floor.\" → Behavior { target: Kind(2), behavior: Rings } · label \"all floors: rings\"",
            "voice: heard \"What is table 14?\" → Describe(Surface(14)) · label \"table 14: streamlines · amber · rms · 1.0\"",
            "voice: heard \"Prism on wall 2.\" → miss NotOnThisKind { behavior: Prism, kind: 3 } · label \"prism doesn't run on a wall\"",
            "voice: heard \"The shelf in blue.\" → miss UnknownSurface(\"shelf\") · label \"No surface called \"shelf\"\"",
        ];
        assert_eq!(lines, want);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn number_words_are_digits() {
        for (said, want) in [
            ("table two", vec!["table", "2"]),
            ("table fourteen", vec!["table", "14"]),
            ("wall twenty one", vec!["wall", "21"]),
            ("forty", vec!["40"]),
            ("fifty nine", vec!["59"]),
            ("twenty twelve", vec!["20", "12"]),
            ("one", vec!["1"]),
            ("this one", vec!["this", "one"]),
            ("that one", vec!["that", "one"]),
            ("next one", vec!["next", "one"]),
            ("brightness zero", vec!["brightness", "zero"]),
        ] {
            assert_eq!(normalize(said), want, "{said}");
        }
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        check(
            &v,
            &[
                ("table two in amber", color_of(Target::Surface(2), 4)),
                ("Wall three on the bass.", band_of(Target::Surface(3), 1)),
                ("next one", Intent::NextEffect),
                (
                    "streamlines on this one",
                    on(Target::Pointed, B::Streamlines),
                ),
                (
                    "the desk brightness zero",
                    strength_of(Target::Surface(0), Strength::Off),
                ),
            ],
        );
        let boxes = replayed_room();
        let v = vocab(&e, &boxes, None);
        check(
            &v,
            &[
                ("Table fourteen in amber.", color_of(Target::Surface(14), 4)),
                ("wall sixteen pulse", on(Target::Surface(16), B::Pulse)),
            ],
        );
    }

    #[test]
    fn split_clauses_cuts_at_the_joiners_and_never_inside_a_name() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        let split = |s: &str| split_clauses(s, &v);
        assert_eq!(
            split("Amber on the desk and the walls on the bass."),
            ["Amber on the desk", "the walls on the bass."]
        );
        assert_eq!(
            split("Next effect, particles off then music play"),
            ["Next effect", "particles off", "music play"]
        );
        assert_eq!(
            split("Embers on the desk and then rings on the floor."),
            ["Embers on the desk", "rings on the floor."]
        );
        assert_eq!(
            split("Next effect. Embers on the desk."),
            ["Next effect", "Embers on the desk."]
        );
        assert_eq!(
            split("next effect; music stop"),
            ["next effect", "music stop"]
        );
        // No splitter, a name without one, a clause of fillers.
        assert_eq!(
            split("The desk in warm white."),
            ["The desk in warm white."]
        );
        assert_eq!(split("Next effect, please."), ["Next effect"]);
        assert_eq!(split("  "), Vec::<String>::new());
        // A name the room has with "and" in it splits nothing inside it.
        let e: Vec<String> = ["Flux Cloud", "Rock and Roll"].map(str::to_owned).to_vec();
        let v = vocab(&e, &boxes, Some(0));
        assert_eq!(
            split_clauses("Rock and Roll and next effect", &v),
            ["Rock and Roll", "next effect"]
        );
        let clauses = parse_clauses("rock and roll then embers on the desk", &v);
        assert_eq!(clauses[0].outcome, Ok(Intent::Effect(1)));
        assert_eq!(clauses[1].outcome, Ok(on(Target::Surface(0), B::Curls)));
    }

    #[test]
    fn parse_clauses_keeps_a_whole_match_and_caps_the_count() {
        let (e, boxes) = (effects(), room());
        let v = vocab(&e, &boxes, Some(0));
        let outcomes = |s: &str| -> Vec<Result<Intent, Reason>> {
            parse_clauses(s, &v)
                .into_iter()
                .map(|c| c.outcome.map_err(|m| m.reason))
                .collect()
        };
        // V2's sentences are one clause, commas and all.
        assert_eq!(
            outcomes("The Desk, in Amber, please."),
            [Ok(color_of(Target::Surface(0), 4))]
        );
        assert_eq!(outcomes("Next effect."), [Ok(Intent::NextEffect)]);
        assert_eq!(outcomes("Flibber jabber."), [Err(Reason::NoMatch)]);
        // Two and three things.
        assert_eq!(
            outcomes("Amber on the desk and the walls on the bass."),
            [
                Ok(color_of(Target::Surface(0), 4)),
                Ok(band_of(Target::Kind(KIND_WALL), 1)),
            ]
        );
        assert_eq!(
            outcomes("Next effect, particles off and table one dimmer."),
            [
                Ok(Intent::NextEffect),
                Ok(Intent::Cloud(false)),
                Ok(strength_of(Target::Surface(1), Strength::Down)),
            ]
        );
        // A clause that misses does not stop the others.
        assert_eq!(
            outcomes("Embers on the desk and flibber jabber."),
            [Ok(on(Target::Surface(0), B::Curls)), Err(Reason::NoMatch)]
        );
        // Four is too many: one miss for the sentence.
        let four = parse_clauses(
            "Next effect, particles off, music play and edit the room.",
            &v,
        );
        assert_eq!(four.len(), 1);
        let miss = four[0].outcome.as_ref().unwrap_err();
        assert_eq!(miss.reason, Reason::TooMany(4));
        assert_eq!(miss_text(miss), TOO_MANY_LABEL);
    }

    #[test]
    fn help_and_every_example_parses() {
        let e = effects();
        let rooms = [room(), replayed_room()];
        for boxes in &rooms {
            for pointed in [None, Some(0)] {
                let v = vocab(&e, boxes, pointed);
                for said in ["What can I say?", "help", "Voice help.", "what can I do"] {
                    assert_eq!(parse(said, &v), Ok(Intent::Help), "{said}");
                }
                // Every example the room gives matches, every clause of
                // it, and fits the label's line.
                let all = pool(&v);
                assert!(all.len() >= 20, "{all:?}");
                for example in &all {
                    assert!(example.chars().count() < 30, "{example}");
                    for c in parse_clauses(example, &v) {
                        assert!(c.outcome.is_ok(), "\"{example}\": {:?}", c.outcome);
                    }
                }
                assert_eq!(shown_pool(&v), all);
                // Five a page, the next five on each ask, wrapping.
                let pages: Vec<Vec<String>> = (0..=all.len()).map(|p| examples(&v, p)).collect();
                assert!(pages.iter().all(|p| p.len() == HELP_LINES));
                assert_ne!(pages[0], pages[1]);
                assert_eq!(pages[1][0], all[HELP_LINES]);
                assert_eq!(pages[all.len()], pages[0]);
                let seen: std::collections::HashSet<&String> = pages.iter().flatten().collect();
                assert_eq!(seen.len(), all.len());
            }
        }
        // The examples name this room's surfaces, and no effect.
        let boxes = replayed_room();
        let v = vocab(&e, &boxes, None);
        assert_eq!(
            examples(&v, 0),
            [
                "next theme",
                "streamlines on table 0",
                "every table curls",
                "open the menu",
                "storage 1 in blue",
            ]
        );
    }

    #[test]
    fn the_hint_is_the_nearest_example() {
        let (e, boxes) = (effects(), replayed_room());
        let v = vocab(&e, &boxes, None);
        // Words in common: the example that shares the most.
        assert_eq!(
            hint("Let me edit this whole room now.", &v),
            "Try \"edit the room\""
        );
        assert_eq!(
            hint("Make table 14 sort of glowy.", &v),
            "Try \"what is table 14\""
        );
        // Nothing near: the help.
        assert_eq!(hint("Flibber jabber.", &v), "Try \"what can I say\"");
    }

    /// `sentence` through the clauses and [`respond`] as the app runs it
    /// (the menu's intents labeled by [`reply`]), on `lanes`, and its log
    /// line.
    fn respond_on(
        sentence: &str,
        v: &Vocabulary<'_>,
        lanes: &mut RoomLanes,
        boxes: &[LaneBox<'_>],
        agent: bool,
        page: &mut usize,
    ) -> (Response, String) {
        let clauses = parse_clauses(sentence, v);
        let response = respond(sentence, &clauses, v, agent, page, |i| {
            apply(i, v, lanes, boxes).unwrap_or_else(|| reply(i, v))
        });
        let line = clauses_log_line(sentence, &clauses, &response.label);
        (response, line)
    }

    #[test]
    fn the_reply_joins_the_clauses_and_the_agent_takes_what_misses() {
        let dir = std::env::temp_dir().join(format!("fosfora-intent-v4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (e, boxes) = (effects(), room());
        let id = room_id(boxes.iter().map(|b| b.uuid));
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        let v = vocab(&e, &boxes, Some(0));
        let mut page = 0;
        let mut say = |sentence: &str, agent: bool, page: &mut usize| {
            respond_on(sentence, &v, &mut lanes, &boxes, agent, page)
        };
        let (r, line) = say(
            "Amber on the desk and the walls on the bass.",
            false,
            &mut page,
        );
        assert_eq!(r.label, "desk: amber \u{b7} all walls: bass");
        assert_close!(r.seconds, REPLY_S);
        assert_eq!(r.agent, None);
        assert_eq!(
            line,
            "voice: heard \"Amber on the desk and the walls on the bass.\" \u{2192} [Color { target: Surface(0), color: 4 }, Band { target: Kind(3), band: 1 }] \u{b7} label \"desk: amber \u{b7} all walls: bass\""
        );
        // A miss joins the reply, with the hint under it.
        let (r, line) = say("Embers on the desk and flibber jabber.", false, &mut page);
        assert_eq!(
            r.label,
            "desk: curls \u{b7} Didn't catch that: \"flibber jabber.\"\nTry \"what can I say\""
        );
        assert_close!(r.seconds, MISS_S);
        assert!(
            line.ends_with(
                "label \"desk: curls \u{b7} Didn't catch that: \"flibber jabber.\" | Try \"what can I say\"\""
            ),
            "{line}"
        );
        // The agent on: the clause that misses goes to it, alone.
        let (r, _) = say("Embers on the desk and make it cozy.", true, &mut page);
        assert_eq!(r.label, "desk: curls \u{b7} Thinking\u{2026}");
        assert_eq!(r.agent.as_deref(), Some("make it cozy."));
        assert_close!(r.seconds, crate::agent::THINKING_S);
        // Every clause misses, or too many: the whole sentence goes.
        for sentence in [
            "Make it cozy and warm.",
            "Something like a campfire on table 1.",
            "Next effect, particles off, music play and edit the room.",
        ] {
            let (r, _) = say(sentence, true, &mut page);
            assert_eq!(r.agent.as_deref(), Some(sentence), "{sentence}");
            assert_eq!(r.label, "Thinking\u{2026}", "{sentence}");
        }
        // Not with the agent off: the misses' own texts.
        let (r, _) = say(
            "Next effect, particles off, music play and edit the room.",
            false,
            &mut page,
        );
        assert_eq!(r.label, TOO_MANY_LABEL);
        // A miss the agent does not take stays the grammar's answer.
        let (r, _) = say("Next effect and the shelf in blue.", true, &mut page);
        assert_eq!(
            r.label,
            "The effects are gone \u{b7} No surface called \"shelf\""
        );
        assert_eq!(r.agent, None);
        // Help: five lines, the page turned.
        let before = page;
        let (r, _) = say("What can I say?", false, &mut page);
        assert_eq!(r.label.lines().count(), HELP_LINES);
        assert_close!(r.seconds, CARD_S);
        assert_eq!(page, before + 1);
        assert_eq!(lanes.params_of(0, &boxes).0, 4);
        assert_eq!(lanes.params_of(3, &boxes).1, 1);
        assert_eq!(lanes.effective(0, &boxes).map(|(b, _)| b), Some(B::Curls));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `say` knob's V4 sentences on the replayed room (nothing pointed,
    /// the agent off), each with the log line and the label the device
    /// should show: two things, numbers, help twice (the page turns), the
    /// hint on two misses, and too many.
    #[test]
    fn the_v4_sentences_on_the_replayed_room() {
        let dir =
            std::env::temp_dir().join(format!("fosfora-intent-v4-say-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let e: Vec<String> = ["Flux Cloud", "Embers", "Flock"]
            .map(str::to_owned)
            .to_vec();
        let boxes = replayed_room();
        let id = room_id(boxes.iter().map(|b| b.uuid));
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        let v = vocab(&e, &boxes, None);
        let mut page = 0;
        let mut got = Vec::new();
        for sentence in [
            "Amber on table 14 and the walls on the bass.",
            "Table fourteen in teal, then wall five on the highs.",
            "Wall sixteen pulse.",
            "What can I say?",
            "Help.",
            "Flibber jabber.",
            "Make table fourteen sort of glowy.",
            "Next effect, particles off, music play and edit the room.",
        ] {
            let (r, line) = respond_on(sentence, &v, &mut lanes, &boxes, false, &mut page);
            got.push((line, r.label, r.seconds));
        }
        let want: [(&str, &str, f32); 8] = [
            (
                "voice: heard \"Amber on table 14 and the walls on the bass.\" \u{2192} [Color { target: Surface(14), color: 4 }, Band { target: Kind(3), band: 1 }] \u{b7} label \"table 14: amber \u{b7} all walls: bass\"",
                "table 14: amber \u{b7} all walls: bass",
                REPLY_S,
            ),
            (
                "voice: heard \"Table fourteen in teal, then wall five on the highs.\" \u{2192} [Color { target: Surface(14), color: 6 }, Band { target: Surface(5), band: 3 }] \u{b7} label \"table 14: teal \u{b7} wall 5: high\"",
                "table 14: teal \u{b7} wall 5: high",
                REPLY_S,
            ),
            (
                "voice: heard \"Wall sixteen pulse.\" \u{2192} Behavior { target: Surface(16), behavior: Pulse } \u{b7} label \"wall 16: pulse\"",
                "wall 16: pulse",
                REPLY_S,
            ),
            (
                "voice: heard \"What can I say?\" \u{2192} Help \u{b7} label \"next theme | streamlines on table 0 | every table curls | open the menu | storage 1 in blue\"",
                "next theme\nstreamlines on table 0\nevery table curls\nopen the menu\nstorage 1 in blue",
                CARD_S,
            ),
            (
                "voice: heard \"Help.\" \u{2192} Help \u{b7} label \"every wall rings | music play and next theme | wall 2 on the bass | every floor tessera | edit the room\"",
                "every wall rings\nmusic play and next theme\nwall 2 on the bass\nevery floor tessera\nedit the room",
                CARD_S,
            ),
            (
                "voice: heard \"Flibber jabber.\" \u{2192} miss NoMatch \u{b7} label \"Didn't catch that: \"Flibber jabber.\" | Try \"what can I say\"\"",
                "Didn't catch that: \"Flibber jabber.\"\nTry \"what can I say\"",
                MISS_S,
            ),
            (
                "voice: heard \"Make table fourteen sort of glowy.\" \u{2192} miss NoMatch \u{b7} label \"Didn't catch that: \"Make table fourteen sort of glowy.\" | Try \"what is table 14\"\"",
                "Didn't catch that: \"Make table fourteen sort of glowy.\"\nTry \"what is table 14\"",
                MISS_S,
            ),
            (
                "voice: heard \"Next effect, particles off, music play and edit the room.\" \u{2192} miss TooMany(4) \u{b7} label \"Three things at most in one sentence\"",
                "Three things at most in one sentence",
                MISS_S,
            ),
        ];
        assert_eq!(got.len(), want.len());
        for ((line, label, seconds), (want_line, want_label, want_s)) in got.iter().zip(want) {
            assert_eq!(line, want_line);
            assert_eq!(label, want_label);
            assert_close!(*seconds, want_s);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The vocabulary `app.rs` builds since board #5470: the cut effects'
    /// names, and the behaviors a surface can still be given.
    fn app_vocab<'a>(effects: &'a [String], boxes: &[LaneBox<'_>]) -> Vocabulary<'a> {
        Vocabulary {
            effects,
            behaviors: &SurfaceBehavior::OFFERED,
            surfaces: surfaces(boxes),
            pointed: Some(0),
            tracks: Tracks::default(),
        }
    }

    /// The three cloud effects are gone: a sentence that steps them or
    /// names one, and an agent's action that does, is answered in one
    /// short sentence and changes nothing.
    #[test]
    fn the_effect_phrases_answer_that_the_effects_are_gone() {
        let dir = std::env::temp_dir().join(format!("fosfora-intent-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (e, boxes) = (cut_effects(), room());
        assert_eq!(e, ["Flux Cloud", "Embers", "Flock"]);
        let v = app_vocab(&e, &boxes);
        let id = room_id(boxes.iter().map(|b| b.uuid));
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        let before: Vec<_> = (0..boxes.len())
            .map(|k| lanes.effective(k, &boxes))
            .collect();
        for said in [
            "Next effect.",
            "previous effect",
            "last effect",
            "Switch to Flux Cloud.",
            "show flock",
            "Go to embers",
            "Embers.",
            "Flock",
        ] {
            let intent = parse(said, &v).unwrap_or_else(|m| panic!("{said}: {m:?}"));
            assert!(asks_for_effect(&intent), "{said}: {intent:?}");
            // Nothing to write, and the label says why.
            assert_eq!(apply(&intent, &v, &mut lanes, &boxes), None, "{said}");
            assert_eq!(reply(&intent, &v), "The effects are gone", "{said}");
        }
        // The agent's ids for them, the on-device model's too.
        for (kind, value) in [
            ("next_effect", ""),
            ("prev_effect", ""),
            ("previous_effect", ""),
            ("effect", "Flock"),
        ] {
            let action = AgentAction {
                kind: kind.to_owned(),
                target: String::new(),
                value: value.to_owned(),
            };
            let intent = Intent::from_json(&action, &v).unwrap_or_else(|m| panic!("{kind}: {m:?}"));
            assert_eq!(reply(&intent, &v), EFFECTS_GONE, "{kind}");
        }
        // One short sentence, on one line of the label.
        assert_eq!(crate::label::layout(EFFECTS_GONE).lines.len(), 1);
        assert!(EFFECTS_GONE.chars().count() < MAX_EXAMPLE_CHARS);
        // The room is as it was.
        let after: Vec<_> = (0..boxes.len())
            .map(|k| lanes.effective(k, &boxes))
            .collect();
        assert_eq!(before, after);
        // Embers and sparks painted a surface with the cloud's particles,
        // which are gone (board #5462): each gets its stand-in, on the
        // kind's list, and the reply names that look.
        for (said, target, look, label) in [
            (
                "Embers on the desk.",
                Target::Surface(0),
                B::Curls,
                "desk: curls",
            ),
            (
                "sparks on the floor",
                Target::Surface(5),
                B::Rings,
                "floor: rings",
            ),
            (
                "every table embers",
                Target::Kind(KIND_TABLE),
                B::Curls,
                "all tables: curls",
            ),
            ("this spark", Target::Pointed, B::Pulse, "desk: pulse"),
        ] {
            let intent = parse(said, &v).unwrap_or_else(|m| panic!("{said}: {m:?}"));
            assert_eq!(intent, on(target, look), "{said}");
            assert_eq!(
                apply(&intent, &v, &mut lanes, &boxes).as_deref(),
                Some(label),
                "{said}"
            );
        }
        assert_eq!(lanes.effective(0, &boxes).map(|(b, _)| b), Some(B::Pulse));
        assert_eq!(lanes.effective(1, &boxes).map(|(b, _)| b), Some(B::Curls));
        assert_eq!(lanes.effective(5, &boxes).map(|(b, _)| b), Some(B::Rings));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Board #5462: astrolabe and fenestra are held back. A sentence or an
    /// agent's action naming one changes nothing and says so in one short
    /// sentence; the reply's other actions still apply. No path the voice
    /// takes writes one of the four looks that are not offered.
    #[test]
    fn a_held_back_look_changes_nothing_and_says_so() {
        let dir = std::env::temp_dir().join(format!("fosfora-intent-held-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (e, boxes) = (cut_effects(), room());
        let v = app_vocab(&e, &boxes);
        let id = room_id(boxes.iter().map(|b| b.uuid));
        let mut lanes = RoomLanes::new(dir.clone());
        lanes.update(id, &boxes);
        let before: Vec<_> = (0..boxes.len())
            .map(|k| lanes.effective(k, &boxes))
            .collect();
        for (said, look) in [
            ("Astrolabe on the ceiling.", B::Astrolabe),
            ("fenestra on wall 3", B::Fenestra),
            ("every wall fenestra", B::Fenestra),
            ("this astrolabe", B::Astrolabe),
        ] {
            let m = parse(said, &v).unwrap_err();
            assert_eq!(m.reason, Reason::NotAvailable(look), "{said}");
            // An answer, not a sentence for the agent.
            assert!(!crate::agent::forwards(&m.reason), "{said}");
            let text = miss_text(&m);
            assert_eq!(crate::label::layout(&text).lines.len(), 1, "{text}");
            assert!(text.chars().count() < MAX_EXAMPLE_CHARS, "{text}");
        }
        // The agent's action, the on-device model's too.
        let act = |kind: &str, target: &str, value: &str| AgentAction {
            kind: kind.to_owned(),
            target: target.to_owned(),
            value: value.to_owned(),
        };
        for value in ["astrolabe", "Fenestra"] {
            let m = Intent::from_json(&act("behavior", "ceiling", value), &v).unwrap_err();
            assert!(matches!(m.reason, Reason::NotAvailable(_)), "{value}");
        }
        // An intent naming one, however it was built, writes nothing.
        for look in [B::Astrolabe, B::Fenestra, B::Embers, B::Sparks] {
            for target in [Target::Surface(6), Target::Kind(KIND_WALL), Target::Pointed] {
                let reply = apply(&on(target, look), &v, &mut lanes, &boxes);
                assert_eq!(reply, Some(not_available(look)), "{look:?}");
            }
        }
        let after: Vec<_> = (0..boxes.len())
            .map(|k| lanes.effective(k, &boxes))
            .collect();
        assert_eq!(before, after);
        // Not in the vocabulary's lists either.
        for gone in [B::Embers, B::Sparks, B::Astrolabe, B::Fenestra] {
            assert!(!v.behaviors.contains(&gone), "{gone:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The help card lists nothing that no longer works: no example steps
    /// or names an effect or paints embers, sparks, astrolabe or fenestra
    /// (board #5462), and each one the grammar hears does something.
    #[test]
    fn the_help_lists_nothing_that_is_gone() {
        let e = cut_effects();
        for boxes in [room(), replayed_room()] {
            let v = app_vocab(&e, &boxes);
            let all = shown_pool(&v);
            assert!(all.len() >= 20, "{all:?}");
            assert_eq!(all, pool(&v), "every example parses");
            for example in &all {
                let low = example.to_lowercase();
                for dead in [
                    "effect",
                    "embers",
                    "sparks",
                    "astrolabe",
                    "fenestra",
                    "cloud",
                    "flock",
                    "flux",
                ] {
                    assert!(!low.contains(dead), "\"{example}\" names {dead}");
                }
                for c in parse_clauses(example, &v) {
                    let intent = c.outcome.unwrap_or_else(|m| panic!("{example}: {m:?}"));
                    assert!(
                        !asks_for_effect(&intent),
                        "\"{example}\" asks for an effect"
                    );
                    assert_ne!(reply(&intent, &v), EFFECTS_GONE, "{example}");
                }
            }
            // Every page of the card, round the pool.
            for page in 0..all.len() {
                for line in examples(&v, page) {
                    assert!(all.contains(&line), "{line}");
                }
            }
            // The theme's step, which the thumb swipe takes too, is the
            // first example.
            assert_eq!(all[0], "next theme");
            // A miss is never sent to a phrase that is gone.
            for said in [
                "Switch over to the flock now.",
                "Make it glow like embers.",
                "Give it an astrolabe dial.",
            ] {
                let hinted = hint(said, &v).to_lowercase();
                for dead in [
                    "effect",
                    "embers",
                    "sparks",
                    "astrolabe",
                    "fenestra",
                    "flock",
                    "flux",
                ] {
                    assert!(!hinted.contains(dead), "{said}: {hinted}");
                }
            }
        }
    }
}
