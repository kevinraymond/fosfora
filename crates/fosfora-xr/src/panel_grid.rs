//! Where the hand menu's and the debug panel's controls sit on the panel
//! texture, and which one the pointer is on (board #3402). Plain numbers in
//! egui points (y down, the texture's top-left at the origin), so the
//! layout and its hit test build and test on the desktop; `hud.rs`, which
//! paints them with egui, is Android-only.
//!
//! The controls fill a block at the bottom of the part of the texture the
//! quad shows (the whole panel with debug on, the menu strip without), the
//! bottom row [`MARGIN`] above that edge in both, so the row under the
//! pointer stays there when debug toggles. Rows are [`ROW_H`] tall with
//! [`ROW_GAP`] between them. A row is wide (one action, or a left and a
//! right one: Prev/Next) or a pair of cells, one per column, each about
//! half the width with its own -/+ end boxes ([`END_BOX_W`]) or one
//! action. The pointer picks a row by its height alone (a row owns half
//! the gap above and below it, so the rows tile the block), a column by
//! the side of the panel's center it is on, and a half of the cell by the
//! side of the cell's center.

/// Texture size (pixels) and egui scale: 3200 px per meter on the 20 x 46
/// cm panel, about the display's density at arm's length.
pub const TEX_W: u32 = 640;
pub const TEX_H: u32 = 1472;
pub const PIXELS_PER_POINT: f32 = 1.6;
/// The texture in points.
pub const PANEL_W: f32 = TEX_W as f32 / PIXELS_PER_POINT;
pub const PANEL_H: f32 = TEX_H as f32 / PIXELS_PER_POINT;
/// The panel frame's inner margin (points; 1 point = 0.5 mm on the panel).
pub const MARGIN: f32 = 10.0;
/// Control rows: height, the gap between rows and between the two columns.
pub const ROW_H: f32 = 34.0;
pub const ROW_GAP: f32 = 6.0;
pub const COL_GAP: f32 = 6.0;
/// The -/+ and Prev/Next boxes at a cell's or a row's ends.
pub const END_BOX_W: f32 = 40.0;
/// The hand menu's rows with Edit room off: the world effect's `<` `>`
/// row on top (board #3336: the effect cycle left the bare pinch-hold),
/// then, in both layouts at the bottom of the block, the pitcher and debug
/// toggles over the room editor's (board #3326), over the cloud's (step
/// 2c), over the music's (board #3472). The debug panel shows the effect
/// row at the top of its block, over the steppers.
pub const MENU_ROWS: usize = 5;
/// The rows Edit room adds under its own (board #3472, D3): the pointed
/// surface's color, band and strength.
pub const SURFACE_ROWS: usize = 3;

/// The hand menu's rows: [`MENU_ROWS`], and the [`SURFACE_ROWS`] with
/// them while Edit room is on.
pub const fn menu_rows(editing: bool) -> usize {
    if editing {
        MENU_ROWS + SURFACE_ROWS
    } else {
        MENU_ROWS
    }
}

/// The hand menu's height (points) with `rows` rows: the top of the
/// texture the quad shows with debug off, a title over the menu rows. The
/// quad keeps its bottom edge, so more rows grow it upward.
pub const fn menu_h(rows: usize) -> f32 {
    84.0 + (rows - 1) as f32 * (ROW_H + ROW_GAP)
}
/// Fonts (points): the title, a stepper's value and label, a button, the
/// end boxes' glyphs, the graph's labels, and the smallest anywhere on the
/// panel (legible at arm's length). The header lines are egui's body text
/// (12.5).
pub const FONT_TITLE: f32 = 15.0;
pub const FONT_VALUE: f32 = 16.0;
pub const FONT_LABEL: f32 = 12.0;
pub const FONT_BUTTON: f32 = 16.0;
pub const FONT_END: f32 = 20.0;
pub const FONT_GRAPH: f32 = 12.0;
pub const MIN_FONT: f32 = 12.0;
/// The height the debug panel's header (the title, timing, the graph,
/// hands, reach, gesture, anchor and audio) needs above the controls, with
/// room to spare: about 250 points in the texture dumps.
pub const HEADER_H: f32 = 320.0;
/// The debug panel's -/+ steppers (`STEPPERS` in `hud.rs`, which checks
/// the count), two to a row.
pub const STEPPERS: usize = 10;
/// The most rows the debug panel shows: the menu's effect row (Prev/Next)
/// at the top, the steppers' pair rows, Recenter and Rescan, then the
/// menu's other rows (Pitcher and Debug, Edit room and its status, with
/// Edit room on the surface's Color, Band and Strength, Particles and All:
/// none, Music).
pub const fn debug_rows(editing: bool) -> usize {
    STEPPERS.div_ceil(2) + 1 + menu_rows(editing)
}

// A stepper's label over its value in one row, and the menu's title over
// its top row, in either menu.
const _: () = assert!(FONT_LABEL + FONT_VALUE <= ROW_H - 4.0);
const _: () = assert!(
    MARGIN + FONT_TITLE * 1.4 + menu_rows(false) as f32 * (ROW_GAP + ROW_H) + MARGIN
        <= menu_h(menu_rows(false))
);
const _: () = assert!(
    MARGIN + FONT_TITLE * 1.4 + menu_rows(true) as f32 * (ROW_GAP + ROW_H) + MARGIN
        <= menu_h(menu_rows(true))
);
// The debug panel's header over its fullest control block: Edit room on.
const _: () = assert!(
    PANEL_H - MARGIN - debug_rows(true) as f32 * (ROW_H + ROW_GAP) + ROW_GAP * 0.5
        >= MARGIN + HEADER_H
);

/// An axis-aligned rectangle in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub min: [f32; 2],
    pub max: [f32; 2],
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self {
            min: [x0, y0],
            max: [x1, y1],
        }
    }

    pub fn width(&self) -> f32 {
        self.max[0] - self.min[0]
    }

    pub fn height(&self) -> f32 {
        self.max[1] - self.min[1]
    }

    pub fn center_x(&self) -> f32 {
        (self.min[0] + self.max[0]) * 0.5
    }

    /// The left and the right half.
    pub fn halves(&self) -> (Self, Self) {
        let x = self.center_x();
        (
            Self::new(self.min[0], self.min[1], x, self.max[1]),
            Self::new(x, self.min[1], self.max[0], self.max[1]),
        )
    }

    /// The end boxes: [`END_BOX_W`] at the left and the right end.
    pub fn end_boxes(&self) -> (Self, Self) {
        (
            Self::new(
                self.min[0],
                self.min[1],
                self.min[0] + END_BOX_W,
                self.max[1],
            ),
            Self::new(
                self.max[0] - END_BOX_W,
                self.min[1],
                self.max[0],
                self.max[1],
            ),
        )
    }

    /// Between the end boxes.
    #[must_use]
    pub fn middle(&self) -> Self {
        Self::new(
            self.min[0] + END_BOX_W,
            self.min[1],
            self.max[0] - END_BOX_W,
            self.max[1],
        )
    }
}

/// The row `k` from the bottom of a block whose visible part ends at
/// `bottom` (points): full width inside the margins.
pub fn row(bottom: f32, k: usize) -> Rect {
    let y1 = bottom - MARGIN - k as f32 * (ROW_H + ROW_GAP);
    Rect::new(MARGIN, y1 - ROW_H, PANEL_W - MARGIN, y1)
}

/// Column `col` (0 left, 1 right) of a pair row.
pub fn cell(row: Rect, col: usize) -> Rect {
    let mid = PANEL_W * 0.5;
    if col == 0 {
        Rect::new(row.min[0], row.min[1], mid - COL_GAP * 0.5, row.max[1])
    } else {
        Rect::new(mid + COL_GAP * 0.5, row.min[1], row.max[0], row.max[1])
    }
}

/// The top of `rows` rows over `bottom`: where the block the pointer can
/// pick in begins (the top row's band included).
pub fn block_top(bottom: f32, rows: usize) -> f32 {
    match rows.checked_sub(1) {
        Some(top) => row(bottom, top).min[1] - ROW_GAP * 0.5,
        None => bottom - MARGIN,
    }
}

/// What the pointer is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// The row, counted from the bottom.
    pub row: usize,
    /// The column: the side of the panel's center (in a wide row, its
    /// half).
    pub col: usize,
    /// The right half of that column's cell.
    pub right: bool,
}

/// The control under `cursor` in a block of `rows` rows over `bottom`, by
/// height, then side: `None` above or below the block.
pub fn hit(bottom: f32, rows: usize, cursor: [f32; 2]) -> Option<Hit> {
    let pitch = ROW_H + ROW_GAP;
    // Down from the bottom row's band edge; row k owns (k, k + 1] pitches.
    let d = bottom - MARGIN + ROW_GAP * 0.5 - cursor[1];
    if d <= 0.0 {
        return None;
    }
    let k = (d / pitch).ceil() as usize - 1;
    if k >= rows {
        return None;
    }
    let col = usize::from(cursor[0] >= PANEL_W * 0.5);
    let right = cursor[0] >= cell(row(bottom, k), col).center_x();
    Some(Hit { row: k, col, right })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rows_tile_the_block_bottom_up() {
        for (bottom, n) in [false, true].into_iter().flat_map(|editing| {
            let menu = menu_rows(editing);
            [(PANEL_H, debug_rows(editing)), (menu_h(menu), menu)]
        }) {
            let top = block_top(bottom, n);
            let base = row(bottom, 0).max[1] + ROW_GAP * 0.5;
            assert!((base - (bottom - MARGIN + ROW_GAP * 0.5)).abs() < 1e-4);
            // Every height in the block is on exactly the row whose band
            // holds it, the rows in order; outside it, nothing.
            let mut last = n;
            let mut y = top;
            while y < base {
                let h = hit(bottom, n, [100.0, y]).unwrap_or_else(|| panic!("a gap at {y}"));
                let r = row(bottom, h.row);
                assert!(
                    y >= r.min[1] - ROW_GAP * 0.5 - 1e-3 && y < r.max[1] + ROW_GAP * 0.5 + 1e-3,
                    "{y} on row {} {r:?}",
                    h.row
                );
                assert!(h.row <= last, "rows out of order at {y}");
                last = h.row;
                y += 0.25;
            }
            assert_eq!(last, 0);
            assert_eq!(hit(bottom, n, [100.0, top - 0.5]), None);
            assert_eq!(hit(bottom, n, [100.0, base + 0.5]), None);
            // Each row is ROW_H tall, ROW_GAP apart, inside the margins.
            for k in 0..n {
                let r = row(bottom, k);
                assert!((r.height() - ROW_H).abs() < 1e-4);
                assert_eq!((r.min[0], r.max[0]), (MARGIN, PANEL_W - MARGIN));
                if k > 0 {
                    assert!((row(bottom, k - 1).min[1] - r.max[1] - ROW_GAP).abs() < 1e-4);
                }
            }
        }
    }

    #[test]
    fn the_bottom_row_sits_as_far_above_the_edge_in_the_menu_and_the_panel() {
        // The quad keeps its bottom edge when debug toggles, so the row
        // under the pointer stays under it; and when Edit room grows the
        // menu, so its bottom rows stay where they were.
        for editing in [false, true] {
            let h = menu_h(menu_rows(editing));
            assert_close!(h - row(h, 0).max[1], MARGIN);
        }
        let panel = PANEL_H - row(PANEL_H, 0).max[1];
        assert_close!(panel, MARGIN);
    }

    #[test]
    fn a_pair_row_is_two_cells_each_with_its_halves_and_end_boxes() {
        let r = row(PANEL_H, 3);
        let (left, right) = (cell(r, 0), cell(r, 1));
        assert!((left.width() - right.width()).abs() < 1e-4);
        assert!((right.min[0] - left.max[0] - COL_GAP).abs() < 1e-4);
        assert_eq!((left.min[0], right.max[0]), (r.min[0], r.max[0]));
        let y = (r.min[1] + r.max[1]) * 0.5;
        // Across the row: left cell's -, its +, right cell's -, its +,
        // switching at each cell's center and at the panel's.
        let mut seen = Vec::new();
        let mut x = 0.0;
        while x < PANEL_W {
            let h = hit(PANEL_H, debug_rows(false), [x, y]).expect("on the row");
            assert_eq!(h.row, 3);
            if seen.last() != Some(&(h.col, h.right)) {
                seen.push((h.col, h.right));
            }
            let col = usize::from(x >= PANEL_W * 0.5);
            assert_eq!((h.col, h.right), (col, x >= cell(r, col).center_x()), "{x}");
            x += 0.5;
        }
        assert_eq!(seen, [(0, false), (0, true), (1, false), (1, true)]);
        // A wide row's halves are the columns.
        let h = hit(PANEL_H, debug_rows(false), [PANEL_W * 0.5 - 1.0, y]).unwrap();
        assert_eq!(h.col, 0);
        assert_eq!(
            hit(PANEL_H, debug_rows(false), [PANEL_W * 0.5, y])
                .unwrap()
                .col,
            1
        );
        // The end boxes are END_BOX_W at the cell's ends, leaving the
        // middle for the label and the value.
        for c in [left, right] {
            let (minus, plus) = c.end_boxes();
            assert_eq!((minus.min[0], minus.width()), (c.min[0], END_BOX_W));
            assert_eq!((plus.max[0], plus.width()), (c.max[0], END_BOX_W));
            assert!(c.middle().width() > 100.0, "{:?}", c.middle());
        }
    }

    #[test]
    fn nothing_is_under_12_points_and_everything_fits() {
        for f in [
            FONT_TITLE,
            FONT_VALUE,
            FONT_LABEL,
            FONT_BUTTON,
            FONT_END,
            FONT_GRAPH,
        ] {
            assert!(f >= MIN_FONT, "{f}");
        }
        assert_close!(MIN_FONT, 12.0);
        // The debug panel: the header over the fullest control block, ten
        // steppers in five full pair rows (the space size filled the fifth
        // row's empty cell, board #3325), so no taller than with nine,
        // the room editor's row under the menu's (board #3326), the
        // cloud's under that (step 2c) and the music's at the bottom
        // (board #3472); with Edit room on, the surface's three rows under
        // its own (D3), which is the headroom the header leaves. The
        // menu's effect row (board #3336) is the panel's Prev/Next row, so
        // the count is as it was.
        assert_eq!(debug_rows(false), 11);
        assert_eq!(debug_rows(true), 14);
        for editing in [false, true] {
            assert!(block_top(PANEL_H, debug_rows(editing)) >= MARGIN + HEADER_H);
        }
        assert!(block_top(PANEL_H, debug_rows(true) + 1) < MARGIN + HEADER_H);
        // The hand menu: a title over five rows (the effect's on top since
        // board #3336), or eight with Edit room on, 40 points taller for
        // each past the first. No row or font shrinks for them.
        assert_eq!((menu_rows(false), menu_rows(true)), (5, 8));
        assert_close!(menu_h(menu_rows(false)), 244.0);
        assert_close!(menu_h(menu_rows(true)), 364.0);
        for editing in [false, true] {
            let n = menu_rows(editing);
            assert!(block_top(menu_h(n), n) >= MARGIN + FONT_TITLE * 1.4);
            assert!(menu_h(n) <= PANEL_H);
        }
        // The texture is unchanged: 640 x 1472 at 1.6 px per point.
        assert_eq!((PANEL_W, PANEL_H), (400.0, 920.0));
    }
}
