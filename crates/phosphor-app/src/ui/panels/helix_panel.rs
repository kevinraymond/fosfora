//! Contextual controls for a Helix (swept audio-history ribbon) effect.
//!
//! Shown only when the active layer's effect carries a `HelixSim`. Modeled on
//! `lattice_panel`: it reads a snapshot of the effect's `HelixParams` and writes
//! edits back as a `HelixCommand` via egui temp data, applied in `main.rs` after
//! the UI pass.
//!
//! **This panel is deliberately small.** Anything a VJ reaches for mid-set —
//! radius, thickness, twist, spectrum, wander, ripple, camera depth and zoom, hue,
//! timbre→hue, absorption, emission — is declared in the `.pfx` `inputs` instead,
//! so it lands in the Parameters panel and can be driven by MIDI / OSC / audio.
//! A contextual panel's state is not reachable from `apply_binding_target`, so
//! parking a performance knob here would quietly make it unbindable.
//!
//! What remains is what should NOT be on the binding bus: the two sizes that
//! reallocate GPU resources when they change, and the marcher internals you set
//! once and leave.

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

    ui.label(
        egui::RichText::new("Ribbon, camera and colour are in Parameters — bindable.")
            .small()
            .color(tc.text_secondary),
    );
    ui.add_space(4.0);

    // ── History window ───────────────────────────────────────────────
    // Both sizes reallocate on change (the volumes and the ring buffer), which is
    // why they are here and not on the binding bus.
    changed |= rows::ParamRow::new("Window (slices)")
        .tooltip(
            "Retained history. With the tick rate this sets how many seconds of the \
             track the ribbon spans. Reallocates the ring.",
        )
        .show_slider(ui, &mut p.slice_count, 16..=1024)
        .changed;
    changed |= rows::ParamRow::new("Tick rate (Hz)")
        .tooltip(
            "History ticks per second. Independent of frame rate, so the window is a \
             fixed number of seconds.",
        )
        .show_slider(ui, &mut p.slice_rate, 4.0..=120.0)
        .changed;
    ui.label(
        egui::RichText::new(format!(
            "≈ {:.1}s of history",
            p.slice_count as f32 / p.slice_rate.max(1.0)
        ))
        .small()
        .color(tc.text_secondary),
    );

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

    // ── Motion ───────────────────────────────────────────────────────
    widgets::subsection(
        ui,
        "helix_motion",
        "Motion",
        None,
        tc.text_secondary,
        false,
        |ui| {
            changed |= rows::ParamRow::new("Beat twist")
                .tooltip(
                    "Extra twist per second, scaled by beat phase — locks the corkscrew \
                     to tempo",
                )
                .show_slider(ui, &mut p.twist_rate, 0.0..=4.0)
                .changed;
            changed |= rows::ParamRow::new("Wander rate")
                .tooltip("Speed of the centreline's drift")
                .show_slider(ui, &mut p.wander_rate, 0.0..=1.0)
                .changed;
            changed |= rows::ParamRow::new("Look yaw")
                .tooltip("Turn away from straight-ahead")
                .show_slider(ui, &mut p.render.cam_yaw, -1.2..=1.2)
                .changed;
            changed |= rows::ParamRow::new("Look pitch")
                .show_slider(ui, &mut p.render.cam_pitch, -1.2..=1.2)
                .changed;
        },
    );

    // ── Marcher ──────────────────────────────────────────────────────
    widgets::subsection(
        ui,
        "helix_marcher",
        "Marcher",
        None,
        tc.text_secondary,
        false,
        |ui| {
            changed |= rows::ParamRow::new("Colour spread")
                .tooltip("Strength of the per-slice hue tint in the marcher")
                .show_slider(ui, &mut p.render.age_influence, 0.0..=1.5)
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
            changed |= rows::ParamRow::new("Density floor")
                .tooltip("Density below this is skipped by the marcher")
                .show_slider(ui, &mut p.render.density_threshold, 0.0..=0.2)
                .changed;
        },
    );

    ui.add_space(6.0);
    if ui.button("Reset to defaults").clicked() {
        // Restore the effect's `.pfx` defaults, preserving the current grid
        // resolution so the volumes are not reallocated by a reset. The
        // param-driven fields are not touched here — Parameters owns those, and
        // they would be overwritten from the slots on the next frame anyway.
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
