//! Contextual controls for a Lattice (3D cellular-automata) effect.
//!
//! Shown only when the active layer's effect is a Lattice effect (its particle
//! system carries a `LatticeSim`). Modeled on `obstacle_panel`: it reads a
//! snapshot of the effect's `LatticeParams` and writes edits back as a
//! `LatticeCommand` via egui temp data, applied in `main.rs` after the UI pass.

use egui::Ui;

use crate::gpu::lattice::{GRID_RES_CHOICES, LatticeParams, PRESET_RULES};

/// Snapshot of the active Lattice effect's params, collected before the UI borrow.
#[derive(Clone)]
pub struct LatticeInfo {
    pub params: LatticeParams,
}

/// Edit emitted by the panel, applied to the active effect in `main.rs`.
#[derive(Clone, Default)]
pub struct LatticeCommand {
    pub params: LatticeParams,
    /// Reseed the grid next frame (preset/init/grid change, or a button press).
    pub reseed: bool,
}

pub fn draw_lattice_panel(ui: &mut Ui, info: &LatticeInfo) {
    let mut p = info.params;
    let mut changed = false;
    let mut reseed = false;

    // Rule preset.
    ui.horizontal(|ui| {
        ui.label("Rule");
        let preset_name = PRESET_RULES
            .get(p.rule_preset as usize)
            .map(|r| r.0)
            .unwrap_or("custom");
        let label = if p.manual_masks {
            "custom"
        } else {
            preset_name
        };
        egui::ComboBox::from_id_salt("lattice_preset")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for (i, (name, _)) in PRESET_RULES.iter().enumerate() {
                    let sel = !p.manual_masks && p.rule_preset == i as u32;
                    if ui.selectable_label(sel, *name).clicked() {
                        p.apply_preset(i as u32);
                        changed = true;
                        reseed = true;
                    }
                }
            });
    });

    // Grid resolution (rebuilds + reseeds on change).
    ui.horizontal(|ui| {
        ui.label("Grid size");
        egui::ComboBox::from_id_salt("lattice_gridres")
            .selected_text(format!("{}³", p.grid_res))
            .show_ui(ui, |ui| {
                for r in GRID_RES_CHOICES {
                    if ui
                        .selectable_value(&mut p.grid_res, r, format!("{r}³"))
                        .changed()
                    {
                        changed = true;
                        reseed = true;
                    }
                }
            });
    });

    ui.horizontal(|ui| {
        ui.label("Speed (gen/s)");
        changed |= ui
            .add(egui::Slider::new(&mut p.gen_per_sec, 0.0..=30.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Silence floor");
        changed |= ui
            .add(egui::Slider::new(&mut p.bass_floor, 0.0..=1.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Smoothing");
        changed |= ui
            .add(egui::Slider::new(&mut p.smooth_rate, 0.5..=40.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("States");
        changed |= ui
            .add(egui::Slider::new(&mut p.num_states, 2..=20))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Neighbourhood");
        let nb = ["Moore (26)", "Von Neumann (6)"];
        let cur = (p.neighborhood.min(1)) as usize;
        egui::ComboBox::from_id_salt("lattice_nbhd")
            .selected_text(nb[cur])
            .show_ui(ui, |ui| {
                for (i, m) in nb.iter().enumerate() {
                    if ui
                        .selectable_value(&mut p.neighborhood, i as u32, *m)
                        .changed()
                    {
                        changed = true;
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Boundary");
        let bnd = ["Wrap", "Dead border"];
        let cur = (p.boundary.min(1)) as usize;
        egui::ComboBox::from_id_salt("lattice_boundary")
            .selected_text(bnd[cur])
            .show_ui(ui, |ui| {
                for (i, m) in bnd.iter().enumerate() {
                    if ui.selectable_value(&mut p.boundary, i as u32, *m).changed() {
                        changed = true;
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Domain");
        let modes = ["Cube", "Sphere"];
        let cur = (p.domain_mode.min(1)) as usize;
        egui::ComboBox::from_id_salt("lattice_domain")
            .selected_text(modes[cur])
            .show_ui(ui, |ui| {
                for (i, m) in modes.iter().enumerate() {
                    if ui
                        .selectable_value(&mut p.domain_mode, i as u32, *m)
                        .changed()
                    {
                        changed = true;
                    }
                }
            });
    });
    if p.domain_mode == 1 {
        ui.horizontal(|ui| {
            ui.label("Domain radius");
            changed |= ui
                .add(egui::Slider::new(&mut p.domain_radius, 0.3..=1.0))
                .changed();
        });
    }
    ui.horizontal(|ui| {
        ui.label("Audio dilation");
        changed |= ui
            .add(egui::Slider::new(&mut p.dilation_max, 0..=2))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Chaos (flux)");
        changed |= ui
            .add(egui::Slider::new(&mut p.perturb_scale, 0.0..=0.2))
            .changed();
    });

    ui.add_space(4.0);
    // Seeding / initialisation.
    ui.horizontal(|ui| {
        ui.label("Init");
        let modes = ["Random", "Center", "Multi-seed", "Clear", "Seed noise"];
        let cur = (p.init_mode.min(4)) as usize;
        egui::ComboBox::from_id_salt("lattice_initmode")
            .selected_text(modes[cur])
            .show_ui(ui, |ui| {
                for (i, m) in modes.iter().enumerate() {
                    if ui.selectable_label(cur == i, *m).clicked() {
                        p.init_mode = i as u32;
                        changed = true;
                        reseed = true;
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Fill density");
        changed |= ui
            .add(egui::Slider::new(&mut p.init_density, 0.01..=0.6))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Seed size");
        changed |= ui
            .add(egui::Slider::new(&mut p.seed_size, 1..=32))
            .changed();
    });
    ui.horizontal(|ui| {
        if ui.button("Reseed").clicked() {
            reseed = true;
        }
        if ui.button("Randomise rule").clicked() {
            p.randomize_rule();
            changed = true;
            reseed = true;
        }
    });

    ui.add_space(6.0);
    ui.label("Look (shared R3 marcher)");
    ui.horizontal(|ui| {
        ui.label("Absorption");
        changed |= ui
            .add(egui::Slider::new(&mut p.render.absorption, 0.05..=6.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Emission");
        changed |= ui
            .add(egui::Slider::new(&mut p.render.emission_gain, 0.0..=4.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Detail amount");
        changed |= ui
            .add(egui::Slider::new(&mut p.render.detail_strength, 0.0..=1.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Detail scale");
        changed |= ui
            .add(egui::Slider::new(&mut p.render.detail_scale, 1.0..=8.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Age → hue");
        changed |= ui
            .add(egui::Slider::new(&mut p.render.age_influence, 0.0..=0.5))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Palette hue");
        changed |= ui
            .add(egui::Slider::new(&mut p.render.palette_hue, 0.0..=1.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Cam distance");
        changed |= ui
            .add(egui::Slider::new(&mut p.render.cam_distance, 1.5..=6.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Cam yaw");
        changed |= ui
            .add(egui::Slider::new(
                &mut p.render.cam_yaw,
                0.0..=std::f32::consts::TAU,
            ))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Cam pitch");
        changed |= ui
            .add(egui::Slider::new(&mut p.render.cam_pitch, -1.2..=1.2))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Orbit speed");
        changed |= ui
            .add(egui::Slider::new(&mut p.render.cam_orbit_speed, 0.0..=1.5))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("March steps");
        changed |= ui
            .add(egui::Slider::new(&mut p.render.march_steps, 16..=160))
            .changed();
    });

    ui.add_space(6.0);
    if ui.button("Reset to defaults").clicked() {
        // Preserve the grid resolution (a rebuild is expensive); reset everything else.
        let grid = p.grid_res;
        p = LatticeParams {
            grid_res: grid,
            ..LatticeParams::default()
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
