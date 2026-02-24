pub mod audio_panel;
pub mod effect_panel;
pub mod param_panel;
pub mod status_bar;

use egui::Context;

use crate::audio::AudioSystem;
use crate::effect::EffectLoader;
use crate::gpu::ShaderUniforms;
use crate::params::ParamStore;

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
) {
    if !visible {
        return;
    }

    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        status_bar::draw_status_bar(ui, shader_error, uniforms, particle_count);
    });

    egui::SidePanel::left("left_panel")
        .default_width(280.0)
        .show(ctx, |ui| {
            ui.heading("Audio");
            ui.separator();
            audio_panel::draw_audio_panel(ui, audio, uniforms);

            ui.add_space(16.0);
            ui.heading("Effects");
            ui.separator();
            effect_panel::draw_effect_panel(ui, effect_loader);
        });

    egui::SidePanel::right("right_panel")
        .default_width(280.0)
        .show(ctx, |ui| {
            ui.heading("Parameters");
            ui.separator();
            param_panel::draw_param_panel(ui, params);

            ui.add_space(16.0);
            ui.heading("Post-Processing");
            ui.separator();
            ui.checkbox(post_process_enabled, "Enable bloom & effects");
        });
}
