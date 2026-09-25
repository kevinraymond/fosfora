use egui::{RichText, Ui};

use crate::palette::import_adobe::import_adobe_url;
use crate::palette::types::{
    hex_to_rgba, hsl_to_rgba, rgba_to_hex, rgba_to_hsl, Palette, DEFAULT_SLOTS,
};
use crate::show::controller::ShowController;

pub struct PalettePanelState {
    pub adobe_url: String,
    pub import_id: String,
    pub hex_edit: String,
    pub selected_slot: String,
    pub new_id: String,
    pub rename_to: String,
    pub new_slot: String,
}

impl Default for PalettePanelState {
    fn default() -> Self {
        Self {
            adobe_url: String::new(),
            import_id: "imported".into(),
            hex_edit: String::new(),
            selected_slot: "color-1".into(),
            new_id: String::new(),
            rename_to: String::new(),
            new_slot: String::new(),
        }
    }
}

pub enum PalettePanelAction {
    None,
    ApplyNow,
    Imported(Palette),
    Edited,
    PrevPalette,
    NextPalette,
    Created,
    Deleted,
    Duplicated,
}

pub fn draw_palette_panel(
    ui: &mut Ui,
    show: &mut ShowController,
    state: &mut PalettePanelState,
) -> PalettePanelAction {
    let mut action = PalettePanelAction::None;
    ui.label(RichText::new("PALETTES").size(11.0).strong());

    ui.horizontal(|ui| {
        if ui.small_button("◀ PREV").clicked() {
            action = PalettePanelAction::PrevPalette;
        }
        if ui.small_button("NEXT ▶").clicked() {
            action = PalettePanelAction::NextPalette;
        }
    });

    let ids: Vec<String> = show.palettes.keys().cloned().collect();
    for id in &ids {
        if ui.selectable_label(show.palette.active_palette_id.as_deref() == Some(id), id).clicked()
        {
            show.palette.manual_set(id.clone());
            action = PalettePanelAction::ApplyNow;
        }
    }

    if let Some(pid) = show.palette.active_palette_id.clone() {
        if let Some(palette) = show.palettes.get_mut(&pid) {
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(&palette.name);
                ui.text_edit_singleline(&mut state.rename_to);
                if ui.small_button("rename").clicked() && !state.rename_to.trim().is_empty() {
                    palette.name = state.rename_to.trim().to_string();
                    action = PalettePanelAction::Edited;
                }
            });
            let slots: Vec<(String, [f32; 4])> = palette
                .swatches
                .iter()
                .map(|s| (s.slot.clone(), s.rgba))
                .collect();
            for (slot, mut rgba) in slots {
                ui.horizontal(|ui| {
                    if ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed() {
                        palette.set_color(&slot, rgba);
                        action = PalettePanelAction::Edited;
                    }
                    ui.label(&slot);
                    if slot == state.selected_slot {
                        ui.label(rgba_to_hex(rgba));
                        let mut hsl = rgba_to_hsl(rgba);
                        let mut h_deg = hsl[0] * 360.0;
                        if ui
                            .add(
                                egui::DragValue::new(&mut h_deg)
                                    .range(0.0..=360.0)
                                    .prefix("H "),
                            )
                            .changed()
                        {
                            hsl[0] = (h_deg / 360.0).clamp(0.0, 1.0);
                            palette.set_color(&slot, hsl_to_rgba(hsl));
                            action = PalettePanelAction::Edited;
                        }
                        let mut s_pct = hsl[1] * 100.0;
                        if ui
                            .add(
                                egui::DragValue::new(&mut s_pct)
                                    .range(0.0..=100.0)
                                    .prefix("S "),
                            )
                            .changed()
                        {
                            hsl[1] = (s_pct / 100.0).clamp(0.0, 1.0);
                            palette.set_color(&slot, hsl_to_rgba(hsl));
                            action = PalettePanelAction::Edited;
                        }
                        let mut l_pct = hsl[2] * 100.0;
                        if ui
                            .add(
                                egui::DragValue::new(&mut l_pct)
                                    .range(0.0..=100.0)
                                    .prefix("L "),
                            )
                            .changed()
                        {
                            hsl[2] = (l_pct / 100.0).clamp(0.0, 1.0);
                            palette.set_color(&slot, hsl_to_rgba(hsl));
                            action = PalettePanelAction::Edited;
                        }
                        let mut a = rgba[3];
                        if ui
                            .add(egui::DragValue::new(&mut a).range(0.0..=1.0).prefix("A "))
                            .changed()
                        {
                            rgba[3] = a.clamp(0.0, 1.0);
                            palette.set_color(&slot, rgba);
                            action = PalettePanelAction::Edited;
                        }
                    }
                    if ui.small_button("sel").clicked() {
                        state.selected_slot = slot.clone();
                        state.hex_edit = rgba_to_hex(rgba);
                    }
                    // Swatch delete: IDs stay stable; validator flags bindings
                    // that still reference a deleted slot.
                    if ui.small_button("✕").clicked() {
                        palette.swatches.retain(|s| s.slot != slot);
                        action = PalettePanelAction::Edited;
                    }
                });
            }
            // Swatch add (slot ID, not array index) + reorder (swap by slot).
            ui.horizontal(|ui| {
                ui.label("slot");
                ui.text_edit_singleline(&mut state.new_slot);
                if ui.small_button("add swatch").clicked() && !state.new_slot.trim().is_empty() {
                    palette.set_color(state.new_slot.trim(), [0.5, 0.5, 0.5, 1.0]);
                    action = PalettePanelAction::Edited;
                }
            });
            ui.horizontal(|ui| {
                if ui.small_button("▲ move up").clicked() {
                    move_slot(palette, &state.selected_slot, -1);
                    action = PalettePanelAction::Edited;
                }
                if ui.small_button("▼ move down").clicked() {
                    move_slot(palette, &state.selected_slot, 1);
                    action = PalettePanelAction::Edited;
                }
            });
            ui.horizontal(|ui| {
                ui.label("HEX");
                if ui.text_edit_singleline(&mut state.hex_edit).lost_focus() {
                    if let Some(c) = hex_to_rgba(&state.hex_edit) {
                        palette.set_color(&state.selected_slot, c);
                        action = PalettePanelAction::Edited;
                    }
                }
            });
        }
    }

    // CRUD: create / duplicate / delete whole palettes (in the open pack).
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("new id");
        ui.text_edit_singleline(&mut state.new_id);
        if ui.small_button("create").clicked() && !state.new_id.trim().is_empty() {
            let id = state.new_id.trim().to_string();
            if !show.palettes.contains_key(&id) {
                let mut p = Palette {
                    schema_version: 1,
                    id: id.clone(),
                    name: id.clone(),
                    swatches: Vec::new(),
                };
                for slot in DEFAULT_SLOTS {
                    p.swatches.push(crate::palette::types::Swatch {
                        slot: (*slot).to_string(),
                        rgba: [0.5, 0.5, 0.5, 1.0],
                    });
                }
                show.palettes.insert(id.clone(), p);
                show.palette.manual_set(id.clone());
                action = PalettePanelAction::Created;
            }
        }
    });
    ui.horizontal(|ui| {
        if ui.small_button("duplicate active").clicked() {
            if let Some(pid) = show.palette.active_palette_id.clone() {
                let to = format!("{pid}-copy");
                if !show.palettes.contains_key(&to) {
                    if let Some(src) = show.palettes.get(&pid).cloned() {
                        let mut dup = src;
                        dup.id = to.clone();
                        dup.name = format!("{} copy", dup.name);
                        show.palettes.insert(to.clone(), dup);
                        action = PalettePanelAction::Duplicated;
                    }
                }
            }
        }
        if ui.small_button("delete active").clicked() {
            if let Some(pid) = show.palette.active_palette_id.clone() {
                let referenced = show
                    .definition
                    .palette_track
                    .iter()
                    .any(|c| c.palette_id == pid);
                if !referenced {
                    show.palettes.remove(&pid);
                    show.palette.manual_set(String::new());
                    action = PalettePanelAction::Deleted;
                } else {
                    log::warn!("cannot delete palette '{pid}': still on the palette track");
                }
            }
        }
    });

    ui.separator();
    ui.label("Adobe Color URL");
    ui.text_edit_singleline(&mut state.adobe_url);
    ui.text_edit_singleline(&mut state.import_id);
    if ui.button("Import").clicked() {
        match import_adobe_url(&state.adobe_url, &state.import_id) {
            Ok(p) => action = PalettePanelAction::Imported(p),
            Err(e) => log::warn!("Adobe import: {e}"),
        }
    }
    if ui.button("Apply Palette Now").clicked() {
        action = PalettePanelAction::ApplyNow;
    }

    action
}

/// Reorder swatches by stable slot ID (bindings follow the ID, not position).
fn move_slot(palette: &mut Palette, slot: &str, delta: i32) {
    let Some(i) = palette.swatches.iter().position(|s| s.slot == slot) else {
        return;
    };
    let j = (i as i32 + delta).clamp(0, palette.swatches.len() as i32 - 1) as usize;
    if i != j {
        palette.swatches.swap(i, j);
    }
}
