//! The room editor's label (board #3326, step 2b): after each tap or hold,
//! a short line of text floats at the hit for [`LABEL_S`], naming the
//! result ("desk: sparks", "all tables: none", "ceiling: nothing to
//! change"), because the only other naming of it is the palm panel's
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
//! it holds for [`LABEL_S`] and fades out over the last [`FADE_S`] (the
//! fade scales the ground and the text alike).
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
//! held while the query has not returned anchors, "Room: 17 surfaces" for
//! [`SCAN_FOUND_S`] when it does, "No room found. Rescan from the hand
//! menu" for [`SCAN_NONE_S`] when the retries give up, each fading over
//! [`FADE_S`]; re-posed every frame [`SCAN_DISTANCE_M`] in front of the
//! head at eye height ([`scan_billboard`]). The editor's label, while one
//! shows, takes its place.

use glam::{Quat, Vec3};

use crate::surfaces::SurfaceBehavior;

/// How long a label shows (s), and the fade at its end (s).
pub const LABEL_S: f32 = 1.5;
pub const FADE_S: f32 = 0.4;
/// The label's texture, width x height (px): large enough that the text
/// is sharp up close, where the label is its narrowest.
pub const LABEL_TEX: [u32; 2] = [1024, 192];
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
    ((LABEL_S - age) / FADE_S).clamp(0.0, 1.0)
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
fn facing(center: Vec3, head: Vec3, head_rot: Quat) -> Billboard {
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
}

/// The label across frames: at most one, the last action's.
#[derive(Debug, Clone, Default)]
pub struct Label {
    shown: Option<(String, Vec3, Vec3, f32)>,
}

impl Label {
    /// Show `text` at the hit `point` on a face with `normal`, from full,
    /// in place of any label showing.
    pub fn show(&mut self, text: String, point: Vec3, normal: Vec3) {
        self.shown = Some((text, point, normal, 0.0));
    }

    /// Age the label by `dt`; it goes at [`LABEL_S`].
    pub fn step(&mut self, dt: f32) {
        if let Some((.., age)) = self.shown.as_mut() {
            *age += dt.max(0.0);
        }
        self.shown = self.shown.take().filter(|(.., age)| *age < LABEL_S);
    }

    /// Drop the label (the mode went off).
    pub fn clear(&mut self) {
        self.shown = None;
    }

    /// The label now, `None` when none shows.
    pub fn now(&self) -> Option<LabelFrame<'_>> {
        self.shown
            .as_ref()
            .map(|(text, point, normal, age)| LabelFrame {
                text,
                alpha: fade(*age),
                point: *point,
                normal: *normal,
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
}

/// The scan label's words for `state`.
pub fn scan_text(state: ScanState) -> String {
    match state {
        ScanState::Scanning => "Scanning the room\u{2026}".to_owned(),
        ScanState::Found(1) => "Room: 1 surface".to_owned(),
        ScanState::Found(n) => format!("Room: {n} surfaces"),
        ScanState::None => "No room found. Rescan from the hand menu".to_owned(),
    }
}

/// The scan label's alpha `age` seconds into `state`: held while
/// scanning; full for [`SCAN_FOUND_S`] or [`SCAN_NONE_S`], then linearly
/// to 0 over [`FADE_S`].
pub fn scan_fade(state: ScanState, age: f32) -> f32 {
    let hold = match state {
        ScanState::Scanning => return 1.0,
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
        match (state, self.shown.as_mut()) {
            (Some(s), Some((was, _, age))) if *was == s => *age += dt.max(0.0),
            (Some(s), _) => self.shown = Some((s, scan_text(s), 0.0)),
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
mod render {
    use egui::{FontId, Pos2, Rect, Vec2};
    use fosfora_app::ui::theme::palette::Palette;

    use super::{Billboard, GROUND_ALPHA, LABEL_TEX};
    use crate::gfx::{Gfx, PanelPose};

    /// The label's colors: the hand menu's theme (board #3523).
    const PALETTE: Palette = Palette::BLUE_ORANGE;

    /// The text's largest size and the margin it keeps from the ground's
    /// ends (px); a longer text shrinks to fit, down to [`MIN_FONT_PX`].
    const FONT_PX: f32 = 88.0;
    const MIN_FONT_PX: f32 = 40.0;
    const MARGIN_PX: f32 = 48.0;
    const CORNER_PX: u8 = 44;
    /// The ground's inset from the texture's edge (px).
    const INSET_PX: f32 = 4.0;

    /// The label's texture and its egui renderer (`Hud::new`'s pattern: a
    /// context and a renderer of its own), redrawn only while a label
    /// shows and only when its text or its fade step changed.
    pub struct LabelTexture {
        ctx: egui::Context,
        renderer: egui_wgpu::Renderer,
        /// Owns the image `Gfx`'s label bind group samples.
        _texture: wgpu::Texture,
        view: wgpu::TextureView,
        /// The text and the alpha (in 1/255) last drawn.
        drawn: Option<(String, u8)>,
    }

    impl LabelTexture {
        /// Build the texture and hand it to `gfx`; hidden until [`Self::show`].
        pub fn new(gfx: &mut Gfx) -> Self {
            let texture = gfx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("xr-label"),
                size: wgpu::Extent3d {
                    width: LABEL_TEX[0],
                    height: LABEL_TEX[1],
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
            gfx.set_label(&view);
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
                drawn: None,
            }
        }

        /// Show `text` at `alpha` on `pose` this frame, or hide the label
        /// with `None`.
        pub fn show(&mut self, gfx: &Gfx, label: Option<(&str, f32, Billboard)>) {
            let Some((text, alpha, pose)) = label else {
                self.drawn = None;
                gfx.set_label_pose(None);
                return;
            };
            let step = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
            if self
                .drawn
                .as_ref()
                .is_none_or(|(t, a)| t != text || *a != step)
            {
                self.draw(gfx, text, f32::from(step) / 255.0);
                self.drawn = Some((text.to_owned(), step));
            }
            gfx.set_label_pose(Some(PanelPose {
                center: pose.center.to_array(),
                right: (pose.right * pose.half_w).to_array(),
                up: (pose.up * pose.half_h).to_array(),
                v_max: 1.0,
            }));
        }

        /// Render `text` at `alpha` into the texture: the rounded ground
        /// filling it, the text centered on it, shrunk to fit its width.
        fn draw(&mut self, gfx: &Gfx, text: &str, alpha: f32) {
            let size = Vec2::new(LABEL_TEX[0] as f32, LABEL_TEX[1] as f32);
            let mut raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..egui::RawInput::default()
            };
            raw.viewports
                .entry(egui::ViewportId::ROOT)
                .or_default()
                .native_pixels_per_point = Some(1.0);
            let output = self.ctx.run(raw, |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::background());
                let ground = Rect::from_min_size(Pos2::ZERO, size).shrink(INSET_PX);
                painter.rect_filled(
                    ground,
                    CORNER_PX,
                    PALETTE.panel.gamma_multiply(GROUND_ALPHA * alpha),
                );
                let ink = PALETTE.text.gamma_multiply(alpha);
                let room = ground.width() - 2.0 * MARGIN_PX;
                let mut galley =
                    painter.layout_no_wrap(text.to_owned(), FontId::proportional(FONT_PX), ink);
                if galley.size().x > room {
                    let px = (FONT_PX * room / galley.size().x).max(MIN_FONT_PX);
                    galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(px), ink);
                }
                let at = ground.center() - galley.size() * 0.5;
                painter.galley(at, galley, ink);
            });
            for (id, delta) in &output.textures_delta.set {
                self.renderer
                    .update_texture(&gfx.device, &gfx.queue, *id, delta);
            }
            let jobs = self.ctx.tessellate(output.shapes, output.pixels_per_point);
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: LABEL_TEX,
                pixels_per_point: output.pixels_per_point,
            };
            let mut encoder = gfx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xr-label"),
                });
            let user_buffers =
                self.renderer
                    .update_buffers(&gfx.device, &gfx.queue, &mut encoder, &jobs, &screen);
            {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("xr-label"),
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
        // Gone within a frame of its time (the frames' sum rounds).
        assert!(
            (frames as f32 * dt - LABEL_S).abs() <= dt * 1.01,
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
        assert_eq!(scan_text(ScanState::Found(17)), "Room: 17 surfaces");
        assert_eq!(scan_text(ScanState::Found(1)), "Room: 1 surface");
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
        assert_eq!(l.now(), Some(("Room: 17 surfaces", 1.0)));
        run(&mut l, Some(ScanState::Found(17)), 0.6);
        assert_eq!(l.now(), None);
        // A requery that returns the same count says nothing again;
        // another count is news.
        run(&mut l, Some(ScanState::Found(17)), 10.0);
        assert_eq!(l.now(), None);
        l.step(Some(ScanState::Found(18)), dt);
        assert_eq!(l.now().map(|n| n.0), Some("Room: 18 surfaces"));
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
