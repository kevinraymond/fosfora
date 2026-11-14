//! How wide a line of text is in egui's default proportional font,
//! Ubuntu-Light (board #5461), so the in-world label's wrap (`label.rs`)
//! and the hand menu's words (`panel_grid.rs`) are sized and tested on the
//! desktop, where egui is not built. The advances are the font's own, from
//! its `hmtx` table at 1000 units an em. egui 0.33 lays a line out by these
//! advances with no kerning (the font has no `kern` table), so a width here
//! is egui's to a fraction of a pixel. The renderers still shrink a line
//! that runs over, for a character this table lacks.

/// The advance of each printable ASCII character, space (0x20) to `~`
/// (0x7E), in thousandths of an em, sixteen to a row: ' ' to '/', '0' to
/// '?', '@' to 'O', 'P' to '_', '`' to 'o', and 'p' to '~'.
#[rustfmt::skip]
const ASCII: [u16; 95] = [
    228, 274, 392, 653, 564, 831, 648, 240, 310, 310, 472, 564, 246, 282, 246, 359,
    564, 564, 564, 564, 564, 564, 564, 564, 564, 564, 246, 246, 564, 564, 564, 380,
    940, 641, 630, 609, 702, 555, 520, 658, 692, 248, 487, 599, 502, 865, 709, 774,
    592, 774, 618, 517, 548, 680, 624, 921, 608, 569, 560, 310, 359, 310, 564, 490,
    376, 509, 583, 460, 583, 548, 376, 573, 569, 237, 237, 495, 255, 863, 569, 582,
    583, 583, 374, 428, 387, 569, 480, 775, 499, 474, 455, 319, 262, 319, 564,
];

/// The other characters the app's words use: the ellipsis, the middle dot
/// and the curly quotes.
const OTHER: [(char, u16); 5] = [
    ('\u{2026}', 996),
    ('\u{b7}', 246),
    ('\u{2019}', 230),
    ('\u{201c}', 392),
    ('\u{201d}', 392),
];

/// A character neither table has: an em, wider than any letter, so a
/// width here errs long and a line that holds one wraps early rather than
/// late.
const UNKNOWN: u16 = 1000;

/// The width of `text` on one line, in ems.
pub fn em(text: &str) -> f32 {
    let thousandths: u32 = text
        .chars()
        .map(|c| {
            let code = c as u32;
            if (0x20..0x7f).contains(&code) {
                u32::from(ASCII[(code - 0x20) as usize])
            } else {
                OTHER
                    .iter()
                    .find(|(o, _)| *o == c)
                    .map_or(u32::from(UNKNOWN), |(_, w)| u32::from(*w))
            }
        })
        .sum();
    thousandths as f32 / 1000.0
}

/// The width of `text` on one line at `size` (in the size's own unit:
/// points on the panel, pixels on the label's texture).
pub fn width(text: &str, size: f32) -> f32 {
    em(text) * size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_widths_are_the_fonts_own() {
        // The advances egui lays out: "Theme: Aurora*" is 113 points at the
        // panel's 16, "Voice: off" 68 (board #5415's measures, `hud.rs`).
        assert!((width("Theme: Aurora*", 16.0) - 113.1).abs() < 0.1);
        assert!((width("Voice: off", 16.0) - 68.1).abs() < 0.1);
        // The label's measure (board #5461): the theme miss at the old
        // 40 px floor was 1118 px on a 1024 px texture.
        let miss = "No theme called \"blue sky\": Classic, Drift, Tide, Aurora or Mirror";
        assert!((width(miss, 40.0) - 1117.8).abs() < 0.1);
        // Digits are tabular; a space is narrower than a letter.
        assert_close!(em("0123456789"), 5.64);
        assert!(em(" ") < em("n"));
        assert_close!(em(""), 0.0);
    }

    #[test]
    fn the_app_s_other_characters_are_known_and_the_rest_err_long() {
        assert_close!(em("\u{2026}"), 0.996);
        assert_close!(em("a \u{b7} b"), em("a . b"));
        assert_close!(em("\u{201c}x\u{201d}"), em("\"x\""));
        // An arrow the font lacks (egui draws it from its emoji fonts):
        // wider than any letter here.
        assert!(em("\u{2192}") > em("W"));
    }
}
