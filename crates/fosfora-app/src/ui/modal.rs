//! Where the workspace's modals go (#3125): the binding matrix and the chain
//! editor cover Build's stack and inspector and stop at the output column, so
//! the output stays in view while they are open.

use egui::{Context, Id, Order, Rect, Sense};

use super::theme::colors::theme_colors;

const BOUNDS: &str = "v2_modal_bounds";

/// Record this frame's modal area. The shell calls it while drawing a
/// workspace that has an output column; a frame without a call has none.
pub fn set_bounds(ctx: &Context, rect: Rect) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(Id::new(BOUNDS), (pass, rect)));
}

/// The area recorded this frame, if the shell recorded one. Without one,
/// modals keep the whole window.
pub fn bounds(ctx: &Context) -> Option<Rect> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<(u64, Rect)>(Id::new(BOUNDS)))
        .filter(|(p, _)| *p == pass)
        .map(|(_, r)| r)
}

/// Dim `rect` and take the clicks that land on it, so nothing under a modal
/// reacts to them. The returned response is a click beside the modal.
pub fn backdrop(ctx: &Context, id: &str, rect: Rect) -> egui::Response {
    let tc = theme_colors(ctx);
    egui::Area::new(Id::new(id))
        .order(Order::Middle)
        .fixed_pos(rect.min)
        .constrain(false)
        .show(ctx, |ui| {
            let (r, resp) = ui.allocate_exact_size(rect.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, tc.backdrop);
            resp
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ctx: &Context, events: Vec<egui::Event>, f: impl FnMut(&Context)) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, f);
    }

    #[test]
    fn the_area_lasts_one_frame() {
        let ctx = Context::default();
        let r = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 500.0));
        let mut seen = None;
        run(&ctx, vec![], |ctx| {
            set_bounds(ctx, r);
            seen = bounds(ctx);
        });
        assert_eq!(seen, Some(r));
        // A frame whose workspace has no output column records none.
        run(&ctx, vec![], |ctx| seen = bounds(ctx));
        assert_eq!(seen, None);
    }

    /// A button under the backdrop must not take a click; one beside it must.
    #[test]
    fn the_backdrop_takes_the_clicks_under_it_and_only_those() {
        let covered = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 600.0));
        for (at, want_under, want_beside) in [
            (egui::pos2(100.0, 20.0), false, true),
            (egui::pos2(700.0, 20.0), true, false),
        ] {
            let ctx = Context::default();
            let (mut under, mut beside) = (false, false);
            let mut frame = |events| {
                run(&ctx, events, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let at = |x: f32| {
                            Rect::from_min_size(egui::pos2(x, 10.0), egui::vec2(80.0, 20.0))
                        };
                        under |= ui.put(at(60.0), egui::Button::new("left")).clicked();
                        under |= ui.put(at(660.0), egui::Button::new("right")).clicked();
                    });
                    beside |= backdrop(ctx, "t", covered).clicked();
                });
            };
            frame(vec![]);
            frame(vec![]);
            let b = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(vec![egui::Event::PointerMoved(at), b(true)]);
            frame(vec![b(false)]);
            assert_eq!(under, want_under, "button clicked at {at:?}");
            assert_eq!(beside, want_beside, "backdrop clicked at {at:?}");
        }
    }
}
