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
//! [`ROW_GAP`] between them. A row is wide (a left and a right action,
//! a surface row's `<` `>`, or one action across it, the Limit flashing
//! toggle) or a pair of cells, one per column, each about
//! half the width with its own -/+ end boxes ([`END_BOX_W`]) or one
//! action. The pointer picks a row by its height alone (a row owns half
//! the gap above and below it, so the rows tile the block), a column by
//! the side of the panel's center it is on, and a half of the cell by the
//! side of the cell's center. The Music cell is the one exception (board
//! #5385): its two end boxes and the middle between them are three
//! targets ([`end_box`]).

use crate::release::Surface;

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
/// The -/+ and `<` `>` boxes at a cell's or a row's ends.
pub const END_BOX_W: f32 = 40.0;
/// The room a status cell's text keeps from each end (points; `hud.rs`).
pub const STATUS_PAD: f32 = 4.0;
/// The hand menu's rows with Edit room off on the developer surface, in
/// both layouts at the bottom of the block: the Limit flashing toggle
/// (board #5534, the wearer's photosensitive setting, on both surfaces),
/// over Recenter and Rescan (in the
/// menu since board #4192, so the scan label's "Rescan from the hand menu"
/// has a row to find; since board #5393 the two share the left cell and
/// Roam has the right one), over the agent's provider (board #3776), over
/// the pitcher and debug toggles, over the room editor's (board #3326),
/// over the particles' (step 2c), over the music's (board #3472). The
/// release surface (board #4204) leaves out the pitcher and debug row
/// ([`menu`]). Board #5470: the world effect's `<` `>` row, which was on
/// top, went with the effects it stepped, so the menu was one row shorter
/// until the Limit flashing row took that place.
pub const MENU_ROWS: usize = 7;
/// The rows Edit room adds under its own (board #3472, D3): the pointed
/// surface's color, band and strength.
pub const SURFACE_ROWS: usize = 3;

/// One row of the hand menu (`hud.rs` paints each).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuRow {
    /// The Limit flashing toggle, one action across the row (board
    /// #5534).
    Flash,
    /// Recenter and Rescan, a half each, beside Roam.
    Recenter,
    /// The Agent control beside the Immersion control.
    Agent,
    /// The Pitcher toggle beside the Debug panel toggle: the developer
    /// surface's only.
    Developer,
    /// Edit room beside its status.
    Edit,
    /// The pointed surface's color, band or strength (Edit room on), by
    /// its index in the three.
    SurfaceParam(usize),
    /// The Particles toggle beside the edge bars' control, which shares
    /// its cell with "All: none" on the developer surface (`all_none`)
    /// and has the cell to itself on the release one.
    Cloud { all_none: bool },
    /// The Music cell beside Voice and Theme.
    Music,
}

/// The hand menu's rows on `surface`, top to bottom. The release surface
/// (board #4204) leaves out the developer row (Pitcher and Debug panel)
/// and the "All: none" half, whose cell the edge bars' control fills: the
/// rows close up and no cell or half stands empty. The Limit flashing row
/// is on both (board #5534): it is a wearer's setting, not a developer's.
/// It went in at the top as the Recenter and the Agent rows did, so no row
/// the wearer already reaches for moved.
pub fn menu(surface: Surface, editing: bool) -> Vec<MenuRow> {
    let developer = surface.developer();
    let mut rows = vec![MenuRow::Flash, MenuRow::Recenter, MenuRow::Agent];
    if developer {
        rows.push(MenuRow::Developer);
    }
    rows.push(MenuRow::Edit);
    if editing {
        rows.extend((0..SURFACE_ROWS).map(MenuRow::SurfaceParam));
    }
    rows.push(MenuRow::Cloud {
        all_none: developer,
    });
    rows.push(MenuRow::Music);
    rows
}

/// The hand menu's row count on `surface`: [`MENU_ROWS`] on the developer
/// surface and one fewer on the release one, and the [`SURFACE_ROWS`]
/// with them while Edit room is on ([`menu`]'s length).
pub const fn menu_rows(surface: Surface, editing: bool) -> usize {
    let base = match surface {
        Surface::Developer => MENU_ROWS,
        Surface::Release => MENU_ROWS - 1,
    };
    if editing { base + SURFACE_ROWS } else { base }
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
/// room to spare: about 250 points in the texture dumps. 300 since the
/// Agent row (board #3776), the most that leaves room for the fullest
/// block (320 before). Board #5534: the Limit flashing row fills that
/// block again, as the effect's row did until board #5470.
pub const HEADER_H: f32 = 300.0;
/// The debug panel's -/+ steppers (`STEPPERS` in `hud.rs`, which checks
/// the count), two to a row.
pub const STEPPERS: usize = 10;
/// The most rows the debug panel shows: the steppers' pair rows, then the
/// menu's rows (Limit flashing, Recenter and Rescan, Agent, Pitcher and
/// Debug, Edit room and its status, with Edit room on the surface's Color,
/// Band and Strength, Particles and All: none, Music and Voice).
pub const fn debug_rows(editing: bool) -> usize {
    STEPPERS.div_ceil(2) + menu_rows(Surface::Developer, editing)
}

// A stepper's label over its value in one row, and the menu's title over
// its top row, in either menu.
const _: () = assert!(FONT_LABEL + FONT_VALUE <= ROW_H - 4.0);
const _: () = assert!(
    MARGIN
        + FONT_TITLE * 1.4
        + menu_rows(Surface::Developer, false) as f32 * (ROW_GAP + ROW_H)
        + MARGIN
        <= menu_h(menu_rows(Surface::Developer, false))
);
const _: () = assert!(
    MARGIN
        + FONT_TITLE * 1.4
        + menu_rows(Surface::Developer, true) as f32 * (ROW_GAP + ROW_H)
        + MARGIN
        <= menu_h(menu_rows(Surface::Developer, true))
);
const _: () = assert!(
    MARGIN
        + FONT_TITLE * 1.4
        + menu_rows(Surface::Release, false) as f32 * (ROW_GAP + ROW_H)
        + MARGIN
        <= menu_h(menu_rows(Surface::Release, false))
);
const _: () = assert!(
    MARGIN
        + FONT_TITLE * 1.4
        + menu_rows(Surface::Release, true) as f32 * (ROW_GAP + ROW_H)
        + MARGIN
        <= menu_h(menu_rows(Surface::Release, true))
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

/// The width a pair row's cell gives its words (points): a button's, which
/// `hud.rs` centers on the whole cell.
pub fn cell_w() -> f32 {
    cell(row(PANEL_H, 0), 0).width()
}

/// The width half a cell gives its words (points): `hud.rs` splits a cell
/// at its center and leaves half a column's gap on each side of the split.
pub fn half_w() -> f32 {
    cell(row(PANEL_H, 0), 0).halves().0.width() - COL_GAP * 0.5
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

/// Which end box of `area` the pointer at `x` is in: `Some(false)` the
/// left, `Some(true)` the right, `None` between them. The Music cell's
/// `<` and `>` around its play/stop (board #5385), where the middle is a
/// third target and the halves cannot pick.
pub fn end_box(area: Rect, x: f32) -> Option<bool> {
    let (left, right) = area.end_boxes();
    if x < left.max[0] {
        Some(false)
    } else if x >= right.min[0] {
        Some(true)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_boxes_and_the_middle_split_a_cell_in_three() {
        let c = cell(row(PANEL_H, 0), 0);
        let (left, right) = c.end_boxes();
        assert_eq!(end_box(c, c.min[0]), Some(false));
        assert_eq!(end_box(c, left.max[0] - 0.1), Some(false));
        assert_eq!(end_box(c, left.max[0]), None);
        assert_eq!(end_box(c, c.center_x()), None);
        assert_eq!(end_box(c, right.min[0] - 0.1), None);
        assert_eq!(end_box(c, right.min[0]), Some(true));
        assert_eq!(end_box(c, c.max[0]), Some(true));
        // Each end box is as wide as a stepper's, 2 cm on the panel.
        assert_close!(left.width(), END_BOX_W);
        assert!(c.middle().width() > 100.0, "{:?}", c.middle());
    }

    #[test]
    fn the_rows_tile_the_block_bottom_up() {
        for (bottom, n) in [false, true].into_iter().flat_map(|editing| {
            let menu = menu_rows(Surface::Developer, editing);
            let release = menu_rows(Surface::Release, editing);
            [
                (PANEL_H, debug_rows(editing)),
                (menu_h(menu), menu),
                (menu_h(release), release),
            ]
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
            for surface in [Surface::Developer, Surface::Release] {
                let h = menu_h(menu_rows(surface, editing));
                assert_close!(h - row(h, 0).max[1], MARGIN);
            }
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
        // particles' under that (step 2c) and the music's at the bottom
        // (board #3472); with Edit room on, the surface's three rows under
        // its own (D3). The Agent row (board #3776) added one. Recenter
        // and Rescan joining the menu (board #4192) added none: the panel
        // had them. The effect row's going (board #5470) took one away,
        // and the Limit flashing row (board #5534) took its place, so the
        // header has no row of headroom over the fullest block.
        assert_eq!(debug_rows(false), 12);
        assert_eq!(debug_rows(true), 15);
        for editing in [false, true] {
            assert!(block_top(PANEL_H, debug_rows(editing)) >= MARGIN + HEADER_H);
        }
        assert!(block_top(PANEL_H, debug_rows(true) + 1) < MARGIN + HEADER_H);
        // The hand menu: a title over seven rows (Limit flashing on top
        // since board #5534, Recenter and Rescan under it since board
        // #4192, the Agent row under them since board #3776; the effect's
        // row was on top until board #5470), or ten with Edit room on, 40
        // points taller for each past the first. No row or font shrinks
        // for them.
        let dev = Surface::Developer;
        assert_eq!((menu_rows(dev, false), menu_rows(dev, true)), (7, 10));
        assert_close!(menu_h(menu_rows(dev, false)), 324.0);
        assert_close!(menu_h(menu_rows(dev, true)), 444.0);
        // The release surface (board #4204): one row fewer, 40 points
        // shorter.
        let rel = Surface::Release;
        assert_eq!((menu_rows(rel, false), menu_rows(rel, true)), (6, 9));
        assert_close!(menu_h(menu_rows(rel, false)), 284.0);
        assert_close!(menu_h(menu_rows(rel, true)), 404.0);
        for editing in [false, true] {
            for surface in [dev, rel] {
                let n = menu_rows(surface, editing);
                assert!(block_top(menu_h(n), n) >= MARGIN + FONT_TITLE * 1.4);
                assert!(menu_h(n) <= PANEL_H);
            }
        }
        // The texture is unchanged: 640 x 1472 at 1.6 px per point.
        assert_eq!((PANEL_W, PANEL_H), (400.0, 920.0));
    }

    #[test]
    fn every_menu_word_fits_its_cell() {
        use crate::text_width::width;
        // Board #5461: "Particles: off (shown: editing)" was 208 points in
        // a 187-point cell, and "Edges: theme" is 97 in a 90.5-point half.
        assert_close!(cell_w(), 187.0);
        assert_close!(half_w(), 90.5);
        let fits = |text: &str, size: f32, room: f32| {
            let w = width(text, size);
            assert!(w <= room, "{text:?} is {w:.1} points in {room:.1}");
        };
        // A button with a cell of its own, at the button's size: the
        // words `hud.rs` paints itself, then the controls' own.
        let mut whole: Vec<&str> = vec!["Edit room: on", "Edit room: off", "Recenter"];
        whole.extend(
            [
                crate::roam::Roam::Off,
                crate::roam::Roam::On(crate::roam::Why::Chosen),
                crate::roam::Roam::On(crate::roam::Why::NoRoom),
            ]
            .map(crate::roam::Roam::label),
        );
        whole.extend(crate::immerse::Depth::ALL.map(crate::immerse::Depth::label));
        {
            use crate::agent::{NONE_LABEL, Pick, ProviderKind};
            whole.extend(
                [
                    Pick::Local,
                    Pick::Network(ProviderKind::Anthropic),
                    Pick::Network(ProviderKind::OpenAi),
                    Pick::Off,
                ]
                .map(Pick::label),
            );
            whole.push(NONE_LABEL);
        }
        let mut edges = crate::edge_bars::Choice::Theme;
        for _ in 0..5 {
            whole.push(edges.label());
            edges = edges.next();
        }
        for on in [true, false] {
            whole.push(crate::room_edit::particles_label(on));
        }
        // Board #5534: the Limit flashing toggle has its row to itself,
        // and its words fit a cell too.
        for on in [true, false] {
            whole.push(crate::flash_choice::row_label(on));
        }
        for text in whole {
            fits(text, FONT_BUTTON, cell_w());
        }
        // Half a cell, on one line at the button's size.
        for text in ["Recenter", "Rescan", "All: none", "Voice: on", "Voice: off"] {
            fits(text, FONT_BUTTON, half_w());
        }
        // Half a cell, a name over a value: Edges over its state, Theme
        // over the theme (cut to fit 6 points short of the half, though
        // none needs it).
        fits("Edges", FONT_LABEL, half_w());
        for _ in 0..5 {
            fits(edges.value(), FONT_VALUE, half_w());
            edges = edges.next();
        }
        fits("Theme", FONT_LABEL, half_w());
        for theme in crate::themes::Theme::ALL {
            fits(&format!("{}*", theme.title()), FONT_VALUE, half_w() - 6.0);
        }
        // The Music cell's press words over the title, between `<` `>`.
        let music = cell(row(PANEL_H, 0), 0).middle().width() - 4.0;
        for playing in [true, false] {
            fits(crate::music::label(playing), FONT_LABEL, music);
        }
        // The status cell: one line at the value's size, or at most two at
        // the label's, word by word (`hud.rs` cuts a third).
        let status = cell_w() - 2.0 * STATUS_PAD;
        fits(crate::label::STATUS_NO_SURFACE, FONT_VALUE, status);
        fits("desk: streamlines", FONT_VALUE, status);
        let widest = crate::surfaces::SurfaceBehavior::ALL
            .into_iter()
            .max_by(|a, b| width(a.name(), 1.0).total_cmp(&width(b.name(), 1.0)))
            .unwrap();
        let longest =
            crate::label::status_text("invisible wall face 12", Some((widest, 1.0)), true);
        assert!(
            width(&longest, FONT_LABEL) > status,
            "{longest:?} needs no wrap"
        );
        let mut lines: Vec<String> = Vec::new();
        for word in longest.split(' ') {
            match lines.last_mut() {
                Some(line) if width(&format!("{line} {word}"), FONT_LABEL) <= status => {
                    line.push(' ');
                    line.push_str(word);
                }
                _ => lines.push(word.to_owned()),
            }
        }
        // Two lines of the label's size fit a row's height as a stepper's
        // label over its value does (the const assert at the top).
        assert_eq!(lines.len(), 2, "{lines:?}");
    }

    #[test]
    fn the_developer_menu_holds_every_row() {
        use MenuRow::*;
        assert_eq!(
            menu(Surface::Developer, false),
            [
                Flash,
                Recenter,
                Agent,
                Developer,
                Edit,
                Cloud { all_none: true },
                Music
            ]
        );
        assert_eq!(
            menu(Surface::Developer, true),
            [
                Flash,
                Recenter,
                Agent,
                Developer,
                Edit,
                SurfaceParam(0),
                SurfaceParam(1),
                SurfaceParam(2),
                Cloud { all_none: true },
                Music
            ]
        );
    }

    #[test]
    fn the_release_menu_hides_the_developer_rows_and_closes_up() {
        use MenuRow::*;
        // Board #4204: no Pitcher or Debug panel row, no "All: none" half;
        // the rows under the gone one move up a row and the edge bars'
        // control takes the whole cell. Board #5534: the Limit flashing
        // row is the wearer's, so the release menu has it, on top.
        assert_eq!(
            menu(Surface::Release, false),
            [
                Flash,
                Recenter,
                Agent,
                Edit,
                Cloud { all_none: false },
                Music
            ]
        );
        assert_eq!(
            menu(Surface::Release, true),
            [
                Flash,
                Recenter,
                Agent,
                Edit,
                SurfaceParam(0),
                SurfaceParam(1),
                SurfaceParam(2),
                Cloud { all_none: false },
                Music
            ]
        );
        for editing in [false, true] {
            for surface in [Surface::Developer, Surface::Release] {
                let rows = menu(surface, editing);
                assert_eq!(rows.len(), menu_rows(surface, editing));
                // Each row once, and the release menu is the developer's
                // with the developer row out: the same order, no blank
                // row in its place.
                for (i, r) in rows.iter().enumerate() {
                    assert!(!rows[i + 1..].contains(r), "{r:?} twice");
                }
            }
            let dev: Vec<MenuRow> = menu(Surface::Developer, editing)
                .into_iter()
                .filter(|r| *r != Developer)
                .map(|r| match r {
                    Cloud { .. } => Cloud { all_none: false },
                    r => r,
                })
                .collect();
            assert_eq!(menu(Surface::Release, editing), dev);
            // The bottom rows sit where the developer menu has them, the
            // quad's bottom edge the same: only the top moves down.
            let (d, r) = (
                menu_rows(Surface::Developer, editing),
                menu_rows(Surface::Release, editing),
            );
            for k in 0..r {
                let in_dev = row(menu_h(d), k).max[1] - menu_h(d);
                let in_rel = row(menu_h(r), k).max[1] - menu_h(r);
                assert_close!(in_dev, in_rel);
            }
            assert_close!(menu_h(d) - menu_h(r), ROW_H + ROW_GAP);
        }
    }
}
