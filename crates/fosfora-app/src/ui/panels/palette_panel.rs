use egui::{RichText, Ui};

use crate::palette::import_adobe::import_adobe_url;
use crate::palette::types::{hex_to_rgba, rgba_to_hex, Palette};
use crate::show::controller::ShowController;

pub struct PalettePanelState {
    pub adobe_url: String,
    pub import_id: String,
    pub hex_edit: String,
    pub selected_slot: String,
}

impl Default for PalettePanelState {
    fn default() -> Self {
        Self {
            adobe_url: String::new(),
            import_id: "imported".into(),
            hex_edit: String::new(),
            selected_slot: "color-1".into(),
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
            ui.label(&palette.name);
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
                    }
                    if ui.small_button("sel").clicked() {
                        state.selected_slot = slot.clone();
                        state.hex_edit = rgba_to_hex(rgba);
                    }
                });
            }
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
