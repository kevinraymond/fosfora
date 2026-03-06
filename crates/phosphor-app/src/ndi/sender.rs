use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::Receiver;

use super::ffi::NdiSender;

/// Frame data sent from the render thread to the NDI sender thread.
pub struct NdiFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Spawn the NDI sender thread.
/// Receives frames via crossbeam channel and sends them over NDI.
pub fn spawn_sender_thread(
    source_name: String,
    frame_rx: Receiver<NdiFrame>,
    shutdown: Arc<AtomicBool>,
    frame_counter: Arc<AtomicU64>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("ndi-sender".into())
        .spawn(move || {
            if let Err(e) = sender_loop(&source_name, &frame_rx, &shutdown, &frame_counter) {
                log::error!("NDI sender thread error: {e}");
            }
            log::info!("NDI sender thread exiting");
        })
        .unwrap_or_else(|e| {
            log::error!("Failed to spawn NDI sender thread: {e}");
            // Return a dummy handle that completes immediately.
            std::thread::Builder::new()
                .name("ndi-sender-noop".into())
                .spawn(|| {})
                .unwrap()
        })
}

fn sender_loop(
    source_name: &str,
    frame_rx: &Receiver<NdiFrame>,
    shutdown: &AtomicBool,
    frame_counter: &AtomicU64,
) -> Result<(), String> {
    let sender = NdiSender::new(source_name)?;

    while !shutdown.load(Ordering::Relaxed) {
        match frame_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => {
                sender.send_video(&frame.data, frame.width, frame.height);
                frame_counter.fetch_add(1, Ordering::Relaxed);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}
