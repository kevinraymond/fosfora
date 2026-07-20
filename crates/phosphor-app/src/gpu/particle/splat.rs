//! GPU packing + per-frame driver for the Splat effect (#1800).
//!
//! [`SplatStatic`] is the 32-byte per-splat attribute record the sim reads at
//! `@group(2) @binding(1)` (binding 0 is the lib's trail-buffer declaration).
//! Positions stay f32 (f16 quantization jitters at close zoom); scales and
//! rotation ride f16 pairs; color+opacity are 8-bit unorm. Zero-padding is the
//! count mechanism: slots past the scene unpack `opacity == 0` and the sim
//! parks them dead, so no separate splat-count uniform exists.
//!
//! [`SplatDriver`] owns the CPU-side camera/envelope state (yaw accumulator,
//! centroid EMA, drop-explode envelope) and writes the `cam_*`/`splat_*`
//! fields appended to `ParticleUniforms` in the #1800 ABI bump.

use bytemuck::{Pod, Zeroable};

use super::splat_source::SplatCloud;
use super::types::ParticleUniforms;
use crate::gpu::half::f32_to_f16;

/// Packed per-splat static attributes: 32 bytes, uploaded once per scene.
/// WGSL mirror (declared in `splat_sim.wgsl`):
/// `struct SplatStatic { pos: vec3f, color: u32, scale_xy: u32, scale_z_e: u32, rot_xy: u32, rot_zw: u32 }`
/// (vec3f align 16 / size 12 puts `color` at offset 12; struct stride 32 —
/// byte-identical to this layout.)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Pod, Zeroable)]
pub struct SplatStatic {
    /// World position, scene-normalized (median-centered, p95 radius ≈ 1).
    pub pos: [f32; 3],
    /// `pack4x8unorm(r, g, b, opacity)` — r in the low byte.
    pub color: u32,
    /// `pack2x16float(scale_x, scale_y)` — linear world-unit scales.
    pub scale_xy: u32,
    /// `pack2x16float(scale_z, 0.0)` — `.y` spare.
    pub scale_z_e: u32,
    /// `pack2x16float(quat.x, quat.y)` — unit rotation, glam order.
    pub rot_xy: u32,
    /// `pack2x16float(quat.z, quat.w)`.
    pub rot_zw: u32,
}

/// Matches WGSL `pack2x16float`: `a` in the low half.
fn pack2x16float(a: f32, b: f32) -> u32 {
    (f32_to_f16(a) as u32) | ((f32_to_f16(b) as u32) << 16)
}

/// Matches WGSL `pack4x8unorm`: `x` in the low byte, values clamped to 0..1.
fn pack4x8unorm(x: f32, y: f32, z: f32, w: f32) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    q(x) | (q(y) << 8) | (q(z) << 16) | (q(w) << 24)
}

/// Pack a decoded scene into the GPU layout. Length equals `cloud.count`;
/// the upload path zero-fills the remaining `max_particles` slots (dead).
pub fn pack_cloud(cloud: &SplatCloud) -> Vec<SplatStatic> {
    (0..cloud.count)
        .map(|i| {
            let c = cloud.colors[i];
            let s = cloud.scales[i];
            let r = cloud.rotations[i];
            SplatStatic {
                pos: cloud.positions[i],
                color: pack4x8unorm(c[0], c[1], c[2], cloud.opacities[i]),
                scale_xy: pack2x16float(s[0], s[1]),
                scale_z_e: pack2x16float(s[2], 0.0),
                rot_xy: pack2x16float(r[0], r[1]),
                rot_zw: pack2x16float(r[2], r[3]),
            }
        })
        .collect()
}

/// CPU-side per-frame state for the splat orbit camera and audio envelopes.
/// Lives on `ParticleSystem` (gated on `def.splat`), updated in `dispatch`
/// just before the uniform upload.
///
/// UI params arrive as .pfx slots 8–11 (`ParticleSystem::splat_ui_params`,
/// forwarded from the param store like `effect_params`):
/// `[orbit_speed, cam_distance, cam_pitch, focal_bias]`.
pub struct SplatDriver {
    yaw: f32,
    centroid_ema: f32,
    explode_env: f32,
}

impl SplatDriver {
    pub fn new() -> Self {
        Self {
            yaw: 0.0,
            centroid_ema: 0.5,
            explode_env: 0.0,
        }
    }

    /// Advance camera + envelopes and write the `cam_*`/`splat_*` uniform
    /// fields. Reads audio (`rms`, `buildup`, `drop`, `centroid`) and
    /// `delta_time` from the uniforms already populated this frame.
    pub fn update(&mut self, u: &mut ParticleUniforms, ui_params: [f32; 4]) {
        let dt = u.delta_time;
        let [orbit_speed, cam_distance, cam_pitch, focal_bias] = ui_params;

        // Audio-scaled orbit: still audible motion at silence, gentle push
        // with level. Yaw stays bounded (it feeds sin/cos in the sim).
        self.yaw += dt * orbit_speed * 0.6 * (0.4 + 0.6 * u.rms);
        self.yaw %= std::f32::consts::TAU;

        // Buildup leans the camera in — released by the drop with the riser.
        let dist = cam_distance.clamp(0.5, 6.0) * (1.0 - 0.15 * u.buildup);

        // Focal plane follows spectral centroid (EMA — focus pulls, never
        // snaps): bright timbre → focus forward (nearer), dark → far field.
        let k = 1.0 - (-dt / 0.35).exp();
        self.centroid_ema += (u.centroid - self.centroid_ema) * k;

        // Drop envelope: latch the one-frame trigger, decay τ = 0.45 s.
        self.explode_env = (self.explode_env * (-dt / 0.45).exp()).max(u.drop);

        u.cam_yaw = self.yaw;
        u.cam_pitch = cam_pitch.clamp(-1.35, 1.35);
        u.cam_distance = dist;
        u.cam_focal = 1.5; // cot(fov/2), volumetric-default field of view
        // View depth spans [dist−1, dist+1] for the unit-radius scene.
        let focal = dist + (1.0 - 2.0 * self.centroid_ema) + focal_bias;
        u.splat_focal_depth = focal.clamp(dist - 1.2, dist + 1.2);
        u.splat_explode = self.explode_env;
    }
}

/// Deterministic procedural test scene: `count` gaussians jittered along a
/// (2,3) torus knot, hue by arc-length — the render probe and perf tests use
/// it so CI never depends on a downloaded asset. Already normalized
/// (radius ≈ 1), matching what `load_splat_file` produces.
#[cfg(test)]
pub fn generate_test_scene(count: usize) -> SplatCloud {
    use super::splat_source::SplatTransform;
    let mut rng: u64 = 0x70CA_11ED_5EED_0001;
    let mut next = move || {
        rng = rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32 // 0..1
    };

    let mut cloud = SplatCloud {
        count,
        positions: Vec::with_capacity(count),
        scales: Vec::with_capacity(count),
        rotations: Vec::with_capacity(count),
        colors: Vec::with_capacity(count),
        opacities: Vec::with_capacity(count),
        source_path: "procedural:torus-knot".to_string(),
        total_in_file: count as u32,
        transform: SplatTransform::default(),
    };

    use std::f32::consts::TAU;
    for i in 0..count {
        let t = i as f32 / count as f32;
        let (r_major, r_minor) = (0.68, 0.3);
        let ring = r_major + r_minor * (3.0 * TAU * t).cos();
        let jitter = 0.035;
        cloud.positions.push([
            ring * (2.0 * TAU * t).cos() + (next() - 0.5) * jitter,
            r_minor * (3.0 * TAU * t).sin() + (next() - 0.5) * jitter,
            ring * (2.0 * TAU * t).sin() + (next() - 0.5) * jitter,
        ]);
        let s = 0.006 + next() * 0.010;
        cloud.scales.push([s, s * (0.5 + next()), s]);
        cloud.rotations.push([0.0, 0.0, 0.0, 1.0]);
        cloud.colors.push([
            0.5 + 0.5 * (TAU * t).cos(),
            0.5 + 0.5 * (TAU * (t + 0.33)).cos(),
            0.5 + 0.5 * (TAU * (t + 0.67)).cos(),
        ]);
        cloud.opacities.push(0.55 + 0.4 * next());
    }
    cloud
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    /// Test-local f16 → f32 (production only packs).
    fn f16_to_f32(h: u16) -> f32 {
        let sign = ((h >> 15) & 1) as u32;
        let exp = ((h >> 10) & 0x1F) as u32;
        let frac = (h & 0x3FF) as u32;
        let bits = if exp == 0 {
            if frac == 0 {
                sign << 31
            } else {
                // Subnormal: renormalize.
                let mut e = 127 - 15 + 1;
                let mut f = frac;
                while f & 0x400 == 0 {
                    f <<= 1;
                    e -= 1;
                }
                (sign << 31) | ((e as u32) << 23) | ((f & 0x3FF) << 13)
            }
        } else if exp == 0x1F {
            (sign << 31) | 0x7F80_0000 | (frac << 13)
        } else {
            (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13)
        };
        f32::from_bits(bits)
    }

    fn unpack2x16(v: u32) -> (f32, f32) {
        (f16_to_f32(v as u16), f16_to_f32((v >> 16) as u16))
    }

    #[test]
    fn splat_static_layout() {
        assert_eq!(std::mem::size_of::<SplatStatic>(), 32);
        assert_eq!(offset_of!(SplatStatic, pos), 0);
        assert_eq!(offset_of!(SplatStatic, color), 12);
        assert_eq!(offset_of!(SplatStatic, scale_xy), 16);
        assert_eq!(offset_of!(SplatStatic, scale_z_e), 20);
        assert_eq!(offset_of!(SplatStatic, rot_xy), 24);
        assert_eq!(offset_of!(SplatStatic, rot_zw), 28);
    }

    #[test]
    fn pack2x16float_roundtrip() {
        for v in [
            0.0f32,
            0.01,
            0.5,
            1.0,
            -std::f32::consts::FRAC_1_SQRT_2,
            3.5,
        ] {
            let (a, b) = unpack2x16(pack2x16float(v, -v));
            let tol = (v.abs() * 1e-3).max(1e-4);
            assert!((a - v).abs() < tol, "{v}: got {a}");
            assert!((b + v).abs() < tol, "{v}: got {b}");
        }
    }

    #[test]
    fn pack4x8unorm_matches_wgsl_layout() {
        // x in the low byte, w in the high byte; clamped.
        assert_eq!(pack4x8unorm(1.0, 0.0, 0.0, 0.0), 0x0000_00FF);
        assert_eq!(pack4x8unorm(0.0, 0.0, 0.0, 1.0), 0xFF00_0000);
        assert_eq!(pack4x8unorm(2.0, -1.0, 0.0, 0.0), 0x0000_00FF);
        let mid = pack4x8unorm(0.5, 0.5, 0.5, 0.5);
        assert_eq!(mid & 0xFF, 128);
    }

    #[test]
    fn pack_cloud_roundtrip() {
        let cloud = generate_test_scene(64);
        let packed = pack_cloud(&cloud);
        assert_eq!(packed.len(), 64);
        for (i, p) in packed.iter().enumerate() {
            assert_eq!(p.pos, cloud.positions[i]);
            let (sx, sy) = unpack2x16(p.scale_xy);
            assert!((sx - cloud.scales[i][0]).abs() < 1e-4);
            assert!((sy - cloud.scales[i][1]).abs() < 1e-4);
            let a = ((p.color >> 24) & 0xFF) as f32 / 255.0;
            assert!((a - cloud.opacities[i]).abs() < 1.0 / 255.0 + 1e-6);
        }
    }

    #[test]
    fn zeroed_static_is_dead_slot() {
        // The zero-fill count mechanism: an all-zero record must unpack to
        // opacity 0 so the sim parks the slot.
        let z = SplatStatic::zeroed();
        assert_eq!((z.color >> 24) & 0xFF, 0);
    }

    fn uniforms_with(dt: f32) -> ParticleUniforms {
        let mut u = ParticleUniforms::zeroed();
        u.delta_time = dt;
        u
    }

    #[test]
    fn driver_explode_latches_and_decays() {
        let mut d = SplatDriver::new();
        let mut u = uniforms_with(1.0 / 60.0);
        u.drop = 1.0;
        d.update(&mut u, [0.0, 1.6, 0.0, 0.0]);
        assert!((u.splat_explode - 1.0).abs() < 1e-6);
        u.drop = 0.0;
        d.update(&mut u, [0.0, 1.6, 0.0, 0.0]);
        let after_one = u.splat_explode;
        assert!(after_one < 1.0 && after_one > 0.9); // τ=0.45s ≫ one frame
        for _ in 0..120 {
            d.update(&mut u, [0.0, 1.6, 0.0, 0.0]);
        }
        assert!(u.splat_explode < 0.02, "envelope must decay out");
    }

    #[test]
    fn driver_yaw_advances_only_with_orbit() {
        let mut d = SplatDriver::new();
        let mut u = uniforms_with(1.0 / 60.0);
        d.update(&mut u, [0.0, 1.6, 0.0, 0.0]);
        assert_eq!(u.cam_yaw, 0.0);
        for _ in 0..60 {
            d.update(&mut u, [0.5, 1.6, 0.0, 0.0]);
        }
        assert!(u.cam_yaw > 0.05, "yaw should accumulate: {}", u.cam_yaw);
    }

    #[test]
    fn driver_centroid_ema_converges_and_focal_clamps() {
        let mut d = SplatDriver::new();
        let mut u = uniforms_with(1.0 / 60.0);
        u.centroid = 1.0;
        for _ in 0..300 {
            d.update(&mut u, [0.0, 1.6, 0.0, 0.0]);
        }
        // Bright timbre pulls focus forward: focal below cam_distance,
        // clamped to the near edge of the scene depth range.
        assert!(u.splat_focal_depth <= u.cam_distance - 0.99);
        assert!(u.splat_focal_depth >= u.cam_distance - 1.21);
    }

    #[test]
    fn driver_distance_clamps_and_buildup_pulls_in() {
        let mut d = SplatDriver::new();
        let mut u = uniforms_with(1.0 / 60.0);
        d.update(&mut u, [0.0, 100.0, 0.0, 0.0]);
        assert!((u.cam_distance - 6.0).abs() < 1e-5);
        u.buildup = 1.0;
        d.update(&mut u, [0.0, 2.0, 0.0, 0.0]);
        assert!((u.cam_distance - 2.0 * 0.85).abs() < 1e-5);
    }
}
