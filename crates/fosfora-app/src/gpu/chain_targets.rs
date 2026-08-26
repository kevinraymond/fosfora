//! Caller-owned output targets, one per trama chain.
//!
//! The executor used to own a single `output: RenderTarget` and hand back a
//! reference to it. Two things make that untenable once every layer can carry
//! a chain:
//!
//! - **One target, N chains.** Chain B would overwrite chain A's picture
//!   before the compositor ever read it. This, not the borrow, is the hard
//!   blocker.
//! - **The borrow.** `execute_and_composite` accumulates `LayerComposite`s
//!   holding `&RenderTarget` across loop iterations while calling back into
//!   `&mut TramaSystem` on each one. A target reached *through* the executor
//!   aliases; a target the caller owns does not.
//!
//! So the caller owns the targets and the executor renders into a `&RenderTarget`
//! it is handed — the same separation `Compositor` already uses for its
//! accumulator, whose rendering methods all take `&self` and write through
//! render-pass attachments.
//!
//! Transient (interior) targets stay in the executor's `TexturePool` and are
//! deliberately shared across chains: chains run as one contiguous sequential
//! run of passes, a chain's interior is dead once its output is written, and no
//! chain reads another's. Only the *outputs* need to be distinct.

use crate::bindings::catalog::MAX_LAYERS;
use crate::gpu::context::GpuContext;
use crate::gpu::render_target::RenderTarget;
use crate::trama::node::ChainId;

/// One slot per layer chain plus the master chain.
const SLOTS: usize = MAX_LAYERS + 1;

struct Slot {
    rt: RenderTarget,
    /// Identity stamp. Bind groups built during a plan can *sample* the output
    /// target (a preview blit of the final producer does exactly that), so a
    /// recreated target must invalidate that chain's plan. Monotonic across
    /// the whole struct, so a freed and re-allocated slot never reuses a
    /// generation.
    generation: u64,
}

pub struct ChainTargets {
    slots: [Option<Slot>; SLOTS],
    next_generation: u64,
    width: u32,
    height: u32,
}

impl ChainTargets {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            next_generation: 1,
            width,
            height,
        }
    }

    fn index(chain: ChainId) -> usize {
        chain.index() as usize
    }

    /// Allocate this chain's output target if it has none. Idempotent at a
    /// stable size — it must be, because it runs every frame and a generation
    /// bump costs a replan (I8).
    pub fn ensure(&mut self, device: &wgpu::Device, chain: ChainId) {
        let i = Self::index(chain);
        if self.slots[i].is_some() {
            return;
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        self.slots[i] = Some(Slot {
            rt: RenderTarget::new(
                device,
                self.width,
                self.height,
                GpuContext::hdr_format(),
                1.0,
                "trama-chain-output",
            ),
            generation,
        });
    }

    /// Drop this chain's output target — its layer lost its chain, or went
    /// away. The executor's own per-chain state is dropped separately, by
    /// `TramaExecutor::drop_chain`.
    #[allow(dead_code)] // driven by the layer-stack sync in stage C3/C5
    pub fn release(&mut self, chain: ChainId) {
        self.slots[Self::index(chain)] = None;
    }

    #[allow(dead_code)] // driven by the layer-stack sync in stage C3/C5
    pub fn has(&self, chain: ChainId) -> bool {
        self.slots[Self::index(chain)].is_some()
    }

    /// This chain's output target. Callers `ensure` first; a missing slot is a
    /// programming error, not a runtime condition.
    pub fn get(&self, chain: ChainId) -> &RenderTarget {
        &self.slots[Self::index(chain)]
            .as_ref()
            .expect("chain output target was ensured before execute")
            .rt
    }

    pub fn generation(&self, chain: ChainId) -> u64 {
        self.slots[Self::index(chain)]
            .as_ref()
            .expect("chain output target was ensured before execute")
            .generation
    }

    /// Output-resolution change: every target is the wrong size, so drop them
    /// all. The executor clears its plans on the same event, and the fresh
    /// generations they get on re-`ensure` make that belt-and-braces.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        self.width = width;
        self.height = height;
        self.slots = std::array::from_fn(|_| None);
    }

    /// How many chain outputs are resident — the stage-E VRAM checkpoint reads
    /// this next to the executor's pool stats.
    #[allow(dead_code)] // read by the stage-E perf checkpoint
    pub fn resident(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The slot arithmetic and generation policy, without a GPU: everything
    // here is about which index and which stamp, not about textures.

    #[test]
    fn every_chain_gets_a_distinct_slot() {
        let mut seen = std::collections::HashSet::new();
        for n in 0..MAX_LAYERS as u8 {
            assert!(
                seen.insert(ChainTargets::index(ChainId::Layer(n))),
                "layer {n} collides with an earlier slot"
            );
        }
        assert!(
            seen.insert(ChainTargets::index(ChainId::Master)),
            "master collides with a layer slot"
        );
        assert_eq!(seen.len(), SLOTS, "every slot is reachable");
        assert!(seen.iter().all(|&i| i < SLOTS), "no slot is out of range");
    }

    // Run: cargo test -p fosfora-app -- --ignored chain_target_generations
    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn chain_target_generations_are_stable_but_never_reused() {
        let (device, _queue) = crate::gpu::test_gpu::test_gpu();
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let mut t = ChainTargets::new(64, 64);
        let a = ChainId::Layer(0);

        t.ensure(&device, a);
        let first = t.generation(a);

        // Idempotent at a stable size. This is the I8 half: `ensure` runs
        // every frame, and a moving generation would replan every frame.
        for _ in 0..4 {
            t.ensure(&device, a);
        }
        assert_eq!(t.generation(a), first, "ensure must not churn the stamp");

        // …but the texture behind the slot is genuinely new after a release,
        // and a plan keyed on the old stamp would sample a dropped texture.
        t.release(a);
        assert!(!t.has(a));
        t.ensure(&device, a);
        assert_ne!(t.generation(a), first, "recreated target reused its stamp");

        // A resize drops everything, so the same rule applies across it.
        let after_release = t.generation(a);
        t.resize(128, 128);
        assert!(!t.has(a), "resize must drop the wrong-sized targets");
        t.ensure(&device, a);
        assert_ne!(t.generation(a), after_release);
        assert_ne!(t.generation(a), first);
    }

    #[test]
    #[ignore = "requires a GPU/software adapter"]
    fn chain_targets_are_independent_per_chain() {
        let (device, _queue) = crate::gpu::test_gpu::test_gpu();
        let _guard = crate::gpu::test_gpu::gpu_guard();
        let mut t = ChainTargets::new(64, 64);
        t.ensure(&device, ChainId::Layer(0));
        t.ensure(&device, ChainId::Master);
        assert_eq!(t.resident(), 2);
        assert_ne!(
            t.generation(ChainId::Layer(0)),
            t.generation(ChainId::Master)
        );
        // Releasing one leaves the other alone — the whole point of the slots.
        t.release(ChainId::Layer(0));
        assert!(!t.has(ChainId::Layer(0)));
        assert!(t.has(ChainId::Master));
    }
}
