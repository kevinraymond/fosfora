use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};

/// Byte layout of captured frame data, derived from the capture texture format.
/// Makes the "readback bytes are BGRA" assumption explicit instead of inheriting
/// whatever surface format the platform happened to pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameLayout {
    Bgra8,
    Rgba8,
}

impl FrameLayout {
    /// Map a capture texture format to its CPU byte layout.
    /// Errors on anything that isn't 4-byte BGRA/RGBA — better to refuse to
    /// stream than to silently emit channel-swapped video.
    pub fn from_texture_format(format: wgpu::TextureFormat) -> Result<Self, String> {
        use wgpu::TextureFormat as TF;
        match format {
            TF::Bgra8Unorm | TF::Bgra8UnormSrgb => Ok(Self::Bgra8),
            TF::Rgba8Unorm | TF::Rgba8UnormSrgb => Ok(Self::Rgba8),
            other => Err(format!("Unsupported capture texture format: {other:?}")),
        }
    }
}

/// Frame data sent from the render thread to a sink's sender thread.
#[derive(Debug)]
pub struct OutputFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub layout: FrameLayout,
}

/// Per-sink frame writer; runs on the sender thread.
///
/// Deliberately not `Send`: a sink is constructed *inside* its sender thread
/// (by the `make_sink` factory, which is what actually crosses threads) and
/// never leaves it, so sinks holding thread-confined handles (Spout's D3D11
/// device, Syphon's ObjC objects) need no unsafe Send claims.
pub trait FrameSink: 'static {
    fn write_frame(&mut self, frame: &OutputFrame) -> Result<(), String>;
}

/// Spawn a sink's sender thread.
///
/// The sink is constructed inside the thread (`make_sink`) so slow work — dylib
/// loading, SDK init — never blocks the render thread. Construction or write
/// failure is reported once through `error_tx` and ends the thread; the render
/// side surfaces it via `OutputPipeline::poll_health`.
pub fn spawn_sink_thread<S: FrameSink>(
    thread_name: &str,
    make_sink: impl FnOnce() -> Result<S, String> + Send + 'static,
    frame_rx: Receiver<OutputFrame>,
    shutdown: Arc<AtomicBool>,
    frame_counter: Arc<AtomicU64>,
    error_tx: Sender<String>,
) -> JoinHandle<()> {
    let name = thread_name.to_string();
    let thread_error_tx = error_tx.clone();
    std::thread::Builder::new()
        .name(name.clone())
        .spawn(move || {
            if let Err(e) = sink_loop(make_sink, &frame_rx, &shutdown, &frame_counter) {
                log::error!("{name} thread error: {e}");
                let _ = thread_error_tx.try_send(e);
            }
            log::info!("{name} thread exiting");
        })
        .unwrap_or_else(|e| {
            log::error!("Failed to spawn {thread_name} thread: {e}");
            let _ = error_tx.try_send(format!("Failed to spawn sender thread: {e}"));
            // Return a dummy handle that completes immediately.
            std::thread::Builder::new()
                .name(format!("{thread_name}-noop"))
                .spawn(|| {})
                .expect("failed to spawn noop thread")
        })
}

fn sink_loop<S: FrameSink>(
    make_sink: impl FnOnce() -> Result<S, String>,
    frame_rx: &Receiver<OutputFrame>,
    shutdown: &AtomicBool,
    frame_counter: &AtomicU64,
) -> Result<(), String> {
    let mut sink = make_sink()?;

    while !shutdown.load(Ordering::Relaxed) {
        match frame_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => {
                sink.write_frame(&frame)?;
                frame_counter.fetch_add(1, Ordering::Relaxed);
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_frame() -> OutputFrame {
        OutputFrame {
            data: vec![0u8; 16],
            width: 2,
            height: 2,
            layout: FrameLayout::Bgra8,
        }
    }

    /// Sink that counts writes and optionally fails on the Nth write.
    struct CountingSink {
        written: Arc<AtomicU64>,
        fail_on: Option<u64>,
    }

    impl FrameSink for CountingSink {
        fn write_frame(&mut self, _frame: &OutputFrame) -> Result<(), String> {
            let n = self.written.fetch_add(1, Ordering::Relaxed) + 1;
            if self.fail_on == Some(n) {
                return Err(format!("write {n} failed"));
            }
            Ok(())
        }
    }

    #[test]
    fn frame_layout_from_texture_format() {
        use wgpu::TextureFormat as TF;
        assert_eq!(
            FrameLayout::from_texture_format(TF::Bgra8UnormSrgb).unwrap(),
            FrameLayout::Bgra8
        );
        assert_eq!(
            FrameLayout::from_texture_format(TF::Bgra8Unorm).unwrap(),
            FrameLayout::Bgra8
        );
        assert_eq!(
            FrameLayout::from_texture_format(TF::Rgba8UnormSrgb).unwrap(),
            FrameLayout::Rgba8
        );
        assert!(FrameLayout::from_texture_format(TF::Rgba16Float).is_err());
    }

    #[test]
    fn factory_error_reaches_error_channel() {
        let (_frame_tx, frame_rx) = crossbeam_channel::bounded::<OutputFrame>(2);
        let (error_tx, error_rx) = crossbeam_channel::bounded(1);
        let handle = spawn_sink_thread::<CountingSink>(
            "test-sink",
            || Err("no device".to_string()),
            frame_rx,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(0)),
            error_tx,
        );
        let err = error_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("factory error should be reported");
        assert_eq!(err, "no device");
        handle.join().unwrap();
    }

    #[test]
    fn frames_are_written_and_counted() {
        let (frame_tx, frame_rx) = crossbeam_channel::bounded(4);
        let (error_tx, error_rx) = crossbeam_channel::bounded(1);
        let written = Arc::new(AtomicU64::new(0));
        let counter = Arc::new(AtomicU64::new(0));
        let sink_written = written.clone();
        let handle = spawn_sink_thread(
            "test-sink",
            move || {
                Ok(CountingSink {
                    written: sink_written,
                    fail_on: None,
                })
            },
            frame_rx,
            Arc::new(AtomicBool::new(false)),
            counter.clone(),
            error_tx,
        );
        for _ in 0..3 {
            frame_tx.send(test_frame()).unwrap();
        }
        drop(frame_tx); // disconnect ends the loop
        handle.join().unwrap();
        assert_eq!(written.load(Ordering::Relaxed), 3);
        assert_eq!(counter.load(Ordering::Relaxed), 3);
        assert!(error_rx.try_recv().is_err(), "no error expected");
    }

    #[test]
    fn write_error_reaches_error_channel_and_ends_thread() {
        let (frame_tx, frame_rx) = crossbeam_channel::bounded(4);
        let (error_tx, error_rx) = crossbeam_channel::bounded(1);
        let written = Arc::new(AtomicU64::new(0));
        let counter = Arc::new(AtomicU64::new(0));
        let sink_written = written.clone();
        let handle = spawn_sink_thread(
            "test-sink",
            move || {
                Ok(CountingSink {
                    written: sink_written,
                    fail_on: Some(2),
                })
            },
            frame_rx,
            Arc::new(AtomicBool::new(false)),
            counter.clone(),
            error_tx,
        );
        for _ in 0..3 {
            let _ = frame_tx.send(test_frame());
        }
        let err = error_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("write error should be reported");
        assert_eq!(err, "write 2 failed");
        drop(frame_tx);
        handle.join().unwrap();
        // First write succeeded and was counted; the failing second was not.
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }
}
