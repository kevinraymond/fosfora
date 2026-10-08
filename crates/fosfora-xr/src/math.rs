//! Per-eye camera math ("Per-eye camera" in `docs/xr/XR_DESIGN.md`). Plain
//! numbers in, so it builds and tests on the desktop as well.

use glam::{Mat4, Quat, Vec3, Vec4};

/// Asymmetric field of view in radians, as OpenXR reports it: left and down
/// are negative.
#[derive(Debug, Clone, Copy)]
pub struct Fov {
    pub left: f32,
    pub right: f32,
    pub up: f32,
    pub down: f32,
}

/// View-projection for one eye. `orientation` is an `(x, y, z, w)`
/// quaternion and `position` the eye's location, both in the reference space
/// the views were located in. The view matrix is the inverse of that pose;
/// the projection maps the fov to wgpu's clip space (-Z forward, depth 0..1).
pub fn view_projection(
    orientation: [f32; 4],
    position: [f32; 3],
    fov: Fov,
    near: f32,
    far: f32,
) -> Mat4 {
    let (view, proj) = view_and_projection(orientation, position, fov, near, far);
    proj * view
}

/// The two halves of [`view_projection`], for shaders that billboard in view
/// space (S5) and need them apart.
pub fn view_and_projection(
    orientation: [f32; 4],
    position: [f32; 3],
    fov: Fov,
    near: f32,
    far: f32,
) -> (Mat4, Mat4) {
    let pose = Mat4::from_rotation_translation(
        Quat::from_xyzw(
            orientation[0],
            orientation[1],
            orientation[2],
            orientation[3],
        ),
        Vec3::from_array(position),
    );
    (pose.inverse(), projection(fov, near, far))
}

pub fn projection(fov: Fov, near: f32, far: f32) -> Mat4 {
    let l = fov.left.tan();
    let r = fov.right.tan();
    let u = fov.up.tan();
    let d = fov.down.tan();
    let w = r - l;
    let h = u - d;
    Mat4::from_cols(
        Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
        Vec4::new((r + l) / w, (u + d) / h, far / (near - far), -1.0),
        Vec4::new(0.0, 0.0, (far * near) / (near - far), 0.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOV: Fov = Fov {
        left: -0.8,
        right: 0.7,
        up: 0.75,
        down: -0.8,
    };

    fn ndc(m: Mat4, p: Vec3) -> Vec3 {
        let c = m * p.extend(1.0);
        c.truncate() / c.w
    }

    #[test]
    fn near_and_far_map_to_zero_and_one() {
        let m = projection(FOV, 0.1, 100.0);
        assert!(ndc(m, Vec3::new(0.0, 0.0, -0.1)).z.abs() < 1e-5);
        assert!((ndc(m, Vec3::new(0.0, 0.0, -100.0)).z - 1.0).abs() < 1e-5);
    }

    #[test]
    fn fov_edges_map_to_clip_edges() {
        let m = projection(FOV, 0.1, 100.0);
        let z = -2.0f32;
        let left = ndc(m, Vec3::new(z.abs() * FOV.left.tan(), 0.0, z));
        let right = ndc(m, Vec3::new(z.abs() * FOV.right.tan(), 0.0, z));
        let up = ndc(m, Vec3::new(0.0, z.abs() * FOV.up.tan(), z));
        let down = ndc(m, Vec3::new(0.0, z.abs() * FOV.down.tan(), z));
        assert!((left.x + 1.0).abs() < 1e-4, "{left}");
        assert!((right.x - 1.0).abs() < 1e-4, "{right}");
        assert!((up.y - 1.0).abs() < 1e-4, "{up}");
        assert!((down.y + 1.0).abs() < 1e-4, "{down}");
    }

    #[test]
    fn view_moves_the_world_opposite_to_the_eye() {
        // Eye 1 m to the right, looking down -Z: a point on the axis ahead
        // lands left of center.
        let vp = view_projection([0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0], FOV, 0.1, 100.0);
        let p = ndc(vp, Vec3::new(0.0, 0.0, -2.0));
        assert!(p.x < 0.0, "{p}");
        assert!(p.z > 0.0 && p.z < 1.0, "{p}");
    }
}

/// A located point moved ahead along its velocity by `lead_s` seconds
/// (board #3753: the hand joints led by the runtime's velocities so the
/// skinned mesh trails less on fast motion). Nothing moves with no lead.
pub fn led(p: [f32; 3], v: [f32; 3], lead_s: f32) -> [f32; 3] {
    [
        p[0] + v[0] * lead_s,
        p[1] + v[1] * lead_s,
        p[2] + v[2] * lead_s,
    ]
}

#[cfg(test)]
mod lead_tests {
    use super::led;

    #[test]
    fn a_point_is_led_along_its_velocity_and_not_at_all_with_no_lead() {
        let same = led([1.0, 2.0, 3.0], [0.5, -1.0, 2.0], 0.0);
        assert!(
            same.iter()
                .zip([1.0, 2.0, 3.0])
                .all(|(a, b)| (a - b).abs() < 1e-9)
        );
        let p = led([1.0, 2.0, 3.0], [0.5, -1.0, 2.0], 0.02);
        assert!(
            (p[0] - 1.01).abs() < 1e-6 && (p[1] - 1.98).abs() < 1e-6 && (p[2] - 3.04).abs() < 1e-6
        );
    }
}
