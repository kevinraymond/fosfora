//! The whole workspace shell, drawn headlessly for tests (#3126).
//!
//! Panels tested one at a time pass while the shell around them breaks
//! them: a section two columns away takes the height, a panel pushes the
//! target off screen. This draws [`draw_shell`] with everything the app
//! passes it, offline (no audio device, MIDI port, socket or server) and
//! without a GPU (the layer rows draw without their pictures), then the
//! tour over it, in the order `main.rs` does.

use egui::{Context, Event, Key, Pos2, RawInput, Rect, Vec2};

use super::panels::scene_panel::SceneInfo;
use super::shell::{ShellState, draw_shell};
use crate::audio::AudioSystem;
use crate::effect::EffectLoader;
use crate::effect::format::{PfxEffect, PostProcessDef};
use crate::gpu::ShaderUniforms;
use crate::gpu::layer::{BlendMode, LayerInfo};
use crate::gpu::volumetric::VolumetricParams;
use crate::midi::MidiSystem;
use crate::osc::OscSystem;
use crate::params::ParamStore;
use crate::preset::PresetStore;
use crate::settings::SettingsConfig;
use crate::ui::catalog_thumbs::CatalogThumbs;
use crate::web::WebSystem;

pub(crate) struct ShellHarness {
    pub ctx: Context,
    pub size: Vec2,
    audio: AudioSystem,
    params: ParamStore,
    uniforms: ShaderUniforms,
    loader: EffectLoader,
    postprocess: PostProcessDef,
    volumetric_enabled: bool,
    volumetric: VolumetricParams,
    midi: MidiSystem,
    osc: OscSystem,
    web: WebSystem,
    presets: PresetStore,
    layers: Vec<LayerInfo>,
    settings: SettingsConfig,
    thumbs: CatalogThumbs,
    scene: SceneInfo,
    time: f64,
    /// The first request to leave the view a tour step shows, seen between
    /// the shell and the tour, which drops it.
    pub leaving: Option<&'static str>,
}

fn effect(name: &str, category: &str) -> PfxEffect {
    serde_json::from_str(&format!(
        r#"{{"name":"{name}","author":"Fosfora","category":"{category}","description":"{name} does a thing."}}"#
    ))
    .unwrap()
}

fn layer(name: &str, effect_index: usize) -> LayerInfo {
    LayerInfo {
        name: name.into(),
        custom_name: None,
        effect_index: Some(effect_index),
        effect_name: Some(name.into()),
        blend_mode: BlendMode::default(),
        opacity: 1.0,
        displace_amount: 0.0,
        enabled: true,
        locked: false,
        pinned: false,
        has_particles: false,
        shader_error: None,
        is_media: false,
        media_file_name: None,
        media_is_animated: false,
        media_is_video: false,
        media_is_live: false,
        chain: None,
        needs_layer_below: false,
    }
}

impl ShellHarness {
    /// A window `size` points large with two layers and a dozen effects,
    /// in the workspace layout.
    pub fn new(size: Vec2) -> Self {
        let ctx = Context::default();
        crate::ui::theme::colors::set_theme_colors(
            &ctx,
            crate::ui::theme::palette::Palette::GRAY.colors(),
        );
        let mut loader = EffectLoader::for_test("");
        for (i, cat) in ["particles", "fluids", "growth", "overlays"]
            .iter()
            .cycle()
            .take(12)
            .enumerate()
        {
            loader.effects.push(effect(&format!("Effect {i}"), cat));
        }
        Self {
            ctx,
            size,
            audio: AudioSystem::offline(),
            params: ParamStore::new(),
            uniforms: bytemuck::Zeroable::zeroed(),
            loader,
            postprocess: PostProcessDef::default(),
            volumetric_enabled: false,
            volumetric: VolumetricParams::default(),
            midi: MidiSystem::offline(),
            osc: OscSystem::offline(),
            web: WebSystem::offline(),
            presets: {
                // Two built-in presets and a user one: tiles to click.
                let mut p = PresetStore::new();
                for name in ["Opener", "Middle", "Mine"] {
                    let preset = serde_json::from_str(r#"{"layers":[]}"#).unwrap();
                    p.presets.push((name.to_string(), preset));
                }
                p.builtin_count = 2;
                p
            },
            // The launch stack: an empty Layer 1 over an effect.
            layers: vec![
                LayerInfo {
                    effect_index: None,
                    effect_name: None,
                    ..layer("Layer 1", 0)
                },
                layer("Effect 0", 0),
            ],
            settings: SettingsConfig {
                classic_layout: false,
                ..Default::default()
            },
            // Nowhere: the cards draw without pictures.
            thumbs: CatalogThumbs::new("/nonexistent".into()),
            scene: super::panels::cue_strip::sample(false),
            time: 0.0,
            leaving: None,
        }
    }

    pub fn screen(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, self.size)
    }

    /// One frame of the app with `events`: the shell, then the tour.
    pub fn frame(&mut self, events: Vec<Event>) {
        let _ = self.frame_output(events);
    }

    /// [`Self::frame`], returning what egui would hand the renderer.
    pub fn frame_output(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.time += 1.0 / 60.0;
        let input = RawInput {
            screen_rect: Some(self.screen()),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let ctx = self.ctx.clone();
        ctx.run(input, |ctx| {
            let mut s = ShellState {
                audio: &mut self.audio,
                params: &mut self.params,
                shader_error: &None,
                uniforms: &self.uniforms,
                effect_loader: &self.loader,
                postprocess: &mut self.postprocess,
                postprocess_previous: None,
                volumetric_enabled: &mut self.volumetric_enabled,
                volumetric_params: &mut self.volumetric,
                particle_count: None,
                midi: &mut self.midi,
                osc: &mut self.osc,
                web: &mut self.web,
                preset_store: &self.presets,
                layers: &self.layers,
                active_layer: 0,
                master_chain: None,
                media_info: None,
                webcam_info: None,
                particle_info: None,
                status_error: &None,
                settings: &self.settings,
                layer_thumbs: None,
                postfx_matches: &[],
                display: None,
                catalog_thumbs: &mut self.thumbs,
                scene: &self.scene,
            };
            draw_shell(ctx, true, &mut s);
            if self.leaving.is_none() {
                self.leaving = crate::ui::tour::leaving_requested(ctx);
            }
            crate::ui::tour::draw(ctx);
        })
    }

    /// `n` frames with nothing happening: egui settles sizes over a few.
    pub fn settle(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![]);
        }
    }

    /// A primary click at `pos`, over two frames as a real one arrives.
    pub fn click(&mut self, pos: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(vec![Event::PointerMoved(pos), button(true)]);
        self.frame(vec![button(false)]);
    }

    pub fn key(&mut self, key: Key) {
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        self.frame(vec![ev(true), ev(false)]);
    }
}
