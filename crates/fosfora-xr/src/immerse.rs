//! Immersion (board #5384, #5409): the room gives way to the music. A level
//! from 0 (the plain room) to 1 (as dark as the wearer allows) dims the
//! passthrough image and raises the lit surfaces' alpha, so the room shows
//! while the song is quiet and the surfaces glow at their own colors over a
//! dark room while it is full. Plain numbers, so it builds and tests on the
//! desktop; `app.rs` steps it once a frame, shows its opening message on the
//! in-world label, sets the passthrough layer's style through `xr.rs` and
//! lifts the surfaces' peak alpha with [`lift_rows`].
//!
//! **The opening** ([`Opening`]). At launch the level is 0. When real audio
//! first drives the analysis and no other label is up, the label says
//! [`OPEN_TEXT`] for [`OPEN_HOLD_S`] with the room plain, then the level
//! glides to [`OPEN_LEVEL`] over [`OPEN_GLIDE_S`]: the show starts there.
//! Once per launch. The scan's and the permission's labels go first, and the
//! voice's and the editor's cover it; its hold counts only while it shows.
//! A pinned level, `debug.fosfora.immerseopen 0` ([`Immersion::skip_opening`])
//! and the depth off skip it.
//!
//! **What it follows: the song against itself** ([`History`], [`position`],
//! [`follow`]). Mastered music is loud all the way through, so an absolute
//! map of `loudness_s` sits at full. The level follows where `loudness_s` is
//! within its own last [`WINDOW_S`]: a robust low and high (the
//! [`RANGE_LOW_P`] and [`RANGE_HIGH_P`] percentiles), at least [`MIN_SPAN`]
//! apart, the span widened downward so a flat song sits at its top. The top
//! [`TOP_SHARE`] of the range is full. The position maps onto
//! [`LOW_LEVEL`]..1 (`debug.fosfora.immerselow`, live: [`low_knob`]), and
//! `buildup` over [`BUILD_FROM`] adds up to
//! [`BUILDUP_LIFT`], scaled by the position, so a build closes the room in
//! while a breakdown, whose missing bass also reads as tension, still opens
//! it. A `drop` holds the target at 1 for [`DROP_HOLD_S`], reached with
//! [`DROP_ATTACK_S`]. It may park: a song that stays full stays closed.
//!
//! **The history** records `loudness_s` at [`HISTORY_HZ`] only while the
//! song is heard and [`LOUDNESS_WARM_S`] after it came back, the time its
//! window needs to mean the song again. At launch it is empty, so the target
//! blends from [`OPEN_LEVEL`] into the followed one as it fills over
//! [`HISTORY_FILL_S`]. On a track change ([`Immersion::new_track`]) it
//! carries over, but the range comes from no more than the samples since
//! the change (at least [`TRACK_ADAPT_S`] of them) until the window is the
//! new song's. A [`SongPlan`], when one is supplied, gives the range instead.
//!
//! **The smoothing.** One pole toward the target, with [`ATTACK_S`] while it
//! rises and [`RELEASE_S`] while it falls, so the room breathes with the
//! sections and never flickers on a beat. For [`RESUME_S`] after the music
//! comes back (a stop, a silence, a pause of the session) the time constant
//! is a third of that, so the room glides to the song's level in about 2 s.
//!
//! **Quiet.** With no real audio driving the analysis, or `loudness_m` under
//! [`QUIET_LOUDNESS`] for [`QUIET_HOLD_S`], the target is 0 with
//! [`QUIET_RELEASE_S`]: the room is back within about 1.5 s. The `rms` this
//! read before is auto-leveled and dips under any fixed threshold between
//! hits inside the music.
//!
//! **The depth** ([`Depth`]): how far the wearer lets it go, off (0), low
//! (0.5) or full (1), multiplied into the level, the opening's included. A
//! change glides over [`DEPTH_S`].
//!
//! **What it drives.** The passthrough image's opacity,
//! `1 - level * (1 - floor)` ([`Immersion::opacity`]), with the floor the
//! darkest the room may get ([`IMMERSE_FLOOR`], 0: fully closed; a parameter
//! of the type, so roam can raise it); the style call goes to the runtime
//! only when the opacity moves by a step of 1/[`OPACITY_STEPS`]
//! ([`StyleGate`]). And the lit surfaces' peak alpha ([`peak_alpha`]): a
//! ported effect's rises from its 0.5 to [`ALPHA_FULL`], the built-in
//! behaviors' by the same ratio, capped at 1.

/// The wearer's depth setting: the hand menu's Immersion control and the
/// voice's "darker" and "lighter".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Depth {
    /// The plain room: the level stays 0.
    Off,
    /// Half way.
    Low,
    /// As dark as the floor allows.
    #[default]
    Full,
}

impl Depth {
    /// In the menu's cycle order.
    pub const ALL: [Self; 3] = [Self::Off, Self::Low, Self::Full];

    /// The multiplier on the level.
    pub fn value(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Low => 0.5,
            Self::Full => 1.0,
        }
    }

    /// Its name in the log, `hand_menu.json` and the voice's replies.
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Full => "full",
        }
    }

    /// The hand menu's words: the state in the words, as the other rows.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Immersion: off",
            Self::Low => "Immersion: low",
            Self::Full => "Immersion: full",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.name() == name)
    }

    /// The menu's press: off, low, full, then off again.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Low,
            Self::Low => Self::Full,
            Self::Full => Self::Off,
        }
    }

    /// One step darker ("darker"), full at most.
    #[must_use]
    pub fn deeper(self) -> Self {
        match self {
            Self::Off => Self::Low,
            Self::Low | Self::Full => Self::Full,
        }
    }

    /// One step lighter ("lighter"), off at least.
    #[must_use]
    pub fn lighter(self) -> Self {
        match self {
            Self::Full => Self::Low,
            Self::Low | Self::Off => Self::Off,
        }
    }

    /// The named depth nearest a multiplier (a launch override between two).
    pub fn nearest(value: f32) -> Self {
        let off_by = |d: Self| (d.value() - value).abs();
        Self::ALL
            .into_iter()
            .min_by(|a, b| off_by(*a).total_cmp(&off_by(*b)))
            .unwrap_or_default()
    }
}

/// The opening's words on the in-world label.
pub const OPEN_TEXT: &str = "Sit back and relax";
/// How long the opening's message shows with the room plain (s).
pub const OPEN_HOLD_S: f32 = 4.0;
/// The glide from the plain room to [`OPEN_LEVEL`] after it (s).
pub const OPEN_GLIDE_S: f32 = 3.0;
/// Where the show starts.
pub const OPEN_LEVEL: f32 = 0.35;
/// After the music comes back, the level reaches the song's within about
/// this (s): the time constant is a third of it, 95 % of the way.
pub const RESUME_S: f32 = 2.0;

/// The level of a song at the bottom of its own range, a breakdown: a
/// little more than half the room back, so a breakdown, after the release,
/// sits about half dimmed (worn, 0.2 brought the room back too bright).
/// Over [`OPEN_LEVEL`], so the music never shows the room as much as the
/// opening does, and the blend from it after the opening only darkens.
pub const LOW_LEVEL: f32 = 0.42;
/// How far back the history looks (s): longer than a breakdown, so the
/// range keeps the full section's high through one.
pub const WINDOW_S: f32 = 30.0;
/// The history's samples of `loudness_s` a second.
pub const HISTORY_HZ: f32 = 4.0;
/// The history's samples, [`WINDOW_S`] of them.
pub const HISTORY_LEN: usize = (WINDOW_S * HISTORY_HZ) as usize;
/// The range's low and high: these percentiles of the history, so a few
/// seconds of a spike or a dip do not stretch it.
pub const RANGE_LOW_P: f32 = 0.1;
pub const RANGE_HIGH_P: f32 = 0.9;
/// The narrowest the range gets (`loudness_s`, 3 dB): a narrower one is
/// widened downward from its high, so a flat song sits at its top and the
/// noise within a section does not swing the level.
pub const MIN_SPAN: f32 = 0.05;
/// The top share of the range that counts as full: the wobble within a full
/// section keeps the room closed.
pub const TOP_SHARE: f32 = 0.25;
/// `buildup` over this reads as tension; within a full section it stays
/// under.
pub const BUILD_FROM: f32 = 0.4;
/// What a full `buildup` adds to the level, scaled by the position: at the
/// bottom of the range, nothing.
pub const BUILDUP_LIFT: f32 = 1.0;
/// After the music comes back, `loudness_s` needs its 3 s window before it
/// reads the song (s): the history does not record until then.
pub const LOUDNESS_WARM_S: f32 = 3.0;
/// The target blends from [`OPEN_LEVEL`] into the followed level as the
/// history fills up to this many seconds.
pub const HISTORY_FILL_S: f32 = 15.0;
/// After a track change the range comes from the samples since the change,
/// and at least this many seconds of the latest (s).
pub const TRACK_ADAPT_S: f32 = 4.0;
/// The time constants (s) while the level rises and while it falls.
pub const ATTACK_S: f32 = 2.0;
pub const RELEASE_S: f32 = 4.0;
/// A drop: the target held at 1 this long (s), reached with this time
/// constant (s).
pub const DROP_HOLD_S: f32 = 8.0;
pub const DROP_ATTACK_S: f32 = 0.25;
/// Quiet: `loudness_m` under this (−51 LUFS, 4 dB over the analysis's
/// silence gate) for [`QUIET_HOLD_S`] (s); the level then falls with the
/// time constant [`QUIET_RELEASE_S`] (s), to under 0.05 of its depth in
/// 0.3 + 3 x 0.35 = 1.35 s.
pub const QUIET_LOUDNESS: f32 = 0.15;
pub const QUIET_HOLD_S: f32 = 0.3;
pub const QUIET_RELEASE_S: f32 = 0.35;
/// A change of the depth setting glides with this time constant (s).
pub const DEPTH_S: f32 = 0.5;
/// The darkest the room may get: the passthrough image's opacity at level
/// 1 (`debug.fosfora.immersefloor`). 0: the room fully closed.
pub const IMMERSE_FLOOR: f32 = 0.0;
/// The style call's steps over the opacity's 0..1.
pub const OPACITY_STEPS: u32 = 64;
/// A ported effect's peak alpha at level 1 (`surface_port::PEAK_ALPHA`, 0.5,
/// is its level-0 value).
pub const ALPHA_FULL: f32 = 1.0;
/// The `color` row of the surfaces pass's and the ported effects' uniform
/// blocks (`surface_fx::rows`, `surface_port::rows`): its w is the face's
/// peak alpha.
pub const PEAK_ROW: usize = 6;

/// The history's samples since a track change that the range takes at
/// least.
const ADAPT_LEN: usize = (TRACK_ADAPT_S * HISTORY_HZ) as usize;

/// What the frame loop hands the level each frame, from this frame's
/// `AudioFeatures` and the label.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Input {
    /// The short-term loudness (3 s), 0..1.
    pub loudness_s: f32,
    /// The momentary loudness (0.4 s), 0..1: silence is under
    /// [`QUIET_LOUDNESS`].
    pub loudness_m: f32,
    /// The riser's tension, 0..1.
    pub buildup: f32,
    /// The drop fired.
    pub drop: bool,
    /// Real audio drives the analysis: the music's tap or the microphones,
    /// not the synthetic groove.
    pub playing: bool,
    /// Another label is up (the voice's, the editor's, the scan's or the
    /// permission's): the opening's message waits.
    pub label_busy: bool,
    /// The playing track's position (s), the time a [`SongPlan`]'s cues are
    /// against; 0 when nothing plays.
    pub position_s: f32,
}

/// What a pass over the whole track before it plays could hand the level:
/// the track's range and its cues. Nothing makes one yet; without it the
/// range comes from the history and the cues from the live `buildup` and
/// `drop`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SongPlan {
    /// The track's `loudness_s` low and high over its whole length.
    pub range: (f32, f32),
    /// Timed cues, sorted by time. Not read yet.
    pub cues: Vec<Cue>,
}

/// A moment in a [`SongPlan`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cue {
    /// The track's position (s).
    pub at_s: f32,
    pub kind: CueKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueKind {
    BuildStart,
    Drop,
    BreakdownStart,
    BreakdownEnd,
}

/// Where the opening is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Opening {
    /// Not played yet: it waits for the music and a free label.
    Waiting,
    /// [`OPEN_TEXT`] is up, the room plain; `shown_s` counts only while no
    /// other label covers it.
    Message { shown_s: f32 },
    /// The level glides to [`OPEN_LEVEL`]; `s` since the glide started.
    Glide { s: f32 },
    /// The show: the level follows the song (or the opening was skipped).
    Done,
}

/// Something the log says when it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The opening's message went up.
    Opening,
    /// The music started with the depth off: no opening.
    OpeningSkipped,
    /// The glide reached [`OPEN_LEVEL`]: the level follows the song.
    Show,
    /// A drop: the level to 1 for [`DROP_HOLD_S`].
    Drop,
}

/// `x` in 0..1, and 0 for anything that is not a number.
fn unit(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// `from` moved toward `to` over `dt` with time constant `tau` (s).
fn approach(from: f32, to: f32, dt: f32, tau: f32) -> f32 {
    from + (to - from) * (1.0 - (-dt / tau).exp())
}

/// The value at `p` (0..1) of `sorted`, between its neighbors.
fn percentile(sorted: &[f32], p: f32) -> f32 {
    let last = sorted.len().saturating_sub(1);
    let at = p * last as f32;
    let i = (at.floor() as usize).min(last);
    let j = (i + 1).min(last);
    sorted[i] + (sorted[j] - sorted[i]) * (at - i as f32)
}

/// Where `loudness_s` is within `range` (low, high), 0..1: the range at
/// least [`MIN_SPAN`] wide, widened downward from its high, and its top
/// [`TOP_SHARE`] all 1.
pub fn position(loudness_s: f32, range: (f32, f32)) -> f32 {
    let high = unit(range.1);
    let low = unit(range.0).min(high - MIN_SPAN);
    let along = ((unit(loudness_s) - low) / (high - low)).clamp(0.0, 1.0);
    (along / (1.0 - TOP_SHARE)).min(1.0)
}

/// The level the song asks for at `position`, before the smoothing and the
/// depth: `low` (the low level, [`LOW_LEVEL`] unless the knob says) to 1
/// along it, plus [`BUILDUP_LIFT`] times the share of `buildup` over
/// [`BUILD_FROM`], times the position; at most 1.
pub fn follow(position: f32, buildup: f32, low: f32) -> f32 {
    let p = unit(position);
    let low = unit(low);
    let tension = ((unit(buildup) - BUILD_FROM) / (1.0 - BUILD_FROM)).max(0.0);
    (low + (1.0 - low) * p + BUILDUP_LIFT * tension * p).min(1.0)
}

/// `debug.fosfora.immerselow`'s value as the low level: a number, clamped
/// to 0..1; `None` when it is unset or not a number (then [`LOW_LEVEL`]).
pub fn low_knob(value: Option<&str>) -> Option<f32> {
    value
        .and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(0.0, 1.0))
}

/// `loudness_s` over the last [`WINDOW_S`], and its range.
#[derive(Debug, Clone)]
pub struct History {
    /// A ring of [`HISTORY_LEN`]; `next` is where the next sample goes.
    samples: [f32; HISTORY_LEN],
    next: usize,
    len: usize,
    /// Time toward the next sample (s).
    due_s: f32,
    /// Samples since a track change, until they fill the window.
    since_change: Option<usize>,
    /// The range of the samples the window takes now.
    range: Option<(f32, f32)>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            samples: [0.0; HISTORY_LEN],
            next: 0,
            len: 0,
            due_s: 0.0,
            since_change: None,
            range: None,
        }
    }
}

impl History {
    /// `dt` of the song heard at `loudness_s`: a sample every
    /// 1/[`HISTORY_HZ`].
    pub fn feed(&mut self, dt: f32, loudness_s: f32) {
        let period = 1.0 / HISTORY_HZ;
        self.due_s += dt;
        if self.due_s >= period {
            self.due_s = (self.due_s - period).min(period);
            self.push(loudness_s);
        }
    }

    /// One sample.
    pub fn push(&mut self, loudness_s: f32) {
        self.samples[self.next] = unit(loudness_s);
        self.next = (self.next + 1) % HISTORY_LEN;
        self.len = (self.len + 1).min(HISTORY_LEN);
        if let Some(n) = self.since_change.as_mut() {
            *n += 1;
            if *n >= HISTORY_LEN {
                self.since_change = None;
            }
        }
        self.update();
    }

    /// A new track: the samples so far stay, but the range comes from the
    /// ones since now, at least [`TRACK_ADAPT_S`] of the latest.
    pub fn new_track(&mut self) {
        self.since_change = Some(0);
        self.update();
    }

    /// How many of the latest samples the range takes.
    fn window(&self) -> usize {
        self.since_change
            .map_or(self.len, |n| n.max(ADAPT_LEN).min(self.len))
    }

    fn update(&mut self) {
        let n = self.window();
        if n == 0 {
            self.range = None;
            return;
        }
        let mut latest = [0.0f32; HISTORY_LEN];
        for (k, v) in latest[..n].iter_mut().enumerate() {
            *v = self.samples[(self.next + HISTORY_LEN - 1 - k) % HISTORY_LEN];
        }
        let latest = &mut latest[..n];
        latest.sort_unstable_by(f32::total_cmp);
        self.range = Some((
            percentile(latest, RANGE_LOW_P),
            percentile(latest, RANGE_HIGH_P),
        ));
    }

    /// The low and high of the window, `None` before the first sample.
    pub fn range(&self) -> Option<(f32, f32)> {
        self.range
    }

    /// The seconds of song recorded, up to [`WINDOW_S`].
    pub fn seconds(&self) -> f32 {
        self.len as f32 / HISTORY_HZ
    }

    /// How far the followed level counts, 0..1, as the history fills over
    /// [`HISTORY_FILL_S`].
    pub fn fill(&self) -> f32 {
        (self.seconds() / HISTORY_FILL_S).min(1.0)
    }
}

/// The immersion level.
#[derive(Debug, Clone)]
pub struct Immersion {
    /// The wearer's setting, the multiplier on the level.
    depth: f32,
    /// The depth as it glides toward `depth`.
    shown_depth: f32,
    /// The passthrough image's opacity at level 1.
    floor: f32,
    /// The level at the bottom of the song's range ([`LOW_LEVEL`], or
    /// `debug.fosfora.immerselow`).
    low: f32,
    /// The music's level, smoothed, before the depth.
    music: f32,
    /// What [`Self::step`] returned last.
    level: f32,
    /// The drop's hold left (s).
    drop_s: f32,
    /// How long it has been quiet (s).
    quiet_s: f32,
    /// How long the song has been heard since it was last quiet (s).
    heard_s: f32,
    /// Since the music came back or the session resumed (s): under
    /// [`RESUME_S`], the level glides.
    back_s: f32,
    opening: Opening,
    /// Why the opening was skipped before it played, for the launch line.
    open_skip: Option<&'static str>,
    history: History,
    /// The range and the position the last step used, for the log.
    range: Option<(f32, f32)>,
    position: f32,
    /// What the log has not taken yet.
    events: Vec<Event>,
    /// `debug.fosfora.immersefix`: the level pinned, whatever the audio
    /// and the depth.
    fixed: Option<f32>,
}

impl Immersion {
    /// At `depth` (0..1) with the room at `floor` (0..1) at most dark.
    pub fn new(depth: f32, floor: f32) -> Self {
        let depth = unit(depth);
        Self {
            depth,
            shown_depth: depth,
            floor: unit(floor),
            low: LOW_LEVEL,
            music: 0.0,
            level: 0.0,
            drop_s: 0.0,
            quiet_s: 0.0,
            heard_s: 0.0,
            back_s: 0.0,
            opening: Opening::Waiting,
            open_skip: None,
            history: History::default(),
            range: None,
            position: 0.0,
            events: Vec::new(),
            fixed: None,
        }
    }

    pub fn depth(&self) -> f32 {
        self.depth
    }

    /// The wearer changed the depth: the level glides to it.
    pub fn set_depth(&mut self, depth: f32) {
        self.depth = unit(depth);
    }

    pub fn floor(&self) -> f32 {
        self.floor
    }

    pub fn set_floor(&mut self, floor: f32) {
        self.floor = unit(floor);
    }

    pub fn low(&self) -> f32 {
        self.low
    }

    /// The level at the bottom of the song's range, 0..1 (the live knob):
    /// the level moves to it over the attack or the release.
    pub fn set_low(&mut self, low: f32) {
        self.low = unit(low);
    }

    /// The log line for a change of the low level: "immersion: low level
    /// 0.45 (debug.fosfora.immerselow)", or the constant's when the knob
    /// went unset.
    pub fn low_line(&self, from_knob: bool) -> String {
        let why = if from_knob {
            "debug.fosfora.immerselow"
        } else {
            "the default"
        };
        format!("immersion: low level {:.2} ({why})", self.low)
    }

    /// Pin the level (`Some`, 0..1) whatever the audio and the depth, or
    /// let it follow the music again (`None`). A pin skips the opening.
    pub fn fix(&mut self, level: Option<f32>) {
        self.fixed = level.map(unit);
        if self.fixed.is_some() {
            self.skip_opening("pinned");
        }
    }

    pub fn fixed(&self) -> Option<f32> {
        self.fixed
    }

    /// No opening this launch (`why`, for the launch line): the music's
    /// start glides to the show as a resume does.
    pub fn skip_opening(&mut self, why: &'static str) {
        if self.opening != Opening::Done {
            self.opening = Opening::Done;
            self.open_skip = Some(why);
        }
    }

    pub fn opening(&self) -> Opening {
        self.opening
    }

    /// The opening's message and its alpha while it is up, fading over the
    /// label's last `label::FADE_S`.
    pub fn message(&self) -> Option<(&'static str, f32)> {
        match self.opening {
            Opening::Message { shown_s } => {
                Some((OPEN_TEXT, crate::label::fade_for(shown_s, OPEN_HOLD_S)))
            }
            _ => None,
        }
    }

    /// The session ran again after a pause: the level glides to the song's
    /// over [`RESUME_S`].
    pub fn resume(&mut self) {
        self.back_s = 0.0;
    }

    /// The player put a new track on: the history adapts to it.
    pub fn new_track(&mut self) {
        self.history.new_track();
    }

    /// The range the last step followed (the plan's or the history's), and
    /// where the song was in it.
    pub fn range(&self) -> Option<(f32, f32)> {
        self.range
    }

    pub fn position(&self) -> f32 {
        self.position
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    /// What happened since the last call, oldest first.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// The level [`Self::step`] returned last.
    pub fn level(&self) -> f32 {
        self.level
    }

    /// Once a frame, `dt_s` after the last, with `plan` for the playing
    /// track when there is one: the level, 0..1, the depth applied.
    pub fn step(&mut self, dt_s: f32, input: Input, plan: Option<&SongPlan>) -> f32 {
        let dt = if dt_s.is_finite() { dt_s.max(0.0) } else { 0.0 };
        self.quiet_s = if !input.playing {
            QUIET_HOLD_S
        } else if unit(input.loudness_m) < QUIET_LOUDNESS {
            self.quiet_s + dt
        } else {
            0.0
        };
        let quiet = self.quiet_s >= QUIET_HOLD_S;
        if quiet {
            self.heard_s = 0.0;
            self.back_s = 0.0;
        } else {
            self.heard_s += dt;
            self.back_s += dt;
            if self.heard_s >= LOUDNESS_WARM_S {
                self.history.feed(dt, input.loudness_s);
            }
        }
        let (range, fill) = match plan {
            Some(p) => (Some(p.range), 1.0),
            None => (self.history.range(), self.history.fill()),
        };
        self.range = range;
        self.position = range.map_or(0.0, |r| position(input.loudness_s, r));
        let followed = range.map_or(OPEN_LEVEL, |_| {
            follow(self.position, input.buildup, self.low)
        });
        let target = OPEN_LEVEL + (followed - OPEN_LEVEL) * fill;
        self.music = match self.opening {
            Opening::Waiting => {
                if !quiet && !input.label_busy {
                    if self.depth > 0.0 {
                        self.opening = Opening::Message { shown_s: 0.0 };
                        self.events.push(Event::Opening);
                    } else {
                        self.opening = Opening::Done;
                        self.events.push(Event::OpeningSkipped);
                    }
                }
                0.0
            }
            Opening::Message { shown_s } => {
                let shown_s = if input.label_busy {
                    shown_s
                } else {
                    shown_s + dt
                };
                self.opening = if !input.playing {
                    Opening::Waiting
                } else if shown_s >= OPEN_HOLD_S {
                    Opening::Glide { s: 0.0 }
                } else {
                    Opening::Message { shown_s }
                };
                0.0
            }
            Opening::Glide { s } => {
                let s = s + dt;
                let x = (s / OPEN_GLIDE_S).min(1.0);
                if x >= 1.0 || !input.playing {
                    self.opening = Opening::Done;
                    self.events.push(Event::Show);
                } else {
                    self.opening = Opening::Glide { s };
                }
                OPEN_LEVEL * x * x * (3.0 - 2.0 * x)
            }
            Opening::Done => self.follow_step(dt, &input, quiet, target),
        };
        self.shown_depth = approach(self.shown_depth, self.depth, dt, DEPTH_S);
        self.level = self.fixed.unwrap_or(self.music * self.shown_depth);
        self.level
    }

    /// The show's smoothing toward `target`: quiet, a drop's hold, the
    /// glide after the music came back, or the attack and the release.
    fn follow_step(&mut self, dt: f32, input: &Input, quiet: bool, target: f32) -> f32 {
        if quiet {
            self.drop_s = 0.0;
        } else if input.drop {
            self.drop_s = DROP_HOLD_S;
            self.events.push(Event::Drop);
        }
        let (goal, tau) = if quiet {
            (0.0, QUIET_RELEASE_S)
        } else if self.drop_s > 0.0 {
            self.drop_s = (self.drop_s - dt).max(0.0);
            (1.0, DROP_ATTACK_S)
        } else if self.back_s < RESUME_S {
            (target, RESUME_S / 3.0)
        } else if target > self.music {
            (target, ATTACK_S)
        } else {
            (target, RELEASE_S)
        };
        approach(self.music, goal, dt, tau)
    }

    /// The passthrough image's opacity at `level`: 1 at 0, the floor at 1.
    pub fn opacity(&self, level: f32) -> f32 {
        1.0 - unit(level) * (1.0 - self.floor)
    }

    /// The launch log line: "immersion: full · floor 0.00 · opening ... ·
    /// follows ... · attack 2.0 s · release 4.0 s", the depth named when it
    /// is one of [`Depth`]'s, marked when `debug.fosfora.immerse` set it
    /// (`from_knob`), with a skipped opening, the pinned level and a missing
    /// passthrough said.
    pub fn launch_line(&self, from_knob: bool, passthrough: bool) -> String {
        let depth = Depth::ALL
            .into_iter()
            .find(|d| (d.value() - self.depth).abs() < 1e-4)
            .map_or_else(|| format!("{:.2}", self.depth), |d| d.name().to_owned());
        let knob = if from_knob {
            " (debug.fosfora.immerse)"
        } else {
            ""
        };
        let opening = match self.open_skip {
            Some(why) => format!("no opening ({why})"),
            None => format!(
                "opening \"{OPEN_TEXT}\" {OPEN_HOLD_S:.1} s, then {OPEN_LEVEL:.2} over {OPEN_GLIDE_S:.1} s"
            ),
        };
        let pinned = self
            .fixed
            .map(|l| format!(" \u{b7} pinned at {l:.2} (debug.fosfora.immersefix)"))
            .unwrap_or_default();
        let low = if (self.low - LOW_LEVEL).abs() < 1e-6 {
            ""
        } else {
            " (debug.fosfora.immerselow)"
        };
        let room = if passthrough {
            ""
        } else {
            " \u{b7} no passthrough: the room and the surfaces stay as they are"
        };
        format!(
            "immersion: {depth}{knob} \u{b7} floor {:.2} \u{b7} {opening} \u{b7} follows loudness_s within its last {WINDOW_S:.0} s (p{:.0}..p{:.0}, span {MIN_SPAN:.2} at least) onto {:.2}..1{low} \u{b7} attack {ATTACK_S:.1} s \u{b7} release {RELEASE_S:.1} s{pinned}{room}",
            self.floor,
            RANGE_LOW_P * 100.0,
            RANGE_HIGH_P * 100.0,
            self.low,
        )
    }

    /// The once-a-second log line at `level` (the one used, after roam):
    /// the level, the passthrough, `loudness_s` within the range and its
    /// position there, `buildup`, the depth, and the opening while it runs.
    pub fn second_line(&self, level: f32, loudness_s: f32, buildup: f32) -> String {
        let range = self.range.map_or_else(
            || "no range yet".to_owned(),
            |(lo, hi)| format!("in {lo:.2}..{hi:.2} at {:.2}", self.position),
        );
        let opening = match self.opening {
            Opening::Waiting | Opening::Done => "",
            Opening::Message { .. } => " \u{b7} opening: the message",
            Opening::Glide { .. } => " \u{b7} opening: the glide",
        };
        format!(
            "immersion: level {level:.2} \u{b7} passthrough {:.2} \u{b7} loudness_s {loudness_s:.2} {range} \u{b7} buildup {buildup:.2} \u{b7} depth {:.2}{opening}",
            self.opacity(level),
            self.depth,
        )
    }

    /// The log line for `event`, with this frame's `loudness_s`.
    pub fn event_line(&self, event: Event, loudness_s: f32) -> String {
        match event {
            Event::Opening => format!(
                "immersion: opening, \"{OPEN_TEXT}\" for {OPEN_HOLD_S:.1} s, then {:.2} over {OPEN_GLIDE_S:.1} s",
                OPEN_LEVEL * self.depth
            ),
            Event::OpeningSkipped => "immersion: no opening, the depth is off".to_owned(),
            Event::Show => format!(
                "immersion: the show starts at {:.2} \u{b7} history {:.1} s",
                self.level,
                self.history.seconds()
            ),
            Event::Drop => format!(
                "immersion: drop, to 1 for {DROP_HOLD_S:.1} s \u{b7} loudness_s {loudness_s:.2}{}",
                self.range
                    .map(|(lo, hi)| format!(" in {lo:.2}..{hi:.2}"))
                    .unwrap_or_default()
            ),
        }
    }
}

/// The step of `opacity` on the style call's grid of [`OPACITY_STEPS`].
pub fn opacity_step(opacity: f32) -> u32 {
    (unit(opacity) * OPACITY_STEPS as f32).round() as u32
}

/// What the runtime was last told: the passthrough opacity's step, sent
/// only when it changes. It starts at the runtime's own default, opacity
/// 1, so a run the level never leaves 0 makes no call.
#[derive(Debug, Clone, Copy)]
pub struct StyleGate {
    sent: u32,
}

impl Default for StyleGate {
    fn default() -> Self {
        Self {
            sent: OPACITY_STEPS,
        }
    }
}

impl StyleGate {
    /// The opacity to send this frame, on the grid, or `None` when the
    /// runtime already has it.
    pub fn update(&mut self, opacity: f32) -> Option<f32> {
        let step = opacity_step(opacity);
        (step != self.sent).then(|| {
            self.sent = step;
            step as f32 / OPACITY_STEPS as f32
        })
    }
}

/// A lit surface's peak alpha at `level`, from `base`, its level-0 value:
/// `base` times the ported effects' ratio (`ALPHA_FULL / 0.5`, 2 at level
/// 1), so a port's 0.5 becomes `0.5 + 0.5 * level` and the built-in
/// behaviors rise in the same proportion; capped at 1, and never under
/// `base`, so level 0 is `base` exactly (a rings or spectrum gain over 4
/// keeps its peak over 1 as before).
pub fn peak_alpha(base: f32, level: f32) -> f32 {
    let lift = ALPHA_FULL / crate::surface_port::PEAK_ALPHA - 1.0;
    (base * (1.0 + lift * unit(level))).min(1.0).max(base)
}

/// A face's uniform rows (`surface_fx::rows` or `surface_port::rows`)
/// with the peak alpha in [`PEAK_ROW`] lifted to `level` ([`peak_alpha`]).
pub fn lift_rows<const N: usize>(mut rows: [[f32; 4]; N], level: f32) -> [[f32; 4]; N] {
    const { assert!(N > PEAK_ROW) };
    rows[PEAK_ROW][3] = peak_alpha(rows[PEAK_ROW][3], level);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    const FPS: f32 = 72.0;

    /// A song as a function of the time since its section's start.
    type Section<'a> = &'a dyn Fn(f32) -> Input;

    /// Step `im` at 72 frames a second for `secs` with `input`, no plan;
    /// returns the level after each frame.
    fn run(im: &mut Immersion, secs: f32, input: Section<'_>) -> Vec<f32> {
        run_plan(im, secs, input, None)
    }

    fn run_plan(
        im: &mut Immersion,
        secs: f32,
        input: Section<'_>,
        plan: Option<&SongPlan>,
    ) -> Vec<f32> {
        let frames = (secs * FPS).round() as u32;
        (0..frames)
            .map(|i| im.step(1.0 / FPS, input(i as f32 / FPS), plan))
            .collect()
    }

    /// The music playing at `loudness_s` (the momentary loudness with it)
    /// and `buildup`.
    fn song(loudness_s: f32, buildup: f32) -> Input {
        Input {
            loudness_s,
            loudness_m: loudness_s,
            buildup,
            playing: true,
            ..Input::default()
        }
    }

    /// The synthetic groove: no real audio.
    fn synth(_: f32) -> Input {
        Input::default()
    }

    /// A loud flat master: `loudness_s` 0.8 with a slow wobble of 0.005,
    /// the tension low.
    fn flat(t: f32) -> Input {
        song(0.8 + 0.005 * (0.9 * t).sin(), 0.2)
    }

    /// From `a` to `b` over the first 3 s of a section (`loudness_s`'s
    /// window), at `t` into it.
    fn ramp(a: f32, b: f32, t: f32) -> f32 {
        a + (b - a) * (t / 3.0).min(1.0)
    }

    /// The level following the song from the start: the opening skipped.
    fn follower() -> Immersion {
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        im.skip_opening("test");
        im
    }

    /// The first time (s) the levels pass `under` going down (`Some`), or
    /// over it going up.
    fn first(levels: &[f32], pass: impl Fn(f32) -> bool) -> Option<f32> {
        levels
            .iter()
            .position(|&l| pass(l))
            .map(|i| (i + 1) as f32 / FPS)
    }

    fn spread(levels: &[f32]) -> (f32, f32) {
        levels
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &l| (lo.min(l), hi.max(l)))
    }

    #[test]
    fn the_opening_shows_the_message_then_glides_to_0_35_once() {
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        // The synthetic groove at launch: nothing shows, the room plain.
        let levels = run(&mut im, 5.0, &synth);
        assert!(levels.iter().all(|&l| l == 0.0));
        assert_eq!((im.message(), im.opening()), (None, Opening::Waiting));
        assert!(im.take_events().is_empty());
        // The music starts: the message for 4 s, the room still plain.
        let mut shown = Vec::new();
        for i in 0..(OPEN_HOLD_S * FPS) as u32 {
            let level = im.step(1.0 / FPS, flat(i as f32 / FPS), None);
            assert_close!(level, 0.0);
            shown.push(im.message());
        }
        assert!(shown.iter().all(|m| m.is_some_and(|(t, _)| t == OPEN_TEXT)));
        assert_close!(shown[0].unwrap().1, 1.0);
        assert!(shown[shown.len() - 1].unwrap().1 < 0.1);
        assert_eq!(im.take_events(), [Event::Opening]);
        // Then the glide to 0.35 over 3 s, half way at its middle.
        let levels = run(&mut im, OPEN_GLIDE_S + 0.1, &flat);
        assert_eq!(im.message(), None);
        let mid = levels[(OPEN_GLIDE_S * 0.5 * FPS) as usize];
        assert!((mid - OPEN_LEVEL * 0.5).abs() < 0.02, "{mid}");
        assert!(
            levels.windows(2).all(|w| w[1] >= w[0] - 1e-6),
            "the glide only rises"
        );
        assert!((levels[(OPEN_GLIDE_S * FPS) as usize - 1] - OPEN_LEVEL).abs() < 0.01);
        assert_eq!(im.take_events(), [Event::Show]);
        assert_eq!(im.opening(), Opening::Done);
        // The show: the flat master closes the room as the history fills.
        run(&mut im, 30.0, &flat);
        assert!(im.level() > 0.95, "{}", im.level());
        // The music stops and starts again: no message, the room back,
        // then the song's level again within about 2 s.
        let levels = run(&mut im, 5.0, &synth);
        assert!(levels[levels.len() - 1] < 0.01);
        let levels = run(&mut im, 3.0, &flat);
        assert_eq!(im.message(), None);
        assert!(im.take_events().is_empty());
        let back = first(&levels, |l| l > 0.9).expect("back");
        assert!(back <= RESUME_S + 0.4, "{back}");
    }

    #[test]
    fn the_opening_waits_for_the_other_labels_and_counts_only_while_up() {
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        let busy = |t: f32| Input {
            label_busy: true,
            ..flat(t)
        };
        // The scan's label is up as the music starts: the opening waits.
        run(&mut im, 3.0, &busy);
        assert_eq!((im.message(), im.opening()), (None, Opening::Waiting));
        // Free: the message, 2 s of it, then covered for 2 s (the voice's
        // label): the hold stops, then runs out its last 2 s.
        run(&mut im, 2.0, &flat);
        assert!(im.message().is_some());
        run(&mut im, 2.0, &busy);
        assert!(matches!(im.opening(), Opening::Message { .. }));
        let levels = run(&mut im, 2.0 + 0.05, &flat);
        assert!(matches!(im.opening(), Opening::Glide { .. }));
        assert!(levels[levels.len() - 1] > 0.0);
        // The music stopped during the message: it starts over next time.
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        run(&mut im, 1.0, &flat);
        run(&mut im, 1.0, &synth);
        assert_eq!(im.opening(), Opening::Waiting);
        run(&mut im, 0.1, &flat);
        assert!(im.message().is_some());
    }

    #[test]
    fn a_pin_or_the_knob_skips_the_opening() {
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        im.fix(Some(0.6));
        let levels = run(&mut im, 10.0, &flat);
        assert!(levels.iter().all(|&l| (l - 0.6).abs() < 1e-6));
        assert_eq!((im.message(), im.opening()), (None, Opening::Done));
        // `immerseopen 0`: no message; the music's start glides the room
        // to where the show starts within about 2 s.
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        im.skip_opening("debug.fosfora.immerseopen 0");
        run(&mut im, 2.0, &synth);
        let levels = run(&mut im, RESUME_S, &flat);
        assert_eq!(im.message(), None);
        assert!(im.take_events().is_empty());
        let end = levels[levels.len() - 1];
        assert!((end - OPEN_LEVEL).abs() < 0.03, "{end}");
    }

    #[test]
    fn the_depth_scales_the_opening_and_off_shows_nothing() {
        let mut im = Immersion::new(Depth::Low.value(), IMMERSE_FLOOR);
        run(&mut im, OPEN_HOLD_S + OPEN_GLIDE_S + 0.05, &flat);
        assert_eq!(im.opening(), Opening::Done);
        assert!(
            (im.level() - OPEN_LEVEL * 0.5).abs() < 0.01,
            "{}",
            im.level()
        );
        let mut im = Immersion::new(Depth::Off.value(), IMMERSE_FLOOR);
        let levels = run(&mut im, 20.0, &flat);
        assert!(levels.iter().all(|&l| l == 0.0));
        assert_eq!(im.message(), None);
        assert_eq!(im.take_events(), [Event::OpeningSkipped]);
    }

    #[test]
    fn a_resume_glides_to_the_song_in_about_2_s() {
        // Full, then a breakdown right as the session pauses: on resume
        // the level falls over the glide, not the 4 s release.
        let mut im = follower();
        run(&mut im, 40.0, &flat);
        assert!(im.level() > 0.95);
        im.resume();
        let levels = run(&mut im, 3.0, &|t| song(ramp(0.8, 0.6, t * 10.0), 0.2));
        let open = first(&levels, |l| l < LOW_LEVEL + 0.05).expect("open");
        assert!(open <= RESUME_S + 0.3, "{open}");
        // Without the resume the release takes its time.
        let mut im = follower();
        run(&mut im, 40.0, &flat);
        let levels = run(&mut im, 3.0, &|t| song(ramp(0.8, 0.6, t * 10.0), 0.2));
        assert!(levels[levels.len() - 1] > 0.5);
    }

    #[test]
    fn the_range_is_robust_and_at_least_the_minimum_span() {
        let mut h = History::default();
        assert_eq!(h.range(), None);
        // A few spikes (5 %) do not stretch it.
        for i in 0..HISTORY_LEN {
            h.push(if i.is_multiple_of(20) { 0.95 } else { 0.5 });
        }
        assert_close!(<[f32; 2]>::from(h.range().unwrap()), [0.5, 0.5]);
        assert_close!(h.seconds(), WINDOW_S);
        // A flat song sits at the top of a range widened down to the
        // minimum span; its top quarter is full.
        let flat = (0.5, 0.5);
        assert_close!(
            [0.5, 0.4875, 0.475, 0.45, 0.3].map(|x| position(x, flat)),
            [1.0, 1.0, 2.0 / 3.0, 0.0, 0.0]
        );
        assert_close!(position(0.5, (0.4, 0.6)), 2.0 / 3.0);
        assert_close!(position(f32::NAN, (0.4, 0.6)), 0.0);
        // The follow: low..1, the tension lifting it in proportion to the
        // position, none at the bottom.
        assert_close!(follow(0.0, 0.9, LOW_LEVEL), LOW_LEVEL);
        assert_close!(follow(1.0, 0.0, LOW_LEVEL), 1.0);
        assert_close!(follow(0.5, BUILD_FROM, 0.2), 0.6);
        assert_close!(follow(0.5, 0.7, 0.2), 0.85);
        assert_close!(follow(0.5, BUILD_FROM, LOW_LEVEL), 0.71);
        assert_close!(follow(0.9, 1.0, LOW_LEVEL), 1.0);
    }

    #[test]
    fn a_flat_loud_master_parks_high() {
        let mut im = follower();
        let levels = run(&mut im, 120.0, &flat);
        // Closed from about the time the history fills, and nothing moves
        // it: parking is fine.
        let tail = &levels[(25.0 * FPS) as usize..];
        let (lo, hi) = spread(tail);
        assert!(lo > 0.95 && hi - lo < 0.05, "{lo}..{hi}");
    }

    /// A song with sections: full at 0.8 for `full_s`, a breakdown at 0.62
    /// for `bd_s` (no bass: `buildup` 0.75), then full again; `t` into it.
    fn with_breakdown(full_s: f32, bd_s: f32, t: f32) -> Input {
        if t < full_s {
            song(0.8, 0.2)
        } else if t < full_s + bd_s {
            song(ramp(0.8, 0.62, t - full_s), 0.75)
        } else {
            song(ramp(0.62, 0.8, t - full_s - bd_s), 0.2)
        }
    }

    #[test]
    fn a_breakdown_opens_the_room_and_the_return_closes_it() {
        let mut im = follower();
        let levels = run(&mut im, 110.0, &|t| with_breakdown(60.0, 20.0, t));
        let at = |s: f32| (s * FPS) as usize;
        assert!(levels[at(59.0)] > 0.95, "{}", levels[at(59.0)]);
        // Open (within 0.15 of the low level) within 8 s of its start,
        // held open to its end, its high tension notwithstanding.
        let bd = &levels[at(60.0)..at(80.0)];
        let open = first(bd, |l| l < LOW_LEVEL + 0.15).expect("open");
        assert!(open < 8.0, "{open}");
        assert!(bd[at(open)..].iter().all(|&l| l < LOW_LEVEL + 0.15));
        assert!(spread(bd).0 < LOW_LEVEL + 0.03, "{:?}", spread(bd));
        // Closed within 7 s of its end.
        let closed = first(&levels[at(80.0)..], |l| l > 0.9).expect("closed");
        assert!(closed < 7.0, "{closed}");
    }

    #[test]
    fn the_low_level_is_a_live_knob() {
        for unset in [None, Some(""), Some("dark"), Some("nan")] {
            assert_eq!(low_knob(unset), None, "{unset:?}");
        }
        assert_eq!(
            [" 0.5 ", "1.7", "-1"].map(|v| low_knob(Some(v))),
            [Some(0.5), Some(1.0), Some(0.0)]
        );
        // Changed 18 s into a breakdown: the room goes to the new low
        // level over the attack.
        let mut im = follower();
        run(&mut im, 58.0, &|t| with_breakdown(40.0, 30.0, t));
        assert!((im.level() - LOW_LEVEL).abs() < 0.05, "{}", im.level());
        im.set_low(0.6);
        run(&mut im, 8.0, &|t| with_breakdown(40.0, 30.0, 58.0 + t));
        assert!((im.level() - 0.6).abs() < 0.05, "{}", im.level());
        assert_eq!(
            im.low_line(true),
            "immersion: low level 0.60 (debug.fosfora.immerselow)"
        );
        assert!(
            im.launch_line(false, true)
                .contains(" onto 0.60..1 (debug.fosfora.immerselow) \u{b7} ")
        );
        im.set_low(LOW_LEVEL);
        assert_eq!(
            im.low_line(false),
            "immersion: low level 0.42 (the default)"
        );
    }

    /// After 40 s full and 8 s of breakdown, a 12 s build from the
    /// breakdown's level to 0.72, its tension from 0.3 to 0.9 (`tense`) or
    /// flat at 0.3, then a drop into the full section.
    fn build(tense: bool, t: f32) -> Input {
        let rise = |t: f32| (t / 12.0).clamp(0.0, 1.0);
        if t < 48.0 {
            with_breakdown(40.0, 8.0, t)
        } else if t < 60.0 {
            let k = rise(t - 48.0);
            song(0.62 + 0.10 * k, if tense { 0.3 + 0.6 * k } else { 0.3 })
        } else {
            Input {
                drop: t < 60.0 + 0.5 / FPS,
                ..song(0.8, 0.2)
            }
        }
    }

    #[test]
    fn a_build_closes_in_and_the_drop_hits_and_holds() {
        let at = |s: f32| (s * FPS) as usize;
        let mut tense = follower();
        let levels = run(&mut tense, 60.0 + DROP_HOLD_S, &|t| build(true, t));
        let mut flat_tension = follower();
        let without = run(&mut flat_tension, 60.0, &|t| build(false, t));
        // The rising loudness alone closes it part way; the tension
        // closes it in.
        let (end, end_without) = (levels[at(60.0) - 1], without[at(60.0) - 1]);
        assert!(end > 0.9 && end_without < 0.85, "{end} vs {end_without}");
        assert!(levels[at(56.0)] > without[at(56.0)] + 0.1);
        // The drop: full within a second and held through its hold.
        assert_eq!(tense.take_events(), [Event::Drop]);
        assert!(levels[at(60.75)] > 0.95, "{}", levels[at(60.75)]);
        assert!(levels[at(61.0)..].iter().all(|&l| l > 0.95));
    }

    #[test]
    fn a_drop_holds_the_room_closed_through_a_breakdown() {
        // A drop fired 8 s into a breakdown: held at 1 for the hold, then
        // back to the follow over the release.
        let mut im = follower();
        run(&mut im, 48.0, &|t| with_breakdown(40.0, 30.0, t));
        assert!(im.level() < LOW_LEVEL + 0.15, "{}", im.level());
        let levels = run(&mut im, DROP_HOLD_S, &|t| Input {
            drop: t < 0.5 / FPS,
            ..song(0.62, 0.75)
        });
        let at = |s: f32| levels[(s * FPS) as usize];
        assert!(at(0.75) > 0.9 && at(1.0) > 0.95, "{} {}", at(0.75), at(1.0));
        assert!(levels[FPS as usize..].iter().all(|&l| l > 0.95));
        let levels = run(&mut im, 2.0 * RELEASE_S, &|_| song(0.62, 0.75));
        assert!(
            levels[levels.len() - 1] < LOW_LEVEL + 0.1,
            "{:?}",
            levels.last()
        );
    }

    #[test]
    fn a_quiet_piece_moves_within_its_own_range() {
        // Sections of 20 s at -36 and -30 LUFS, one after the other: far
        // under where the absolute map started to dim.
        let piece = |t: f32| {
            let k = (t / 20.0) as u32;
            let (from, to) = if k.is_multiple_of(2) {
                (0.5, 0.4)
            } else {
                (0.4, 0.5)
            };
            song(ramp(from, to, t - k as f32 * 20.0), 0.2)
        };
        let mut im = follower();
        let levels = run(&mut im, 160.0, &piece);
        let at = |s: f32| (s * FPS) as usize;
        for k in 2..8u32 {
            let section = &levels[at(k as f32 * 20.0)..at((k + 1) as f32 * 20.0)];
            let (lo, hi) = spread(section);
            if k.is_multiple_of(2) {
                assert!(lo < LOW_LEVEL + 0.1, "section {k}: {lo}");
            } else {
                assert!(hi > 0.9, "section {k}: {hi}");
            }
        }
    }

    /// 60 s of a loud song, 1 s of silence, then a quieter one, flat; the
    /// player's new track at the second's start when `told`. Returns the
    /// levels of the second.
    fn quieter_after_louder(told: bool) -> Vec<f32> {
        let mut im = follower();
        run(&mut im, 60.0, &|t| {
            song(0.85 + 0.005 * (0.9 * t).sin(), 0.2)
        });
        run(&mut im, 1.0, &|_| Input {
            loudness_m: 0.0,
            ..song(0.5, 0.0)
        });
        if told {
            im.new_track();
        }
        run(&mut im, 40.0, &|t| song(0.6 + 0.005 * (0.9 * t).sin(), 0.2))
    }

    #[test]
    fn a_track_change_adapts_within_a_few_seconds() {
        let levels = quieter_after_louder(true);
        let closed = first(&levels, |l| l > 0.8).expect("closed");
        assert!(closed < 12.0, "{closed}");
        // Without the track change the louder song's range holds it open.
        let levels = quieter_after_louder(false);
        assert!(levels[(20.0 * FPS) as usize] < LOW_LEVEL + 0.1);
    }

    #[test]
    fn a_beat_rate_wobble_hardly_moves_the_level() {
        // 140 BPM: `loudness_s` swings by 0.02, `loudness_m` by 0.2 and
        // `buildup` by 0.2 on every beat.
        let beat_hz = 140.0 / 60.0;
        let wobble = |t: f32| {
            let s = (std::f32::consts::TAU * beat_hz * t).sin();
            Input {
                loudness_s: 0.7 + 0.01 * s,
                loudness_m: 0.7 + 0.1 * s,
                buildup: 0.25 + 0.1 * s,
                playing: true,
                ..Input::default()
            }
        };
        let mut im = follower();
        let levels = run(&mut im, 60.0, &wobble);
        let (lo, hi) = spread(&levels[levels.len() - 10 * FPS as usize..]);
        assert!(hi - lo < 0.02, "{lo}..{hi}");
        // Parked high, as the steady song is.
        assert!(lo > 0.9, "{lo}");
        assert!(opacity_step(im.opacity(hi)).abs_diff(opacity_step(im.opacity(lo))) <= 1);
    }

    #[test]
    fn silence_or_stopped_music_brings_the_room_back_within_1_5_s() {
        let back = |input: Section<'_>| {
            let mut im = follower();
            run(&mut im, 30.0, &flat);
            assert!(im.level() > 0.95);
            first(&run(&mut im, 3.0, input), |l| l < 0.05)
        };
        // Silence: the momentary loudness at 0 while `loudness_s` is still
        // in its window.
        let silence = |_: f32| Input {
            loudness_m: 0.0,
            ..song(0.7, 0.3)
        };
        let t = back(&silence).expect("back");
        assert!(t <= 1.5, "{t}");
        // The music stopped: the synthetic groove's features, no hold.
        let t = back(&synth).expect("back");
        assert!(t <= 1.5 - QUIET_HOLD_S, "{t}");
        // A short gap between hits is not silence.
        let mut im = follower();
        run(&mut im, 30.0, &flat);
        run(&mut im, 0.25, &silence);
        let levels = run(&mut im, 1.0, &flat);
        assert!(levels.iter().all(|&l| l > 0.95), "{:?}", levels.first());
    }

    #[test]
    fn a_song_plan_range_replaces_the_history() {
        let mut im = follower();
        run(&mut im, 30.0, &flat);
        assert!(im.level() > 0.95);
        // The whole track's range puts 0.8 a fifth of the way up it.
        let plan = SongPlan {
            range: (0.75, 1.0),
            cues: vec![Cue {
                at_s: 90.0,
                kind: CueKind::Drop,
            }],
        };
        run_plan(&mut im, 20.0, &|_| song(0.8, 0.2), Some(&plan));
        assert_eq!(im.range(), Some(plan.range));
        let want = follow(position(0.8, plan.range), 0.2, LOW_LEVEL);
        assert!((im.level() - want).abs() < 0.01, "{} vs {want}", im.level());
        assert!(want < 0.65);
        // Without it, the history's range again: the top.
        run(&mut im, 20.0, &|_| song(0.8, 0.2));
        assert!(im.level() > 0.95);
    }

    #[test]
    fn the_depth_scales_the_ceiling() {
        for (depth, ceiling) in [(Depth::Full, 1.0), (Depth::Low, 0.5), (Depth::Off, 0.0)] {
            let mut im = Immersion::new(depth.value(), IMMERSE_FLOOR);
            let levels = run(&mut im, 40.0, &|_| song(0.72, 0.3));
            assert!(
                levels.iter().all(|&l| l <= ceiling + 1e-4),
                "{depth:?}: {:?}",
                levels.last()
            );
            assert!(
                (im.level() - ceiling).abs() < 0.01,
                "{depth:?}: {}",
                im.level()
            );
        }
        // A change glides: seen within a frame, settled within 1.5 s.
        let mut im = follower();
        run(&mut im, 40.0, &|_| song(0.72, 0.3));
        im.set_depth(Depth::Low.value());
        let levels = run(&mut im, 1.5, &|_| song(0.72, 0.3));
        assert!(levels[0] < 0.99 && levels[0] > 0.9, "{}", levels[0]);
        assert!((im.level() - 0.5).abs() < 0.03, "{}", im.level());
    }

    #[test]
    fn the_floor_is_the_darkest_and_a_parameter() {
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        assert_close!(
            [im.opacity(0.0), im.opacity(0.5), im.opacity(1.0)],
            [1.0, 0.5, 0.0]
        );
        im.set_floor(0.6);
        assert_close!([im.opacity(1.0), im.floor()], [0.6, 0.6]);
        assert_close!(im.opacity(2.0), 0.6);
    }

    #[test]
    fn a_pinned_level_ignores_the_audio_and_the_depth() {
        let mut im = Immersion::new(0.0, IMMERSE_FLOOR);
        im.fix(Some(0.8));
        for input in [song(0.4, 0.05), flat(0.0), synth(0.0)] {
            assert_close!(im.step(1.0 / FPS, input, None), 0.8);
        }
        im.fix(Some(3.0));
        assert_close!(im.step(1.0 / FPS, flat(0.0), None), 1.0);
        im.fix(None);
        assert!(im.step(1.0 / FPS, flat(0.0), None) < 0.01);
    }

    #[test]
    fn the_style_goes_out_only_when_its_step_changes() {
        let mut gate = StyleGate::default();
        // The runtime's default is opacity 1: nothing to send at level 0.
        assert_eq!(gate.update(1.0), None);
        assert_eq!(gate.update(0.996), None);
        let sent = gate.update(0.9).expect("a new step");
        assert_close!(sent, 58.0 / 64.0);
        assert_eq!(gate.update(0.9), None);
        assert_eq!(gate.update(0.903), None);
        assert!(gate.update(0.88).is_some());
        // A full ramp sends at most one call per step.
        let mut im = follower();
        let mut gate = StyleGate::default();
        let calls = run(&mut im, 40.0, &flat)
            .into_iter()
            .filter_map(|l| gate.update(im.opacity(l)))
            .count();
        let steps = opacity_step(1.0) - opacity_step(IMMERSE_FLOOR);
        assert!(calls as u32 <= steps, "{calls} calls for {steps} steps");
        assert!(calls > 10);
    }

    #[test]
    fn the_peak_alpha_rises_with_the_level_and_stays_at_level_zero() {
        use crate::surface_fx::{CURL_PEAK_ALPHA, PULSE_PEAK_ALPHA, STREAM_PEAK_ALPHA};
        use crate::surface_port::PEAK_ALPHA;
        // Level 0: every base exactly as it was, a gain over 4 included.
        for base in [
            PEAK_ALPHA,
            STREAM_PEAK_ALPHA,
            PULSE_PEAK_ALPHA,
            0.0,
            0.125,
            1.3,
        ] {
            assert_eq!(peak_alpha(base, 0.0).to_bits(), base.to_bits(), "{base}");
        }
        // A port: from 0.5 to ALPHA_FULL along the level.
        assert_close!(
            [peak_alpha(PEAK_ALPHA, 0.5), peak_alpha(PEAK_ALPHA, 1.0)],
            [0.75, ALPHA_FULL]
        );
        // The built-in behaviors by the same ratio, capped at 1.
        assert_close!(peak_alpha(PULSE_PEAK_ALPHA, 1.0), 0.9);
        assert_close!(peak_alpha(STREAM_PEAK_ALPHA, 1.0), 1.0);
        assert_close!(peak_alpha(CURL_PEAK_ALPHA, 0.5), 0.825);
        assert_close!(peak_alpha(crate::ripple::PEAK_ALPHA, 1.0), 0.5);
        assert_close!(peak_alpha(1.3, 1.0), 1.3);
    }

    #[test]
    fn the_rows_carry_the_lifted_peak_alpha_in_the_color_row() {
        use crate::surface_fx::{Audio, PULSE_PEAK_ALPHA, Slot};
        use crate::surfaces::{Face, KIND_FRAME, SurfaceBehavior};
        use glam::Vec3;
        let face = Face {
            normal: Vec3::Z,
            center: Vec3::new(0.0, 1.5, -2.0),
            axes: [Vec3::X, Vec3::Y],
            half: [0.4, 0.3],
        };
        // The surfaces pass's pulse: the color row's w.
        let slot = Slot {
            face,
            kind: KIND_FRAME,
            behavior: SurfaceBehavior::Pulse,
            strength: 0.7,
            color: 0,
            band: 0,
        };
        let rows = crate::surface_fx::rows(&slot, Audio::default(), None, None, 0.0);
        assert_close!(rows[PEAK_ROW][3], PULSE_PEAK_ALPHA);
        assert_eq!(lift_rows(rows, 0.0), rows);
        let lifted = lift_rows(rows, 1.0);
        assert_close!(lifted[PEAK_ROW][3], 0.9);
        // Only the peak alpha moves: the strength, the color and the rest
        // are the pass's.
        let mut back = lifted;
        back[PEAK_ROW][3] = rows[PEAK_ROW][3];
        assert_eq!(back, rows);
        // A port's.
        let rows = crate::surface_port::rows(&face, 1.0, true, 9, [1.0; 3]);
        assert_close!(rows[PEAK_ROW][3], crate::surface_port::PEAK_ALPHA);
        assert_close!(lift_rows(rows, 1.0)[PEAK_ROW][3], ALPHA_FULL);
        assert_eq!(lift_rows(rows, 0.0), rows);
    }

    #[test]
    fn the_depth_steps_names_and_nearest() {
        use Depth::{Full, Low, Off};
        assert_eq!(Depth::default(), Full);
        assert_eq!([Off.next(), Low.next(), Full.next()], [Low, Full, Off]);
        assert_eq!(
            [Off.deeper(), Low.deeper(), Full.deeper()],
            [Low, Full, Full]
        );
        assert_eq!(
            [Off.lighter(), Low.lighter(), Full.lighter()],
            [Off, Off, Low]
        );
        for d in Depth::ALL {
            assert_eq!(Depth::from_name(d.name()), Some(d));
            assert!(d.label().starts_with("Immersion: "), "{}", d.label());
            assert_eq!(Depth::nearest(d.value()), d);
        }
        assert_eq!(Depth::from_name("dark"), None);
        assert_eq!(
            [0.2, 0.3, 0.74, 0.76].map(Depth::nearest),
            [Off, Low, Low, Full]
        );
    }

    #[test]
    fn the_launch_line_says_the_opening_the_map_and_the_time_constants() {
        let im = Immersion::new(1.0, IMMERSE_FLOOR);
        assert_eq!(
            im.launch_line(false, true),
            "immersion: full \u{b7} floor 0.00 \u{b7} opening \"Sit back and relax\" 4.0 s, then 0.35 over 3.0 s \u{b7} follows loudness_s within its last 30 s (p10..p90, span 0.05 at least) onto 0.42..1 \u{b7} attack 2.0 s \u{b7} release 4.0 s"
        );
        let mut im = Immersion::new(0.75, 0.3);
        im.fix(Some(0.8));
        assert_eq!(
            im.launch_line(true, false),
            "immersion: 0.75 (debug.fosfora.immerse) \u{b7} floor 0.30 \u{b7} no opening (pinned) \u{b7} follows loudness_s within its last 30 s (p10..p90, span 0.05 at least) onto 0.42..1 \u{b7} attack 2.0 s \u{b7} release 4.0 s \u{b7} pinned at 0.80 (debug.fosfora.immersefix) \u{b7} no passthrough: the room and the surfaces stay as they are"
        );
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        im.skip_opening("debug.fosfora.immerseopen 0");
        assert!(
            im.launch_line(false, true)
                .contains(" \u{b7} no opening (debug.fosfora.immerseopen 0) \u{b7} ")
        );
    }

    #[test]
    fn the_log_lines_say_the_range_the_opening_the_show_and_the_drop() {
        let mut im = Immersion::new(1.0, IMMERSE_FLOOR);
        assert_eq!(
            im.second_line(0.0, 0.0, 0.0),
            "immersion: level 0.00 \u{b7} passthrough 1.00 \u{b7} loudness_s 0.00 no range yet \u{b7} buildup 0.00 \u{b7} depth 1.00"
        );
        run(&mut im, 1.0, &flat);
        assert_eq!(
            im.event_line(Event::Opening, 0.8),
            "immersion: opening, \"Sit back and relax\" for 4.0 s, then 0.35 over 3.0 s"
        );
        assert!(
            im.second_line(0.0, 0.8, 0.2)
                .ends_with(" \u{b7} opening: the message")
        );
        run(&mut im, 6.1, &flat);
        assert_eq!(
            im.event_line(Event::Show, 0.8),
            "immersion: the show starts at 0.35 \u{b7} history 4.0 s"
        );
        run(&mut im, 20.0, &|_| song(0.8, 0.2));
        assert_eq!(
            im.second_line(0.62, 0.8, 0.2),
            "immersion: level 0.62 \u{b7} passthrough 0.38 \u{b7} loudness_s 0.80 in 0.80..0.80 at 1.00 \u{b7} buildup 0.20 \u{b7} depth 1.00"
        );
        assert_eq!(
            im.event_line(Event::Drop, 0.81),
            "immersion: drop, to 1 for 8.0 s \u{b7} loudness_s 0.81 in 0.80..0.80"
        );
        assert_eq!(
            im.event_line(Event::OpeningSkipped, 0.0),
            "immersion: no opening, the depth is off"
        );
    }
}
