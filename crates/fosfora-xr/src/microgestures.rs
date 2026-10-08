//! Thumb microgestures (`XR_META_hand_tracking_microgestures`, board
//! #3336): a thumb swipe left, right, forward or backward along the index
//! finger, or a thumb tap on it, reported as boolean inputs on the EXT hand
//! interaction profile (`XR_EXT_hand_interaction`). Small, deliberate and
//! hard to make by accident, so a swipe steps the world effect.
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

    /// The world effect step this swipe asks for: right the next effect,
    /// left the previous one; forward and backward are reserved.
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
    /// Steps to the next world effect.
    Next,
    /// Steps to the previous world effect.
    Previous,
    /// Forward and backward: nothing assigned yet.
    Unassigned,
    /// The hand menu is up: its hands are the menu's.
    MenuUp,
    /// Edit room is on and this is the right hand: the editor's.
    EditRoom,
    /// Not in world mode, or only one world effect: nothing to step to.
    NoOtherEffect,
}

impl Outcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Next => "next effect",
            Self::Previous => "previous effect",
            Self::Unassigned => "unassigned",
            Self::MenuUp => "nothing (hand menu up)",
            Self::EditRoom => "nothing (edit room)",
            Self::NoOtherEffect => "nothing (no other world effect)",
        }
    }

    fn step(self) -> Option<i32> {
        match self {
            Self::Next => Some(1),
            Self::Previous => Some(-1),
            _ => None,
        }
    }
}

/// What `swipe` on `hand` (0 left, 1 right) does, under the gesture
/// block's rules: nothing while the hand menu is up, nothing from the
/// right hand while Edit room is on, nothing without a second world effect
/// (`world_effects` is 0 outside world mode).
pub fn outcome(
    swipe: Swipe,
    hand: usize,
    panel_up: bool,
    edit_on: bool,
    world_effects: usize,
) -> Outcome {
    if panel_up {
        Outcome::MenuUp
    } else if edit_on && hand == 1 {
        Outcome::EditRoom
    } else {
        match swipe.step() {
            None => Outcome::Unassigned,
            Some(_) if world_effects < 2 => Outcome::NoOtherEffect,
            Some(1) => Outcome::Next,
            Some(_) => Outcome::Previous,
        }
    }
}

/// The world effect step this frame asks for: +1 next, -1 previous, `None`
/// for nothing. Either hand counts; when both step in one frame the right
/// hand's wins.
pub fn consume(
    frame: &MicroFrame,
    panel_up: bool,
    edit_on: bool,
    world_effects: usize,
) -> Option<i32> {
    [1, 0].into_iter().find_map(|hand| {
        frame.swipe[hand].and_then(|s| outcome(s, hand, panel_up, edit_on, world_effects).step())
    })
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
                        "microgestures: suggested {} bindings on {PROFILE}: {e}",
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
        assert_eq!(consume(&swipe(1, Swipe::Right), false, false, 3), Some(1));
        assert_eq!(consume(&swipe(1, Swipe::Left), false, false, 3), Some(-1));
    }

    #[test]
    fn either_hand_counts() {
        assert_eq!(consume(&swipe(0, Swipe::Right), false, false, 3), Some(1));
        assert_eq!(consume(&swipe(0, Swipe::Left), false, false, 3), Some(-1));
    }

    #[test]
    fn nothing_with_the_menu_up() {
        for hand in [0, 1] {
            for s in [Swipe::Left, Swipe::Right] {
                assert_eq!(consume(&swipe(hand, s), true, false, 3), None);
                assert_eq!(outcome(s, hand, true, false, 3), Outcome::MenuUp);
            }
        }
    }

    #[test]
    fn edit_room_takes_the_right_hand_only() {
        assert_eq!(consume(&swipe(1, Swipe::Right), false, true, 3), None);
        assert_eq!(outcome(Swipe::Right, 1, false, true, 3), Outcome::EditRoom);
        assert_eq!(consume(&swipe(0, Swipe::Right), false, true, 3), Some(1));
        assert_eq!(consume(&swipe(0, Swipe::Left), false, true, 3), Some(-1));
    }

    #[test]
    fn nothing_with_one_effect_or_outside_world_mode() {
        for n in [0, 1] {
            assert_eq!(consume(&swipe(1, Swipe::Right), false, false, n), None);
            assert_eq!(
                outcome(Swipe::Left, 0, false, false, n),
                Outcome::NoOtherEffect
            );
        }
        assert_eq!(consume(&swipe(1, Swipe::Right), false, false, 2), Some(1));
    }

    #[test]
    fn forward_backward_and_tap_are_unassigned() {
        for hand in [0, 1] {
            for s in [Swipe::Forward, Swipe::Backward] {
                assert_eq!(consume(&swipe(hand, s), false, false, 3), None);
                assert_eq!(outcome(s, hand, false, false, 3), Outcome::Unassigned);
            }
            let mut tap = MicroFrame::default();
            tap.tap[hand] = true;
            assert_eq!(consume(&tap, false, false, 3), None);
        }
        assert_eq!(consume(&MicroFrame::default(), false, false, 3), None);
    }

    #[test]
    fn the_right_hand_wins_a_tie() {
        let f = MicroFrame {
            swipe: [Some(Swipe::Left), Some(Swipe::Right)],
            tap: [false; 2],
        };
        assert_eq!(consume(&f, false, false, 3), Some(1));
        // With Edit room on the right hand's swipe is the editor's, so the
        // left one steps.
        assert_eq!(consume(&f, false, true, 3), Some(-1));
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
}
