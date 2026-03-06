use wgpu::{Device, Queue};

/// Resolution of the 3D curl noise texture.
const FLOW_RES: u32 = 64;

/// Pre-baked 3D curl noise texture for particle flow fields.
/// Stores curl(noise(x,y,z)) as Rgba16Float: xyz = velocity, w = 0.
pub struct FlowFieldTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

impl FlowFieldTexture {
    /// Bake a 64x64x64 curl noise 3D texture on CPU, upload to GPU.
    pub fn new(device: &Device, queue: &Queue) -> Self {
        let size = wgpu::Extent3d {
            width: FLOW_RES,
            height: FLOW_RES,
            depth_or_array_layers: FLOW_RES,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("flow-field-3d"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("flow-field-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Bake curl noise on CPU
        let data = bake_curl_noise(FLOW_RES);

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(FLOW_RES * 8), // 4 x f16 = 8 bytes per texel
                rows_per_image: Some(FLOW_RES),
            },
            size,
        );

        Self {
            texture,
            view,
            sampler,
        }
    }

    /// Create a 1x1x1 placeholder texture (zero velocity) for effects without flow fields.
    pub fn placeholder(device: &Device, queue: &Queue) -> Self {
        let size = wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("flow-field-placeholder"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("flow-field-placeholder-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Zero velocity
        let data = [0u8; 8]; // 4 x f16 zeros
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8),
                rows_per_image: Some(1),
            },
            size,
        );

        Self {
            texture,
            view,
            sampler,
        }
    }
}

/// Bake a 3D curl noise field into Rgba16Float data.
/// Returns Vec<u8> of size res^3 * 8 bytes (4 channels x f16 per texel).
fn bake_curl_noise(res: u32) -> Vec<u8> {
    let n = res as usize;
    let mut data = Vec::with_capacity(n * n * n * 8);

    let eps = 1.0 / res as f32;

    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let px = x as f32 / res as f32;
                let py = y as f32 / res as f32;
                let pz = z as f32 / res as f32;

                // Curl of 3D noise: curl(F) where F = (noise_a, noise_b, noise_c)
                // Each component is a different noise field (offset seeds)
                let curl = curl_3d(px, py, pz, eps);

                // Normalize to [-1, 1] range (curl values are typically small)
                // Scale by 2.0 to get useful velocity magnitudes
                let vx = curl[0] * 2.0;
                let vy = curl[1] * 2.0;
                let vz = curl[2] * 2.0;

                // Pack as f16 (Rgba16Float)
                data.extend_from_slice(&f32_to_f16(vx).to_le_bytes());
                data.extend_from_slice(&f32_to_f16(vy).to_le_bytes());
                data.extend_from_slice(&f32_to_f16(vz).to_le_bytes());
                data.extend_from_slice(&f32_to_f16(0.0).to_le_bytes());
            }
        }
    }

    data
}

/// Compute curl of a potential vector field at point (x,y,z).
/// Uses central differences of 3D simplex-style noise.
fn curl_3d(x: f32, y: f32, z: f32, eps: f32) -> [f32; 3] {
    // We use three independent noise fields (A, B, C) as a potential vector field.
    // curl(A,B,C) = (dC/dy - dB/dz, dA/dz - dC/dx, dB/dx - dA/dy)

    let dc_dy = (noise_3d(x, y + eps, z, 2.0) - noise_3d(x, y - eps, z, 2.0)) / (2.0 * eps);
    let db_dz = (noise_3d(x, y, z + eps, 1.0) - noise_3d(x, y, z - eps, 1.0)) / (2.0 * eps);

    let da_dz = (noise_3d(x, y, z + eps, 0.0) - noise_3d(x, y, z - eps, 0.0)) / (2.0 * eps);
    let dc_dx = (noise_3d(x + eps, y, z, 2.0) - noise_3d(x - eps, y, z, 2.0)) / (2.0 * eps);

    let db_dx = (noise_3d(x + eps, y, z, 1.0) - noise_3d(x - eps, y, z, 1.0)) / (2.0 * eps);
    let da_dy = (noise_3d(x, y + eps, z, 0.0) - noise_3d(x, y - eps, z, 0.0)) / (2.0 * eps);

    [dc_dy - db_dz, da_dz - dc_dx, db_dx - da_dy]
}

/// Simple 3D value noise with seed offset. Returns value in [0, 1].
fn noise_3d(x: f32, y: f32, z: f32, seed: f32) -> f32 {
    // Scale coordinates for interesting detail
    let sx = x * 4.0 + seed * 17.1;
    let sy = y * 4.0 + seed * 31.7;
    let sz = z * 4.0 + seed * 47.3;

    // FBM with 3 octaves for smooth curl fields
    let mut val = 0.0f32;
    let mut amp = 0.5f32;
    let mut fx = sx;
    let mut fy = sy;
    let mut fz = sz;

    for _ in 0..3 {
        val += amp * value_noise_3d(fx, fy, fz);
        fx *= 2.0;
        fy *= 2.0;
        fz *= 2.0;
        amp *= 0.5;
    }

    val
}

/// 3D value noise using integer hashing and trilinear interpolation.
fn value_noise_3d(x: f32, y: f32, z: f32) -> f32 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let iz = z.floor() as i32;
    let fx = x - x.floor();
    let fy = y - y.floor();
    let fz = z - z.floor();

    // Quintic interpolation for smoother gradients
    let ux = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let uy = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
    let uz = fz * fz * fz * (fz * (fz * 6.0 - 15.0) + 10.0);

    let v000 = ihash_f(ix, iy, iz);
    let v100 = ihash_f(ix + 1, iy, iz);
    let v010 = ihash_f(ix, iy + 1, iz);
    let v110 = ihash_f(ix + 1, iy + 1, iz);
    let v001 = ihash_f(ix, iy, iz + 1);
    let v101 = ihash_f(ix + 1, iy, iz + 1);
    let v011 = ihash_f(ix, iy + 1, iz + 1);
    let v111 = ihash_f(ix + 1, iy + 1, iz + 1);

    let x00 = lerp(v000, v100, ux);
    let x10 = lerp(v010, v110, ux);
    let x01 = lerp(v001, v101, ux);
    let x11 = lerp(v011, v111, ux);

    let y0 = lerp(x00, x10, uy);
    let y1 = lerp(x01, x11, uy);

    lerp(y0, y1, uz)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Integer hash → float in [0, 1].
fn ihash_f(x: i32, y: i32, z: i32) -> f32 {
    let mut n = x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663) ^ z.wrapping_mul(83492791);
    n = (n >> 13) ^ n;
    n = n
        .wrapping_mul(n.wrapping_mul(n.wrapping_mul(60493)).wrapping_add(19990303))
        .wrapping_add(1376312589);
    (n as u32 as f32) / (u32::MAX as f32)
}

/// Convert f32 to IEEE 754 half-precision (f16) as u16.
fn f32_to_f16(val: f32) -> u16 {
    let bits = val.to_bits();
    let sign = (bits >> 31) & 1;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let frac = bits & 0x7FFFFF;

    if exp == 0xFF {
        // Inf or NaN
        return ((sign << 15) | 0x7C00 | (if frac != 0 { 0x200 } else { 0 })) as u16;
    }

    let new_exp = exp - 127 + 15;

    if new_exp >= 31 {
        // Overflow → Inf
        return ((sign << 15) | 0x7C00) as u16;
    }

    if new_exp <= 0 {
        // Underflow → zero or denorm
        if new_exp < -10 {
            return (sign << 15) as u16;
        }
        let frac = (frac | 0x800000) >> (1 - new_exp);
        return ((sign << 15) | (frac >> 13)) as u16;
    }

    ((sign << 15) | ((new_exp as u32) << 10) | (frac >> 13)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bake_curl_noise_correct_size() {
        let data = bake_curl_noise(8);
        assert_eq!(data.len(), 8 * 8 * 8 * 8); // 8^3 texels * 8 bytes per texel
    }

    #[test]
    fn curl_values_bounded() {
        // Check that curl values are reasonable
        let curl = curl_3d(0.5, 0.5, 0.5, 0.01);
        for c in &curl {
            assert!(c.is_finite(), "curl component should be finite");
            assert!(c.abs() < 100.0, "curl component should be bounded");
        }
    }

    #[test]
    fn f16_roundtrip() {
        let test_vals = [0.0f32, 1.0, -1.0, 0.5, -0.5, 0.001];
        for &v in &test_vals {
            let h = f32_to_f16(v);
            // Reconstruct f32 from f16 to verify (approximate)
            let sign = ((h >> 15) & 1) as f32;
            let exp = ((h >> 10) & 0x1F) as i32;
            let frac = (h & 0x3FF) as f32;
            if exp == 0 && frac == 0.0 {
                assert!(v.abs() < 0.001, "zero should roundtrip");
                continue;
            }
            let reconstructed =
                (-1.0f32).powf(sign) * 2.0f32.powi(exp - 15) * (1.0 + frac / 1024.0);
            let error = (reconstructed - v).abs();
            assert!(
                error < 0.01,
                "f16 roundtrip error too large: {v} → {reconstructed} (err={error})"
            );
        }
    }
}
