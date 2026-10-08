//! Setup (#3237, Kevin's design in decision #3243): a list of pages down the
//! left, each with its state as a mark and words, and one page at a time
//! beside it. Every device and stream is a block with its state in words and
//! an On/Off switch over its settings.
//!
//! The pages send requests that `main.rs` handles, as the rest of the
//! interface does.

mod audio;
mod control;
pub mod kit;
mod more;
#[cfg(feature = "webcam")]
pub use more::StreamStatuses;
mod outputs;

use egui::{Context, RichText, ScrollArea, Sense, Ui, Vec2};

use super::shell::ShellState;
use super::theme::colors::theme_colors;
use super::widgets::{Mark, paint_mark_at};
use kit::Status;

/// A page of Setup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Audio,
    Control,
    Outputs,
    Sync,
    Appearance,
    Tutorials,
    General,
}

impl Page {
    pub const ALL: [Page; 7] = [
        Page::Audio,
        Page::Control,
        Page::Outputs,
        Page::Sync,
        Page::Appearance,
        Page::Tutorials,
        Page::General,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Page::Audio => "Audio",
            Page::Control => "Control",
            Page::Outputs => "Outputs and streams",
            Page::Sync => "Sync",
            Page::Appearance => "Appearance",
            Page::Tutorials => "Tutorials",
            Page::General => "General",
        }
    }

    fn desc(self) -> &'static str {
        match self {
            Page::Audio => {
                "What Fosfora listens to, what it hears in it, and the tempo it follows."
            }
            Page::Control => "MIDI, OSC and the web remote: ways to drive Fosfora from outside it.",
            Page::Outputs => {
                "Where the picture goes besides this window: a second display, a recording, \
                 and live streams to other apps."
            }
            Page::Sync => "Keep the beat with other software on the network.",
            Page::Appearance => "Theme and interface size.",
            Page::Tutorials => {
                "Guided tours that light one part of the window at a time. Esc or Skip tour \
                 ends one."
            }
            Page::General => "Performance, flash safety, cameras and layout.",
        }
    }
}

fn page_id() -> egui::Id {
    egui::Id::new("v2_setup_page")
}

/// The page Setup shows.
pub fn current(ctx: &Context) -> Page {
    ctx.data(|d| d.get_temp(page_id())).unwrap_or(Page::Audio)
}

/// Show `page` in Setup.
pub fn open(ctx: &Context, page: Page) {
    ctx.data_mut(|d| d.insert_temp(page_id(), page));
}

/// Each page's state for the list, where it has one.
fn page_status(ctx: &Context, s: &ShellState<'_>, page: Page) -> Option<Status> {
    match page {
        Page::Audio => Some(audio::status(s)),
        Page::Control => Some(control::summary(s)),
        Page::Outputs => Some(outputs::summary(ctx)),
        Page::Sync => Some(more::sync_status(ctx)),
        // Words only: these report a setting, not a state, so no mark.
        Page::Appearance => Some(Status::new(Mark::Off, s.settings.theme.display_name(&[]))),
        Page::Tutorials => {
            let done = super::tour::Tour::ALL
                .iter()
                .filter(|t| s.settings.tours_done.iter().any(|k| k == t.key()))
                .count();
            Some(Status::new(
                Mark::Off,
                format!("{done} of {} done", super::tour::Tour::ALL.len()),
            ))
        }
        Page::General => None,
    }
}

/// Setup: the list of pages, then the page. The output column is drawn by
/// the shell before this, so the central panel is what is left.
pub fn draw(ctx: &Context, s: &mut ShellState<'_>, fill: egui::Color32) {
    let page = current(ctx);
    egui::SidePanel::left("v2_setup_nav")
        .exact_width(250.0)
        .resizable(false)
        .frame(egui::Frame {
            fill,
            inner_margin: egui::Margin::same(10),
            ..Default::default()
        })
        .show(ctx, |ui| {
            let tc = theme_colors(ui.ctx());
            ui.add_space(4.0);
            ui.label(
                RichText::new("SETUP")
                    .size(12.0)
                    .strong()
                    .color(tc.text_secondary),
            );
            ui.add_space(8.0);
            for p in Page::ALL {
                let st = page_status(ctx, s, p);
                let shows_mark =
                    matches!(p, Page::Audio | Page::Control | Page::Outputs | Page::Sync);
                if nav_item(ui, p.title(), st.as_ref(), shows_mark, p == page).clicked() {
                    open(ui.ctx(), p);
                }
            }
            ui.add_space(14.0);
            kit::help(
                ui,
                "A filled dot is on and working, a ring is on but waiting, a dash is off. \
                 The words beside it say which.",
            );
        });

    egui::CentralPanel::default()
        .frame(egui::Frame {
            fill,
            inner_margin: egui::Margin::symmetric(24, 14),
            ..Default::default()
        })
        .show(ctx, |ui| {
            ScrollArea::vertical()
                .id_salt(("v2_setup_scroll", page as u8))
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(kit::PAGE_MAX_W));
                    kit::page_heading(ui, page.title(), page.desc());
                    match page {
                        Page::Audio => audio::page(ui, s),
                        Page::Control => control::page(ui, s),
                        Page::Outputs => outputs::page(ui, s),
                        Page::Sync => more::sync_page(ui),
                        Page::Appearance => more::appearance_page(ui, s),
                        Page::Tutorials => more::tutorials_page(ui, s),
                        Page::General => more::general_page(ui, s),
                    }
                    ui.add_space(24.0);
                });
        });
}

/// One row of the page list: a mark, the page's name, its state in words.
fn nav_item(
    ui: &mut Ui,
    title: &str,
    st: Option<&Status>,
    shows_mark: bool,
    selected: bool,
) -> egui::Response {
    let tc = theme_colors(ui.ctx());
    // Two lines: the name, and under it the state in words, which the
    // longest name leaves no room for beside it.
    let size = Vec2::new(ui.available_width(), 50.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let words = st.map(|s| s.words.as_str()).unwrap_or("");
    resp.widget_info(|| {
        let label = if words.is_empty() {
            title.to_string()
        } else {
            format!("{title}, {words}")
        };
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label)
    });
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let r = rect.height() / 2.0;
        let (text, sub) = if selected {
            p.rect_filled(rect, r, tc.selection);
            (tc.on_selection, tc.on_selection)
        } else {
            if resp.hovered() {
                p.rect_filled(rect, r, tc.hover_fill);
            }
            (tc.text_primary, tc.text_secondary)
        };
        if shows_mark && let Some(st) = st {
            // On the selection's fill every mark takes its text color, or
            // one drawn in the ink the fill is made of would vanish.
            let mut mc = tc;
            if selected {
                mc.success = text;
                mc.text_secondary = text;
                mc.text_dim = text;
                mc.warning = text;
                mc.error = text;
            }
            paint_mark_at(
                p,
                &mc,
                egui::pos2(rect.left() + 22.0, rect.center().y),
                st.mark,
                1.4,
            );
        }
        let font = |w: f32| egui::FontId::proportional(w);
        let x = rect.left() + 40.0;
        let one_line = words.is_empty();
        p.text(
            egui::pos2(
                x,
                if one_line {
                    rect.center().y
                } else {
                    rect.center().y - 8.0
                },
            ),
            egui::Align2::LEFT_CENTER,
            title,
            font(15.0),
            text,
        );
        if !one_line {
            let galley = p.layout(words.to_string(), font(12.0), sub, rect.right() - 14.0 - x);
            p.galley(egui::pos2(x, rect.center().y + 2.0), galley, sub);
        }
        if resp.has_focus() {
            p.rect_stroke(
                rect.shrink(1.0),
                r,
                egui::Stroke::new(2.0_f32, tc.text_primary),
                egui::StrokeKind::Inside,
            );
        }
    }
    resp
}

#[cfg(test)]
mod tests;
