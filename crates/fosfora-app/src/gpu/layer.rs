use serde::{Deserialize, Serialize};

use crate::effect::format::PostProcessDef;
use crate::gpu::ShaderUniforms;
use crate::gpu::audio_textures::AudioTextures;
use crate::gpu::pass_executor::PassExecutor;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::render_target::RenderTarget;
use crate::gpu::uniforms::UniformBuffer;
use crate::media::MediaLayer;
use crate::params::ParamStore;

/// Default warp strength for the displacement blend modes (#1478).
/// Normalized 0..1, like opacity — the shader scales it to a bounded UV offset.
pub const DEFAULT_DISPLACE_AMOUNT: f32 = 0.35;

/// Blend mode for compositing layers.
///
/// Two families. The first ten are *color* blends: arithmetic on the foreground
/// and background colors at the same pixel. The last three are *displacement*
/// blends (#1478) — the foreground is read as a warp field rather than as an
/// image, and its luminance offsets the UV used to sample everything beneath.
/// A displacing layer draws none of its own color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Add,
    Screen,
    ColorDodge,
    Multiply,
    #[serde(alias = "SoftLight")]
    Overlay,
    HardLight,
    Difference,
    Exclusion,
    Subtract,
    Displace,
    Refract,
    Lens,
}

impl BlendMode {
    /// The color-blend family: arithmetic on fg and bg at the same pixel.
    ///
    /// Kept as its own list because `from_normalized` maps over it — see there.
    pub const COLOR: &[BlendMode] = &[
        BlendMode::Normal,
        BlendMode::Add,
        BlendMode::Screen,
        BlendMode::ColorDodge,
        BlendMode::Multiply,
        BlendMode::Overlay,
        BlendMode::HardLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Subtract,
    ];

    /// The displacement family (#1478): fg luminance warps what is beneath.
    pub const DISPLACEMENT: &[BlendMode] =
        &[BlendMode::Displace, BlendMode::Refract, BlendMode::Lens];

    pub const ALL: &[BlendMode] = &[
        BlendMode::Normal,
        BlendMode::Add,
        BlendMode::Screen,
        BlendMode::ColorDodge,
        BlendMode::Multiply,
        BlendMode::Overlay,
        BlendMode::HardLight,
        BlendMode::Difference,
        BlendMode::Exclusion,
        BlendMode::Subtract,
        BlendMode::Displace,
        BlendMode::Refract,
        BlendMode::Lens,
    ];

    pub fn as_u32(&self) -> u32 {
        match self {
            BlendMode::Normal => 0,
            BlendMode::Add => 1,
            BlendMode::Screen => 2,
            BlendMode::ColorDodge => 3,
            BlendMode::Multiply => 4,
            BlendMode::Overlay => 5,
            BlendMode::HardLight => 6,
            BlendMode::Difference => 7,
            BlendMode::Exclusion => 8,
            BlendMode::Subtract => 9,
            BlendMode::Displace => 10,
            BlendMode::Refract => 11,
            BlendMode::Lens => 12,
        }
    }

    pub fn from_u32(value: u32) -> Self {
        match value {
            0 => BlendMode::Normal,
            1 => BlendMode::Add,
            2 => BlendMode::Screen,
            3 => BlendMode::ColorDodge,
            4 => BlendMode::Multiply,
            5 => BlendMode::Overlay,
            6 => BlendMode::HardLight,
            7 => BlendMode::Difference,
            8 => BlendMode::Exclusion,
            9 => BlendMode::Subtract,
            10 => BlendMode::Displace,
            11 => BlendMode::Refract,
            12 => BlendMode::Lens,
            _ => BlendMode::Normal,
        }
    }

    /// Does this mode read the foreground as a warp field instead of an image?
    pub fn is_displacement(&self) -> bool {
        matches!(
            self,
            BlendMode::Displace | BlendMode::Refract | BlendMode::Lens
        )
    }

    /// Map a normalized 0..1 control value (e.g. a binding-bus output) onto
    /// the color-blend list: 0.0 → Normal, 1.0 → Subtract, evenly spaced
    /// in between (#1792). Out-of-range input clamps; NaN falls back to Normal.
    ///
    /// Deliberately maps over `COLOR`, not `ALL` (#1478). Two reasons: adding
    /// the displacement modes to the sweep would silently move every shipped
    /// preset that drives blend from the bus onto a different mode, and the
    /// displacement family is a structural break in the sweep anyway — those
    /// modes ignore fg color entirely, so passing through them mid-set reads as
    /// a glitch rather than a transition. Reach them from the UI, OSC or a preset.
    pub fn from_normalized(v: f32) -> Self {
        let max_index = (Self::COLOR.len() - 1) as f32;
        Self::from_u32((v.clamp(0.0, 1.0) * max_index).round() as u32)
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Add => "Add",
            BlendMode::Screen => "Screen",
            BlendMode::ColorDodge => "Color Dodge",
            BlendMode::Multiply => "Multiply",
            BlendMode::Overlay => "Overlay",
            BlendMode::HardLight => "Hard Light",
            BlendMode::Difference => "Difference",
            BlendMode::Exclusion => "Exclusion",
            BlendMode::Subtract => "Subtract",
            BlendMode::Displace => "Displace",
            BlendMode::Refract => "Refract",
            BlendMode::Lens => "Lens",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            BlendMode::Normal => "Replaces background with foreground",
            BlendMode::Add => "Brightens — adds colors together (glow, fire)",
            BlendMode::Screen => "Lightens — like projecting two slides together",
            BlendMode::ColorDodge => "Intense brighten — burns through to white",
            BlendMode::Multiply => "Darkens — like stacking two transparencies",
            BlendMode::Overlay => "Contrast boost — darks darker, lights lighter",
            BlendMode::HardLight => "Strong contrast — like Overlay from the other side",
            BlendMode::Difference => "Inverts where bright — psychedelic color shifts",
            BlendMode::Exclusion => "Softer Difference — grays out similar colors",
            BlendMode::Subtract => "Darkens — removes foreground color from background",
            BlendMode::Displace => "Edges shove what's beneath — shockwaves, heat haze",
            BlendMode::Refract => "Bright areas bend like thick glass, splitting color",
            BlendMode::Lens => "Bright areas magnify what's beneath — a breathing zoom",
        }
    }
}

/// Effect-specific layer data: shader pipeline, uniforms, hot-reload state.
pub struct EffectLayer {
    pub pass_executor: PassExecutor,
    pub uniform_buffer: UniformBuffer,
    pub uniforms: ShaderUniforms,
    pub effect_index: Option<usize>,
    pub shader_sources: Vec<String>,
    pub shader_error: Option<String>,
    /// Set when a load failed and left `effect_index` pointing at an effect this
    /// layer's GPU state is not actually running (#1855). While it is set, shader
    /// hot-reload must do a full atomic rebuild rather than swapping a pipeline
    /// into the previous effect's executor — the bind-group layouts do not match,
    /// so every attempt fails and the failure repeats on every file change.
    pub pending_rebuild: bool,
}

/// Content type for a layer.
pub enum LayerContent {
    Effect(Box<EffectLayer>),
    Media(Box<MediaLayer>),
}

/// A single compositing layer. Owns its own rendering pipeline and parameters.
pub struct Layer {
    pub name: String,
    pub custom_name: Option<String>,
    pub param_store: ParamStore,
    pub content: LayerContent,
    pub blend_mode: BlendMode,
    pub opacity: f32,
    /// Warp strength for the displacement blend modes (#1478). Ignored by the
    /// color blends, so it survives a round-trip through them unchanged.
    pub displace_amount: f32,
    pub enabled: bool,
    pub locked: bool,
    pub pinned: bool,
    pub postprocess: PostProcessDef,
}

impl Layer {
    /// Create a new Effect layer.
    pub fn new_effect(name: String, effect: EffectLayer, param_store: ParamStore) -> Self {
        Self {
            name,
            custom_name: None,
            param_store,
            content: LayerContent::Effect(Box::new(effect)),
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
            displace_amount: DEFAULT_DISPLACE_AMOUNT,
            enabled: true,
            locked: false,
            pinned: false,
            postprocess: PostProcessDef::default(),
        }
    }

    /// Create a new Media layer.
    pub fn new_media(name: String, media: MediaLayer) -> Self {
        Self {
            name,
            custom_name: None,
            param_store: ParamStore::new(),
            content: LayerContent::Media(Box::new(media)),
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
            displace_amount: DEFAULT_DISPLACE_AMOUNT,
            enabled: true,
            locked: false,
            pinned: false,
            postprocess: PostProcessDef::default(),
        }
    }

    /// Get the effect content, if this is an Effect layer.
    pub fn as_effect(&self) -> Option<&EffectLayer> {
        match &self.content {
            LayerContent::Effect(e) => Some(e),
            _ => None,
        }
    }

    /// Get mutable effect content, if this is an Effect layer.
    pub fn as_effect_mut(&mut self) -> Option<&mut EffectLayer> {
        match &mut self.content {
            LayerContent::Effect(e) => Some(e),
            _ => None,
        }
    }

    /// Does this layer's effect sample `@backdrop` (#2061)? Media layers never do.
    pub fn wants_backdrop(&self) -> bool {
        self.as_effect()
            .is_some_and(|e| e.pass_executor.wants_backdrop())
    }

    /// Get the media content, if this is a Media layer.
    pub fn as_media(&self) -> Option<&MediaLayer> {
        match &self.content {
            LayerContent::Media(m) => Some(m),
            _ => None,
        }
    }

    /// Get mutable media content, if this is a Media layer.
    pub fn as_media_mut(&mut self) -> Option<&mut MediaLayer> {
        match &mut self.content {
            LayerContent::Media(m) => Some(m),
            _ => None,
        }
    }

    /// Check if this is a media layer.
    pub fn is_media(&self) -> bool {
        matches!(&self.content, LayerContent::Media(_))
    }

    /// Get effect_index (None for non-effect layers).
    pub fn effect_index(&self) -> Option<usize> {
        self.as_effect().and_then(|e| e.effect_index)
    }

    /// Get shader error string, if any.
    pub fn shader_error(&self) -> Option<&str> {
        self.as_effect().and_then(|e| e.shader_error.as_deref())
    }

    /// Check if this layer has an active particle system.
    pub fn has_particles(&self) -> bool {
        self.as_effect()
            .map_or(false, |e| e.pass_executor.particle_system.is_some())
    }

    /// Execute this layer's render passes. Returns the final HDR target.
    pub fn execute(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
    ) -> &RenderTarget {
        match &self.content {
            LayerContent::Effect(e) => {
                e.pass_executor
                    .execute(encoder, &e.uniform_buffer, queue, &e.uniforms)
            }
            LayerContent::Media(m) => m.execute(encoder),
        }
    }

    /// Flip ping-pong targets for next frame.
    pub fn flip(&mut self) {
        match &mut self.content {
            LayerContent::Effect(e) => e.pass_executor.flip(),
            LayerContent::Media(_) => {} // no ping-pong for media
        }
    }

    /// Resize all render targets.
    pub fn resize(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        placeholder: &PlaceholderTexture,
        audio: &AudioTextures,
    ) {
        match &mut self.content {
            LayerContent::Effect(e) => {
                e.pass_executor.resize(
                    device,
                    queue,
                    width,
                    height,
                    &e.uniform_buffer,
                    placeholder,
                    audio,
                );
            }
            LayerContent::Media(_) => {
                // Media resize handled separately (needs queue for uniform upload)
            }
        }
    }

    /// Resize media layer (needs queue for uniform upload).
    pub fn resize_media(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
    ) {
        if let LayerContent::Media(ref mut m) = self.content {
            m.resize(device, queue, width, height);
        }
    }
}

/// Lightweight snapshot of layer state for UI rendering (avoids borrow conflicts).
#[derive(Debug, Clone)]
pub struct LayerInfo {
    pub name: String,
    pub custom_name: Option<String>,
    pub effect_index: Option<usize>,
    pub effect_name: Option<String>,
    pub blend_mode: BlendMode,
    pub opacity: f32,
    pub displace_amount: f32,
    pub enabled: bool,
    pub locked: bool,
    pub pinned: bool,
    #[allow(dead_code)]
    pub has_particles: bool,
    #[allow(dead_code)]
    pub shader_error: Option<String>,
    pub is_media: bool,
    pub media_file_name: Option<String>,
    #[allow(dead_code)]
    pub media_is_animated: bool,
    #[allow(dead_code)]
    pub media_is_video: bool,
    pub media_is_live: bool,
}

/// Manages an ordered stack of layers.
pub struct LayerStack {
    pub layers: Vec<Layer>,
    pub active_layer: usize,
}

impl LayerStack {
    pub fn new() -> Self {
        Self {
            layers: Vec::new(),
            active_layer: 0,
        }
    }

    /// Remove a layer by index. Adjusts active_layer if needed.
    pub fn remove_layer(&mut self, index: usize) {
        if self.layers.len() <= 1 || index >= self.layers.len() {
            return; // never remove the last layer
        }
        self.layers.remove(index);
        self.active_layer =
            adjusted_active_after_remove(self.active_layer, index, self.layers.len());
    }

    /// Move a layer from `from` to `to` position.
    pub fn move_layer(&mut self, from: usize, to: usize) {
        if from >= self.layers.len() || to >= self.layers.len() || from == to {
            return;
        }
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
        self.active_layer = adjusted_active_after_move(self.active_layer, from, to);
    }

    pub fn active(&self) -> Option<&Layer> {
        self.layers.get(self.active_layer)
    }

    pub fn active_mut(&mut self) -> Option<&mut Layer> {
        self.layers.get_mut(self.active_layer)
    }

    /// Collect lightweight snapshots for UI.
    pub fn layer_infos(&self, effects: &[crate::effect::format::PfxEffect]) -> Vec<LayerInfo> {
        self.layers
            .iter()
            .map(|l| {
                let (is_media, media_file_name, media_is_animated, media_is_video, media_is_live) =
                    match &l.content {
                        LayerContent::Media(m) => (
                            true,
                            Some(m.file_name.clone()),
                            m.is_animated(),
                            m.is_video(),
                            m.is_live(),
                        ),
                        _ => (false, None, false, false, false),
                    };
                LayerInfo {
                    name: l.name.clone(),
                    custom_name: l.custom_name.clone(),
                    effect_index: l.effect_index(),
                    effect_name: l
                        .effect_index()
                        .and_then(|i| effects.get(i))
                        .map(|e| e.name.clone()),
                    blend_mode: l.blend_mode,
                    opacity: l.opacity,
                    displace_amount: l.displace_amount,
                    enabled: l.enabled,
                    locked: l.locked,
                    pinned: l.pinned,
                    has_particles: l.has_particles(),
                    shader_error: l.shader_error().map(|s| s.to_string()),
                    is_media,
                    media_file_name,
                    media_is_animated,
                    media_is_video,
                    media_is_live,
                }
            })
            .collect()
    }

    /// Number of enabled layers.
    #[allow(dead_code)]
    pub fn enabled_count(&self) -> usize {
        self.layers.iter().filter(|l| l.enabled).count()
    }
}

/// Compute adjusted active layer index after removing a layer.
pub(crate) fn adjusted_active_after_remove(
    active: usize,
    _removed: usize,
    new_len: usize,
) -> usize {
    if active >= new_len {
        new_len.saturating_sub(1)
    } else {
        active
    }
}

/// Compute adjusted active layer index after moving a layer from `from` to `to`.
pub fn adjusted_active_after_move(active: usize, from: usize, to: usize) -> usize {
    if active == from {
        to
    } else if from < to && active > from && active <= to {
        active - 1
    } else if from > to && active >= to && active < from {
        active + 1
    } else {
        active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_mode_all_count() {
        assert_eq!(BlendMode::ALL.len(), 13);
    }

    #[test]
    fn blend_mode_families_partition_all() {
        // ALL must stay COLOR ++ DISPLACEMENT, in that order: `as_u32` is the
        // wire format for OSC, the web page and presets, so the color modes
        // have to keep indices 0..9 and the warp modes have to follow them.
        let joined: Vec<BlendMode> = BlendMode::COLOR
            .iter()
            .chain(BlendMode::DISPLACEMENT)
            .copied()
            .collect();
        assert_eq!(joined, BlendMode::ALL);
        assert_eq!(BlendMode::COLOR.len(), 10);
        assert_eq!(BlendMode::DISPLACEMENT.len(), 3);
    }

    #[test]
    fn blend_mode_is_displacement_matches_the_family_list() {
        for mode in BlendMode::COLOR {
            assert!(!mode.is_displacement(), "{mode:?} is a color blend");
        }
        for mode in BlendMode::DISPLACEMENT {
            assert!(mode.is_displacement(), "{mode:?} is a warp");
        }
    }

    #[test]
    fn blend_mode_as_u32() {
        for (i, mode) in BlendMode::ALL.iter().enumerate() {
            assert_eq!(mode.as_u32(), i as u32);
        }
    }

    #[test]
    fn blend_mode_display_names_non_empty() {
        for mode in BlendMode::ALL {
            assert!(!mode.display_name().is_empty());
        }
    }

    #[test]
    fn blend_mode_default_is_normal() {
        assert_eq!(BlendMode::default(), BlendMode::Normal);
    }

    #[test]
    fn blend_mode_serde_roundtrip() {
        for mode in BlendMode::ALL {
            let json = serde_json::to_string(mode).unwrap();
            let m2: BlendMode = serde_json::from_str(&json).unwrap();
            assert_eq!(*mode, m2);
        }
    }

    // --- adjusted_active_after_remove tests ---

    #[test]
    fn remove_before_active_keeps_active() {
        // 4 layers [0,1,2,3], active=2, remove index 0 -> new_len=3, active=2 (still valid)
        assert_eq!(adjusted_active_after_remove(2, 0, 3), 2);
    }

    #[test]
    fn remove_active_layer_at_end_clamps() {
        // 3 layers [0,1,2], active=2, remove index 2 -> new_len=2, active was 2 >= 2 -> 1
        assert_eq!(adjusted_active_after_remove(2, 2, 2), 1);
    }

    #[test]
    fn remove_after_active_unchanged() {
        // 4 layers, active=1, remove index 3 -> new_len=3, active=1 (still valid)
        assert_eq!(adjusted_active_after_remove(1, 3, 3), 1);
    }

    #[test]
    fn remove_only_remaining_saturates_to_zero() {
        // Edge case: new_len=0 (shouldn't happen in practice, but saturating_sub handles it)
        assert_eq!(adjusted_active_after_remove(0, 0, 0), 0);
    }

    // --- adjusted_active_after_move tests ---

    #[test]
    fn move_active_layer_follows() {
        // active=1, move from=1 to=3 -> active becomes 3
        assert_eq!(adjusted_active_after_move(1, 1, 3), 3);
    }

    #[test]
    fn move_forward_shifts_middle_down() {
        // active=2, move from=1 to=3 -> active was between from+1..=to -> 2-1=1
        assert_eq!(adjusted_active_after_move(2, 1, 3), 1);
    }

    #[test]
    fn move_backward_shifts_middle_up() {
        // active=1, move from=3 to=0 -> active in [to..from) = [0..3) -> 1+1=2
        assert_eq!(adjusted_active_after_move(1, 3, 0), 2);
    }

    #[test]
    fn move_unrelated_unchanged() {
        // active=0, move from=2 to=3 -> active not affected
        assert_eq!(adjusted_active_after_move(0, 2, 3), 0);
    }

    #[test]
    fn move_same_position_unchanged() {
        // from==to edge (would be caught by caller, but function handles it)
        assert_eq!(adjusted_active_after_move(2, 1, 1), 2);
    }

    // ---- Additional tests ----

    #[test]
    fn blend_mode_exact_display_names() {
        assert_eq!(BlendMode::Normal.display_name(), "Normal");
        assert_eq!(BlendMode::Add.display_name(), "Add");
        assert_eq!(BlendMode::Screen.display_name(), "Screen");
        assert_eq!(BlendMode::ColorDodge.display_name(), "Color Dodge");
        assert_eq!(BlendMode::Multiply.display_name(), "Multiply");
        assert_eq!(BlendMode::Overlay.display_name(), "Overlay");
        assert_eq!(BlendMode::HardLight.display_name(), "Hard Light");
        assert_eq!(BlendMode::Difference.display_name(), "Difference");
        assert_eq!(BlendMode::Exclusion.display_name(), "Exclusion");
        assert_eq!(BlendMode::Subtract.display_name(), "Subtract");
        assert_eq!(BlendMode::Displace.display_name(), "Displace");
        assert_eq!(BlendMode::Refract.display_name(), "Refract");
        assert_eq!(BlendMode::Lens.display_name(), "Lens");
    }

    #[test]
    fn blend_mode_from_u32_roundtrip() {
        for mode in BlendMode::ALL {
            assert_eq!(BlendMode::from_u32(mode.as_u32()), *mode);
        }
        // Out of range falls back to Normal
        assert_eq!(BlendMode::from_u32(99), BlendMode::Normal);
    }

    #[test]
    fn blend_mode_serde_alias_soft_light() {
        let m: BlendMode = serde_json::from_str("\"SoftLight\"").unwrap();
        assert_eq!(m, BlendMode::Overlay);
    }

    #[test]
    fn adjusted_active_after_remove_active_equals_removed() {
        // active=1, removed=1, new_len=2 -> active=1 (still valid)
        assert_eq!(adjusted_active_after_remove(1, 1, 2), 1);
    }

    #[test]
    fn adjusted_active_after_remove_active_equals_removed_at_end() {
        // active=2, removed=2, new_len=2 -> active=2 >= 2 -> clamp to 1
        assert_eq!(adjusted_active_after_remove(2, 2, 2), 1);
    }

    #[test]
    fn adjusted_active_after_move_boundary_from_zero() {
        // active=0, move from=0 to=3 -> active follows = 3
        assert_eq!(adjusted_active_after_move(0, 0, 3), 3);
    }

    #[test]
    fn adjusted_active_after_move_boundary_to_zero() {
        // active=0, move from=2 to=0 -> active in [to..from) = [0..2) -> 0+1=1
        assert_eq!(adjusted_active_after_move(0, 2, 0), 1);
    }

    // --- BlendMode::from_normalized (#1792) ---

    #[test]
    fn blend_mode_from_normalized_endpoints() {
        assert_eq!(BlendMode::from_normalized(0.0), BlendMode::Normal);
        assert_eq!(BlendMode::from_normalized(1.0), BlendMode::Subtract);
    }

    #[test]
    fn blend_mode_from_normalized_reaches_all_color_modes() {
        for (i, mode) in BlendMode::COLOR.iter().enumerate() {
            let v = i as f32 / (BlendMode::COLOR.len() - 1) as f32;
            assert_eq!(BlendMode::from_normalized(v), *mode, "step {i} (v={v})");
        }
    }

    /// The displacement family (#1478) is deliberately outside the bus sweep.
    /// Two things break if this stops holding: every shipped preset with a
    /// bus-bound blend silently lands on a different mode, and a sweep passes
    /// through modes that draw no color at all, which reads as a dropout.
    #[test]
    fn blend_mode_from_normalized_never_returns_a_displacement_mode() {
        for step in 0..=1000 {
            let v = step as f32 / 1000.0;
            let mode = BlendMode::from_normalized(v);
            assert!(
                !mode.is_displacement(),
                "from_normalized({v}) returned {mode:?}"
            );
        }
    }

    #[test]
    fn blend_mode_from_normalized_clamps_out_of_range() {
        assert_eq!(BlendMode::from_normalized(-0.5), BlendMode::Normal);
        assert_eq!(BlendMode::from_normalized(2.0), BlendMode::Subtract);
    }

    #[test]
    fn blend_mode_from_normalized_interior_rounding() {
        // 0.5 * 9 = 4.5 rounds half-away-from-zero to 5 = Overlay.
        assert_eq!(BlendMode::from_normalized(0.5), BlendMode::Overlay);
        // Boundary between step 0 and 1 sits at 0.5/9 ≈ 0.0556.
        assert_eq!(BlendMode::from_normalized(0.049), BlendMode::Normal);
        assert_eq!(BlendMode::from_normalized(0.056), BlendMode::Add);
    }
}
