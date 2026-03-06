use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindingResource, Buffer,
    Device, Queue, Sampler, TextureView,
};

/// Shader uniforms packed for GPU consumption (288 bytes).
/// Must be kept in sync with the WGSL `PhosphorUniforms` struct.
#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct ShaderUniforms {
    pub time: f32,
    pub delta_time: f32,
    pub resolution: [f32; 2],
    // 16 bytes

    // Audio bands (7) + rms
    pub sub_bass: f32,
    pub bass: f32,
    pub low_mid: f32,
    pub mid: f32,
    pub upper_mid: f32,
    pub presence: f32,
    pub brilliance: f32,
    pub rms: f32,
    // 32 bytes (48 total)

    // Audio features (12)
    pub kick: f32,
    pub centroid: f32,
    pub flux: f32,
    pub flatness: f32,
    pub rolloff: f32,
    pub bandwidth: f32,
    pub zcr: f32,
    pub onset: f32,
    pub beat: f32,
    pub beat_phase: f32,
    pub bpm: f32,
    pub beat_strength: f32,
    // 48 bytes (96 total)

    // User params
    pub params: [f32; 16],
    // 64 bytes (160 total)

    // Feedback / multi-pass uniforms
    pub feedback_decay: f32,
    pub frame_index: f32,
    // 8 bytes (168 total)

    // Derived audio features
    pub dominant_chroma: f32,
    // Align to 16-byte boundary for GPU array types
    pub _pad_align: f32,
    // 8 bytes (176 total)

    // MFCC: 13 coefficients + 3 padding (array<vec4f, 4> on GPU)
    pub mfcc: [f32; 16],
    // 64 bytes (240 total)

    // Chroma: 12 pitch class energies (array<vec4f, 3> on GPU)
    pub chroma: [f32; 12],
    // 48 bytes (288 total)
}

pub struct UniformBuffer {
    pub buffer: Buffer,
}

impl UniformBuffer {
    pub fn new(device: &Device) -> Self {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("phosphor-uniforms"),
            size: std::mem::size_of::<ShaderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer }
    }

    pub fn update(&self, queue: &Queue, uniforms: &ShaderUniforms) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(uniforms));
    }

    /// Create a bind group with uniform buffer + feedback texture + sampler.
    pub fn create_bind_group(
        &self,
        device: &Device,
        layout: &BindGroupLayout,
        prev_frame_view: &TextureView,
        prev_frame_sampler: &Sampler,
    ) -> BindGroup {
        device.create_bind_group(&BindGroupDescriptor {
            label: Some("phosphor-bind-group"),
            layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: self.buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::TextureView(prev_frame_view),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(prev_frame_sampler),
                },
            ],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_uniforms_size_288() {
        assert_eq!(std::mem::size_of::<ShaderUniforms>(), 288);
    }

    #[test]
    fn shader_uniforms_zeroed() {
        let u: ShaderUniforms = bytemuck::Zeroable::zeroed();
        assert_eq!(u.time, 0.0);
        assert_eq!(u.delta_time, 0.0);
        assert_eq!(u.resolution, [0.0, 0.0]);
        assert_eq!(u.sub_bass, 0.0);
        assert_eq!(u.beat, 0.0);
        assert_eq!(u.feedback_decay, 0.0);
        assert_eq!(u.frame_index, 0.0);
        assert_eq!(u.params, [0.0; 16]);
    }
}
