//! Volumetric (R3) mode controls — a global toggle applied to the active
//! particle layer, rendering it as fog/nebula instead of discrete dots.
//! Extracted from the `draw_panels` inline block; rows use the shared widgets.

use egui::Ui;

use crate::gpu::volumetric::VolumetricParams;
use crate::ui::theme::colors::theme_colors;
use crate::ui::widgets::{self, rows};

pub fn draw_volumetric_panel(ui: &mut Ui, enabled: &mut bool, p: &mut VolumetricParams) {
    let tc = theme_colors(ui.ctx());
    ui.checkbox(enabled, "Enable (active particle layer)");
    let vol_on = *enabled;
    ui.add_space(4.0);
    ui.add_enabled_ui(vol_on, |ui| {
        widgets::subsection(
            ui,
            "vol_volume",
            "Volume",
            None,
            tc.text_secondary,
            true,
            |ui| {
                rows::ParamRow::new("Density").show_slider(ui, &mut p.density_gain, 0.02..=1.0);
                rows::ParamRow::new("Absorption").show_slider(ui, &mut p.absorption, 0.05..=6.0);
                rows::ParamRow::new("Volume depth").show_slider(ui, &mut p.volume_depth, 0.0..=1.0);
                rows::ParamRow::new("Emission").show_slider(ui, &mut p.emission_gain, 0.0..=4.0);
                rows::ParamRow::new("Detail amount").show_slider(
                    ui,
                    &mut p.detail_strength,
                    0.0..=1.0,
                );
                rows::ParamRow::new("Detail scale").show_slider(ui, &mut p.detail_scale, 0.5..=8.0);
                rows::ParamRow::new("Palette hue").show_slider(ui, &mut p.palette_hue, 0.0..=1.0);
            },
        );
        widgets::subsection(
            ui,
            "vol_camera",
            "Camera",
            None,
            tc.text_secondary,
            false,
            |ui| {
                rows::ParamRow::new("Cam distance").show_slider(ui, &mut p.cam_distance, 1.5..=6.0);
                rows::ParamRow::new("Cam yaw").show_slider(
                    ui,
                    &mut p.cam_yaw,
                    0.0..=std::f32::consts::TAU,
                );
                rows::ParamRow::new("Cam pitch").show_slider(ui, &mut p.cam_pitch, -1.2..=1.2);
                rows::ParamRow::new("Orbit speed").show_slider(
                    ui,
                    &mut p.cam_orbit_speed,
                    0.0..=1.5,
                );
                rows::ParamRow::new("March steps")
                    .tooltip("Ray-march quality vs GPU cost")
                    .show_slider(ui, &mut p.march_steps, 16..=160);
            },
        );
        ui.add_space(6.0);
        if ui.button("Reset to defaults").clicked() {
            *p = VolumetricParams::default();
        }
    });
}
