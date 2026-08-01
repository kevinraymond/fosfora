use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindingResource, Buffer,
    Device, Queue, Sampler, TextureView,
};

/// Shader uniforms packed for GPU consumption (400 bytes).
/// Must be kept in sync with the WGSL `PhosphorUniforms` struct in
/// `effect/loader.rs` (UNIFORM_BLOCK) and `assets/shaders/default.wgsl`.
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
    // Fractional mel-spectrogram scroll phase (0..1) for continuous terrain motion
    // (#1508 Strata). Repurposed from a 16-byte alignment pad — same slot/offset.
    pub scroll_phase: f32,
    // 8 bytes (176 total)

    // MFCC: 13 coefficients + 3 padding (array<vec4f, 4> on GPU)
    pub mfcc: [f32; 16],
    // 64 bytes (240 total)

    // Chroma: 12 pitch class energies (array<vec4f, 3> on GPU)
    pub chroma: [f32; 12],
    // 48 bytes (288 total)

    // ---- Reserved audio features (batched ABI bump #1505, "v2") ----
    // 15 scalars = 60 bytes. The single trailing pad the v2 bump added is now
    // consumed by the v3 tail below (percussive_energy), so no pad remains here.
    // All read 0.0 until each detector lands (then filled with zero ABI churn).
    // A10 loudness (#1461)
    pub loudness_m: f32,
    pub loudness_s: f32,
    pub loudness_trend: f32,
    // A11 key (#1462)
    pub key_class: f32,
    pub key_is_minor: f32,
    pub key_confidence: f32,
    // A12 downbeat (#1463)
    pub downbeat: f32,
    pub bar_phase: f32,
    pub beat_in_bar: f32,
    // A13 stereo (#1464)
    pub pan: f32,
    pub stereo_width: f32,
    pub stereo_corr: f32,
    // A18 structure (#1469)
    pub section_novelty: f32,
    pub buildup: f32,
    pub drop: f32,
    // 60 bytes (348 total)

    // ---- Reserved audio features (batched ABI bump #1629, "v3") ----
    // 13 scalars = 52 bytes. 288 base + 28 reserved scalars = 400, a multiple of 16
    // (no trailing pad needed — the former _pad_features slot is now percussive_energy).
    // All read 0.0 until each detector lands (then filled with zero ABI churn).
    // A14 HPSS (#1465)
    pub percussive_energy: f32,
    pub harmonic_energy: f32,
    pub harmonic_ratio: f32,
    // A15 pitch (#1466)
    pub pitch: f32,
    pub pitch_confidence: f32,
    // A16 spectral contrast (#1467)
    pub contrast_0: f32,
    pub contrast_1: f32,
    pub contrast_2: f32,
    pub contrast_3: f32,
    pub contrast_4: f32,
    pub contrast_5: f32,
    pub contrast_mean: f32,
    pub timbre_flux: f32,
    // 52 bytes (400 total)

    // ---- A13b per-band pan (#1801) ----
    // 7 band pans + 1 pad = 32 bytes, appended so every offset above stays put. Declared
    // `array<vec4f, 2>` in WGSL: uniform-address-space arrays need a 16-byte element stride,
    // the same reason `mfcc`/`chroma` are vec4 arrays there. Index it with the `band_pan()`
    // helper rather than by hand.
    pub band_pan: [f32; 8],
    // 32 bytes (432 total)

    // ---- Overlay clock (v4 ABI bump) ----
    // Monotonic 0-based counters from the DownbeatTracker (raw counts, exact in f32 to
    // 2^24): each steps by 1 exactly when its phase sawtooth wraps, so
    // `bar_index + bar_phase` is a continuous multi-bar clock — the primitive the overlay
    // family's `bars_per_cycle` runs on. Appended so every offset above stays put.
    pub bar_index: f32,
    pub beat_index: f32,
    pub _pad_clock: [f32; 2],
    // 16 bytes (448 total)
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

    /// Create a bind group for the effect layout (see `ShaderPipeline`).
    ///
    /// Bindings: 0 = uniform buffer, 1/2 = previous-frame feedback texture +
    /// sampler, 3/4/5 = A17 waveform / spectrum / spectrogram audio textures,
    /// 6 = the shared audio-texture sampler. During the reserve phase all three
    /// audio textures are the 1x1 placeholder view; the A17 DSP swaps in the real
    /// textures later without changing this layout (finding #1492).
    ///
    /// `inputs` are the multi-pass graph inputs (#1481) in declared order: each
    /// `(view, sampler)` binds a prior pass's output at binding `7+2i` / `8+2i`.
    /// The `layout` must have been built with a matching `input_count`.
    #[allow(clippy::too_many_arguments)]
    pub fn create_bind_group(
        &self,
        device: &Device,
        layout: &BindGroupLayout,
        prev_frame_view: &TextureView,
        prev_frame_sampler: &Sampler,
        waveform_view: &TextureView,
        spectrum_view: &TextureView,
        spectrogram_view: &TextureView,
        audio_sampler: &Sampler,
        inputs: &[(&TextureView, &Sampler)],
    ) -> BindGroup {
        let mut entries = vec![
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
            BindGroupEntry {
                binding: 3,
                resource: BindingResource::TextureView(waveform_view),
            },
            BindGroupEntry {
                binding: 4,
                resource: BindingResource::TextureView(spectrum_view),
            },
            BindGroupEntry {
                binding: 5,
                resource: BindingResource::TextureView(spectrogram_view),
            },
            BindGroupEntry {
                binding: 6,
                resource: BindingResource::Sampler(audio_sampler),
            },
        ];
        for (i, (view, sampler)) in inputs.iter().enumerate() {
            let i = i as u32;
            entries.push(BindGroupEntry {
                binding: 7 + 2 * i,
                resource: BindingResource::TextureView(view),
            });
            entries.push(BindGroupEntry {
                binding: 8 + 2 * i,
                resource: BindingResource::Sampler(sampler),
            });
        }
        device.create_bind_group(&BindGroupDescriptor {
            label: Some("phosphor-bind-group"),
            layout,
            entries: &entries,
        })
    }
}

/// Mirror every [`AudioFeatures`] field into its shader-uniform slot.
///
/// Extracted verbatim from `App::update`'s drain block so the headless scene
/// renderer (#2027) forwards features identically — a slot missed here is a
/// shader input that silently stays 0.0 offline while moving live.
pub fn mirror_audio_features(u: &mut ShaderUniforms, f: &crate::audio::AudioFeatures) {
    u.sub_bass = f.sub_bass;
    u.bass = f.bass;
    u.low_mid = f.low_mid;
    u.mid = f.mid;
    u.upper_mid = f.upper_mid;
    u.presence = f.presence;
    u.brilliance = f.brilliance;
    u.rms = f.rms;
    u.kick = f.kick;
    u.centroid = f.centroid;
    u.flux = f.flux;
    u.flatness = f.flatness;
    u.rolloff = f.rolloff;
    u.bandwidth = f.bandwidth;
    u.zcr = f.zcr;
    u.onset = f.onset;
    u.beat = f.beat;
    u.beat_phase = f.beat_phase;
    u.bpm = f.bpm;
    u.beat_strength = f.beat_strength;
    u.mfcc[..13].copy_from_slice(&f.mfcc);
    u.mfcc[13..].fill(0.0);
    u.chroma.copy_from_slice(&f.chroma);
    u.dominant_chroma = f.dominant_chroma;
    // Reserved audio features (batched ABI bump #1505) — forwarded now so
    // each detector's follow-up only has to fill the AudioFeatures field.
    u.loudness_m = f.loudness_m;
    u.loudness_s = f.loudness_s;
    u.loudness_trend = f.loudness_trend;
    u.key_class = f.key_class;
    u.key_is_minor = f.key_is_minor;
    u.key_confidence = f.key_confidence;
    u.downbeat = f.downbeat;
    u.bar_phase = f.bar_phase;
    u.beat_in_bar = f.beat_in_bar;
    u.pan = f.pan;
    u.stereo_width = f.stereo_width;
    u.stereo_corr = f.stereo_corr;
    // A13b per-band pan (#1801). Slot 7 is padding for the vec4 stride.
    u.band_pan[0] = f.band_pan_sub_bass;
    u.band_pan[1] = f.band_pan_bass;
    u.band_pan[2] = f.band_pan_low_mid;
    u.band_pan[3] = f.band_pan_mid;
    u.band_pan[4] = f.band_pan_upper_mid;
    u.band_pan[5] = f.band_pan_presence;
    u.band_pan[6] = f.band_pan_brilliance;
    u.section_novelty = f.section_novelty;
    u.buildup = f.buildup;
    u.drop = f.drop;
    // Reserved audio features (batched ABI bump #1629, "v3").
    u.percussive_energy = f.percussive_energy;
    u.harmonic_energy = f.harmonic_energy;
    u.harmonic_ratio = f.harmonic_ratio;
    u.pitch = f.pitch;
    u.pitch_confidence = f.pitch_confidence;
    u.contrast_0 = f.contrast_0;
    u.contrast_1 = f.contrast_1;
    u.contrast_2 = f.contrast_2;
    u.contrast_3 = f.contrast_3;
    u.contrast_4 = f.contrast_4;
    u.contrast_5 = f.contrast_5;
    u.contrast_mean = f.contrast_mean;
    u.timbre_flux = f.timbre_flux;
    // Overlay clock (v4).
    u.bar_index = f.bar_index;
    u.beat_index = f.beat_index;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_uniforms_size_448() {
        // 288 (through chroma) + 28 reserved audio scalars = 400, the A13b per-band pan
        // block (#1801) appends 8 slots = 432, then the v4 overlay clock appends
        // bar_index/beat_index + 2 pads = 448. Must stay a multiple of 16 for the
        // array<vec4f> members and match the WGSL PhosphorUniforms struct byte-for-byte
        // (declared twice: effect/loader.rs UNIFORM_BLOCK and assets/shaders/default.wgsl).
        assert_eq!(std::mem::size_of::<ShaderUniforms>(), 448);
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
