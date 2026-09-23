use egui::{RichText, Ui};

use crate::show::clock::ClockTransport;
use crate::show::controller::ShowController;

pub fn format_hms(elapsed_ms: u64) -> String {
    let secs = elapsed_ms / 1000;
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

pub fn draw_show_panel(ui: &mut Ui, show: &ShowController, blackout: bool) -> ShowPanelAction {
    let mut action = ShowPanelAction::None;
    let now = std::time::Instant::now();
    let snap = show.snapshot(now, blackout);

    ui.label(RichText::new("SHOW").size(11.0).strong());
    ui.label(
        RichText::new(format_hms(snap.elapsed_ms))
            .size(22.0)
            .monospace(),
    );
    ui.label(RichText::new(snap.validation).size(10.0));
    ui.label(format!(
        "scene {} → {}",
        snap.scene.as_deref().unwrap_or("—"),
        snap.next_scene.as_deref().unwrap_or("—")
    ));
    ui.label(format!(
        "palette {} → {}",
        snap.palette.as_deref().unwrap_or("—"),
        snap.next_palette.as_deref().unwrap_or("—")
    ));

    ui.horizontal(|ui| {
        match show.clock.transport() {
            ClockTransport::Running => {
                if ui.button("PAUSE").clicked() {
                    action = ShowPanelAction::Pause;
                }
            }
            ClockTransport::Paused => {
                if ui.button("RESUME").clicked() {
                    action = ShowPanelAction::Resume;
                }
            }
            ClockTransport::Idle => {
                if ui.button("START SHOW").clicked() {
                    action = ShowPanelAction::Start;
                }
            }
        }
        if ui.button("STOP AUTO").clicked() {
            action = ShowPanelAction::StopAuto;
        }
        let bo = if blackout { "BLACKOUT ON" } else { "BLACKOUT" };
        if ui.button(bo).clicked() {
            action = ShowPanelAction::ToggleBlackout;
        }
    });

    ui.horizontal(|ui| {
        if ui.small_button("◀ PREV VISUAL").clicked() {
            action = ShowPanelAction::PrevVisual;
        }
        if ui.small_button("NEXT VISUAL ▶").clicked() {
            action = ShowPanelAction::NextVisual;
        }
        if ui.small_button("RESET").clicked() {
            action = ShowPanelAction::Reset;
        }
    });

    ui.horizontal(|ui| {
        ui.label("SEEK s");
        let mut secs = (snap.elapsed_ms / 1000) as u32;
        if ui
            .add(egui::DragValue::new(&mut secs).range(0..=show.definition.duration_secs))
            .changed()
        {
            action = ShowPanelAction::Seek(secs);
        }
    });

    action
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowPanelAction {
    None,
    Start,
    Pause,
    Resume,
    Reset,
    StopAuto,
    ToggleBlackout,
    Seek(u32),
    NextVisual,
    PrevVisual,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hms_format() {
        assert_eq!(format_hms(0), "00:00:00");
        assert_eq!(format_hms(1_847_000), "00:30:47");
        assert_eq!(format_hms(6_443_000), "01:47:23");
        assert_eq!(format_hms(10_800_000), "03:00:00");
    }
}
