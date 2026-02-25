use egui::Ui;

use crate::preset::PresetStore;

pub fn draw_preset_panel(ui: &mut Ui, store: &PresetStore) {
    // Save row: text input + save button
    let mut name = ui
        .ctx()
        .data_mut(|d| d.get_temp::<String>(egui::Id::new("preset_save_name")))
        .unwrap_or_default();

    ui.horizontal(|ui| {
        let save_width = 40.0;
        let spacing = ui.spacing().item_spacing.x;
        let text_width = (ui.available_width() - save_width - spacing).max(1.0);
        let response = ui.add(
            egui::TextEdit::singleline(&mut name)
                .desired_width(text_width)
                .hint_text("Preset name..."),
        );
        if ui
            .add_enabled(!name.trim().is_empty(), egui::Button::new("Save"))
            .clicked()
            || (response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !name.trim().is_empty())
        {
            let trimmed = name.trim().to_string();
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("save_preset"), trimmed);
            });
            name.clear();
        }
    });

    ui.ctx()
        .data_mut(|d| d.insert_temp(egui::Id::new("preset_save_name"), name));

    if store.presets.is_empty() {
        ui.label("No presets saved");
        return;
    }

    ui.add_space(4.0);

    for (i, (name, _preset)) in store.presets.iter().enumerate() {
        let is_current = store.current_preset == Some(i);
        let text = if is_current {
            format!("> {name}")
        } else {
            name.clone()
        };

        ui.horizontal(|ui| {
            let delete_width = 18.0;
            let spacing = ui.spacing().item_spacing.x;
            let label_width = (ui.available_width() - delete_width - spacing).max(1.0);
            let response = ui.add_sized(
                [label_width, ui.spacing().interact_size.y],
                egui::SelectableLabel::new(is_current, &text),
            );
            if response.clicked() && !is_current {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("pending_preset"), i));
            }
            if ui.small_button("\u{00d7}").clicked() {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("delete_preset"), i));
            }
        });
    }
}
