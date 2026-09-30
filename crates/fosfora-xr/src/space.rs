//! The space size control (board #3325): the half extent of the cube the
//! world effect's particles live in and respawn out of, around the anchor.
//! Plain numbers, so the bookkeeping builds and tests on the desktop;
//! `scene.rs` applies it to the particle system and `app.rs` drives it from
//! the debug panel's "space half m" stepper and `debug.fosfora.space`.
//!
//! The world sims read the volume from the core uniform `emitter_radius`,
//! which the particle system copies from its def every update, so setting
//! the def's `emitter.radius` resizes the volume live: a particle outside
//! a smaller cube respawns inside it on its next step, and the surface
//! weights ([`crate::surfaces::emitter_weights`]) are taken against the new
//! half the next frame. The near fade, the settle drift and the hand reach
//! do not depend on it. Murmur's 3D spatial hash spans the same cube with a
//! fixed cell count, and its speeds and predator reach are in half extents,
//! so a larger volume there means coarser cells and a faster, wider flock.
//!
//! **The room fit** (step 2h): for the surface-born effect (Embers, the
//! preset with `surface_emit`) the size follows the room until one is
//! asked, [`fit_room`]. At its preset's 1.5 m, recentered on the wearer,
//! a table 2 m away lay outside the cube: the editor assigned it, its
//! weight was 0, and it emitted nothing ("Table 13 worked for a
//! pinch-hold but not a single pinch", Kevin, worn, Sep 30). The other
//! effects keep their presets' sizes: the flock was tuned at 1.5 m.

use glam::{Quat, Vec3};

/// The stepper's range and step (meters).
pub const SPACE_HALF_MIN: f32 = 0.5;
pub const SPACE_HALF_MAX: f32 = 6.0;
pub const SPACE_HALF_STEP: f32 = 0.25;
/// The sims floor the half extent at this (`max(u.emitter_radius, 0.05)`).
pub const EMITTER_HALF_FLOOR: f32 = 0.05;

/// Room around the farthest room box's corner in a room fit (m).
pub const FIT_MARGIN_M: f32 = 0.25;

/// The space size that fits the room: the half extent of the smallest
/// cube around `anchor` that holds every corner of `boxes` (center, box
/// -> world rotation, half extents; the room's boxes, never the stage
/// floor, which is 20 m across), plus [`FIT_MARGIN_M`], rounded up to the
/// stepper's [`SPACE_HALF_STEP`] and clamped to its range, so the stepper
/// shows it and the wearer can step it from there. The rounding also holds
/// the size still while a drag moves the anchor a few centimeters. `None`
/// with no boxes: the preset's size stands.
pub fn fit_room(anchor: Vec3, boxes: impl IntoIterator<Item = (Vec3, Quat, Vec3)>) -> Option<f32> {
    let mut reach: Option<f32> = None;
    for (center, rot, half) in boxes {
        for i in 0..8 {
            let sign = Vec3::new(
                if i & 1 == 0 { -1.0 } else { 1.0 },
                if i & 2 == 0 { -1.0 } else { 1.0 },
                if i & 4 == 0 { -1.0 } else { 1.0 },
            );
            let d = (center + rot * (half * sign) - anchor).abs().max_element();
            reach = Some(reach.map_or(d, |r| r.max(d)));
        }
    }
    reach.map(|r| {
        // A thousandth of a step off the grid is on it: float noise must
        // not add a step.
        let steps = ((r + FIT_MARGIN_M) / SPACE_HALF_STEP - 1e-3).ceil();
        (steps * SPACE_HALF_STEP).clamp(SPACE_HALF_MIN, SPACE_HALF_MAX)
    })
}

/// The emitter radius a world effect runs at: `requested` (meters) when
/// above 0, else the preset's own.
pub fn space_radius(preset: f32, requested: f32) -> f32 {
    if requested > 0.0 { requested } else { preset }
}

/// The half extent the sim uses for `radius`, floored as the sim floors
/// it: what the surface weights are taken against.
pub fn emitter_half(radius: f32) -> f32 {
    radius.max(EMITTER_HALF_FLOOR)
}

/// The radius to write back into a particle system whose def holds
/// `def_radius` while its scene asked for `requested` (0 = the preset's):
/// `Some` when they differ, as after a rebuild from the preset.
pub fn reapply(def_radius: f32, preset: f32, requested: f32) -> Option<f32> {
    let want = space_radius(preset, requested);
    ((def_radius - want).abs() > 1e-6).then_some(want)
}

/// The app's side of the control. Until the knob or the stepper asks for a
/// size, each world effect keeps its preset's, or the room fit for the
/// surface-born one ([`fit_room`]), and the stepper shows it; once asked,
/// the size holds for every effect a pinch-hold swaps in, the room fit
/// too.
#[derive(Debug, Clone, Copy)]
pub struct SpaceControl {
    /// The half extent asked for (clamped to the stepper's range).
    asked: Option<f32>,
    /// The value last put on the stepper: a different one there means the
    /// wearer stepped it.
    shown: f32,
}

impl SpaceControl {
    /// `knob` (`debug.fosfora.space`; 0 or below is unset) overrides every
    /// preset; `preset` is the showing effect's radius, what the stepper
    /// shows without the knob.
    pub fn new(knob: Option<f32>, preset: f32) -> Self {
        let asked = knob
            .filter(|&v| v > 0.0)
            .map(|v| v.clamp(SPACE_HALF_MIN, SPACE_HALF_MAX));
        Self {
            asked,
            shown: asked.unwrap_or(preset),
        }
    }

    /// The stepper's value.
    pub fn shown(&self) -> f32 {
        self.shown
    }

    /// The size the knob or the stepper asked for, if any: it wins over
    /// the presets and the room fit.
    pub fn asked(&self) -> Option<f32> {
        self.asked
    }

    /// Once a frame, for the showing world effect: `panel` is the
    /// stepper's value, `requested` and `preset` the scene's (what it was
    /// last set for, 0 = its preset's, and its preset's radius), `fit` the
    /// room fit when the effect takes it ([`fit_room`]; `None` for the
    /// others and a room with no boxes). Returns the half to set on the
    /// scene when that differs (the stepper moved, the room fit changed,
    /// or a pinch-hold swapped in an effect set for another) and leaves
    /// `panel` showing the size the scene runs at.
    pub fn update(
        &mut self,
        panel: &mut f32,
        requested: f32,
        preset: f32,
        fit: Option<f32>,
    ) -> Option<f32> {
        if (*panel - self.shown).abs() > 1e-4 {
            self.asked = Some(panel.clamp(SPACE_HALF_MIN, SPACE_HALF_MAX));
        }
        let want = self.asked.or(fit).unwrap_or(0.0);
        let set = ((want - requested).abs() > 1e-6).then_some(want);
        *panel = space_radius(preset, want);
        self.shown = *panel;
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_a_knob_each_preset_keeps_its_own_until_the_stepper_moves() {
        let mut c = SpaceControl::new(None, 1.5);
        let mut panel = c.shown();
        assert_close!(panel, 1.5);
        // Embers at its preset's 1.5: nothing to set.
        assert_eq!(c.update(&mut panel, 0.0, 1.5, None), None);
        assert_close!(panel, 1.5);
        // A pinch-hold swaps in an effect with a 1 m preset: still its own.
        assert_eq!(c.update(&mut panel, 0.0, 1.0, None), None);
        assert_close!(panel, 1.0);
        // The wearer steps it: set, and shown.
        panel = 1.25;
        assert_eq!(c.update(&mut panel, 0.0, 1.0, None), Some(1.25));
        assert_close!(panel, 1.25);
        // The scene now holds it: nothing more to set.
        assert_eq!(c.update(&mut panel, 1.25, 1.0, None), None);
        assert_close!(panel, 1.25);
    }

    #[test]
    fn an_asked_size_holds_for_every_effect_swapped_in() {
        let mut c = SpaceControl::new(Some(3.0), 1.5);
        let mut panel = c.shown();
        assert_close!(panel, 3.0);
        assert_eq!(c.update(&mut panel, 0.0, 1.5, None), Some(3.0));
        assert_eq!(c.update(&mut panel, 3.0, 1.5, None), None);
        // Swapped in, still at its preset: set to the asked size.
        assert_eq!(c.update(&mut panel, 0.0, 1.0, None), Some(3.0));
        // Swapped back in, set for an older step: set again.
        panel = 2.75;
        assert_eq!(c.update(&mut panel, 3.0, 1.5, None), Some(2.75));
        assert_close!(panel, 2.75);
    }

    #[test]
    fn the_knob_is_clamped_to_the_stepper_range_and_zero_is_unset() {
        assert_close!(SpaceControl::new(Some(9.0), 1.5).shown(), SPACE_HALF_MAX);
        assert_close!(SpaceControl::new(Some(0.1), 1.5).shown(), SPACE_HALF_MIN);
        assert_close!(SpaceControl::new(Some(0.0), 1.5).shown(), 1.5);
        let mut c = SpaceControl::new(Some(0.0), 1.5);
        let mut panel = c.shown();
        assert_eq!(c.update(&mut panel, 0.0, 1.5, None), None);
    }

    /// A box with no rotation: center and half extents.
    fn aligned(center: [f32; 3], half: [f32; 3]) -> (Vec3, Quat, Vec3) {
        (Vec3::from(center), Quat::IDENTITY, Vec3::from(half))
    }

    #[test]
    fn the_room_fit_holds_every_box_corner_around_an_off_center_anchor() {
        // The anchor recentered on a wearer off the room's center.
        let anchor = Vec3::new(0.5, 1.0, -0.3);
        let boxes = [
            // A desk: its far corner 1.4 m out in x from the anchor.
            aligned([1.5, 0.37, -0.8], [0.4, 0.37, 0.3]),
            // A table whose far edge is 2.4 m out in -z.
            aligned([0.0, 0.37, -2.5], [0.6, 0.37, 0.2]),
            // The ceiling, 2.52 m up: 1.52 m above the anchor.
            aligned([0.5, 2.5, -0.3], [1.0, 0.02, 1.0]),
        ];
        // 2.4 + 0.25 = 2.65, up to the stepper's 2.75.
        assert_close!(fit_room(anchor, boxes).unwrap(), 2.75);
        // Rotated boxes count by their corners: the table turned 90° about
        // y reaches 0.4 m farther in z (2.8 m).
        let turned = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let (c, _, h) = boxes[1];
        let fit = fit_room(anchor, [boxes[0], (c, turned, h), boxes[2]]).unwrap();
        // 2.8 + 0.25 = 3.05, up to 3.25.
        assert_close!(fit, 3.25);
    }

    #[test]
    fn the_room_fit_has_its_margin_and_its_clamp_and_none_without_boxes() {
        let anchor = Vec3::new(0.0, 1.0, 0.0);
        // A corner exactly 3 m out: 3.25, not 3.5.
        assert_close!(
            fit_room(anchor, [aligned([2.5, 1.0, 0.0], [0.5, 0.5, 0.5])]).unwrap(),
            3.0 + FIT_MARGIN_M
        );
        // A hair past it: the next step.
        assert_close!(
            fit_room(anchor, [aligned([2.55, 1.0, 0.0], [0.5, 0.5, 0.5])]).unwrap(),
            3.5
        );
        // A small box at the anchor: the stepper's floor.
        assert_close!(
            fit_room(anchor, [aligned([0.0, 1.0, 0.0], [0.05, 0.05, 0.05])]).unwrap(),
            SPACE_HALF_MIN
        );
        // A hall 20 m long: the stepper's ceiling.
        assert_close!(
            fit_room(anchor, [aligned([0.0, 1.0, -10.0], [2.0, 1.5, 10.0])]).unwrap(),
            SPACE_HALF_MAX
        );
        assert_eq!(fit_room(anchor, []), None);
    }

    #[test]
    fn the_room_fit_is_shown_and_applied_until_a_size_is_asked() {
        let mut c = SpaceControl::new(None, 1.5);
        let mut panel = c.shown();
        // Embers, the room not in yet: its preset.
        assert_eq!(c.update(&mut panel, 0.0, 1.5, None), None);
        assert_close!(panel, 1.5);
        // The room arrives: the fit is set and shown, and is not an ask.
        assert_eq!(c.update(&mut panel, 0.0, 1.5, Some(3.75)), Some(3.75));
        assert_close!(panel, 3.75);
        assert_eq!(c.update(&mut panel, 3.75, 1.5, Some(3.75)), None);
        assert_eq!(c.asked(), None);
        // The anchor moves: the new fit.
        assert_eq!(c.update(&mut panel, 3.75, 1.5, Some(3.0)), Some(3.0));
        // A pinch-hold swaps in the flock (no fit): its preset.
        assert_eq!(c.update(&mut panel, 0.0, 1.5, None), None);
        assert_close!(panel, 1.5);
        // Back on Embers, set for the old fit: the fit again.
        assert_eq!(c.update(&mut panel, 3.0, 1.5, Some(3.0)), None);
        assert_close!(panel, 3.0);
        // The wearer steps it down: the asked size wins over the fit.
        panel = 2.75;
        assert_eq!(c.update(&mut panel, 3.0, 1.5, Some(3.0)), Some(2.75));
        assert_eq!(c.asked(), Some(2.75));
        assert_eq!(c.update(&mut panel, 2.75, 1.5, Some(4.0)), None);
        assert_close!(panel, 2.75);
        // The knob's size is not overridden either.
        let mut c = SpaceControl::new(Some(2.0), 1.5);
        let mut panel = c.shown();
        assert_eq!(c.update(&mut panel, 0.0, 1.5, Some(3.75)), Some(2.0));
        assert_close!(panel, 2.0);
    }

    #[test]
    fn the_stored_half_is_reapplied_after_a_rebuild() {
        // A rebuild from the preset puts the def back at 1.5 while the
        // scene asked for 3: write 3 back.
        assert_eq!(reapply(1.5, 1.5, 3.0), Some(3.0));
        // Already there, or asking for the preset's: nothing.
        assert_eq!(reapply(3.0, 1.5, 3.0), None);
        assert_eq!(reapply(1.5, 1.5, 0.0), None);
        // Back to the preset's from a set size.
        assert_eq!(reapply(3.0, 1.5, 0.0), Some(1.5));
    }

    #[test]
    fn the_emitter_half_is_floored_as_the_sim_floors_it() {
        assert_close!(emitter_half(space_radius(1.5, 0.0)), 1.5);
        assert_close!(emitter_half(space_radius(1.5, 0.75)), 0.75);
        assert_close!(emitter_half(space_radius(0.0, 0.0)), EMITTER_HALF_FLOOR);
    }
}
