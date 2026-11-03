//! Fixed-size ring buffer with the median/MAD statistics the onset threshold uses.

// ---------------------------------------------------------------------------
// Circular buffer (fixed-size ring buffer with statistical methods)
// ---------------------------------------------------------------------------

pub(super) struct CircularBuffer {
    buf: Vec<f64>,
    cap: usize,
    write: usize,
    count: usize,
    /// Reused by the median/MAD selection so the per-hop threshold allocates nothing.
    scratch: Vec<f64>,
}

impl CircularBuffer {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0.0; capacity],
            cap: capacity,
            write: 0,
            count: 0,
            scratch: Vec::with_capacity(capacity),
        }
    }

    pub(super) fn push(&mut self, value: f64) {
        self.buf[self.write] = value;
        self.write = (self.write + 1) % self.cap;
        if self.count < self.cap {
            self.count += 1;
        }
    }

    pub(super) fn len(&self) -> usize {
        self.count
    }

    /// The contents oldest-first, written into `out` (cleared first).
    pub(super) fn copy_values_into(&self, out: &mut Vec<f64>) {
        out.clear();
        if self.count < self.cap {
            out.extend_from_slice(&self.buf[..self.count]);
        } else {
            out.extend_from_slice(&self.buf[self.write..]);
            out.extend_from_slice(&self.buf[..self.write]);
        }
    }

    #[cfg(test)]
    pub(super) fn values(&self) -> Vec<f64> {
        let mut v = Vec::new();
        self.copy_values_into(&mut v);
        v
    }

    #[cfg(test)]
    pub(super) fn median(&mut self) -> f64 {
        self.median_mad().0
    }

    #[cfg(test)]
    pub(super) fn mad(&mut self) -> f64 {
        self.median_mad().1
    }

    /// Median and median absolute deviation, `(0.0, 0.0)` when empty. One pass fills
    /// the scratch, so the median is found once and nothing is allocated (#75).
    pub(super) fn median_mad(&mut self) -> (f64, f64) {
        if self.count == 0 {
            return (0.0, 0.0);
        }
        self.scratch.clear();
        self.scratch.extend_from_slice(&self.buf[..self.count]);
        let med = median_in_place(&mut self.scratch);
        for v in &mut self.scratch {
            *v = (*v - med).abs();
        }
        (med, median_in_place(&mut self.scratch))
    }

    pub(super) fn max(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        if self.count < self.cap {
            self.buf[..self.count]
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max)
        } else {
            self.buf.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
        }
    }

    // Retained stats-utility sibling of `median`/`mad`/`max` (has its own unit test). Its
    // prod caller was the tempo estimator's runtime frame-timing average, removed in A5
    // (#1456) now that the analysis hop is fixed.
    #[allow(dead_code)]
    pub(super) fn mean(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        if self.count < self.cap {
            self.buf[..self.count].iter().sum::<f64>() / self.count as f64
        } else {
            self.buf.iter().sum::<f64>() / self.cap as f64
        }
    }
}

/// Median of a non-empty slice (mean of the middle two for an even length), by
/// selection; reorders `v`. Matches sorting and indexing the middle.
fn median_in_place(v: &mut [f64]) -> f64 {
    let mid = v.len() / 2;
    let even = v.len().is_multiple_of(2);
    let (below, at, _) = v.select_nth_unstable_by(mid, f64::total_cmp);
    if even {
        let lower = below.iter().copied().max_by(f64::total_cmp).unwrap_or(*at);
        f64::midpoint(lower, *at)
    } else {
        *at
    }
}
