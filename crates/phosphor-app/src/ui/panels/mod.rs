pub mod audio_panel;
pub mod effect_panel;
pub mod layer_panel;
pub mod midi_panel;
pub mod osc_panel;
pub mod param_panel;
pub mod preset_panel;
pub mod status_bar;

use egui::{Context, Frame, Margin, ScrollArea};

use crate::audio::AudioSystem;
use crate::effect::EffectLoader;
use crate::gpu::layer::LayerInfo;
use crate::gpu::ShaderUniforms;
use crate::midi::MidiSystem;
use crate::osc::OscSystem;
use crate::params::ParamStore;
use crate::preset::PresetStore;
use crate::ui::theme::tokens::*;
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
    post_process_enabled: &mut bool,
    particle_count: Option<u32>,
    midi: &mut MidiSystem,
    osc: &mut OscSystem,
    preset_store: &PresetStore,
    layers: &[LayerInfo],
    active_layer: usize,
) {
    if !visible {
        return;
    }

    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        let midi_port = midi.connected_port().unwrap_or("");
        let midi_active = midi.connected_port().is_some();
        let midi_recently_active = midi.is_recently_active();
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
        );
    });

    let panel_frame = Frame {
        fill: DARK_PANEL,
        inner_margin: Margin::same(6),
        ..Default::default()
    };

    egui::SidePanel::left("left_panel")
        .default_width(260.0)
        .max_width(400.0)
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
            });
        });

    egui::SidePanel::right("right_panel")
        .default_width(280.0)
        .frame(panel_frame)
        .show(ctx, |ui| {
            ScrollArea::vertical().show(ui, |ui| {
                // Parameters section
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

                // Post-Processing section
                widgets::section(
                    ui,
                    "sec_postprocess",
                    "Post-Processing",
                    None,
                    true,
                    |ui| {
                        ui.checkbox(post_process_enabled, "Enable bloom & effects");
                    },
                );
            });
        });
}
