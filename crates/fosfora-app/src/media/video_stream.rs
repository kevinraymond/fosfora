//! A long video decoded as it plays, rather than every frame up front (#3094).
//!
//! One ffmpeg runs on a thread from a start time, at the clip's frame rate
//! forced constant, writing raw RGBA into a pipe. The thread hands frames
//! over a small bounded channel, the ring: it blocks while the ring is full,
//! so a paused layer costs nothing and no frame is ever dropped. The main
//! thread owns the clock ([`super::MediaLayer`] advances it) and pops the
//! frames that are due through [`VideoStream::frame_due`].
//!
//! A seek ends ffmpeg and starts it again at the new time under a new
//! `epoch`; frames of an older epoch are discarded as they come. At the end
//! of the clip a looping stream starts again at 0 on the thread while the
//! ring still holds the last frames, so the wrap costs the picture nothing;
//! a stream that does not loop waits for a seek.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, SendTimeoutError, Sender, bounded, unbounded};

use super::types::DecodedFrame;
use super::video::VideoMeta;

/// Frames decoded ahead of the picture (the ring). Six at 1080p is ~50 MB.
pub const RING_FRAMES: usize = 6;

/// How long a seek waits for the next one before ffmpeg restarts: a scrubber
/// seeks on every tick of a drag.
const SEEK_SETTLE: Duration = Duration::from_millis(80);

/// How long opening waits for the first frame before the layer opens black.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(3);

/// How long a blocked hand-over waits before looking for commands again.
const SEND_SLICE: Duration = Duration::from_millis(50);

/// One decoded frame with where it belongs.
pub struct StreamFrame {
    /// The seek it was decoded for.
    pub epoch: u32,
    /// Frames since that seek, counting on through loop wraps: what the
    /// clock is compared with.
    pub seq: u64,
    /// Frame index within the clip (`index / fps` is its time): what the
    /// position readout shows.
    pub index: u64,
    pub frame: DecodedFrame,
}

enum Cmd {
    Seek(f64),
    Looping(bool),
    Quit,
}

/// A video file played through ffmpeg on its own thread. Dropping it stops
/// the thread and ffmpeg.
pub struct VideoStream {
    pub meta: VideoMeta,
    pub path: PathBuf,
    rx: Receiver<StreamFrame>,
    cmd_tx: Sender<Cmd>,
    epoch: Arc<AtomicU32>,
    shutdown: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    error: Arc<Mutex<Option<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The next frame, popped but not yet due.
    held: Option<StreamFrame>,
    looping: bool,
}

impl VideoStream {
    /// Start decoding `path` from the beginning, looping, and wait briefly
    /// for the first frame so the layer opens with a picture.
    pub fn open(path: &Path, meta: VideoMeta) -> Result<Self, String> {
        Self::open_at(path, meta, 0.0, true)
    }

    /// [`Self::open`] from `start_secs`, looping or not.
    pub fn open_at(
        path: &Path,
        meta: VideoMeta,
        start_secs: f64,
        looping: bool,
    ) -> Result<Self, String> {
        if meta.width == 0 || meta.height == 0 || meta.fps <= 0.0 || meta.fps.is_nan() {
            return Err(format!(
                "Video has no usable picture ({}x{} at {} fps)",
                meta.width, meta.height, meta.fps
            ));
        }
        let (tx, rx) = bounded(RING_FRAMES);
        let (cmd_tx, cmd_rx) = unbounded();
        let epoch = Arc::new(AtomicU32::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let error = Arc::new(Mutex::new(None));
        let job = Job {
            path: path.to_path_buf(),
            meta: meta.clone(),
            tx,
            cmd_rx,
            epoch: epoch.clone(),
            shutdown: shutdown.clone(),
            child: child.clone(),
            error: error.clone(),
            start: start_secs.max(0.0),
            looping,
        };
        let thread = std::thread::Builder::new()
            .name("video-stream".into())
            .spawn(move || decode_thread(job))
            .map_err(|e| format!("Failed to spawn the video thread: {e}"))?;
        let mut stream = Self {
            meta,
            path: path.to_path_buf(),
            rx,
            cmd_tx,
            epoch,
            shutdown,
            child,
            error,
            thread: Some(thread),
            held: None,
            looping,
        };
        let deadline = Instant::now() + FIRST_FRAME_WAIT;
        while stream.held.is_none() && Instant::now() < deadline {
            match stream.rx.recv_timeout(Duration::from_millis(20)) {
                Ok(frame) => stream.held = Some(frame),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        if stream.held.is_none() {
            if let Some(e) = stream.error.lock().ok().and_then(|e| e.clone()) {
                return Err(e);
            }
        }
        Ok(stream)
    }

    /// The first frame, for the layer's first upload. `None` when it has not
    /// arrived yet.
    pub fn peek(&self) -> Option<&DecodedFrame> {
        self.held.as_ref().map(|f| &f.frame)
    }

    /// Frames in the clip at its rate.
    pub fn frame_count(&self) -> usize {
        (self.meta.duration_secs * self.meta.fps).ceil().max(1.0) as usize
    }

    /// The latest frame due at `clock_seq` frames since the last seek, if a
    /// new one has arrived. Due frames older than it are skipped (the clock
    /// got ahead), a frame not yet due is held for the next call, and frames
    /// from before the last seek are discarded.
    pub fn frame_due(&mut self, clock_seq: u64) -> Option<StreamFrame> {
        let epoch = self.epoch.load(Ordering::Relaxed);
        let mut due = None;
        loop {
            let next = match self.held.take() {
                Some(f) => f,
                None => match self.rx.try_recv() {
                    Ok(f) => f,
                    Err(_) => break,
                },
            };
            if next.epoch != epoch {
                continue;
            }
            if next.seq > clock_seq {
                self.held = Some(next);
                break;
            }
            due = Some(next);
        }
        due
    }

    /// Start again from `secs`. Frames already decoded are discarded as they
    /// arrive, and the caller's clock starts again at 0.
    pub fn seek(&mut self, secs: f64) {
        self.epoch.fetch_add(1, Ordering::Relaxed);
        self.held = None;
        let _ = self.cmd_tx.send(Cmd::Seek(secs.max(0.0)));
    }

    pub fn looping(&self) -> bool {
        self.looping
    }

    /// Whether the thread starts the clip again at its end.
    pub fn set_looping(&mut self, looping: bool) {
        if looping != self.looping {
            self.looping = looping;
            let _ = self.cmd_tx.send(Cmd::Looping(looping));
        }
    }

    /// The decode thread's error, if it has given up.
    pub fn error(&self) -> Option<String> {
        self.error.lock().ok().and_then(|e| e.clone())
    }

    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.cmd_tx.send(Cmd::Quit);
        end_child(&self.child);
        if let Some(handle) = self.thread.take() {
            let deadline = Instant::now() + Duration::from_millis(500);
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for VideoStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn end_child(slot: &Mutex<Option<Child>>) {
    if let Some(mut child) = slot.lock().ok().and_then(|mut c| c.take()) {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// What the decode thread works from.
struct Job {
    path: PathBuf,
    meta: VideoMeta,
    tx: Sender<StreamFrame>,
    cmd_rx: Receiver<Cmd>,
    epoch: Arc<AtomicU32>,
    shutdown: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    error: Arc<Mutex<Option<String>>>,
    start: f64,
    looping: bool,
}

/// What a batch of commands asks the thread to do next.
enum Flow {
    Continue,
    Seek(f64),
    Quit,
}

/// Drain every command waiting, keeping the last seek.
fn take_cmds(cmd_rx: &Receiver<Cmd>, looping: &mut bool) -> Flow {
    let mut flow = Flow::Continue;
    while let Ok(cmd) = cmd_rx.try_recv() {
        match cmd {
            Cmd::Quit => return Flow::Quit,
            Cmd::Seek(t) => flow = Flow::Seek(t),
            Cmd::Looping(b) => *looping = b,
        }
    }
    flow
}

/// After a seek, wait a moment for the seeks a drag sends behind it, so
/// ffmpeg restarts once at the last one.
fn settle_seek(cmd_rx: &Receiver<Cmd>, looping: &mut bool, mut secs: f64) -> Flow {
    let deadline = Instant::now() + SEEK_SETTLE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Flow::Seek(secs);
        }
        match cmd_rx.recv_timeout(left) {
            Ok(Cmd::Quit) | Err(RecvTimeoutError::Disconnected) => return Flow::Quit,
            Ok(Cmd::Seek(t)) => secs = t,
            Ok(Cmd::Looping(b)) => *looping = b,
            Err(RecvTimeoutError::Timeout) => return Flow::Seek(secs),
        }
    }
}

/// ffmpeg from `start`, raw RGBA at the clip's size and a constant rate, so
/// `start + n / fps` is frame `n`'s time exactly.
fn spawn_ffmpeg(path: &Path, meta: &VideoMeta, start: f64) -> Result<Child, String> {
    Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-ss",
            &format!("{start:.3}"),
            "-i",
        ])
        .arg(path)
        .args([
            "-an",
            "-vf",
            &format!("fps={}", meta.fps),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &format!("{}x{}", meta.width, meta.height),
            "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg: {e}"))
}

fn decode_thread(job: Job) {
    let Job {
        path,
        meta,
        tx,
        cmd_rx,
        epoch,
        shutdown,
        child: child_slot,
        error,
        mut start,
        mut looping,
    } = job;
    let frame_size = meta.width as usize * meta.height as usize * 4;
    let mut seq: u64 = 0;
    let mut buf = vec![0u8; frame_size];

    'run: while !shutdown.load(Ordering::Relaxed) {
        let current_epoch = epoch.load(Ordering::Relaxed);
        let mut child = match spawn_ffmpeg(&path, &meta, start) {
            Ok(c) => c,
            Err(e) => {
                if let Ok(mut slot) = error.lock() {
                    *slot = Some(e);
                }
                return;
            }
        };
        let Some(mut stdout) = child.stdout.take() else {
            if let Ok(mut slot) = error.lock() {
                *slot = Some("ffmpeg: no stdout pipe".into());
            }
            return;
        };
        if let Ok(mut slot) = child_slot.lock() {
            *slot = Some(child);
        }
        let mut index = (start * meta.fps).round() as u64;

        // Read frames until the clip ends, a seek asks for another start,
        // or the stream is stopped.
        let flow = loop {
            match take_cmds(&cmd_rx, &mut looping) {
                Flow::Continue => {}
                other => break other,
            }
            if stdout.read_exact(&mut buf).is_err() {
                break Flow::Continue; // the clip ended, or ffmpeg was ended
            }
            let mut frame = StreamFrame {
                epoch: current_epoch,
                seq,
                index,
                frame: DecodedFrame {
                    data: buf.clone(),
                    width: meta.width,
                    height: meta.height,
                },
            };
            seq += 1;
            index += 1;
            // Hand it over; while the ring is full keep listening, so a
            // paused layer can still seek or stop.
            let handed = loop {
                match tx.send_timeout(frame, SEND_SLICE) {
                    Ok(()) => break Flow::Continue,
                    Err(SendTimeoutError::Disconnected(_)) => break Flow::Quit,
                    Err(SendTimeoutError::Timeout(f)) => {
                        if shutdown.load(Ordering::Relaxed) {
                            break Flow::Quit;
                        }
                        match take_cmds(&cmd_rx, &mut looping) {
                            Flow::Continue => frame = f,
                            other => break other, // a seek drops this frame
                        }
                    }
                }
            };
            match handed {
                Flow::Continue => {}
                other => break other,
            }
        };
        drop(stdout);
        end_child(&child_slot);
        if shutdown.load(Ordering::Relaxed) {
            return;
        }

        match flow {
            Flow::Quit => return,
            Flow::Seek(t) => match settle_seek(&cmd_rx, &mut looping, t) {
                Flow::Seek(t) => {
                    start = t;
                    seq = 0;
                }
                _ => return,
            },
            Flow::Continue if looping => {
                // The clip ended: start it again, the clock counting on.
                start = 0.0;
            }
            Flow::Continue => {
                // The clip ended. Wait for a seek, or for looping to be
                // switched on.
                loop {
                    match cmd_rx.recv() {
                        Ok(Cmd::Quit) | Err(_) => return,
                        Ok(Cmd::Seek(t)) => match settle_seek(&cmd_rx, &mut looping, t) {
                            Flow::Seek(t) => {
                                start = t;
                                seq = 0;
                                continue 'run;
                            }
                            _ => return,
                        },
                        Ok(Cmd::Looping(true)) => {
                            looping = true;
                            start = 0.0;
                            continue 'run;
                        }
                        Ok(Cmd::Looping(false)) => looping = false,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `secs` long, 10 fps test clip, or None where ffmpeg is missing.
    pub(crate) fn clip(dir: &Path, secs: u32) -> Option<PathBuf> {
        if !super::super::video::ffmpeg_available() {
            return None;
        }
        let path = dir.join(format!("clip{secs}.mp4"));
        let ok = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!("testsrc=size=64x36:rate=10:duration={secs}"))
            .args(["-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        ok.then_some(path)
    }

    fn open(dir: &Path, looping: bool) -> Option<VideoStream> {
        let path = clip(dir, 2)?;
        let meta = super::super::video::probe_video(&path).expect("probe");
        Some(VideoStream::open_at(&path, meta, 0.0, looping).expect("opens"))
    }

    /// Wait for the frame due at `clock`, the way the frame loop polls.
    fn due(s: &mut VideoStream, clock: u64) -> Option<StreamFrame> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(f) = s.frame_due(clock) {
                return Some(f);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }

    /// Tick the clock up to `clock` the way the app does, one frame at a
    /// time, and return the frame shown at the end.
    fn run_to(s: &mut VideoStream, clock: u64) -> StreamFrame {
        let mut last = None;
        for c in 0..=clock {
            if let Some(f) = due(s, c) {
                last = Some(f);
            }
        }
        last.expect("a frame was shown")
    }

    #[test]
    fn opens_with_a_first_frame_and_the_clip_length() {
        let dir = tempfile::tempdir().unwrap();
        let Some(s) = open(dir.path(), true) else {
            eprintln!("ffmpeg not found; skipped");
            return;
        };
        let first = s.peek().expect("first frame waited for");
        assert_eq!((first.width, first.height), (64, 36));
        assert_eq!(first.data.len(), 64 * 36 * 4);
        assert_eq!(s.frame_count(), 20);
    }

    #[test]
    fn frames_arrive_in_clock_order_with_their_clip_index() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut s) = open(dir.path(), true) else {
            eprintln!("ffmpeg not found; skipped");
            return;
        };
        let indices: Vec<u64> = (0..20u64)
            .map(|clock| due(&mut s, clock).expect("a frame is due").index)
            .collect();
        assert_eq!(indices, (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn a_clock_that_jumps_ahead_gets_the_latest_due_frame_only() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut s) = open(dir.path(), true) else {
            eprintln!("ffmpeg not found; skipped");
            return;
        };
        // Let the ring fill past frame 5, then ask for it in one go.
        std::thread::sleep(Duration::from_millis(300));
        let f = due(&mut s, 5).expect("frame 5 is due");
        assert_eq!(f.index, 5);
        assert!(s.frame_due(5).is_none(), "nothing newer is due yet");
        assert_eq!(due(&mut s, 6).expect("frame 6").index, 6);
    }

    #[test]
    fn a_seek_starts_a_new_epoch_at_the_time_asked() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut s) = open(dir.path(), true) else {
            eprintln!("ffmpeg not found; skipped");
            return;
        };
        for clock in 0..3 {
            due(&mut s, clock).expect("warm");
        }
        s.seek(1.0);
        let f = due(&mut s, 0).expect("the new epoch's first frame");
        assert_eq!(f.seq, 0);
        assert!(
            (9..=11).contains(&f.index),
            "index {} is not ~1.0 s",
            f.index
        );
        assert_eq!(due(&mut s, 1).expect("next").index, f.index + 1);
    }

    #[test]
    fn a_looping_stream_wraps_to_the_start_without_a_new_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut s) = open(dir.path(), true) else {
            eprintln!("ffmpeg not found; skipped");
            return;
        };
        let last = run_to(&mut s, 19);
        assert_eq!(last.index, 19);
        let wrapped = due(&mut s, 20).expect("the wrap");
        assert_eq!((wrapped.epoch, wrapped.seq, wrapped.index), (0, 20, 0));
    }

    #[test]
    fn a_stream_that_does_not_loop_stops_at_the_end_until_a_seek() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut s) = open(dir.path(), false) else {
            eprintln!("ffmpeg not found; skipped");
            return;
        };
        assert_eq!(run_to(&mut s, 19).index, 19);
        std::thread::sleep(Duration::from_millis(300));
        assert!(s.frame_due(40).is_none(), "nothing after the end");
        s.seek(0.0);
        assert_eq!(due(&mut s, 0).expect("after the seek").index, 0);
    }

    #[test]
    fn switching_looping_on_at_the_end_starts_the_clip_again() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut s) = open(dir.path(), false) else {
            eprintln!("ffmpeg not found; skipped");
            return;
        };
        assert_eq!(run_to(&mut s, 19).index, 19);
        s.set_looping(true);
        let f = due(&mut s, 20).expect("the clip again");
        assert_eq!(f.index, 0);
    }

    #[test]
    fn dropping_the_stream_ends_its_thread() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut s) = open(dir.path(), true) else {
            eprintln!("ffmpeg not found; skipped");
            return;
        };
        let child = s.child.clone();
        s.stop();
        assert!(s.thread.is_none());
        assert!(child.lock().unwrap().is_none(), "ffmpeg was ended");
        drop(s);
    }
}
