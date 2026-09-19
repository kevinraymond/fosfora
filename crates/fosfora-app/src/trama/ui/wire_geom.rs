//! Where a canvas wire actually runs, so something can be placed ON it.
//!
//! egui-snarl 0.9 draws and hit-tests its wires internally and exposes
//! neither: `SnarlViewer::show_wire_widget` exists in the trait but is never
//! called, and `hit_wire` sits in a private module. Its one wire affordance is
//! "right-click a hovered wire to remove it", which no one discovers. To hang
//! a remove button on a wire we need the curve, so this reproduces it.
//!
//! [`control_points`] and [`adjust_frame_size`] are egui-snarl 0.9.0's
//! `wire_bezier_5` and `adjust_frame_size` (`src/ui/wire.rs`, MIT OR
//! Apache-2.0), copied rather than approximated: the button has to sit on the
//! line that was drawn, not near it. The single deviation is the two
//! `unreachable!()` arms, which a NaN position reaches — every comparison
//! above them is false — and which here fall back to a straight run instead
//! of panicking the UI. If egui-snarl is upgraded and wires change shape, the
//! symptom is a button floating beside its wire; re-copy the function.

use egui::{Pos2, pos2};

/// A wire's curve in the canvas's own (untransformed) space.
pub struct WireCurve {
    points: [Pos2; 6],
}

/// Polyline resolution for distance queries. A wire is a gentle S; 24 chords
/// keep the polyline within a fraction of a pixel of it.
const SEGMENTS: usize = 24;

impl WireCurve {
    /// `from` is the output pin's center, `to` the input pin's, `frame_size`
    /// the style's wire frame size (snarl's default: three pin sizes).
    pub fn new(from: Pos2, to: Pos2, frame_size: f32) -> Self {
        // snarl's defaults: shrink the frame for short wires, never grow it.
        let frame_size = adjust_frame_size(frame_size, false, true, from, to);
        Self {
            points: control_points(frame_size, from, to),
        }
    }

    /// de Casteljau over the six control points.
    pub fn at(&self, t: f32) -> Pos2 {
        let mut p = self.points;
        for level in (1..p.len()).rev() {
            for i in 0..level {
                p[i] = p[i].lerp(p[i + 1], t);
            }
        }
        p[0]
    }

    /// Shortest distance from `point` to the curve.
    pub fn distance_to(&self, point: Pos2) -> f32 {
        let mut best = f32::INFINITY;
        let mut a = self.at(0.0);
        for i in 1..=SEGMENTS {
            let b = self.at(i as f32 / SEGMENTS as f32);
            best = best.min(distance_to_segment(point, a, b));
            a = b;
        }
        best
    }
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_sq();
    if len_sq <= f32::EPSILON {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

fn adjust_frame_size(
    mut frame_size: f32,
    upscale: bool,
    downscale: bool,
    from: Pos2,
    to: Pos2,
) -> f32 {
    let length = (from - to).length();
    if upscale {
        frame_size = frame_size.max(length / 6.0);
    }
    if downscale {
        frame_size = frame_size.min(length / 6.0);
    }
    frame_size
}

/// 5th degree bezier control points for a wire — egui-snarl's `wire_bezier_5`.
fn control_points(frame_size: f32, from: Pos2, to: Pos2) -> [Pos2; 6] {
    let from_norm_x = frame_size;
    let from_2 = pos2(from.x + from_norm_x, from.y);
    let to_norm_x = -from_norm_x;
    let to_2 = pos2(to.x + to_norm_x, to.y);

    let between = (from_2 - to_2).length();

    if from_2.x <= to_2.x && between >= frame_size * 2.0 {
        let middle_1 = from_2 + (to_2 - from_2).normalized() * frame_size;
        let middle_2 = to_2 + (from_2 - to_2).normalized() * frame_size;

        [from, from_2, middle_1, middle_2, to_2, to]
    } else if from_2.x <= to_2.x {
        let t = (between - (to_2.y - from_2.y).abs())
            / frame_size.mul_add(2.0, -(to_2.y - from_2.y).abs());

        let mut middle_1 = from_2 + (to_2 - from_2).normalized() * frame_size;
        let mut middle_2 = to_2 + (from_2 - to_2).normalized() * frame_size;

        if from_2.y >= to_2.y + frame_size {
            let u = (from_2.y - to_2.y - frame_size) / frame_size;

            let t0_middle_1 = pos2(
                (1.0 - u).mul_add(frame_size, from_2.x),
                frame_size.mul_add(-u, from_2.y),
            );
            let t0_middle_2 = pos2(to_2.x, to_2.y + frame_size);

            middle_1 = t0_middle_1.lerp(middle_1, t);
            middle_2 = t0_middle_2.lerp(middle_2, t);
        } else if from_2.y >= to_2.y {
            let u = (from_2.y - to_2.y) / frame_size;

            let t0_middle_1 = pos2(
                u.mul_add(frame_size, from_2.x),
                frame_size.mul_add(1.0 - u, from_2.y),
            );
            let t0_middle_2 = pos2(to_2.x, to_2.y + frame_size);

            middle_1 = t0_middle_1.lerp(middle_1, t);
            middle_2 = t0_middle_2.lerp(middle_2, t);
        } else if to_2.y >= from_2.y + frame_size {
            let u = (to_2.y - from_2.y - frame_size) / frame_size;

            let t0_middle_1 = pos2(from_2.x, from_2.y + frame_size);
            let t0_middle_2 = pos2(
                (1.0 - u).mul_add(-frame_size, to_2.x),
                frame_size.mul_add(-u, to_2.y),
            );

            middle_1 = t0_middle_1.lerp(middle_1, t);
            middle_2 = t0_middle_2.lerp(middle_2, t);
        } else if to_2.y >= from_2.y {
            let u = (to_2.y - from_2.y) / frame_size;

            let t0_middle_1 = pos2(from_2.x, from_2.y + frame_size);
            let t0_middle_2 = pos2(
                u.mul_add(-frame_size, to_2.x),
                frame_size.mul_add(1.0 - u, to_2.y),
            );

            middle_1 = t0_middle_1.lerp(middle_1, t);
            middle_2 = t0_middle_2.lerp(middle_2, t);
        }
        // else: a NaN position. Upstream is `unreachable!()`; keep the
        // straight run already in `middle_1`/`middle_2`.

        [from, from_2, middle_1, middle_2, to_2, to]
    } else if from_2.y >= frame_size.mul_add(2.0, to_2.y) {
        let middle_1 = pos2(from_2.x, from_2.y - frame_size);
        let middle_2 = pos2(to_2.x, to_2.y + frame_size);

        [from, from_2, middle_1, middle_2, to_2, to]
    } else if from_2.y >= to_2.y + frame_size {
        let t = (from_2.y - to_2.y - frame_size) / frame_size;

        let middle_1 = pos2(
            (1.0 - t).mul_add(frame_size, from_2.x),
            frame_size.mul_add(-t, from_2.y),
        );
        let middle_2 = pos2(to_2.x, to_2.y + frame_size);

        [from, from_2, middle_1, middle_2, to_2, to]
    } else if from_2.y >= to_2.y {
        let t = (from_2.y - to_2.y) / frame_size;

        let middle_1 = pos2(
            t.mul_add(frame_size, from_2.x),
            frame_size.mul_add(1.0 - t, from_2.y),
        );
        let middle_2 = pos2(to_2.x, to_2.y + frame_size);

        [from, from_2, middle_1, middle_2, to_2, to]
    } else if to_2.y >= frame_size.mul_add(2.0, from_2.y) {
        let middle_1 = pos2(from_2.x, from_2.y + frame_size);
        let middle_2 = pos2(to_2.x, to_2.y - frame_size);

        [from, from_2, middle_1, middle_2, to_2, to]
    } else if to_2.y >= from_2.y + frame_size {
        let t = (to_2.y - from_2.y - frame_size) / frame_size;

        let middle_1 = pos2(from_2.x, from_2.y + frame_size);
        let middle_2 = pos2(
            (1.0 - t).mul_add(-frame_size, to_2.x),
            frame_size.mul_add(-t, to_2.y),
        );

        [from, from_2, middle_1, middle_2, to_2, to]
    } else if to_2.y >= from_2.y {
        let t = (to_2.y - from_2.y) / frame_size;

        let middle_1 = pos2(from_2.x, from_2.y + frame_size);
        let middle_2 = pos2(
            t.mul_add(-frame_size, to_2.x),
            frame_size.mul_add(1.0 - t, to_2.y),
        );

        [from, from_2, middle_1, middle_2, to_2, to]
    } else {
        // A NaN position; upstream is `unreachable!()`.
        [from, from_2, from_2, to_2, to_2, to]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 27.0;

    fn close(a: Pos2, b: Pos2) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn a_wire_starts_and_ends_on_its_pins() {
        // Forward, backward, and steeply offset: every branch family.
        for (from, to) in [
            (pos2(0.0, 0.0), pos2(300.0, 40.0)),
            (pos2(300.0, 0.0), pos2(0.0, 40.0)),
            (pos2(0.0, 0.0), pos2(40.0, 200.0)),
            (pos2(0.0, 200.0), pos2(-40.0, 0.0)),
            (pos2(0.0, 0.0), pos2(10.0, 5.0)),
        ] {
            let c = WireCurve::new(from, to, FRAME);
            assert!(close(c.at(0.0), from), "{from:?} -> {to:?}");
            assert!(close(c.at(1.0), to), "{from:?} -> {to:?}");
        }
    }

    #[test]
    fn an_ordinary_wire_passes_through_the_midpoint_of_its_pins() {
        // Left-to-right with room to spare is point-symmetric about the
        // midpoint, so that is where the remove button lands.
        let (from, to) = (pos2(100.0, 50.0), pos2(400.0, 170.0));
        let c = WireCurve::new(from, to, FRAME);
        assert!(close(c.at(0.5), from.lerp(to, 0.5)));
    }

    #[test]
    fn distance_is_zero_on_the_wire_and_grows_off_it() {
        let c = WireCurve::new(pos2(0.0, 0.0), pos2(300.0, 120.0), FRAME);
        let on = c.at(0.37);
        assert!(c.distance_to(on) < 0.5, "a point of the curve is on it");
        // Leaves the output pin heading +x, so straight above the start is
        // off the wire by about that much.
        let d = c.distance_to(pos2(0.0, -40.0));
        assert!((39.0..=41.0).contains(&d), "got {d}");
    }

    #[test]
    fn a_nan_pin_position_does_not_panic() {
        // Upstream's `unreachable!()` arms are reachable exactly this way.
        let c = WireCurve::new(pos2(f32::NAN, 0.0), pos2(10.0, f32::NAN), FRAME);
        let _ = c.at(0.5);
        let _ = c.distance_to(pos2(0.0, 0.0));
    }
}
