//! The first run (board #5452): the show waits for the wearer, and the
//! gestures are taught as the wearer makes them. Plain numbers and words,
//! so the rules build and test on the desktop. `app.rs` holds a [`Wait`]
//! and a [`Hints`], feeds them once a frame and does what they return.
//!
//! **The wait** ([`Wait`]). Every launch waits. Once the scan's and the
//! permission's labels have gone, a card at the center of view says
//! "Pinch to begin" ([`card_text`]). Until the show begins the music does
//! not play, the looks get silence in place of the audio, the room is
//! dimmed to `immerse::WAIT_LEVEL` and a pinch, a drag and a thumb swipe do
//! nothing. The show begins ([`Begun`]) when the wearer makes the start
//! gesture with the card up, says "begin" (`intent::Intent::Begin`),
//! starts the music from the hand menu or by voice, or when the card has
//! been up [`WAIT_MAX_S`] and nobody answered it, so nobody is stuck in
//! front of it. `debug.fosfora.begin 1` begins at once, for unworn runs.
//!
//! **The start gesture** ([`start_gesture`], [`START_WORD`]) is a pinch.
//! That function and that word are the only places that say so.
//!
//! **The hints** ([`Hints`], [`Hint`]). Three gestures are taught once per
//! install, in this order: the thumb swipe, the thumb tap that opens the
//! voice window, and the left palm that brings the menu. A hint shows when
//! the show is on and no other label has been up for [`HINT_GAP_S`], one at
//! a time, ahead of the view where the voice label floats. It waits for
//! the wearer to make its gesture. When they do, the label says
//! [`GOT_IT`] for [`GOT_IT_S`] and the app plays the understood cue with
//! it. The words wait up to [`GOT_IT_WAIT_S`] for a label that covers
//! them, the swipe's caption, and are dropped behind one that stays
//! longer: after a thumb tap the voice window's own "Listening…" is the
//! answer. A hint that has been in view [`HINT_GIVE_UP_S`] gives up.
//! Either way it is saved as taught ([`Taught`], in `hand_menu.json`) and
//! never shows again.
//! A wearer who makes a gesture before its hint has shown has taught
//! themselves, and that hint is saved as taught without showing. A hint
//! never holds the show and never covers another label: its time counts
//! only while it is the label showing.
//!
//! **The caption** ([`swipe_caption`]). A thumb swipe that changes the
//! world effect names the new one on the label, every time.

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

/// The card's words while the show waits. With `voice` (voice is on and
/// the microphone is allowed) it names the spoken way too. With
/// `no_hands` (no hand has been tracked for `label::HANDS_WAIT_S`) the
/// controllers line goes under it.
pub fn card_text(voice: bool, no_hands: bool) -> String {
    let mut text = format!("{START_WORD} to begin");
    if voice {
        text.push_str(", or say begin");
    }
    if no_hands {
        text.push('\n');
        text.push_str(HANDS_LABEL);
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
    /// The wearer started the music, from the hand menu or by voice.
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
}

/// A gesture the first run teaches, in the order the hints show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    /// The thumb swipe along the index finger, which steps the effect.
    Swipe,
    /// The thumb tap on the index finger, which opens the voice window.
    Tap,
    /// The left palm turned to the face, which brings the hand menu.
    Palm,
}

/// The swipe hint's words.
pub const SWIPE_HINT: &str = "Swipe your thumb\nalong your finger";
/// The tap hint's words. The second line is a sentence the grammar has.
pub const TAP_HINT: &str = "Tap your thumb to talk.\nTry: what can I say";
/// The palm hint's words.
pub const PALM_HINT: &str = "Left palm to your face\nfor the menu. It stays there.";
/// What the label says when the wearer makes the gesture a hint asks for,
/// and for how long (s).
pub const GOT_IT: &str = "Got it";
pub const GOT_IT_S: f32 = 2.5;
/// How long [`GOT_IT`] waits for another label to go before it is dropped
/// (s): longer than a swipe's caption, shorter than a voice window.
pub const GOT_IT_WAIT_S: f32 = 3.0;
/// How long the label is free, with the show on, before a hint shows (s).
pub const HINT_GAP_S: f32 = 5.0;
/// How long a hint stays in view before it gives up (s).
pub const HINT_GIVE_UP_S: f32 = 20.0;

impl Hint {
    /// Every hint, in the order they show.
    pub const ALL: [Self; 3] = [Self::Swipe, Self::Tap, Self::Palm];

    /// Its place in [`Self::ALL`], and in the arrays of [`HintInput`].
    pub fn index(self) -> usize {
        match self {
            Self::Swipe => 0,
            Self::Tap => 1,
            Self::Palm => 2,
        }
    }

    /// The label's words.
    pub fn text(self) -> &'static str {
        match self {
            Self::Swipe => SWIPE_HINT,
            Self::Tap => TAP_HINT,
            Self::Palm => PALM_HINT,
        }
    }

    /// Its name in `hand_menu.json` and in the log.
    pub fn name(self) -> &'static str {
        match self {
            Self::Swipe => "swipe",
            Self::Tap => "tap",
            Self::Palm => "palm",
        }
    }

    /// The hint `name` names.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|h| h.name() == name)
    }
}

/// The hints the wearer has been taught, saved in `hand_menu.json` as
/// `"taught":["swipe","tap"]`. A file without the field has taught none.
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

    /// The taught hints' names, in the hints' order.
    pub fn names(self) -> Vec<&'static str> {
        Hint::ALL
            .into_iter()
            .filter(|h| self.has(*h))
            .map(Hint::name)
            .collect()
    }

    /// From the file's `taught` field. A missing field, one that is not a
    /// list, and names that are no hint's teach nothing.
    pub fn from_json(value: Option<&serde_json::Value>) -> Self {
        let mut taught = Self::default();
        let names = value.and_then(serde_json::Value::as_array);
        for hint in names
            .into_iter()
            .flatten()
            .filter_map(|n| n.as_str().and_then(Hint::from_name))
        {
            taught.set(hint);
        }
        taught
    }
}

/// One frame of the hints. The arrays are in [`Hint::ALL`]'s order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HintInput {
    /// The show is on: it has begun and its opening is over.
    pub show_on: bool,
    /// No other label shows this frame.
    pub label_free: bool,
    /// A hand has been tracked this run. Without one the hints wait, so a
    /// wearer still holding the controllers does not use them up.
    pub hands_seen: bool,
    /// The gesture works in this run: the swipe has an effect to step to,
    /// the tap has a voice window to open, the palm has a menu.
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
}

impl HintEvent {
    /// Whether the event taught a hint, so the saved state changed.
    pub fn teaches(self) -> bool {
        matches!(self, Self::Made(_) | Self::SelfTaught(_) | Self::GaveUp(_))
    }

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
                "first run: the {} hint's gesture was made (taught, saved)",
                h.name()
            ),
            Self::GotIt => format!("first run: the label reads \"{GOT_IT}\""),
            Self::SelfTaught(h) => format!(
                "first run: the {} gesture was made before its hint (taught, saved)",
                h.name()
            ),
            Self::GaveUp(h) => format!(
                "first run: the {} hint gave up after {HINT_GIVE_UP_S:.0} s in view (taught, saved)",
                h.name()
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
}

/// The hints across frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hints {
    taught: Taught,
    stage: Stage,
}

impl Hints {
    /// With what the wearer has been taught already.
    pub fn new(taught: Taught) -> Self {
        Self {
            taught,
            stage: Stage::Idle { free_s: 0.0 },
        }
    }

    /// What has been taught, for `hand_menu.json`.
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
        }
    }

    /// This frame, `dt` seconds after the last: what happened, oldest
    /// first.
    pub fn step(&mut self, dt: f32, input: HintInput) -> Vec<HintEvent> {
        let dt = dt.max(0.0);
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
                let next = Hint::ALL
                    .into_iter()
                    .find(|h| !self.taught.has(*h) && input.can[h.index()]);
                match next {
                    Some(hint) if free && free_s >= HINT_GAP_S => {
                        events.push(HintEvent::Shown(hint));
                        Stage::Showing { hint, shown_s: 0.0 }
                    }
                    _ => Stage::Idle { free_s },
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

/// The label after a thumb swipe stepped the world effect: the new
/// effect's name as the hand menu shows it. With the particles off the
/// swipe changes nothing in view, so the caption says so.
pub fn swipe_caption(effect: &str, particles: bool) -> String {
    if particles {
        effect.to_owned()
    } else {
        format!("{effect} \u{b7} particles off")
    }
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
    fn the_card_names_the_pinch_the_voice_and_the_controllers() {
        assert_eq!(card_text(false, false), "Pinch to begin");
        assert_eq!(card_text(true, false), "Pinch to begin, or say begin");
        assert_eq!(
            card_text(false, true),
            "Pinch to begin\nPut the controllers down. Fosfora uses your hands."
        );
        assert_eq!(
            card_text(true, true),
            "Pinch to begin, or say begin\nPut the controllers down. Fosfora uses your hands."
        );
        // One line each without the controllers line, three with it.
        assert_eq!(layout(&card_text(false, false)).lines.len(), 1);
        assert_eq!(layout(&card_text(true, false)).lines.len(), 1);
        for voice in [false, true] {
            let laid = layout(&card_text(voice, true));
            assert_eq!(
                laid.lines[1..],
                ["Put the controllers down.", "Fosfora uses your hands."],
                "{laid:?}"
            );
        }
    }

    #[test]
    fn every_hint_is_two_lines_at_the_two_line_size() {
        for (hint, lines) in [
            (Hint::Swipe, ["Swipe your thumb", "along your finger"]),
            (
                Hint::Tap,
                ["Tap your thumb to talk.", "Try: what can I say"],
            ),
            (
                Hint::Palm,
                ["Left palm to your face", "for the menu. It stays there."],
            ),
        ] {
            let laid = layout(hint.text());
            assert_eq!(laid.lines, lines);
            assert!(laid.px > 64.0, "{hint:?} {laid:?}");
            assert!(laid.px <= TWO_LINES_FONT_PX);
            // A hint stays far longer than it takes to read.
            assert!(HINT_GIVE_UP_S > 2.0 * read_s(hint.text()));
        }
        assert_eq!(layout(GOT_IT).lines, [GOT_IT]);
        assert!(GOT_IT_S >= read_s(GOT_IT));
        // The tap hint's example is a sentence the grammar has.
        let said = TAP_HINT.rsplit("Try: ").next().expect("the example");
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

    #[test]
    fn the_hints_show_in_order_one_at_a_time_a_few_seconds_apart() {
        let mut hints = Hints::new(Taught::default());
        // Nothing before the show is on, however long.
        let before = HintInput {
            show_on: false,
            ..ON
        };
        assert!(hints_for(&mut hints, 30.0, before).is_empty());
        assert_eq!(hints.now(), None);
        // The swipe first, after the gap.
        assert!(hints_for(&mut hints, HINT_GAP_S - 0.1, ON).is_empty());
        assert_eq!(
            hints_for(&mut hints, 0.2, ON),
            [HintEvent::Shown(Hint::Swipe)]
        );
        assert_eq!(hints.now().map(|n| n.0), Some(SWIPE_HINT));
        // The wearer swipes: "Got it" with its cue, and the hint is
        // taught.
        assert_eq!(
            hints.step(DT, made(Hint::Swipe)),
            [HintEvent::Made(Hint::Swipe), HintEvent::GotIt]
        );
        assert_eq!(hints.now().map(|n| n.0), Some(GOT_IT));
        assert!(hints.taught().has(Hint::Swipe));
        // "Got it" has its time, then the gap, then the tap.
        assert!(hints_for(&mut hints, GOT_IT_S + HINT_GAP_S - 0.1, ON).is_empty());
        assert_eq!(
            hints_for(&mut hints, 0.2, ON),
            [HintEvent::Shown(Hint::Tap)]
        );
        assert_eq!(
            hints.step(DT, made(Hint::Tap)),
            [HintEvent::Made(Hint::Tap), HintEvent::GotIt]
        );
        // The palm last.
        assert_eq!(
            hints_for(&mut hints, GOT_IT_S + HINT_GAP_S + 0.1, ON),
            [HintEvent::Shown(Hint::Palm)]
        );
        assert_eq!(hints.now().map(|n| n.0), Some(PALM_HINT));
        assert_eq!(
            hints.step(DT, made(Hint::Palm)),
            [HintEvent::Made(Hint::Palm), HintEvent::GotIt]
        );
        assert_eq!(hints.taught(), Taught::ALL);
        // Every hint taught: nothing shows again, in this run or the next.
        assert!(hints_for(&mut hints, 120.0, ON).is_empty());
        assert_eq!(hints.now(), None);
        let mut next = Hints::new(hints.taught());
        assert!(hints_for(&mut next, 120.0, ON).is_empty());
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
        // The swipe hint shows, and the wearer taps instead: the tap is
        // taught by doing, and the swipe hint stays.
        hints_for(&mut hints, HINT_GAP_S + 0.1, ON);
        assert_eq!(
            hints.step(DT, made(Hint::Tap)),
            [HintEvent::SelfTaught(Hint::Tap)]
        );
        assert_eq!(hints.now().map(|n| n.0), Some(SWIPE_HINT));
        // Made again, a taught gesture says nothing.
        assert!(hints.step(DT, made(Hint::Tap)).is_empty());
        // The swipe is the only hint this install shows.
        assert_eq!(
            hints.step(DT, made(Hint::Swipe)),
            [HintEvent::Made(Hint::Swipe), HintEvent::GotIt]
        );
        assert!(hints_for(&mut hints, 120.0, ON).is_empty());
        assert_eq!(hints.taught(), Taught::ALL);
    }

    #[test]
    fn a_hint_gives_up_after_its_time_in_view_and_is_taught() {
        let mut hints = Hints::new(Taught::default());
        hints_for(&mut hints, HINT_GAP_S + 0.1, ON);
        // Another label covers it for a while: its time stops.
        let covered = HintInput {
            label_free: false,
            ..ON
        };
        assert!(hints_for(&mut hints, 60.0, covered).is_empty());
        assert_eq!(hints.now().map(|n| n.0), Some(SWIPE_HINT));
        // Back in view, it has its 20 s, fading at the end, then gives up.
        assert!(hints_for(&mut hints, HINT_GIVE_UP_S - 0.3, ON).is_empty());
        assert!(hints.now().is_some_and(|n| n.1 < 1.0));
        assert_eq!(
            hints_for(&mut hints, 0.4, ON),
            [HintEvent::GaveUp(Hint::Swipe)]
        );
        assert!(hints.taught().has(Hint::Swipe));
        assert_eq!(hints.now(), None);
        // The next one follows after the gap.
        assert_eq!(
            hints_for(&mut hints, HINT_GAP_S + 0.1, ON),
            [HintEvent::Shown(Hint::Tap)]
        );
        // Only an event that teaches changes the saved state, and only
        // "Got it" plays the cue.
        assert!(!HintEvent::Shown(Hint::Tap).teaches());
        assert!(HintEvent::GaveUp(Hint::Tap).teaches());
        assert!(HintEvent::SelfTaught(Hint::Tap).teaches());
        assert!(HintEvent::Made(Hint::Tap).teaches());
        assert!(HintEvent::GotIt.confirms() && !HintEvent::GotIt.teaches());
        for quiet in [
            HintEvent::Shown(Hint::Tap),
            HintEvent::Made(Hint::Tap),
            HintEvent::SelfTaught(Hint::Tap),
            HintEvent::GaveUp(Hint::Tap),
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
        let mut hints = Hints::new(Taught::default());
        hints_for(&mut hints, HINT_GAP_S + 0.1, ON);
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
        // The thumb tap opens a voice window, whose label stays longer
        // than the wait: no "Got it" and no cue, and the palm hint follows
        // once the label is free again.
        hints_for(&mut hints, HINT_GAP_S + 0.1, ON);
        assert_eq!(hints.now().map(|n| n.0), Some(TAP_HINT));
        let tapped = HintInput {
            label_free: false,
            ..made(Hint::Tap)
        };
        assert_eq!(hints.step(DT, tapped), [HintEvent::Made(Hint::Tap)]);
        assert!(hints_for(&mut hints, GOT_IT_WAIT_S + 0.1, covered).is_empty());
        assert_eq!(hints.now(), None);
        assert!(hints_for(&mut hints, 20.0, covered).is_empty());
        assert_eq!(
            hints_for(&mut hints, HINT_GAP_S + 0.1, ON),
            [HintEvent::Shown(Hint::Palm)]
        );
    }

    #[test]
    fn a_hint_whose_gesture_does_not_work_is_passed_over_untaught() {
        // Voice is off: the tap opens nothing, so its hint does not show.
        let no_voice = HintInput {
            can: [true, false, true],
            ..ON
        };
        let mut taught = Taught::default();
        taught.set(Hint::Swipe);
        let mut hints = Hints::new(taught);
        assert_eq!(
            hints_for(&mut hints, HINT_GAP_S + 0.1, no_voice),
            [HintEvent::Shown(Hint::Palm)]
        );
        hints.step(DT, made(Hint::Palm));
        assert!(hints_for(&mut hints, 60.0, no_voice).is_empty());
        assert!(!hints.taught().has(Hint::Tap));
        // Voice comes on: the tap hint shows now.
        assert_eq!(
            hints_for(&mut hints, 0.1, ON),
            [HintEvent::Shown(Hint::Tap)]
        );
        // Voice goes off while it shows: it goes, untaught, and comes
        // back with voice.
        assert!(hints_for(&mut hints, 1.0, no_voice).is_empty());
        assert_eq!(hints.now(), None);
        assert!(!hints.taught().has(Hint::Tap));
        assert_eq!(
            hints_for(&mut hints, HINT_GAP_S + 0.1, ON),
            [HintEvent::Shown(Hint::Tap)]
        );
    }

    #[test]
    fn the_hints_wait_for_a_free_label_and_a_tracked_hand() {
        let mut hints = Hints::new(Taught::default());
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
            [HintEvent::Shown(Hint::Swipe)]
        );
    }

    #[test]
    fn the_taught_hints_round_trip_through_their_names() {
        assert_eq!(Taught::from_json(None), Taught::default());
        for hint in Hint::ALL {
            assert_eq!(Hint::from_name(hint.name()), Some(hint));
            let mut taught = Taught::default();
            taught.set(hint);
            let json = serde_json::json!(taught.names());
            assert_eq!(Taught::from_json(Some(&json)), taught);
        }
        assert_eq!(Taught::ALL.names(), ["swipe", "tap", "palm"]);
        let json = serde_json::json!(Taught::ALL.names());
        assert_eq!(Taught::from_json(Some(&json)), Taught::ALL);
        // Names that are no hint's, and a field that is no list.
        let odd = serde_json::json!(["fist", 3, "tap"]);
        let mut tap = Taught::default();
        tap.set(Hint::Tap);
        assert_eq!(Taught::from_json(Some(&odd)), tap);
        assert_eq!(
            Taught::from_json(Some(&serde_json::json!(true))),
            Taught::default()
        );
    }

    #[test]
    fn the_events_log_what_happened() {
        assert_eq!(
            HintEvent::Shown(Hint::Tap).line(),
            "first run: the tap hint shows, \"Tap your thumb to talk. | Try: what can I say\""
        );
        assert_eq!(
            HintEvent::Made(Hint::Swipe).line(),
            "first run: the swipe hint's gesture was made (taught, saved)"
        );
        assert_eq!(
            HintEvent::GotIt.line(),
            "first run: the label reads \"Got it\""
        );
        assert_eq!(
            HintEvent::SelfTaught(Hint::Palm).line(),
            "first run: the palm gesture was made before its hint (taught, saved)"
        );
        assert_eq!(
            HintEvent::GaveUp(Hint::Palm).line(),
            "first run: the palm hint gave up after 20 s in view (taught, saved)"
        );
    }

    #[test]
    fn the_swipe_caption_names_the_new_effect() {
        assert_eq!(swipe_caption("Flux Cloud", true), "Flux Cloud");
        assert_eq!(swipe_caption("Embers", true), "Embers");
        assert_eq!(
            swipe_caption("Embers", false),
            "Embers \u{b7} particles off"
        );
        // One line, at the label's largest size for a short name.
        let laid = layout(&swipe_caption("Embers", true));
        assert_eq!(laid.lines.len(), 1);
        assert_close!(laid.px, crate::label::FONT_PX);
        assert_eq!(layout(&swipe_caption("Flux Cloud", false)).lines.len(), 1);
    }
}
