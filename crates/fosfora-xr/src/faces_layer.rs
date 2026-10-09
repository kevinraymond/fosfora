//! The surfaces in their own composition layer (board #3793,
//! `docs/xr/SURFACES_DESIGN.md`, D2c): the lit room faces render into a
//! second, smaller per-eye swapchain, submitted as a projection layer
//! between passthrough and the main projection layer, so the runtime
//! upsamples them at composition instead of the app shading every covered
//! eye pixel. Plain numbers, so the sizing and the layer list build and
//! test on the desktop; `xr.rs` creates and submits the layer, `gfx.rs`
//! renders into it.

/// The faces swapchain's size against the runtime's recommended eye size
/// when `debug.fosfora.facescale` is unset.
pub const FACE_SCALE_DEFAULT: f32 = 0.5;
/// The range `debug.fosfora.facescale` is clamped to.
pub const FACE_SCALE_MIN: f32 = 0.25;
pub const FACE_SCALE_MAX: f32 = 1.0;
/// The smallest side any eye swapchain is created with.
pub const MIN_SIDE: u32 = 64;

/// The faces layer's knobs, read at launch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FacesOptions {
    /// `debug.fosfora.faceslayer 0|1`: the surfaces in their own layer
    /// (1, the default in world and mr) or back in the eye pass (0).
    pub layer: bool,
    /// `debug.fosfora.facescale`: the faces swapchain against the
    /// recommended size, [`FACE_SCALE_MIN`]..[`FACE_SCALE_MAX`].
    pub scale: f32,
    /// `debug.fosfora.facesharpen 0|1`: `XR_FB_composition_layer_settings`
    /// normal sharpening on the faces layer's upsample (default 0).
    pub sharpen: bool,
}

/// Off: the surfaces in the eye pass, as without passthrough.
impl Default for FacesOptions {
    fn default() -> Self {
        Self {
            layer: false,
            scale: FACE_SCALE_DEFAULT,
            sharpen: false,
        }
    }
}

impl FacesOptions {
    /// The options from the knobs' raw values (`None`: unset), with
    /// `layer_default` the mode's default for `faceslayer`.
    pub fn from_knobs(
        layer: Option<&str>,
        scale: Option<&str>,
        sharpen: Option<&str>,
        layer_default: bool,
    ) -> Self {
        let toggle = |v: Option<&str>, default: bool| match v.map(str::trim) {
            Some("0") => false,
            Some("1") => true,
            _ => default,
        };
        Self {
            layer: toggle(layer, layer_default),
            scale: face_scale(scale.and_then(|s| s.trim().parse::<f32>().ok())),
            sharpen: toggle(sharpen, false),
        }
    }
}

/// The face scale for a knob value: [`FACE_SCALE_DEFAULT`] when unset or
/// not a number, else clamped to [`FACE_SCALE_MIN`]..[`FACE_SCALE_MAX`].
pub fn face_scale(knob: Option<f32>) -> f32 {
    knob.filter(|s| s.is_finite())
        .map_or(FACE_SCALE_DEFAULT, |s| {
            s.clamp(FACE_SCALE_MIN, FACE_SCALE_MAX)
        })
}

/// A swapchain's size for the runtime's `recommended` size scaled by
/// `scale`: each side rounded, at least [`MIN_SIDE`] and at most `max` (the
/// runtime's largest image rect side).
pub fn scaled_extent(recommended: [u32; 2], scale: f32, max: u32) -> [u32; 2] {
    recommended.map(|v| ((v as f32 * scale).round() as u32).clamp(MIN_SIDE, max.max(MIN_SIDE)))
}

/// The faces swapchain's size: [`scaled_extent`] at the clamped `scale`
/// ([`face_scale`]).
pub fn faces_extent(recommended: [u32; 2], scale: f32, max: u32) -> [u32; 2] {
    scaled_extent(recommended, face_scale(Some(scale)), max)
}

/// Whether the faces layer exists this session: only over passthrough
/// (without it the projection layer is opaque and the surfaces stay in the
/// eye pass, the quad and particles test modes) and with the knob on.
pub fn faces_layer_on(passthrough: bool, knob: bool) -> bool {
    passthrough && knob
}

/// One composition layer of a frame, in submission order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// `XR_FB_passthrough`'s reconstruction layer, the camera image.
    Passthrough,
    /// The surfaces' projection layer, premultiplied over passthrough.
    Faces,
    /// The main projection layer: the cloud, the panel, the labels.
    Main,
}

/// The layers `xrEndFrame` submits, bottom first: passthrough (when on),
/// the faces layer (when it exists, which needs passthrough), the main
/// projection layer.
pub fn layer_order(passthrough: bool, faces: bool) -> Vec<Layer> {
    let mut layers = Vec::with_capacity(3);
    if passthrough {
        layers.push(Layer::Passthrough);
        if faces {
            layers.push(Layer::Faces);
        }
    }
    layers.push(Layer::Main);
    layers
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Quest 3's recommended eye size and its largest image rect side.
    const REC: [u32; 2] = [1680, 1760];
    const MAX: u32 = 4096;

    #[test]
    fn faces_extent_halves_the_recommended_size_by_default() {
        assert_eq!(faces_extent(REC, FACE_SCALE_DEFAULT, MAX), [840, 880]);
        assert_eq!(faces_extent(REC, 1.0, MAX), REC);
        assert_eq!(faces_extent(REC, 0.25, MAX), [420, 440]);
        // Rounded, not truncated.
        assert_eq!(faces_extent([1001, 999], 0.5, MAX), [501, 500]);
    }

    #[test]
    fn faces_extent_clamps_the_scale_and_the_sides() {
        assert_eq!(faces_extent(REC, 0.1, MAX), [420, 440]);
        assert_eq!(faces_extent(REC, 3.0, MAX), REC);
        assert_eq!(faces_extent(REC, f32::NAN, MAX), [840, 880]);
        assert_eq!(faces_extent([100, 100], 0.25, MAX), [MIN_SIDE, MIN_SIDE]);
        assert_eq!(faces_extent(REC, 1.0, 1000), [1000, 1000]);
    }

    #[test]
    fn eye_extent_keeps_the_eye_scale_range() {
        // The eye swapchain's own knob goes past 1.0; only the max caps it.
        assert_eq!(scaled_extent(REC, 1.5, MAX), [2520, 2640]);
        assert_eq!(scaled_extent(REC, 1.0, MAX), REC);
    }

    #[test]
    fn face_scale_knob() {
        assert_close!(face_scale(None), FACE_SCALE_DEFAULT);
        assert_close!(face_scale(Some(0.75)), 0.75);
        assert_close!(face_scale(Some(0.0)), FACE_SCALE_MIN);
        assert_close!(face_scale(Some(2.0)), FACE_SCALE_MAX);
        assert_close!(face_scale(Some(f32::INFINITY)), FACE_SCALE_DEFAULT);
    }

    #[test]
    fn options_from_knobs() {
        let o = FacesOptions::from_knobs(None, None, None, true);
        assert_eq!(
            o,
            FacesOptions {
                layer: true,
                scale: FACE_SCALE_DEFAULT,
                sharpen: false
            }
        );
        let o = FacesOptions::from_knobs(Some("0"), Some("0.3"), Some("1"), true);
        assert_eq!(
            o,
            FacesOptions {
                layer: false,
                scale: 0.3,
                sharpen: true
            }
        );
        // The quad and particles modes default off; the knob turns it on.
        assert!(!FacesOptions::from_knobs(None, None, None, false).layer);
        assert!(FacesOptions::from_knobs(Some("1"), Some("x"), None, false).layer);
    }

    #[test]
    fn layer_order_with_passthrough_and_the_knob() {
        use Layer::{Faces, Main, Passthrough};
        let order = |pt: bool, knob: bool| layer_order(pt, faces_layer_on(pt, knob));
        assert_eq!(order(true, true), [Passthrough, Faces, Main]);
        assert_eq!(order(true, false), [Passthrough, Main]);
        // Without passthrough there is no faces layer, knob or not.
        assert_eq!(order(false, true), [Main]);
        assert_eq!(order(false, false), [Main]);
        assert!(!faces_layer_on(false, true));
    }
}
