//! Sending the output to a second display, in its own window (#3122).
//!
//! Both layouts draw this: the v1 Outputs subsection, next to Recording and
//! the streams, and the v2 Setup workspace under the output preview. The
//! window itself is created in `main.rs` — only the event loop can make one —
//! so a click here leaves a request in egui data for the frame loop to pick
//! up, the same route "Full output" takes.

use egui::Ui;

use crate::output_window::DisplayInfo;
use crate::ui::theme::colors::theme_colors;

/// What the frame knows about the second output window. Published into egui
/// data once per frame rather than passed in: neither layout's draw function
/// takes it as an argument, and `draw_panels` already carries thirty
/// positional parameters. `recording_info` travels the same way.
#[derive(Clone, Default)]
pub struct OutputWindowInfo {
    /// Every display the window system reports, in winit's order — which is
    /// the order `OutputWindow::open` indexes.
    pub displays: Vec<DisplayInfo>,
    /// The display the output window is open on, when one is open.
    pub open_on: Option<String>,
    /// The display it was last opened on, from settings, to seed the picker.
    pub last_display: Option<String>,
}

fn data_id() -> egui::Id {
    egui::Id::new("output_window_info")
}

pub fn publish(ctx: &egui::Context, info: OutputWindowInfo) {
    ctx.data_mut(|d| d.insert_temp(data_id(), info));
}

pub fn read(ctx: &egui::Context) -> Option<OutputWindowInfo> {
    ctx.data_mut(|d| d.get_temp::<OutputWindowInfo>(data_id()))
}

/// Requests this control leaves behind, read once per frame in `main.rs`.
pub fn take_open_request(ctx: &egui::Context) -> Option<usize> {
    ctx.data_mut(|d| d.remove_temp(egui::Id::new("request_output_window")))
}

pub fn take_close_request(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("request_close_output_window")))
        == Some(true)
}

pub fn draw(ui: &mut Ui, info: &OutputWindowInfo) {
    let tc = theme_colors(ui.ctx());

    if let Some(name) = &info.open_on {
        ui.label(egui::RichText::new(format!("On {name}")).size(11.0));
        if ui
            .button(egui::RichText::new("Close output window").size(11.0))
            .on_hover_text("Esc on that window closes it too")
            .clicked()
        {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("request_close_output_window"), true);
            });
        }
        return;
    }

    if info.displays.len() < 2 {
        ui.label(
            egui::RichText::new("One display — connect a second to send the output to it.")
                .size(10.0)
                .color(tc.text_secondary),
        );
        return;
    }

    // Which display is selected. Seeded from the one last used, by name, so a
    // reordered display list does not send the output somewhere else; failing
    // that, the second display, which is the one that is not usually the
    // desktop.
    let id = egui::Id::new("output_display_choice");
    let mut index: usize = ui.ctx().data_mut(|d| d.get_temp(id)).unwrap_or_else(|| {
        info.last_display
            .as_deref()
            .and_then(|name| info.displays.iter().position(|d| d.name == name))
            .unwrap_or(1)
    });
    index = index.min(info.displays.len() - 1);

    egui::ComboBox::from_id_salt("output_display_combo")
        .width((ui.available_width() - 8.0).max(120.0))
        .selected_text(egui::RichText::new(info.displays[index].label()).size(11.0))
        .show_ui(ui, |ui| {
            for (i, d) in info.displays.iter().enumerate() {
                ui.selectable_value(&mut index, i, egui::RichText::new(d.label()).size(11.0));
            }
        });
    ui.ctx().data_mut(|d| d.insert_temp(id, index));

    if ui
        .button(egui::RichText::new("Send output here").size(11.0))
        .on_hover_text("A borderless window on that display, with no interface on it")
        .clicked()
    {
        ui.ctx().data_mut(|d| {
            d.insert_temp(egui::Id::new("request_output_window"), index);
        });
    }
}
