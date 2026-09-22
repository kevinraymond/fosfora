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
    audio_panel, effect_panel, layer_panel, media_panel, midi_panel, osc_panel, param_panel,
    postfx_panel, preset_panel, recording_panel, settings_panel, status_bar, triggers_panel,
    volumetric_panel, web_panel,
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

    fn read(ctx: &Context) -> Self {
        ctx.data_mut(|d| match d.get_temp::<u8>(egui::Id::new("v2_workspace")) {
            Some(0) => Workspace::Perform,
            Some(2) => Workspace::Setup,
            // Nothing stored yet, or a value from an older build: open on Build.
            _ => Workspace::Build,
        })
    }

    fn write(self, ctx: &Context) {
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
                widgets::section(ui, "v2_layers", "Layers", Some(&layer_badge), true, |ui| {
                    layer_panel::draw_layer_panel(ui, s.layers, s.active_layer, s.master_chain);
                });
                preset_panel::draw_preset_section(ui, s.preset_store);
                let fx_badge = format!("{}", s.effect_loader.effects.len());
                widgets::section(ui, "v2_catalog", "Catalog", Some(&fx_badge), true, |ui| {
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
                widgets::section(ui, "v2_preview", "Output", None, true, |ui| {
                    preview(ui, s.display);
                });
                let bpm = s.uniforms.bpm * 300.0;
                let bpm_badge = (bpm > 1.0).then(|| format!("{bpm:.0}"));
                widgets::section(ui, "v2_audio", "Audio", bpm_badge.as_deref(), true, |ui| {
                    audio_panel::draw_audio_panel(ui, s.audio, s.uniforms);
                });
            });
        });

    egui::CentralPanel::default()
        .frame(panel_frame(fill))
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                let name = s
                    .layers
                    .get(s.active_layer)
                    .map(|l| l.name.as_str())
                    .unwrap_or("No layer");
                ui.label(
                    egui::RichText::new(format!("Layer {}", s.active_layer + 1))
                        .size(11.0)
                        .color(theme_colors(ui.ctx()).text_secondary),
                );
                ui.label(egui::RichText::new(name).size(20.0).strong());
                ui.add_space(6.0);

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
                        widgets::section(
                            ui,
                            "v2_particles",
                            "Particles",
                            Some(&badge),
                            false,
                            |ui| {
                                super::panels::particle_panel::draw_particle_panel(ui, pinfo);
                            },
                        );
                    }
                }

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("MASTER")
                            .size(11.0)
                            .color(theme_colors(ui.ctx()).text_secondary),
                    );
                    // The matrix is a window, not a panel — a button is the
                    // whole control, so it does not need a section of its own.
                    let active = s.binding_bus.active_count();
                    let label = if active > 0 {
                        format!("Bindings · {active} active  (B)")
                    } else {
                        "Bindings  (B)".to_string()
                    };
                    if ui
                        .button(egui::RichText::new(label).size(11.0))
                        .on_hover_text("Open the binding matrix")
                        .clicked()
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("open_binding_matrix"), true);
                        });
                    }
                });
                // Side by side: neither is tall, and stacked they pushed the
                // layer's own parameters off the screen.
                ui.columns(2, |cols| {
                    widgets::section(
                        &mut cols[0],
                        "v2_postfx",
                        "Post-Processing",
                        None,
                        true,
                        |ui| {
                            postfx_panel::draw_postfx_panel(ui, s.postprocess);
                        },
                    );
                    widgets::section(
                        &mut cols[1],
                        "v2_volumetric",
                        "Volumetric (R3)",
                        None,
                        true,
                        |ui| {
                            volumetric_panel::draw_volumetric_panel(
                                ui,
                                s.volumetric_enabled,
                                s.volumetric_params,
                            );
                        },
                    );
                });
            });
        });
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
            widgets::section(ui, "v2_setup_out", "Output", None, true, |ui| {
                preview(ui, s.display);
            });
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
