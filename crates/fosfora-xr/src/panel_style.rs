//! How the hand menu's cells and the floating plates are colored (board
//! #5553). Plain data, so the rules build and test on the desktop;
//! `hud.rs` and `label.rs` turn a [`Token`] into the Blue and orange
//! theme's color (`Palette::BLUE_ORANGE`, the desktop's tokens).
//!
//! **Blue rests, orange is on or now.** Until this the menu at rest was
//! one blue: orange showed only under the pointer and on a press, and
//! whether Voice or Roam was on stood in the words alone. Now:
//!
//! - **Off**: the blue block every pressable thing is, light text.
//! - **On** (a two-state toggle that is on, the music while it plays): the
//!   same block with an orange outline, and the value ("on", the track's
//!   title) in orange.
//! - **Now** (the Theme control's theme): the value in orange, no
//!   outline. A control that steps through several values is never "on".
//! - **Hover** (the pointer is over it): filled orange, dark text. The
//!   largest change of the four, since a wearer has to see where the
//!   pointer is before pinching, with one eye too. A toggle that is on
//!   keeps an outline, in the dark text's color.
//! - **Press**: the light text's color as the fill, dark text and an
//!   orange outline, for as long as the press is held. Until this a press
//!   was the orange fill, which the pointer has now.
//!
//! Nothing is told apart by hue alone: on is an outline as well as a
//! color, the pointer and a press are fills a step apart in lightness
//! (blue, orange, near white), and the words still say "on" and "off".
//!
//! **One plate.** The hand menu, the label (the flash question, "Pinch to
//! begin", the hints, the voice's label and card, the menu's replies) and
//! the surface labels all sit on the same navy plate with a thin outline
//! in the theme's line blue, so a plate keeps its edge over a bright sky.

/// A color of the Blue and orange theme, by its token's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    /// The plates' ground.
    Panel,
    /// A cell with nothing to press.
    Well,
    /// A pressable cell at rest.
    Rule,
    /// The plates' outline.
    Line,
    /// Light text.
    Text,
    /// A name over a value.
    Sub,
    /// The pointer's fill.
    SelBg,
    /// Dark text, on the orange fills.
    SelFg,
    /// On and now: outlines and values. A press's outline.
    Accent,
}

/// What a pressable cell is doing, the strongest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// Held by a press.
    Press,
    /// Under the pointer.
    Hover,
    /// A toggle that is on.
    On,
    /// At rest.
    Off,
}

/// The look of a cell `pressed`, `hovered` (under the pointer) or `on`.
pub const fn look(pressed: bool, hovered: bool, on: bool) -> Look {
    if pressed {
        Look::Press
    } else if hovered {
        Look::Hover
    } else if on {
        Look::On
    } else {
        Look::Off
    }
}

/// An outline just inside a cell's edge: its width (points) and color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outline {
    pub width: f32,
    pub color: Token,
}

/// A cell's colors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub fill: Token,
    /// The words, or a name beside its value.
    pub text: Token,
    /// The value: "on", a track's title, a theme's name.
    pub value: Token,
    /// A name over a value.
    pub sub: Token,
    pub outline: Option<Outline>,
}

/// The outline of a toggle that is on (points).
pub const ON_OUTLINE_PT: f32 = 2.0;
/// The outline of a press (points).
pub const PRESS_OUTLINE_PT: f32 = 3.0;
/// The hand menu's plate outline (points; 1.6 texture pixels a point).
pub const PLATE_OUTLINE_PT: f32 = 2.0;
/// The label's and the surface labels' plate outline (texture pixels):
/// about 1.5 mm on a 0.4 m card, two pixels of the display at a meter.
pub const PLATE_OUTLINE_PX: f32 = 4.0;
/// The surface labels' (texture pixels of their smaller cells).
pub const TAG_OUTLINE_PX: f32 = 3.0;
/// The flash question's answers as buttons on the card (texture pixels):
/// how far a button's plate reaches past its words at each end, how much
/// shorter than a line's pitch it is, its corner radius and its outline at
/// rest. The words stay where the line put them; the plate is drawn
/// around them.
pub const ANSWER_PAD_PX: f32 = 22.0;
pub const ANSWER_SHORT_PX: f32 = 12.0;
pub const ANSWER_CORNER_PX: f32 = 18.0;
pub const ANSWER_OUTLINE_PX: f32 = 3.0;
// Shorter than a line's pitch, so a plate touches neither the line over
// the answers nor the countdown under them.
const _: () = assert!(ANSWER_SHORT_PX >= 8.0);

/// The colors of a cell with this `look`; `on` is whether it is a toggle
/// that is on, which a hovered cell still marks.
pub const fn style(look: Look, on: bool) -> Style {
    match look {
        Look::Off => Style {
            fill: Token::Rule,
            text: Token::Text,
            value: Token::Text,
            sub: Token::Sub,
            outline: None,
        },
        Look::On => Style {
            fill: Token::Rule,
            text: Token::Text,
            value: Token::Accent,
            sub: Token::Sub,
            outline: Some(Outline {
                width: ON_OUTLINE_PT,
                color: Token::Accent,
            }),
        },
        Look::Hover => Style {
            fill: Token::SelBg,
            text: Token::SelFg,
            value: Token::SelFg,
            sub: Token::SelFg,
            outline: if on {
                Some(Outline {
                    width: ON_OUTLINE_PT,
                    color: Token::SelFg,
                })
            } else {
                None
            },
        },
        Look::Press => Style {
            fill: Token::Text,
            text: Token::SelFg,
            value: Token::SelFg,
            sub: Token::SelFg,
            outline: Some(Outline {
                width: PRESS_OUTLINE_PT,
                color: Token::Accent,
            }),
        },
    }
}

/// The value's color for a control that shows what is on now without
/// being a toggle (the Theme control): orange at rest, the look's own
/// otherwise.
pub const fn now_value(look: Look, style: Style) -> Token {
    match look {
        Look::Off => Token::Accent,
        _ => style.value,
    }
}

/// A one-line control's words as the name with its separator and the
/// value, so the value can take its own color: "Voice: on" is ("Voice: ",
/// "on"), and "Recenter" has no value. The two parts laid out one after
/// the other are as wide as the whole, so nothing moves.
pub fn name_value(words: &str) -> (&str, &str) {
    match words.find(": ") {
        Some(i) => words.split_at(i + 2),
        None => (words, ""),
    }
}

#[cfg(test)]
mod tests {
    use fosfora_app::ui::theme::palette::Palette;

    use super::*;

    const LOOKS: [Look; 4] = [Look::Press, Look::Hover, Look::On, Look::Off];

    /// A token's color in the theme the menu draws with, as sRGB bytes.
    fn rgb(token: Token) -> [u8; 3] {
        let p = Palette::BLUE_ORANGE;
        let c = match token {
            Token::Panel => p.panel,
            Token::Well => p.well,
            Token::Rule => p.rule,
            Token::Line => p.line,
            Token::Text => p.text,
            Token::Sub => p.sub,
            Token::SelBg => p.sel_bg,
            Token::SelFg => p.sel_fg,
            Token::Accent => p.accent,
        };
        [c.r(), c.g(), c.b()]
    }

    /// WCAG relative luminance.
    fn luminance(token: Token) -> f32 {
        let lin = |v: u8| {
            let v = f32::from(v) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let [r, g, b] = rgb(token);
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    }

    /// WCAG contrast ratio of two colors.
    fn contrast(a: Token, b: Token) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn a_press_beats_the_pointer_which_beats_on() {
        assert_eq!(look(true, true, true), Look::Press);
        assert_eq!(look(true, false, false), Look::Press);
        assert_eq!(look(false, true, true), Look::Hover);
        assert_eq!(look(false, false, true), Look::On);
        assert_eq!(look(false, false, false), Look::Off);
    }

    #[test]
    fn every_text_reads_on_its_fill() {
        // 4.5:1, WCAG's floor for text, for the words, the value and the
        // name over a value, in every look, on or not.
        for l in LOOKS {
            for on in [false, true] {
                let s = style(l, on);
                for (what, ink) in [("text", s.text), ("value", s.value), ("sub", s.sub)] {
                    let c = contrast(ink, s.fill);
                    assert!(
                        c >= 4.5,
                        "{l:?} on {on}: {what} {ink:?} on {:?} {c:.2}",
                        s.fill
                    );
                }
                // The Theme control's value.
                let now = now_value(l, s);
                assert!(contrast(now, s.fill) >= 4.5, "{l:?}: now {now:?}");
            }
        }
        // The plates' text on their ground, and a status cell's on its well.
        assert!(contrast(Token::Text, Token::Panel) >= 7.0);
        assert!(contrast(Token::Text, Token::Well) >= 7.0);
    }

    #[test]
    fn the_four_looks_differ_by_more_than_hue() {
        let (off, on) = (style(Look::Off, false), style(Look::On, true));
        let (hover, press) = (style(Look::Hover, false), style(Look::Press, false));
        // On is off with an outline and its value in the outline's color.
        assert_eq!((on.fill, on.text), (off.fill, off.text));
        assert!(off.outline.is_none());
        let outline = on.outline.unwrap();
        assert_eq!(outline.color, on.value);
        assert_ne!(on.value, off.value);
        // An outline has to stand off the fill it runs along: 3:1, WCAG's
        // floor for a graphic.
        assert!(contrast(outline.color, on.fill) >= 3.0);
        // The pointer is a fill, far from the block at rest in lightness,
        // so it shows without color vision.
        assert_ne!(hover.fill, off.fill);
        assert!(contrast(hover.fill, off.fill) >= 3.0);
        assert!(hover.outline.is_none());
        // Hovered, a toggle that is on keeps an outline.
        let kept = style(Look::Hover, true).outline.unwrap();
        assert!(contrast(kept.color, hover.fill) >= 3.0);
        // A press is a lighter fill again than the pointer's, with an
        // outline wider than on's.
        assert!(luminance(press.fill) > luminance(hover.fill));
        assert!(contrast(press.fill, hover.fill) >= 1.5);
        let pressed = press.outline.unwrap();
        assert!(pressed.width > outline.width);
        assert!(contrast(pressed.color, press.fill) >= 1.5);
        // The plates' outline stands off their ground.
        assert!(contrast(Token::Line, Token::Panel) >= 3.0);
        // Orange and the resting blue are far apart in lightness, which
        // carries the difference for a wearer who sees no hue.
        assert!(contrast(Token::Accent, Token::Rule) >= 4.5);
    }

    #[test]
    fn the_theme_controls_value_is_orange_only_at_rest() {
        for l in LOOKS {
            let s = style(l, false);
            let want = if l == Look::Off {
                Token::Accent
            } else {
                s.value
            };
            assert_eq!(now_value(l, s), want, "{l:?}");
        }
    }

    #[test]
    fn a_controls_words_split_into_the_name_and_the_value() {
        assert_eq!(name_value("Voice: on"), ("Voice: ", "on"));
        assert_eq!(
            name_value("Limit flashing: off"),
            ("Limit flashing: ", "off")
        );
        assert_eq!(name_value("Roam: on (no room)"), ("Roam: ", "on (no room)"));
        assert_eq!(name_value("Recenter"), ("Recenter", ""));
        // The parts are the whole, in order: laid out one after the other
        // they are the same glyphs.
        for words in ["Edit room: on", "All: none", "Rescan", "Agent: local"] {
            let (name, value) = name_value(words);
            assert_eq!(format!("{name}{value}"), words);
        }
    }
}
