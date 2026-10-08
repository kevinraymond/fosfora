//! The voice path, V2: the grammar (board #3751, `docs/xr/VOICE_DESIGN.md`,
//! "V2 as built"). The sentence V1 transcribes becomes one [`Intent`]: an
//! action the hand menu or the room editor already performs, reached by
//! voice. Nothing here is new behavior: every variant names its hand-driven
//! equivalent.
//!
//! **Data plus one matcher.** The words come from what the app holds at the
//! moment of the sentence ([`Vocabulary`]): the world effects' names, the
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
    SurfaceBehavior, color_name, friendly_name, kind_name,
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
        .enumerate()
        .map(|(k, b)| Surface {
            name: friendly_name(k, boxes),
            kind: b.kind,
            index: k,
        })
        .collect()
}

/// What the grammar can name, built at the moment of the sentence.
#[derive(Debug, Clone)]
pub struct Vocabulary<'a> {
    /// The world effects, in the hand menu's order ("Flux Cloud", "Embers",
    /// "Flock").
    pub effects: &'a [String],
    /// The behaviors a surface can run (`SurfaceBehavior::ALL`).
    pub behaviors: &'a [SurfaceBehavior],
    /// The room's surfaces, one per lane box.
    pub surfaces: Vec<Surface>,
    /// The lane box under the room editor's beam, if any.
    pub pointed: Option<usize>,
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

/// What a sentence asks for. Each variant does what a hand already does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// The hand menu's effect row, `>`.
    NextEffect,
    /// The hand menu's effect row, `<`.
    PrevEffect,
    /// The world effect at this index, as stepping the effect row to it.
    Effect(usize),
    /// The hand menu's "Edit room" toggle.
    EditRoom(bool),
    /// The hand menu's "Particles" toggle (the cloud, `Controls::cloud`).
    Cloud(bool),
    /// The hand menu's "Pitcher" toggle.
    Pitcher(bool),
    /// The hand menu's Music row: play or stop.
    Music(bool),
    /// The hand menu's "Rescan the room".
    Rescan,
    /// The hand menu's "Recenter".
    Recenter,
    /// The hand menu's "All: none".
    AllNone,
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
const RECENTER: [&str; 6] = [
    "recenter cloud",
    "recenter particles",
    "recenter",
    "re center cloud",
    "re center particles",
    "re center",
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
/// stage floor are one floor to the eye), else to [`Aim::Several`].
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

/// The behavior slot: one word, a behavior's name in the vocabulary (a
/// trailing "s" forgiven: "streamline", "curl"), or none's words.
fn behavior(ts: &[&str], vocab: &Vocabulary<'_>) -> Option<SurfaceBehavior> {
    let &[w] = ts else { return None };
    if NONE_WORDS.contains(&w) {
        return Some(SurfaceBehavior::None);
    }
    let known =
        |name: &str| SurfaceBehavior::from_name(name).filter(|b| vocab.behaviors.contains(b));
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
/// not the question which one).
fn fill(slot: Slot, target_words: &[&str], vocab: &Vocabulary<'_>) -> Result<Intent, Reason> {
    let check = |kind: Option<u32>| match (slot, kind) {
        (Slot::Behavior(b), Some(kind)) if !SurfaceBehavior::catalogue(kind).contains(&b) => {
            Err(Reason::NotOnThisKind { behavior: b, kind })
        }
        _ => Ok(()),
    };
    match aim(target_words, vocab) {
        Aim::Hit(target) => {
            check(kind_of(target, vocab))?;
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

/// The menu's phrases: the effect row, the toggles and the buttons.
fn menu(ts: &[&str], vocab: &Vocabulary<'_>) -> Option<Intent> {
    if any(ts, &HELP) {
        return Some(Intent::Help);
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
    if any(ts, &ALL_NONE) {
        return Some(Intent::AllNone);
    }
    if let Some(t) = toggle(ts) {
        return Some(t);
    }
    effect(ts, vocab).map(Intent::Effect)
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
/// reads), the pitcher, the music.
fn device(w: &str) -> Option<fn(bool) -> Intent> {
    match w {
        "cloud" | "particles" => Some(Intent::Cloud),
        "pitcher" => Some(Intent::Pitcher),
        "music" => Some(Intent::Music),
        _ => None,
    }
}

/// A world effect by name, alone or after a switching verb.
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

/// The surface templates, in order: "what is {target}", "clear
/// {target}", `{x} on {target}` and `{target} on {band}`, `{target} in
/// {color}`, `{target} follows {band}`, then `{target} {x}` with the
/// target possibly absent (the pointed surface). The first that fits
/// whole is the intent; a template whose slot fits but whose target does
/// not gives the reason, the first such one being the answer when nothing
/// fits.
fn surface(ts: &[&str], vocab: &Vocabulary<'_>) -> Result<Intent, Reason> {
    match ts {
        ["what", "is" | "s", rest @ ..] | ["whats" | "describe", rest @ ..] => {
            let rest = rest.strip_prefix(&["on"]).unwrap_or(rest);
            return match aim(rest, vocab) {
                Aim::Hit(t) => Ok(Intent::Describe(t)),
                Aim::Several(ks) => Err(Reason::Ambiguous(names(&ks, vocab))),
                Aim::Miss(r) => Err(r),
            };
        }
        ["clear", rest @ ..] if !rest.is_empty() => {
            return fill(Slot::Behavior(SurfaceBehavior::None), rest, vocab);
        }
        _ => {}
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
/// effect row, the toggles, the buttons, a world effect by name), then the
/// surface templates.
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
    surface(&ts, vocab).map_err(miss)
}

/// The most clauses one sentence asks for (V4): "amber on the desk and
/// the walls on the bass" is two; more than this is [`Reason::TooMany`].
pub const MAX_CLAUSES: usize = 3;
/// How long the label shows "what can I say"'s examples (s).
pub const HELP_S: f32 = 4.0;
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
        .chain((1..=COLOR_KEY).map(|i| color_name(i).to_owned()));
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
/// color, a band, a strength and "what is"), the world effects and the
/// kinds ("every wall aurora"). Names come from the vocabulary, so each
/// one is a surface or an effect of this room.
fn pool(vocab: &Vocabulary<'_>) -> Vec<String> {
    let menu: Vec<String> = [
        "next effect",
        "music play and next effect",
        "edit the room",
        "particles off",
        "previous effect",
        "music stop",
        "recenter the cloud",
        "rescan the room",
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
    let effects: Vec<String> = vocab
        .effects
        .iter()
        .map(|e| {
            let switch = format!("switch to {e}");
            if switch.chars().count() <= MAX_EXAMPLE_CHARS {
                switch
            } else {
                e.clone()
            }
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
    let lists = [menu, surfaces, effects, kinds];
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
    let (mut missed, mut helped) = (false, false);
    for c in clauses {
        match &c.outcome {
            Ok(Intent::Help) => {
                extra.extend(examples(vocab, *help_page));
                *help_page = help_page.wrapping_add(1);
                helped = true;
            }
            Ok(intent) => parts.push(apply(intent)),
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
    /// [`Reason::NotOnThisKind`]; an unknown kind, effect, color, band or
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
            "prev_effect" => Ok(Self::PrevEffect),
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
/// spectrum", "desk: amber", "next effect".
pub fn reply(intent: &Intent, vocab: &Vocabulary<'_>) -> String {
    let on_off = |on: bool| if on { "on" } else { "off" };
    let of = |t: Target, what: &str| format!("{}: {what}", target_name(t, vocab));
    match *intent {
        Intent::NextEffect => "next effect".to_owned(),
        Intent::PrevEffect => "previous effect".to_owned(),
        Intent::Effect(i) => vocab.effects.get(i).cloned().unwrap_or_default(),
        Intent::EditRoom(on) => format!("edit room {}", on_off(on)),
        Intent::Cloud(on) => format!("particles {}", on_off(on)),
        Intent::Pitcher(on) => format!("pitcher {}", on_off(on)),
        Intent::Music(on) => format!("music {}", on_off(on)),
        Intent::Rescan => "rescanning the room".to_owned(),
        Intent::Recenter => "cloud recentered".to_owned(),
        Intent::AllNone => "every surface: none".to_owned(),
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

/// The label for a miss: "Didn't catch that" with the sentence, or what
/// was missing.
pub fn miss_text(miss: &Miss) -> String {
    match &miss.reason {
        Reason::NoMatch => format!("{}: \"{}\"", crate::voice::NOTHING_LABEL, miss.heard),
        Reason::NoSurface => "Point at a surface, or name one".to_owned(),
        Reason::UnknownSurface(name) => format!("No surface called \"{name}\""),
        Reason::UnknownBehavior(name) => format!("No behavior called \"{name}\""),
        Reason::NotOnThisKind { behavior, kind } => {
            format!("{} doesn't run on {}", behavior.name(), a_kind(*kind))
        }
        Reason::Ambiguous(names) => format!("Which one: {}?", listed(names)),
        Reason::TooMany(_) => TOO_MANY_LABEL.to_owned(),
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
/// surface of it in turn.
pub fn apply(
    intent: &Intent,
    vocab: &Vocabulary<'_>,
    lanes: &mut RoomLanes,
    boxes: &[LaneBox<'_>],
) -> Option<String> {
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

/// What `target` runs, for "what is this": one surface's behavior, color,
/// band and strength (`label::param_text`); a kind's behaviors, "all
/// tables: streamlines, curls".
fn describe(
    target: Target,
    ks: &[usize],
    vocab: &Vocabulary<'_>,
    lanes: &RoomLanes,
    boxes: &[LaneBox<'_>],
) -> String {
    let name = target_name(target, vocab);
    match (target, ks) {
        (Target::Kind(_), _) | (_, [] | [_, _, ..]) => {
            let mut runs: Vec<&str> = Vec::new();
            for &k in ks {
                let b = lanes.effective(k, boxes).map_or("none", |(b, _)| b.name());
                if !runs.contains(&b) {
                    runs.push(b);
                }
            }
            if runs.is_empty() {
                format!("{name}: none in the room")
            } else {
                format!("{name}: {}", runs.join(", "))
            }
        }
        (_, &[k]) => {
            let behavior = lanes
                .effective(k, boxes)
                .map(|(b, _)| b)
                .unwrap_or_default();
            crate::label::param_text(&name, behavior, lanes.params_of(k, boxes))
        }
    }
}

/// The `debug.fosfora.say` knob (board #3751): a sentence fed to the
/// grammar as if heard, for the unworn gate. A value is fed once, when it
/// differs from the last one fed; an app cannot clear a `debug.` property,
/// so to say the same sentence again, set it to "" first (or to another
/// sentence). While the room is on but has no anchors yet (`waiting`) the
/// value waits for them, as the `surface` knob's does.
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

    /// The fixture's vocabulary, the desk pointed at or `pointed`.
    fn vocab<'a>(
        effects: &'a [String],
        boxes: &[LaneBox<'_>],
        pointed: Option<usize>,
    ) -> Vocabulary<'a> {
        Vocabulary {
            effects,
            behaviors: &SurfaceBehavior::ALL,
            surfaces: surfaces(boxes),
            pointed,
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
                ("Embers on the desk.", on(Target::Surface(0), B::Embers)),
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
                ("that sparks", on(Target::Pointed, B::Sparks)),
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
            ],
        );
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
                ("every desk sparks", on(Target::Kind(KIND_TABLE), B::Sparks)),
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
                // Embers fall off volumes: a wall's catalogue lacks them,
                // whichever wall was meant.
                (
                    "Embers on the wall.",
                    Reason::NotOnThisKind {
                        behavior: B::Embers,
                        kind: KIND_WALL,
                    },
                ),
                (
                    "spectrum on the desk",
                    Reason::NotOnThisKind {
                        behavior: B::Spectrum,
                        kind: KIND_TABLE,
                    },
                ),
                (
                    "every ceiling sparks",
                    Reason::NotOnThisKind {
                        behavior: B::Sparks,
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
            (Intent::NextEffect, "next effect"),
            (Intent::PrevEffect, "previous effect"),
            (Intent::Effect(1), "Embers"),
            (Intent::EditRoom(true), "edit room on"),
            (Intent::EditRoom(false), "edit room off"),
            (Intent::Cloud(false), "particles off"),
            (Intent::Pitcher(true), "pitcher on"),
            (Intent::Music(true), "music on"),
            (Intent::Music(false), "music off"),
            (Intent::Rescan, "rescanning the room"),
            (Intent::Recenter, "cloud recentered"),
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
                    behavior: B::Embers,
                    kind: KIND_WALL,
                },
                "embers doesn't run on a wall",
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
            Some("all tables: pulse, curls")
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
            "Embers on wall 2.",
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
            "voice: heard \"Next effect.\" → NextEffect · label \"next effect\"",
            "voice: heard \"Embers.\" → Effect(1) · label \"Embers\"",
            "voice: heard \"Edit the room.\" → EditRoom(true) · label \"edit room on\"",
            "voice: heard \"Every table streamlines.\" → Behavior { target: Kind(1), behavior: Streamlines } · label \"all tables: streamlines\"",
            "voice: heard \"Table 14 in amber.\" → Color { target: Surface(14), color: 4 } · label \"table 14: amber\"",
            "voice: heard \"Wall 5 on the bass.\" → Band { target: Surface(5), band: 1 } · label \"wall 5: bass\"",
            "voice: heard \"Rings on the floor.\" → Behavior { target: Kind(2), behavior: Rings } · label \"all floors: rings\"",
            "voice: heard \"What is table 14?\" → Describe(Surface(14)) · label \"table 14: streamlines · amber · rms · 1.0\"",
            "voice: heard \"Embers on wall 2.\" → miss NotOnThisKind { behavior: Embers, kind: 3 } · label \"embers doesn't run on a wall\"",
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
        assert_eq!(clauses[1].outcome, Ok(on(Target::Surface(0), B::Embers)));
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
            [Ok(on(Target::Surface(0), B::Embers)), Err(Reason::NoMatch)]
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
        // The examples name this room's surfaces and effects.
        let boxes = replayed_room();
        let v = vocab(&e, &boxes, None);
        assert_eq!(
            examples(&v, 0),
            [
                "next effect",
                "streamlines on table 0",
                "switch to Flux Cloud",
                "every table curls",
                "music play and next effect",
            ]
        );
    }

    #[test]
    fn the_hint_is_the_nearest_example() {
        let (e, boxes) = (effects(), replayed_room());
        let v = vocab(&e, &boxes, None);
        // Words in common: the example that shares the most.
        assert_eq!(
            hint("Switch over to the flock now.", &v),
            "Try \"switch to Flock\""
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
            "desk: embers \u{b7} Didn't catch that: \"flibber jabber.\"\nTry \"what can I say\""
        );
        assert_close!(r.seconds, MISS_S);
        assert!(
            line.ends_with(
                "label \"desk: embers \u{b7} Didn't catch that: \"flibber jabber.\" | Try \"what can I say\"\""
            ),
            "{line}"
        );
        // The agent on: the clause that misses goes to it, alone.
        let (r, _) = say("Embers on the desk and make it cozy.", true, &mut page);
        assert_eq!(r.label, "desk: embers \u{b7} Thinking\u{2026}");
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
        assert_eq!(r.label, "next effect \u{b7} No surface called \"shelf\"");
        assert_eq!(r.agent, None);
        // Help: five lines, the page turned.
        let before = page;
        let (r, _) = say("What can I say?", false, &mut page);
        assert_eq!(r.label.lines().count(), HELP_LINES);
        assert_close!(r.seconds, HELP_S);
        assert_eq!(page, before + 1);
        assert_eq!(lanes.params_of(0, &boxes).0, 4);
        assert_eq!(lanes.params_of(3, &boxes).1, 1);
        assert_eq!(lanes.effective(0, &boxes).map(|(b, _)| b), Some(B::Embers));
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
                "voice: heard \"What can I say?\" \u{2192} Help \u{b7} label \"next effect | streamlines on table 0 | switch to Flux Cloud | every table curls | music play and next effect\"",
                "next effect\nstreamlines on table 0\nswitch to Flux Cloud\nevery table curls\nmusic play and next effect",
                HELP_S,
            ),
            (
                "voice: heard \"Help.\" \u{2192} Help \u{b7} label \"storage 1 in blue | switch to Embers | every wall rings | edit the room | wall 2 on the bass\"",
                "storage 1 in blue\nswitch to Embers\nevery wall rings\nedit the room\nwall 2 on the bass",
                HELP_S,
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
}
