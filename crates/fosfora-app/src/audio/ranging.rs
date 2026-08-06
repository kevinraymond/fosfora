//! Windowed percentile ranging — the shared primitive behind gated percentile
//! normalization (A2 #1453) and the kick's single detector-owned normalizer (A3 #1454).
//!
//! A [`PercentileWindow`] is a fixed-length ring of recent samples that answers
//! quantile queries (P5/P95 for adaptive ranging, long-term P95 for the kick). The
//! window length *is* the recovery-time knob: a transient spike ages out after
//! `capacity` pushes, so there is no separate "spike decay" heuristic — the old
//! running-min/max normalizer needed one because a single spike poisoned its max for
//! seconds.

/// Fixed-length ring of recent samples supporting percentile queries.
pub struct PercentileWindow {
    buf: Vec<f32>,
    cap: usize,
    head: usize,
    len: usize,
    /// Reused sort scratch so a query allocates nothing.
    scratch: Vec<f32>,
}

impl PercentileWindow {
    /// A window holding the most recent `cap` samples (`cap` is clamped to at least 1).
    pub fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            buf: vec![0.0; cap],
            cap,
            head: 0,
            len: 0,
            scratch: Vec::with_capacity(cap),
        }
    }

    /// Append a sample, overwriting the oldest once the window is full.
    pub fn push(&mut self, v: f32) {
        self.buf[self.head] = v;
        self.head = (self.head + 1) % self.cap;
        if self.len < self.cap {
            self.len += 1;
        }
    }

    /// The `q`-quantile (`q` in 0..1) of the current contents, or 0.0 if empty. Used by
    /// the A3 kick as its single long-term P95 normalizer.
    pub fn percentile(&mut self, q: f32) -> f32 {
        self.range(q, q).0
    }

    /// Both the `p_lo` and `p_hi` quantiles from a single sort — the hot path for
    /// adaptive ranging, which needs P5 and P95 together. Returns `(0.0, 0.0)` if empty.
    pub fn range(&mut self, p_lo: f32, p_hi: f32) -> (f32, f32) {
        if self.len == 0 {
            return (0.0, 0.0);
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.buf[..self.len]);
        self.scratch
            .sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        (
            quantile_of_sorted(&self.scratch, p_lo),
            quantile_of_sorted(&self.scratch, p_hi),
        )
    }
}

/// Linear-interpolated quantile of an already-sorted slice.
fn quantile_of_sorted(sorted: &[f32], q: f32) -> f32 {
    let n = sorted.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return sorted[0];
    }
    let q = q.clamp(0.0, 1.0);
    let idx = q * (n - 1) as f32;
    let lo = idx.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = idx - lo as f32;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_window_returns_zero() {
        let mut w = PercentileWindow::new(16);
        assert_eq!(w.range(0.05, 0.95), (0.0, 0.0));
    }

    #[test]
    fn percentiles_of_uniform_ramp() {
        // 0..=100 → median ~50, P5 ~5, P95 ~95.
        let mut w = PercentileWindow::new(101);
        for i in 0..=100 {
            w.push(i as f32);
        }
        assert!((w.range(0.5, 0.5).0 - 50.0).abs() < 1e-3);
        let (p5, p95) = w.range(0.05, 0.95);
        assert!((p5 - 5.0).abs() < 1e-3, "p5={p5}");
        assert!((p95 - 95.0).abs() < 1e-3, "p95={p95}");
    }

    #[test]
    fn ring_evicts_oldest() {
        // Capacity 4: after pushing 0,1,2,3,10,11,12,13 the window holds only 10..=13.
        let mut w = PercentileWindow::new(4);
        for v in [0.0, 1.0, 2.0, 3.0, 10.0, 11.0, 12.0, 13.0] {
            w.push(v);
        }
        let (lo, hi) = w.range(0.0, 1.0);
        assert_eq!(lo, 10.0);
        assert_eq!(hi, 13.0);
    }

    #[test]
    fn spike_ages_out() {
        // A lone spike lifts P95 while it is in the window, then leaves once enough
        // ordinary samples have been pushed — no separate decay logic needed.
        let mut w = PercentileWindow::new(8);
        w.push(100.0);
        for _ in 0..8 {
            w.push(1.0);
        }
        assert!((w.range(0.95, 0.95).0 - 1.0).abs() < 1e-3);
    }
}
