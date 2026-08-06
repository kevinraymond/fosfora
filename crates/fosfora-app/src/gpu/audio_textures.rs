//! A17 audio textures (#1468): the three GPU textures that back the shader `waveform(x)`
//! / `spectrum(x)` / `spectrogram(uv)` helpers, filling the bind-group slots (bindings
//! 3/4/5 + sampler 6) that #1505 reserved and wired to a 1×1 placeholder.
//!
//! - **waveform** — `Rg16Float` 1024×1, r=min / g=max of a min/max-decimated PCM window,
//!   zero-crossing-triggered CPU-side so the scope doesn't jitter.
//! - **spectrum** — `R16Float` 512×1, log-frequency magnitude in 0..1.
//! - **spectrogram** — `R8Unorm` `HISTORY_FRAMES`×`MELS`, a scrolling mel-spectrogram.
//!   The ABI is frozen (no `spectrogram_head` uniform), so the shader samples raw `uv`;
//!   the CPU history is therefore kept in linear time order (uv.x = time, oldest→newest;
//!   uv.y = mel) and re-uploaded each frame rather than addressed circularly.
//!
//! The textures are fixed-size and never resized, so their views are stable — bind groups
//! built once stay valid while per-frame `queue.write_texture` refreshes the contents.

use wgpu::{Device, Queue, Sampler, Texture, TextureView};

use crate::audio::analyzer::{SPECTROGRAM_MELS, SPECTRUM_BINS};
use crate::gpu::half::f32_to_f16;

/// Waveform texture width (texels).
const WAVEFORM_TEXELS: usize = 1024;
/// PCM samples decimated into the waveform texture (2 samples → 1 min/max texel).
const WAVEFORM_WINDOW: usize = WAVEFORM_TEXELS * 2;
/// Lead samples searched for a rising zero-crossing to stabilize the scope trigger.
const WAVEFORM_TRIGGER_SEARCH: usize = 512;
/// Total PCM window to peek from the ring: decimation window + trigger search room.
pub const WAVEFORM_PEEK: usize = WAVEFORM_WINDOW + WAVEFORM_TRIGGER_SEARCH;

/// Number of time frames (columns) of mel-spectrogram history.
const HISTORY_FRAMES: usize = 512;

/// The three A17 audio textures plus their shared sampler and the CPU-side spectrogram
/// history buffer.
pub struct AudioTextures {
    waveform_tex: Texture,
    spectrum_tex: Texture,
    spectrogram_tex: Texture,
    pub waveform_view: TextureView,
    pub spectrum_view: TextureView,
    pub spectrogram_view: TextureView,
    pub sampler: Sampler,
    /// Mel-spectrogram history in mel-major, time-ordered layout: row `m` occupies
    /// `[m*HISTORY_FRAMES .. m*HISTORY_FRAMES + HISTORY_FRAMES]`, time increasing left to
    /// right (index 0 = oldest, HISTORY_FRAMES-1 = newest). Held as f32 magnitude and
    /// converted to f16 on upload — the texture is **R16Float** (#1508): 8-bit R8Unorm
    /// quantized the height to 256 levels, and with the shader's `pow(0.55)` perceptual
    /// lift (whose derivative explodes near 0) each LSB became a visible terrace ledge.
    /// f16's fine precision removes those steps.
    mel_history: Vec<f32>,
    /// Reused f16 (little-endian) byte scratch for the R16Float upload (2 bytes/texel).
    mel_upload: Vec<u8>,
    /// Per-band running EMA of the incoming mel column. Each new column is low-passed
    /// against this before being committed, so terrain ridges rise/fall smoothly instead
    /// of snapping per column (#1508). Strata is the sole real `spectrogram()` consumer.
    mel_smoothed: [f32; SPECTROGRAM_MELS],
}

/// Per-column low-pass weight for the mel EMA (columns arrive at the fixed audio hop rate,
/// ~43 Hz, so a constant weight is frame-rate stable). ~0.4 gives a ~80–100 ms time constant:
/// fast enough to keep transients legible, slow enough to kill per-frame terrain snap.
const MEL_EMA_ALPHA: f32 = 0.4;

impl AudioTextures {
    pub fn new(device: &Device, queue: &Queue) -> Self {
        let waveform_tex = create_tex(
            device,
            "audio-waveform",
            WAVEFORM_TEXELS as u32,
            1,
            wgpu::TextureFormat::Rg16Float,
        );
        let spectrum_tex = create_tex(
            device,
            "audio-spectrum",
            SPECTRUM_BINS as u32,
            1,
            wgpu::TextureFormat::R16Float,
        );
        let spectrogram_tex = create_tex(
            device,
            "audio-spectrogram",
            HISTORY_FRAMES as u32,
            SPECTROGRAM_MELS as u32,
            wgpu::TextureFormat::R16Float,
        );

        let waveform_view = waveform_tex.create_view(&Default::default());
        let spectrum_view = spectrum_tex.create_view(&Default::default());
        let spectrogram_view = spectrogram_tex.create_view(&Default::default());

        // Clamp so the y=0.5 row (waveform/spectrum) and the uv edges don't wrap.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("audio-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Zero-init all three so they read 0.0 before audio arrives (placeholder parity).
        queue.write_texture(
            tex_copy(&waveform_tex),
            &vec![0u8; WAVEFORM_TEXELS * 4],
            row_layout(WAVEFORM_TEXELS as u32 * 4, 1),
            extent(WAVEFORM_TEXELS as u32, 1),
        );
        queue.write_texture(
            tex_copy(&spectrum_tex),
            &vec![0u8; SPECTRUM_BINS * 2],
            row_layout(SPECTRUM_BINS as u32 * 2, 1),
            extent(SPECTRUM_BINS as u32, 1),
        );
        let mel_history = vec![0.0f32; HISTORY_FRAMES * SPECTROGRAM_MELS];
        let mel_upload = vec![0u8; HISTORY_FRAMES * SPECTROGRAM_MELS * 2]; // f16 = 2 bytes/texel
        queue.write_texture(
            tex_copy(&spectrogram_tex),
            &mel_upload,
            row_layout(HISTORY_FRAMES as u32 * 2, SPECTROGRAM_MELS as u32),
            extent(HISTORY_FRAMES as u32, SPECTROGRAM_MELS as u32),
        );

        Self {
            waveform_tex,
            spectrum_tex,
            spectrogram_tex,
            waveform_view,
            spectrum_view,
            spectrogram_view,
            sampler,
            mel_history,
            mel_upload,
            mel_smoothed: [0.0; SPECTROGRAM_MELS],
        }
    }

    /// Upload a PCM window as a zero-crossing-triggered min/max waveform. `peeked` should
    /// hold [`WAVEFORM_PEEK`] samples (newest last); a shorter slice is padded/handled
    /// gracefully. Each output texel stores (min, max) over its 2 source samples.
    pub fn upload_waveform(&self, queue: &Queue, peeked: &[f32]) {
        // Find a rising zero-crossing in the lead window to lock the scope horizontally.
        // Fall back to offset 0 if the window is short or has no crossing.
        let max_offset = peeked.len().saturating_sub(WAVEFORM_WINDOW);
        let search = WAVEFORM_TRIGGER_SEARCH.min(max_offset);
        let mut offset = 0;
        for i in 0..search {
            if peeked[i] <= 0.0 && peeked[i + 1] > 0.0 {
                offset = i;
                break;
            }
        }

        let mut bytes = Vec::with_capacity(WAVEFORM_TEXELS * 4);
        for t in 0..WAVEFORM_TEXELS {
            let a = peeked.get(offset + 2 * t).copied().unwrap_or(0.0);
            let b = peeked.get(offset + 2 * t + 1).copied().unwrap_or(0.0);
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            bytes.extend_from_slice(&f32_to_f16(lo).to_le_bytes());
            bytes.extend_from_slice(&f32_to_f16(hi).to_le_bytes());
        }

        queue.write_texture(
            tex_copy(&self.waveform_tex),
            &bytes,
            row_layout(WAVEFORM_TEXELS as u32 * 4, 1),
            extent(WAVEFORM_TEXELS as u32, 1),
        );
    }

    /// Upload the log-frequency magnitude spectrum (length [`SPECTRUM_BINS`], values 0..1).
    pub fn upload_spectrum(&self, queue: &Queue, spectrum: &[f32]) {
        let mut bytes = Vec::with_capacity(SPECTRUM_BINS * 2);
        for i in 0..SPECTRUM_BINS {
            let v = spectrum.get(i).copied().unwrap_or(0.0);
            bytes.extend_from_slice(&f32_to_f16(v).to_le_bytes());
        }
        queue.write_texture(
            tex_copy(&self.spectrum_tex),
            &bytes,
            row_layout(SPECTRUM_BINS as u32 * 2, 1),
            extent(SPECTRUM_BINS as u32, 1),
        );
    }

    /// Scroll new mel columns into the history and re-upload the spectrogram. Each column
    /// has [`SPECTROGRAM_MELS`] bands in 0..1; columns are applied oldest-first. A no-op
    /// upload is skipped when no columns arrived (the texture holds its last contents).
    pub fn upload_spectrogram(&mut self, queue: &Queue, columns: &[Box<[f32]>]) {
        if columns.is_empty() {
            return;
        }
        for col in columns {
            // Temporally low-pass each band before committing (#1508): ridges rise/fall
            // smoothly instead of snapping per column. The EMA state advances once per
            // incoming column (fixed audio hop rate), so the weight is frame-rate stable.
            // Shift every mel row left by one (drop oldest) and append the smoothed sample
            // at the right edge, keeping the buffer in linear time order for raw-uv sampling.
            for m in 0..SPECTROGRAM_MELS {
                let raw = col.get(m).copied().unwrap_or(0.0).clamp(0.0, 1.0);
                self.mel_smoothed[m] += MEL_EMA_ALPHA * (raw - self.mel_smoothed[m]);
                let row = &mut self.mel_history[m * HISTORY_FRAMES..(m + 1) * HISTORY_FRAMES];
                row.copy_within(1.., 0);
                row[HISTORY_FRAMES - 1] = self.mel_smoothed[m];
            }
        }
        // Convert the f32 history to f16 (little-endian) and upload as R16Float.
        for (i, &v) in self.mel_history.iter().enumerate() {
            let bytes = f32_to_f16(v).to_le_bytes();
            self.mel_upload[i * 2] = bytes[0];
            self.mel_upload[i * 2 + 1] = bytes[1];
        }
        queue.write_texture(
            tex_copy(&self.spectrogram_tex),
            &self.mel_upload,
            row_layout(HISTORY_FRAMES as u32 * 2, SPECTROGRAM_MELS as u32),
            extent(HISTORY_FRAMES as u32, SPECTROGRAM_MELS as u32),
        );
    }
}

fn create_tex(
    device: &Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: extent(width, height),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

fn tex_copy(texture: &Texture) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo {
        texture,
        mip_level: 0,
        origin: wgpu::Origin3d::ZERO,
        aspect: wgpu::TextureAspect::All,
    }
}

fn row_layout(bytes_per_row: u32, rows: u32) -> wgpu::TexelCopyBufferLayout {
    wgpu::TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(bytes_per_row),
        rows_per_image: Some(rows),
    }
}
