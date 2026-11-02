//! The negative half of the user A/V sync offset (#80): hold the analysis output back.
//!
//! A positive offset is a prediction (the beat grid knows its next instant, see
//! `BeatScheduler::user_lead`). A negative one is the opposite case — the sound reaches the
//! audience after the picture does (a long throw from the PA) — and there everything has to
//! wait: levels, bands and phases as much as the pulses. So the whole hop output is queued on
//! the sample clock and released `delay` seconds after the hop it came from.

use std::collections::VecDeque;

/// Hop outputs waiting out the delay, oldest first, each with its hop's sample-clock time.
pub(crate) struct OutputDelay<T> {
    queue: VecDeque<(f64, T)>,
}

impl<T> OutputDelay<T> {
    pub(crate) fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    pub(crate) fn push(&mut self, timestamp: f64, item: T) {
        self.queue.push_back((timestamp, item));
    }

    /// The oldest queued item that is due at `now` under `delay` seconds, if any. Reading the
    /// delay at release rather than at push keeps the queue in order when the user moves the
    /// offset: shortening it releases the backlog at once, lengthening it holds what is
    /// queued. Zero delay releases everything up to `now`, so the queue only ever holds the
    /// hop just pushed for the length of one call.
    pub(crate) fn pop_due(&mut self, now: f64, delay: f64) -> Option<T> {
        let &(ts, _) = self.queue.front()?;
        if ts + delay <= now {
            self.queue.pop_front().map(|(_, item)| item)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_delay_passes_straight_through() {
        let mut d = OutputDelay::new();
        d.push(1.0, 'a');
        assert_eq!(d.pop_due(1.0, 0.0), Some('a'));
        assert_eq!(d.pop_due(1.0, 0.0), None);
    }

    #[test]
    fn holds_each_item_for_the_delay_in_order() {
        let mut d = OutputDelay::new();
        // Binary-exact times, so the comparison is about the queue, not float rounding.
        let hop = 0.125;
        let mut out = Vec::new();
        for i in 0..20 {
            let now = f64::from(i) * hop;
            d.push(now, i);
            while let Some(x) = d.pop_due(now, 5.0 * hop) {
                out.push((x, i));
            }
        }
        assert!(!out.is_empty());
        for (x, released_at) in out {
            assert_eq!(released_at - x, 5, "item {x} released at hop {released_at}");
        }
    }

    #[test]
    fn shortening_the_delay_releases_the_backlog_in_order() {
        let mut d = OutputDelay::new();
        for i in 0..5 {
            d.push(f64::from(i) * 0.01, i);
        }
        let mut out = Vec::new();
        while let Some(x) = d.pop_due(0.05, 0.0) {
            out.push(x);
        }
        assert_eq!(out, vec![0, 1, 2, 3, 4]);
    }
}
