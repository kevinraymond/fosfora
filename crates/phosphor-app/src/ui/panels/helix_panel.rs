//! Contextual controls for a Helix (swept audio-history ribbon) effect.
//!
//! Shown only when the active layer's effect carries a `HelixSim`. Modeled on
//! `lattice_panel`: it reads a snapshot of the effect's `HelixParams` and writes
//! edits back as a `HelixCommand` via egui temp data, applied in `main.rs` after
//! the UI pass.
//!
//! Layout: the controls that change what the ribbon IS (Ribbon) stay visible;
//! camera and look are collapsible.

use egui::Ui;

use crate::gpu::helix::{GRID_RES_CHOICES, HelixParams};
use crate::ui::theme::colors::theme_colors;
use crate::ui::widgets::{self, rows};

/// Snapshot of the active Helix effect's params, collected before the UI borrow.
#[derive(Clone)]
pub struct HelixInfo {
    pub params: HelixParams,
    /// The effect's `.pfx` defaults, so "Reset to defaults" restores the preset's
    /// look rather than the hard-coded base defaults.
    pub defaults: HelixParams,
}

/// Edit emitted by the panel, applied to the active effect in `main.rs`.
#[derive(Clone, Default)]
pub struct HelixCommand {
    pub params: HelixParams,
}

pub fn draw_helix_panel(ui: &mut Ui, info: &HelixInfo) {
    let tc = theme_colors(ui.ctx());
    let mut p = info.params;
    let mut changed = false;

    // ── Always visible: what the ribbon is ───────────────────────────
    changed |= rows::ParamRow::new("Radius")
        .tooltip("Cross-section calibre. Larger puts the wall further from the camera")
        .show_slider(ui, &mut p.radius, 0.1..=0.9)
        .changed;
    changed |= rows::ParamRow::new("Thickness")
        .tooltip("Shell thickness. Thin reads as a crisp ribbon; thick washes into fog")
        .show_slider(ui, &mut p.thickness, 0.005..=0.15)
        .changed;
    changed |= rows::ParamRow::new("Spectrum")
        .tooltip("How far the 7 bands deform the cross-section — 0 is a plain tube")
        .show_slider(ui, &mut p.spectrum_gain, 0.0..=1.5)
        .changed;
    changed |= rows::ParamRow::new("Twist")
        .tooltip("Corkscrew rate along the ribbon's length")
        .show_slider(ui, &mut p.twist_gain, -8.0..=8.0)
        .changed;

    // ── History ──────────────────────────────────────────────────────
    widgets::subsection(
        ui,
        "helix_history",
        "History",
        Some(&format!(
            "{:.1}s",
            p.slice_count as f32 / p.slice_rate.max(1.0)
        )),
        tc.text_secondary,
        false,
        |ui| {
            // Both of these reallocate the ring, so they are grouped away from
            // the live-performance controls above.
            changed |= rows::ParamRow::new("Window (slices)")
                .tooltip("Retained history. Combined with the tick rate this sets how many seconds of the track the ribbon spans")
                .show_slider(ui, &mut p.slice_count, 16..=1024)
                .changed;
            changed |= rows::ParamRow::new("Tick rate (Hz)")
                .tooltip("History ticks per second. Independent of frame rate, so the window is a fixed number of seconds")
                .show_slider(ui, &mut p.slice_rate, 4.0..=120.0)
                .changed;
            changed |= rows::ParamRow::new("Wander")
                .tooltip("How far the centreline drifts off the axis")
                .show_slider(ui, &mut p.wander, 0.0..=0.8)
                .changed;
            changed |= rows::ParamRow::new("Wander rate")
                .tooltip("Speed of the centreline's drift")
                .show_slider(ui, &mut p.wander_rate, 0.0..=1.0)
                .changed;
            changed |= rows::ParamRow::new("Beat twist")
                .tooltip(
                    "Extra twist per second, scaled by beat phase — locks the corkscrew to tempo",
                )
                .show_slider(ui, &mut p.twist_rate, 0.0..=4.0)
                .changed;
            changed |= rows::ParamRow::new("Ripple")
                .tooltip("Transient detail on the shell surface")
                .show_slider(ui, &mut p.ripple_gain, 0.0..=0.2)
                .changed;
        },
    );

    // ── Camera ───────────────────────────────────────────────────────
    widgets::subsection(
        ui,
        "helix_camera",
        "Camera",
        None,
        tc.text_secondary,
        false,
        |ui| {
            changed |= rows::ParamRow::new("Depth")
                .tooltip(
                    "Where along the ribbon the camera sits: +1 is the newest audio, -1 the oldest",
                )
                .show_slider(ui, &mut p.render.cam_distance, -0.95..=0.95)
                .changed;
            changed |= rows::ParamRow::new("Look yaw")
                .tooltip("Turn away from straight-ahead")
                .show_slider(ui, &mut p.render.cam_yaw, -1.2..=1.2)
                .changed;
            changed |= rows::ParamRow::new("Look pitch")
                .show_slider(ui, &mut p.render.cam_pitch, -1.2..=1.2)
                .changed;
            changed |= rows::ParamRow::new("Zoom")
                .tooltip("Higher is a narrower field of view")
                .show_slider(ui, &mut p.render.fov, 0.8..=4.0)
                .changed;
        },
    );

    // ── Look ─────────────────────────────────────────────────────────
    widgets::subsection(
        ui,
        "helix_look",
        "Look",
        None,
        tc.text_secondary,
        false,
        |ui| {
            changed |= rows::ParamRow::new("Hue")
                .show_slider(ui, &mut p.render.palette_hue, 0.0..=1.0)
                .changed;
            changed |= rows::ParamRow::new("Timbre → hue")
                .tooltip("How strongly the spectral centroid colours each moment of the ribbon")
                .show_slider(ui, &mut p.hue_spread, 0.0..=2.0)
                .changed;
            changed |= rows::ParamRow::new("Colour spread")
                .tooltip("Strength of the per-slice hue tint in the marcher")
                .show_slider(ui, &mut p.render.age_influence, 0.0..=1.5)
                .changed;
            changed |= rows::ParamRow::new("Absorption")
                .tooltip("Higher makes near material occlude far material — the main defence against flat fog")
                .show_slider(ui, &mut p.render.absorption, 0.2..=6.0)
                .changed;
            changed |= rows::ParamRow::new("Emission")
                .show_slider(ui, &mut p.render.emission_gain, 0.0..=3.0)
                .changed;
            changed |= rows::ParamRow::new("Detail scale")
                .tooltip("Frequency of the surface noise on the wall")
                .show_slider(ui, &mut p.render.detail_scale, 0.5..=20.0)
                .changed;
            changed |= rows::ParamRow::new("Detail strength")
                .show_slider(ui, &mut p.render.detail_strength, 0.0..=1.0)
                .changed;
            changed |= rows::ParamRow::new("March steps")
                .tooltip("Ray-march samples. Higher is smoother and costs more")
                .show_slider(ui, &mut p.render.march_steps, 16..=256)
                .changed;
        },
    );

    // ── Quality ──────────────────────────────────────────────────────
    rows::combo_row(
        ui,
        "helix_gridres",
        "Grid size",
        Some("Higher = finer volume, more GPU. Changing rebuilds the volumes."),
        &format!("{}³", p.grid_res),
        |ui| {
            for r in GRID_RES_CHOICES {
                if ui
                    .selectable_value(&mut p.grid_res, r, format!("{r}³"))
                    .changed()
                {
                    changed = true;
                }
            }
        },
    );

    ui.add_space(6.0);
    if ui.button("Reset to defaults").clicked() {
        // Restore the effect's `.pfx` defaults, preserving the current grid
        // resolution so the volumes are not reallocated by a reset.
        let grid = p.grid_res;
        p = HelixParams {
            grid_res: grid,
            ..info.defaults
        };
        changed = true;
    }

    if changed {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("helix_cmd"), HelixCommand { params: p });
        });
    }
}
