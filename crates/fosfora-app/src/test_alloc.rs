//! Test-only counting allocator — the I8 verifier.
//!
//! Invariant I8 (docs/trama/HANDOFF.md): after graph topology stabilizes,
//! the trama frame loop performs zero heap allocations and zero texture
//! creation. Texture/resource identity is asserted by the GPU steady-state
//! probes (pool stats, feedback generation, plan version); THIS module
//! catches the heap half, which no resource counter can see.
//!
//! The counter is a thread-local, so parallel tests never pollute each
//! other's measurements; the `#[global_allocator]` only exists in the test
//! binary. Counting covers `alloc`/`alloc_zeroed`/`realloc` — a `dealloc`
//! is not an allocation and steady-state code is allowed to drop things it
//! didn't allocate that frame.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

pub struct CountingAllocator;

// SAFETY: pure passthrough to `System`; the only addition is a thread-local
// counter bump, and `try_with` keeps TLS teardown from recursing.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        // SAFETY: caller upholds GlobalAlloc's contract; forwarded verbatim.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: caller upholds GlobalAlloc's contract; forwarded verbatim.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        // SAFETY: caller upholds GlobalAlloc's contract; forwarded verbatim.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        // SAFETY: caller upholds GlobalAlloc's contract; forwarded verbatim.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

/// Heap allocations made by the calling thread while `f` ran.
pub fn count_allocs<R>(f: impl FnOnce() -> R) -> (u64, R) {
    let before = ALLOCS.with(Cell::get);
    let result = f();
    (ALLOCS.with(Cell::get) - before, result)
}
