//! Transient render-target pool for the trama executor.
//!
//! Targets are keyed `(width, height, format)` and handed out at plan build —
//! not per frame — so with stable topology every node keeps the same texture
//! across frames (which is what makes cached bind groups sound and gives I8's
//! zero-steady-state-allocation directly; see `DECISIONS.md` on the §9.3
//! timing deviation). The pool is dropped wholesale only on output-resolution
//! change.

use crate::gpu::render_target::RenderTarget;

pub struct TexturePool {
    entries: Vec<PoolEntry>,
}

struct PoolEntry {
    rt: RenderTarget,
    in_use: bool,
}

/// Pick the first free entry matching `key` — factored pure so the free-list
/// bookkeeping tests without a GPU.
fn select_free(
    entries: &[(u32, u32, wgpu::TextureFormat, bool)],
    key: (u32, u32, wgpu::TextureFormat),
) -> Option<usize> {
    entries
        .iter()
        .position(|&(w, h, f, in_use)| !in_use && (w, h, f) == key)
}

impl TexturePool {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Reuse a free matching target or create one. Returns a stable index —
    /// valid until `clear()`.
    pub fn acquire(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> usize {
        let keys: Vec<(u32, u32, wgpu::TextureFormat, bool)> = self
            .entries
            .iter()
            .map(|e| (e.rt.width, e.rt.height, e.rt.format, e.in_use))
            .collect();
        if let Some(i) = select_free(&keys, (width, height, format)) {
            self.entries[i].in_use = true;
            return i;
        }
        self.entries.push(PoolEntry {
            rt: RenderTarget::new(device, width, height, format, 1.0, "trama-pool"),
            in_use: true,
        });
        self.entries.len() - 1
    }

    pub fn get(&self, index: usize) -> &RenderTarget {
        &self.entries[index].rt
    }

    /// Return every target to the free list (start of a plan build).
    pub fn release_all(&mut self) {
        for e in &mut self.entries {
            e.in_use = false;
        }
    }

    /// Drop all targets — output-resolution change only (handoff §9.3).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// `(in_use, total)` — the canvas debug line.
    pub fn stats(&self) -> (usize, usize) {
        let in_use = self.entries.iter().filter(|e| e.in_use).count();
        (in_use, self.entries.len())
    }
}

#[cfg(test)]
mod tests {
    use super::select_free;
    use wgpu::TextureFormat::{Rgba8Unorm, Rgba16Float};

    #[test]
    fn select_free_reuses_released_matching_target() {
        let entries = [
            (1920, 1080, Rgba16Float, true),
            (1920, 1080, Rgba16Float, false),
            (1920, 1080, Rgba16Float, false),
        ];
        assert_eq!(select_free(&entries, (1920, 1080, Rgba16Float)), Some(1));
    }

    #[test]
    fn select_free_rejects_size_and_format_mismatch() {
        let entries = [
            (960, 540, Rgba16Float, false),
            (1920, 1080, Rgba8Unorm, false),
        ];
        assert_eq!(select_free(&entries, (1920, 1080, Rgba16Float)), None);
    }

    #[test]
    fn select_free_skips_in_use() {
        let entries = [(1920, 1080, Rgba16Float, true)];
        assert_eq!(select_free(&entries, (1920, 1080, Rgba16Float)), None);
    }
}
