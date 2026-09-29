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
//! The texture is [`LABEL_TEX`], shown [`LABEL_W_M`] wide at its aspect;
//! it holds for [`LABEL_S`] and fades out over the last [`FADE_S`] (the
//! fade scales the ground and the text alike).
//!
//! **The pose.** A billboard: centered at the hit moved [`OUT_M`] out
//! along the face normal and [`UP_M`] up, facing the head, its right the
//! head's right projected onto the plane across the line of sight to it
//! (so it rolls with the head and never mirrors), its up from those two.

use glam::{Quat, Vec3};

use crate::surfaces::SurfaceBehavior;

/// How long a label shows (s), and the fade at its end (s).
pub const LABEL_S: f32 = 1.5;
pub const FADE_S: f32 = 0.4;
/// The label's texture, width x height (px).
pub const LABEL_TEX: [u32; 2] = [512, 96];
/// The label's width in the room (m); its height by the texture's aspect.
pub const LABEL_W_M: f32 = 0.28;
/// The label's center off the hit: out along the face normal and up (m).
pub const OUT_M: f32 = 0.08;
pub const UP_M: f32 = 0.06;
/// The ground's alpha at full label.
pub const GROUND_ALPHA: f32 = 0.85;

/// What an editor action came to: `after`, or "nothing to change" when it
/// is what the surface already ran (a kind whose catalogue is only `none`,
/// a ceiling, on `none`).
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
/// seen from `head` turned by `head_rot`.
pub fn billboard(point: Vec3, normal: Vec3, head: Vec3, head_rot: Quat) -> Billboard {
    let center = point + normal.normalize_or_zero() * OUT_M + Vec3::Y * UP_M;
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
    let half_w = LABEL_W_M * 0.5;
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

#[cfg(target_os = "android")]
pub use render::LabelTexture;

#[cfg(target_os = "android")]
mod render {
    use egui::{Color32, FontId, Pos2, Rect, Vec2};

    use super::{Billboard, GROUND_ALPHA, LABEL_TEX};
    use crate::gfx::{Gfx, PanelPose};

    /// The text's largest size and the margin it keeps from the ground's
    /// ends (px); a longer text shrinks to fit, down to [`MIN_FONT_PX`].
    const FONT_PX: f32 = 44.0;
    const MIN_FONT_PX: f32 = 20.0;
    const MARGIN_PX: f32 = 24.0;
    const CORNER_PX: u8 = 22;

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
                let ground = Rect::from_min_size(Pos2::ZERO, size).shrink(2.0);
                painter.rect_filled(
                    ground,
                    CORNER_PX,
                    Color32::from_rgba_unmultiplied(18, 18, 22, 255)
                        .gamma_multiply(GROUND_ALPHA * alpha),
                );
                let ink = Color32::WHITE.gamma_multiply(alpha);
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
        // A kind with nothing to cycle.
        let after = B::None.next_for(KIND_CEILING);
        assert_eq!(
            cycle_text("ceiling", B::None, after),
            "ceiling: nothing to change"
        );
        assert_eq!(
            class_text(KIND_FRAME, B::None, B::None.next_for(KIND_FRAME)),
            "all frames: nothing to change"
        );
        // A ceiling the knob put on embers does change.
        assert_eq!(
            cycle_text("ceiling", B::Embers, B::Embers.next_for(KIND_CEILING)),
            "ceiling: none"
        );
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
        assert_close!(b.half_w, 0.14);
        assert_close!(b.half_h, 0.14 * 96.0 / 512.0);
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
}
