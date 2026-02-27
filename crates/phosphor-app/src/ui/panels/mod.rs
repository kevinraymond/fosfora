pub mod audio_mappings_panel;
pub mod audio_panel;
pub mod effect_panel;
pub mod layer_panel;
pub mod media_panel;
pub mod midi_panel;
#[cfg(feature = "ndi")]
pub mod ndi_panel;
pub mod osc_panel;
pub mod param_panel;
pub mod preset_panel;
pub mod settings_panel;
pub mod shader_editor;
pub mod status_bar;
pub mod web_panel;
pub mod webcam_panel;

use egui::{Context, Frame, Margin, ScrollArea};

use crate::audio::AudioSystem;
use crate::effect::EffectLoader;
use crate::effect::format::PostProcessDef;
use crate::gpu::layer::LayerInfo;
use crate::gpu::ShaderUniforms;
use crate::midi::MidiSystem;
use crate::osc::OscSystem;
use crate::params::ParamStore;
use crate::preset::PresetStore;
use crate::ui::theme::ThemeMode;
use crate::ui::theme::colors::theme_colors;
use crate::web::WebSystem;
use crate::ui::widgets;

/// Draw all UI panels when overlay is visible.
pub fn draw_panels(
    ctx: &Context,
    visible: bool,
    audio: &mut AudioSystem,
    params: &mut ParamStore,
    shader_error: &Option<String>,
    uniforms: &ShaderUniforms,
    effect_loader: &EffectLoader,
    postprocess: &mut PostProcessDef,
    particle_count: Option<u32>,
    midi: &mut MidiSystem,
    osc: &mut OscSystem,
    web: &mut WebSystem,
    preset_store: &PresetStore,
    layers: &[LayerInfo],
    active_layer: usize,
    media_info: Option<media_panel::MediaInfo>,
    webcam_info: Option<webcam_panel::WebcamInfo>,
    status_error: &Option<(String, std::time::Instant)>,
    current_theme: ThemeMode,
) {
    if !visible {
        return;
    }

    let tc = theme_colors(ctx);

    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        let midi_port = midi.connected_port().unwrap_or("");
        let midi_active = midi.connected_port().is_some();
        let midi_recently_active = midi.is_recently_active();
        #[cfg(feature = "ndi")]
        let ndi_running = ctx.data_mut(|d| {
            d.get_temp::<bool>(egui::Id::new("ndi_running")).unwrap_or(false)
        });
        #[cfg(not(feature = "ndi"))]
        let ndi_running = false;
        status_bar::draw_status_bar(
            ui,
            shader_error,
            uniforms,
            particle_count,
            midi_port,
            midi_active,
            midi_recently_active,
            osc.config.enabled,
            osc.is_recently_active(),
            web.config.enabled,
            web.client_count,
            ndi_running,
            status_error,
        );
    });

    let panel_frame = Frame {
        fill: tc.panel,
        inner_margin: Margin::same(6),
        ..Default::default()
    };

    egui::SidePanel::left("left_panel")
        .exact_width(270.0)
        .resizable(false)
        .frame(panel_frame)
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                // Audio section
                let bpm = uniforms.bpm * 300.0;
                let bpm_badge = if bpm > 1.0 {
                    Some(format!("{:.0}", bpm))
                } else {
                    None
                };
                widgets::section(
                    ui,
                    "sec_audio",
                    "Audio",
                    bpm_badge.as_deref(),
                    true,
                    |ui| {
                        audio_panel::draw_audio_panel(ui, audio, uniforms);
                    },
                );

                // Effects section
                let fx_badge = format!("{}", effect_loader.effects.len());
                widgets::section(
                    ui,
                    "sec_effects",
                    "Effects",
                    Some(&fx_badge),
                    true,
                    |ui| {
                        effect_panel::draw_effect_panel(ui, effect_loader);
                    },
                );

                // Presets section
                let preset_badge = if preset_store.presets.is_empty() {
                    None
                } else {
                    Some(format!("{}", preset_store.presets.len()))
                };
                widgets::section(
                    ui,
                    "sec_presets",
                    "Presets",
                    preset_badge.as_deref(),
                    true,
                    |ui| {
                        preset_panel::draw_preset_panel(ui, preset_store);
                    },
                );

                // Layers section
                let layer_badge = format!("{}", layers.len());
                widgets::section(
                    ui,
                    "sec_layers",
                    "Layers",
                    Some(&layer_badge),
                    true,
                    |ui| {
                        layer_panel::draw_layer_panel(ui, layers, active_layer);
                    },
                );

                // MIDI section (default collapsed)
                let midi_badge = if !midi.config.enabled {
                    Some("OFF")
                } else if midi.connected_port().is_some() {
                    Some("ON")
                } else {
                    None
                };
                widgets::section(
                    ui,
                    "sec_midi",
                    "MIDI",
                    midi_badge,
                    false,
                    |ui| {
                        midi_panel::draw_midi_panel(ui, midi);
                    },
                );

                // OSC section (default collapsed)
                let osc_badge = if !osc.config.enabled {
                    Some("OFF")
                } else {
                    Some("ON")
                };
                widgets::section(
                    ui,
                    "sec_osc",
                    "OSC",
                    osc_badge,
                    false,
                    |ui| {
                        osc_panel::draw_osc_panel(ui, osc);
                    },
                );

                // Web section (default collapsed)
                let web_badge_text;
                let web_badge = if !web.config.enabled {
                    "OFF"
                } else if web.client_count > 0 {
                    web_badge_text = format!("{} client{}", web.client_count, if web.client_count == 1 { "" } else { "s" });
                    &web_badge_text
                } else {
                    "ON"
                };
                widgets::section(
                    ui,
                    "sec_web",
                    "Web",
                    Some(web_badge),
                    false,
                    |ui| {
                        web_panel::draw_web_panel(ui, web);
                    },
                );

                // NDI Outputs section (feature-gated, default collapsed)
                #[cfg(feature = "ndi")]
                {
                    let ndi_info: Option<ndi_panel::NdiInfo> = ui.ctx().data_mut(|d| {
                        d.remove_temp(egui::Id::new("ndi_info"))
                    });
                    if let Some(info) = ndi_info {
                        let ndi_badge = if info.running { "ON" } else { "OFF" };
                        widgets::section(
                            ui,
                            "sec_ndi",
                            "Outputs",
                            Some(ndi_badge),
                            false,
                            |ui| {
                                ndi_panel::draw_ndi_panel(ui, &info);
                            },
                        );
                    }
                }

                // Settings section (default collapsed)
                widgets::section(
                    ui,
                    "sec_settings",
                    "Settings",
                    None,
                    false,
                    |ui| {
                        settings_panel::draw_settings_panel(ui, current_theme);
                    },
                );
            });
        });

    egui::SidePanel::right("right_panel")
        .exact_width(270.0)
        .resizable(false)
        .frame(panel_frame)
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                if let Some(ref info) = webcam_info {
                    // Webcam layer: show webcam controls
                    widgets::section(
                        ui,
                        "sec_webcam",
                        "Webcam",
                        None,
                        true,
                        |ui| {
                            webcam_panel::draw_webcam_panel(ui, info);
                        },
                    );
                } else if let Some(ref info) = media_info {
                    // Media layer: show media controls instead of params
                    widgets::section(
                        ui,
                        "sec_media",
                        "Media",
                        None,
                        true,
                        |ui| {
                            media_panel::draw_media_panel(ui, info);
                        },
                    );
                } else {
                    // Effect layer: show parameters
                    widgets::section(
                        ui,
                        "sec_params",
                        "Parameters",
                        None,
                        true,
                        |ui| {
                            param_panel::draw_param_panel(ui, params, midi, osc);
                        },
                    );

                    // Audio Reactivity section (default collapsed)
                    let active_info = layers.get(active_layer);
                    let mappings = active_info
                        .and_then(|info| info.effect_index)
                        .and_then(|idx| effect_loader.effects.get(idx))
                        .map(|fx| fx.audio_mappings.as_slice())
                        .unwrap_or(&[]);
                    if !mappings.is_empty() {
                        let mapping_badge = format!("{}", mappings.len());
                        widgets::section(
                            ui,
                            "sec_audio_react",
                            "Audio Reactivity",
                            Some(&mapping_badge),
                            false,
                            |ui| {
                                audio_mappings_panel::draw_audio_mappings(ui, mappings);
                            },
                        );
                    }
                }

                // Post-Processing section
                widgets::section(
                    ui,
                    "sec_postprocess",
                    "Post-Processing",
                    None,
                    true,
                    |ui| {
                        ui.checkbox(&mut postprocess.enabled, "Enable");
                        let global_on = postprocess.enabled;

                        ui.add_space(4.0);

                        // Bloom
                        ui.add_enabled_ui(global_on, |ui| {
                            ui.checkbox(&mut postprocess.bloom_enabled, "Bloom");
                        });
                        ui.add_enabled_ui(global_on && postprocess.bloom_enabled, |ui| {
                            ui.indent("bloom_params", |ui| {
                                ui.horizontal(|ui| {
                                    ui.label("Threshold");
                                    ui.add(egui::Slider::new(&mut postprocess.bloom_threshold, 0.0..=1.5).show_value(true));
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Intensity");
                                    ui.add(egui::Slider::new(&mut postprocess.bloom_intensity, 0.0..=1.0).show_value(true));
                                });
                            });
                        });

                        ui.add_space(2.0);

                        // Chromatic Aberration
                        ui.add_enabled_ui(global_on, |ui| {
                            ui.checkbox(&mut postprocess.ca_enabled, "Chromatic Aberration");
                        });
                        ui.add_enabled_ui(global_on && postprocess.ca_enabled, |ui| {
                            ui.indent("ca_params", |ui| {
                                ui.horizontal(|ui| {
                                    ui.label("Intensity");
                                    ui.add(egui::Slider::new(&mut postprocess.ca_intensity, 0.0..=1.0).show_value(true));
                                });
                            });
                        });

                        ui.add_space(2.0);

                        // Vignette
                        ui.add_enabled_ui(global_on, |ui| {
                            ui.checkbox(&mut postprocess.vignette_enabled, "Vignette");
                        });
                        ui.add_enabled_ui(global_on && postprocess.vignette_enabled, |ui| {
                            ui.indent("vignette_params", |ui| {
                                ui.horizontal(|ui| {
                                    ui.label("Strength");
                                    ui.add(egui::Slider::new(&mut postprocess.vignette, 0.0..=1.0).show_value(true));
                                });
                            });
                        });

                        ui.add_space(2.0);

                        // Film Grain
                        ui.add_enabled_ui(global_on, |ui| {
                            ui.checkbox(&mut postprocess.grain_enabled, "Film Grain");
                        });
                        ui.add_enabled_ui(global_on && postprocess.grain_enabled, |ui| {
                            ui.indent("grain_params", |ui| {
                                ui.horizontal(|ui| {
                                    ui.label("Intensity");
                                    ui.add(egui::Slider::new(&mut postprocess.grain_intensity, 0.0..=1.0).show_value(true));
                                });
                            });
                        });
                    },
                );
            });
        });
}
