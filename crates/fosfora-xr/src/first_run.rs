//! The first run (board #5452): the show waits for the wearer, and the
//! gestures are taught as the wearer makes them. Plain numbers and words,
//! so the rules build and test on the desktop. `app.rs` holds a [`Wait`]
//! and a [`Hints`], feeds them once a frame and does what they return.
//!
//! **The wait** ([`Wait`]). Every launch waits. Once the scan's and the
//! permission's labels have gone, a card at the center of view says
//! "Pinch to begin" ([`card_text`]), and with the tips on, under it, that
//! tips will follow ([`TIPS_FOLLOW`], board #5575). Until the show begins the music does
//! not play, the looks get silence in place of the audio, the room is
//! dimmed to `immerse::WAIT_LEVEL` and the hands do one thing, begin the
//! show ([`Live`], board #5531): a pinch, a drag and a thumb swipe do
//! nothing, nothing is thrown or lifted, and the hand menu does not come.
//! A pinch closed during the wait, the one that begins the show included,
//! is no gesture until it opens ([`HeldPinches`]). The show begins
//! ([`Begun`]) when the wearer makes the start
//! gesture with the card up, says "begin" (`intent::Intent::Begin`),
//! starts the music by voice, or when the card has
//! been up [`WAIT_MAX_S`] and nobody answered it, so nobody is stuck in
//! front of it. `debug.fosfora.begin 1` begins at once, for unworn runs.
//!
//! **Before the card, once** (board #5534, `flash_choice.rs`). On a launch
//! where the wearer has not chosen whether to limit flashing, the card is
//! first the warning and that question. While it is up the wait is handed
//! no card and no start gesture (`flash_choice::wait_input`), so its
//! [`WAIT_MAX_S`] start when the question is over, and the pinch that
//! answers does not begin the show.
//!
//! **The start gesture** ([`start_gesture`], [`START_WORD`]) is a pinch.
//! That function and that word are the only places that say so.
//!
//! **The hints** ([`Hints`], [`Hint`]). Three things are taught on every
//! launch (board #5575; once per install until then, which left a wearer
//! who missed one no way to see it again), in this order
//! ([`Hint::SHOW_ORDER`], board #5547): the left
//! palm that brings the menu, how to talk (hold a fist, or tap the thumb
//! on the index finger), and the thumb swipe. The two that change nothing
//! in the room come first, and the swipe, which steps the theme, last.
//! No hint shows until the show has been on [`FIRST_HINT_S`]: the opening
//! belongs to the song and the look made for it. After that a hint shows
//! when no other label has been up for [`HINT_GAP_S`], one at
//! a time, ahead of the view where the voice label floats. It waits for
//! the wearer to make its gesture. When they do, the label says
//! [`GOT_IT`] for [`GOT_IT_S`] and the app plays the understood cue with
//! it. The words wait up to [`GOT_IT_WAIT_S`] for a label that covers
//! them, the swipe's caption, and are dropped behind one that stays
//! longer: after a spoken command its own answer is the answer. A hint
//! that has been in view [`HINT_GIVE_UP_S`] gives up.
//! Either way it is taught for this run ([`Taught`]) and shows again on
//! the next launch.
//! A wearer who makes a gesture before its hint has shown has taught
//! themselves, and that hint does not show in this run. A hint
//! never holds the show and never covers another label: its time counts
//! only while it is the label showing.
//!
//! **The Tips control** (board #5575, `hud.rs`). The hand menu's "Tips:
//! on" / "Tips: off" ([`tips_label`]), saved in `hand_menu.json` as
//! `"tips":false` once off. Off, no hint shows, in this run or at a
//! launch ([`Hints::stop`], [`Hints::at_launch`]). Turned on, the hints
//! start over at once, whatever this run taught ([`Hints::replay`]). When
//! the last hint of a run has gone, one more label says where that
//! control is ([`TIPS_END`]), once: the hints come back on every launch,
//! and the wearer has to be told how to stop them.
//!
//! **The caption** ([`swipe_caption`]). A thumb swipe that changes the
//! room's theme names the new one on the label, every time.

use crate::label::{HANDS_LABEL, fade_for};

/// How long the card waits for the wearer before the show begins by
/// itself (s). It counts only while the card may show.
pub const WAIT_MAX_S: f32 = 60.0;
/// The start gesture as the card names it.
pub const START_WORD: &str = "Pinch";

/// What the hands did this frame that could start the show, per hand
/// (left, right): rising edges only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HandEdges {
    /// A pinch closed.
    pub pinch_began: [bool; 2],
    /// The thumb tapped the index.
    pub thumb_tap: [bool; 2],
}

/// The hand that made the start gesture this frame, if one did. The start
/// gesture is a pinch closing on either hand, and the right hand wins a
/// tie, as it does for the other gestures. To start the show with another
/// gesture, change this function and [`START_WORD`].
pub fn start_gesture(hands: &HandEdges) -> Option<usize> {
    [1, 0].into_iter().find(|&h| hands.pinch_began[h])
}

/// What the begin card says under "Pinch to begin" with the tips on
/// (board #5575): the hints come later, so the wearer is told to expect
/// them and that nothing is asked of them until then.
pub const TIPS_FOLLOW: &str = "Then listen and watch.\nTips will follow.";

/// The card's first line: the start gesture and what it does.
fn begin_line() -> String {
    format!("{START_WORD} to begin")
}

/// The card's words while the show waits. With `no_hands` (no hand has
/// been tracked for `label::HANDS_WAIT_S`) the controllers line goes under
/// it. Else, with `tips` (the hints will show in this run), [`TIPS_FOLLOW`]
/// does: the controllers line comes first, since without hands nothing
/// begins. The card does not name the spoken "begin": the app does not
/// listen by itself, so a wearer who read "say begin" and said it would
/// get no answer.
pub fn card_text(no_hands: bool, tips: bool) -> String {
    let mut text = begin_line();
    if no_hands {
        text.push('\n');
        text.push_str(HANDS_LABEL);
    } else if tips {
        text.push('\n');
        text.push_str(TIPS_FOLLOW);
    }
    text
}

/// Why the show began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Begun {
    /// `debug.fosfora.begin 1`.
    Knob,
    /// A spoken "begin".
    Spoken,
    /// The wearer started the music, by voice: the hand menu and its
    /// Music row do not come while the show waits (board #5531).
    Music,
    /// The start gesture, on this hand (0 left, 1 right).
    Gesture { hand: usize },
    /// Nobody answered the card in [`WAIT_MAX_S`].
    Timeout,
}

impl Begun {
    /// The log line.
    pub fn line(self) -> String {
        let why = match self {
            Self::Knob => "debug.fosfora.begin 1".to_owned(),
            Self::Spoken => "a spoken begin".to_owned(),
            Self::Music => "the wearer started the music".to_owned(),
            Self::Gesture { hand } => format!(
                "a {} on the {} hand",
                START_WORD.to_lowercase(),
                if hand == 0 { "left" } else { "right" }
            ),
            Self::Timeout => format!("nobody answered the card in {WAIT_MAX_S:.0} s"),
        };
        format!("first run: the show begins ({why})")
    }
}

/// One frame of the wait.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WaitInput {
    /// The card may show: the scan's and the permission's labels are gone.
    pub ready: bool,
    /// The hand that made the start gesture ([`start_gesture`]).
    pub gesture: Option<usize>,
    /// A sentence asked to begin.
    pub spoken: bool,
    /// The music is asked for (the Music row's play).
    pub music: bool,
    /// `debug.fosfora.begin 1`.
    pub knob: bool,
}

/// The show waiting for the wearer, across frames.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Wait {
    begun: Option<Begun>,
    /// How long the card has been free to show (s).
    ready_s: f32,
}

impl Wait {
    /// Whether the show still waits.
    pub fn waiting(&self) -> bool {
        self.begun.is_none()
    }

    /// Why the show began, once it has.
    pub fn begun(&self) -> Option<Begun> {
        self.begun
    }

    /// Whether the card shows this frame, given `ready` as in
    /// [`WaitInput`]. Another label may still cover it.
    pub fn card_up(&self, ready: bool) -> bool {
        self.waiting() && ready
    }

    /// This frame, `dt` seconds after the last. Returns why the show
    /// begins on the frame it does, and `None` on every other frame. The
    /// knob, a spoken begin and the music begin it whether the card is up
    /// or not. The start gesture and the timeout need the card.
    pub fn step(&mut self, dt: f32, input: WaitInput) -> Option<Begun> {
        if self.begun.is_some() {
            return None;
        }
        if input.ready {
            self.ready_s += dt.max(0.0);
        }
        let why = if input.knob {
            Some(Begun::Knob)
        } else if input.spoken {
            Some(Begun::Spoken)
        } else if input.music {
            Some(Begun::Music)
        } else if !input.ready {
            None
        } else if let Some(hand) = input.gesture {
            Some(Begun::Gesture { hand })
        } else if self.ready_s >= WAIT_MAX_S {
            Some(Begun::Timeout)
        } else {
            None
        };
        self.begun = why;
        why
    }

    /// What the hands may do now, apart from beginning the show.
    pub fn live(&self) -> Live {
        let on = !self.waiting();
        Live {
            menu: on,
            pinches: on,
            swipes: on,
            instruments: on,
        }
    }
}

/// What the hands may do, apart from beginning the show (board #5531).
/// While the show waits they do one thing, begin it, so every field is
/// off. Every way to begin ([`Begun`]) turns them all on together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Live {
    /// The hand menu: the left palm and the spoken "menu" bring it, and it
    /// shows.
    pub menu: bool,
    /// A pinch is a tap, a drag or a hold.
    pub pinches: bool,
    /// A thumb swipe steps the theme.
    pub swipes: bool,
    /// The hand instruments: a tap throws, an open palm lifts, the pitcher
    /// pours.
    pub instruments: bool,
}

/// The label for a spoken "menu" while the show waits (board #5531): the
/// card's words and "first".
pub fn menu_waits_reply() -> String {
    format!("{} first", begin_line())
}

/// The pinches the begin holds back, per hand (left, right; board #5531).
/// A pinch that was closed while the show waited is no gesture until it
/// opens: the pinch that begins the show, and one held through a begin
/// that came another way (the 60 s, the knob, a spoken begin). Its release
/// is no tap, so it throws nothing and presses nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldPinches {
    held: [bool; 2],
    was_waiting: bool,
}

impl Default for HeldPinches {
    fn default() -> Self {
        Self {
            held: [false; 2],
            was_waiting: true,
        }
    }
}

impl HeldPinches {
    /// This frame, after [`Wait::step`]: whether each hand is `pinching`,
    /// and whether the show still waits. Returns the hands whose pinch no
    /// gesture may read: both while the show waits, then each hand that
    /// has not opened since.
    pub fn step(&mut self, pinching: [bool; 2], waiting: bool) -> [bool; 2] {
        let from_the_wait = waiting || self.was_waiting;
        self.held = [0, 1].map(|h| pinching[h] && (from_the_wait || self.held[h]));
        self.was_waiting = waiting;
        self.held.map(|held| waiting || held)
    }
}

/// A gesture the first run teaches. The variants are in the order of
/// [`Hint::ALL`] and the arrays; the hints show in [`Hint::SHOW_ORDER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// The thumb swipe along the index finger, which steps the theme.
    Swipe,
    /// How to talk (board #5531): a fist held while speaking and opened
    /// to send, or a thumb tap on the index finger. It is taught by a
    /// voice window with something said in it, opened either way. A fist
    /// or a tap that opened a window nobody spoke in has taught nothing.
    Talk,
    /// The left palm turned to the face, which brings the hand menu.
    Palm,
}

/// The swipe hint's words (board #5547). The last line says what the
/// swipe does, so the hint reads as an offer: a wearer who likes the look
/// the track brought can leave it.
pub const SWIPE_HINT: &str = "Swipe your thumb\nalong your finger\nto change the look";
/// The talk hint's words (board #5531): the fist, step by step, on two
/// lines, the thumb tap as the other way on the third, and a sentence the
/// grammar has on the last. Four lines, where the other hints have two:
/// the fist's three steps do not fit one line at a size worth reading,
/// and the tap has to be told here, since a wearer who cannot hold a fist
/// has no other way to ask what they can say.
pub const TALK_HINT: &str = "Make a fist, say a command,\nthen open your hand.\nOr tap thumb on index finger.\nTry: what can I say";
/// The palm hint's words. Board #5572: the palm opens the menu and, held
/// to the face again, closes it (board #5539's read "Away hides it.").
pub const PALM_HINT: &str = "Left palm to your face\nopens the menu, and closes it.";
/// What the label says when the wearer makes the gesture a hint asks for,
/// and for how long (s).
pub const GOT_IT: &str = "Got it";
pub const GOT_IT_S: f32 = 2.5;
/// How long [`GOT_IT`] waits for another label to go before it is dropped
/// (s): longer than a swipe's caption, shorter than a voice window.
pub const GOT_IT_WAIT_S: f32 = 3.0;
/// How long the label is free, with the show on, before a hint shows (s).
pub const HINT_GAP_S: f32 = 5.0;
/// How long the show has been on, in a run, before its first hint (s,
/// board #5547). The first hint used to show 10 s after the begin and ask
/// for a swipe, which stepped the theme away from the one the opening
/// track brings. It was 45 s until board #5575: people trying the app
/// had stopped expecting any help by then. The swipe is the last of the
/// three now, so at 20 s the two hints that change nothing come first and
/// the swipe's falls about where it did. `debug.fosfora.hintwait <s>`
/// sets another for a launch ([`Hints::at_launch`]).
pub const FIRST_HINT_S: f32 = 20.0;
/// How long after the Tips control is turned on its first hint shows (s,
/// board #5575): the press is the wearer asking, so nothing waits for the
/// opening or the gap.
pub const REPLAY_GAP_S: f32 = 2.0;
/// The label after the last hint of a run (board #5575): the hints show
/// on every launch, and this is where they are turned off.
pub const TIPS_END: &str = "Tips show at every start.\nMenu: Tips turns them off.";
/// How long [`TIPS_END`] stays (s).
pub const TIPS_END_S: f32 = 8.0;

/// The hand menu's Tips control (board #5575): what the setting is now.
pub fn tips_label(on: bool) -> &'static str {
    if on { "Tips: on" } else { "Tips: off" }
}
/// How long a hint stays in view before it gives up (s).
pub const HINT_GIVE_UP_S: f32 = 20.0;

impl Hint {
    /// Every hint, in the order of the arrays of [`HintInput`] and of the
    /// saved names. Not the order they show in ([`Self::SHOW_ORDER`]).
    pub const ALL: [Self; 3] = [Self::Swipe, Self::Talk, Self::Palm];

    /// The order the hints show in (board #5547): the palm and the talk
    /// first, which change nothing in the room, and the swipe last, which
    /// steps the theme.
    pub const SHOW_ORDER: [Self; 3] = [Self::Palm, Self::Talk, Self::Swipe];

    /// Its place in [`Self::ALL`], and in the arrays of [`HintInput`].
    pub fn index(self) -> usize {
        match self {
            Self::Swipe => 0,
            Self::Talk => 1,
            Self::Palm => 2,
        }
    }

    /// The label's words.
    pub fn text(self) -> &'static str {
        match self {
            Self::Swipe => SWIPE_HINT,
            Self::Talk => TALK_HINT,
            Self::Palm => PALM_HINT,
        }
    }

    /// Its name in the log.
    pub fn name(self) -> &'static str {
        match self {
            Self::Swipe => "swipe",
            Self::Talk => "talk",
            Self::Palm => "palm",
        }
    }
}

/// The hints the wearer has been taught in this run. Nothing of it is
/// saved (board #5575): every launch starts with none taught, or with all
/// of them when the Tips control is off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Taught {
    done: [bool; 3],
}

impl Taught {
    /// Every hint taught.
    pub const ALL: Self = Self { done: [true; 3] };

    pub fn has(self, hint: Hint) -> bool {
        self.done[hint.index()]
    }

    pub fn set(&mut self, hint: Hint) {
        self.done[hint.index()] = true;
    }
}

/// One frame of the hints. The arrays are in [`Hint::ALL`]'s order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HintInput {
    /// The show is on: it has begun.
    pub show_on: bool,
    /// No other label shows this frame.
    pub label_free: bool,
    /// A hand has been tracked this run. Without one the hints wait, so a
    /// wearer still holding the controllers does not use them up.
    pub hands_seen: bool,
    /// The gesture works in this run: the swipe has a theme to step, the
    /// voice has a window to open, the palm has a menu.
    pub can: [bool; 3],
    /// The wearer made the gesture this frame, and it did what it does.
    pub made: [bool; 3],
}

/// What the hints did this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintEvent {
    /// The hint went up.
    Shown(Hint),
    /// The wearer made the gesture while its hint showed.
    Made(Hint),
    /// [`GOT_IT`] went up: the understood cue plays.
    GotIt,
    /// The wearer made the gesture before its hint showed.
    SelfTaught(Hint),
    /// The hint had [`HINT_GIVE_UP_S`] in view.
    GaveUp(Hint),
    /// The run's last hint has gone and [`TIPS_END`] went up.
    TipsEnd,
}

impl HintEvent {
    /// Whether the understood cue plays.
    pub fn confirms(self) -> bool {
        self == Self::GotIt
    }

    /// The log line.
    pub fn line(self) -> String {
        match self {
            Self::Shown(h) => format!(
                "first run: the {} hint shows, \"{}\"",
                h.name(),
                h.text().replace('\n', " | ")
            ),
            Self::Made(h) => format!(
                "first run: the {} hint's gesture was made (taught for this run)",
                h.name()
            ),
            Self::GotIt => format!("first run: the label reads \"{GOT_IT}\""),
            Self::SelfTaught(h) => format!(
                "first run: the {} gesture was made before its hint (taught for this run)",
                h.name()
            ),
            Self::GaveUp(h) => format!(
                "first run: the {} hint gave up after {HINT_GIVE_UP_S:.0} s in view (taught for this run)",
                h.name()
            ),
            Self::TipsEnd => format!(
                "first run: the hints are over, the label reads \"{}\"",
                TIPS_END.replace('\n', " | ")
            ),
        }
    }
}

/// Where the hints are.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Stage {
    /// Between hints: how long the label has been free with the show on.
    Idle { free_s: f32 },
    /// A hint is up: how long it has been the label showing.
    Showing { hint: Hint, shown_s: f32 },
    /// The gesture was made on its hint: how long [`GOT_IT`] has been the
    /// label showing, and how long another label has covered it.
    GotIt { s: f32, covered_s: f32 },
    /// The run's hints are over: how long [`TIPS_END`] has been the label
    /// showing.
    End { s: f32 },
}

/// The hints across frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hints {
    taught: Taught,
    stage: Stage,
    /// How long the show has been on this run (s), for `first_s`.
    on_s: f32,
    /// How long the show is on before the first hint (s): [`FIRST_HINT_S`],
    /// the knob's, or none once the Tips control asked for the hints.
    first_s: f32,
    /// A hint has shown since the hints last started, so [`TIPS_END`] is
    /// owed when they are over. A wearer who made every gesture before
    /// its hint saw no hint and is told nothing.
    end_owed: bool,
}

impl Hints {
    /// With what the wearer has been taught already, the first hint
    /// [`FIRST_HINT_S`] into the show.
    pub fn new(taught: Taught) -> Self {
        Self {
            taught,
            stage: Stage::Idle { free_s: 0.0 },
            on_s: 0.0,
            first_s: FIRST_HINT_S,
            end_owed: false,
        }
    }

    /// The hints of a launch (board #5575): every one with `tips` (the
    /// Tips control, on unless saved off), none without. The first shows
    /// `first_s` into the show ([`FIRST_HINT_S`], or the knob's).
    pub fn at_launch(tips: bool, first_s: f32) -> Self {
        Self {
            first_s: first_s.max(0.0),
            ..Self::new(if tips { Taught::default() } else { Taught::ALL })
        }
    }

    /// The Tips control was turned on: the hints start over, whatever
    /// this run taught, and the first shows after [`REPLAY_GAP_S`] of
    /// free label.
    pub fn replay(&mut self) {
        *self = Self {
            stage: Stage::Idle {
                free_s: HINT_GAP_S - REPLAY_GAP_S,
            },
            on_s: self.on_s,
            first_s: 0.0,
            ..Self::new(Taught::default())
        };
    }

    /// The Tips control was turned off: the hint showing goes, and no
    /// other shows in this run.
    pub fn stop(&mut self) {
        self.taught = Taught::ALL;
        self.stage = Stage::Idle { free_s: 0.0 };
        self.end_owed = false;
    }

    /// Whether a hint may still show in this run: the begin card then
    /// says tips will follow ([`card_text`]).
    pub fn pending(&self) -> bool {
        self.taught != Taught::ALL
    }

    /// What has been taught in this run.
    pub fn taught(&self) -> Taught {
        self.taught
    }

    /// The label's words and alpha now: the hint showing, or [`GOT_IT`].
    /// The app shows them only while no other label is up.
    pub fn now(&self) -> Option<(&'static str, f32)> {
        match self.stage {
            Stage::Idle { .. } => None,
            Stage::Showing { hint, shown_s } => {
                Some((hint.text(), fade_for(shown_s, HINT_GIVE_UP_S)))
            }
            Stage::GotIt { s, .. } => Some((GOT_IT, fade_for(s, GOT_IT_S))),
            Stage::End { s } => Some((TIPS_END, fade_for(s, TIPS_END_S))),
        }
    }

    /// This frame, `dt` seconds after the last: what happened, oldest
    /// first.
    pub fn step(&mut self, dt: f32, input: HintInput) -> Vec<HintEvent> {
        let dt = dt.max(0.0);
        if input.show_on {
            self.on_s += dt;
        }
        let mut events = Vec::new();
        // A gesture made teaches its hint, whether the hint shows or not.
        for hint in Hint::ALL {
            if !input.made[hint.index()] || self.taught.has(hint) {
                continue;
            }
            self.taught.set(hint);
            if matches!(self.stage, Stage::Showing { hint: up, .. } if up == hint) {
                self.stage = Stage::GotIt {
                    s: 0.0,
                    covered_s: 0.0,
                };
                events.push(HintEvent::Made(hint));
            } else {
                events.push(HintEvent::SelfTaught(hint));
            }
        }
        self.stage = match self.stage {
            Stage::Idle { free_s } => {
                let free = input.show_on && input.label_free && input.hands_seen;
                let free_s = if free { free_s + dt } else { free_s };
                let next = Hint::SHOW_ORDER
                    .into_iter()
                    .find(|h| !self.taught.has(*h) && input.can[h.index()]);
                let due = free && free_s >= HINT_GAP_S && self.on_s >= self.first_s;
                match next {
                    Some(hint) if due => {
                        events.push(HintEvent::Shown(hint));
                        self.end_owed = true;
                        Stage::Showing { hint, shown_s: 0.0 }
                    }
                    // No hint is left that can show, and one did: where
                    // the hints are turned off, once.
                    None if due && self.end_owed => {
                        events.push(HintEvent::TipsEnd);
                        self.end_owed = false;
                        Stage::End { s: 0.0 }
                    }
                    _ => Stage::Idle { free_s },
                }
            }
            Stage::End { s } => {
                let s = if input.label_free { s + dt } else { s };
                if s >= TIPS_END_S {
                    Stage::Idle { free_s: 0.0 }
                } else {
                    Stage::End { s }
                }
            }
            // Its gesture stopped working (voice went off): it comes back
            // when the gesture does, untaught.
            Stage::Showing { hint, .. } if !input.can[hint.index()] => Stage::Idle { free_s: 0.0 },
            Stage::Showing { hint, shown_s } => {
                let shown_s = if input.label_free {
                    shown_s + dt
                } else {
                    shown_s
                };
                if shown_s >= HINT_GIVE_UP_S {
                    self.taught.set(hint);
                    events.push(HintEvent::GaveUp(hint));
                    Stage::Idle { free_s: 0.0 }
                } else {
                    Stage::Showing { hint, shown_s }
                }
            }
            // The words count their time while they are the label
            // showing, and wait a little behind another label.
            Stage::GotIt { s, covered_s } if input.label_free => {
                if s <= 0.0 {
                    events.push(HintEvent::GotIt);
                }
                if s + dt >= GOT_IT_S {
                    Stage::Idle { free_s: 0.0 }
                } else {
                    Stage::GotIt {
                        s: s + dt,
                        covered_s,
                    }
                }
            }
            Stage::GotIt { s, covered_s } => {
                if covered_s + dt >= GOT_IT_WAIT_S {
                    Stage::Idle { free_s: 0.0 }
                } else {
                    Stage::GotIt {
                        s,
                        covered_s: covered_s + dt,
                    }
                }
            }
        };
        events
    }
}

/// The label after a thumb swipe stepped the room's theme (board #5470):
/// the new theme's name as the hand menu's Theme control shows it.
pub fn swipe_caption(theme: crate::themes::Theme) -> String {
    theme.title().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::label::{TWO_LINES_FONT_PX, layout, read_s};

    const DT: f32 = 1.0 / 72.0;

    /// Step `wait` for `secs` with `input`. Returns the first begin.
    fn wait_for(wait: &mut Wait, secs: f32, input: WaitInput) -> Option<Begun> {
        (0..(secs / DT).round() as usize).find_map(|_| wait.step(DT, input))
    }

    const READY: WaitInput = WaitInput {
        ready: true,
        gesture: None,
        spoken: false,
        music: false,
        knob: false,
    };

    #[test]
    fn the_show_waits_until_the_wearer_pinches_with_the_card_up() {
        let mut wait = Wait::default();
        assert!(wait.waiting());
        // The scan's label is up: no card, and a pinch begins nothing.
        let scanning = WaitInput {
            ready: false,
            gesture: Some(1),
            ..READY
        };
        assert!(!wait.card_up(false));
        assert_eq!(wait_for(&mut wait, 5.0, scanning), None);
        // The card is up and nobody pinches: it waits.
        assert!(wait.card_up(true));
        assert_eq!(wait_for(&mut wait, 10.0, READY), None);
        assert!(wait.waiting());
        // A pinch on the left hand begins the show, once.
        let pinch = WaitInput {
            gesture: Some(0),
            ..READY
        };
        assert_eq!(wait.step(DT, pinch), Some(Begun::Gesture { hand: 0 }));
        assert!(!wait.waiting() && !wait.card_up(true));
        assert_eq!(wait.begun(), Some(Begun::Gesture { hand: 0 }));
        assert_eq!(wait.step(DT, pinch), None);
        assert_eq!(
            Begun::Gesture { hand: 0 }.line(),
            "first run: the show begins (a pinch on the left hand)"
        );
    }

    #[test]
    fn the_start_gesture_is_a_pinch_on_either_hand() {
        let none = HandEdges::default();
        assert_eq!(start_gesture(&none), None);
        let left = HandEdges {
            pinch_began: [true, false],
            ..none
        };
        assert_eq!(start_gesture(&left), Some(0));
        let both = HandEdges {
            pinch_began: [true, true],
            ..none
        };
        assert_eq!(start_gesture(&both), Some(1));
        // A thumb tap opens the voice window. It does not start the show.
        let tap = HandEdges {
            thumb_tap: [true, true],
            ..none
        };
        assert_eq!(start_gesture(&tap), None);
    }

    #[test]
    fn a_spoken_begin_the_music_and_the_knob_begin_without_the_card() {
        for (input, why) in [
            (
                WaitInput {
                    spoken: true,
                    ..WaitInput::default()
                },
                Begun::Spoken,
            ),
            (
                WaitInput {
                    music: true,
                    ..WaitInput::default()
                },
                Begun::Music,
            ),
            (
                WaitInput {
                    knob: true,
                    ..WaitInput::default()
                },
                Begun::Knob,
            ),
        ] {
            // No room yet, the scan's label still up.
            let mut wait = Wait::default();
            assert_eq!(wait.step(DT, input), Some(why));
            assert!(!wait.waiting());
            // And with the card up.
            let mut wait = Wait::default();
            let ready = WaitInput {
                ready: true,
                ..input
            };
            assert_eq!(wait.step(DT, ready), Some(why));
        }
        assert_eq!(
            Begun::Knob.line(),
            "first run: the show begins (debug.fosfora.begin 1)"
        );
        assert_eq!(
            Begun::Spoken.line(),
            "first run: the show begins (a spoken begin)"
        );
        assert_eq!(
            Begun::Music.line(),
            "first run: the show begins (the wearer started the music)"
        );
    }

    /// Board #5531.
    #[test]
    fn before_the_begin_the_hands_only_begin_and_every_begin_opens_everything() {
        let off = Live {
            menu: false,
            pinches: false,
            swipes: false,
            instruments: false,
        };
        let on = Live {
            menu: true,
            pinches: true,
            swipes: true,
            instruments: true,
        };
        // Under the scan's label and with the card up alike.
        let mut wait = Wait::default();
        assert_eq!(wait.live(), off);
        assert_eq!(wait_for(&mut wait, 5.0, WaitInput::default()), None);
        assert_eq!(wait.live(), off);
        assert_eq!(wait_for(&mut wait, 10.0, READY), None);
        assert_eq!(wait.live(), off);
        // The pinch, the 60 s, the knob, a spoken begin and the music open
        // the same things.
        let pinch = WaitInput {
            gesture: Some(1),
            ..READY
        };
        let knob = WaitInput {
            knob: true,
            ..READY
        };
        let spoken = WaitInput {
            spoken: true,
            ..READY
        };
        let music = WaitInput {
            music: true,
            ..READY
        };
        for (input, secs) in [
            (pinch, DT),
            (READY, WAIT_MAX_S + 1.0),
            (knob, DT),
            (spoken, DT),
            (music, DT),
        ] {
            let mut wait = Wait::default();
            let why = wait_for(&mut wait, secs, input);
            assert!(why.is_some(), "{input:?}");
            assert_eq!(wait.live(), on, "{why:?}");
        }
        assert_eq!(menu_waits_reply(), "Pinch to begin first");
    }

    /// The gestures `pinching` makes over the frames of a wait that ends
    /// on frame `begins`, with the held pinches kept from the machine as
    /// `app.rs` keeps them.
    fn gestures_through_a_begin(
        frames: &[[bool; 2]],
        begins: usize,
    ) -> Vec<crate::gesture::Gesture> {
        use crate::gesture::{Gestures, PinchInput};
        let mut held = HeldPinches::default();
        let mut gestures = Gestures::default();
        let mut out = Vec::new();
        for (i, pinching) in frames.iter().enumerate() {
            let back = held.step(*pinching, i < begins);
            let hands = [0, 1].map(|h| PinchInput {
                pinching: pinching[h] && !back[h],
                point: Some([0.0, 1.0, -0.4]),
            });
            out.extend(gestures.step(hands, DT));
        }
        out
    }

    /// Board #5531.
    #[test]
    fn the_pinch_that_begins_throws_nothing_and_its_release_is_no_tap() {
        use crate::gesture::Gesture;
        const OPEN: [bool; 2] = [false, false];
        const RIGHT: [bool; 2] = [false, true];
        const LEFT: [bool; 2] = [true, false];
        // A pinch before the card, released: nothing. Then the pinch that
        // begins (it closes on frame 6, the show's first), held 10 frames
        // and released: no tap, so no throw and no press. The next pinch
        // on that hand is a tap again.
        let mut frames = vec![OPEN, RIGHT, RIGHT, OPEN, OPEN, OPEN];
        frames.extend([RIGHT; 10]);
        frames.extend([OPEN; 3]);
        assert_eq!(gestures_through_a_begin(&frames, 6), []);
        frames.extend([RIGHT, RIGHT, OPEN]);
        assert_eq!(
            gestures_through_a_begin(&frames, 6),
            [Gesture::Tap { hand: 1 }]
        );
        // A pinch closed during the wait and still closed when the show
        // begins by itself is held the same way. Its release is nothing.
        let mut frames = vec![OPEN, LEFT, LEFT, LEFT, LEFT, LEFT, OPEN, OPEN];
        assert_eq!(gestures_through_a_begin(&frames, 3), []);
        // The other hand, open at the begin, is free at once.
        frames.splice(4..6, [[true, true], LEFT]);
        assert_eq!(
            gestures_through_a_begin(&frames, 3),
            [Gesture::Tap { hand: 1 }]
        );
        // While the show waits both hands are held back, pinching or not.
        // After it, only a hand that has not opened since.
        let mut held = HeldPinches::default();
        assert_eq!(held.step(OPEN, true), [true, true]);
        assert_eq!(held.step(RIGHT, false), [false, true]);
        assert_eq!(held.step(RIGHT, false), [false, true]);
        assert_eq!(held.step(OPEN, false), [false, false]);
        assert_eq!(held.step(RIGHT, false), [false, false]);
    }

    #[test]
    fn nobody_is_stuck_the_show_begins_by_itself_after_a_minute() {
        let mut wait = Wait::default();
        // The time under the scan's label does not count.
        assert_eq!(
            wait_for(&mut wait, 90.0, WaitInput::default()),
            None,
            "not ready"
        );
        assert_eq!(wait_for(&mut wait, WAIT_MAX_S - 0.5, READY), None);
        assert_eq!(wait_for(&mut wait, 1.0, READY), Some(Begun::Timeout));
        assert!(!wait.waiting());
        assert_eq!(
            Begun::Timeout.line(),
            "first run: the show begins (nobody answered the card in 60 s)"
        );
    }

    #[test]
    fn the_card_names_the_pinch_and_the_controllers() {
        assert_eq!(card_text(false, false), "Pinch to begin");
        for tips in [false, true] {
            assert_eq!(
                card_text(true, tips),
                "Pinch to begin\nPut the controllers down. Fosfora uses your hands."
            );
        }
        // One line without the controllers line, three with it.
        assert_eq!(layout(&card_text(false, false)).lines.len(), 1);
        let laid = layout(&card_text(true, false));
        assert_eq!(
            laid.lines[1..],
            ["Put the controllers down.", "Fosfora uses your hands."],
            "{laid:?}"
        );
        // Board #5575: with the tips on the card says they will follow,
        // three lines as written, none wrapped, at the hints' size.
        let laid = layout(&card_text(false, true));
        assert_eq!(
            laid.lines,
            [
                "Pinch to begin",
                "Then listen and watch.",
                "Tips will follow."
            ],
            "{laid:?}"
        );
        assert_close!(laid.px, crate::label::LINES_FONT_PX);
        // A spoken "menu" during the wait is answered with the first line.
        assert_eq!(menu_waits_reply(), "Pinch to begin first");
    }

    #[test]
    fn every_hint_shows_as_it_is_written_at_a_size_worth_reading() {
        let laid = layout(PALM_HINT);
        assert_eq!(
            laid.lines,
            ["Left palm to your face", "opens the menu, and closes it."]
        );
        assert!(laid.px > 64.0, "{laid:?}");
        assert!(laid.px <= TWO_LINES_FONT_PX);
        // A hint stays far longer than it takes to read.
        assert!(HINT_GIVE_UP_S > 2.0 * read_s(PALM_HINT));
        // The swipe hint is three lines as written, none wrapped, at the
        // size the talk hint has: the gesture on two, what it does on the
        // third.
        let laid = layout(SWIPE_HINT);
        assert_eq!(
            laid.lines,
            [
                "Swipe your thumb",
                "along your finger",
                "to change the look"
            ]
        );
        assert_close!(laid.px, crate::label::LINES_FONT_PX);
        assert!(HINT_GIVE_UP_S > 2.0 * read_s(SWIPE_HINT));
        // The talk hint is four lines, each as written, none wrapped, at
        // the size the voice's help card has: the fist step by step, the
        // tap, and something to say.
        let laid = layout(TALK_HINT);
        assert_eq!(
            laid.lines,
            [
                "Make a fist, say a command,",
                "then open your hand.",
                "Or tap thumb on index finger.",
                "Try: what can I say",
            ]
        );
        assert_close!(laid.px, crate::label::LINES_FONT_PX);
        assert!(laid.lines.len() <= crate::label::MAX_LINES);
        assert!(HINT_GIVE_UP_S > 2.0 * read_s(TALK_HINT));
        // Either hand's fist opens the window, so it names no hand. It
        // says what the thumb taps, and that the hand opens to finish, as
        // the label under a held fist does.
        assert!(!TALK_HINT.contains("left") && !TALK_HINT.contains("right"));
        assert!(TALK_HINT.contains("thumb on index finger"));
        assert!(TALK_HINT.contains("open your hand"));
        assert!(crate::voice::HOLD_LABEL.contains("Open your hand"));
        assert_eq!(layout(GOT_IT).lines, [GOT_IT]);
        assert!(GOT_IT_S >= read_s(GOT_IT));
        // Board #5575: the label after the last hint, two lines as
        // written, up longer than it takes to read, and it names the
        // control by the word on it.
        let laid = layout(TIPS_END);
        assert_eq!(
            laid.lines,
            ["Tips show at every start.", "Menu: Tips turns them off."]
        );
        assert!(laid.px > 64.0, "{laid:?}");
        assert!(TIPS_END_S > read_s(TIPS_END));
        for on in [true, false] {
            assert!(tips_label(on).starts_with("Tips: "));
        }
        // The talk hint's example is a sentence the grammar has.
        let said = TALK_HINT.rsplit("Try: ").next().expect("the example");
        let vocab = crate::intent::Vocabulary {
            effects: &[],
            behaviors: &[],
            surfaces: Vec::new(),
            pointed: None,
            tracks: crate::intent::Tracks::default(),
        };
        assert_eq!(
            crate::intent::parse(said, &vocab),
            Ok(crate::intent::Intent::Help)
        );
    }

    /// The show on, the label free, a hand seen, every gesture possible.
    const ON: HintInput = HintInput {
        show_on: true,
        label_free: true,
        hands_seen: true,
        can: [true; 3],
        made: [false; 3],
    };

    /// Step `hints` for `secs` with `input`. Returns every event.
    fn hints_for(hints: &mut Hints, secs: f32, input: HintInput) -> Vec<HintEvent> {
        (0..(secs / DT).round() as usize)
            .flat_map(|_| hints.step(DT, input))
            .collect()
    }

    fn made(hint: Hint) -> HintInput {
        let mut input = ON;
        input.made[hint.index()] = true;
        input
    }

    /// The hints of a run whose opening is over: the show has been on
    /// [`FIRST_HINT_S`] under another label, so the gap has not started.
    fn after_the_opening(taught: Taught) -> Hints {
        let mut hints = Hints::new(taught);
        let covered = HintInput {
            label_free: false,
            ..ON
        };
        assert!(hints_for(&mut hints, FIRST_HINT_S + 0.1, covered).is_empty());
        hints
    }

    #[test]
    fn no_hint_shows_until_the_opening_has_had_its_time() {
        let mut hints = Hints::new(Taught::default());
        // The show on, the label free, a hand seen: nothing for the whole
        // opening, however long the label has been free.
        assert!(hints_for(&mut hints, FIRST_HINT_S - 0.2, ON).is_empty());
        assert_eq!(hints.now(), None);
        assert_eq!(hints.taught(), Taught::default());
        // Then the first hint, at once: the label has been free all along.
        assert_eq!(
            hints_for(&mut hints, 0.4, ON),
            [HintEvent::Shown(Hint::Palm)]
        );
        // The time before the begin does not count.
        let mut hints = Hints::new(Taught::default());
        let before = HintInput {
            show_on: false,
            ..ON
        };
        assert!(hints_for(&mut hints, 2.0 * FIRST_HINT_S, before).is_empty());
        assert!(hints_for(&mut hints, FIRST_HINT_S - 0.2, ON).is_empty());
        // A gesture made during the opening still teaches itself.
        assert_eq!(
            hints.step(DT, made(Hint::Swipe)),
            [HintEvent::SelfTaught(Hint::Swipe)]
        );
        // The opening is long enough to hold the hints back and short
        // against a song.
        const { assert!(FIRST_HINT_S >= 15.0 && FIRST_HINT_S <= 90.0) };
    }

    #[test]
    fn the_hints_show_in_order_one_at_a_time_a_few_seconds_apart() {
        // The order: the two that change nothing in the room, then the
        // swipe, which steps the theme.
        assert_eq!(Hint::SHOW_ORDER, [Hint::Palm, Hint::Talk, Hint::Swipe]);
        for hint in Hint::ALL {
            assert!(Hint::SHOW_ORDER.contains(&hint), "{hint:?}");
        }
        let mut hints = Hints::new(Taught::default());
        // Nothing before the show is on, however long.
        let before = HintInput {
            show_on: false,
            ..ON
        };
        assert!(hints_for(&mut hints, 30.0, before).is_empty());
        assert_eq!(hints.now(), None);
        // The palm first, after the opening and the gap.
        let mut hints = after_the_opening(Taught::default());
        assert!(hints_for(&mut hints, HINT_GAP_S - 0.1, ON).is_empty());
        assert_eq!(
            hints_for(&mut hints, 0.2, ON),
            [HintEvent::Shown(Hint::Palm)]
        );
        assert_eq!(hints.now().map(|n| n.0), Some(PALM_HINT));
        // The wearer turns the palm: "Got it" with its cue, and the hint
        // is taught.
        assert_eq!(
            hints.step(DT, made(Hint::Palm)),
            [HintEvent::Made(Hint::Palm), HintEvent::GotIt]
        );
        assert_eq!(hints.now().map(|n| n.0), Some(GOT_IT));
        assert!(hints.taught().has(Hint::Palm));
        // "Got it" has its time, then the gap, then how to talk.
        assert!(hints_for(&mut hints, GOT_IT_S + HINT_GAP_S - 0.1, ON).is_empty());
        assert_eq!(
            hints_for(&mut hints, 0.2, ON),
            [HintEvent::Shown(Hint::Talk)]
        );
        assert_eq!(
            hints.step(DT, made(Hint::Talk)),
            [HintEvent::Made(Hint::Talk), HintEvent::GotIt]
        );
        // The swipe last.
        assert_eq!(
            hints_for(&mut hints, GOT_IT_S + HINT_GAP_S + 0.1, ON),
            [HintEvent::Shown(Hint::Swipe)]
        );
        assert_eq!(hints.now().map(|n| n.0), Some(SWIPE_HINT));
        assert_eq!(
            hints.step(DT, made(Hint::Swipe)),
            [HintEvent::Made(Hint::Swipe), HintEvent::GotIt]
        );
        assert_eq!(hints.taught(), Taught::ALL);
        // Every hint taught: where they are turned off, once (board
        // #5575), and then nothing more in this run.
        assert_eq!(hints_for(&mut hints, 120.0, ON), [HintEvent::TipsEnd]);
        assert_eq!(hints.now(), None);
        assert!(hints_for(&mut hints, 120.0, ON).is_empty());
    }

    #[test]
    fn a_gesture_made_before_its_hint_teaches_it_without_showing() {
        let mut hints = Hints::new(Taught::default());
        // The palm brings the menu during the wait, before any hint.
        let waiting = HintInput {
            show_on: false,
            ..made(Hint::Palm)
        };
        assert_eq!(hints.step(DT, waiting), [HintEvent::SelfTaught(Hint::Palm)]);
        assert_eq!(hints.now(), None);
        // The talk hint shows after the opening, and the wearer swipes
        // instead: the swipe is taught by doing, and the talk hint stays.
        let covered = HintInput {
            label_free: false,
            ..ON
        };
        hints_for(&mut hints, FIRST_HINT_S + 0.1, covered);
        hints_for(&mut hints, HINT_GAP_S + 0.1, ON);
        assert_eq!(hints.now().map(|n| n.0), Some(TALK_HINT));
        assert_eq!(
            hints.step(DT, made(Hint::Swipe)),
            [HintEvent::SelfTaught(Hint::Swipe)]
        );
        assert_eq!(hints.now().map(|n| n.0), Some(TALK_HINT));
        // Made again, a taught gesture says nothing.
        assert!(hints.step(DT, made(Hint::Swipe)).is_empty());
        // How to talk is the only hint this run shows, and one hint
        // shown is enough to be told where they are turned off.
        assert_eq!(
            hints.step(DT, made(Hint::Talk)),
            [HintEvent::Made(Hint::Talk), HintEvent::GotIt]
        );
        assert_eq!(hints_for(&mut hints, 120.0, ON), [HintEvent::TipsEnd]);
        assert_eq!(hints.taught(), Taught::ALL);
    }

    #[test]
    fn a_hint_gives_up_after_its_time_in_view_and_is_taught() {
        let mut hints = after_the_opening(Taught::default());
        hints_for(&mut hints, HINT_GAP_S + 0.1, ON);
        // Another label covers it for a while: its time stops.
        let covered = HintInput {
            label_free: false,
            ..ON
        };
        assert!(hints_for(&mut hints, 60.0, covered).is_empty());
        assert_eq!(hints.now().map(|n| n.0), Some(PALM_HINT));
        // Back in view, it has its 20 s, fading at the end, then gives up.
        assert!(hints_for(&mut hints, HINT_GIVE_UP_S - 0.3, ON).is_empty());
        assert!(hints.now().is_some_and(|n| n.1 < 1.0));
        assert_eq!(
            hints_for(&mut hints, 0.4, ON),
            [HintEvent::GaveUp(Hint::Palm)]
        );
        assert!(hints.taught().has(Hint::Palm));
        assert_eq!(hints.now(), None);
        // The next one follows after the gap.
        assert_eq!(
            hints_for(&mut hints, HINT_GAP_S + 0.1, ON),
            [HintEvent::Shown(Hint::Talk)]
        );
        // Only "Got it" plays the cue.
        assert!(HintEvent::GotIt.confirms());
        for quiet in [
            HintEvent::TipsEnd,
            HintEvent::Shown(Hint::Talk),
            HintEvent::Made(Hint::Talk),
            HintEvent::SelfTaught(Hint::Talk),
            HintEvent::GaveUp(Hint::Talk),
        ] {
            assert!(!quiet.confirms(), "{quiet:?}");
        }
    }

    #[test]
    fn got_it_waits_for_the_caption_and_is_dropped_behind_a_voice_window() {
        let covered = HintInput {
            label_free: false,
            ..ON
        };
        // The swipe's caption covers the label for 2 s: "Got it" and its
        // cue come when it goes, for their whole time.
        let mut others = Taught::default();
        others.set(Hint::Palm);
        others.set(Hint::Talk);
        let mut hints = after_the_opening(others);
        hints_for(&mut hints, HINT_GAP_S + 0.1, ON);
        assert_eq!(hints.now().map(|n| n.0), Some(SWIPE_HINT));
        let swiped = HintInput {
            label_free: false,
            ..made(Hint::Swipe)
        };
        assert_eq!(hints.step(DT, swiped), [HintEvent::Made(Hint::Swipe)]);
        assert!(hints_for(&mut hints, 2.0, covered).is_empty());
        assert_eq!(hints.step(DT, ON), [HintEvent::GotIt]);
        assert!(hints_for(&mut hints, GOT_IT_S - 0.5, ON).is_empty());
        assert_eq!(hints.now().map(|n| n.0), Some(GOT_IT));
        hints_for(&mut hints, 0.6, ON);
        assert_eq!(hints.now(), None);
        // A spoken "what can I say" brings its card, which stays longer
        // than the wait: no "Got it" and no cue, and the swipe hint follows
        // once the label is free again.
        let mut palm = Taught::default();
        palm.set(Hint::Palm);
        let mut hints = after_the_opening(palm);
        hints_for(&mut hints, HINT_GAP_S + 0.1, ON);
        assert_eq!(hints.now().map(|n| n.0), Some(TALK_HINT));
        let tapped = HintInput {
            label_free: false,
            ..made(Hint::Talk)
        };
        assert_eq!(hints.step(DT, tapped), [HintEvent::Made(Hint::Talk)]);
        assert!(hints_for(&mut hints, GOT_IT_WAIT_S + 0.1, covered).is_empty());
        assert_eq!(hints.now(), None);
        assert!(hints_for(&mut hints, 20.0, covered).is_empty());
        assert_eq!(
            hints_for(&mut hints, HINT_GAP_S + 0.1, ON),
            [HintEvent::Shown(Hint::Swipe)]
        );
    }

    #[test]
    fn a_hint_whose_gesture_does_not_work_is_passed_over_untaught() {
        // Voice is off: nothing opens a window, so the talk hint does not
        // show.
        let no_voice = HintInput {
            can: [true, false, true],
            ..ON
        };
        let mut taught = Taught::default();
        taught.set(Hint::Palm);
        let mut hints = after_the_opening(taught);
        assert_eq!(
            hints_for(&mut hints, HINT_GAP_S + 0.1, no_voice),
            [HintEvent::Shown(Hint::Swipe)]
        );
        hints.step(DT, made(Hint::Swipe));
        // No hint is left that can show: the last label (board #5575),
        // and the talk hint is still untaught.
        assert_eq!(hints_for(&mut hints, 60.0, no_voice), [HintEvent::TipsEnd]);
        assert_eq!(hints.now(), None);
        assert!(!hints.taught().has(Hint::Talk));
        // Voice comes on: the talk hint shows now.
        assert_eq!(
            hints_for(&mut hints, 0.1, ON),
            [HintEvent::Shown(Hint::Talk)]
        );
        // Voice goes off while it shows: it goes, untaught, and comes
        // back with voice.
        assert!(hints_for(&mut hints, 1.0, no_voice).is_empty());
        assert_eq!(hints.now(), None);
        assert!(!hints.taught().has(Hint::Talk));
        assert_eq!(
            hints_for(&mut hints, HINT_GAP_S + 0.1, ON),
            [HintEvent::Shown(Hint::Talk)]
        );
    }

    #[test]
    fn the_hints_wait_for_a_free_label_and_a_tracked_hand() {
        let mut hints = after_the_opening(Taught::default());
        // The controllers are still in hand: no hint is used up.
        let no_hands = HintInput {
            hands_seen: false,
            ..ON
        };
        assert!(hints_for(&mut hints, 120.0, no_hands).is_empty());
        assert_eq!(hints.taught(), Taught::default());
        // A label is up: the gap counts only the free time.
        let busy = HintInput {
            label_free: false,
            ..ON
        };
        hints_for(&mut hints, HINT_GAP_S - 1.0, ON);
        assert!(hints_for(&mut hints, 30.0, busy).is_empty());
        assert!(hints_for(&mut hints, 0.9, ON).is_empty());
        assert_eq!(
            hints_for(&mut hints, 0.2, ON),
            [HintEvent::Shown(Hint::Palm)]
        );
    }

    /// Board #5575: the hints show on every launch, the Tips control
    /// stops them and starts them over, and the run's last hint is
    /// followed, once, by where that control is.
    #[test]
    fn the_hints_show_every_launch_until_the_tips_are_turned_off() {
        // A launch with the tips on: every hint, the first after the
        // knob's wait.
        let mut hints = Hints::at_launch(true, 3.0);
        assert!(hints.pending());
        assert!(hints_for(&mut hints, 2.8, ON).is_empty());
        assert_eq!(
            hints_for(&mut hints, HINT_GAP_S, ON),
            [HintEvent::Shown(Hint::Palm)]
        );
        // The three give up in turn, then the last label, once.
        let events = hints_for(&mut hints, 3.0 * (HINT_GIVE_UP_S + HINT_GAP_S) + 1.0, ON);
        assert_eq!(
            events,
            [
                HintEvent::GaveUp(Hint::Palm),
                HintEvent::Shown(Hint::Talk),
                HintEvent::GaveUp(Hint::Talk),
                HintEvent::Shown(Hint::Swipe),
                HintEvent::GaveUp(Hint::Swipe),
                HintEvent::TipsEnd,
            ]
        );
        assert_eq!(hints.now().map(|n| n.0), Some(TIPS_END));
        assert!(!hints.pending());
        // It counts its time only while it is the label showing.
        let covered = HintInput {
            label_free: false,
            ..ON
        };
        hints_for(&mut hints, 2.0 * TIPS_END_S, covered);
        assert_eq!(hints.now().map(|n| n.0), Some(TIPS_END));
        assert!(hints_for(&mut hints, TIPS_END_S, ON).is_empty());
        assert_eq!(hints.now(), None);
        assert!(hints_for(&mut hints, 120.0, ON).is_empty());
        // The next launch shows them again: nothing of a run is kept.
        assert!(Hints::at_launch(true, FIRST_HINT_S).pending());
        assert_eq!(
            Hints::at_launch(true, FIRST_HINT_S),
            Hints::new(Taught::default())
        );

        // A launch with the tips off: nothing, however long.
        let mut off = Hints::at_launch(false, FIRST_HINT_S);
        assert!(!off.pending());
        assert!(hints_for(&mut off, 300.0, ON).is_empty());
        // Turned on in the menu: the first hint 2 s later, with no wait
        // for the opening, and the rest follow as at a launch.
        off.replay();
        assert!(off.pending());
        assert!(hints_for(&mut off, REPLAY_GAP_S - 0.2, ON).is_empty());
        assert_eq!(hints_for(&mut off, 0.4, ON), [HintEvent::Shown(Hint::Palm)]);
        // Turned off while a hint shows: it goes, nothing follows, and
        // the last label is not owed.
        off.stop();
        assert_eq!(off.now(), None);
        assert!(!off.pending());
        assert!(hints_for(&mut off, 300.0, ON).is_empty());

        // A wearer who made every gesture before its hint saw no hint,
        // and is not told how to turn them off.
        let mut knows = Hints::at_launch(true, FIRST_HINT_S);
        for hint in Hint::ALL {
            assert_eq!(knows.step(DT, made(hint)), [HintEvent::SelfTaught(hint)]);
        }
        assert!(hints_for(&mut knows, 300.0, ON).is_empty());
        // With the microphone denied the talk hint cannot show: the last
        // label follows the two that can.
        let no_voice = HintInput {
            can: [true, false, true],
            ..ON
        };
        let mut hints = Hints::at_launch(true, 0.0);
        let events = hints_for(&mut hints, 3.0 * (HINT_GIVE_UP_S + HINT_GAP_S), no_voice);
        assert_eq!(events.last(), Some(&HintEvent::TipsEnd));
        assert!(!events.contains(&HintEvent::Shown(Hint::Talk)));
    }

    #[test]
    fn the_events_log_what_happened() {
        assert_eq!(
            HintEvent::Shown(Hint::Talk).line(),
            "first run: the talk hint shows, \"Make a fist, say a command, | then open your hand. | Or tap thumb on index finger. | Try: what can I say\""
        );
        assert_eq!(
            HintEvent::Made(Hint::Swipe).line(),
            "first run: the swipe hint's gesture was made (taught for this run)"
        );
        assert_eq!(
            HintEvent::GotIt.line(),
            "first run: the label reads \"Got it\""
        );
        assert_eq!(
            HintEvent::SelfTaught(Hint::Palm).line(),
            "first run: the palm gesture was made before its hint (taught for this run)"
        );
        assert_eq!(
            HintEvent::GaveUp(Hint::Palm).line(),
            "first run: the palm hint gave up after 20 s in view (taught for this run)"
        );
        assert_eq!(
            HintEvent::TipsEnd.line(),
            "first run: the hints are over, the label reads \"Tips show at every start. | Menu: Tips turns them off.\""
        );
    }

    #[test]
    fn the_swipe_caption_names_the_new_theme() {
        use crate::themes::{Shown, Theme};
        assert_eq!(swipe_caption(Theme::Aurora), "Aurora");
        assert_eq!(swipe_caption(Theme::Classic), "Classic");
        for theme in Theme::ALL {
            let text = swipe_caption(theme);
            // The name the menu's Theme control shows for it.
            assert_eq!(text, Shown::Theme(theme).value());
            // One line, at the label's largest size.
            let laid = layout(&text);
            assert_eq!(laid.lines.len(), 1, "{text}");
            assert_close!(laid.px, crate::label::FONT_PX);
        }
        // The theme a swipe came to is the one named: right from Tide is
        // Aurora, left from Classic wraps to the cycle's last.
        assert_eq!(swipe_caption(Shown::Theme(Theme::Tide).step(1)), "Aurora");
        assert_eq!(
            swipe_caption(Shown::Theme(Theme::Classic).step(-1)),
            Theme::ALL[Theme::ALL.len() - 1].title()
        );
    }

    /// The swipe hint is taught by a swipe that changed the theme: "Got
    /// it" goes up once the swipe's caption has had the label, and the
    /// hint is saved as taught. A swipe that changed nothing (the menu was
    /// up, or there is no theme to step) is not fed as made and teaches
    /// nothing.
    #[test]
    fn the_swipe_hint_is_taught_by_a_swipe_that_changed_the_theme() {
        use crate::microgestures::{MicroFrame, Swipe, consume};
        let frame = MicroFrame {
            swipe: [None, Some(Swipe::Left)],
            tap: [false; 2],
        };
        // What `app.rs` feeds as made: whether the swipe stepped the theme.
        let fed = |panel_up: bool, themes: bool| {
            let mut made = [false; 3];
            made[Hint::Swipe.index()] = consume(&frame, panel_up, false, themes).is_some();
            HintInput { made, ..ON }
        };
        let mut others = Taught::default();
        others.set(Hint::Palm);
        others.set(Hint::Talk);
        let mut hints = after_the_opening(others);
        assert_eq!(
            hints_for(&mut hints, HINT_GAP_S + 0.1, ON),
            [HintEvent::Shown(Hint::Swipe)]
        );
        assert_eq!(hints.now().map(|n| n.0), Some(SWIPE_HINT));
        // The menu is up, or no theme to step: the hint stays.
        assert!(hints.step(DT, fed(true, true)).is_empty());
        assert!(hints.step(DT, fed(false, false)).is_empty());
        assert!(!hints.taught().has(Hint::Swipe));
        // The swipe steps the theme: taught, and "Got it".
        assert_eq!(
            hints.step(DT, fed(false, true)),
            [HintEvent::Made(Hint::Swipe), HintEvent::GotIt]
        );
        assert!(hints.taught().has(Hint::Swipe));
        assert_eq!(hints.now().map(|n| n.0), Some(GOT_IT));
        // The hint says the gesture, then what it does.
        assert_eq!(
            SWIPE_HINT,
            "Swipe your thumb\nalong your finger\nto change the look"
        );
    }
}
