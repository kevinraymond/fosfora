//! Theme tokens (#3125): every color the interface uses, named once.
//!
//! A [`Palette`] is the whole theme. egui's [`Visuals`] and the panels'
//! [`ThemeColors`] are both derived from it, so a theme — built in, or a
//! token file a user wrote — is one set of fifteen colors and nothing else.

use std::collections::BTreeMap;

use egui::{Color32, CornerRadius, Stroke, Visuals};

use super::colors::ThemeColors;
use super::tokens::{PANEL_ROUNDING, WIDGET_ROUNDING};

/// Declares the palette's fields once, with the token name a theme file uses
/// for each, so the struct, the file reader and the file writer can't drift.
macro_rules! palette {
    ($($(#[doc = $doc:literal])* $field:ident),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq)]
        pub struct Palette {
            $($(#[doc = $doc])* pub $field: Color32,)*
        }

        impl Palette {
            /// Token names, in file order.
            pub const KEYS: &[&str] = &[$(stringify!($field)),*];

            fn slot(&mut self, key: &str) -> Option<&mut Color32> {
                match key {
                    $(stringify!($field) => Some(&mut self.$field),)*
                    _ => None,
                }
            }

            /// Every token as `"#rrggbb"`, for writing a theme file.
            pub fn to_map(self) -> BTreeMap<String, String> {
                let mut m = BTreeMap::new();
                $(m.insert(stringify!($field).to_string(), to_hex(self.$field));)*
                m
            }
        }
    };
}

palette! {
    /// Behind everything: the page the panels sit on.
    bg,
    /// Panels and cards.
    panel,
    /// Insets: text fields, slider rails, meters, checkboxes.
    well,
    /// Body text.
    text,
    /// Secondary text: hints, units, captions.
    sub,
    /// Disabled text.
    dim,
    /// Separators and card borders.
    rule,
    /// Outlines of controls.
    line,
    /// Fill of whatever is selected or on.
    sel_bg,
    /// Text on `sel_bg`.
    sel_fg,
    /// Hover and focus outlines, links, emphasis.
    accent,
    /// Errors. Never the only sign of one: say it in words or a shape too.
    error,
    /// Warnings, under the same rule.
    warning,
    /// Success, connected, running.
    success,
    /// The beat pulse.
    beat,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Palette {
    /// The Light table theme from the v2 mockup: near-white ground, ink text,
    /// selection shown by inverting to ink.
    pub const LIGHT: Palette = Palette {
        bg: rgb(0xEEF1F0),
        panel: rgb(0xFFFFFF),
        well: rgb(0xDDE2E5),
        text: rgb(0x1F2328),
        sub: rgb(0x555C66),
        dim: rgb(0x8A919A),
        rule: rgb(0xC9CED4),
        line: rgb(0x80878F),
        sel_bg: rgb(0x1F2328),
        sel_fg: rgb(0xFFFFFF),
        accent: rgb(0x1F2328),
        error: rgb(0x1F2328),
        warning: rgb(0x1F2328),
        success: rgb(0x1F2328),
        beat: rgb(0x1F2328),
    };

    /// Mid-gray ground, the mockup's default.
    pub const GRAY: Palette = Palette {
        bg: rgb(0x3A3F45),
        panel: rgb(0x484E55),
        well: rgb(0x25292D),
        text: rgb(0xF1F0EC),
        sub: rgb(0xC9CDD2),
        dim: rgb(0x8C939B),
        rule: rgb(0x5E656D),
        line: rgb(0x9AA0A7),
        sel_bg: rgb(0xFFFFFF),
        sel_fg: rgb(0x1E2124),
        accent: rgb(0xF1F0EC),
        error: rgb(0xF1F0EC),
        warning: rgb(0xF1F0EC),
        success: rgb(0xF1F0EC),
        beat: rgb(0xF1F0EC),
    };

    /// Near-black, for a dark room.
    pub const BLACK: Palette = Palette {
        bg: rgb(0x0E0F11),
        panel: rgb(0x15171A),
        well: rgb(0x1C1F23),
        text: rgb(0xECEAE4),
        sub: rgb(0xA5A9B0),
        dim: rgb(0x5E646C),
        rule: rgb(0x2A2E34),
        line: rgb(0x5E656D),
        sel_bg: rgb(0xECEAE4),
        sel_fg: rgb(0x111214),
        accent: rgb(0xECEAE4),
        error: rgb(0xECEAE4),
        warning: rgb(0xECEAE4),
        success: rgb(0xECEAE4),
        beat: rgb(0xECEAE4),
    };

    /// Navy with the Okabe–Ito orange and sky blue: a pair that stays apart
    /// for red–green color blindness. Blue is the ground, the outlines and
    /// what is live; orange is what is selected, hovered or focused. A state
    /// still never rests on hue alone.
    pub const BLUE_ORANGE: Palette = Palette {
        bg: rgb(0x0E1A2B),
        panel: rgb(0x15253B),
        well: rgb(0x0A1320),
        text: rgb(0xEEF2F7),
        sub: rgb(0xB3C2D6),
        dim: rgb(0x62748C),
        rule: rgb(0x26405E),
        line: rgb(0x5B8FC7),
        sel_bg: rgb(0xE69F00),
        sel_fg: rgb(0x1A1203),
        accent: rgb(0xF0A830),
        error: rgb(0xFF8A4C),
        warning: rgb(0xF0E442),
        success: rgb(0x56B4E9),
        beat: rgb(0xE69F00),
    };

    /// A dark theme: egui's dark defaults underneath, light text on dark.
    pub fn is_dark(&self) -> bool {
        luminance(self.bg) < 0.5
    }

    /// Replace the tokens a theme file names. Unknown names and colors that
    /// don't parse are returned, one message each, and change nothing.
    pub fn apply(&mut self, colors: &BTreeMap<String, String>) -> Vec<String> {
        let mut problems = Vec::new();
        for (key, value) in colors {
            match (self.slot(key), parse_hex(value)) {
                (Some(slot), Some(c)) => *slot = c,
                (None, _) => problems.push(format!(
                    "\"{key}\" is not a theme color (the names are: {})",
                    Self::KEYS.join(", ")
                )),
                (Some(_), None) => problems.push(format!(
                    "\"{key}\": \"{value}\" is not a color; write it as #rrggbb"
                )),
            }
        }
        problems
    }

    /// Pairs that must stay readable, with WCAG 2.2's minimums: 4.5:1 for
    /// text, 3:1 for the outline that shows where a control is. A pair below
    /// its minimum comes back as a sentence, for the Appearance panel.
    pub fn contrast_problems(&self) -> Vec<String> {
        let pairs: [(&str, Color32, &str, Color32, f32); 7] = [
            ("text", self.text, "panel", self.panel, 4.5),
            ("text", self.text, "bg", self.bg, 4.5),
            ("sub", self.sub, "panel", self.panel, 4.5),
            ("sub", self.sub, "bg", self.bg, 4.5),
            ("text", self.text, "well", self.well, 4.5),
            ("sel_fg", self.sel_fg, "sel_bg", self.sel_bg, 4.5),
            ("line", self.line, "panel", self.panel, 3.0),
        ];
        pairs
            .iter()
            .filter_map(|&(a, ca, b, cb, min)| {
                let r = contrast(ca, cb);
                (r < min).then(|| format!("{a} on {b} is {r:.1}:1, below {min}:1"))
            })
            .collect()
    }

    /// egui's look, from the tokens.
    pub fn visuals(&self) -> Visuals {
        let p = self;
        let mut v = if p.is_dark() {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        v.override_text_color = Some(p.text);
        v.panel_fill = p.bg;
        v.window_fill = p.panel;
        v.extreme_bg_color = p.well;
        v.faint_bg_color = mix(p.panel, p.bg, 0.5);
        v.code_bg_color = p.well;
        v.hyperlink_color = p.accent;
        v.warn_fg_color = p.warning;
        v.error_fg_color = p.error;
        // Selected text is drawn in `sel_fg` over `sel_bg`, so selection
        // inverts rather than tints, and reads without hue.
        v.selection.bg_fill = p.sel_bg;
        v.selection.stroke = Stroke::new(1.0_f32, p.sel_fg);

        let radius = CornerRadius::same(WIDGET_ROUNDING);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = p.panel;
        w.noninteractive.weak_bg_fill = p.panel;
        w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.rule);
        w.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.sub);
        w.noninteractive.corner_radius = radius;

        w.inactive.bg_fill = p.well;
        w.inactive.weak_bg_fill = mix(p.panel, p.text, 0.06);
        w.inactive.bg_stroke = Stroke::new(1.0_f32, p.line);
        w.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
        w.inactive.corner_radius = radius;

        w.hovered.bg_fill = mix(p.well, p.text, 0.14);
        w.hovered.weak_bg_fill = mix(p.panel, p.text, 0.14);
        w.hovered.bg_stroke = Stroke::new(1.5_f32, p.accent);
        w.hovered.fg_stroke = Stroke::new(1.5_f32, p.text);
        w.hovered.corner_radius = radius;

        w.active.bg_fill = mix(p.well, p.text, 0.24);
        w.active.weak_bg_fill = mix(p.panel, p.text, 0.22);
        w.active.bg_stroke = Stroke::new(2.0_f32, p.accent);
        w.active.fg_stroke = Stroke::new(2.0_f32, p.text);
        w.active.corner_radius = radius;

        w.open = w.active;

        v.window_corner_radius = CornerRadius::same(PANEL_ROUNDING);
        v.window_stroke = Stroke::new(1.0_f32, p.rule);
        v
    }

    /// The panels' colors, from the tokens.
    pub fn colors(&self) -> ThemeColors {
        let p = self;
        let dark = p.is_dark();
        ThemeColors {
            canvas: p.bg,
            panel: p.bg,
            text_primary: p.text,
            text_secondary: p.sub,
            accent: p.accent,
            error: p.error,
            warning: p.warning,
            success: p.success,
            widget_bg: p.well,
            card_bg: p.panel,
            card_border: p.rule,
            beat_color: p.beat,
            meter_bg: p.well,
            separator: p.rule,
            text_dim: p.dim,
            hover_fill: with_alpha(p.text, 18),
            hover_border: with_alpha(p.text, 36),
            backdrop: Color32::from_black_alpha(if dark { 180 } else { 120 }),
            selection: p.sel_bg,
            on_selection: p.sel_fg,
        }
    }
}

fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

/// `a` moved `t` of the way toward `b`, in sRGB.
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// WCAG relative luminance of an sRGB color.
fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(h, 16).ok().map(rgb)
}

fn to_hex(c: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILT_IN: [(&str, Palette); 4] = [
        ("Light", Palette::LIGHT),
        ("Gray", Palette::GRAY),
        ("Black", Palette::BLACK),
        ("Blue and orange", Palette::BLUE_ORANGE),
    ];

    #[test]
    fn every_built_in_theme_meets_the_contrast_minimums() {
        for (name, p) in BUILT_IN {
            assert!(
                p.contrast_problems().is_empty(),
                "{name}: {:?}",
                p.contrast_problems()
            );
        }
    }

    #[test]
    fn the_blue_and_orange_state_colors_read_on_its_panels() {
        let p = Palette::BLUE_ORANGE;
        for (what, c) in [
            ("error", p.error),
            ("warning", p.warning),
            ("success", p.success),
        ] {
            assert!(contrast(c, p.panel) >= 4.5, "{what} on panel");
        }
    }

    #[test]
    fn light_is_light_and_the_rest_are_dark() {
        assert!(!Palette::LIGHT.is_dark());
        assert!(Palette::GRAY.is_dark());
        assert!(Palette::BLACK.is_dark());
        assert!(Palette::BLUE_ORANGE.is_dark());
    }

    #[test]
    fn a_palette_survives_its_own_file() {
        for (_, p) in BUILT_IN {
            let mut q = Palette::LIGHT;
            assert!(q.apply(&p.to_map()).is_empty());
            assert_eq!(q, p);
        }
    }

    #[test]
    fn a_file_names_every_token() {
        assert_eq!(Palette::GRAY.to_map().len(), Palette::KEYS.len());
    }

    #[test]
    fn bad_names_and_bad_colors_are_reported_and_change_nothing() {
        let mut p = Palette::GRAY;
        let colors = BTreeMap::from([
            ("bgg".to_string(), "#000000".to_string()),
            ("text".to_string(), "red".to_string()),
            ("accent".to_string(), "#56b4e9".to_string()),
        ]);
        let problems = p.apply(&colors);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems.iter().any(|m| m.contains("\"bgg\"")));
        assert!(problems.iter().any(|m| m.contains("\"red\"")));
        assert_eq!(p.text, Palette::GRAY.text);
        assert_eq!(p.accent, rgb(0x56B4E9));
    }

    #[test]
    fn wcag_contrast_matches_known_values() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(rgb(0x777777), Color32::WHITE) - 4.48).abs() < 0.01);
    }

    #[test]
    fn selected_text_is_drawn_in_the_selection_foreground() {
        for (_, p) in BUILT_IN {
            let v = p.visuals();
            assert_eq!(v.selection.bg_fill, p.sel_bg);
            assert_eq!(v.selection.stroke.color, p.sel_fg);
        }
    }
}
