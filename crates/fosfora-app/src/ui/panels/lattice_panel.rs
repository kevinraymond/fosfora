//! Contextual controls for a Lattice (3D cellular-automata) effect.
//!
//! Shown only when the active layer's effect is a Lattice effect (its particle
//! system carries a `LatticeSim`). Modeled on `obstacle_panel`: it reads a
//! snapshot of the effect's `LatticeParams` and writes edits back as a
//! `LatticeCommand` via egui temp data, applied in `main.rs` after the UI pass.
//!
//! Layout: the live-performance controls (Rule, Grid, Reseed/Randomise) stay
//! always visible; everything else is grouped into collapsible subsections so
//! the ~28 controls don't read as one wall.

use egui::Ui;

use crate::gpu::lattice::{GRID_RES_CHOICES, LatticeParams, PRESET_RULES};
use crate::ui::theme::colors::theme_colors;
use crate::ui::widgets::{self, rows};

/// Snapshot of the active Lattice effect's params, collected before the UI borrow.
#[derive(Clone)]
pub struct LatticeInfo {
    pub params: LatticeParams,
    /// The effect's `.pfx` defaults, so "Reset to defaults" restores the preset's
    /// look rather than the hard-coded base defaults.
    pub defaults: LatticeParams,
}

/// Edit emitted by the panel, applied to the active effect in `main.rs`.
#[derive(Clone, Default)]
pub struct LatticeCommand {
    pub params: LatticeParams,
    /// Reseed the grid next frame (preset/init/grid change, or a button press).
    pub reseed: bool,
}

pub fn draw_lattice_panel(ui: &mut Ui, info: &LatticeInfo) {
    let tc = theme_colors(ui.ctx());
    let mut p = info.params;
    let mut changed = false;
    let mut reseed = false;

    // ── Always visible: the live-performance verbs ───────────────────

    // Rule preset.
    let preset_name = PRESET_RULES
        .get(p.rule_preset as usize)
        .map(|r| r.0)
        .unwrap_or("custom");
    let label = if p.manual_masks {
        "custom"
    } else {
        preset_name
    };
    rows::combo_row(ui, "lattice_preset", "Rule", None, label, |ui| {
        for (i, (name, _)) in PRESET_RULES.iter().enumerate() {
            let sel = !p.manual_masks && p.rule_preset == i as u32;
            if ui.selectable_label(sel, *name).clicked() {
                p.apply_preset(i as u32);
                changed = true;
                reseed = true;
            }
        }
    });

    // Grid resolution (rebuilds + reseeds on change).
    rows::combo_row(
        ui,
        "lattice_gridres",
        "Grid size",
        Some("Higher = finer structure, more GPU. Changing rebuilds and reseeds."),
        &format!("{}³", p.grid_res),
        |ui| {
            for r in GRID_RES_CHOICES {
                if ui
                    .selectable_value(&mut p.grid_res, r, format!("{r}³"))
                    .changed()
                {
                    changed = true;
                    reseed = true;
                }
            }
        },
    );

    ui.add_space(2.0);
    ui.horizontal(|ui| {
        if ui
            .button("Reseed")
            .on_hover_text("Restart the automaton from a fresh seed")
            .clicked()
        {
            reseed = true;
        }
        if ui
            .button("Randomise rule")
            .on_hover_text("Roll random birth/survival masks — expect chaos")
            .clicked()
        {
            p.randomize_rule();
            changed = true;
            reseed = true;
        }
    });

    // ── Simulation ───────────────────────────────────────────────────
    widgets::subsection(
        ui,
        "lattice_sim",
        "Simulation",
        None,
        tc.text_secondary,
        true,
        |ui| {
            changed |= rows::ParamRow::new("Speed (gen/s)")
                .tooltip("Generations per second under full audio drive")
                .show_slider(ui, &mut p.gen_per_sec, 0.0..=30.0)
                .changed;
            changed |= rows::ParamRow::new("Silence floor")
                .tooltip("Fraction of full speed kept during silence, so the field keeps breathing")
                .show_slider(ui, &mut p.bass_floor, 0.0..=1.0)
                .changed;
            changed |= rows::ParamRow::new("Smoothing")
                .tooltip("Display crossfade rate — low = dreamy fades, high = crisp steps")
                .show_slider(ui, &mut p.smooth_rate, 0.5..=40.0)
                .changed;
            changed |= rows::ParamRow::new("States")
                .tooltip("Cell lifetime states; >2 adds a dying-cell halo around live structure")
                .show_slider(ui, &mut p.num_states, 2..=20)
                .changed;
            changed |= rows::ParamRow::new("Lifetime")
                .tooltip(
                    "Generations a cell lives before it dies of old age. The main \
                     breathing knob: lower = faster turnover / thinner hollow shells, \
                     higher = denser structure. 0 = off (cells never age out).",
                )
                .show_slider(ui, &mut p.max_age, 0..=120)
                .changed;
            let nb = ["Moore (26)", "Von Neumann (6)"];
            let cur = (p.neighborhood.min(1)) as usize;
            rows::combo_row(ui, "lattice_nbhd", "Neighbourhood", None, nb[cur], |ui| {
                for (i, m) in nb.iter().enumerate() {
                    if ui
                        .selectable_value(&mut p.neighborhood, i as u32, *m)
                        .changed()
                    {
                        changed = true;
                    }
                }
            });
            let bnd = ["Wrap", "Dead border"];
            let cur = (p.boundary.min(1)) as usize;
            rows::combo_row(ui, "lattice_boundary", "Boundary", None, bnd[cur], |ui| {
                for (i, m) in bnd.iter().enumerate() {
                    if ui.selectable_value(&mut p.boundary, i as u32, *m).changed() {
                        changed = true;
                    }
                }
            });
        },
    );

    // ── Domain & seeding ─────────────────────────────────────────────
    widgets::subsection(
        ui,
        "lattice_seed",
        "Domain & seeding",
        None,
        tc.text_secondary,
        false,
        |ui| {
            let modes = ["Cube", "Sphere"];
            let cur = (p.domain_mode.min(1)) as usize;
            rows::combo_row(
                ui,
                "lattice_domain",
                "Domain",
                Some("Sphere kills the cube silhouette for growth rules"),
                modes[cur],
                |ui| {
                    for (i, m) in modes.iter().enumerate() {
                        if ui
                            .selectable_value(&mut p.domain_mode, i as u32, *m)
                            .changed()
                        {
                            changed = true;
                        }
                    }
                },
            );
            if p.domain_mode == 1 {
                changed |= rows::ParamRow::new("Domain radius")
                    .show_slider(ui, &mut p.domain_radius, 0.3..=1.0)
                    .changed;
            }
            changed |= rows::ParamRow::new("Audio dilation")
                .tooltip("Extra rule permissiveness on loud audio — 0 keeps rules distinct")
                .show_slider(ui, &mut p.dilation_max, 0..=2)
                .changed;
            changed |= rows::ParamRow::new("Chaos (flux)")
                .tooltip("Random cell perturbation driven by spectral flux")
                .show_slider(ui, &mut p.perturb_scale, 0.0..=0.2)
                .changed;

            ui.add_space(2.0);
            let modes = ["Random", "Center", "Multi-seed", "Clear", "Seed noise"];
            let cur = (p.init_mode.min(4)) as usize;
            rows::combo_row(
                ui,
                "lattice_initmode",
                "Init",
                Some("Seed shape on (re)start — growth rules need Seed noise to propagate"),
                modes[cur],
                |ui| {
                    for (i, m) in modes.iter().enumerate() {
                        if ui.selectable_label(cur == i, *m).clicked() {
                            p.init_mode = i as u32;
                            changed = true;
                            reseed = true;
                        }
                    }
                },
            );
            changed |= rows::ParamRow::new("Fill density")
                .show_slider(ui, &mut p.init_density, 0.01..=0.6)
                .changed;
            changed |= rows::ParamRow::new("Seed size")
                .show_slider(ui, &mut p.seed_size, 1..=32)
                .changed;
        },
    );

    // ── Look (shared R3 marcher) ─────────────────────────────────────
    widgets::subsection(
        ui,
        "lattice_look",
        "Look",
        Some("R3 marcher"),
        tc.text_secondary,
        false,
        |ui| {
            changed |= rows::ParamRow::new("Absorption")
                .show_slider(ui, &mut p.render.absorption, 0.05..=6.0)
                .changed;
            changed |= rows::ParamRow::new("Emission")
                .show_slider(ui, &mut p.render.emission_gain, 0.0..=4.0)
                .changed;
            changed |= rows::ParamRow::new("Detail amount")
                .show_slider(ui, &mut p.render.detail_strength, 0.0..=1.0)
                .changed;
            changed |= rows::ParamRow::new("Detail scale")
                .show_slider(ui, &mut p.render.detail_scale, 1.0..=8.0)
                .changed;
            changed |= rows::ParamRow::new("Age \u{2192} hue")
                .tooltip("Hue shift by cell age — separates young frontier from old core")
                .show_slider(ui, &mut p.render.age_influence, 0.0..=0.5)
                .changed;
            changed |= rows::ParamRow::new("Palette hue")
                .show_slider(ui, &mut p.render.palette_hue, 0.0..=1.0)
                .changed;
            changed |= rows::ParamRow::new("March steps")
                .tooltip("Ray-march quality vs GPU cost")
                .show_slider(ui, &mut p.render.march_steps, 16..=160)
                .changed;
        },
    );

    // ── Camera ───────────────────────────────────────────────────────
    widgets::subsection(
        ui,
        "lattice_camera",
        "Camera",
        None,
        tc.text_secondary,
        false,
        |ui| {
            changed |= rows::ParamRow::new("Cam distance")
                .show_slider(ui, &mut p.render.cam_distance, 1.0..=6.0)
                .changed;
            // Magnifies the structure without moving the camera into it — the reliable
            // way to fill the frame when the structure is small inside the domain.
            changed |= rows::ParamRow::new("Zoom (FOV)")
                .show_slider(ui, &mut p.render.fov, 1.0..=5.0)
                .changed;
            changed |= rows::ParamRow::new("Cam yaw")
                .show_slider(ui, &mut p.render.cam_yaw, 0.0..=std::f32::consts::TAU)
                .changed;
            changed |= rows::ParamRow::new("Cam pitch")
                .show_slider(ui, &mut p.render.cam_pitch, -1.2..=1.2)
                .changed;
            changed |= rows::ParamRow::new("Orbit speed")
                .show_slider(ui, &mut p.render.cam_orbit_speed, 0.0..=1.5)
                .changed;
        },
    );

    ui.add_space(6.0);
    if ui.button("Reset to defaults").clicked() {
        // Restore the effect's `.pfx` defaults (rule, look, domain — everything),
        // preserving only the current grid resolution to avoid a rebuild.
        let grid = p.grid_res;
        p = LatticeParams {
            grid_res: grid,
            ..info.defaults
        };
        changed = true;
        reseed = true;
    }

    if changed || reseed {
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("lattice_cmd"),
                LatticeCommand { params: p, reseed },
            );
        });
    }
}
