//! A QR code painted straight onto the UI: dark modules as filled squares on a
//! white card, with the standard four-module quiet zone. No texture to upload or
//! keep in sync; it is a few hundred rects, drawn only while a panel shows it.

use egui::{Color32, Pos2, Rect, Response, Sense, Ui, Vec2, pos2, vec2};
use qrcodegen::{QrCode, QrCodeEcc};

/// Light modules around the code that scanners need to find its edges.
const QUIET_ZONE: i32 = 4;

/// Draw `text` as a QR code at most `size` points square; `None` (nothing drawn)
/// if it does not fit in a QR code. Modules are whole physical pixels, so the
/// code stays sharp at any UI scale; it may come out slightly under `size`.
pub fn qr_code(ui: &mut Ui, text: &str, size: f32) -> Option<Response> {
    let qr = QrCode::encode_text(text, QrCodeEcc::Medium).ok()?;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint(ui, &qr, rect);
    Some(response)
}

fn paint(ui: &Ui, qr: &QrCode, rect: Rect) {
    let n = qr.size();
    let cells = (n + 2 * QUIET_ZONE) as f32;
    let ppp = ui.ctx().pixels_per_point();
    let module = (rect.width() * ppp / cells).floor().max(1.0) / ppp;
    let side = module * cells;
    let snap = |p: Pos2| pos2((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp);
    let origin = snap(rect.center() - Vec2::splat(side / 2.0));

    let painter = ui.painter_at(rect);
    painter.rect_filled(
        Rect::from_min_size(origin, Vec2::splat(side)),
        0.0,
        Color32::WHITE,
    );
    let at = |x: i32, y: i32| origin + vec2(x as f32, y as f32) * module;
    for y in 0..n {
        // One rect per horizontal run of dark modules.
        let mut x = 0;
        while x < n {
            if !qr.get_module(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < n && qr.get_module(x, y) {
                x += 1;
            }
            painter.rect_filled(
                Rect::from_min_max(
                    at(start + QUIET_ZONE, y + QUIET_ZONE),
                    at(x + QUIET_ZONE, y + 1 + QUIET_ZONE),
                ),
                0.0,
                Color32::BLACK,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paint a code headlessly and read back which modules came out dark.
    fn painted(text: &str, size: f32, ppp: f32) -> (QrCode, Vec<Rect>) {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(ppp);
        let mut drawn = None;
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                drawn = qr_code(ui, text, size);
            });
        });
        assert!(drawn.is_some());
        let rects = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.fill == Color32::BLACK => Some(r.rect),
                _ => None,
            })
            .collect();
        (QrCode::encode_text(text, QrCodeEcc::Medium).unwrap(), rects)
    }

    #[test]
    fn every_dark_module_is_painted_and_no_light_one() {
        let url = "http://192.168.1.23:9002/?key=k7m2xq9fhr4tzw8bn3pa";
        for ppp in [1.0, 1.5, 2.0] {
            let (qr, rects) = painted(url, 160.0, ppp);
            let n = qr.size();
            let first = rects[0].min;
            let module = rects
                .iter()
                .map(|r| r.height())
                .fold(f32::INFINITY, f32::min);
            // Whole physical pixels per module.
            assert!(
                ((module * ppp).round() - module * ppp).abs() < 1e-3,
                "{module}"
            );
            // Module (0, 0) is a finder-pattern corner, always dark and painted
            // first, so its rect anchors the grid.
            let dark = |x: i32, y: i32| {
                let centre = first + vec2(x as f32 + 0.5, y as f32 + 0.5) * module;
                rects.iter().any(|r| r.contains(centre))
            };
            for y in 0..n {
                for x in 0..n {
                    assert_eq!(
                        dark(x, y),
                        qr.get_module(x, y),
                        "module ({x}, {y}) at {ppp}x"
                    );
                }
            }
        }
    }

    #[test]
    fn it_fits_the_space_given() {
        let (_, rects) = painted("http://10.0.0.2:9002/?key=k7m2xq9fhr4tzw8bn3pa", 120.0, 2.0);
        let bounds = rects.iter().fold(Rect::NOTHING, |b, r| b.union(*r));
        assert!(
            bounds.width() <= 120.0 && bounds.height() <= 120.0,
            "{bounds:?}"
        );
    }
}
