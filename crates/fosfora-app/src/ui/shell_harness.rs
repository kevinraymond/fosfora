//! The whole workspace shell, drawn headlessly for tests (#3126).
//!
//! Panels tested one at a time pass while the shell around them breaks
//! them: a section two columns away takes the height, a panel pushes the
//! target off screen. This draws [`draw_shell`] with everything the app
//! passes it, offline (no audio device, MIDI port, socket or server) and
//! without a GPU (the layer rows draw without their pictures), then the
//! binding matrix and the tour over it, in the order `main.rs` does.
//!
//! The chain editor needs a GPU (its nodes are compiled effects with live
//! previews), so it is drawn only by a harness made [`ShellHarness::with_chains`],
//! whose tests are `#[ignore]`d like the other GPU probes.

use egui::{Context, Event, Key, Pos2, RawInput, Rect, Vec2};

use super::panels::scene_panel::SceneInfo;
use super::shell::{ShellState, draw_shell};
use crate::audio::AudioSystem;
use crate::bindings::bus::BindingBus;
use crate::effect::EffectLoader;
use crate::effect::format::{PfxEffect, PostProcessDef};
use crate::gpu::ShaderUniforms;
use crate::gpu::layer::{BlendMode, LayerInfo};
use crate::gpu::volumetric::VolumetricParams;
use crate::midi::MidiSystem;
use crate::osc::OscSystem;
use crate::params::{ParamDef, ParamStore};
use crate::preset::PresetStore;
use crate::settings::SettingsConfig;
use crate::ui::catalog_thumbs::CatalogThumbs;
use crate::ui::panels::binding_helpers::{BindingPanelInfo, LayerParamInfo};
use crate::ui::panels::binding_matrix::BindingMatrixState;
use crate::web::WebSystem;

pub(crate) struct ShellHarness {
    pub ctx: Context,
    pub size: Vec2,
    audio: AudioSystem,
    /// Each layer's controls: the empty Layer 1 has none.
    params: Vec<ParamStore>,
    /// The selected layer. Follows `select_layer` as `main.rs` does.
    pub active_layer: usize,
    pub bindings: BindingBus,
    pub matrix: BindingMatrixState,
    uniforms: ShaderUniforms,
    loader: EffectLoader,
    postprocess: PostProcessDef,
    volumetric_enabled: bool,
    volumetric: VolumetricParams,
    midi: MidiSystem,
    osc: OscSystem,
    web: WebSystem,
    presets: PresetStore,
    pub layers: Vec<LayerInfo>,
    settings: SettingsConfig,
    thumbs: CatalogThumbs,
    scene: SceneInfo,
    time: f64,
    /// The first request to leave the view a tour step shows, seen between
    /// the shell and the tour, which drops it.
    pub leaving: Option<&'static str>,
    /// The chain editor and the layers it edits: with a GPU only.
    pub chains: Option<Chains>,
}

/// What the chain editor draws from: the app's trama system, and real
/// layers for the chains to live on (one per harness layer).
pub(crate) struct Chains {
    pub trama: crate::trama::TramaSystem,
    pub stack: crate::gpu::layer::LayerStack,
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

/// The controls of the harness's effect layer.
pub(crate) const PARAMS: [&str; 3] = ["trail_decay", "beat_snap", "flow_stretch"];

fn effect_params() -> ParamStore {
    let mut p = ParamStore::new();
    let defs: Vec<ParamDef> = PARAMS
        .iter()
        .map(|n| ParamDef::Float {
            name: n.to_string(),
            default: 0.5,
            min: 0.0,
            max: 1.0,
        })
        .collect();
    p.load_from_defs(&defs);
    p
}

impl ShellHarness {
    /// A window `size` points large with two layers and a dozen effects,
    /// in the workspace layout.
    pub fn new(size: Vec2) -> Self {
        let ctx = Context::default();
        // The app's fonts and spacing: text laid out in egui's defaults is
        // narrower, and a label cut off in the app fit here.
        crate::ui::overlay::configure(&ctx, &crate::ui::theme::palette::Palette::GRAY);
        let mut loader = EffectLoader::for_test("");
        for (i, cat) in ["particles", "fluids", "growth", "overlays"]
            .iter()
            .cycle()
            .take(12)
            .enumerate()
        {
            loader.effects.push(effect(&format!("Effect {i}"), cat));
        }
        // What the Layers tour loads into an empty top layer.
        loader
            .effects
            .push(effect(crate::ui::tour::STARTER_EFFECT, "pattern"));
        Self {
            ctx,
            size,
            audio: AudioSystem::offline(),
            params: vec![ParamStore::new(), effect_params()],
            active_layer: 0,
            bindings: {
                // Nothing from this machine's config, and the audio sources
                // listed as they are once analysis runs.
                let mut b = BindingBus::new_isolated();
                b.last_snapshot = crate::bindings::sources::collect_audio(&Default::default());
                b
            },
            matrix: BindingMatrixState::new(),
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
            chains: None,
        }
    }

    /// [`Self::new`] with the chain editor: a GPU device (the shared probe
    /// one), the shipped trama effects, and a real layer under each harness
    /// layer. For `#[ignore]`d tests.
    pub fn with_chains(size: Vec2) -> Self {
        use crate::gpu::context::GpuContext;
        // The effects load CWD-relative, from the repo root.
        if !std::path::Path::new("assets/effects").is_dir() {
            let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            std::env::set_current_dir(&repo).unwrap();
        }
        let (device, queue) = crate::gpu::test_gpu::test_gpu();
        let mut loader = EffectLoader::new();
        loader.scan_effects_directory();
        let placeholder = crate::gpu::placeholder::PlaceholderTexture::new(
            &device,
            &queue,
            GpuContext::hdr_format(),
        );
        let audio = crate::gpu::audio_textures::AudioTextures::new(&device, &queue);
        const DIM: u32 = 64;
        let build = crate::gpu::layer_builder::LayerBuildCtx {
            device: &device,
            queue: &queue,
            pipeline_cache: None,
            width: DIM,
            height: DIM,
            placeholder: &placeholder,
            audio_textures: &audio,
            particle_quality: Default::default(),
            backdrop: None,
        };
        let mut h = Self::new(size);
        let mut stack = crate::gpu::layer::LayerStack::new();
        for l in &h.layers {
            let layer = crate::gpu::layer_builder::new_default_layer(&build, l.name.clone())
                .expect("a layer");
            stack.layers.push(layer);
        }
        let trama =
            crate::trama::TramaSystem::new(&device, None, &loader, &placeholder, &audio, DIM, DIM);
        h.chains = Some(Chains { trama, stack });
        h
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
        // As App::update does before the interface: the canvas on its chain.
        if let Some(c) = &mut self.chains {
            c.stack.active_layer = self.active_layer;
            c.trama.resolve_active_chain(&mut c.stack);
            for (info, layer) in self.layers.iter_mut().zip(&c.stack.layers) {
                info.chain = layer
                    .chain
                    .as_ref()
                    .and_then(|c| crate::gpu::layer::ChainBadge::of(&c.graph));
            }
        }
        let ctx = self.ctx.clone();
        ctx.run(input, |ctx| {
            let active = self.active_layer;
            let mut s = ShellState {
                audio: &mut self.audio,
                params: &mut self.params[active],
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
                active_layer: active,
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
                bindings: &self.bindings,
            };
            draw_shell(ctx, true, &mut s);
            if self.leaving.is_none() {
                self.leaving = crate::ui::tour::leaving_requested(ctx);
            }
            crate::ui::tour::gate(ctx);
            if let Some(c) = &mut self.chains {
                crate::trama::ui::canvas::follow_tour(ctx, &mut c.trama);
                if let Some(bounds) = crate::ui::modal::bounds(ctx) {
                    crate::trama::ui::canvas::draw_trama_modal(
                        ctx,
                        &mut c.trama,
                        &mut c.stack,
                        bounds,
                    );
                }
            }
            let (layers, params) = (&self.layers, &self.params);
            crate::ui::panels::binding_matrix::frame(
                ctx,
                &mut self.matrix,
                &mut self.bindings,
                || BindingPanelInfo {
                    layers: layers
                        .iter()
                        .zip(params)
                        .enumerate()
                        .map(|(index, (l, p))| LayerParamInfo {
                            index,
                            effect_name: l.effect_name.clone().unwrap_or_default(),
                            param_names: p.defs.iter().map(|d| d.name().to_string()).collect(),
                        })
                        .collect(),
                    active_layer: active,
                    layer_count: layers.len(),
                    preset_name: "(unsaved)".into(),
                },
            );
            crate::ui::tour::draw(ctx);
            // As main.rs does after the frame, in its order: a catalog drop,
            // the selection, then the selected layer's blend mode.
            use crate::ui::panels::catalog_panel::{CatalogDrop, DROP_INTENT};
            let drop = ctx.data_mut(|d| {
                let id = egui::Id::new(DROP_INTENT);
                let v = d.get_temp::<(String, CatalogDrop)>(id);
                d.remove::<(String, CatalogDrop)>(id);
                v
            });
            if let Some((name, CatalogDrop::Replace(i))) = drop
                && let Some(fx) = self.loader.effects.iter().position(|e| e.name == name)
                && self.layers.get(i).is_some_and(|l| !l.locked)
            {
                self.layers[i].effect_index = Some(fx);
                self.layers[i].effect_name = Some(name);
                self.active_layer = i;
            }
            if let Some(i) = ctx.data_mut(|d| d.remove_temp::<usize>(egui::Id::new("select_layer")))
                && i < self.layers.len()
            {
                self.active_layer = i;
            }
            if let Some(mode) = ctx.data_mut(|d| d.remove_temp::<u32>(egui::Id::new("layer_blend")))
                && let Some(l) = self.layers.get_mut(self.active_layer)
                && !l.locked
            {
                l.blend_mode = BlendMode::from_u32(mode);
            }
        })
    }

    /// The effect layer: the one with controls.
    pub const EFFECT_LAYER: usize = 1;

    /// Where text reading exactly `text` is painted in one frame, top to
    /// bottom.
    pub fn text_rects(&mut self, text: &str) -> Vec<Rect> {
        fn walk(shape: &egui::Shape, text: &str, found: &mut Vec<Rect>) {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == text => {
                    found.push(t.galley.rect.translate(t.pos.to_vec2()));
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, text, found)),
                _ => {}
            }
        }
        let out = self.frame_output(vec![]);
        let mut found = Vec::new();
        out.shapes
            .iter()
            .for_each(|c| walk(&c.shape, text, &mut found));
        found.sort_by(|a, b| a.top().total_cmp(&b.top()));
        found
    }

    /// Every text painted in one frame: what it says, where, and the clip
    /// it was painted in.
    pub fn texts(&mut self) -> Vec<(String, Rect, Rect)> {
        fn walk(shape: &egui::Shape, clip: Rect, out: &mut Vec<(String, Rect, Rect)>) {
            match shape {
                egui::Shape::Text(t) => out.push((
                    t.galley.text().to_string(),
                    t.galley.rect.translate(t.pos.to_vec2()),
                    clip,
                )),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, out)),
                _ => {}
            }
        }
        let out = self.frame_output(vec![]);
        let mut found = Vec::new();
        out.shapes
            .iter()
            .for_each(|c| walk(&c.shape, c.clip_rect, &mut found));
        found
    }

    /// The first of [`Self::text_rects`].
    pub fn text_rect(&mut self, text: &str) -> Option<Rect> {
        self.text_rects(text).first().copied()
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
