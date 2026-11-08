//! In-process producers and readers of the web remote's vocabulary.
//!
//! The embedded REPL (`repl` feature) issues the same [`WsInMessage`]s a
//! WebSocket client does, through a queue that outlives server restarts, and
//! reads the same state and audio snapshots the remote is sent.

use super::WebSystem;
use super::types::WsInMessage;

impl WebSystem {
    /// Start the embedded nREPL on `port` (0 = any free port).
    #[cfg(feature = "repl")]
    pub fn start_repl(&mut self, port: u16) {
        self.repl = None;
        let bridge = crate::repl::Bridge {
            commands: self.local_tx.clone(),
            state: self.latest_state.clone(),
            audio: self.latest_audio.clone(),
        };
        match crate::repl::start(port, bridge) {
            Ok(repl) => self.repl = Some(repl),
            Err(e) => log::error!("Failed to start embedded nREPL: {e:#}"),
        }
    }

    /// Port of the embedded nREPL, when one is running.
    pub fn repl_port(&self) -> Option<u16> {
        #[cfg(feature = "repl")]
        return self.repl.as_ref().map(crate::repl::Repl::port);
        #[cfg(not(feature = "repl"))]
        None
    }

    /// Whether an in-process reader wants the snapshots kept fresh even with
    /// no WebSocket client connected.
    pub(super) fn local_reader(&self) -> bool {
        self.repl_port().is_some()
    }

    /// Everything queued this frame: network clients first, then in-process.
    pub(super) fn drain_inbound(&self) -> Vec<WsInMessage> {
        let mut messages: Vec<WsInMessage> = self
            .inbound_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        messages.extend(self.local_rx.try_iter());
        messages
    }

    /// Keep the latest audio snapshot for in-process readers.
    pub(super) fn store_audio(&self, json: &str) {
        if let Ok(mut audio) = self.latest_audio.lock() {
            json.clone_into(&mut audio);
        }
    }
}
