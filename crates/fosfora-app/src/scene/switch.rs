//! Switching presets with a transition: a scene cue's, or the default one a
//! plain preset switch uses (GH #217).
//!
//! Both kinds of switch run through here, so a cue and a preset click look the
//! same and share one clock. A switch is staged before it is applied:
//!
//! 1. **Decoding** (only when the incoming preset has media): the old preset
//!    stays live while the media decodes off-thread. A video can take many
//!    seconds; starting the fade at the request instead would fade a frozen
//!    frame into the *old* preset and then cut to the new one whenever the
//!    decode landed.
//! 2. **Capture**: the next render copies the frame on screen, crossfade
//!    included, so switching again mid-transition starts from what is visible
//!    rather than jumping.
//! 3. **Apply**: the next update loads the preset and starts an
//!    [`ActiveTransition`].
//!
//! Everything here is GPU-free so the staging and the morph rule unit-test in
//! the default build.

use std::path::PathBuf;

use super::cueing::MorphSnapshot;
use super::types::TransitionType;
use crate::gpu::layer::BlendMode;
use crate::preset::loader::PresetDecodeResult;
use crate::trama::node::NodeKind;

/// Renders an update may go without before a staged switch stops waiting for
/// its capture and applies without a frame dissolve (a minimized window can
/// skip renders while updates keep running).
const MAX_CAPTURE_WAIT: u8 = 3;

/// How a switch is shown: which transition, and for how long.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransitionStyle {
    pub kind: TransitionType,
    pub secs: f32,
}

impl TransitionStyle {
    /// A switch with nothing to animate: a Cut, or a transition with no length.
    pub fn is_cut(&self) -> bool {
        self.kind == TransitionType::Cut || self.secs.is_nan() || self.secs <= 0.0
    }
}

/// Where a staged switch is. See the module doc for the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Decoding,
    AwaitCapture {
        waited: u8,
    },
    /// Ready to apply. `captured` is false when the capture never happened,
    /// and the switch must not crossfade from whatever the snapshot holds.
    Ready {
        captured: bool,
    },
}

/// A switch waiting to be applied.
pub struct StagedSwitch {
    pub preset: usize,
    /// The cue this switch plays, whose `param_overrides` apply on load.
    pub cue: Option<usize>,
    pub style: TransitionStyle,
    pub stage: Stage,
    /// The decoded media, once the decode lands.
    pub decoded: Option<PresetDecodeResult>,
}

impl StagedSwitch {
    pub fn new(preset: usize, cue: Option<usize>, style: TransitionStyle, decoding: bool) -> Self {
        Self {
            preset,
            cue,
            style,
            stage: if decoding {
                Stage::Decoding
            } else {
                Stage::AwaitCapture { waited: 0 }
            },
            decoded: None,
        }
    }

    /// The decode landed: capture the frame that is on screen *now*.
    pub fn decode_landed(&mut self, result: PresetDecodeResult) {
        if self.stage == Stage::Decoding {
            self.decoded = Some(result);
            self.stage = Stage::AwaitCapture { waited: 0 };
        }
    }

    pub fn wants_capture(&self) -> bool {
        matches!(self.stage, Stage::AwaitCapture { .. })
    }

    pub fn captured(&mut self) {
        if self.wants_capture() {
            self.stage = Stage::Ready { captured: true };
        }
    }

    /// Called once per update. `Some(captured)` means apply now.
    pub fn poll(&mut self) -> Option<bool> {
        match self.stage {
            Stage::Decoding => None,
            Stage::AwaitCapture { waited } if waited + 1 >= MAX_CAPTURE_WAIT => Some(false),
            Stage::AwaitCapture { waited } => {
                self.stage = Stage::AwaitCapture { waited: waited + 1 };
                None
            }
            Stage::Ready { captured } => Some(captured),
        }
    }
}

/// What a layer shows, as far as a Morph can carry it across a switch. Two
/// equal keys mean the incoming layer continues the outgoing one, so its
/// params can morph; a different key is a different picture, which only a
/// frame dissolve hides.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerKey {
    pub content: ContentKey,
    pub blend: BlendMode,
    /// The trama chain's node kinds and wire count. A chain is rebuilt from
    /// the preset on load, so a different shape is a different picture.
    pub chain: Option<(Vec<NodeKind>, usize)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContentKey {
    /// A kept effect keeps running (preset load skips reloading the same
    /// effect), so its params morph smoothly.
    Effect(Option<usize>),
    /// A media layer is rebuilt on load and restarts from frame 0: the frame
    /// index makes a restarted video differ while a still image, or a locked
    /// layer the load skipped, compares equal.
    Media(PathBuf, usize),
}

/// Whether a switch crossfades the frame. Dissolve always does. Morph does
/// only when some layer changes what it shows (#217): params cannot morph one
/// effect into another, and without the dissolve that switch would cut.
pub fn dissolves_frame(kind: TransitionType, before: &[LayerKey], after: &[LayerKey]) -> bool {
    match kind {
        TransitionType::Cut => false,
        TransitionType::Dissolve => true,
        TransitionType::ParamMorph => before != after,
    }
}

/// Stop the params of every changed layer from morphing: a param that two
/// different effects both happen to name means something else in each, and
/// lerping it would only add motion to a layer the dissolve is replacing.
pub fn keep_changed_layers_still(
    from: &mut MorphSnapshot,
    before: &[LayerKey],
    after: &[LayerKey],
) {
    for (i, params) in from.params.iter_mut().enumerate() {
        if before.get(i) != after.get(i) {
            params.clear();
        }
    }
}

/// The outgoing side of a switch, as far as keeping it moving is concerned.
#[derive(Debug, Clone, Copy, Default)]
pub struct Outgoing {
    /// Some layer, or the master, carries a trama chain. Chain slots are
    /// allocated per stack, so an outgoing chain would collide with the
    /// incoming preset's.
    pub has_chains: bool,
    /// A locked layer stays in the live stack and cannot also animate in the
    /// outgoing one without advancing twice a frame.
    pub has_locked: bool,
    /// A live fade is already running: its two pictures are captured as a
    /// still rather than nesting a third stack.
    pub fade_running_live: bool,
}

/// Whether a switch keeps the outgoing preset animating through its fade,
/// rather than fading from a still of it. Only a Dissolve does, and only
/// with "Keep moving" on: it renders both presets every frame of the fade.
/// Morph keeps its layers in place to slide their params, so there is no
/// outgoing stack to keep.
pub fn keeps_outgoing_moving(kind: TransitionType, keep_moving: bool, out: Outgoing) -> bool {
    kind == TransitionType::Dissolve
        && keep_moving
        && !out.has_chains
        && !out.has_locked
        && !out.fade_running_live
}

/// A switch in flight: drives the frame crossfade and the param morph.
pub struct ActiveTransition {
    duration: f32,
    elapsed: f32,
    /// Crossfade the captured frame into the live one.
    pub dissolve_frame: bool,
    morph: Option<(MorphSnapshot, MorphSnapshot)>,
}

impl ActiveTransition {
    pub fn new(
        duration: f32,
        dissolve_frame: bool,
        morph: Option<(MorphSnapshot, MorphSnapshot)>,
    ) -> Self {
        Self {
            duration: duration.max(0.0),
            elapsed: 0.0,
            dissolve_frame,
            morph,
        }
    }

    pub fn progress(&self) -> f32 {
        if self.duration > 0.0 {
            (self.elapsed / self.duration).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// Advance by `dt` seconds; true once the transition has finished.
    pub fn advance(&mut self, dt: f32) -> bool {
        self.elapsed += dt.max(0.0);
        self.elapsed >= self.duration
    }

    pub fn morph(&self) -> Option<(&MorphSnapshot, &MorphSnapshot)> {
        self.morph.as_ref().map(|(from, to)| (from, to))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::ParamValue;
    use std::collections::HashMap;

    fn effect(idx: usize) -> LayerKey {
        LayerKey {
            content: ContentKey::Effect(Some(idx)),
            blend: BlendMode::default(),
            chain: None,
        }
    }

    fn style(kind: TransitionType, secs: f32) -> TransitionStyle {
        TransitionStyle { kind, secs }
    }

    fn decode_result() -> PresetDecodeResult {
        PresetDecodeResult {
            preset_index: 0,
            preset: crate::preset::Preset {
                layers: vec![],
                active_layer: 0,
                postprocess: Default::default(),
                volumetric: None,
                master_chain: None,
            },
            decoded_media: HashMap::new(),
            generation: 1,
        }
    }

    #[test]
    fn a_cut_or_a_zero_length_transition_is_a_cut() {
        assert!(style(TransitionType::Cut, 2.0).is_cut());
        assert!(style(TransitionType::Dissolve, 0.0).is_cut());
        assert!(style(TransitionType::ParamMorph, f32::NAN).is_cut());
        assert!(!style(TransitionType::Dissolve, 0.5).is_cut());
    }

    #[test]
    fn a_switch_without_media_captures_then_applies() {
        let mut s = StagedSwitch::new(3, None, style(TransitionType::Dissolve, 1.0), false);
        assert!(s.wants_capture());
        s.captured();
        assert_eq!(s.poll(), Some(true));
    }

    /// The media case: nothing is captured, and nothing applies, until the
    /// decode lands. Capturing at the request is what faded a frozen frame
    /// into the old preset and then cut to the new one.
    #[test]
    fn a_decoding_switch_waits_for_the_decode_before_capturing() {
        let mut s = StagedSwitch::new(3, None, style(TransitionType::Dissolve, 1.0), true);
        for _ in 0..100 {
            assert!(!s.wants_capture());
            assert_eq!(s.poll(), None);
        }
        s.decode_landed(decode_result());
        assert!(s.wants_capture());
        assert!(s.decoded.is_some());
        s.captured();
        assert_eq!(s.poll(), Some(true));
    }

    #[test]
    fn a_capture_that_never_comes_applies_without_a_dissolve() {
        let mut s = StagedSwitch::new(3, None, style(TransitionType::Dissolve, 1.0), false);
        let mut polls = 0;
        let applied = loop {
            polls += 1;
            if let Some(captured) = s.poll() {
                break captured;
            }
            assert!(polls < 10, "a missed capture must not stall the switch");
        };
        assert!(!applied);
        assert_eq!(polls, MAX_CAPTURE_WAIT as usize);
    }

    #[test]
    fn dissolve_always_crossfades_and_cut_never_does() {
        let same = [effect(1), effect(2)];
        assert!(dissolves_frame(TransitionType::Dissolve, &same, &same));
        let other = [effect(1), effect(5)];
        assert!(!dissolves_frame(TransitionType::Cut, &same, &other));
    }

    /// The #217 rule: Morph is a pure param morph while every layer keeps its
    /// effect, and adds a frame dissolve as soon as any layer changes.
    #[test]
    fn morph_crossfades_only_when_a_layer_changes() {
        let before = [effect(1), effect(2)];
        assert!(!dissolves_frame(
            TransitionType::ParamMorph,
            &before,
            &before
        ));
        assert!(dissolves_frame(
            TransitionType::ParamMorph,
            &before,
            &[effect(1), effect(5)]
        ));
        assert!(dissolves_frame(
            TransitionType::ParamMorph,
            &before,
            &[effect(1)]
        ));
        let mut reblended = before.clone();
        reblended[1].blend = BlendMode::Add;
        assert!(dissolves_frame(
            TransitionType::ParamMorph,
            &before,
            &reblended
        ));
    }

    #[test]
    fn a_restarted_video_differs_but_a_still_image_does_not() {
        let at = |frame| LayerKey {
            content: ContentKey::Media(PathBuf::from("clip.mp4"), frame),
            blend: BlendMode::default(),
            chain: None,
        };
        assert_ne!(at(120), at(0));
        assert_eq!(at(0), at(0));
    }

    #[test]
    fn changed_layers_keep_still_while_kept_ones_morph() {
        let layer = |v: f32| HashMap::from([("speed".to_string(), ParamValue::Float(v))]);
        let mut from = MorphSnapshot {
            params: vec![layer(0.0), layer(0.0)],
            opacities: vec![1.0, 1.0],
        };
        keep_changed_layers_still(&mut from, &[effect(1), effect(2)], &[effect(1), effect(9)]);
        assert_eq!(from.params[0].len(), 1);
        assert!(from.params[1].is_empty());
    }

    #[test]
    fn only_a_dissolve_with_keep_moving_keeps_the_outgoing_preset_moving() {
        let clean = Outgoing::default();
        assert!(keeps_outgoing_moving(TransitionType::Dissolve, true, clean));
        assert!(!keeps_outgoing_moving(
            TransitionType::Dissolve,
            false,
            clean
        ));
        assert!(!keeps_outgoing_moving(
            TransitionType::ParamMorph,
            true,
            clean
        ));
        assert!(!keeps_outgoing_moving(TransitionType::Cut, true, clean));
    }

    #[test]
    fn chains_locks_and_a_running_live_fade_fall_back_to_the_still() {
        for out in [
            Outgoing {
                has_chains: true,
                ..Default::default()
            },
            Outgoing {
                has_locked: true,
                ..Default::default()
            },
            Outgoing {
                fade_running_live: true,
                ..Default::default()
            },
        ] {
            assert!(
                !keeps_outgoing_moving(TransitionType::Dissolve, true, out),
                "{out:?}"
            );
        }
    }

    #[test]
    fn progress_runs_zero_to_one_and_reports_the_end() {
        let mut t = ActiveTransition::new(2.0, true, None);
        assert_eq!(t.progress(), 0.0);
        assert!(!t.advance(1.0));
        assert!((t.progress() - 0.5).abs() < 1e-6);
        assert!(t.advance(1.5));
        assert_eq!(t.progress(), 1.0);
    }
}
