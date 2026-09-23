//! The v2 workspace shell (#3122, GH #32).
//!
//! The v1 layout puts every control into two fixed 315 px side panels drawn
//! over a full-window render. This draws the alternative: a top bar that
//! switches between three workspaces, the output as a preview rather than the
//! backdrop, and each setting at one level of the Preset → Layers → Effect
//! hierarchy.
//!
//! It ships behind `SettingsConfig::classic_layout` for one release, so the
//! old panels stay reachable while this settles. Both layouts call the same
//! per-section draw functions in [`super::panels`] — only the arrangement
//! differs, so a control fixed in one is fixed in both.

use egui::{Context, Frame, Margin, ScrollArea};

use super::panels::{
    audio_panel, effect_panel, layer_panel, media_panel, midi_panel, osc_panel,
    output_window_panel, param_panel, postfx_panel, preset_panel, recording_panel, settings_panel,
    stack_panel, status_bar, triggers_panel, volumetric_panel, web_panel,
};
use super::widgets;
use crate::audio::AudioSystem;
use crate::bindings::bus::BindingBus;
use crate::effect::EffectLoader;
use crate::effect::format::PostProcessDef;
use crate::gpu::ShaderUniforms;
use crate::gpu::layer::LayerInfo;
use crate::gpu::volumetric::VolumetricParams;
use crate::midi::MidiSystem;
use crate::osc::OscSystem;
use crate::params::ParamStore;
use crate::preset::PresetStore;
use crate::settings::SettingsConfig;
use crate::ui::theme::colors::theme_colors;
use crate::web::WebSystem;

/// Which workspace is open. Held in egui data, not in settings: it is where
/// you are right now, not a preference, and a fresh launch should open on
/// Build rather than wherever the last session ended.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Workspace {
    Perform,
    Build,
    Setup,
}

impl Workspace {
    const ALL: [(Workspace, &'static str); 3] = [
        (Workspace::Perform, "Perform"),
        (Workspace::Build, "Build"),
        (Workspace::Setup, "Setup"),
    ];

    pub fn read(ctx: &Context) -> Self {
        ctx.data_mut(|d| match d.get_temp::<u8>(egui::Id::new("v2_workspace")) {
            Some(0) => Workspace::Perform,
            Some(2) => Workspace::Setup,
            // Nothing stored yet, or a value from an older build: open on Build.
            _ => Workspace::Build,
        })
    }

    pub fn write(self, ctx: &Context) {
        let v: u8 = match self {
            Workspace::Perform => 0,
            Workspace::Build => 1,
            Workspace::Setup => 2,
        };
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("v2_workspace"), v));
    }
}

/// Everything the shell draws. One struct rather than thirty arguments: the
/// v1 `draw_panels` takes them positionally and adding a section there means
/// threading another parameter through every call site.
pub struct ShellState<'a> {
    pub audio: &'a mut AudioSystem,
    pub params: &'a mut ParamStore,
    pub shader_error: &'a Option<String>,
    pub uniforms: &'a ShaderUniforms,
    pub effect_loader: &'a EffectLoader,
    pub postprocess: &'a mut PostProcessDef,
    pub volumetric_enabled: &'a mut bool,
    pub volumetric_params: &'a mut VolumetricParams,
    pub particle_count: Option<u32>,
    pub midi: &'a mut MidiSystem,
    pub osc: &'a mut OscSystem,
    pub web: &'a mut WebSystem,
    pub binding_bus: &'a mut BindingBus,
    pub preset_store: &'a PresetStore,
    pub layers: &'a [LayerInfo],
    pub active_layer: usize,
    pub master_chain: Option<crate::gpu::layer::ChainBadge>,
    pub media_info: Option<media_panel::MediaInfo>,
    pub webcam_info: Option<super::panels::webcam_panel::WebcamInfo>,
    pub particle_info: Option<super::panels::particle_panel::ParticleInfo>,
    pub status_error: &'a Option<(String, std::time::Instant)>,
    pub settings: &'a SettingsConfig,
    /// The trama canvas is open: Build leaves its middle column to it, and
    /// `main.rs` draws it there after the shell (#3123).
    pub trama_docked: bool,
    /// The layer rows' pictures (#3123).
    pub layer_thumbs: &'a crate::gpu::layer_thumbs::LayerThumbs,
    /// Per layer: does its effect's own post-processing equal Master's?
    pub postfx_matches: &'a [bool],
    /// The finished frame and its aspect ratio, drawn as the output preview.
    pub display: Option<(egui::TextureId, f32)>,
}

/// Draw the workspace shell. Returns without drawing when the overlay is
/// hidden, exactly like the v1 panels — F still gives a clean full-window
/// output with no interface over it.
pub fn draw_shell(ctx: &Context, visible: bool, s: &mut ShellState<'_>) {
    if !visible {
        return;
    }
    let tc = theme_colors(ctx);
    let ws = Workspace::read(ctx);

    egui::TopBottomPanel::top("v2_top")
        .exact_height(40.0)
        .frame(Frame {
            fill: tc.panel,
            inner_margin: Margin::symmetric(10, 4),
            ..Default::default()
        })
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(egui::RichText::new("fosfora").size(15.0).strong());
                ui.separator();

                let preset_name = s
                    .preset_store
                    .current_preset
                    .and_then(|i| s.preset_store.presets.get(i))
                    .map(|(n, _)| n.as_str())
                    .unwrap_or("Unsaved preset");
                ui.label(egui::RichText::new(preset_name).size(13.0));
                if s.preset_store.dirty {
                    ui.label(
                        egui::RichText::new("edited")
                            .size(11.0)
                            .color(tc.text_secondary),
                    );
                }

                ui.separator();
                for (w, label) in Workspace::ALL {
                    if ui
                        .selectable_label(w == ws, egui::RichText::new(label).size(13.0))
                        .clicked()
                    {
                        w.write(ui.ctx());
                    }
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button(egui::RichText::new("Full output").size(12.0))
                        .on_hover_text("Hide the interface — Esc or D brings it back")
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("request_hide_overlay"), true);
                        });
                    }
                    let bpm = s.uniforms.bpm * 300.0;
                    if bpm > 1.0 {
                        ui.label(
                            egui::RichText::new(format!("{bpm:.0} BPM"))
                                .size(12.0)
                                .color(tc.text_secondary),
                        );
                    }
                });
            });
        });

    egui::TopBottomPanel::bottom("v2_status").show(ctx, |ui| {
        status_bar::draw_status_bar(
            ui,
            s.shader_error,
            s.uniforms,
            s.particle_count,
            s.midi.config.enabled,
            s.midi.is_recently_active(),
            s.osc.config.enabled,
            s.osc.is_recently_active(),
            s.web.config.enabled,
            s.web.client_count,
            false,
            false,
            false,
            false,
            false,
            None,
            s.status_error,
            None,
            s.audio.indicator(),
        );
    });

    match ws {
        Workspace::Build => build_workspace(ctx, s, tc.panel),
        Workspace::Perform => perform_workspace(ctx, s, tc.panel),
        Workspace::Setup => setup_workspace(ctx, s, tc.panel),
    }
}

/// The output: the picture, and under it the control that sends it to a second
/// display.
///
/// One function rather than a section per workspace, because the two belong
/// together — the picker first went only into Setup, and from Build or Perform
/// there was no sign the feature existed.
fn output_section(ui: &mut egui::Ui, id: &str, display: Option<(egui::TextureId, f32)>) {
    let ow = output_window_panel::read(ui.ctx()).unwrap_or_default();
    let badge = ow.open_on.is_some().then_some("2nd window");
    widgets::section(ui, id, "Output", badge, true, |ui| {
        preview(ui, display);
        ui.add_space(8.0);
        output_window_panel::draw(ui, &ow);
    });
}

/// The output as a picture, sized to the width it is given.
fn preview(ui: &mut egui::Ui, display: Option<(egui::TextureId, f32)>) {
    if let Some((tex, aspect)) = display {
        let w = ui.available_width();
        let size = egui::vec2(w, (w / aspect.max(0.01)).round());
        ui.add(egui::Image::new(egui::load::SizedTexture::new(tex, size)).corner_radius(3.0));
    }
}

fn panel_frame(fill: egui::Color32) -> Frame {
    Frame {
        fill,
        inner_margin: Margin::same(8),
        ..Default::default()
    }
}

/// Build: the structure on the left, the selection's controls in the middle,
/// the output and its audio on the right.
fn build_workspace(ctx: &Context, s: &mut ShellState<'_>, fill: egui::Color32) {
    // Wide enough for layer rows to breathe: at 300 px they were cramped while
    // the middle column had width to spare. Resizable inside a range rather
    // than fixed, since the right size depends on the window.
    egui::SidePanel::left("v2_structure")
        .default_width(680.0)
        .width_range(600.0..=800.0)
        .resizable(true)
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                let layer_badge = format!("{}/{}", s.layers.len(), 8);
                widgets::section(ui, "v2_layers", "Stack", Some(&layer_badge), true, |ui| {
                    let pics = stack_panel::StackPictures {
                        thumbs: Some(s.layer_thumbs),
                        output: s.display.map(|(t, _)| t),
                        aspect: s.display.map_or(16.0 / 9.0, |(_, a)| a),
                    };
                    stack_panel::draw_stack(
                        ui,
                        s.layers,
                        s.active_layer,
                        s.master_chain,
                        &postfx_on(s.postprocess),
                        &pics,
                    );
                });
                // Closed to start: in Build the stack is what this column is
                // for, and both are a click away.
                preset_panel::draw_preset_section_open(ui, s.preset_store, false);
                let fx_badge = format!("{}", s.effect_loader.effects.len());
                widgets::section(ui, "v2_catalog", "Catalog", Some(&fx_badge), false, |ui| {
                    effect_panel::draw_effect_panel(
                        ui,
                        s.effect_loader,
                        &s.settings.favorite_effects,
                    );
                });
            });
        });

    egui::SidePanel::right("v2_output")
        .exact_width(420.0)
        .resizable(false)
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                output_section(ui, "v2_preview", s.display);
                let bpm = s.uniforms.bpm * 300.0;
                let bpm_badge = (bpm > 1.0).then(|| format!("{bpm:.0}"));
                widgets::section(ui, "v2_audio", "Audio", bpm_badge.as_deref(), true, |ui| {
                    audio_panel::draw_audio_panel(ui, s.audio, s.uniforms);
                });
            });
        });

    if s.trama_docked {
        return;
    }
    egui::CentralPanel::default()
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                inspector_width(ui);
                if stack_panel::master_selected(ui.ctx()) {
                    master_inspector(ui, s);
                } else {
                    layer_inspector(ui, s);
                }
            });
        });
}

/// Widest the inspector's content gets, however wide its column is.
const INSPECTOR_MAX_WIDTH: f32 = 720.0;

/// Hold the inspector to a reading width: at full width a description ran to
/// ~130 characters a line and every slider stretched across 600 px. `min()`
/// because a bare `set_max_width` inside a ScrollArea acts as a MINIMUM too —
/// the content stayed 720 px however narrow the column got, and clipped every
/// row's right-hand buttons.
fn inspector_width(ui: &mut egui::Ui) {
    ui.set_max_width(ui.available_width().min(INSPECTOR_MAX_WIDTH));
}

/// The post-processing stages switched on, in words, for Master's row.
fn postfx_on(pp: &PostProcessDef) -> Vec<&'static str> {
    if !pp.enabled {
        return Vec::new();
    }
    [
        (pp.bloom_enabled, "bloom"),
        (pp.ca_enabled, "chromatic aberration"),
        (pp.vignette_enabled, "vignette"),
        (pp.grain_enabled, "grain"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect()
}

/// The inspector's heading: what scope, what it is called, what it does.
fn inspector_heading(ui: &mut egui::Ui, kicker: &str, name: &str, desc: &str) {
    let tc = theme_colors(ui.ctx());
    ui.label(
        egui::RichText::new(kicker)
            .size(12.0)
            .color(tc.text_secondary),
    );
    ui.label(egui::RichText::new(name).size(20.0).strong());
    if !desc.is_empty() {
        ui.label(
            egui::RichText::new(desc)
                .size(13.0)
                .color(tc.text_secondary),
        );
    }
    ui.add_space(8.0);
}

/// A chain's state and the way into the canvas, for either scope.
fn chain_line(
    ui: &mut egui::Ui,
    badge: Option<crate::gpu::layer::ChainBadge>,
    empty: &str,
    open: impl FnOnce(&egui::Context),
) {
    let tc = theme_colors(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Chain").size(13.0).strong());
        let words = match badge {
            None => empty.to_string(),
            Some(b) => {
                let nodes = match b.nodes {
                    1 => "1 node".to_string(),
                    n => format!("{n} nodes"),
                };
                if b.active {
                    format!("{nodes}, reaches Output")
                } else {
                    format!("{nodes}, inactive: nothing reaches Output yet")
                }
            }
        };
        ui.label(
            egui::RichText::new(words)
                .size(13.0)
                .color(tc.text_secondary),
        );
        if ui
            .button(egui::RichText::new("Edit chain  (G)").size(12.0))
            .clicked()
        {
            open(ui.ctx());
        }
    });
}

/// Layer scope: everything that belongs to the selected layer, and nothing
/// that belongs to the preset as a whole.
fn layer_inspector(ui: &mut egui::Ui, s: &mut ShellState<'_>) {
    let Some(layer) = s.layers.get(s.active_layer) else {
        inspector_heading(ui, "Layer", "No layer", "");
        return;
    };
    let kind = stack_panel::layer_kind(layer);
    let kicker = format!(
        "Layer {} of {} · {kind}",
        s.active_layer + 1,
        s.layers.len()
    );
    let desc = if layer.is_media {
        layer.media_file_name.clone().unwrap_or_default()
    } else {
        layer
            .effect_index
            .and_then(|i| s.effect_loader.effects.get(i))
            .map(|e| e.description.clone())
            .unwrap_or_default()
    };
    inspector_heading(ui, &kicker, stack_panel::layer_name(layer), &desc);

    let bottom = s.layers.iter().rposition(|l| l.enabled) == Some(s.active_layer);
    widgets::section(ui, "v2_blend", "Blend", None, true, |ui| {
        blend_controls(ui, layer, s.active_layer, bottom);
    });

    if let Some(ref info) = s.webcam_info {
        widgets::section(ui, "v2_webcam", "Camera", None, true, |ui| {
            super::panels::webcam_panel::draw_webcam_panel(ui, info);
        });
    } else if let Some(ref info) = s.media_info {
        widgets::section(ui, "v2_media", "Media", None, true, |ui| {
            media_panel::draw_media_panel(ui, info);
        });
    } else {
        widgets::section(ui, "v2_params", "Parameters", None, true, |ui| {
            param_panel::draw_param_panel(ui, s.params, s.midi, s.osc);
        });
        if let Some(ref pinfo) = s.particle_info {
            let badge = if pinfo.alive_count >= 1000 {
                format!("{:.1}K", pinfo.alive_count as f32 / 1000.0)
            } else {
                format!("{}", pinfo.alive_count)
            };
            widgets::section(ui, "v2_particles", "Particles", Some(&badge), false, |ui| {
                super::panels::particle_panel::draw_particle_panel(ui, pinfo);
            });
        }
    }

    ui.add_space(6.0);
    let i = s.active_layer;
    chain_line(ui, layer.chain, "none yet", |ctx| {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("open_trama_on_layer"), i));
    });
}

/// Blend mode, visibility and opacity: how the layer lands on the stack.
fn blend_controls(ui: &mut egui::Ui, layer: &crate::gpu::layer::LayerInfo, i: usize, bottom: bool) {
    use crate::gpu::layer::BlendMode;
    let tc = theme_colors(ui.ctx());
    ui.add_enabled_ui(!layer.locked, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Mode").size(13.0));
            egui::ComboBox::from_id_salt("v2_blend_mode")
                .selected_text(layer.blend_mode.display_name())
                .width(180.0)
                .show_ui(ui, |ui| {
                    for &mode in BlendMode::ALL {
                        // The displacement family warps instead of coloring.
                        if mode == BlendMode::DISPLACEMENT[0] {
                            ui.separator();
                        }
                        let r = ui
                            .selectable_label(mode == layer.blend_mode, mode.display_name())
                            .on_hover_text(mode.description());
                        if r.clicked() && mode != layer.blend_mode {
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::new("layer_blend"), mode.as_u32());
                            });
                        }
                    }
                });
            ui.add_space(12.0);
            let mut visible = layer.enabled;
            if ui.checkbox(&mut visible, "Visible").changed() {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("layer_toggle_enable"), (i, visible));
                });
            }
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Opacity").size(13.0));
            let mut opacity = layer.opacity;
            let r = ui.add(
                egui::Slider::new(&mut opacity, 0.0..=1.0)
                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
            );
            if r.changed() {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("layer_opacity"), opacity));
            }
        });
        if layer.blend_mode.is_displacement() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Displace").size(13.0));
                let mut amount = layer.displace_amount;
                let r = ui.add(
                    egui::Slider::new(&mut amount, 0.0..=1.0)
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                );
                if r.changed() {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(egui::Id::new("layer_displace"), amount));
                }
            });
        }
    });
    let note = if layer.locked {
        Some("This layer is locked. Unlock it from its row's right-click menu.")
    } else if bottom {
        Some(
            "The bottom layer has nothing beneath it, so its blend mode is not applied; \
             only its opacity is.",
        )
    } else {
        None
    };
    if let Some(note) = note {
        ui.label(
            egui::RichText::new(note)
                .size(12.0)
                .color(tc.text_secondary),
        );
    }
}

/// Where Master's post-processing came from, and the way back to an effect's
/// recommended look. Post-processing belongs to Master alone; what an effect
/// contributes is the block its author wrote into its `.pfx` — adopted by
/// itself when the effect is the only layer, and offered here otherwise.
fn postfx_source(ui: &mut egui::Ui, s: &ShellState<'_>) {
    let tc = theme_colors(ui.ctx());
    // One entry per effect that recommends a look: an effect with no
    // post-processing block would "recommend" plain defaults, and two layers
    // of the same effect recommend the same thing once.
    let mut effects: Vec<(usize, &str, bool)> = Vec::new();
    for (i, l) in s.layers.iter().enumerate() {
        let Some(fx) = l.effect_index.and_then(|e| s.effect_loader.effects.get(e)) else {
            continue;
        };
        if l.is_media || fx.postprocess.is_none() || effects.iter().any(|e| e.1 == fx.name) {
            continue;
        }
        let matches = s.postfx_matches.get(i).copied().unwrap_or(false);
        effects.push((i, fx.name.as_str(), matches));
    }
    let source = match effects.iter().find(|e| e.2) {
        Some((_, name, _)) => format!("Matches {name}'s recommended look."),
        None => "This preset's own settings.".to_string(),
    };
    ui.label(
        egui::RichText::new(format!("{source} Selecting a layer never changes them."))
            .size(12.0)
            .color(tc.text_secondary),
    );
    let offers: Vec<_> = effects.iter().filter(|e| !e.2).collect();
    if !offers.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Reset to an effect's recommended look:").size(12.0));
            for &&(i, name, _) in &offers {
                if ui
                    .small_button(name)
                    .on_hover_text(format!(
                        "{name}'s effect file recommends its own post-processing. \
                         Copy it to Master, replacing the settings below."
                    ))
                    .clicked()
                {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("adopt_layer_postprocess"), i);
                    });
                }
            }
        });
    }
}

/// Master scope: what runs on the finished blend, and is saved with the
/// preset rather than with any one layer.
fn master_inspector(ui: &mut egui::Ui, s: &mut ShellState<'_>) {
    inspector_heading(
        ui,
        "Master · runs on the blended stack",
        "Master",
        "Post-processing and the master chain apply after every layer is blended. \
         Master's row at the top of the stack shows the result.",
    );
    // The matrix is a window, not a panel — a button is the whole control,
    // so it does not need a section of its own.
    let active = s.binding_bus.active_count();
    let label = if active > 0 {
        format!("Bindings · {active} active  (B)")
    } else {
        "Bindings  (B)".to_string()
    };
    if ui
        .button(egui::RichText::new(label).size(12.0))
        .on_hover_text("Open the binding matrix")
        .clicked()
    {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("open_binding_matrix"), true);
        });
    }
    ui.add_space(6.0);
    widgets::section(ui, "v2_postfx", "Post-Processing", None, true, |ui| {
        postfx_source(ui, s);
        ui.add_space(4.0);
        postfx_panel::draw_postfx_panel(ui, s.postprocess);
    });
    widgets::section(ui, "v2_volumetric", "Volumetric (R3)", None, true, |ui| {
        volumetric_panel::draw_volumetric_panel(ui, s.volumetric_enabled, s.volumetric_params);
    });
    ui.add_space(6.0);
    chain_line(
        ui,
        s.master_chain,
        "empty: passes the blend through",
        |ctx| {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("open_trama_on_master"), true));
        },
    );
}

/// Perform: presets on the left, the output large in the middle.
fn perform_workspace(ctx: &Context, s: &mut ShellState<'_>, fill: egui::Color32) {
    egui::SidePanel::left("v2_presets")
        .default_width(680.0)
        .width_range(600.0..=800.0)
        .resizable(true)
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                preset_panel::draw_preset_section(ui, s.preset_store);
                let layer_badge = format!("{}/{}", s.layers.len(), 8);
                widgets::section(
                    ui,
                    "v2_perform_layers",
                    "Layers",
                    Some(&layer_badge),
                    true,
                    |ui| {
                        layer_panel::draw_layer_panel(ui, s.layers, s.active_layer, s.master_chain);
                    },
                );
            });
        });

    egui::CentralPanel::default()
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                let w = (ui.available_width() - 20.0).max(160.0);
                if let Some((tex, aspect)) = s.display {
                    let h = (w / aspect.max(0.01)).round();
                    let h = h.min(ui.available_height() - 60.0).max(90.0);
                    let size = egui::vec2((h * aspect).round(), h);
                    ui.add(
                        egui::Image::new(egui::load::SizedTexture::new(tex, size))
                            .corner_radius(3.0),
                    );
                }
                ui.add_space(8.0);
                let bpm = s.uniforms.bpm * 300.0;
                if bpm > 1.0 {
                    ui.label(egui::RichText::new(format!("{bpm:.0} BPM")).size(14.0));
                }
                // Sending the output to a second display belongs with the
                // output wherever it is shown — including here, where there is
                // no section to put it in.
                ui.add_space(10.0);
                ui.scope(|ui| {
                    ui.set_max_width(360.0);
                    let ow = output_window_panel::read(ui.ctx()).unwrap_or_default();
                    output_window_panel::draw(ui, &ow);
                });
            });
        });
}

/// Setup: audio, control surfaces and preferences, at full width.
fn setup_workspace(ctx: &Context, s: &mut ShellState<'_>, fill: egui::Color32) {
    egui::SidePanel::left("v2_setup_preview")
        .exact_width(360.0)
        .resizable(false)
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            output_section(ui, "v2_setup_out", s.display);
        });

    egui::CentralPanel::default()
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                // Cards across the width rather than one tall stack: Setup is
                // the widest workspace and a single column wasted most of it.
                // One column per ~420 px, capped at three.
                let n = ((ui.available_width() / 420.0).floor() as usize).clamp(1, 3);
                let rec_info: Option<recording_panel::RecordingInfo> = ui
                    .ctx()
                    .data_mut(|d| d.get_temp(egui::Id::new("recording_info")));
                ui.columns(n, |cols| {
                    let col = |i: usize| i % n;

                    widgets::section(
                        &mut cols[col(0)],
                        "v2_setup_audio",
                        "Audio",
                        None,
                        true,
                        |ui| {
                            audio_panel::draw_audio_panel(ui, s.audio, s.uniforms);
                        },
                    );

                    let c = &mut cols[col(1)];
                    widgets::section(c, "v2_setup_midi", "MIDI", None, true, |ui| {
                        midi_panel::draw_midi_panel(ui, s.midi);
                    });
                    widgets::section(c, "v2_setup_osc", "OSC", None, true, |ui| {
                        osc_panel::draw_osc_panel(ui, s.osc);
                    });
                    let mapped = s.midi.config.triggers.len() + s.osc.config.triggers.len();
                    let trig_badge = (mapped > 0).then(|| format!("{mapped}"));
                    widgets::section(
                        c,
                        "v2_setup_triggers",
                        "Triggers",
                        trig_badge.as_deref(),
                        false,
                        |ui| {
                            triggers_panel::draw_triggers_table(ui, s.midi, s.osc);
                        },
                    );

                    let c = &mut cols[col(2)];
                    widgets::section(c, "v2_setup_web", "Web", None, false, |ui| {
                        web_panel::draw_web_panel(ui, s.web);
                    });
                    if let Some(ref info) = rec_info {
                        let badge = info.recording.then_some("REC");
                        widgets::section(c, "v2_setup_rec", "Recording", badge, false, |ui| {
                            recording_panel::draw_recording_panel(ui, info);
                        });
                    }
                    widgets::section(c, "v2_setup_global", "Global", None, true, |ui| {
                        settings_panel::draw_settings_panel(
                            ui,
                            s.settings.theme,
                            s.settings.particle_quality,
                            s.settings.band_scale,
                            s.settings.use_ffmpeg_webcam,
                            s.settings.auto_reconnect,
                            s.settings.output_alpha,
                            s.settings.classic_layout,
                        );
                    });
                });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(
                        "Streams (NDI, virtual camera, Spout, Syphon) and Scenes are still \
                         only in the Classic layout.",
                    )
                    .size(10.0)
                    .color(theme_colors(ui.ctx()).text_secondary),
                );
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay a parameter-style row out in the inspector's frame, in a window
    /// `w` wide. Returns (how far the row's right-hand button reaches past the
    /// column, how wide the row is).
    fn row_extent(w: f32) -> (f32, f32) {
        let ctx = Context::default();
        crate::ui::theme::colors::set_theme_colors(
            &ctx,
            crate::ui::theme::colors::ThemeColors::dark(),
        );
        let mut out = (0.0, 0.0);
        // A few passes: egui settles sizes over frames.
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(w, 900.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(panel_frame(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        let edge = ui.max_rect().right();
                        ScrollArea::vertical().show(ui, |ui| {
                            inspector_width(ui);
                            let mut v = 0.5f32;
                            let row = ui.horizontal(|ui| {
                                ui.label("splat_force");
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let o = ui.button("O");
                                        ui.spacing_mut().slider_width = ui.available_width();
                                        ui.add(egui::Slider::new(&mut v, 0.0..=1.0));
                                        o.rect.right()
                                    },
                                )
                                .inner
                            });
                            out = (row.inner - edge, row.response.rect.width());
                        });
                    });
            });
        }
        out
    }

    // Kevin narrowed the middle column and each parameter row's "O" button
    // went under its edge. A bare `set_max_width(720)` inside a ScrollArea
    // held the content at 720 px however narrow the column got. Put the bare
    // form back in `inspector_width` and the first assertion fails by exactly
    // what the column lacks.
    #[test]
    fn inspector_rows_fit_the_column_and_stop_at_a_reading_width() {
        for w in [900.0, 700.0, 500.0, 300.0] {
            let (past, _) = row_extent(w);
            assert!(
                past <= 0.5,
                "at {w} px the row reaches {past:.0} px past its column"
            );
        }
        let (_, width) = row_extent(1600.0);
        assert!(
            width <= INSPECTOR_MAX_WIDTH + 0.5,
            "in a wide column the row is {width:.0} px, past the {INSPECTOR_MAX_WIDTH} cap"
        );
    }
}
