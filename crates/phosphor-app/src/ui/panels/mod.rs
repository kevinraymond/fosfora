pub mod audio_panel;
pub mod effect_panel;
pub mod midi_panel;
pub mod param_panel;
pub mod preset_panel;
pub mod status_bar;

use egui::Context;

use crate::audio::AudioSystem;
use crate::effect::EffectLoader;
use crate::gpu::ShaderUniforms;
use crate::midi::MidiSystem;
use crate::params::ParamStore;
use crate::preset::PresetStore;

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
    preset_store: &PresetStore,
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
        );
    });

    egui::SidePanel::left("left_panel")
        .default_width(280.0)
        .max_width(320.0)
        .show(ctx, |ui| {
            ui.heading("Audio");
            ui.separator();
            audio_panel::draw_audio_panel(ui, audio, uniforms);

            ui.add_space(16.0);
            ui.heading("Effects");
            ui.separator();
            effect_panel::draw_effect_panel(ui, effect_loader);

            ui.add_space(16.0);
            ui.heading("Presets");
            ui.separator();
            preset_panel::draw_preset_panel(ui, preset_store);

            ui.add_space(16.0);
            ui.heading("MIDI");
            ui.separator();
            midi_panel::draw_midi_panel(ui, midi);
        });

    egui::SidePanel::right("right_panel")
        .default_width(280.0)
        .show(ctx, |ui| {
            ui.heading("Parameters");
            ui.separator();
            param_panel::draw_param_panel(ui, params, midi);

            ui.add_space(16.0);
            ui.heading("Post-Processing");
            ui.separator();
            ui.checkbox(post_process_enabled, "Enable bloom & effects");
        });
}
