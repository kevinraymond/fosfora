//! The flash limit is the wearer's choice (board #5534): the setting's
//! words, and the question a new wearer is asked before the first show.
//! Plain numbers and words, so the rules build and test on the desktop.
//! `flash.rs` holds the limit itself and what turns it on and off
//! (`flash::decide`); `app.rs` holds an [`Ask`], feeds it once a frame
//! while the show waits and does what it returns.
//!
//! **The setting.** "Limit flashing", on or off, saved in `hand_menu.json`
//! as `"limit_flashing"` (`palm_panel::MenuFile`). A file without the
//! field means the wearer has not chosen, and the limit is on. The hand
//! menu's row reads [`row_label`]; a press, a spoken "limit flashing" or
//! "full show" (`intent::Intent::FlashLimit`) changes it at once.
//!
//! **The question** ([`Ask`], [`card_text`]). On a launch where nothing is
//! saved, the first card is the warning and the question, before "Pinch to
//! begin" (`first_run.rs`). The show is still waiting behind it, so no
//! music plays and nothing the audio drives lights. It ends
//! ([`Outcome`]) when the wearer answers by hand or by voice, when it has
//! been up [`ASK_MAX_S`] and nobody answered, or when the show begins
//! another way (a spoken "begin", the music by voice,
//! `debug.fosfora.begin 1`). Only an answer is saved. Without one the
//! limit stays on and the next launch asks again: nobody chose, and a
//! wearer who was not ready for the card should still get the choice.
//! `debug.fosfora.begin 1` at launch skips the question, and
//! `debug.fosfora.flashask 1` asks it again on an install that has
//! answered ([`Ask::at_launch`]).
//!
//! **The card** ([`card_text`], board #5538). The warning, a blank line,
//! how to answer, the two answers, and a last line that counts the
//! seconds down and names what happens when they run out
//! ([`countdown_line`], [`Ask::left_s`]). Until then nothing on the card
//! said that it would answer for the wearer after a minute, or with what.
//!
//! **Answering by hand** ([`over`]). The two answers sit at the two ends
//! of the answers' line. The wearer puts a hand over one, as they see
//! it, and pinches. The app reads the line from the head through the
//! pinching fingers to the card: left of the gap between the answers is
//! "Limit flashing", right of it "Full show", with a dead band between
//! them. Either hand answers either way. A pinch answers only when its
//! hand has been over the same answer for [`DWELL_S`], and by then the
//! line over the answers names that answer ("Pinch to choose Full show"), so
//! the wearer reads what the pinch will do before making it. A hand at
//! rest in the lap, a hand at the headset's strap and a pinch in passing
//! are over neither answer or have not dwelt, and answer nothing.
//!
//! The head stands in for the eye the wearer looks with, which sits about
//! 3 cm to one side of it. That moves what the app reads by up to 7 cm on
//! the card for a hand 32 cm out, toward the side of that eye. The gap
//! between the answers and the dead band are sized so a hand that covers
//! any part of one answer for either eye never reads as the other
//! ([`REACH_MIN_M`]); at worst it reads as neither, and the line over the
//! answers does not change.
//!
//! **The pinch that answers** does nothing else ([`wait_input`]): on a
//! frame the question is up the wait is handed no card and no start
//! gesture, and the wait begins on a pinch closing, so the pinch that
//! answered has to open and close again to begin the show. It is held
//! back from every other gesture as any pinch of the wait is
//! (`first_run::HeldPinches`).
//!
//! **The strobe** ([`strobe_may_run`]). `debug.fosfora.flash 1`, the
//! measurement strobe, runs only with the limit turned off by its knob.
//! The wearer's "Limit flashing: off" alone never starts it.

use glam::Vec3;

use crate::first_run::WaitInput;
use crate::flash::By;
use crate::label::{Billboard, HANDS_LABEL, LABEL_TEX, LINE_PX, Layout, tex_h};

/// The setting's name in `hand_menu.json`.
pub const FIELD: &str = "limit_flashing";

/// The hand menu's row: what the setting does now.
pub fn row_label(on: bool) -> &'static str {
    if on {
        "Limit flashing: on"
    } else {
        "Limit flashing: off"
    }
}

/// One of the question's two answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// The limit on.
    Limit,
    /// The limit off.
    Full,
}

impl Answer {
    /// Both, left to right on the card.
    pub const ALL: [Self; 2] = [Self::Limit, Self::Full];

    /// What the limit is set to.
    pub fn limits(self) -> bool {
        self == Self::Limit
    }

    /// The answer that sets the limit to `on`.
    pub fn from_limit(on: bool) -> Self {
        if on { Self::Limit } else { Self::Full }
    }

    /// Its words on the card, named for what the wearer gets.
    pub fn words(self) -> &'static str {
        match self {
            Self::Limit => "Limit flashing",
            Self::Full => "Full show",
        }
    }
}

/// The warning, four lines as the card shows them.
pub const WARNING: &str = "This show has flashing lights\nthat follow the music. They can\ntrigger seizures in people\nwith photosensitive epilepsy.";
/// How to answer, the line over the answers while no hand is over one.
pub const HOW: &str = "Put a hand over one and pinch:";
/// The spaces between the two answers on their line: the widest gap the
/// line holds at the card's text size. The countdown has a line of its
/// own for that reason: beside "Limit flashing" it would halve the gap,
/// and a hand on one answer could then read as the other for one eye.
pub const GAP_SPACES: usize = 18;
/// The label for a spoken "menu" while the question is up: the hand menu
/// does not come while the show waits.
pub const MENU_WAITS: &str = "Pick one first";

/// How long the question waits for an answer (s), counted only while the
/// card may show: as long as "Pinch to begin" waits after it.
pub const ASK_MAX_S: f32 = 60.0;
/// How long a hand has to be over an answer before its pinch answers and
/// the card names the answer (s).
pub const DWELL_S: f32 = 0.3;
/// The nearest and the farthest a pinch is read from the head (m). Under
/// the nearest a hand is at the face or the headset, and the eye's offset
/// from the head could move a hand over one answer onto the other.
pub const REACH_MIN_M: f32 = 0.32;
pub const REACH_MAX_M: f32 = 0.90;
/// The dead band between the answers, each side of the middle of the gap
/// between them, in card half-widths (3 cm on the 0.4 m card).
pub const DEAD_HALF_W: f32 = 0.15;
/// How far out from that middle an answer reaches, in card half-widths
/// (0.35 m: 15 cm past the card's end).
pub const SIDE_HALF_W: f32 = 1.75;
/// How far over and under the answers' line a hand still answers, in the
/// card's lines (7.5 cm up, to the fourth line of the warning, and
/// 11 cm down, under the card's edge: a hand comes from below).
pub const ABOVE_LINES: f32 = 2.0;
pub const BELOW_LINES: f32 = 3.0;

/// The countdown, the card's last line (board #5538): the seconds left
/// before the question answers itself, and with what. `limit` is what
/// stands when nobody answers: the limit on a first run, where nothing is
/// saved, and the saved choice when `debug.fosfora.flashask` asks again.
pub fn countdown_line(left_s: u32, limit: bool) -> String {
    format!(
        "No answer in {left_s} s: {}",
        Answer::from_limit(limit).words()
    )
}

/// The line over the answers once a hand has dwelt over `answer`: what a
/// pinch would choose.
pub fn choose_line(answer: Answer) -> String {
    format!("Pinch to choose {}", answer.words())
}

/// The answers on a card laid out as `lines` (`label::layout`), for the
/// label's painter (board #5553), which draws each as a button: the
/// answers' line by its index, and the answer the line over it names
/// ([`choose_line`]), which draws as the control under the pointer does on
/// the hand menu. `None` for any text that is not the question.
pub fn card_answers<S: AsRef<str>>(lines: &[S]) -> Option<(usize, Option<Answer>)> {
    let answers = answers_line();
    let at = lines.iter().rposition(|l| l.as_ref() == answers)?;
    let named = at.checked_sub(1).and_then(|over| {
        Answer::ALL
            .into_iter()
            .find(|a| lines[over].as_ref() == choose_line(*a))
    });
    Some((at, named))
}

/// The answers' line: the two answers, the gap between them.
pub fn answers_line() -> String {
    format!(
        "{}{}{}",
        Answer::Limit.words(),
        " ".repeat(GAP_SPACES),
        Answer::Full.words()
    )
}

/// The question card's words: the warning, a blank line, how to answer,
/// the answers, and with `countdown` (the seconds left and what stands
/// when they run out, [`Ask::left_s`], [`Ask::unanswered_limit`]) the
/// countdown under them. The line over the answers names the answer a
/// pinch would give once a
/// hand has dwelt over it (`over`, from [`Ask::armed`]). With `no_hands`
/// (no hand tracked for `label::HANDS_WAIT_S`) it says to put the
/// controllers down, in the controllers label's own words.
pub fn card_text(over: Option<Answer>, no_hands: bool, countdown: Option<(u32, bool)>) -> String {
    let how = match over {
        Some(answer) => choose_line(answer),
        None if no_hands => {
            let down = HANDS_LABEL.split_once(". ").map_or(HANDS_LABEL, |s| s.0);
            format!("{down}.")
        }
        None => HOW.to_owned(),
    };
    let mut text = format!("{WARNING}\n\n{how}\n{}", answers_line());
    if let Some((left_s, limit)) = countdown {
        text.push('\n');
        text.push_str(&countdown_line(left_s, limit));
    }
    text
}

/// The answer the hand at `point` (the pinch point, between the thumb's
/// and the index finger's tips) is over, as a wearer at `head` sees it on
/// the question card: `card` is the card's one-line pose
/// (`voice::Follow::billboard`) and `laid` its text as the label shows it
/// (`label::layout`). `None` when the hand is nearer than [`REACH_MIN_M`]
/// or farther than [`REACH_MAX_M`], behind the head's side of the card, in
/// the dead band between the answers, or off the answers' line.
pub fn over(head: Vec3, card: &Billboard, laid: &Layout, point: Vec3) -> Option<Answer> {
    let to_hand = point - head;
    if !(REACH_MIN_M..=REACH_MAX_M).contains(&to_hand.length()) {
        return None;
    }
    // The card's plane, its normal away from the head.
    let away = card.up.cross(card.right);
    let ahead = to_hand.dot(away);
    let to_card = (card.center - head).dot(away);
    // The hand is ahead of the head and on the head's side of the card.
    if ahead <= 1e-4 || to_card < ahead {
        return None;
    }
    let on_card = head + to_hand * (to_card / ahead) - card.center;
    let (x, y) = (on_card.dot(card.right), on_card.dot(card.up));
    // The texture's pixels in meters, and the answers' line on it,
    // [`LINE_PX`] a line from the top: the line that is the answers (the
    // countdown may follow it), else the last.
    let m_per_px = 2.0 * card.half_w / LABEL_TEX[0] as f32;
    let lines = laid.lines.len();
    let answers = answers_line();
    let nth = laid
        .lines
        .iter()
        .rposition(|l| *l == answers)
        .map_or(lines, |i| i + 1);
    let line_y = (0.5 * tex_h(lines) as f32 - (LINE_PX * nth as u32) as f32) * m_per_px;
    let line = LINE_PX as f32 * m_per_px;
    if y > line_y + ABOVE_LINES * line || y < line_y - BELOW_LINES * line {
        return None;
    }
    // The line is centered, so the middle of the gap between the answers
    // is off the card's center by half of what the left answer is wider.
    let em = crate::text_width::em;
    let split = 0.5 * (em(Answer::Limit.words()) - em(Answer::Full.words())) * laid.px * m_per_px;
    let dx = x - split;
    let (dead, side) = (DEAD_HALF_W * card.half_w, SIDE_HALF_W * card.half_w);
    if (-side..=-dead).contains(&dx) {
        Some(Answer::Limit)
    } else if (dead..=side).contains(&dx) {
        Some(Answer::Full)
    } else {
        None
    }
}

/// How the question ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A pinch over an answer, on this hand (0 left, 1 right).
    Hand { answer: Answer, hand: usize },
    /// A spoken "limit flashing" or "full show".
    Spoken(Answer),
    /// Nobody answered in [`ASK_MAX_S`].
    Nobody,
    /// The show began before an answer: a spoken "begin", the music by
    /// voice, or `debug.fosfora.begin 1` set while the card was up.
    Begun,
}

impl Outcome {
    /// The answer to save as the wearer's setting. `None` when nobody
    /// answered: nothing is saved, the limit stays on, and the next launch
    /// asks again.
    pub fn answer(self) -> Option<Answer> {
        match self {
            Self::Hand { answer, .. } | Self::Spoken(answer) => Some(answer),
            Self::Nobody | Self::Begun => None,
        }
    }

    /// The log line.
    pub fn line(self) -> String {
        let said = |answer: Answer| {
            format!(
                "\"{}\", the limit {} (saved)",
                answer.words(),
                if answer.limits() { "on" } else { "off" }
            )
        };
        let why = match self {
            Self::Hand { answer, hand } => format!(
                "{} by a pinch on the {} hand",
                said(answer),
                if hand == 0 { "left" } else { "right" }
            ),
            Self::Spoken(answer) => format!("{} by voice", said(answer)),
            Self::Nobody => format!(
                "nobody answered in {ASK_MAX_S:.0} s: the limit stays on, nothing saved, asked again next launch"
            ),
            Self::Begun => "the show began before an answer: the limit stays on, nothing saved, asked again next launch".to_owned(),
        };
        format!("flash question: {why}")
    }
}

/// One frame of the question.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AskInput {
    /// The card shows: the scan's and the permission's labels are gone and
    /// no other label covers it.
    pub shown: bool,
    /// The answer each hand (left, right) is over ([`over`]).
    pub over: [Option<Answer>; 2],
    /// A pinch closed on each hand this frame.
    pub pinch_began: [bool; 2],
    /// A sentence answered.
    pub spoken: Option<Answer>,
    /// The show began this frame (`first_run::Wait::step` returned why).
    pub begun: bool,
}

/// The question across frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ask {
    asking: bool,
    /// What stands when nobody answers: the saved choice, the limit on
    /// when nothing is saved.
    unanswered_limit: bool,
    /// How long the card has shown (s).
    shown_s: f32,
    /// The answer each hand is over, and for how long (s).
    dwell: [(Option<Answer>, f32); 2],
}

impl Ask {
    /// The question for this launch. It is asked when nothing is saved
    /// (`saved`, `hand_menu.json`'s `limit_flashing`) or when
    /// `debug.fosfora.flashask 1` asks again (`again`), and never when
    /// `debug.fosfora.begin 1` skips the wait (`begin_knob`): unworn runs
    /// start the show at once with the limit as the file and the knob have
    /// it, on when nothing is saved.
    pub fn at_launch(saved: Option<bool>, begin_knob: bool, again: bool) -> Self {
        Self {
            asking: (saved.is_none() || again) && !begin_knob,
            unanswered_limit: saved.unwrap_or(true),
            shown_s: 0.0,
            dwell: [(None, 0.0); 2],
        }
    }

    /// Whether the question is still open: the card is the question, and
    /// "Pinch to begin" waits behind it.
    pub fn asking(&self) -> bool {
        self.asking
    }

    /// Board #5538: the whole seconds left before the question answers
    /// itself ([`ASK_MAX_S`], counted only while the card shows), for the
    /// card's countdown: 60 at first, 1 through the last second.
    pub fn left_s(&self) -> u32 {
        (ASK_MAX_S - self.shown_s).max(0.0).ceil() as u32
    }

    /// What stands when nobody answers, for the countdown's words: the
    /// limit on a first run, the saved choice when the question is asked
    /// again.
    pub fn unanswered_limit(&self) -> bool {
        self.unanswered_limit
    }

    /// The launch's log line.
    pub fn launch_line(&self, saved: Option<bool>, begin_knob: bool, again: bool) -> String {
        let why = match (self.asking, saved) {
            (true, None) => "asked before the begin, nothing is saved".to_owned(),
            (true, Some(_)) => "asked again before the begin (debug.fosfora.flashask 1)".to_owned(),
            (false, saved) if begin_knob && (saved.is_none() || again) => {
                "not asked (debug.fosfora.begin 1), nothing saved by this launch".to_owned()
            }
            (false, saved) => format!(
                "not asked, the wearer's setting is saved ({})",
                row_label(saved.unwrap_or(true))
            ),
        };
        format!("flash question: {why}")
    }

    /// The answer a pinch would give now: the one a hand has been over
    /// for [`DWELL_S`], the right hand's first. The card names it.
    pub fn armed(&self) -> Option<Answer> {
        [1, 0].into_iter().find_map(|h| self.armed_on(h))
    }

    fn armed_on(&self, hand: usize) -> Option<Answer> {
        let (answer, s) = self.dwell[hand];
        answer.filter(|_| s >= DWELL_S)
    }

    /// This frame, `dt` seconds after the last. Returns how the question
    /// ended on the frame it does, and `None` on every other frame. A
    /// spoken answer and a begin end it whether the card shows or not. A
    /// pinch answers only with the card showing, on a hand that has been
    /// over one answer for [`DWELL_S`]; the right hand wins a tie. The
    /// [`ASK_MAX_S`] count only while the card shows.
    pub fn step(&mut self, dt: f32, input: AskInput) -> Option<Outcome> {
        if !self.asking {
            return None;
        }
        let dt = dt.max(0.0);
        // What each hand was over before this frame decides its pinch, so
        // a hand that arrives and pinches on one frame has not dwelt.
        let armed = [0, 1].map(|h| self.armed_on(h).filter(|a| input.over[h] == Some(*a)));
        for h in 0..2 {
            let over = input.over[h].filter(|_| input.shown);
            self.dwell[h] = match (over, self.dwell[h]) {
                (Some(now), (Some(was), s)) if now == was => (over, s + dt),
                _ => (over, 0.0),
            };
        }
        if input.shown {
            self.shown_s += dt;
        }
        let by_hand = [1, 0]
            .into_iter()
            .filter(|&h| input.shown && input.pinch_began[h])
            .find_map(|h| armed[h].map(|answer| Outcome::Hand { answer, hand: h }));
        let outcome = if let Some(answer) = input.spoken {
            Some(Outcome::Spoken(answer))
        } else if by_hand.is_some() {
            by_hand
        } else if input.begun {
            Some(Outcome::Begun)
        } else if self.shown_s >= ASK_MAX_S {
            Some(Outcome::Nobody)
        } else {
            None
        };
        if outcome.is_some() {
            self.asking = false;
            self.dwell = [(None, 0.0); 2];
        }
        outcome
    }
}

/// What the wait (`first_run::Wait::step`) is handed on a frame: `input`
/// as it is once the question is over. While the question is up
/// (`asking`, [`Ask::asking`] before this frame's step) the begin card is
/// not, so the card is not ready and no pinch is a start gesture: the
/// pinch that answers the question does not begin the show, and the
/// begin's 60 s do not run under the question. A spoken begin, the music
/// and the knob still begin it.
pub fn wait_input(asking: bool, input: WaitInput) -> WaitInput {
    if asking {
        WaitInput {
            ready: false,
            gesture: None,
            ..input
        }
    } else {
        input
    }
}

/// Whether `debug.fosfora.flash`'s beat strobe may run: only with the
/// limit turned off by its knob, never by the wearer's setting alone.
pub fn strobe_may_run(limit_on: bool, by: By) -> bool {
    !limit_on && by == By::Knob
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::first_run::{Begun, HandEdges, HeldPinches, Wait, start_gesture};
    use crate::gesture::{Gesture, Gestures, PinchInput};
    use crate::label::{LINES_FONT_PX, MAX_LINES, TEX_LINES, layout};
    use glam::Quat;

    const DT: f32 = 1.0 / 72.0;
    const HEAD: Vec3 = Vec3::new(0.2, 1.1, -0.4);

    /// The question card as `app.rs` poses it, for a wearer lying back and
    /// turned a little: its one-line pose and its layout.
    fn card() -> (Billboard, Layout) {
        let rot = Quat::from_rotation_y(0.4) * Quat::from_rotation_x(0.5);
        let pose = crate::voice::Follow::centered(rot).billboard(HEAD, rot);
        (pose, layout(&card_text(None, false, Some((60, true)))))
    }

    /// Where the answers are on the card, in meters right of its center:
    /// each answer's left and right end, and the line's height over the
    /// center.
    fn spans(card: &Billboard, laid: &Layout) -> ([[f32; 2]; 2], f32) {
        let m_per_px = 2.0 * card.half_w / LABEL_TEX[0] as f32;
        let w = |text: &str| crate::text_width::em(text) * laid.px * m_per_px;
        let half = 0.5 * w(&answers_line());
        let n = laid.lines.len();
        let nth = laid
            .lines
            .iter()
            .position(|l| *l == answers_line())
            .expect("the answers' line")
            + 1;
        let y = (0.5 * tex_h(n) as f32 - (LINE_PX * nth as u32) as f32) * m_per_px;
        (
            [
                [-half, -half + w(Answer::Limit.words())],
                [half - w(Answer::Full.words()), half],
            ],
            y,
        )
    }

    /// A hand `reach` meters from an eye that sits `eye` meters to the
    /// right of the head, covering the card's point (`x`, `y`) for that
    /// eye.
    fn hand_over(card: &Billboard, eye: f32, at: [f32; 2], reach: f32) -> Vec3 {
        let from = HEAD + card.right * eye;
        let target = card.center + card.right * at[0] + card.up * at[1];
        from + (target - from).normalize() * reach
    }

    #[test]
    fn the_card_warns_asks_and_names_both_answers_at_the_cards_size() {
        assert_eq!(
            card_text(None, false, Some((60, true))),
            "This show has flashing lights\n\
             that follow the music. They can\n\
             trigger seizures in people\n\
             with photosensitive epilepsy.\n\
             \n\
             Put a hand over one and pinch:\n\
             Limit flashing                  Full show\n\
             No answer in 60 s: Limit flashing"
        );
        // Every state of the card: eight lines, each as written and none
        // wrapped, at the size the voice's help card and the talk hint
        // have, so the answers stay where they are when the line over
        // them changes and as the seconds count down. A blank line sets
        // the warning apart from the question.
        let sixths = [
            (None, false, "Put a hand over one and pinch:"),
            (None, true, "Put the controllers down."),
            (Some(Answer::Limit), false, "Pinch to choose Limit flashing"),
            (Some(Answer::Limit), true, "Pinch to choose Limit flashing"),
            (Some(Answer::Full), false, "Pinch to choose Full show"),
        ];
        for (over, no_hands, sixth) in sixths {
            for left_s in [60, 45, 10, 9, 1] {
                let text = card_text(over, no_hands, Some((left_s, true)));
                let laid = layout(&text);
                assert_eq!(laid.lines, text.split('\n').collect::<Vec<_>>(), "{text}");
                assert_eq!(laid.lines.len(), TEX_LINES);
                assert_close!(laid.px, LINES_FONT_PX);
                assert_eq!(laid.lines[4], "");
                assert_eq!(laid.lines[5], sixth);
                assert_eq!(laid.lines[6], answers_line());
                assert_eq!(laid.lines[7], countdown_line(left_s, true));
                assert!(text.starts_with(WARNING));
            }
        }
        // The countdown says how long is left and what the wearer gets
        // when it runs out: the limit on a first run, the saved choice
        // when the question is asked again.
        assert_eq!(
            countdown_line(45, true),
            "No answer in 45 s: Limit flashing"
        );
        assert_eq!(countdown_line(1, false), "No answer in 1 s: Full show");
        let mut ask = Ask::at_launch(None, false, false);
        assert_eq!((ask.left_s(), ask.unanswered_limit()), (60, true));
        ask.step(0.5, SHOWN);
        assert_eq!(ask.left_s(), 60);
        ask.step(0.6, SHOWN);
        assert_eq!(ask.left_s(), 59);
        ask.step(58.4, SHOWN);
        assert_eq!(ask.left_s(), 1);
        // Under another label the seconds do not run.
        let covered = AskInput {
            shown: false,
            ..SHOWN
        };
        ask.step(30.0, covered);
        assert_eq!(ask.left_s(), 1);
        assert!(!Ask::at_launch(Some(false), false, true).unanswered_limit());
        assert!(Ask::at_launch(Some(true), false, true).unanswered_limit());
        // Without the countdown (the log's line) the card ends on the
        // answers, and a hand reads the same there.
        let plain = card_text(None, false, None);
        assert!(plain.ends_with(&answers_line()));
        assert_eq!(layout(&plain).lines.len(), TEX_LINES - 1);
        // The card is the one label taller than the others.
        const { assert!(TEX_LINES > MAX_LINES) };
        // The warning says what flashes, what it can do and to whom.
        for said in [
            "flashing lights",
            "follow the music",
            "seizures",
            "photosensitive epilepsy",
        ] {
            assert!(WARNING.replace('\n', " ").contains(said), "{said}");
        }
        // The answers are named for what the wearer gets, and the left one
        // is the limit.
        assert_eq!(
            Answer::ALL.map(Answer::words),
            ["Limit flashing", "Full show"]
        );
        let line = answers_line();
        assert!(line.starts_with("Limit flashing") && line.ends_with("Full show"));
        // The controllers line is the controllers label's own first
        // sentence.
        assert!(HANDS_LABEL.starts_with("Put the controllers down. "));
        // The question waits as long as the begin card does after it.
        assert_close!(ASK_MAX_S, crate::first_run::WAIT_MAX_S);
    }

    #[test]
    fn the_setting_s_words_say_what_it_does_now() {
        assert_eq!(row_label(true), "Limit flashing: on");
        assert_eq!(row_label(false), "Limit flashing: off");
        assert_eq!(FIELD, "limit_flashing");
        for answer in Answer::ALL {
            assert_eq!(Answer::from_limit(answer.limits()), answer);
        }
        assert!(Answer::Limit.limits() && !Answer::Full.limits());
        // The row's words start with the answer that turns it on.
        assert!(row_label(true).starts_with(Answer::Limit.words()));
    }

    #[test]
    fn a_hand_over_an_answer_reads_as_that_answer() {
        let (card, laid) = card();
        let (ends, y) = spans(&card, &laid);
        // Along each answer's words, at every reach, seen from the head.
        for (answer, [left, right]) in Answer::ALL.into_iter().zip(ends) {
            for k in 0..=20 {
                let x = left + (right - left) * k as f32 / 20.0;
                for reach in [0.33, 0.4, 0.5, 0.6, 0.75, 0.88] {
                    let hand = hand_over(&card, 0.0, [x, y], reach);
                    assert_eq!(over(HEAD, &card, &laid, hand), Some(answer), "{x} {reach}");
                }
            }
        }
        let at = |x: f32, y: f32| over(HEAD, &card, &laid, hand_over(&card, 0.0, [x, y], 0.45));
        // The gap's middle is a dead band, 3 cm each side.
        let split = 0.5 * (ends[0][1] + ends[1][0]);
        assert_eq!(at(split, y), None);
        assert_eq!(at(split - 0.025, y), None);
        assert_eq!(at(split + 0.025, y), None);
        assert_eq!(at(split - 0.035, y), Some(Answer::Limit));
        assert_eq!(at(split + 0.035, y), Some(Answer::Full));
        // The answers reach past the card's ends and under its edge, and
        // stop: a hand far to a side, over the warning or well under the
        // card is over neither.
        assert_eq!(at(-card.half_w - 0.1, y), Some(Answer::Limit));
        assert_eq!(at(card.half_w + 0.1, y - 0.08), Some(Answer::Full));
        assert_eq!(at(-card.half_w - 0.25, y), None);
        assert_eq!(at(card.half_w + 0.25, y), None);
        let line = LINE_PX as f32 * 2.0 * card.half_w / LABEL_TEX[0] as f32;
        assert_eq!(at(ends[0][0] + 0.05, y + 1.9 * line), Some(Answer::Limit));
        assert_eq!(at(ends[0][0] + 0.05, y + 2.1 * line), None);
        assert_eq!(at(ends[1][1] - 0.05, y - 2.9 * line), Some(Answer::Full));
        assert_eq!(at(ends[1][1] - 0.05, y - 3.1 * line), None);
        // The warning's first line, at the top of the card.
        assert_eq!(at(ends[1][1] - 0.05, y + 5.0 * line), None);
    }

    #[test]
    fn the_painter_finds_the_answers_and_the_one_a_hand_is_over() {
        // Board #5553: the label draws the two answers as buttons, the
        // one the line above names in the pointer's fill. Every state of
        // the card, with and without the countdown.
        for over in [None, Some(Answer::Limit), Some(Answer::Full)] {
            for countdown in [None, Some((45, true))] {
                let laid = layout(&card_text(over, false, countdown));
                assert_eq!(card_answers(&laid.lines), Some((6, over)));
            }
        }
        // No hands: the line above says to put the controllers down, and
        // neither answer is named.
        let laid = layout(&card_text(None, true, Some((60, true))));
        assert_eq!(card_answers(&laid.lines), Some((6, None)));
        // No other label is the question.
        for text in [
            "Pinch to begin",
            MENU_WAITS,
            "Limit flashing: on",
            "Pinch to choose Full show",
        ] {
            assert_eq!(card_answers(&layout(text).lines), None, "{text}");
        }
        // A button's plate reaches past its words by the pad at each end.
        // Between the answers that leaves the dead band clear, with room:
        // a plate never says "press here" where a pinch answers nothing.
        let px_per_half_w = 0.5 * LABEL_TEX[0] as f32;
        let gap = crate::text_width::em(&" ".repeat(GAP_SPACES)) * LINES_FONT_PX;
        let between_plates = gap - 2.0 * crate::panel_style::ANSWER_PAD_PX;
        let dead = 2.0 * DEAD_HALF_W * px_per_half_w;
        assert!(between_plates > dead + 40.0, "{between_plates} over {dead}");
    }

    #[test]
    fn a_hand_at_rest_at_the_face_or_past_the_card_is_over_neither() {
        let (card, laid) = card();
        let (ends, y) = spans(&card, &laid);
        let view = (card.center - HEAD).normalize();
        // In the lap: 45 cm down and a little ahead, on either side.
        for side in [-0.15, 0.15] {
            let lap = HEAD + view * 0.25 - card.up * 0.45 + card.right * side;
            assert_eq!(over(HEAD, &card, &laid, lap), None);
        }
        // At the headset's strap and at the face: nearer than a reach,
        // though on the line to an answer.
        for (answer, [left, right]) in Answer::ALL.into_iter().zip(ends) {
            let x = 0.5 * (left + right);
            for reach in [0.08, 0.15, 0.25, 0.31] {
                let hand = hand_over(&card, 0.0, [x, y], reach);
                assert_eq!(over(HEAD, &card, &laid, hand), None, "{answer:?} {reach}");
            }
            // Past the card and past a reach.
            for reach in [0.95, 1.2, 2.0] {
                let hand = hand_over(&card, 0.0, [x, y], reach);
                assert_eq!(over(HEAD, &card, &laid, hand), None, "{answer:?} {reach}");
            }
            // Behind the head.
            let behind = HEAD - (hand_over(&card, 0.0, [x, y], 0.45) - HEAD);
            assert_eq!(over(HEAD, &card, &laid, behind), None);
        }
        assert_eq!(over(HEAD, &card, &laid, HEAD), None);
    }

    /// The wearer looks with an eye, the app reads the head. Whichever eye
    /// it is, a hand that covers any part of one answer never reads as the
    /// other, and one that covers the middle of an answer from 35 cm out
    /// reads as it. 32 mm is half the usual distance between the eyes,
    /// 36 mm half a wide one.
    #[test]
    fn whichever_eye_the_wearer_looks_with_a_hand_on_one_answer_is_never_the_other() {
        let (card, laid) = card();
        let (ends, y) = spans(&card, &laid);
        for eye in [-0.036, -0.032, 0.0, 0.032, 0.036] {
            for (answer, [left, right]) in Answer::ALL.into_iter().zip(ends) {
                let other = Answer::from_limit(!answer.limits());
                for step in 0..=12 {
                    let reach = 0.30 + 0.05 * step as f32;
                    for k in 0..=20 {
                        let x = left + (right - left) * k as f32 / 20.0;
                        for dy in [-0.03, 0.0, 0.03] {
                            let hand = hand_over(&card, eye, [x, y + dy], reach);
                            assert_ne!(
                                over(HEAD, &card, &laid, hand),
                                Some(other),
                                "eye {eye} reach {reach} x {x}"
                            );
                        }
                    }
                    if (0.35..=0.85).contains(&reach) {
                        let hand = hand_over(&card, eye, [0.5 * (left + right), y], reach);
                        assert_eq!(
                            over(HEAD, &card, &laid, hand),
                            Some(answer),
                            "eye {eye} reach {reach}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_question_is_asked_when_nothing_is_saved_and_never_with_the_begin_knob() {
        // (saved, debug.fosfora.begin 1, debug.fosfora.flashask 1) -> asked.
        for (saved, begin, again, asked) in [
            (None, false, false, true),
            (Some(true), false, false, false),
            (Some(false), false, false, false),
            // The begin knob skips it whatever the file holds.
            (None, true, false, false),
            (Some(false), true, false, false),
            (None, true, true, false),
            (Some(true), true, true, false),
            // The flashask knob asks an install that has answered.
            (Some(true), false, true, true),
            (Some(false), false, true, true),
            (None, false, true, true),
        ] {
            let ask = Ask::at_launch(saved, begin, again);
            assert_eq!(ask.asking(), asked, "{saved:?} {begin} {again}");
            assert_eq!(ask.armed(), None);
        }
        // A file from before the field, read as the app reads it, is asked;
        // one that holds either answer is not.
        let file = |json: &str| crate::palm_panel::MenuFile::parse(json).limit_flashing;
        assert!(Ask::at_launch(file(r#"{"debug":false,"voice":true}"#), false, false).asking());
        assert!(Ask::at_launch(file(""), false, false).asking());
        for json in [r#"{"limit_flashing":true}"#, r#"{"limit_flashing":false}"#] {
            assert!(!Ask::at_launch(file(json), false, false).asking(), "{json}");
        }
        let line = |saved, begin, again| {
            Ask::at_launch(saved, begin, again).launch_line(saved, begin, again)
        };
        assert_eq!(
            line(None, false, false),
            "flash question: asked before the begin, nothing is saved"
        );
        assert_eq!(
            line(Some(false), false, false),
            "flash question: not asked, the wearer's setting is saved (Limit flashing: off)"
        );
        assert_eq!(
            line(Some(true), false, false),
            "flash question: not asked, the wearer's setting is saved (Limit flashing: on)"
        );
        assert_eq!(
            line(None, true, false),
            "flash question: not asked (debug.fosfora.begin 1), nothing saved by this launch"
        );
        assert_eq!(
            line(Some(true), false, true),
            "flash question: asked again before the begin (debug.fosfora.flashask 1)"
        );
        assert_eq!(
            line(Some(false), true, false),
            "flash question: not asked, the wearer's setting is saved (Limit flashing: off)"
        );
    }

    /// The card showing, no hand near it.
    const SHOWN: AskInput = AskInput {
        shown: true,
        over: [None; 2],
        pinch_began: [false; 2],
        spoken: None,
        begun: false,
    };

    /// `hand` over `answer`, pinching closed this frame or not.
    fn hand_on(hand: usize, answer: Answer, pinch: bool) -> AskInput {
        let mut input = SHOWN;
        input.over[hand] = Some(answer);
        input.pinch_began[hand] = pinch;
        input
    }

    /// Step `ask` for `secs` with `input`. Returns the first outcome.
    fn ask_for(ask: &mut Ask, secs: f32, input: AskInput) -> Option<Outcome> {
        (0..(secs / DT).round() as usize).find_map(|_| ask.step(DT, input))
    }

    #[test]
    fn either_hand_alone_gives_either_answer_with_a_pinch_over_it() {
        for hand in [0, 1] {
            for answer in Answer::ALL {
                let mut ask = Ask::at_launch(None, false, false);
                // The hand comes over the answer: once it has dwelt the
                // card names the answer, and nothing is answered yet.
                assert_eq!(
                    ask_for(&mut ask, DWELL_S + 0.1, hand_on(hand, answer, false)),
                    None
                );
                assert_eq!(ask.armed(), Some(answer));
                assert!(ask.asking());
                // The pinch closes: answered, once.
                let outcome = ask.step(DT, hand_on(hand, answer, true));
                assert_eq!(outcome, Some(Outcome::Hand { answer, hand }));
                assert_eq!(outcome.and_then(Outcome::answer), Some(answer));
                assert!(!ask.asking());
                assert_eq!(ask.armed(), None);
                assert_eq!(ask_for(&mut ask, 5.0, hand_on(hand, answer, true)), None);
            }
        }
        assert_eq!(
            Outcome::Hand {
                answer: Answer::Full,
                hand: 0
            }
            .line(),
            "flash question: \"Full show\", the limit off (saved) by a pinch on the left hand"
        );
        assert_eq!(
            Outcome::Hand {
                answer: Answer::Limit,
                hand: 1
            }
            .line(),
            "flash question: \"Limit flashing\", the limit on (saved) by a pinch on the right hand"
        );
    }

    #[test]
    fn a_pinch_answers_nothing_by_accident() {
        let mut ask = Ask::at_launch(None, false, false);
        let pinch_anywhere = AskInput {
            pinch_began: [true; 2],
            ..SHOWN
        };
        // Pinches with no hand over an answer, however many: a hand at
        // rest, a hand on the strap.
        assert_eq!(ask_for(&mut ask, 10.0, pinch_anywhere), None);
        // A hand that arrives over an answer pinching, or pinches before it
        // has dwelt: a pinch in passing.
        assert_eq!(ask.step(DT, hand_on(1, Answer::Full, true)), None);
        assert_eq!(
            ask_for(&mut ask, DWELL_S - 0.1, hand_on(1, Answer::Full, true)),
            None
        );
        assert_eq!(ask.armed(), None);
        // A hand that slides from one answer onto the other starts its
        // dwell again, so the card has named the answer a pinch gives.
        let mut ask = Ask::at_launch(None, false, false);
        ask_for(&mut ask, 1.0, hand_on(1, Answer::Limit, false));
        assert_eq!(ask.armed(), Some(Answer::Limit));
        assert_eq!(ask.step(DT, hand_on(1, Answer::Full, true)), None);
        assert_eq!(ask.armed(), None);
        assert_eq!(ask.step(DT, hand_on(1, Answer::Full, true)), None);
        // A hand that left the answer as it pinched.
        ask_for(&mut ask, 1.0, hand_on(1, Answer::Full, false));
        let left_it = AskInput {
            pinch_began: [false, true],
            ..SHOWN
        };
        assert_eq!(ask.step(DT, left_it), None);
        // The other hand's pinch, away from the card, while this one hovers.
        ask_for(&mut ask, 1.0, hand_on(1, Answer::Full, false));
        let mut other = hand_on(1, Answer::Full, false);
        other.pinch_began[0] = true;
        assert_eq!(ask.step(DT, other), None);
        // Under another label (the voice's "Listening…") the card takes no
        // pinch, and a dwell from before does not carry over.
        let covered = AskInput {
            shown: false,
            ..hand_on(1, Answer::Full, true)
        };
        assert_eq!(ask_for(&mut ask, 2.0, covered), None);
        assert_eq!(ask.armed(), None);
        assert_eq!(ask.step(DT, hand_on(1, Answer::Full, true)), None);
        assert!(ask.asking());
        // Both hands over different answers, both dwelt, both pinching on
        // one frame: the right hand's, as for the other gestures.
        let mut both = hand_on(0, Answer::Full, false);
        both.over[1] = Some(Answer::Limit);
        ask_for(&mut ask, 1.0, both);
        assert_eq!(ask.armed(), Some(Answer::Limit));
        both.pinch_began = [true; 2];
        assert_eq!(
            ask.step(DT, both),
            Some(Outcome::Hand {
                answer: Answer::Limit,
                hand: 1
            })
        );
    }

    #[test]
    fn a_spoken_answer_answers_whether_the_card_shows_or_not() {
        for answer in Answer::ALL {
            for shown in [true, false] {
                let mut ask = Ask::at_launch(None, false, false);
                let said = AskInput {
                    shown,
                    spoken: Some(answer),
                    ..SHOWN
                };
                assert_eq!(ask.step(DT, said), Some(Outcome::Spoken(answer)));
                assert!(!ask.asking());
                assert_eq!(Outcome::Spoken(answer).answer(), Some(answer));
            }
        }
        assert_eq!(
            Outcome::Spoken(Answer::Limit).line(),
            "flash question: \"Limit flashing\", the limit on (saved) by voice"
        );
        assert_eq!(
            Outcome::Spoken(Answer::Full).line(),
            "flash question: \"Full show\", the limit off (saved) by voice"
        );
        // The spoken words are the grammar's, and the card's.
        for answer in Answer::ALL {
            assert_eq!(
                crate::intent::answers_flash(answer.words()),
                Some(answer.limits())
            );
        }
    }

    #[test]
    fn nobody_answers_the_limit_stays_on_and_nothing_is_saved() {
        let mut ask = Ask::at_launch(None, false, false);
        // The time under the scan's label, or under the voice's, does not
        // count.
        let covered = AskInput {
            shown: false,
            ..SHOWN
        };
        assert_eq!(ask_for(&mut ask, 90.0, covered), None);
        assert_eq!(ask_for(&mut ask, ASK_MAX_S - 0.5, SHOWN), None);
        assert!(ask.asking());
        assert_eq!(ask_for(&mut ask, 1.0, SHOWN), Some(Outcome::Nobody));
        assert!(!ask.asking());
        // No answer: nothing to save, so the file still has no choice, the
        // limit is on, and the next launch asks again.
        assert_eq!(Outcome::Nobody.answer(), None);
        assert_eq!(crate::flash::decide(None, None), (true, By::NotAsked));
        assert!(Ask::at_launch(None, false, false).asking());
        assert_eq!(
            Outcome::Nobody.line(),
            "flash question: nobody answered in 60 s: the limit stays on, nothing saved, asked again next launch"
        );
        // The show begun another way before an answer is the same.
        let mut ask = Ask::at_launch(None, false, false);
        let begun = AskInput {
            begun: true,
            ..SHOWN
        };
        assert_eq!(ask.step(DT, begun), Some(Outcome::Begun));
        assert!(!ask.asking());
        assert_eq!(Outcome::Begun.answer(), None);
        assert_eq!(
            Outcome::Begun.line(),
            "flash question: the show began before an answer: the limit stays on, nothing saved, asked again next launch"
        );
        // With `flashask` on an install that had chosen the full show, no
        // answer leaves that choice as it was.
        let mut again = Ask::at_launch(Some(false), false, true);
        let outcome = ask_for(&mut again, ASK_MAX_S + 1.0, SHOWN);
        assert_eq!(outcome.and_then(Outcome::answer), None);
        assert_eq!(crate::flash::decide(None, Some(false)), (false, By::Wearer));
        // A launch that does not ask never answers.
        let mut not_asked = Ask::at_launch(Some(true), false, false);
        assert_eq!(
            ask_for(&mut not_asked, 120.0, hand_on(1, Answer::Full, true)),
            None
        );
    }

    /// What the frames' pinches and hands do through the question and the
    /// begin, wired as `app.rs` wires them: the question's outcomes, the
    /// begins, and the gestures the gesture machine made.
    fn through_the_cards(
        frames: &[([bool; 2], [Option<Answer>; 2])],
    ) -> (Vec<Outcome>, Vec<Begun>, Vec<Gesture>) {
        let mut ask = Ask::at_launch(None, false, false);
        let mut wait = Wait::default();
        let mut held = HeldPinches::default();
        let mut gestures = Gestures::default();
        let mut was = [false; 2];
        let (mut outcomes, mut begins, mut made) = (Vec::new(), Vec::new(), Vec::new());
        for &(pinching, over) in frames {
            let pinch_began = [0, 1].map(|h| pinching[h] && !was[h]);
            was = pinching;
            let edges = HandEdges {
                pinch_began,
                thumb_tap: [false; 2],
            };
            let asking = ask.asking();
            let begun = wait.step(
                DT,
                wait_input(
                    asking,
                    WaitInput {
                        ready: true,
                        gesture: start_gesture(&edges),
                        ..WaitInput::default()
                    },
                ),
            );
            if asking {
                outcomes.extend(ask.step(
                    DT,
                    AskInput {
                        shown: true,
                        over,
                        pinch_began,
                        spoken: None,
                        begun: begun.is_some(),
                    },
                ));
            }
            begins.extend(begun);
            let back = held.step(pinching, wait.waiting());
            let hands = [0, 1].map(|h| PinchInput {
                pinching: pinching[h] && !back[h],
                point: Some([0.0, 1.0, -0.4]),
            });
            made.extend(gestures.step(hands, DT));
        }
        (outcomes, begins, made)
    }

    #[test]
    fn the_pinch_that_answers_does_not_begin_the_show_or_throw() {
        const OPEN: [bool; 2] = [false, false];
        const RIGHT: [bool; 2] = [false, true];
        let on_full = [None, Some(Answer::Full)];
        let away = [None; 2];
        // The right hand comes over "Full show" and dwells, pinches for 10
        // frames and opens, still over it.
        let mut frames = vec![(OPEN, on_full); 40];
        frames.extend([(RIGHT, on_full); 10]);
        frames.extend([(OPEN, on_full); 20]);
        let answered = [Outcome::Hand {
            answer: Answer::Full,
            hand: 1,
        }];
        // Answered, and nothing else: the show still waits, and neither the
        // pinch nor its release is a gesture.
        assert_eq!(
            through_the_cards(&frames),
            (answered.to_vec(), vec![], vec![])
        );
        // Held on for seconds after the answer, the same.
        let mut long = vec![(OPEN, on_full); 40];
        long.extend([(RIGHT, on_full); 300]);
        long.extend([(OPEN, away); 20]);
        assert_eq!(
            through_the_cards(&long),
            (answered.to_vec(), vec![], vec![])
        );
        // The next pinch, anywhere, is the begin's. It throws nothing
        // either, and its release is no tap.
        frames.extend([(RIGHT, away); 10]);
        frames.extend([(OPEN, away); 20]);
        let begun = [Begun::Gesture { hand: 1 }];
        assert_eq!(
            through_the_cards(&frames),
            (answered.to_vec(), begun.to_vec(), vec![])
        );
        // And the one after that is a tap again.
        frames.extend([(RIGHT, away); 3]);
        frames.extend([(OPEN, away); 3]);
        assert_eq!(
            through_the_cards(&frames),
            (
                answered.to_vec(),
                begun.to_vec(),
                vec![Gesture::Tap { hand: 1 }]
            )
        );
        // A pinch away from the answers under the question neither answers
        // nor begins, however long the question is up.
        let mut stray = vec![(OPEN, away); 10];
        for _ in 0..20 {
            stray.extend([(RIGHT, away); 5]);
            stray.extend([([true, false], away); 5]);
            stray.extend([(OPEN, away); 5]);
        }
        assert_eq!(through_the_cards(&stray), (vec![], vec![], vec![]));
    }

    #[test]
    fn under_the_question_the_wait_has_no_card_and_no_start_gesture() {
        let input = WaitInput {
            ready: true,
            gesture: Some(1),
            spoken: true,
            music: true,
            knob: true,
        };
        assert_eq!(wait_input(false, input), input);
        assert_eq!(
            wait_input(true, input),
            WaitInput {
                ready: false,
                gesture: None,
                ..input
            }
        );
        // The begin's 60 s do not run under the question, and a pinch does
        // not begin.
        let pinch = WaitInput {
            ready: true,
            gesture: Some(0),
            ..WaitInput::default()
        };
        let mut wait = Wait::default();
        for _ in 0..(100.0 / DT) as usize {
            assert_eq!(wait.step(DT, wait_input(true, pinch)), None);
        }
        assert!(wait.waiting());
        // A spoken begin, the music and the knob still begin the show under
        // it, and the question then ends unanswered.
        for (input, why) in [
            (
                WaitInput {
                    spoken: true,
                    ..pinch
                },
                Begun::Spoken,
            ),
            (
                WaitInput {
                    music: true,
                    ..pinch
                },
                Begun::Music,
            ),
            (
                WaitInput {
                    knob: true,
                    ..pinch
                },
                Begun::Knob,
            ),
        ] {
            let mut wait = Wait::default();
            let mut ask = Ask::at_launch(None, false, false);
            let begun = wait.step(DT, wait_input(ask.asking(), input));
            assert_eq!(begun, Some(why));
            let ended = AskInput {
                begun: begun.is_some(),
                ..SHOWN
            };
            assert_eq!(ask.step(DT, ended), Some(Outcome::Begun));
        }
        // Once the question is over the same pinch begins.
        assert_eq!(
            wait.step(DT, wait_input(false, pinch)),
            Some(Begun::Gesture { hand: 0 })
        );
    }

    #[test]
    fn the_strobe_runs_only_with_the_limit_off_by_its_knob() {
        assert!(strobe_may_run(false, By::Knob));
        for (on, by) in [
            (false, By::Wearer),
            (true, By::Wearer),
            (true, By::Knob),
            (true, By::NotAsked),
        ] {
            assert!(!strobe_may_run(on, by), "{on} {by:?}");
        }
        // As `app.rs` decides it: the wearer's "Limit flashing: off" alone
        // starts no strobe; the knob at 0 does, whatever is saved.
        let may = |knob, setting| {
            let (on, by) = crate::flash::decide(knob, setting);
            strobe_may_run(on, by)
        };
        assert!(!may(None, Some(false)));
        assert!(!may(Some("1"), Some(false)));
        assert!(may(Some("0"), Some(false)));
        assert!(may(Some("0"), Some(true)));
        assert!(may(Some("0"), None));
    }
}
