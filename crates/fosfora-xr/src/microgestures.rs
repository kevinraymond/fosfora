//! Thumb microgestures (`XR_META_hand_tracking_microgestures`, board
//! #3336): a thumb swipe left, right, forward or backward along the index
//! finger, or a thumb tap on it, reported as boolean inputs on the EXT hand
//! interaction profile (`XR_EXT_hand_interaction`). Small, deliberate and
//! hard to make by accident, so a swipe steps the room's theme (board
//! #5470; it stepped the world effect while there were three).
//!
//! Hard to make by accident, with one exception found worn (board #5531):
//! a hand closing into a fist drags its thumb along the index, and the
//! runtime reports a swipe. A swipe is therefore held back a moment
//! ([`SwipeGate`], [`SWIPE_HOLD_S`]) and dropped if its hand is a fist by
//! then.
//!
//! Two halves. The decision (what a frame's swipes do) is plain data and
//! builds and tests on every target. The OpenXR half, Android only, is the
//! app's one action set: five boolean actions with the two hands as
//! subaction paths, suggested on `/interaction_profiles/ext/hand_interaction_ext`
//! and attached to the session before it begins, synced once a frame while
//! the session is focused. No controller profile is bound (I5).

/// A thumb swipe along the index finger, in the extension's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Swipe {
    Left,
    Right,
    Forward,
    Backward,
}

impl Swipe {
    /// In the order a frame keeps them when two fire at once.
    pub const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Forward, Self::Backward];

    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Forward => "forward",
            Self::Backward => "backward",
        }
    }

    /// The theme step this swipe asks for: right the next theme, left the
    /// one before; forward and backward are reserved.
    pub fn step(self) -> Option<i32> {
        match self {
            Self::Right => Some(1),
            Self::Left => Some(-1),
            Self::Forward | Self::Backward => None,
        }
    }
}

/// One frame's microgestures per hand (left, right): rising edges only, so
/// a gesture is reported on the frame it fires and not again until it
/// ends and fires anew.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MicroFrame {
    pub swipe: [Option<Swipe>; 2],
    pub tap: [bool; 2],
}

impl MicroFrame {
    pub fn is_empty(&self) -> bool {
        self.swipe == [None, None] && self.tap == [false, false]
    }
}

/// What one hand's swipe does this frame (the log's words after `->`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Steps to the next theme.
    Next,
    /// Steps to the theme before.
    Previous,
    /// Forward and backward: nothing assigned yet.
    Unassigned,
    /// The hand menu is up: its hands are the menu's.
    MenuUp,
    /// Edit room is on and this is the right hand: the editor's.
    EditRoom,
    /// Not in world mode: no room's theme to step.
    NoTheme,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Next => "next theme",
            Self::Previous => "previous theme",
            Self::Unassigned => "unassigned",
            Self::MenuUp => "nothing (hand menu up)",
            Self::EditRoom => "nothing (edit room)",
            Self::NoTheme => "nothing (no theme outside world mode)",
        }
    }

    /// The theme step this outcome takes, if any.
    pub fn step(self) -> Option<i32> {
        match self {
            Self::Next => Some(1),
            Self::Previous => Some(-1),
            _ => None,
        }
    }
}

/// What `swipe` on `hand` (0 left, 1 right) does, under the gesture
/// block's rules: nothing while the hand menu is up, nothing from the
/// right hand while Edit room is on, nothing where there is no theme to
/// step (`themes` is false outside world mode).
pub fn outcome(swipe: Swipe, hand: usize, panel_up: bool, edit_on: bool, themes: bool) -> Outcome {
    if panel_up {
        Outcome::MenuUp
    } else if edit_on && hand == 1 {
        Outcome::EditRoom
    } else {
        match swipe.step() {
            None => Outcome::Unassigned,
            Some(_) if !themes => Outcome::NoTheme,
            Some(1) => Outcome::Next,
            Some(_) => Outcome::Previous,
        }
    }
}

/// The theme step this frame asks for: +1 the next theme, -1 the one
/// before, `None` for nothing. Either hand counts; when both step in one
/// frame the right hand's wins.
pub fn consume(frame: &MicroFrame, panel_up: bool, edit_on: bool, themes: bool) -> Option<i32> {
    [1, 0].into_iter().find_map(|hand| {
        frame.swipe[hand].and_then(|s| outcome(s, hand, panel_up, edit_on, themes).step())
    })
}

/// How long a swipe is held back before it acts (s), to see whether its
/// hand is closing into a fist (board #5531). A fist's voice window steps
/// no theme. In the one worn session that shows it, two fists swiped on
/// the way: one hand was already a fist when its swipe fired, and the
/// other read as one at most 0.11 s after (its window opened 0.375 s after
/// the swipe, less the fist's 0.25 s of dwell, at 72 frames a second).
/// This is that plus three frames. The cost is that every swipe acts this
/// much later.
pub const SWIPE_HOLD_S: f32 = 0.15;
/// How long after a swipe went through a fist on its hand is still
/// reported (s), so the log shows a hold that was too short.
pub const SWIPE_WATCH_S: f32 = 0.5;

/// What the gate did with this frame's swipes, per hand (left, right).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Gated {
    /// The swipes to act on now: held back [`SWIPE_HOLD_S`] ago, their
    /// hands no fist since.
    pub swipe: [Option<Swipe>; 2],
    /// A swipe dropped because its hand is a fist, and how long after the
    /// swipe the fist was seen (s; 0 when it already was one).
    pub dropped: [Option<(Swipe, f32)>; 2],
    /// A fist seen this long after a swipe that went through (s, within
    /// [`SWIPE_WATCH_S`]): the hold would have had to be this long.
    pub late: [Option<f32>; 2],
}

/// Holds each hand's swipe back for [`SWIPE_HOLD_S`] and drops it if the
/// hand is a fist by then: the swipe was the thumb sliding along the index
/// as the hand closed, not a gesture. A swipe from a hand that stays open
/// goes through unchanged, that much later. A second swipe from the same
/// hand lets the first through at once.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SwipeGate {
    /// The swipe held back and for how long (s).
    pending: [Option<(Swipe, f32)>; 2],
    /// How long ago a swipe went through (s), while it is watched.
    passed: [Option<f32>; 2],
}

impl SwipeGate {
    /// This frame, `dt` seconds after the last: the swipes that fired
    /// (`swipe`, rising edges) and whether each hand is a fist now
    /// (`fist`: the pose reader says so, or its fingers are curled under
    /// `pose::FIST_ON_M`).
    pub fn step(&mut self, swipe: [Option<Swipe>; 2], fist: [bool; 2], dt: f32) -> Gated {
        let dt = dt.max(0.0);
        let mut out = Gated::default();
        for h in 0..2 {
            if let Some(age) = self.passed[h].take() {
                if fist[h] {
                    out.late[h] = Some(age);
                } else if age + dt < SWIPE_WATCH_S {
                    self.passed[h] = Some(age + dt);
                }
            }
            if let Some((held, age)) = self.pending[h].take() {
                let age = age + dt;
                if fist[h] {
                    out.dropped[h] = Some((held, age));
                } else if age >= SWIPE_HOLD_S - 1e-6 || swipe[h].is_some() {
                    out.swipe[h] = Some(held);
                    self.passed[h] = Some(0.0);
                } else {
                    self.pending[h] = Some((held, age));
                }
            }
            if let Some(new) = swipe[h] {
                if fist[h] {
                    out.dropped[h] = Some((new, 0.0));
                } else {
                    self.pending[h] = Some((new, 0.0));
                }
            }
        }
        out
    }
}

/// The first of the swipes that rose this frame, in [`Swipe::ALL`] order,
/// and how many rose in all (more than one is logged).
pub fn first_swipe(rose: [bool; 4]) -> (Option<Swipe>, usize) {
    let count = rose.iter().filter(|r| **r).count();
    let first = Swipe::ALL
        .into_iter()
        .zip(rose)
        .find_map(|(s, r)| r.then_some(s));
    (first, count)
}

/// A boolean action's rising edge: it changed since the last sync and is
/// now on.
pub fn rising(changed_since_last_sync: bool, current_state: bool) -> bool {
    changed_since_last_sync && current_state
}

#[cfg(target_os = "android")]
pub use device::Microgestures;

#[cfg(target_os = "android")]
mod device {
    use anyhow::{Context, Result};
    use log::{info, warn};
    use openxr as xr;

    use super::{MicroFrame, Swipe, first_swipe, rising};

    /// The profile the extension's paths extend.
    pub const PROFILE: &str = "/interaction_profiles/ext/hand_interaction_ext";
    const HANDS: [&str; 2] = ["/user/hand/left", "/user/hand/right"];
    /// Action name, localized name and input subpath, in [`Swipe::ALL`]
    /// order. The subpaths are the registry's for
    /// `XR_META_hand_tracking_microgestures` (`xr.xml`, extension 253).
    const SWIPES: [(&str, &str, &str); 4] = [
        (
            "swipe_left",
            "Thumb swipe left",
            "/input/swipe_left_meta/click",
        ),
        (
            "swipe_right",
            "Thumb swipe right",
            "/input/swipe_right_meta/click",
        ),
        (
            "swipe_forward",
            "Thumb swipe forward",
            "/input/swipe_forward_meta/click",
        ),
        (
            "swipe_backward",
            "Thumb swipe backward",
            "/input/swipe_backward_meta/click",
        ),
    ];
    const TAP: (&str, &str, &str) = ("tap_thumb", "Thumb tap", "/input/tap_thumb_meta/click");

    /// The app's action set: the five microgesture actions per hand.
    /// Each action holds its set alive (the crate's handles are
    /// reference-counted), so field order does not matter here.
    pub struct Microgestures {
        set: xr::ActionSet,
        swipe: [xr::Action<bool>; 4],
        tap: xr::Action<bool>,
        hands: [xr::Path; 2],
    }

    impl Microgestures {
        /// Create the action set, suggest its bindings on the EXT hand
        /// interaction profile and attach it to `session`. Must run before
        /// the session begins (`xrAttachSessionActionSets` precedes
        /// `xrBeginSession`), and only once per session: an attached
        /// session takes no other action set.
        pub fn new(instance: &xr::Instance, session: &xr::Session<xr::Vulkan>) -> Result<Self> {
            let hands = HANDS.map(|p| instance.string_to_path(p));
            let hands = [
                hands[0].context("xrStringToPath(/user/hand/left)")?,
                hands[1].context("xrStringToPath(/user/hand/right)")?,
            ];
            let set = instance
                .create_action_set("fosfora", "Fosfora", 0)
                .context("xrCreateActionSet(fosfora)")?;
            let action = |(name, localized, _): (&str, &str, &str)| {
                set.create_action::<bool>(name, localized, &hands)
                    .with_context(|| format!("xrCreateAction({name})"))
            };
            let swipe = [
                action(SWIPES[0])?,
                action(SWIPES[1])?,
                action(SWIPES[2])?,
                action(SWIPES[3])?,
            ];
            let tap = action(TAP)?;

            let profile = instance
                .string_to_path(PROFILE)
                .with_context(|| format!("xrStringToPath({PROFILE})"))?;
            let mut paths = Vec::with_capacity(10);
            for (a, (_, _, sub)) in swipe.iter().zip(SWIPES).chain([(&tap, TAP)]) {
                for hand in HANDS {
                    let full = format!("{hand}{sub}");
                    let path = instance
                        .string_to_path(&full)
                        .with_context(|| format!("xrStringToPath({full})"))?;
                    paths.push((a, path));
                }
            }
            let bindings: Vec<xr::Binding<'_>> = paths
                .iter()
                .map(|(a, p)| xr::Binding::new(*a, *p))
                .collect();
            match instance.suggest_interaction_profile_bindings(profile, &bindings) {
                Ok(()) => info!(
                    "microgestures: suggested {} bindings on {PROFILE}: ok",
                    bindings.len()
                ),
                Err(e) => {
                    warn!(
                        "microgestures: suggested {} bindings on {PROFILE}: {e:?} ({e})",
                        bindings.len()
                    );
                    return Err(e).context("xrSuggestInteractionProfileBindings");
                }
            }
            session
                .attach_action_sets(&[&set])
                .context("xrAttachSessionActionSets")?;
            info!(
                "microgestures: action set fosfora attached (swipe_left, swipe_right, swipe_forward, swipe_backward, tap_thumb; both hands)"
            );
            Ok(Self {
                set,
                swipe,
                tap,
                hands,
            })
        }

        /// The profile each hand is bound to now, for the log on
        /// `INTERACTION_PROFILE_CHANGED`.
        pub fn log_profiles(&self, instance: &xr::Instance, session: &xr::Session<xr::Vulkan>) {
            for (name, path) in ["left", "right"].into_iter().zip(self.hands) {
                let profile = session
                    .current_interaction_profile(path)
                    .map_err(anyhow::Error::from)
                    .and_then(|p| {
                        if p == xr::Path::NULL {
                            Ok("none".to_owned())
                        } else {
                            instance.path_to_string(p).map_err(anyhow::Error::from)
                        }
                    });
                match profile {
                    Ok(p) => info!("microgestures: {name} hand profile {p}"),
                    Err(e) => warn!("microgestures: {name} hand profile unknown: {e:#}"),
                }
            }
        }

        /// Sync the action set and read each hand's rising edges. Call once
        /// a frame, and only while the session is focused: actions are
        /// inactive otherwise.
        pub fn sync(&self, session: &xr::Session<xr::Vulkan>) -> Result<MicroFrame> {
            session
                .sync_actions(&[(&self.set).into()])
                .context("xrSyncActions")?;
            let mut frame = MicroFrame::default();
            for (h, &path) in self.hands.iter().enumerate() {
                let edge = |a: &xr::Action<bool>| -> Result<bool> {
                    let s = a.state(session, path).context("xrGetActionStateBoolean")?;
                    Ok(s.is_active && rising(s.changed_since_last_sync, s.current_state))
                };
                let mut rose = [false; 4];
                for (r, a) in rose.iter_mut().zip(&self.swipe) {
                    *r = edge(a)?;
                }
                let (first, count) = first_swipe(rose);
                if count > 1 {
                    let fired: Vec<&str> = Swipe::ALL
                        .into_iter()
                        .zip(rose)
                        .filter_map(|(s, r)| r.then_some(s.label()))
                        .collect();
                    info!(
                        "microgesture: {count} swipes on the {} hand in one frame ({}), kept {}",
                        if h == 0 { "left" } else { "right" },
                        fired.join(", "),
                        first.map_or("none", Swipe::label)
                    );
                }
                frame.swipe[h] = first;
                frame.tap[h] = edge(&self.tap)?;
            }
            Ok(frame)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swipe(hand: usize, s: Swipe) -> MicroFrame {
        let mut f = MicroFrame::default();
        f.swipe[hand] = Some(s);
        f
    }

    #[test]
    fn right_swipe_steps_forward_left_back() {
        assert_eq!(
            consume(&swipe(1, Swipe::Right), false, false, true),
            Some(1)
        );
        assert_eq!(
            consume(&swipe(1, Swipe::Left), false, false, true),
            Some(-1)
        );
        // The log names the theme's step.
        assert_eq!(
            outcome(Swipe::Right, 1, false, false, true).label(),
            "next theme"
        );
        assert_eq!(
            outcome(Swipe::Left, 1, false, false, true).label(),
            "previous theme"
        );
    }

    /// Right is the next theme and left the one before, from the theme the
    /// room shows, round the cycle and back, wrapping both ways.
    #[test]
    fn a_swipe_steps_the_theme_both_ways_and_wraps() {
        use crate::themes::{Shown, Theme};
        let swiped = |shown: Shown, s: Swipe| {
            consume(&swipe(1, s), false, false, true).map(|step| shown.step(step))
        };
        let mut shown = Shown::Theme(Theme::Classic);
        // Round the cycle and one step on, to Drift: every theme in the
        // cycle's order.
        let n = Theme::ALL.len();
        let mut right = Vec::new();
        for _ in 0..=n {
            let t = swiped(shown, Swipe::Right).expect("a right swipe steps");
            right.push(t);
            shown = Shown::Theme(t);
        }
        let want: Vec<Theme> = (1..=n + 1).map(|i| Theme::ALL[i % n]).collect();
        assert_eq!(right, want);
        assert_eq!(right[..3], [Theme::Drift, Theme::Tide, Theme::Aurora]);
        assert_eq!(right[n - 1..], [Theme::Classic, Theme::Drift]);
        // And back from Drift, wrapping at Classic to the cycle's last.
        let mut left = Vec::new();
        for _ in 0..=n {
            let t = swiped(shown, Swipe::Left).expect("a left swipe steps");
            left.push(t);
            shown = Shown::Theme(t);
        }
        let want: Vec<Theme> = (0..=n).map(|i| Theme::ALL[(n - i) % n]).collect();
        assert_eq!(left, want);
        assert_eq!(left[0], Theme::Classic);
        assert_eq!(left[n], Theme::Classic);
        // The same step as the menu's Theme control and the spoken "next
        // theme" take, from an edited theme and from a custom room too.
        for shown in [
            Shown::Theme(Theme::Aurora),
            Shown::Edited(Theme::Aurora),
            Shown::Custom,
        ] {
            assert_eq!(swiped(shown, Swipe::Right), Some(shown.next()));
            assert_eq!(swiped(shown, Swipe::Left), Some(shown.prev()));
        }
        // Forward and backward step nothing.
        assert_eq!(swiped(shown, Swipe::Forward), None);
    }

    #[test]
    fn either_hand_counts() {
        assert_eq!(
            consume(&swipe(0, Swipe::Right), false, false, true),
            Some(1)
        );
        assert_eq!(
            consume(&swipe(0, Swipe::Left), false, false, true),
            Some(-1)
        );
    }

    #[test]
    fn nothing_with_the_menu_up() {
        for hand in [0, 1] {
            for s in [Swipe::Left, Swipe::Right] {
                assert_eq!(consume(&swipe(hand, s), true, false, true), None);
                assert_eq!(outcome(s, hand, true, false, true), Outcome::MenuUp);
            }
        }
    }

    #[test]
    fn edit_room_takes_the_right_hand_only() {
        assert_eq!(consume(&swipe(1, Swipe::Right), false, true, true), None);
        assert_eq!(
            outcome(Swipe::Right, 1, false, true, true),
            Outcome::EditRoom
        );
        assert_eq!(consume(&swipe(0, Swipe::Right), false, true, true), Some(1));
        assert_eq!(consume(&swipe(0, Swipe::Left), false, true, true), Some(-1));
    }

    #[test]
    fn nothing_outside_world_mode() {
        assert_eq!(consume(&swipe(1, Swipe::Right), false, false, false), None);
        assert_eq!(
            outcome(Swipe::Left, 0, false, false, false),
            Outcome::NoTheme
        );
        assert_eq!(
            consume(&swipe(1, Swipe::Right), false, false, true),
            Some(1)
        );
    }

    #[test]
    fn forward_backward_and_tap_are_unassigned() {
        for hand in [0, 1] {
            for s in [Swipe::Forward, Swipe::Backward] {
                assert_eq!(consume(&swipe(hand, s), false, false, true), None);
                assert_eq!(outcome(s, hand, false, false, true), Outcome::Unassigned);
            }
            let mut tap = MicroFrame::default();
            tap.tap[hand] = true;
            assert_eq!(consume(&tap, false, false, true), None);
        }
        assert_eq!(consume(&MicroFrame::default(), false, false, true), None);
    }

    #[test]
    fn the_right_hand_wins_a_tie() {
        let f = MicroFrame {
            swipe: [Some(Swipe::Left), Some(Swipe::Right)],
            tap: [false; 2],
        };
        assert_eq!(consume(&f, false, false, true), Some(1));
        // With Edit room on the right hand's swipe is the editor's, so the
        // left one steps.
        assert_eq!(consume(&f, false, true, true), Some(-1));
    }

    #[test]
    fn first_swipe_keeps_the_registry_order() {
        assert_eq!(first_swipe([false; 4]), (None, 0));
        assert_eq!(
            first_swipe([false, false, true, false]),
            (Some(Swipe::Forward), 1)
        );
        assert_eq!(
            first_swipe([false, true, true, true]),
            (Some(Swipe::Right), 3)
        );
        assert_eq!(first_swipe([true; 4]), (Some(Swipe::Left), 4));
    }

    #[test]
    fn only_a_rising_edge_counts() {
        assert!(rising(true, true));
        assert!(!rising(true, false), "a release is not a gesture");
        assert!(!rising(false, true), "a held state reports once");
        assert!(!rising(false, false));
    }

    #[test]
    fn an_empty_frame_is_empty() {
        assert!(MicroFrame::default().is_empty());
        assert!(!swipe(0, Swipe::Forward).is_empty());
        let mut tap = MicroFrame::default();
        tap.tap[1] = true;
        assert!(!tap.is_empty());
    }

    const DT: f32 = 1.0 / 72.0;

    /// Step `gate` for `seconds` with no new swipe and the hands as
    /// `fist`, collecting what it did on each frame that it did anything.
    fn gate_for(gate: &mut SwipeGate, fist: [bool; 2], seconds: f32) -> Vec<(f32, Gated)> {
        (0..(seconds / DT).round() as usize)
            .filter_map(|i| {
                let out = gate.step([None; 2], fist, DT);
                (out != Gated::default()).then_some(((i + 1) as f32 * DT, out))
            })
            .collect()
    }

    #[test]
    fn a_swipe_from_an_open_hand_goes_through_after_the_hold_once() {
        for hand in [0, 1] {
            let mut gate = SwipeGate::default();
            let mut fired = [None; 2];
            fired[hand] = Some(Swipe::Right);
            // Held back on the frame it fires.
            assert_eq!(gate.step(fired, [false; 2], DT), Gated::default());
            let did = gate_for(&mut gate, [false; 2], 2.0);
            let [(at, out)] = did.as_slice() else {
                panic!("{did:?}");
            };
            assert!((at - SWIPE_HOLD_S).abs() <= DT, "{at}");
            assert_eq!(out.swipe[hand], Some(Swipe::Right));
            assert_eq!(out.swipe[1 - hand], None);
            assert_eq!((out.dropped, out.late), ([None; 2], [None; 2]));
            // What comes out steps the theme as the swipe itself would.
            let frame = MicroFrame {
                swipe: out.swipe,
                tap: [false; 2],
            };
            assert_eq!(consume(&frame, false, false, true), Some(1));
        }
        // The hold is short: under a fifth of a second.
        const { assert!(SWIPE_HOLD_S < 0.2) };
    }

    #[test]
    fn a_swipe_from_a_hand_closing_into_a_fist_is_dropped() {
        // The hand is a fist already when the swipe fires.
        let mut gate = SwipeGate::default();
        let out = gate.step([Some(Swipe::Left), None], [true, false], DT);
        assert_eq!(out.dropped, [Some((Swipe::Left, 0.0)), None]);
        assert_eq!(out.swipe, [None; 2]);
        assert!(gate_for(&mut gate, [true, false], 1.0).is_empty());
        // The fist reads 0.11 s after the swipe, the longest seen worn.
        let mut gate = SwipeGate::default();
        gate.step([Some(Swipe::Left), None], [false; 2], DT);
        assert!(gate_for(&mut gate, [false; 2], 0.11 - DT).is_empty());
        let did = gate_for(&mut gate, [true, false], 1.0);
        let [(_, out)] = did.as_slice() else {
            panic!("{did:?}");
        };
        let Some((Swipe::Left, age)) = out.dropped[0] else {
            panic!("{out:?}");
        };
        assert!((age - 0.11).abs() <= DT, "{age}");
        assert_eq!(out.swipe, [None; 2]);
        // The other hand's fist drops nothing: the right hand swipes
        // while the left one holds the voice window.
        let mut gate = SwipeGate::default();
        gate.step([None, Some(Swipe::Right)], [true, false], DT);
        let did = gate_for(&mut gate, [true, false], 1.0);
        assert!(
            matches!(did.as_slice(), [(_, out)] if out.swipe == [None, Some(Swipe::Right)]),
            "{did:?}"
        );
    }

    #[test]
    fn a_fist_after_the_hold_is_too_late_and_the_log_says_by_how_much() {
        let mut gate = SwipeGate::default();
        gate.step([Some(Swipe::Left), None], [false; 2], DT);
        let did = gate_for(&mut gate, [false; 2], 0.3);
        assert!(
            matches!(did.as_slice(), [(_, out)] if out.swipe[0] == Some(Swipe::Left)),
            "{did:?}"
        );
        // The swipe went through 0.15 s in, the fist reads at 0.3 s.
        let did = gate_for(&mut gate, [true, false], 1.0);
        let [(_, out)] = did.as_slice() else {
            panic!("{did:?}");
        };
        let late = out.late[0].expect("a late fist");
        assert!((late - (0.3 - SWIPE_HOLD_S)).abs() <= 2.0 * DT, "{late}");
        // A fist long after is no business of the swipe's.
        let mut gate = SwipeGate::default();
        gate.step([Some(Swipe::Left), None], [false; 2], DT);
        gate_for(&mut gate, [false; 2], SWIPE_HOLD_S + SWIPE_WATCH_S + 0.1);
        assert!(gate_for(&mut gate, [true, false], 1.0).is_empty());
    }

    #[test]
    fn two_quick_swipes_from_one_hand_both_go_through() {
        let mut gate = SwipeGate::default();
        gate.step([None, Some(Swipe::Right)], [false; 2], DT);
        assert!(gate_for(&mut gate, [false; 2], 0.08).is_empty());
        // The second lets the first through at once and is held in turn.
        let out = gate.step([None, Some(Swipe::Left)], [false; 2], DT);
        assert_eq!(out.swipe, [None, Some(Swipe::Right)]);
        let did = gate_for(&mut gate, [false; 2], 1.0);
        assert!(
            matches!(did.as_slice(), [(_, out)] if out.swipe == [None, Some(Swipe::Left)]),
            "{did:?}"
        );
    }

    /// The swipes the worn sessions recorded that the wearer meant: 25
    /// that stepped the theme or the effect, on two days, either hand.
    /// Each is the swiping hand's smallest curl (m) in the second the
    /// swipe fired in and in the second after. The log has the curl as a
    /// range once a second, so these bound the curl at every frame the
    /// gate looks at. None comes near the fist's, so the gate drops none.
    #[test]
    fn every_recorded_swipe_that_was_meant_still_goes_through() {
        const MEANT: [(f32, f32); 25] = [
            (0.115, 0.111),
            (0.114, 0.119),
            (0.117, 0.118),
            (0.121, 0.104),
            (0.104, 0.106),
            (0.105, 0.132),
            (0.121, 0.104),
            (0.104, 0.101),
            (0.094, 0.130),
            (0.114, 0.116),
            (0.067, 0.118),
            (0.112, 0.066),
            (0.109, 0.113),
            (0.104, 0.104),
            (0.118, 0.117),
            (0.121, 0.114),
            (0.119, 0.105),
            (0.100, 0.117),
            (0.069, 0.089),
            (0.090, 0.083),
            (0.118, 0.130),
            (0.124, 0.126),
            (0.118, 0.119),
            (0.123, 0.125),
            (0.123, 0.132),
        ];
        // The two that came from a closing fist: under the fist's curl in
        // the second they fired in.
        const FISTS: [f32; 2] = [0.050, 0.046];
        let fist = |curl: f32| curl < crate::pose::FIST_ON_M;
        for (now, next) in MEANT {
            assert!(!fist(now) && !fist(next), "{now} {next}");
            // Through the gate with the hand at its most curled the whole
            // time: out after the hold.
            let mut gate = SwipeGate::default();
            let curled = [false, fist(now.min(next))];
            gate.step([None, Some(Swipe::Right)], curled, DT);
            let did = gate_for(&mut gate, curled, 1.0);
            assert!(
                matches!(did.as_slice(), [(_, out)] if out.swipe[1] == Some(Swipe::Right)),
                "{now} {next}"
            );
        }
        for curl in FISTS {
            assert!(fist(curl), "{curl}");
        }
        // The closest a meant swipe came to the fist's curl.
        let least = MEANT
            .iter()
            .map(|(a, b)| a.min(*b))
            .fold(f32::MAX, f32::min);
        assert!(least - crate::pose::FIST_ON_M > 0.01, "{least}");
    }
}
