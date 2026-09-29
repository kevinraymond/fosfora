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

/// The stepper's range and step (meters).
pub const SPACE_HALF_MIN: f32 = 0.5;
pub const SPACE_HALF_MAX: f32 = 6.0;
pub const SPACE_HALF_STEP: f32 = 0.25;
/// The sims floor the half extent at this (`max(u.emitter_radius, 0.05)`).
pub const EMITTER_HALF_FLOOR: f32 = 0.05;

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
/// size, each world effect keeps its preset's and the stepper shows it;
/// once asked, the size holds for every effect a pinch-hold swaps in.
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

    /// Once a frame, for the showing world effect: `panel` is the
    /// stepper's value, `requested` and `preset` the scene's (what it was
    /// last set for, 0 = its preset's, and its preset's radius). Returns
    /// the half to set on the scene when that differs (the stepper moved,
    /// or a pinch-hold swapped in an effect set for another) and leaves
    /// `panel` showing the size the scene runs at.
    pub fn update(&mut self, panel: &mut f32, requested: f32, preset: f32) -> Option<f32> {
        if (*panel - self.shown).abs() > 1e-4 {
            self.asked = Some(panel.clamp(SPACE_HALF_MIN, SPACE_HALF_MAX));
        }
        let want = self.asked.unwrap_or(0.0);
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
        // Flux XR Room at its preset's 1.5: nothing to set.
        assert_eq!(c.update(&mut panel, 0.0, 1.5), None);
        assert_close!(panel, 1.5);
        // A pinch-hold swaps in an effect with a 1 m preset: still its own.
        assert_eq!(c.update(&mut panel, 0.0, 1.0), None);
        assert_close!(panel, 1.0);
        // The wearer steps it: set, and shown.
        panel = 1.25;
        assert_eq!(c.update(&mut panel, 0.0, 1.0), Some(1.25));
        assert_close!(panel, 1.25);
        // The scene now holds it: nothing more to set.
        assert_eq!(c.update(&mut panel, 1.25, 1.0), None);
        assert_close!(panel, 1.25);
    }

    #[test]
    fn an_asked_size_holds_for_every_effect_swapped_in() {
        let mut c = SpaceControl::new(Some(3.0), 1.5);
        let mut panel = c.shown();
        assert_close!(panel, 3.0);
        assert_eq!(c.update(&mut panel, 0.0, 1.5), Some(3.0));
        assert_eq!(c.update(&mut panel, 3.0, 1.5), None);
        // Swapped in, still at its preset: set to the asked size.
        assert_eq!(c.update(&mut panel, 0.0, 1.0), Some(3.0));
        // Swapped back in, set for an older step: set again.
        panel = 2.75;
        assert_eq!(c.update(&mut panel, 3.0, 1.5), Some(2.75));
        assert_close!(panel, 2.75);
    }

    #[test]
    fn the_knob_is_clamped_to_the_stepper_range_and_zero_is_unset() {
        assert_close!(SpaceControl::new(Some(9.0), 1.5).shown(), SPACE_HALF_MAX);
        assert_close!(SpaceControl::new(Some(0.1), 1.5).shown(), SPACE_HALF_MIN);
        assert_close!(SpaceControl::new(Some(0.0), 1.5).shown(), 1.5);
        let mut c = SpaceControl::new(Some(0.0), 1.5);
        let mut panel = c.shown();
        assert_eq!(c.update(&mut panel, 0.0, 1.5), None);
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
