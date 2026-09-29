//! Bounded inbound queues that keep the newest message.
//!
//! OSC, the web remote, MIDI and the audio analysis thread feed the render thread
//! through bounded queues. A plain `try_send` on a full queue drops the message
//! being sent, and during a render stall (a shader compiling on effect load) that
//! is the end of a fader drag: the parameter stays at whatever intermediate value
//! got through (#103), and the audio features lag behind the music (#59). A full
//! queue here drops its oldest message instead, so the newest always arrives.

use crossbeam_channel::{Receiver, Sender, TrySendError};

/// A bounded queue whose sender evicts the oldest queued message when full.
pub(crate) fn bounded<T>(cap: usize) -> (DropOldestSender<T>, Receiver<T>) {
    let (tx, rx) = crossbeam_channel::bounded(cap);
    (
        DropOldestSender {
            tx,
            evict: rx.clone(),
        },
        rx,
    )
}

/// Sending end of [`bounded`]. Clone it for each producer thread.
pub(crate) struct DropOldestSender<T> {
    tx: Sender<T>,
    /// A second receiver on the same queue, used only to evict. It also keeps
    /// the queue connected, so a send after the consumer is gone is a no-op
    /// that fills and then cycles the queue rather than an error.
    evict: Receiver<T>,
}

impl<T> Clone for DropOldestSender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            evict: self.evict.clone(),
        }
    }
}

impl<T> DropOldestSender<T> {
    /// Queue `msg`, dropping the oldest queued message if the queue is full.
    /// Never blocks.
    pub(crate) fn send(&self, mut msg: T) {
        loop {
            match self.tx.try_send(msg) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => return,
                Err(TrySendError::Full(m)) => {
                    // Another producer may refill the slot before the retry;
                    // then this evicts again. Each round removes one message,
                    // so the loop cannot outrun the queue's capacity forever.
                    let _ = self.evict.try_recv();
                    msg = m;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_queue_drops_the_oldest_and_keeps_the_newest() {
        let (tx, rx) = bounded(4);
        for i in 0..10 {
            tx.send(i);
        }
        let got: Vec<i32> = rx.try_iter().collect();
        assert_eq!(got, vec![6, 7, 8, 9]);
    }

    #[test]
    fn below_capacity_nothing_is_dropped() {
        let (tx, rx) = bounded(8);
        let tx2 = tx.clone();
        tx.send(1);
        tx2.send(2);
        tx.send(3);
        assert_eq!(rx.try_iter().collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    #[test]
    fn concurrent_producers_never_block_and_keep_their_newest() {
        let (tx, rx) = bounded(16);
        let handles: Vec<_> = (0..4)
            .map(|p| {
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for i in 0..1000 {
                        tx.send((p, i));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(rx.len(), 16);
        // Per producer, what survived is its newest messages, in order.
        let got: Vec<(i32, i32)> = rx.try_iter().collect();
        for p in 0..4 {
            let mine: Vec<i32> = got
                .iter()
                .filter(|(q, _)| *q == p)
                .map(|(_, i)| *i)
                .collect();
            let n = mine.len() as i32;
            assert_eq!(mine, (1000 - n..1000).collect::<Vec<_>>(), "producer {p}");
        }
    }
}
