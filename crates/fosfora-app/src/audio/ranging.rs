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
    /// Reused selection scratch so a query allocates nothing.
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

    /// Both the `p_lo` and `p_hi` quantiles — the hot path for adaptive ranging, which
    /// needs P5 and P95 together every hop. Returns `(0.0, 0.0)` if empty.
    ///
    /// Selection rather than a sort (#74): the higher quantile is found with one O(n)
    /// partition, which leaves every smaller value in the prefix, so the lower one only
    /// partitions that prefix. Results match a full sort exactly.
    pub fn range(&mut self, p_lo: f32, p_hi: f32) -> (f32, f32) {
        let n = self.len;
        match n {
            0 => return (0.0, 0.0),
            1 => return (self.buf[0], self.buf[0]),
            _ => {}
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.buf[..n]);

        let (q_small, q_big) = if p_lo <= p_hi {
            (p_lo, p_hi)
        } else {
            (p_hi, p_lo)
        };
        let (big_i, big_frac) = quantile_index(n, q_big);
        let (a, b) = select_pair(&mut self.scratch, big_i);
        let big = lerp(a, b, big_frac);
        let (small_i, small_frac) = quantile_index(n, q_small);
        let small = if small_i == big_i {
            lerp(a, b, small_frac)
        } else {
            let (c, d) = select_pair(&mut self.scratch[..=big_i], small_i);
            lerp(c, d, small_frac)
        };

        if p_lo <= p_hi {
            (small, big)
        } else {
            (big, small)
        }
    }
}

/// Where the linear-interpolated `q`-quantile of `n` sorted values falls: the lower
/// index and the fraction of the way to the next one.
fn quantile_index(n: usize, q: f32) -> (usize, f32) {
    let idx = q.clamp(0.0, 1.0) * (n - 1) as f32;
    let lo = (idx.floor() as usize).min(n - 1);
    (lo, idx - lo as f32)
}

/// The values that would sit at sorted positions `i` and `i + 1` (or `i` twice at the
/// end). Partitions `v` so everything before `i` is no larger than `v[i]`.
fn select_pair(v: &mut [f32], i: usize) -> (f32, f32) {
    let (_, at, above) = v.select_nth_unstable_by(i, f32::total_cmp);
    let at = *at;
    let next = above.iter().copied().min_by(f32::total_cmp).unwrap_or(at);
    (at, next)
}

fn lerp(a: f32, b: f32, frac: f32) -> f32 {
    a * (1.0 - frac) + b * frac
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

    /// Selection must give exactly what the old full sort gave, for any window contents
    /// and quantile pair — including duplicates, a partly filled window and q outside 0..1.
    #[test]
    fn selection_matches_a_full_sort() {
        fn sorted_quantile(sorted: &[f32], q: f32) -> f32 {
            let (lo, frac) = quantile_index(sorted.len(), q);
            let hi = (lo + 1).min(sorted.len() - 1);
            lerp(sorted[lo], sorted[hi], frac)
        }

        let mut seed = 0x2545_f491_u32;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let pairs = [
            (0.05, 0.95),
            (0.95, 0.05),
            (0.5, 0.5),
            (0.0, 1.0),
            (-0.2, 1.3),
            (0.33, 0.34),
            (0.95, 0.95),
        ];
        for cap in [1usize, 2, 3, 7, 64, 344] {
            let mut w = PercentileWindow::new(cap);
            for step in 0..(cap * 3) {
                // Coarse values so duplicates are common.
                w.push((rand() % 23) as f32 * 0.5 - 3.0);
                let mut sorted = w.buf[..w.len].to_vec();
                sorted.sort_by(f32::total_cmp);
                for &(lo, hi) in &pairs {
                    let want = (sorted_quantile(&sorted, lo), sorted_quantile(&sorted, hi));
                    assert_eq!(
                        w.range(lo, hi),
                        want,
                        "cap {cap} step {step} q ({lo}, {hi})"
                    );
                }
            }
        }
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
