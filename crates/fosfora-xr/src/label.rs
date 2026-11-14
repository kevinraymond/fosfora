//! The room editor's label (board #3326, step 2b): after each tap or hold,
//! a short line of text floats at the hit for [`LABEL_S`], naming the
//! result ("desk: sparks", "all tables: none", "ceiling: nothing to
//! change"), or after a first hold what a second would do ("desk: hold
//! again for all tables -> curls", board #3336), because the only other naming of it is the palm panel's
//! status cell, which the wearer is not looking at while pointing (Kevin,
//! worn, Sep 29). The text, the fade and the pose are plain numbers, so
//! they build and test on the desktop; on Android [`LabelTexture`]
//! renders the text with egui into a texture of its own that `gfx.rs`
//! draws with the panel's pipeline, right after the panel.
//!
//! **The look.** White text on a dark rounded ground at alpha
//! [`GROUND_ALPHA`], egui's default fonts as on the panel: text on a
//! ground, never hue alone, so it reads with one eye against any wall.
//! The texture is [`LABEL_TEX`], shown at its aspect [`label_w_m`] wide
//! for its distance from the head, so it keeps about the same angular size
//! from the chair to the far wall (step 2c: a fixed 0.28 m label was a few
//! dozen eye pixels at 3 m, "just a blob of pixels", Kevin, worn, Sep 29);
//! it fades out over the last [`FADE_S`] (the fade scales the ground and
//! the text alike).
//!
//! **How long** (board #5461). A label holds long enough to read: its
//! [`read_s`], [`READ_BASE_S`] and [`READ_WORD_S`] a word, at most
//! [`READ_MAX_S`]. Most labels held [`LABEL_S`], 1.5 s, which left a
//! sentence about a second at full strength. [`Label::show`] holds a text
//! for its reading time, [`Label::show_for`] for a caller's time or the
//! reading time, whichever is longer (the voice's replies and misses keep
//! their own times as a floor), and [`Label::show_held`] for exactly a
//! caller's time: an arm's "hold again" goes when the arm's window closes
//! (`room_edit::ARM_S`), whatever its reading time.
//!
//! **Lines and sizes** (board #5461, [`layout`]). A text that fits one line
//! at [`LINES_FONT_PX`] or larger shows on one line, as large as fits up to
//! [`FONT_PX`]. A longer one wraps instead of shrinking: a sentence a line,
//! and a sentence that still runs over on as few lines as it takes, as
//! even as they come, a line ending at a colon when that costs no extra
//! line. It takes the largest size that fits, up to
//! [`TWO_LINES_FONT_PX`] for two lines and [`LINES_FONT_PX`] for more. Before,
//! a long label shrank to fit one line down to [`MIN_FONT_PX`] and was
//! clipped past it: "No room found. Rescan from the hand menu" drew at
//! 46 px, an arm's "hold again" at 48, and a theme miss ran off both ends.
//! The widths are the font's own (`text_width.rs`), so the wrap tests on
//! the desktop.
//!
//! **The pose.** A billboard: centered at the hit moved [`OUT_M`] out
//! along the face normal and [`UP_M`] up, facing the head, its right the
//! head's right projected onto the plane across the line of sight to it
//! (so it rolls with the head and never mirrors), its up from those two.
//!
//! **The scan label** (board #3488). A launch whose scene query has not
//! returned anchors was silent in the headset (the log said "scene: no
//! anchors yet, retrying"; 0 anchors on eight launches in a row, Sep 30).
//! The same label, with no hit, says where the room scan is
//! ([`ScanLabel`], from `room.rs`'s [`ScanState`]): "Scanning the room…"
//! held while the query has not returned anchors, "Loading your room" for
//! [`SCAN_FOUND_S`] when it does (board #4204: the count, "Room: 17
//! surfaces" until then, is the log's alone), "No room found. Rescan from the hand
//! menu" for [`SCAN_NONE_S`] when the retries give up, each fading over
//! [`FADE_S`]; re-posed every frame [`SCAN_DISTANCE_M`] in front of the
//! head at eye height ([`scan_billboard`]). The editor's label, while one
//! shows, takes its place. When the scan's end turns roam on (board
//! #5393), the none reads "No room here: roaming. Rescan from the hand
//! menu" instead (`roam::scan_text`, [`ScanLabel::step_with`]).
//!
//! **The permission** (board #3264). Without `USE_SCENE` the query returns
//! nothing, so while the app waits for the grant (`permissions::Watch`)
//! and the room has no anchors the label reads "Allow spatial data to see
//! the room" instead ([`ScanState::NotAllowed`], [`with_scene_permission`]),
//! held like the scan; once the grant arrives the requery scans again, and
//! once the wait gives up the room's own state shows.
//!
//! **The voice path's label** (board #3751, V4) is the same label with two
//! more things the render reads: a meter, a thin bar under "Listening…"
//! whose length is the microphone's level ([`Label::meter`],
//! `voice::meter_width`), and more than one line ("Didn't catch that" and
//! a hint under it, or "what can I say"'s five examples): the text's
//! `\n`s are lines, and the label grows down and up from its center by
//! [`LINE_PX`] of the texture a line ([`tex_h`], [`Billboard::with_lines`]),
//! up to [`MAX_LINES`].
//!
//! **The surface labels** (board #5378, `surface_labels.rs`) put every
//! surface's name on the surface in this label's look, from one atlas
//! drawn through the same `TextureCanvas`; their second line is the
//! status cell's word for what the surface runs ([`behavior_word`]).

use glam::{Quat, Vec3};

use crate::surfaces::SurfaceBehavior;

/// The shortest a label shows (s), and the fade at its end (s).
pub const LABEL_S: f32 = 1.5;
pub const FADE_S: f32 = 0.4;
/// A label's reading time ([`read_s`]): this much, and this much a word,
/// at most [`READ_MAX_S`] (s).
pub const READ_BASE_S: f32 = LABEL_S;
pub const READ_WORD_S: f32 = 0.3;
pub const READ_MAX_S: f32 = 6.0;
/// The label's texture, width x height (px): large enough that the text
/// is sharp up close, where the label is its narrowest.
pub const LABEL_TEX: [u32; 2] = [1024, 192];
/// The texture's height per line of a label of more than one line (px):
/// a one-line label is [`LABEL_TEX`] high, two of these.
pub const LINE_PX: u32 = 96;
/// The most lines a label shows; a longer text's last ones are dropped.
pub const MAX_LINES: usize = 6;
/// The texture's height, for [`MAX_LINES`] (px); a label of fewer lines
/// shows its top [`tex_h`].
pub const LABEL_TEX_H_MAX: u32 = tex_h(MAX_LINES);

/// The texture's height a label of `lines` lines shows (px): a line's
/// pitch each, and one more for the margins, [`LABEL_TEX`]'s height for
/// one.
pub const fn tex_h(lines: usize) -> u32 {
    let lines = if lines == 0 {
        1
    } else if lines > MAX_LINES {
        MAX_LINES
    } else {
        lines
    };
    LINE_PX * (lines as u32 + 1)
}

/// The lines of a label's text: split at `\n`, at most [`MAX_LINES`].
pub fn lines(text: &str) -> Vec<&str> {
    text.split('\n').take(MAX_LINES).collect()
}

/// The text's largest size on a label of one line, of two, and of more,
/// and its smallest (px of the texture). Two lines at 76 px keep a gap
/// between them on the [`LINE_PX`] pitch; more lines (the voice's help
/// and lists) keep the 64 they had.
pub const FONT_PX: f32 = 88.0;
pub const TWO_LINES_FONT_PX: f32 = 76.0;
pub const LINES_FONT_PX: f32 = 64.0;
pub const MIN_FONT_PX: f32 = 40.0;
/// The ground's inset from the texture's edge, and the margin the text
/// keeps from the ground's ends (px).
pub const INSET_PX: f32 = 4.0;
pub const MARGIN_PX: f32 = 48.0;
/// The width a line of text has on the texture (px).
pub const TEXT_W_PX: f32 = LABEL_TEX[0] as f32 - 2.0 * (INSET_PX + MARGIN_PX);
/// The step between the sizes [`layout`] tries (px).
const SIZE_STEP_PX: f32 = 2.0;

/// The words of `text` a reader reads: the runs between spaces that hold a
/// letter or a digit ("·" and "->" are not words).
pub fn words(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
}

/// How long `text` needs to be read (board #5461): [`READ_BASE_S`] and
/// [`READ_WORD_S`] a word, at most [`READ_MAX_S`]. "desk: sparks" 2.1 s,
/// a voice miss with its hint about 3.6, "what can I say" 6.
pub fn read_s(text: &str) -> f32 {
    (READ_BASE_S + READ_WORD_S * words(text) as f32).min(READ_MAX_S)
}

/// A label's text as the texture shows it ([`layout`]): its lines and
/// their size (px of the texture).
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub lines: Vec<String>,
    pub px: f32,
}

/// The largest size the text of a label of `lines` lines takes (px).
pub fn font_cap(lines: usize) -> f32 {
    match lines {
        0 | 1 => FONT_PX,
        2 => TWO_LINES_FONT_PX,
        _ => LINES_FONT_PX,
    }
}

/// How `text` shows on the label (board #5461). One line when it fits at
/// [`LINES_FONT_PX`] or larger, as large as fits up to [`FONT_PX`].
/// Otherwise its `\n`s and its sentences start lines and a line that still
/// runs over wraps (`wrap`), at the largest size from
/// [`TWO_LINES_FONT_PX`] down that fits its lines within [`font_cap`] and
/// [`MAX_LINES`]. A text too long even at [`MIN_FONT_PX`] keeps its first
/// [`MAX_LINES`] lines at that size, the last cut with an ellipsis.
pub fn layout(text: &str) -> Layout {
    let em = crate::text_width::em;
    if !text.contains('\n') {
        let width = em(text);
        let px = if width > 0.0 {
            (TEXT_W_PX / width).min(FONT_PX)
        } else {
            FONT_PX
        };
        if px >= LINES_FONT_PX {
            return Layout {
                lines: vec![text.to_owned()],
                px,
            };
        }
    }
    let widest = |lines: &[String]| lines.iter().map(|l| em(l)).fold(0.0, f32::max);
    let mut px = TWO_LINES_FONT_PX;
    while px >= MIN_FONT_PX {
        let lines = wrap(text, TEXT_W_PX / px);
        let n = lines.len();
        if n <= MAX_LINES && px <= font_cap(n) && widest(&lines) * px <= TEXT_W_PX {
            let px = (TEXT_W_PX / widest(&lines)).min(font_cap(n));
            return Layout { lines, px };
        }
        px -= SIZE_STEP_PX;
    }
    let mut lines = wrap(text, TEXT_W_PX / MIN_FONT_PX);
    if lines.len() > MAX_LINES {
        lines.truncate(MAX_LINES);
        if let Some(last) = lines.last_mut() {
            last.push('\u{2026}');
        }
    }
    Layout {
        lines,
        px: MIN_FONT_PX,
    }
}

/// `text` on lines of at most `max_em` ems: each `\n` starts a line; a
/// part that runs over starts a line at each sentence (after ". ", "? ",
/// "! "), and a sentence that still runs over wraps ([`wrap_words`]). A
/// word wider than a line has a line of its own.
fn wrap(text: &str, max_em: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for part in text.split('\n') {
        if crate::text_width::em(part) <= max_em {
            lines.push(part.to_owned());
            continue;
        }
        let mut sentence: Vec<&str> = Vec::new();
        for word in part.split_whitespace() {
            sentence.push(word);
            if word.ends_with(['.', '?', '!']) {
                lines.extend(wrap_words(&sentence, max_em));
                sentence.clear();
            }
        }
        lines.extend(wrap_words(&sentence, max_em));
    }
    lines
}

/// `words` on as few lines of at most `max_em` ems as they take
/// ([`even_lines`]), with a line ending at a colon when that costs no more
/// lines (`No theme called "x":` over the names, "Fosfora stopped:" over
/// the reason): of the colons that do, the one whose widest line is
/// narrowest.
fn wrap_words(words: &[&str], max_em: f32) -> Vec<String> {
    let lines = even_lines(words, max_em);
    let widest = |lines: &[String]| {
        lines
            .iter()
            .map(|l| crate::text_width::em(l))
            .fold(0.0, f32::max)
    };
    (1..words.len())
        .filter(|&i| words[i - 1].ends_with(':'))
        .map(|i| {
            let mut at = even_lines(&words[..i], max_em);
            at.extend(even_lines(&words[i..], max_em));
            at
        })
        .filter(|at| at.len() == lines.len())
        .min_by(|a, b| widest(a).total_cmp(&widest(b)))
        .unwrap_or(lines)
}

/// `words` on as few lines of at most `max_em` ems as they take (filling
/// each line in turn takes the fewest), and as even as that allows: filled
/// again to the narrowest width that needs no more. A word wider than
/// `max_em` has a line of its own.
fn even_lines(words: &[&str], max_em: f32) -> Vec<String> {
    let em = |ws: &[&str]| crate::text_width::em(&ws.join(" "));
    let fill = |width: f32| {
        let mut lines: Vec<Vec<&str>> = Vec::new();
        for &word in words {
            match lines.last_mut() {
                Some(line) if em(&[line.as_slice(), &[word]].concat()) <= width => {
                    line.push(word);
                }
                _ => lines.push(vec![word]),
            }
        }
        lines
    };
    let count = fill(max_em).len();
    if count <= 1 {
        return fill(max_em).iter().map(|l| l.join(" ")).collect();
    }
    let (mut narrow, mut wide) = (0.0, max_em);
    for _ in 0..24 {
        let mid = 0.5 * (narrow + wide);
        if fill(mid).len() <= count {
            wide = mid;
        } else {
            narrow = mid;
        }
    }
    fill(wide).iter().map(|l| l.join(" ")).collect()
}

/// The label's width in the room per meter from the head (m/m, about 6.3
/// degrees across), and its narrowest and widest (m); its height by the
/// texture's aspect.
pub const LABEL_W_PER_M: f32 = 0.11;
pub const LABEL_W_MIN_M: f32 = 0.28;
pub const LABEL_W_MAX_M: f32 = 1.4;
/// The label's center off the hit: out along the face normal and up (m).
pub const OUT_M: f32 = 0.08;
pub const UP_M: f32 = 0.06;
/// The ground's alpha at full label.
pub const GROUND_ALPHA: f32 = 0.85;

/// What an editor action came to: `after`, or "nothing to change" when it
/// is what the surface already ran.
fn result(before: SurfaceBehavior, after: SurfaceBehavior) -> &'static str {
    if before == after {
        "nothing to change"
    } else {
        after.name()
    }
}

/// The label for a tap: the surface's name (`surfaces::friendly_name`)
/// and its new behavior, "desk: sparks".
pub fn cycle_text(name: &str, before: SurfaceBehavior, after: SurfaceBehavior) -> String {
    format!("{name}: {}", result(before, after))
}

/// What a surface runs, in one word: its effective behavior's name
/// (`lanes::RoomLanes::effective`), "?" for a box the lanes do not have.
/// The editor's status cell ([`status_text`]) and the surface labels'
/// second line (`surface_labels.rs`, board #5378) both read it.
pub fn behavior_word(effective: Option<(SurfaceBehavior, f32)>) -> &'static str {
    effective.map_or("?", |(b, _)| b.name())
}

/// The editor's status cell for the surface under the beam: its name
/// (`surfaces::friendly_name`) and [`behavior_word`], with " · armed"
/// while the class cycle is armed on it (board #3336): "desk: curls",
/// "desk: curls · armed".
pub fn status_text(name: &str, effective: Option<(SurfaceBehavior, f32)>, armed: bool) -> String {
    let armed = if armed { " \u{b7} armed" } else { "" };
    format!("{name}: {}{armed}", behavior_word(effective))
}

/// The status cell with the editor on and nothing under the beam: what to
/// do, not "no surface" (board #5461).
pub const STATUS_NO_SURFACE: &str = "Point at a surface";
/// The status cell with the editor off: empty. It read "edit room off"
/// beside the "Edit room: off" button, so it looked like a second button
/// saying the same thing (board #5461).
pub const STATUS_OFF: &str = "";

/// The label for a step of the hand menu's surface rows (board #3472,
/// D3): the surface's name, its behavior and the three values it runs
/// with now, color, band and strength (`lanes::RoomLanes::params_of`),
/// "desk: curls · amber · mid · 0.7".
pub fn param_text(name: &str, behavior: SurfaceBehavior, params: (u32, u32, f32)) -> String {
    let (color, band, strength) = params;
    format!(
        "{name}: {} · {} · {} · {strength:.1}",
        behavior.name(),
        crate::surfaces::color_name(color),
        crate::surface_fx::band_name(band)
    )
}

/// A label's text for an action whose result `after` spawns particles
/// (embers, sparks) on a surface whose top face is `outside` the volume
/// the effect runs in (`surfaces::reaches`; its emitter weight is 0):
/// "table 13: sparks (outside the space)". The room fit (step 2h) makes it
/// rare; it stays for an asked space size too small for the surface.
/// Without it the editor assigned a surface that could not emit and said
/// nothing (Kevin, worn, Sep 30). Any other result, the text as it is.
pub fn space_text(text: String, after: SurfaceBehavior, outside: bool) -> String {
    if outside && after.emits() {
        format!("{text} (outside the space)")
    } else {
        text
    }
}

/// The label for a hold: every surface of `kind` and their new behavior,
/// "all tables: none".
pub fn class_text(kind: u32, before: SurfaceBehavior, after: SurfaceBehavior) -> String {
    format!("all {}: {}", kind_plural(kind), result(before, after))
}

/// The label for a first hold, which arms the class cycle (board #3336):
/// the surface's name (`surfaces::friendly_name`), and what a second hold
/// would do, in [`class_text`]'s words, "desk: hold again for all tables
/// -> streamlines".
pub fn arm_text(name: &str, kind: u32, next: SurfaceBehavior) -> String {
    format!(
        "{name}: hold again for all {} -> {}",
        kind_plural(kind),
        next.name()
    )
}

/// A kind's name (`surfaces::kind_name`) for "all ...": "tables",
/// "other surfaces", "unlabeled surfaces" for an anchor without labels.
pub fn kind_plural(kind: u32) -> String {
    match crate::surfaces::kind_name(kind) {
        "other" => "other surfaces".to_owned(),
        "none" => "unlabeled surfaces".to_owned(),
        name => format!("{name}s"),
    }
}

/// The label's width in the room (m) at `distance_m` from the head:
/// [`LABEL_W_PER_M`] per meter, no narrower than [`LABEL_W_MIN_M`] (the
/// step 2b label, from arm's length to 2.5 m) and no wider than
/// [`LABEL_W_MAX_M`] (from 12.7 m, past the editor's 8 m cast).
pub fn label_w_m(distance_m: f32) -> f32 {
    (LABEL_W_PER_M * distance_m).clamp(LABEL_W_MIN_M, LABEL_W_MAX_M)
}

/// The label's alpha at `age` seconds: 1 until the fade, then linearly to
/// 0 at [`LABEL_S`].
pub fn fade(age: f32) -> f32 {
    fade_for(age, LABEL_S)
}

/// The alpha at `age` seconds of a label that shows for `life` seconds:
/// 1 until the last [`FADE_S`], then linearly to 0 at `life`.
pub fn fade_for(age: f32, life: f32) -> f32 {
    ((life - age) / FADE_S).clamp(0.0, 1.0)
}

/// The label's quad: its center and the unit vectors along its right and
/// up edges, with its half extents (m).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Billboard {
    pub center: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub half_w: f32,
    pub half_h: f32,
}

impl Billboard {
    /// This pose for a label of `lines` lines: as wide, taller by the
    /// texture's height for them ([`tex_h`]), about the same center.
    #[must_use]
    pub fn with_lines(self, lines: usize) -> Self {
        Self {
            half_h: self.half_h * tex_h(lines) as f32 / LABEL_TEX[1] as f32,
            ..self
        }
    }
}

/// The label's pose for a hit at `point` on a face with outward `normal`,
/// seen from `head` turned by `head_rot`, [`label_w_m`] wide for its
/// center's distance from the head.
pub fn billboard(point: Vec3, normal: Vec3, head: Vec3, head_rot: Quat) -> Billboard {
    facing(
        point + normal.normalize_or_zero() * OUT_M + Vec3::Y * UP_M,
        head,
        head_rot,
    )
}

/// The scan label's pose: [`SCAN_DISTANCE_M`] in front of `head` along its
/// view turned level, at eye height, facing the head.
pub fn scan_billboard(head: Vec3, head_rot: Quat) -> Billboard {
    let ahead = head_rot * Vec3::NEG_Z;
    let level = Vec3::new(ahead.x, 0.0, ahead.z).normalize_or(Vec3::NEG_Z);
    facing(head + level * SCAN_DISTANCE_M, head, head_rot)
}

/// A label centered at `center` facing `head` turned by `head_rot`.
pub(crate) fn facing(center: Vec3, head: Vec3, head_rot: Quat) -> Billboard {
    let view = (center - head).normalize_or(head_rot * Vec3::NEG_Z);
    let head_right = head_rot * Vec3::X;
    // The head's right across the line of sight; with the label nearly
    // along the head's right itself (far out of view, where that
    // projection turns anywhere), the level right instead.
    let across = head_right - view * head_right.dot(view);
    let right = if across.length() > 0.2 {
        across.normalize()
    } else {
        Vec3::Y.cross(-view).normalize_or(Vec3::X)
    };
    let up = right.cross(view);
    let half_w = label_w_m(center.distance(head)) * 0.5;
    Billboard {
        center,
        right,
        up,
        half_w,
        half_h: half_w * LABEL_TEX[1] as f32 / LABEL_TEX[0] as f32,
    }
}

/// A label showing, as [`Label::now`] gives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabelFrame<'a> {
    pub text: &'a str,
    pub alpha: f32,
    /// The hit and its face normal.
    pub point: Vec3,
    pub normal: Vec3,
    /// The meter's level under the text (the voice window's microphone,
    /// RMS on the ±1 scale), `None` for no meter.
    pub level: Option<f32>,
}

/// The label across frames: at most one, the last action's.
#[derive(Debug, Clone, Default)]
pub struct Label {
    /// The text, the hit's point and normal, the age and the life (s).
    shown: Option<(String, Vec3, Vec3, f32, f32)>,
    /// The meter's level, until the next label.
    level: Option<f32>,
}

impl Label {
    /// Show `text` at the hit `point` on a face with `normal`, from full,
    /// in place of any label showing, for its reading time ([`read_s`]).
    pub fn show(&mut self, text: String, point: Vec3, normal: Vec3) {
        self.show_for(text, point, normal, LABEL_S);
    }

    /// [`Label::show`] for `seconds` or the text's reading time, whichever
    /// is longer, fading over its last [`FADE_S`] the same way: the voice
    /// window's "Listening…" for the whole window, a reply for its own time
    /// at least.
    pub fn show_for(&mut self, text: String, point: Vec3, normal: Vec3, seconds: f32) {
        let seconds = seconds.max(read_s(&text));
        self.show_held(text, point, normal, seconds);
    }

    /// [`Label::show`] for exactly `seconds`: the arm's "hold again" goes
    /// when the arm's window closes (`room_edit::ARM_S`, board #3336).
    pub fn show_held(&mut self, text: String, point: Vec3, normal: Vec3, seconds: f32) {
        self.shown = Some((text, point, normal, 0.0, seconds));
        self.level = None;
    }

    /// The meter under the label showing: its level this frame (`None`
    /// hides it). The next label shows without one.
    pub fn meter(&mut self, level: Option<f32>) {
        self.level = level.filter(|_| self.shown.is_some());
    }

    /// Age the label by `dt`; it goes at its life.
    pub fn step(&mut self, dt: f32) {
        if let Some((.., age, _)) = self.shown.as_mut() {
            *age += dt.max(0.0);
        }
        self.shown = self.shown.take().filter(|(.., age, life)| *age < *life);
        if self.shown.is_none() {
            self.level = None;
        }
    }

    /// Drop the label (the mode went off).
    pub fn clear(&mut self) {
        self.shown = None;
        self.level = None;
    }

    /// The label now, `None` when none shows.
    pub fn now(&self) -> Option<LabelFrame<'_>> {
        self.shown
            .as_ref()
            .map(|(text, point, normal, age, life)| LabelFrame {
                text,
                alpha: fade_for(*age, *life),
                point: *point,
                normal: *normal,
                level: self.level,
            })
    }
}

/// How long the scan label names the room once found, and says none was
/// found (s), before its fade; how far in front of the head it floats (m).
pub const SCAN_FOUND_S: f32 = 3.0;
pub const SCAN_NONE_S: f32 = 6.0;
pub const SCAN_DISTANCE_M: f32 = 1.2;

/// Where the room's scene query is (`Room::scan_state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanState {
    /// No query has returned anchors yet (the first, its retries, Space
    /// Setup and the query after it).
    Scanning,
    /// The last query that returned anchors returned this many.
    Found(usize),
    /// The retries gave up with no anchors.
    None,
    /// No anchors and `USE_SCENE` not granted yet: the query cannot see
    /// the room until the wearer allows spatial data.
    NotAllowed,
}

/// The scan label's state: the room's, or [`ScanState::NotAllowed`] while
/// the app waits for `USE_SCENE` (`waiting`) and the room has no anchors.
/// `None` (no room) stays `None`.
pub fn with_scene_permission(state: Option<ScanState>, waiting: bool) -> Option<ScanState> {
    match state {
        Some(ScanState::Scanning | ScanState::None) if waiting => Some(ScanState::NotAllowed),
        other => other,
    }
}

/// The scan label's words once the scan has found the room (board #4204).
pub const FOUND_LABEL: &str = "Loading your room";

/// The scan label's words for `state`.
pub fn scan_text(state: ScanState) -> String {
    match state {
        ScanState::Scanning => "Scanning the room\u{2026}".to_owned(),
        // Board #4204: the same words for any count; the log's
        // "room scan: Found(17)" keeps the count.
        ScanState::Found(_) => FOUND_LABEL.to_owned(),
        ScanState::None => "No room found. Rescan from the hand menu".to_owned(),
        ScanState::NotAllowed => "Allow spatial data to see the room".to_owned(),
    }
}

/// The scan label's alpha `age` seconds into `state`: held while
/// scanning or waiting for the permission; full for [`SCAN_FOUND_S`] or [`SCAN_NONE_S`], then linearly
/// to 0 over [`FADE_S`].
pub fn scan_fade(state: ScanState, age: f32) -> f32 {
    let hold = match state {
        ScanState::Scanning | ScanState::NotAllowed => return 1.0,
        ScanState::Found(_) => SCAN_FOUND_S,
        ScanState::None => SCAN_NONE_S,
    };
    ((hold + FADE_S - age) / FADE_S).clamp(0.0, 1.0)
}

/// The scan label across frames: the state it last saw, its words and how
/// long it has been in it. A change of state (another anchor count too)
/// starts it over; the same state ages it.
#[derive(Debug, Clone, Default)]
pub struct ScanLabel {
    shown: Option<(ScanState, String, f32)>,
}

impl ScanLabel {
    /// This frame's scan `state` (`None` without a room: nothing shows),
    /// `dt` seconds after the last.
    pub fn step(&mut self, state: Option<ScanState>, dt: f32) {
        self.step_with(state, dt, scan_text);
    }

    /// [`Self::step`] with the words of a new state from `text` instead of
    /// [`scan_text`] (board #5393: roam's "No room here: roaming…",
    /// `roam::scan_text`).
    pub fn step_with(
        &mut self,
        state: Option<ScanState>,
        dt: f32,
        text: impl FnOnce(ScanState) -> String,
    ) {
        match (state, self.shown.as_mut()) {
            (Some(s), Some((was, _, age))) if *was == s => *age += dt.max(0.0),
            (Some(s), _) => self.shown = Some((s, text(s), 0.0)),
            (None, _) => self.shown = None,
        }
    }

    /// The label's words and alpha now, `None` when it shows nothing.
    pub fn now(&self) -> Option<(&str, f32)> {
        let (state, text, age) = self.shown.as_ref()?;
        let alpha = scan_fade(*state, *age);
        (alpha > 0.0).then_some((text.as_str(), alpha))
    }
}

#[cfg(target_os = "android")]
pub use render::LabelTexture;
#[cfg(target_os = "android")]
pub(crate) use render::{PALETTE, TextureCanvas};

#[cfg(target_os = "android")]
mod render {
    use egui::{FontId, Pos2, Rect, Vec2};
    use fosfora_app::ui::theme::palette::Palette;

    use super::{
        Billboard, GROUND_ALPHA, INSET_PX, LABEL_TEX, LABEL_TEX_H_MAX, LINE_PX, Layout, MARGIN_PX,
        MIN_FONT_PX, layout, tex_h,
    };
    use crate::gfx::{Gfx, PanelPose};

    /// The label's colors: the hand menu's theme (board #3523). The
    /// surface labels' too (board #5378).
    pub(crate) const PALETTE: Palette = Palette::BLUE_ORANGE;

    /// The meter under "Listening…": its height, its bottom's distance
    /// from the ground's, and how far the text moves up for it (px).
    const METER_H_PX: f32 = 12.0;
    const METER_BOTTOM_PX: f32 = 30.0;
    const METER_LIFT_PX: f32 = 18.0;
    const CORNER_PX: u8 = 44;

    /// A texture egui paints into, with a context and a renderer of its
    /// own (`Hud::new`'s pattern): the label's, and the surface labels'
    /// atlas (`surface_labels.rs`, board #5378).
    pub(crate) struct TextureCanvas {
        ctx: egui::Context,
        renderer: egui_wgpu::Renderer,
        /// Owns the image the bind groups sample.
        _texture: wgpu::Texture,
        pub(crate) view: wgpu::TextureView,
        /// Its size (px), one egui point a pixel.
        size: [u32; 2],
        /// Its name in the GPU's labels.
        name: &'static str,
    }

    impl TextureCanvas {
        /// A transparent texture of `size`, named `name`.
        pub(crate) fn new(gfx: &Gfx, name: &'static str, size: [u32; 2]) -> Self {
            let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(name),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let renderer = egui_wgpu::Renderer::new(
                &gfx.device,
                wgpu::TextureFormat::Rgba8UnormSrgb,
                egui_wgpu::RendererOptions::default(),
            );
            let ctx = egui::Context::default();
            ctx.style_mut(|s| s.visuals = egui::Visuals::dark());
            Self {
                ctx,
                renderer,
                _texture: texture,
                view,
                size,
                name,
            }
        }

        /// Clear the texture and paint it: `paint` draws on egui's
        /// background layer over the whole texture, a point a pixel; one
        /// render pass and one submission.
        pub(crate) fn paint(&mut self, gfx: &Gfx, mut paint: impl FnMut(&egui::Painter)) {
            let size = Vec2::new(self.size[0] as f32, self.size[1] as f32);
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..egui::RawInput::default()
            };
            raw.viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .native_pixels_per_point = Some(1.0);
            let output = self.ctx.run(raw, |ctx| {
                paint(&ctx.layer_painter(egui::LayerId::background()));
            });
            for (id, delta) in &output.textures_delta.set {
                self.renderer
                    .update_texture(&gfx.device, &gfx.queue, *id, delta);
            }
            let jobs = self.ctx.tessellate(output.shapes, output.pixels_per_point);
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: self.size,
                pixels_per_point: output.pixels_per_point,
            };
            let mut encoder = gfx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some(self.name),
                });
            let user_buffers =
                self.renderer
                    .update_buffers(&gfx.device, &gfx.queue, &mut encoder, &jobs, &screen);
            {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(self.name),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                self.renderer
                    .render(&mut pass.forget_lifetime(), &jobs, &screen);
            }
            gfx.queue.submit(
                user_buffers
                    .into_iter()
                    .chain(std::iter::once(encoder.finish())),
            );
            for id in &output.textures_delta.free {
                self.renderer.free_texture(id);
            }
        }
    }

    /// The label's texture ([`TextureCanvas`]), redrawn only while a label
    /// shows and only when its text or its fade step changed.
    pub struct LabelTexture {
        canvas: TextureCanvas,
        /// The text, the alpha (in 1/255) and the meter's width (in
        /// 1/256) last drawn.
        drawn: Option<(String, u8, Option<u16>)>,
        /// The text last laid out and its layout (board #5461), worked out
        /// again only when the text changes.
        laid: Option<(String, Layout)>,
    }

    impl LabelTexture {
        /// Build the texture and hand it to `gfx`; hidden until [`Self::show`].
        pub fn new(gfx: &mut Gfx) -> Self {
            let canvas = TextureCanvas::new(gfx, "xr-label", [LABEL_TEX[0], LABEL_TEX_H_MAX]);
            gfx.set_label(&canvas.view);
            Self {
                canvas,
                drawn: None,
                laid: None,
            }
        }

        /// Show `text` at `alpha` on `pose` this frame, with a meter at
        /// `level` under it (the voice window's microphone), or hide the
        /// label with `None`. A text of several lines, its own or wrapped
        /// ([`layout`]), shows taller.
        pub fn show(&mut self, gfx: &Gfx, label: Option<(&str, f32, Option<f32>, Billboard)>) {
            let Some((text, alpha, level, pose)) = label else {
                self.drawn = None;
                gfx.set_label_pose(None);
                return;
            };
            if self.laid.as_ref().is_none_or(|(t, _)| t != text) {
                self.laid = Some((text.to_owned(), layout(text)));
            }
            let Some((_, laid)) = self.laid.as_ref() else {
                return;
            };
            let step = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
            let meter = level.map(|l| (crate::voice::meter_width(l) * 256.0).round() as u16);
            if self
                .drawn
                .as_ref()
                .is_none_or(|(t, a, m)| t != text || *a != step || *m != meter)
            {
                draw(
                    &mut self.canvas,
                    gfx,
                    laid,
                    f32::from(step) / 255.0,
                    meter.map(|m| f32::from(m) / 256.0),
                );
                self.drawn = Some((text.to_owned(), step, meter));
            }
            let n = laid.lines.len();
            let pose = pose.with_lines(n);
            gfx.set_label_pose(Some(PanelPose {
                center: pose.center.to_array(),
                right: (pose.right * pose.half_w).to_array(),
                up: (pose.up * pose.half_h).to_array(),
                v_max: tex_h(n) as f32 / LABEL_TEX_H_MAX as f32,
            }));
        }
    }

    /// Render `laid` at `alpha` into the texture's top [`tex_h`]: the
    /// rounded ground filling it, the text centered on it a line at a
    /// time at the layout's size (shrunk further should egui measure a
    /// line wider than the layout did, never under [`MIN_FONT_PX`]); with
    /// `meter` (0..1), a bar that long under a one-line text over its dim
    /// track.
    fn draw(canvas: &mut TextureCanvas, gfx: &Gfx, laid: &Layout, alpha: f32, meter: Option<f32>) {
        let tex_w = LABEL_TEX[0] as f32;
        let lines = &laid.lines;
        let shown_h = tex_h(lines.len()) as f32;
        canvas.paint(gfx, |painter| {
            let ground =
                Rect::from_min_size(Pos2::ZERO, Vec2::new(tex_w, shown_h)).shrink(INSET_PX);
            painter.rect_filled(
                ground,
                CORNER_PX,
                PALETTE.panel.gamma_multiply(GROUND_ALPHA * alpha),
            );
            let ink = PALETTE.text.gamma_multiply(alpha);
            let room = ground.width() - 2.0 * MARGIN_PX;
            let galleys_at = |px: f32| -> Vec<std::sync::Arc<egui::Galley>> {
                lines
                    .iter()
                    .map(|l| painter.layout_no_wrap(l.clone(), FontId::proportional(px), ink))
                    .collect()
            };
            let mut galleys = galleys_at(laid.px);
            let widest = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max);
            if widest > room {
                galleys = galleys_at((laid.px * room / widest).max(MIN_FONT_PX));
            }
            let lift = if meter.is_some() && galleys.len() == 1 {
                METER_LIFT_PX
            } else {
                0.0
            };
            for (i, galley) in galleys.into_iter().enumerate() {
                let center = if lines.len() > 1 {
                    Pos2::new(ground.center().x, (LINE_PX * (i as u32 + 1)) as f32)
                } else {
                    ground.center() - Vec2::new(0.0, lift)
                };
                painter.galley(center - galley.size() * 0.5, galley, ink);
            }
            if let Some(width) = meter {
                let bottom = ground.bottom() - METER_BOTTOM_PX;
                let track = Rect::from_min_max(
                    Pos2::new(ground.left() + MARGIN_PX, bottom - METER_H_PX),
                    Pos2::new(ground.right() - MARGIN_PX, bottom),
                );
                painter.rect_filled(track, METER_H_PX * 0.5, ink.gamma_multiply(0.25));
                let fill = Rect::from_min_size(
                    track.min,
                    Vec2::new(track.width() * width.clamp(0.0, 1.0), track.height()),
                );
                if fill.width() > 0.5 {
                    painter.rect_filled(fill, METER_H_PX * 0.5, ink);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surfaces::{KIND_CEILING, KIND_FRAME, KIND_NONE, KIND_OTHER, KIND_TABLE};
    use SurfaceBehavior as B;

    #[test]
    fn the_text_names_the_result() {
        assert_eq!(cycle_text("desk", B::Embers, B::Sparks), "desk: sparks");
        assert_eq!(cycle_text("wall 12", B::Spectrum, B::None), "wall 12: none");
        assert_eq!(
            class_text(KIND_TABLE, B::Sparks, B::None),
            "all tables: none"
        );
        assert_eq!(
            class_text(KIND_OTHER, B::None, B::Embers),
            "all other surfaces: embers"
        );
        assert_eq!(
            class_text(KIND_NONE, B::None, B::Embers),
            "all unlabeled surfaces: embers"
        );
        // The surface shaders by name (D1).
        let after = B::None.next_for(KIND_CEILING);
        assert_eq!(cycle_text("ceiling", B::None, after), "ceiling: rings");
        assert_eq!(
            class_text(KIND_FRAME, B::Rings, B::Rings.next_for(KIND_FRAME)),
            "all frames: streamlines"
        );
        // Nothing changed: an action that lands on what the surface ran.
        assert_eq!(
            cycle_text("ceiling", B::Rings, B::Rings),
            "ceiling: nothing to change"
        );
        // A ceiling the knob put on embers goes to the first after none.
        assert_eq!(
            cycle_text("ceiling", B::Embers, B::Embers.next_for(KIND_CEILING)),
            "ceiling: rings"
        );
    }

    #[test]
    fn the_status_cell_names_the_surface_and_what_it_runs() {
        assert_eq!(behavior_word(Some((B::Aurora, 0.7))), "aurora");
        assert_eq!(behavior_word(Some((B::None, 1.0))), "none");
        assert_eq!(behavior_word(None), "?");
        assert_eq!(
            status_text("desk", Some((B::Curls, 1.0)), false),
            "desk: curls"
        );
        assert_eq!(
            status_text("desk", Some((B::Curls, 1.0)), true),
            "desk: curls \u{b7} armed"
        );
        assert_eq!(status_text("wall 6", None, false), "wall 6: ?");
    }

    #[test]
    fn the_arm_text_names_the_kind_and_what_a_second_hold_does() {
        assert_eq!(
            arm_text("desk", KIND_TABLE, B::Streamlines),
            "desk: hold again for all tables -> streamlines"
        );
        assert_eq!(
            arm_text("lamp", KIND_OTHER, B::Curls),
            "lamp: hold again for all other surfaces -> curls"
        );
        // The next behavior is the class cycle's: the step after the
        // surface's own in its kind's catalogue.
        let next = B::None.next_for(KIND_CEILING);
        assert_eq!(
            arm_text("ceiling", KIND_CEILING, next),
            "ceiling: hold again for all ceilings -> rings"
        );
        // The same words as the result the second hold labels.
        assert_eq!(
            class_text(KIND_FRAME, B::Rings, B::Streamlines),
            "all frames: streamlines"
        );
        assert!(
            arm_text("frame 4", KIND_FRAME, B::Streamlines).ends_with("all frames -> streamlines")
        );
    }

    #[test]
    fn the_meter_shows_with_its_label_until_the_next() {
        let mut l = Label::default();
        // No label, no meter.
        l.meter(Some(0.1));
        assert_eq!(l.now(), None);
        l.show_for("Listening…".to_owned(), Vec3::ZERO, Vec3::Z, 7.0);
        assert_eq!(l.now().unwrap().level, None);
        l.meter(Some(0.1));
        l.step(0.1);
        assert_eq!(l.now().unwrap().level, Some(0.1));
        l.meter(Some(0.02));
        assert_eq!(l.now().unwrap().level, Some(0.02));
        // The window closes: "…" without a meter.
        l.show_for("…".to_owned(), Vec3::ZERO, Vec3::Z, 10.0);
        assert_eq!(l.now().unwrap().level, None);
        l.meter(Some(0.1));
        l.clear();
        assert_eq!(l.now(), None);
        l.show("next effect".to_owned(), Vec3::ZERO, Vec3::Z);
        assert_eq!(l.now().unwrap().level, None);
    }

    #[test]
    fn a_label_of_several_lines_grows_by_a_line_each() {
        assert_eq!(tex_h(1), LABEL_TEX[1]);
        assert_eq!(tex_h(0), LABEL_TEX[1]);
        assert_eq!(tex_h(2), 288);
        assert_eq!(tex_h(5), 576);
        assert_eq!(tex_h(MAX_LINES), LABEL_TEX_H_MAX);
        assert_eq!(tex_h(MAX_LINES + 3), LABEL_TEX_H_MAX);
        assert_eq!(lines("desk: amber"), ["desk: amber"]);
        assert_eq!(
            lines("Didn't catch that: \"x\"\nTry \"next effect\""),
            ["Didn't catch that: \"x\"", "Try \"next effect\""]
        );
        assert_eq!(lines("1\n2\n3\n4\n5\n6\n7\n8").len(), MAX_LINES);
        let head = Vec3::new(0.0, 1.6, 0.0);
        let one = billboard(Vec3::new(0.0, 1.2, -0.5), Vec3::Z, head, Quat::IDENTITY);
        assert_close!(one.with_lines(1).half_h, one.half_h);
        let five = one.with_lines(5);
        assert_close!(five.half_w, one.half_w);
        assert_close!(five.half_h, one.half_h * 3.0);
        assert_eq!(five.center, one.center);
    }

    #[test]
    fn the_param_text_names_the_surface_the_behavior_and_the_three_values() {
        assert_eq!(
            param_text("desk", B::Curls, (4, 2, 0.7)),
            "desk: curls · amber · mid · 0.7"
        );
        assert_eq!(
            param_text("wall 12", B::Aurora, (0, 1, 1.0)),
            "wall 12: aurora · kind · bass · 1.0"
        );
        assert_eq!(
            param_text("lamp", B::Pulse, (crate::surfaces::COLOR_KEY, 3, 0.0)),
            "lamp: pulse · key · high · 0.0"
        );
        assert_eq!(
            param_text("floor", B::Rings, (3, 0, 0.5)),
            "floor: rings · warm white · rms · 0.5"
        );
    }

    #[test]
    fn the_text_says_when_a_surface_cannot_emit() {
        let tapped = cycle_text("table 13", B::Embers, B::Sparks);
        assert_eq!(
            space_text(tapped.clone(), B::Sparks, true),
            "table 13: sparks (outside the space)"
        );
        assert_eq!(space_text(tapped, B::Sparks, false), "table 13: sparks");
        assert_eq!(
            space_text(class_text(KIND_TABLE, B::None, B::Embers), B::Embers, true),
            "all tables: embers (outside the space)"
        );
        // A result that spawns nothing is not "outside": it would not
        // emit anywhere.
        for after in [B::None, B::Spectrum, B::Rings, B::Streamlines] {
            let text = cycle_text("floor", B::Sparks, after);
            assert_eq!(space_text(text.clone(), after, true), text);
        }
    }

    #[test]
    fn the_label_holds_then_fades_over_its_last_part() {
        assert_close!(fade(0.0), 1.0);
        assert_close!(fade(LABEL_S - FADE_S), 1.0);
        assert_close!(fade(LABEL_S - FADE_S * 0.5), 0.5);
        assert_close!(fade(LABEL_S), 0.0);
        assert_close!(fade(LABEL_S + 1.0), 0.0);
        // Falls linearly, never rises.
        let mut last = 1.0;
        for i in 0..=150 {
            let a = fade(i as f32 * 0.01);
            assert!(a <= last + 1e-6, "{i}");
            last = a;
        }
    }

    #[test]
    fn a_label_shows_for_its_time_and_the_next_one_replaces_it() {
        let dt = 1.0 / 72.0;
        let mut l = Label::default();
        assert_eq!(l.now(), None);
        l.show("desk: sparks".to_owned(), Vec3::ONE, Vec3::Y);
        let f = l.now().unwrap();
        assert_eq!(
            (f.text, f.point, f.normal),
            ("desk: sparks", Vec3::ONE, Vec3::Y)
        );
        assert_close!(f.alpha, 1.0);
        let mut frames = 0;
        while l.now().is_some() {
            l.step(dt);
            frames += 1;
        }
        // Gone within a frame of its reading time, two words' (the frames'
        // sum rounds).
        assert!(
            (frames as f32 * dt - read_s("desk: sparks")).abs() <= dt * 1.01,
            "{frames}"
        );
        // Another action restarts it from full with its own text.
        l.show("desk: none".to_owned(), Vec3::ONE, Vec3::Y);
        for _ in 0..60 {
            l.step(dt);
        }
        l.show("desk: embers".to_owned(), Vec3::ZERO, Vec3::X);
        let f = l.now().unwrap();
        assert_eq!(f.text, "desk: embers");
        assert_close!(f.alpha, 1.0);
        l.clear();
        // A held life (the arm's): full until its last FADE_S, the same
        // fade, gone at its time.
        let life = crate::room_edit::ARM_S;
        assert!(life > LABEL_S);
        l.show_held("desk: hold again".to_owned(), Vec3::ONE, Vec3::Y, life);
        let mut frames = 0;
        while let Some(f) = l.now() {
            let age = frames as f32 * dt;
            if age <= life - FADE_S - dt {
                assert_close!(f.alpha, 1.0);
            }
            // The frames' sum rounds in f32.
            assert!((f.alpha - fade_for(age, life)).abs() < 1e-3, "{age}");
            l.step(dt);
            frames += 1;
        }
        assert!((frames as f32 * dt - life).abs() <= dt * 1.01, "{frames}");
        assert_close!(fade_for(life - FADE_S * 0.5, life), 0.5);
        assert_close!(fade_for(LABEL_S, LABEL_S), fade(LABEL_S));
        assert_eq!(l.now(), None);
    }

    fn assert_orthonormal_and_facing(b: &Billboard, head: Vec3) {
        assert!((b.right.length() - 1.0).abs() < 1e-5, "{b:?}");
        assert!((b.up.length() - 1.0).abs() < 1e-5, "{b:?}");
        assert!(b.right.dot(b.up).abs() < 1e-5, "{b:?}");
        // The quad's front (right x up) points straight at the head.
        let front = b.right.cross(b.up);
        let to_head = (head - b.center).normalize();
        assert!(front.dot(to_head) > 1.0 - 1e-5, "{b:?}");
    }

    #[test]
    fn the_billboard_sits_off_the_hit_and_faces_the_head() {
        let head = Vec3::new(0.0, 1.2, 0.0);
        // A wall 2 m ahead, hit at eye height, the head level.
        let b = billboard(Vec3::new(0.3, 1.2, -2.0), Vec3::Z, head, Quat::IDENTITY);
        assert!(b.center.abs_diff_eq(Vec3::new(0.3, 1.26, -1.92), 1e-5));
        assert_orthonormal_and_facing(&b, head);
        // Level head: the right is (nearly: the label sits a little above
        // and to the side of the view) level and toward +x, the up upward.
        assert!(b.right.y.abs() < 0.01 && b.right.x > 0.9, "{b:?}");
        assert!(b.up.y > 0.9, "{b:?}");
        // 1.94 m off: the narrowest label.
        assert_close!(b.half_w, 0.14);
        assert_close!(b.half_h, 0.14 * 192.0 / 1024.0);
        // A desk below and to the side, the head turned toward it and
        // rolled: still facing the head, the right following the roll.
        let rot = Quat::from_rotation_y(-0.6) * Quat::from_rotation_z(0.3);
        let desk = billboard(Vec3::new(1.0, 0.77, -0.8), Vec3::Y, head, rot);
        assert!(desk.center.abs_diff_eq(Vec3::new(1.0, 0.91, -0.8), 1e-5));
        assert_orthonormal_and_facing(&desk, head);
        assert!(desk.right.dot(rot * Vec3::X) > 0.9, "{desk:?}");
        // Looking along the head's right at it: the level right.
        let side = billboard(Vec3::new(2.0, 1.14, 0.0), Vec3::NEG_X, head, Quat::IDENTITY);
        assert_orthonormal_and_facing(&side, head);
        assert!(side.right.y.abs() < 1e-5, "{side:?}");
    }

    #[test]
    fn the_label_widens_with_distance_between_its_clamps() {
        // 1 m (arm's length and a little): the step 2b width.
        assert_close!(label_w_m(1.0), 0.28);
        // 3 m (a far wall from the chair), 8 m (the cast's range).
        assert_close!(label_w_m(3.0), 0.33);
        assert_close!(label_w_m(8.0), 0.88);
        // The clamp ends: up close and past 12.7 m.
        assert_close!(label_w_m(0.0), LABEL_W_MIN_M);
        assert_close!(label_w_m(LABEL_W_MIN_M / LABEL_W_PER_M), LABEL_W_MIN_M);
        assert_close!(label_w_m(LABEL_W_MAX_M / LABEL_W_PER_M), LABEL_W_MAX_M);
        assert_close!(label_w_m(40.0), LABEL_W_MAX_M);
        // Never narrower as the distance grows.
        let mut last = 0.0;
        for i in 0..=200 {
            let w = label_w_m(i as f32 * 0.1);
            assert!(w >= last, "{i}");
            last = w;
        }
        // The billboard takes it from its center's distance to the head.
        let head = Vec3::new(0.0, 1.2, 0.0);
        let b = billboard(Vec3::new(0.0, 1.14, -8.08), Vec3::Z, head, Quat::IDENTITY);
        assert!((b.center.distance(head) - 8.0).abs() < 1e-4, "{b:?}");
        assert!((b.half_w - 0.44).abs() < 1e-4, "{b:?}");
        assert!((b.half_h - 0.44 * 192.0 / 1024.0).abs() < 1e-5, "{b:?}");
    }

    #[test]
    fn the_scan_label_says_where_the_room_scan_is() {
        assert_eq!(scan_text(ScanState::Scanning), "Scanning the room\u{2026}");
        // Board #4204: one generic line, whatever the count.
        assert_eq!(scan_text(ScanState::Found(17)), "Loading your room");
        assert_eq!(scan_text(ScanState::Found(1)), "Loading your room");
        assert_eq!(
            scan_text(ScanState::None),
            "No room found. Rescan from the hand menu"
        );
        // Scanning holds however long; found shows 3 s then fades; none
        // 6 s.
        assert_close!(scan_fade(ScanState::Scanning, 600.0), 1.0);
        for (state, hold) in [
            (ScanState::Found(17), SCAN_FOUND_S),
            (ScanState::None, SCAN_NONE_S),
        ] {
            assert_close!(scan_fade(state, 0.0), 1.0);
            assert_close!(scan_fade(state, hold), 1.0);
            assert_close!(scan_fade(state, hold + 0.5 * FADE_S), 0.5);
            assert_close!(scan_fade(state, hold + FADE_S), 0.0);
        }
        let dt = 1.0 / 72.0;
        let run = |l: &mut ScanLabel, state, s: f32| {
            for _ in 0..(s / dt).round() as usize {
                l.step(state, dt);
            }
        };
        let mut l = ScanLabel::default();
        assert_eq!(l.now(), None);
        // Eight retries' worth of scanning: held at full.
        run(&mut l, Some(ScanState::Scanning), 24.0);
        assert_eq!(l.now(), Some(("Scanning the room\u{2026}", 1.0)));
        // Found: named, then gone after 3 s and its fade.
        run(&mut l, Some(ScanState::Found(17)), 2.9);
        assert_eq!(l.now(), Some((FOUND_LABEL, 1.0)));
        run(&mut l, Some(ScanState::Found(17)), 0.6);
        assert_eq!(l.now(), None);
        // A requery that returns the same count says nothing again;
        // another count is news.
        run(&mut l, Some(ScanState::Found(17)), 10.0);
        assert_eq!(l.now(), None);
        l.step(Some(ScanState::Found(18)), dt);
        assert_eq!(l.now().map(|n| n.0), Some(FOUND_LABEL));
        // None: 6 s, then gone.
        let mut l = ScanLabel::default();
        run(&mut l, Some(ScanState::None), 5.9);
        assert_eq!(
            l.now().map(|n| n.0),
            Some("No room found. Rescan from the hand menu")
        );
        run(&mut l, Some(ScanState::None), 0.6);
        assert_eq!(l.now(), None);
        // A rescan scans again: held; without a room, nothing.
        l.step(Some(ScanState::Scanning), dt);
        assert!(l.now().is_some());
        l.step(None, dt);
        assert_eq!(l.now(), None);
    }

    #[test]
    fn the_scan_label_says_when_spatial_data_is_not_allowed() {
        assert_eq!(
            scan_text(ScanState::NotAllowed),
            "Allow spatial data to see the room"
        );
        // Held while the app waits, like the scan.
        assert_close!(scan_fade(ScanState::NotAllowed, 600.0), 1.0);
        // Waiting for USE_SCENE: scanning or given up reads as not allowed;
        // anchors (a replay) and no room are left alone.
        for state in [ScanState::Scanning, ScanState::None] {
            assert_eq!(
                with_scene_permission(Some(state), true),
                Some(ScanState::NotAllowed)
            );
            assert_eq!(with_scene_permission(Some(state), false), Some(state));
        }
        assert_eq!(
            with_scene_permission(Some(ScanState::Found(3)), true),
            Some(ScanState::Found(3))
        );
        assert_eq!(with_scene_permission(None, true), None);
        // The grant: the requery scans again, then names the room.
        let dt = 1.0 / 72.0;
        let mut l = ScanLabel::default();
        for _ in 0..720 {
            l.step(with_scene_permission(Some(ScanState::None), true), dt);
        }
        assert_eq!(l.now(), Some(("Allow spatial data to see the room", 1.0)));
        l.step(with_scene_permission(Some(ScanState::Scanning), false), dt);
        assert_eq!(l.now(), Some(("Scanning the room\u{2026}", 1.0)));
        l.step(with_scene_permission(Some(ScanState::Found(17)), false), dt);
        assert_eq!(l.now(), Some((FOUND_LABEL, 1.0)));
    }

    /// `text`'s layout, checked: every line fits the texture's width at
    /// its size, the size within its line count's cap and the floor, the
    /// lines within [`MAX_LINES`].
    fn laid(text: &str) -> Layout {
        let l = layout(text);
        for line in &l.lines {
            let w = crate::text_width::width(line, l.px);
            assert!(w <= TEXT_W_PX + 1e-3, "{line:?} is {w} px at {}", l.px);
        }
        assert!(
            l.px <= font_cap(l.lines.len()) && l.px >= MIN_FONT_PX,
            "{l:?}"
        );
        assert!(l.lines.len() <= MAX_LINES, "{l:?}");
        l
    }

    #[test]
    fn a_label_holds_for_its_reading_time() {
        // Words are what a reader reads: "·", "->" and "…" are not.
        assert_eq!(words("desk: sparks"), 2);
        assert_eq!(words("desk: curls · amber · mid · 0.7"), 5);
        assert_eq!(words("desk: hold again for all tables -> curls"), 7);
        assert_eq!(words("\u{2026}"), 0);
        assert_eq!(words(""), 0);
        // 1.5 s and 0.3 s a word, up to 6.
        assert_close!(read_s(""), 1.5);
        assert_close!(read_s("next"), 1.8);
        assert_close!(read_s("desk: sparks"), 2.1);
        assert_close!(read_s("No room found. Rescan from the hand menu"), 3.9);
        assert_close!(read_s(&"word ".repeat(40)), READ_MAX_S);
        let dt = 1.0 / 72.0;
        let life = |l: &mut Label| {
            let mut frames = 0;
            while l.now().is_some() {
                l.step(dt);
                frames += 1;
            }
            frames as f32 * dt
        };
        let mut l = Label::default();
        // `show`: the reading time.
        let sentence = "No room found. Rescan from the hand menu";
        l.show(sentence.to_owned(), Vec3::ZERO, Vec3::Z);
        assert!((life(&mut l) - read_s(sentence)).abs() <= dt * 1.01);
        // `show_for`: the caller's time as a floor (a voice reply's 1.5 s
        // grows to its reading time; "Listening…" keeps its window).
        l.show_for(sentence.to_owned(), Vec3::ZERO, Vec3::Z, 1.5);
        assert!((life(&mut l) - read_s(sentence)).abs() <= dt * 1.01);
        l.show_for("Listening\u{2026}".to_owned(), Vec3::ZERO, Vec3::Z, 7.0);
        assert!((life(&mut l) - 7.0).abs() <= dt * 1.01);
        // `show_held`: exactly the caller's time, read or not.
        let arm = arm_text("desk", crate::surfaces::KIND_TABLE, B::Curls);
        l.show_held(arm.clone(), Vec3::ZERO, Vec3::Z, crate::room_edit::ARM_S);
        assert!((life(&mut l) - crate::room_edit::ARM_S).abs() <= dt * 1.01);
        // Which leaves the arm's seven words short of their reading time
        // in its window: 3.6 s against 2.5.
        assert!(read_s(&arm) > crate::room_edit::ARM_S);
    }

    #[test]
    fn every_fixed_label_holds_long_enough_to_read() {
        let holds = [
            (FOUND_LABEL.to_owned(), SCAN_FOUND_S),
            (scan_text(ScanState::None), SCAN_NONE_S),
            (crate::roam::NO_ROOM_LABEL.to_owned(), SCAN_NONE_S),
            (
                crate::immerse::OPEN_TEXT.to_owned(),
                crate::immerse::OPEN_HOLD_S,
            ),
        ];
        for (text, hold) in holds {
            assert!(hold >= read_s(&text), "{text:?} holds {hold} s");
        }
        for stop in [
            crate::release::Stop::SessionLost,
            crate::release::Stop::OutOfMemory,
            crate::release::Stop::Headset,
            crate::release::Stop::App,
        ] {
            let text = crate::release::fatal_text(stop);
            assert!(crate::release::FATAL_S >= read_s(&text), "{text:?}");
        }
    }

    #[test]
    fn a_long_label_wraps_instead_of_shrinking() {
        // Short: one line at the largest size.
        let short = laid("Sit back and relax");
        assert_eq!(short.lines, ["Sit back and relax"]);
        assert_close!(short.px, FONT_PX);
        // A line that fits at 64 px or more stays one line, a little
        // smaller, as before.
        let mid = laid("Allow music files in Settings.");
        assert_eq!(mid.lines.len(), 1);
        assert!(mid.px >= LINES_FONT_PX && mid.px < FONT_PX, "{mid:?}");
        // The ones that shrank to 46 and 48 px, and the one clipped at
        // both ends at 40.
        let none = laid("No room found. Rescan from the hand menu");
        assert_eq!(none.lines, ["No room found.", "Rescan from the hand menu"]);
        assert!(none.px > 73.0, "{none:?}");
        let roam = laid(crate::roam::NO_ROOM_LABEL);
        assert_eq!(
            roam.lines,
            ["No room here: roaming.", "Rescan from the hand menu"]
        );
        let arm = laid(&arm_text(
            "desk",
            crate::surfaces::KIND_TABLE,
            B::Streamlines,
        ));
        assert_eq!(arm.lines.len(), 2);
        assert_close!(arm.px, TWO_LINES_FONT_PX);
        let theme = "No theme called \"blue sky\": Classic, Drift, Tide, Aurora or Mirror";
        let miss = laid(theme);
        assert_eq!(miss.lines.len(), 3, "{miss:?}");
        assert_eq!(miss.lines[0], "No theme called \"blue sky\":");
        assert_eq!(miss.lines.join(" "), theme);
        assert_close!(miss.px, LINES_FONT_PX);
        // A colon ends a line when that costs no extra one.
        let colon = laid("Fosfora stopped: the session was lost");
        assert_eq!(colon.lines, ["Fosfora stopped:", "the session was lost"]);
        // The voice's own lines stay lines, and only a line that runs over
        // wraps: a miss and its hint.
        let hint = laid("Didn't catch that: \"x\"\nTry \"next effect\"");
        assert_eq!(
            hint.lines,
            ["Didn't catch that: \"x\"", "Try \"next effect\""]
        );
        let long = laid(
            "Didn't catch that: \"make the left wall a deep ocean blue\"\nTry \"wall 2 in blue\"",
        );
        assert_eq!(long.lines.len(), 3, "{long:?}");
        assert_eq!(long.lines[2], "Try \"wall 2 in blue\"");
        // Five help lines keep their size of more lines.
        let help = laid(
            "storage 1 in blue\nswitch to Embers\nevery wall rings\nedit the room\nwall 2 on the bass",
        );
        assert_eq!(help.lines.len(), 5);
        assert_close!(help.px, LINES_FONT_PX);
        // Too long for the label even at its smallest: its first lines,
        // the last cut.
        let flood = laid(&"Lorem ipsum dolor sit amet, consectetur. ".repeat(30));
        assert_eq!(flood.lines.len(), MAX_LINES);
        assert!(flood.lines[MAX_LINES - 1].ends_with('\u{2026}'));
        // Nothing to say is one empty line.
        assert_eq!(layout("").lines, [""]);
    }

    #[test]
    fn the_scan_label_floats_ahead_at_eye_height_facing_the_head() {
        let head = Vec3::new(0.3, 1.2, -0.4);
        // Looking down at the desk and turned: still level, 1.2 m ahead.
        let rot = Quat::from_rotation_y(0.7) * Quat::from_rotation_x(-0.8);
        let b = scan_billboard(head, rot);
        assert_close!(b.center.y, head.y);
        assert!((b.center.distance(head) - SCAN_DISTANCE_M).abs() < 1e-5);
        let ahead = rot * Vec3::NEG_Z;
        let level = Vec3::new(ahead.x, 0.0, ahead.z).normalize();
        assert!((b.center - head).normalize().abs_diff_eq(level, 1e-5));
        assert_orthonormal_and_facing(&b, head);
        // The narrowest label at 1.2 m.
        assert_close!(b.half_w, 0.5 * LABEL_W_MIN_M);
        // Straight down: ahead is the view's level fallback.
        let down = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
        let b = scan_billboard(head, down);
        assert!((b.center.distance(head) - SCAN_DISTANCE_M).abs() < 1e-4);
        assert!(b.center.y.is_finite() && (b.center.y - head.y).abs() < 1e-5);
    }

    #[test]
    fn the_texture_is_twice_the_step_2b_one_at_its_aspect() {
        assert_eq!(LABEL_TEX, [1024, 192]);
        assert_eq!(LABEL_TEX[0] * 96, LABEL_TEX[1] * 512);
    }
}
