use serde::{Deserialize, Serialize};

use crate::effect::format::PostProcessDef;
use crate::gpu::pass_executor::PassExecutor;
use crate::gpu::placeholder::PlaceholderTexture;
use crate::gpu::render_target::RenderTarget;
use crate::gpu::uniforms::UniformBuffer;
use crate::gpu::ShaderUniforms;
use crate::media::MediaLayer;
use crate::params::ParamStore;

/// Blend mode for compositing layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendMode {
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
}

impl BlendMode {
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
            _ => BlendMode::Normal,
        }
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
}

/// Content type for a layer.
pub enum LayerContent {
    Effect(EffectLayer),
    Media(MediaLayer),
}

/// A single compositing layer. Owns its own rendering pipeline and parameters.
pub struct Layer {
    pub name: String,
    pub custom_name: Option<String>,
    pub param_store: ParamStore,
    pub content: LayerContent,
    pub blend_mode: BlendMode,
    pub opacity: f32,
    pub enabled: bool,
    pub locked: bool,
    pub pinned: bool,
    pub postprocess: PostProcessDef,
}

impl Layer {
    /// Create a new Effect layer.
    pub fn new_effect(
        name: String,
        effect: EffectLayer,
        param_store: ParamStore,
    ) -> Self {
        Self {
            name,
            custom_name: None,
            param_store,
            content: LayerContent::Effect(effect),
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
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
            content: LayerContent::Media(media),
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
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
        width: u32,
        height: u32,
        placeholder: &PlaceholderTexture,
    ) {
        match &mut self.content {
            LayerContent::Effect(e) => {
                e.pass_executor
                    .resize(device, width, height, &e.uniform_buffer, placeholder);
            }
            LayerContent::Media(_) => {
                // Media resize handled separately (needs queue for uniform upload)
            }
        }
    }

    /// Resize media layer (needs queue for uniform upload).
    pub fn resize_media(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) {
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
    pub enabled: bool,
    pub locked: bool,
    pub pinned: bool,
    pub has_particles: bool,
    pub shader_error: Option<String>,
    pub is_media: bool,
    pub media_file_name: Option<String>,
    pub media_is_animated: bool,
    pub media_is_video: bool,
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
                let (is_media, media_file_name, media_is_animated, media_is_video) = match &l.content {
                    LayerContent::Media(m) => (true, Some(m.file_name.clone()), m.is_animated(), m.is_video()),
                    _ => (false, None, false, false),
                };
                LayerInfo {
                    name: l.name.clone(),
                    custom_name: l.custom_name.clone(),
                    effect_index: l.effect_index(),
                    effect_name: l.effect_index().and_then(|i| effects.get(i)).map(|e| e.name.clone()),
                    blend_mode: l.blend_mode,
                    opacity: l.opacity,
                    enabled: l.enabled,
                    locked: l.locked,
                    pinned: l.pinned,
                    has_particles: l.has_particles(),
                    shader_error: l.shader_error().map(|s| s.to_string()),
                    is_media,
                    media_file_name,
                    media_is_animated,
                    media_is_video,
                }
            })
            .collect()
    }

    /// Number of enabled layers.
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
pub(crate) fn adjusted_active_after_move(active: usize, from: usize, to: usize) -> usize {
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
        assert_eq!(BlendMode::ALL.len(), 10);
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
}
