#![allow(dead_code)]

// Layout constants
pub const PANEL_ROUNDING: u8 = 6;
pub const WIDGET_ROUNDING: u8 = 4;
pub const CARD_ROUNDING: u8 = 8;
pub const CARD_PADDING: f32 = 8.0;
pub const CARD_MARGIN: f32 = 4.0;
pub const METER_HEIGHT: f32 = 48.0;
pub const METER_GAP: f32 = 3.0;
pub const SPACING: f32 = 6.0;
pub const SPACING_Y: f32 = 4.0;
pub const MIN_INTERACT_HEIGHT: f32 = 22.0;
pub const MIN_INTERACT_WIDTH: f32 = 40.0;
pub const FOCUS_RING_WIDTH: f32 = 2.0;

// Typography (#3125). At 100 % interface scale no text is drawn below
// MIN_TEXT_SIZE; a test holds every size written in the interface to it.
pub const MIN_TEXT_SIZE: f32 = 12.0;
/// Hints, units, badges, dense tables: the floor itself.
pub const SMALL_SIZE: f32 = MIN_TEXT_SIZE;
pub const MONO_SIZE: f32 = MIN_TEXT_SIZE;
pub const HEADING_SIZE: f32 = 13.0;
pub const BODY_SIZE: f32 = 14.0;

#[cfg(test)]
mod tests {
    use super::MIN_TEXT_SIZE;
    use std::path::Path;

    /// Every font size written as a number in the interface's source, with
    /// where it is. Covers `.size(12.0)` and `FontId::proportional(12.0)` /
    /// `monospace(..)`, the two ways a panel sets one.
    fn literal_sizes(dir: &Path, out: &mut Vec<(String, usize, f32)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                literal_sizes(&path, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for (i, line) in text.lines().enumerate() {
                    for open in [".size(", "FontId::proportional(", "FontId::monospace("] {
                        for (at, _) in line.match_indices(open) {
                            let arg = &line[at + open.len()..];
                            let arg = arg[..arg.find(')').unwrap_or(arg.len())].trim();
                            let arg = arg.trim_end_matches("f32").trim_end_matches('_');
                            // Only numbers: a token or an expression is checked
                            // where it is defined.
                            if let Ok(v) = arg.parse::<f32>() {
                                out.push((path.display().to_string(), i + 1, v));
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn no_text_in_the_interface_is_written_below_the_floor() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sizes = Vec::new();
        for dir in ["ui", "trama/ui"] {
            literal_sizes(&src.join(dir), &mut sizes);
        }
        assert!(
            sizes.len() > 20,
            "the scan found almost nothing: {}",
            sizes.len()
        );
        let small: Vec<_> = sizes
            .iter()
            .filter(|(_, _, v)| *v < MIN_TEXT_SIZE)
            .map(|(f, l, v)| format!("{f}:{l} size {v}"))
            .collect();
        assert!(
            small.is_empty(),
            "text below {MIN_TEXT_SIZE} px; use SMALL_SIZE or larger:\n{}",
            small.join("\n")
        );
    }
}
