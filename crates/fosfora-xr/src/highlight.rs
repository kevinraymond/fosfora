//! The room editor's highlight (board #3326): the face the pointing ray hit,
//! tinted. Plain numbers in, so it builds and tests on the desktop; `gfx.rs`
//! draws it with [`HIGHLIGHT_WGSL`] from the rows [`uniform`] packs, through
//! the same lit-quad pipeline as the floor ripple and the wall spectrum,
//! right after them: behind a hand or a chair in front of the surface,
//! under the embers.
//!
//! **The look.** The ripple's family: premultiplied warm white, light on the
//! real surface rather than paint over it. A faint fill ([`FILL_ALPHA`])
//! under a border [`BORDER_M`] wide ([`BORDER_ALPHA`]): the wearer may have
//! only one eye's view, and a fill alone reads as a haze at no particular
//! depth where an outline reads as the surface's edge. A pulse raises both
//! by [`PULSE_ALPHA`] and decays over 0.3 s (`room_edit.rs`: once for a
//! cycle, twice for a class assignment). The quad is the hit face
//! (`surfaces::Face::across`) lifted [`LIFT_M`] toward the room.

use crate::surfaces::Face;

/// Height off the face the quad sits at (m), plus the pipeline's depth
/// bias: the surface's occluder writes depth at the face.
pub const LIFT_M: f32 = 0.01;
/// The fill's and the border's alpha.
pub const FILL_ALPHA: f32 = 0.10;
pub const BORDER_ALPHA: f32 = 0.30;
/// The border's width (m), inward from the face's edge.
pub const BORDER_M: f32 = 0.025;
/// What a full pulse adds to both alphas.
pub const PULSE_ALPHA: f32 = 0.25;
/// Warm white, the ripple's.
pub const COLOR: [f32; 3] = crate::ripple::COLOR;
/// Rows of [`uniform`], `struct Highlight` in [`HIGHLIGHT_WGSL`].
pub const UNIFORM_ROWS: usize = 7;

/// The uniform rows for `face` at pulse `pulse` (0..1): the four corners
/// ([`Face::corners`] at [`LIFT_M`]), the shape (the two axes' half
/// extents and the border width), the alphas (fill, border, the pulse and
/// what a full one adds) and the color.
pub fn uniform(face: &Face, pulse: f32) -> [[f32; 4]; UNIFORM_ROWS] {
    let mut rows = [[0.0f32; 4]; UNIFORM_ROWS];
    for (row, c) in rows.iter_mut().zip(face.corners(LIFT_M)) {
        *row = [c.x, c.y, c.z, 1.0];
    }
    rows[4] = [face.half[0], face.half[1], BORDER_M, 0.0];
    rows[5] = [FILL_ALPHA, BORDER_ALPHA, pulse.clamp(0.0, 1.0), PULSE_ALPHA];
    rows[6] = [COLOR[0], COLOR[1], COLOR[2], 0.0];
    rows
}

/// The alpha the fragment shader writes at the face's local point
/// `(u, v)` (meters along its two axes from its center), from the rows,
/// without the edge's antialiasing: the border within [`BORDER_M`] of an
/// edge, the fill inside, both raised by the pulse.
pub fn alpha_at(rows: &[[f32; 4]; UNIFORM_ROWS], u: f32, v: f32) -> f32 {
    let [hu, hv, border, _] = rows[4];
    let [fill, edge_alpha, pulse, pulse_alpha] = rows[5];
    let edge = (hu - u.abs()).min(hv - v.abs());
    let base = if edge < border { edge_alpha } else { fill };
    (base + pulse_alpha * pulse).clamp(0.0, 1.0)
}

/// The highlight's shader: group 0 is the eye pass's shared camera
/// (`view_proj`), group 1 the rows of [`uniform`]. The vertex stage places
/// the quad from its corners and hands the fragment its point on the face
/// in meters; the fragment writes the border near the edges, the fill
/// inside, premultiplied.
pub const HIGHLIGHT_WGSL: &str = r"
struct Eye { view_proj: mat4x4<f32> }
struct Highlight {
    corners: array<vec4<f32>, 4>,
    // x, y the two axes' half extents (m), z the border width (m)
    shape: vec4<f32>,
    // x fill alpha, y border alpha, z pulse 0..1, w a full pulse's alpha
    alpha: vec4<f32>,
    color: vec4<f32>,
}
@group(0) @binding(0) var<uniform> eye: Eye;
@group(1) @binding(0) var<uniform> highlight: Highlight;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    var order = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    // The corners go -a-b, +a-b, +a+b, -a+b.
    var uv = array<vec2<f32>, 4>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let k = order[i];
    var out: VsOut;
    out.pos = eye.view_proj * vec4<f32>(highlight.corners[k].xyz, 1.0);
    out.local = uv[k] * highlight.shape.xy;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Distance to the nearest edge (m), and a pixel's worth of it for a
    // soft line.
    let edge = min(
        highlight.shape.x - abs(in.local.x),
        highlight.shape.y - abs(in.local.y),
    );
    let aa = max(fwidth(edge), 1e-4);
    let w = highlight.shape.z;
    let border = 1.0 - smoothstep(w - aa, w + aa, edge);
    let base = mix(highlight.alpha.x, highlight.alpha.y, border);
    let a = clamp(base + highlight.alpha.w * highlight.alpha.z, 0.0, 1.0);
    return vec4<f32>(highlight.color.rgb * a, a);
}
";

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    /// A desk top: 1.2 x 0.6 m at 0.75 m, its axes along x and -z.
    fn desk() -> Face {
        Face {
            normal: Vec3::Y,
            center: Vec3::new(0.5, 0.75, -0.9),
            axes: [Vec3::X, Vec3::NEG_Z],
            half: [0.6, 0.3],
        }
    }

    #[test]
    fn the_rows_carry_the_lifted_face_and_the_look() {
        let rows = uniform(&desk(), 0.0);
        assert_close!(rows[0], [-0.1, 0.76, -0.6, 1.0]);
        assert_close!(rows[1], [1.1, 0.76, -0.6, 1.0]);
        assert_close!(rows[2], [1.1, 0.76, -1.2, 1.0]);
        assert_close!(rows[3], [-0.1, 0.76, -1.2, 1.0]);
        assert_close!(rows[4], [0.6, 0.3, 0.025, 0.0]);
        assert_close!(rows[5], [0.10, 0.30, 0.0, 0.25]);
        assert_close!(rows[6], [1.0, 0.86, 0.68, 0.0]);
        // The pulse is clamped.
        assert_close!(uniform(&desk(), 3.0)[5][2], 1.0);
    }

    #[test]
    fn the_border_is_brighter_than_the_fill_and_the_pulse_raises_both() {
        let rows = uniform(&desk(), 0.0);
        assert_close!(alpha_at(&rows, 0.0, 0.0), FILL_ALPHA);
        // 1 cm inside the long edge and inside a corner: the border.
        assert_close!(alpha_at(&rows, 0.0, 0.29), BORDER_ALPHA);
        assert_close!(alpha_at(&rows, -0.59, -0.29), BORDER_ALPHA);
        // Just past the border's width: the fill.
        assert_close!(alpha_at(&rows, 0.57, 0.0), FILL_ALPHA);
        let pulsed = uniform(&desk(), 1.0);
        assert_close!(alpha_at(&pulsed, 0.0, 0.0), FILL_ALPHA + PULSE_ALPHA);
        assert_close!(alpha_at(&pulsed, 0.0, 0.29), BORDER_ALPHA + PULSE_ALPHA);
    }

    #[test]
    fn the_shader_validates_and_matches_the_rows() {
        let module = naga::front::wgsl::parse_str(HIGHLIGHT_WGSL).expect("highlight WGSL parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("highlight WGSL validates");
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("Highlight"))
            .expect("struct Highlight");
        let naga::TypeInner::Struct { span, .. } = ty.inner else {
            panic!("Highlight is not a struct");
        };
        assert_eq!(span as usize, UNIFORM_ROWS * 16);
    }
}
