//! Post-processing chain controls (bloom / chromatic aberration / vignette /
//! film grain). Extracted from the `draw_panels` inline block; the per-effect
//! checkbox + indented-params structure is kept, with sliders on shared rows.

use egui::Ui;

use crate::effect::format::PostProcessDef;
use crate::ui::widgets::rows;

pub fn draw_postfx_panel(ui: &mut Ui, postprocess: &mut PostProcessDef) {
    ui.checkbox(&mut postprocess.enabled, "Enable");
    let global_on = postprocess.enabled;

    ui.add_space(4.0);

    // Bloom
    ui.add_enabled_ui(global_on, |ui| {
        ui.checkbox(&mut postprocess.bloom_enabled, "Bloom");
    });
    ui.add_enabled_ui(global_on && postprocess.bloom_enabled, |ui| {
        ui.indent("bloom_params", |ui| {
            rows::ParamRow::new("Threshold").show_slider(
                ui,
                &mut postprocess.bloom_threshold,
                0.0..=1.5,
            );
            rows::ParamRow::new("Intensity").show_slider(
                ui,
                &mut postprocess.bloom_intensity,
                0.0..=1.0,
            );
        });
    });

    ui.add_space(2.0);

    // Chromatic Aberration
    ui.add_enabled_ui(global_on, |ui| {
        ui.checkbox(&mut postprocess.ca_enabled, "Chromatic Aberration");
    });
    ui.add_enabled_ui(global_on && postprocess.ca_enabled, |ui| {
        ui.indent("ca_params", |ui| {
            rows::ParamRow::new("Intensity").show_slider(
                ui,
                &mut postprocess.ca_intensity,
                0.0..=1.0,
            );
        });
    });

    ui.add_space(2.0);

    // Vignette
    ui.add_enabled_ui(global_on, |ui| {
        ui.checkbox(&mut postprocess.vignette_enabled, "Vignette");
    });
    ui.add_enabled_ui(global_on && postprocess.vignette_enabled, |ui| {
        ui.indent("vignette_params", |ui| {
            rows::ParamRow::new("Strength").show_slider(ui, &mut postprocess.vignette, 0.0..=1.0);
        });
    });

    ui.add_space(2.0);

    // Film Grain
    ui.add_enabled_ui(global_on, |ui| {
        ui.checkbox(&mut postprocess.grain_enabled, "Film Grain");
    });
    ui.add_enabled_ui(global_on && postprocess.grain_enabled, |ui| {
        ui.indent("grain_params", |ui| {
            rows::ParamRow::new("Intensity").show_slider(
                ui,
                &mut postprocess.grain_intensity,
                0.0..=1.0,
            );
        });
    });
}
