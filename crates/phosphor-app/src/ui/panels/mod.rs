pub mod audio_mappings_panel;
pub mod audio_panel;
pub mod effect_panel;
pub mod layer_panel;
pub mod media_panel;
pub mod midi_panel;
#[cfg(feature = "ndi")]
pub mod ndi_panel;
pub mod obstacle_panel;
pub mod osc_panel;
pub mod param_panel;
pub mod particle_panel;
pub mod preset_panel;
pub mod scene_panel;
pub mod settings_panel;
pub mod shader_editor;
pub mod status_bar;
pub mod timeline_bar;
pub mod web_panel;
pub mod webcam_panel;

use egui::{Context, Frame, Margin, ScrollArea};

use crate::audio::AudioSystem;
use crate::effect::EffectLoader;
use crate::effect::format::PostProcessDef;
use crate::gpu::ShaderUniforms;
use crate::gpu::layer::LayerInfo;
use crate::midi::MidiSystem;
use crate::osc::OscSystem;
use crate::params::ParamStore;
use crate::preset::PresetStore;
use crate::settings::ParticleQuality;
use crate::ui::theme::ThemeMode;
use crate::ui::theme::colors::theme_colors;
use crate::ui::widgets;
use crate::web::WebSystem;

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
    particle_info: Option<particle_panel::ParticleInfo>,
    obstacle_info: Option<obstacle_panel::ObstacleInfo>,
    scene_info: Option<scene_panel::SceneInfo>,
    status_error: &Option<(String, std::time::Instant)>,
    current_theme: ThemeMode,
    particle_quality: ParticleQuality,
) {
    if !visible {
        return;
    }

    let tc = theme_colors(ctx);

    // Status bar must be drawn FIRST so it claims the bottom-most position.
    // Timeline bar draws second and sits directly above it.
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        let midi_recently_active = midi.is_recently_active();
        #[cfg(feature = "ndi")]
        let ndi_running = ctx.data_mut(|d| {
            d.get_temp::<bool>(egui::Id::new("ndi_running"))
                .unwrap_or(false)
        });
        #[cfg(not(feature = "ndi"))]
        let ndi_running = false;
        let preset_loading: Option<String> = ctx.data_mut(|d| {
            d.get_temp::<crate::preset::loader::PresetLoadingState>(egui::Id::new(
                "preset_loading_state",
            ))
            .and_then(|s| match s {
                crate::preset::loader::PresetLoadingState::Loading { preset_name, .. } => {
                    Some(preset_name)
                }
                _ => None,
            })
        });
        let scene_active = scene_info
            .as_ref()
            .and_then(|s| s.timeline.as_ref())
            .map_or(false, |t| t.active);
        let scene_cue = scene_info
            .as_ref()
            .and_then(|s| s.timeline.as_ref())
            .filter(|t| t.active)
            .map(|t| (t.current_cue, t.cue_count));
        status_bar::draw_status_bar(
            ui,
            shader_error,
            uniforms,
            particle_count,
            midi.config.enabled,
            midi_recently_active,
            osc.config.enabled,
            osc.is_recently_active(),
            web.config.enabled,
            web.client_count,
            ndi_running,
            scene_active,
            scene_cue,
            status_error,
            preset_loading.as_deref(),
        );
    });

    // Timeline bar (above status bar) — only when scene is active
    if let Some(ref scene) = scene_info {
        if let Some(ref tl) = scene.timeline {
            if tl.active {
                egui::TopBottomPanel::bottom("timeline_bar")
                    .exact_height(36.0)
                    .frame(Frame {
                        fill: tc.panel,
                        inner_margin: Margin::symmetric(2, 2),
                        ..Default::default()
                    })
                    .show(ctx, |ui| {
                        let cue_names: Vec<String> = scene
                            .cue_list
                            .iter()
                            .map(|c| c.preset_name.clone())
                            .collect();
                        timeline_bar::draw_timeline_bar(ui, tl, &cue_names);
                    });
            }
        }
    }

    let panel_frame = Frame {
        fill: tc.panel,
        inner_margin: Margin::same(6),
        ..Default::default()
    };

    egui::SidePanel::left("left_panel")
        .exact_width(315.0)
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
                widgets::section(ui, "sec_audio", "Audio", bpm_badge.as_deref(), true, |ui| {
                    audio_panel::draw_audio_panel(ui, audio, uniforms);
                });

                // Effects section
                let fx_badge = format!("{}", effect_loader.effects.len());
                widgets::section(ui, "sec_effects", "Effects", Some(&fx_badge), true, |ui| {
                    effect_panel::draw_effect_panel(ui, effect_loader);
                });

                // Layers section
                let layer_badge = format!("{}/{}", layers.len(), 8);
                widgets::section(ui, "sec_layers", "Layers", Some(&layer_badge), true, |ui| {
                    layer_panel::draw_layer_panel(ui, layers, active_layer);
                });

                // Presets section
                preset_panel::draw_preset_section(ui, preset_store);

                // Scenes section (default collapsed)
                if let Some(ref scene) = scene_info {
                    let scene_count = scene.scene_store_names.len();
                    let scene_badge_owned = format!("{}", scene_count);
                    let scene_badge: Option<&str> =
                        if scene.timeline.as_ref().map_or(false, |t| t.active) {
                            Some("LIVE")
                        } else if scene_count > 0 {
                            Some(&scene_badge_owned)
                        } else {
                            None
                        };
                    widgets::section(ui, "sec_scenes", "Scenes", scene_badge, false, |ui| {
                        scene_panel::draw_scene_panel(ui, scene);
                    });
                }

                // Consolidated Settings section
                let midi_on = midi.config.enabled && midi.connected_port().is_some();
                let osc_on = osc.config.enabled;
                let web_on = web.config.enabled;
                #[cfg(feature = "ndi")]
                let ndi_info: Option<ndi_panel::NdiInfo> = ui
                    .ctx()
                    .data_mut(|d| d.remove_temp(egui::Id::new("ndi_info")));
                #[cfg(feature = "ndi")]
                let ndi_on = ndi_info.as_ref().map_or(false, |i| i.running);

                let dot_active_midi = egui::Color32::from_rgb(0x60, 0xA0, 0xE0);
                let dot_active_osc = egui::Color32::from_rgb(0x50, 0xC0, 0x70);
                let dot_active_web = egui::Color32::from_rgb(0x50, 0x90, 0xE0);
                #[cfg(feature = "ndi")]
                let dot_active_ndi = egui::Color32::from_rgb(0x40, 0xC0, 0x40);
                let dot_off = egui::Color32::from_rgb(0x33, 0x33, 0x33);

                widgets::section_with_header(
                    ui,
                    "sec_settings",
                    "Settings",
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let dim_label = egui::Color32::from_white_alpha(38); // ~0.15
                        let on_label = egui::Color32::from_white_alpha(90); // ~0.35
                        // Helper: dot + tiny label (right-to-left order)
                        let mut status_dot = |ui: &mut egui::Ui,
                                              on: bool,
                                              color: egui::Color32,
                                              label: &str| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                let (r, _) = ui.allocate_exact_size(
                                    egui::vec2(4.0, 4.0),
                                    egui::Sense::hover(),
                                );
                                let c = if on { color } else { dot_off };
                                ui.painter().circle_filled(r.center(), 2.0, c);
                                ui.label(
                                    egui::RichText::new(label)
                                        .size(7.0)
                                        .color(if on { on_label } else { dim_label }),
                                );
                            });
                        };
                        // Drawn right-to-left, so reverse visual order
                        #[cfg(feature = "ndi")]
                        status_dot(ui, ndi_on, dot_active_ndi, "NDI");
                        status_dot(ui, web_on, dot_active_web, "WEB");
                        status_dot(ui, osc_on, dot_active_osc, "OSC");
                        status_dot(ui, midi_on, dot_active_midi, "MIDI");
                    },
                    false,
                    |ui| {
                        let tc = theme_colors(ui.ctx());
                        let dim = tc.text_secondary;

                        // MIDI subsection
                        let (midi_badge, midi_color) = if !midi.config.enabled {
                            (Some("OFF"), dim)
                        } else if midi.connected_port().is_some() {
                            (Some("ON"), dot_active_midi)
                        } else {
                            (None, dim)
                        };
                        widgets::subsection(
                            ui,
                            "sub_midi",
                            "MIDI",
                            midi_badge,
                            midi_color,
                            true,
                            |ui| {
                                midi_panel::draw_midi_panel(ui, midi);
                            },
                        );

                        // OSC subsection
                        let (osc_badge, osc_color) = if !osc.config.enabled {
                            (Some("OFF"), dim)
                        } else {
                            (Some("ON"), dot_active_osc)
                        };
                        widgets::subsection(
                            ui,
                            "sub_osc",
                            "OSC",
                            osc_badge,
                            osc_color,
                            true,
                            |ui| {
                                osc_panel::draw_osc_panel(ui, osc);
                            },
                        );

                        // Web subsection (default collapsed)
                        let web_badge_text;
                        let (web_badge, web_color) = if !web.config.enabled {
                            ("OFF", dim)
                        } else if web.client_count > 0 {
                            web_badge_text = format!(
                                "{} client{}",
                                web.client_count,
                                if web.client_count == 1 { "" } else { "s" }
                            );
                            (web_badge_text.as_str(), dot_active_web)
                        } else {
                            ("ON", dot_active_web)
                        };
                        widgets::subsection(
                            ui,
                            "sub_web",
                            "Web",
                            Some(web_badge),
                            web_color,
                            false,
                            |ui| {
                                web_panel::draw_web_panel(ui, web);
                            },
                        );

                        // NDI Outputs subsection (feature-gated)
                        #[cfg(feature = "ndi")]
                        {
                            if let Some(ref info) = ndi_info {
                                let (ndi_badge, ndi_color) = if info.running {
                                    ("ON", dot_active_ndi)
                                } else {
                                    ("OFF", dim)
                                };
                                widgets::subsection(
                                    ui,
                                    "sub_ndi",
                                    "Outputs",
                                    Some(ndi_badge),
                                    ndi_color,
                                    true,
                                    |ui| {
                                        ndi_panel::draw_ndi_panel(ui, info);
                                    },
                                );
                            }
                        }

                        // Global subsection
                        widgets::subsection(
                            ui,
                            "sub_global",
                            "Global",
                            None,
                            dim,
                            true,
                            |ui| {
                                settings_panel::draw_settings_panel(
                                    ui,
                                    current_theme,
                                    particle_quality,
                                );
                            },
                        );
                    },
                );
            });
        });

    egui::SidePanel::right("right_panel")
        .exact_width(315.0)
        .resizable(false)
        .frame(panel_frame)
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                if let Some(ref info) = webcam_info {
                    // Webcam layer: show webcam controls
                    widgets::section(ui, "sec_webcam", "Webcam", None, true, |ui| {
                        webcam_panel::draw_webcam_panel(ui, info);
                    });
                } else if let Some(ref info) = media_info {
                    // Media layer: show media controls instead of params
                    widgets::section(ui, "sec_media", "Media", None, true, |ui| {
                        media_panel::draw_media_panel(ui, info);
                    });
                } else {
                    // Effect layer: show parameters
                    widgets::section(ui, "sec_params", "Parameters", None, true, |ui| {
                        param_panel::draw_param_panel(ui, params, midi, osc);
                    });

                    // Particle section (shows when active layer has particles)
                    if let Some(ref pinfo) = particle_info {
                        let particle_badge = (if pinfo.alive_count >= 1000 {
                            format!("{:.1}K", pinfo.alive_count as f32 / 1000.0)
                        } else {
                            format!("{}", pinfo.alive_count)
                        })
                        .to_string();
                        widgets::section(
                            ui,
                            "sec_particles",
                            "Particles",
                            Some(&particle_badge),
                            true,
                            |ui| {
                                particle_panel::draw_particle_panel(ui, pinfo);
                            },
                        );
                    }

                    // Obstacle section (shows when active layer has particles)
                    if let Some(ref oinfo) = obstacle_info {
                        if oinfo.has_particles {
                            widgets::section(
                                ui,
                                "sec_obstacle",
                                "Obstacle",
                                if oinfo.enabled { Some("ON") } else { None },
                                false, // default collapsed
                                |ui| {
                                    obstacle_panel::draw_obstacle_panel(ui, oinfo);
                                },
                            );
                        }
                    }

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
                widgets::section(ui, "sec_postprocess", "Post-Processing", None, true, |ui| {
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
                                ui.add(
                                    egui::Slider::new(&mut postprocess.bloom_threshold, 0.0..=1.5)
                                        .show_value(true),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Intensity");
                                ui.add(
                                    egui::Slider::new(&mut postprocess.bloom_intensity, 0.0..=1.0)
                                        .show_value(true),
                                );
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
                                ui.add(
                                    egui::Slider::new(&mut postprocess.ca_intensity, 0.0..=1.0)
                                        .show_value(true),
                                );
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
                                ui.add(
                                    egui::Slider::new(&mut postprocess.vignette, 0.0..=1.0)
                                        .show_value(true),
                                );
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
                                ui.add(
                                    egui::Slider::new(&mut postprocess.grain_intensity, 0.0..=1.0)
                                        .show_value(true),
                                );
                            });
                        });
                    });
                });
            });
        });
}
