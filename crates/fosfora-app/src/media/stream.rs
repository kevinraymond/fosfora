use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};

use super::webcam::WebcamFrame;
use crate::settings::RtmpStream;

/// The frame size a stream is taken to have until its first frame says.
const ASSUMED_SIZE: (u32, u32) = (1280, 720);
/// Frames wider than this are scaled down by ffmpeg: every frame crosses a
/// pipe as raw RGBA, and a 4K stream would be a gigabyte a second.
const MAX_WIDTH: u32 = 1920;
/// The largest frame side a header may claim.
const MAX_SIDE: u32 = 8192;
/// How long a stream that could not be opened waits before it is tried
/// again.
const RETRY: Duration = Duration::from_secs(2);
/// The same for a stream that ran and ended: short, so a sender that
/// reconnects at once finds it listening.
const REOPEN: Duration = Duration::from_millis(200);

/// What a stream's status light shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamLight {
    /// Red: not listening and not connected.
    Down,
    /// Yellow: listening for the sender, or connecting to the server.
    Waiting,
    /// Green: the sender is connected.
    Connected,
}

/// A network stream (RTMP, or anything else ffmpeg opens by URL) read as a
/// camera.
///
/// Unlike a camera it is allowed not to be there: starting never waits for
/// the sender, and a stream that ends is opened again until capture stops,
/// so a preset can be loaded before the stream is live.
pub struct StreamCapture {
    frame_rx: Receiver<WebcamFrame>,
    shutdown: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The running ffmpeg, kept so stopping can end it: the capture thread
    /// sits in a read that only returns when ffmpeg writes or exits.
    child: Arc<Mutex<Option<Child>>>,
    size: Arc<(AtomicU32, AtomicU32)>,
    /// Where the stream stands, in words for the settings page.
    status: Arc<Mutex<(StreamLight, String)>>,
    pub device_name: String,
    /// What it was started with, to tell when the settings have moved on.
    pub config: RtmpStream,
}

impl StreamCapture {
    pub fn start(config: &RtmpStream) -> Result<Self, String> {
        if !super::webcam_ffmpeg::ffmpeg_available() {
            return Err(
                "FFmpeg not found. Install FFmpeg and ensure it is in your PATH.".to_string(),
            );
        }
        let (frame_tx, frame_rx) = crossbeam_channel::bounded(2);
        let shutdown = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let size = Arc::new((
            AtomicU32::new(ASSUMED_SIZE.0),
            AtomicU32::new(ASSUMED_SIZE.1),
        ));

        let status = Arc::new(Mutex::new((StreamLight::Down, String::new())));

        let thread = {
            let (config, shutdown, child, size, status) = (
                config.clone(),
                shutdown.clone(),
                child.clone(),
                size.clone(),
                status.clone(),
            );
            std::thread::Builder::new()
                .name("stream-capture".into())
                .spawn(move || {
                    capture_thread(&config, &frame_tx, &shutdown, &child, &size, &status);
                })
                .map_err(|e| format!("Failed to spawn stream capture thread: {e}"))?
        };

        Ok(Self {
            frame_rx,
            shutdown,
            thread: Some(thread),
            child,
            size,
            status,
            device_name: config.name.clone(),
            config: config.clone(),
        })
    }

    /// Non-blocking read of the latest frame.
    pub fn try_recv_frame(&self) -> Option<WebcamFrame> {
        let mut latest = None;
        while let Ok(frame) = self.frame_rx.try_recv() {
            latest = Some(frame);
        }
        latest
    }

    /// The size of the last frame, or the assumed size before the first.
    pub fn resolution(&self) -> (u32, u32) {
        (
            self.size.0.load(Ordering::Relaxed),
            self.size.1.load(Ordering::Relaxed),
        )
    }

    /// Where the stream stands: waiting, live, or why it is not.
    pub fn status(&self) -> (StreamLight, String) {
        self.status
            .lock()
            .map_or((StreamLight::Down, String::new()), |s| s.clone())
    }

    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        end_child(&self.child);
        if let Some(handle) = self.thread.take() {
            if super::webcam::wait_finished(&handle, Duration::from_millis(500)) {
                let _ = handle.join();
            }
        }
    }

    pub fn is_running(&self) -> bool {
        !self.shutdown.load(Ordering::Relaxed)
            && self.thread.as_ref().is_some_and(|h| !h.is_finished())
    }
}

impl Drop for StreamCapture {
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

/// The ffmpeg arguments that read `config` and write its frames to stdout
/// as PAM images. PAM rather than raw video because each image states its
/// own size: a stream's size is not known before it starts and can change
/// when the sender reconnects.
fn ffmpeg_args(config: &RtmpStream) -> Vec<String> {
    // No `-fflags nobuffer`: it discards the packets read while the stream
    // is identified, and with them the keyframe everything after depends on.
    // At `info` ffmpeg says when the input has opened, which tells a sender
    // that never connected from one that connected and sent no picture.
    let mut args: Vec<String> = ["-hide_banner", "-nostats", "-loglevel", "info"]
        .map(String::from)
        .into();
    if config.listen {
        args.extend(["-listen", "1"].map(String::from));
    } else {
        // A server that stops answering ends ffmpeg, so it is tried again.
        // Not while listening: no sender yet is not a failure.
        args.extend(["-rw_timeout", "10000000"].map(String::from));
    }
    args.extend(["-i".to_string(), config.url.trim().to_string()]);
    args.extend(
        [
            "-an",
            "-vf",
            &format!("scale='min({MAX_WIDTH},iw)':-2"),
            "-f",
            "image2pipe",
            "-c:v",
            "pam",
            "-pix_fmt",
            "rgba",
            "pipe:1",
        ]
        .map(String::from),
    );
    args
}

/// Read one PAM header, up to and including `ENDHDR`. `Ok(None)` at a clean
/// end of the stream.
fn read_pam_header(reader: &mut impl BufRead) -> Result<Option<(u32, u32)>, String> {
    let (mut width, mut height, mut depth) = (0, 0, 0);
    let mut line = String::new();
    for n in 0..16 {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) if n == 0 => return Ok(None),
            Ok(0) => return Err("stream ended inside a frame header".into()),
            Ok(_) => {}
            Err(e) => return Err(format!("read error: {e}")),
        }
        let mut words = line.split_whitespace();
        let (key, value) = (words.next().unwrap_or_default(), words.next());
        let number = || value.and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        match key {
            "P7" if n == 0 => {}
            _ if n == 0 => return Err("not a PAM image".into()),
            "WIDTH" => width = number(),
            "HEIGHT" => height = number(),
            "DEPTH" => depth = number(),
            "ENDHDR" => {
                let sane = |side| (1..=MAX_SIDE).contains(&side);
                return if depth == 4 && sane(width) && sane(height) {
                    Ok(Some((width, height)))
                } else {
                    Err(format!("unusable frame: {width}x{height}, depth {depth}"))
                };
            }
            _ => {}
        }
    }
    Err("frame header has no end".into())
}

/// Sleep for `time`, waking early when capture is stopped. False if it was.
fn pause(time: Duration, shutdown: &AtomicBool) -> bool {
    let until = std::time::Instant::now() + time;
    while std::time::Instant::now() < until {
        if shutdown.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !shutdown.load(Ordering::Relaxed)
}

fn capture_thread(
    config: &RtmpStream,
    frame_tx: &Sender<WebcamFrame>,
    shutdown: &AtomicBool,
    child_slot: &Mutex<Option<Child>>,
    size: &(AtomicU32, AtomicU32),
    status: &Arc<Mutex<(StreamLight, String)>>,
) {
    let name = &config.name;
    let say = |light: StreamLight, words: String| {
        if let Ok(mut status) = status.lock() {
            *status = (light, words);
        }
    };
    // Said once per outage, not once per attempt.
    let mut waiting = false;
    while !shutdown.load(Ordering::Relaxed) {
        let mut child = match Command::new("ffmpeg")
            .args(ffmpeg_args(config))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                log::error!("Stream '{name}': failed to start ffmpeg: {e}");
                return;
            }
        };
        if !waiting {
            let words = if config.listen {
                "Listening, waiting for the sender to connect"
            } else {
                "Connecting"
            };
            say(StreamLight::Waiting, words.into());
        }
        let stdout = child.stdout.take();
        // Read as it comes: a stream with damaged packets says so on every
        // one, and a full pipe would stall ffmpeg.
        let last_error = Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = child.stderr.take() {
            let (last_error, status, name) = (last_error.clone(), status.clone(), name.clone());
            // An outage is explained once, not on every attempt.
            let quiet = waiting;
            std::thread::spawn(move || {
                let lines = BufReader::new(stderr).lines().map_while(Result::ok);
                for (n, line) in lines.enumerate() {
                    // What ffmpeg says about the stream, without letting a
                    // damaged one fill the log.
                    if n < 40 && !quiet {
                        log::debug!("Stream '{name}' ffmpeg: {line}");
                    }
                    if line.starts_with("Input #0") {
                        if let Ok(mut status) = status.lock() {
                            *status = (
                                StreamLight::Connected,
                                "Sender connected, waiting for its picture".into(),
                            );
                        }
                    }
                    if let Ok(mut last) = last_error.lock() {
                        *last = line;
                    }
                }
            });
        }
        // Handed over so `stop` can end it while this thread is in a read.
        if let Ok(mut slot) = child_slot.lock() {
            *slot = Some(child);
        }
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let mut frames = 0u64;
        if let Some(stdout) = stdout {
            let mut reader = BufReader::with_capacity(1 << 20, stdout);
            loop {
                let (width, height) = match read_pam_header(&mut reader) {
                    Ok(Some(size)) => size,
                    Ok(None) => break,
                    Err(e) => {
                        log::warn!("Stream '{name}': {e}");
                        break;
                    }
                };
                let mut data = vec![0u8; (width as usize) * (height as usize) * 4];
                if reader.read_exact(&mut data).is_err() {
                    break;
                }
                if frames == 0 {
                    log::info!("Stream '{name}' is live: {width}x{height}");
                    waiting = false;
                }
                let known = (
                    size.0.load(Ordering::Relaxed),
                    size.1.load(Ordering::Relaxed),
                );
                if frames == 0 || (width, height) != known {
                    say(StreamLight::Connected, format!("Live, {width}x{height}"));
                }
                frames += 1;
                size.0.store(width, Ordering::Relaxed);
                size.1.store(height, Ordering::Relaxed);
                // try_send: drop the frame if the consumer is behind
                let _ = frame_tx.try_send(WebcamFrame {
                    data,
                    width,
                    height,
                });
            }
        }
        end_child(child_slot);
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        if frames > 0 {
            log::warn!("Stream '{name}' ended; waiting for it to come back");
            say(
                StreamLight::Waiting,
                "Ended, waiting for it to come back".into(),
            );
            waiting = true;
        } else {
            let reason = last_error.lock().map(|l| l.clone()).unwrap_or_default();
            if !waiting {
                log::warn!("Stream '{name}' is not available yet ({reason}); retrying");
            }
            say(
                StreamLight::Down,
                format!("Not available, retrying: {reason}"),
            );
            waiting = true;
        }
        if !pause(if frames > 0 { REOPEN } else { RETRY }, shutdown) {
            break;
        }
    }
    end_child(child_slot);
    log::info!("Stream capture stopped: '{name}'");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(listen: bool) -> RtmpStream {
        RtmpStream {
            name: "Stage".into(),
            url: " rtmp://127.0.0.1/live/stage ".into(),
            enabled: true,
            listen,
        }
    }

    #[test]
    fn reads_the_size_each_frame_states() {
        let bytes = b"P7\nWIDTH 320\nHEIGHT 240\nDEPTH 4\nMAXVAL 255\n\
                      TUPLTYPE RGB_ALPHA\nENDHDR\nrest";
        let mut reader = &bytes[..];
        assert_eq!(read_pam_header(&mut reader), Ok(Some((320, 240))));
        // Left at the first byte of pixel data.
        assert_eq!(reader, b"rest");
        assert_eq!(read_pam_header(&mut &b""[..]), Ok(None));
    }

    /// A header is trusted for an allocation, so one that is not RGBA or
    /// claims an absurd size ends the read instead.
    #[test]
    fn refuses_headers_it_cannot_use() {
        for bad in [
            &b"P6\n320 240\n255\n"[..],
            b"P7\nWIDTH 320\nHEIGHT 240\nDEPTH 3\nENDHDR\n",
            b"P7\nWIDTH 0\nHEIGHT 240\nDEPTH 4\nENDHDR\n",
            b"P7\nWIDTH 99999\nHEIGHT 99999\nDEPTH 4\nENDHDR\n",
            b"P7\nWIDTH 320\n",
        ] {
            assert!(read_pam_header(&mut &bad[..]).is_err());
        }
    }

    /// The whole path: a sender publishes to a listening stream, twice, and
    /// frames of the sender's size arrive both times.
    #[test]
    #[ignore = "requires ffmpeg and a free local port"]
    fn receives_a_published_stream_and_its_return() {
        let url = "rtmp://127.0.0.1:19355/live/test";
        let mut config = stream(true);
        config.url = url.into();
        let capture = StreamCapture::start(&config).unwrap();
        assert_eq!(capture.resolution(), ASSUMED_SIZE);

        for size in ["320x240", "640x360"] {
            std::thread::sleep(Duration::from_secs(1));
            let source = format!("testsrc=size={size}:rate=15");
            let mut sender = Command::new("ffmpeg")
                .args(["-loglevel", "error", "-re", "-f", "lavfi", "-i", &source])
                .args(["-t", "4", "-c:v", "flv", "-f", "flv", url])
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            let frame = loop {
                if let Some(frame) = capture.try_recv_frame() {
                    break frame;
                }
                assert!(std::time::Instant::now() < deadline, "no frame at {size}");
                std::thread::sleep(Duration::from_millis(20));
            };
            let (w, h) = size.split_once('x').unwrap();
            assert_eq!(
                (
                    frame.width.to_string().as_str(),
                    frame.height.to_string().as_str()
                ),
                (w, h)
            );
            assert_eq!(frame.data.len(), (frame.width * frame.height * 4) as usize);
            assert!(capture.is_running());
            let _ = sender.wait();
            // Frames of this sender still in the channel.
            std::thread::sleep(Duration::from_millis(500));
            while capture.try_recv_frame().is_some() {}
        }
    }

    #[test]
    fn a_listening_stream_waits_for_its_sender() {
        let connect = ffmpeg_args(&stream(false));
        assert!(connect.contains(&"-rw_timeout".to_string()));
        assert!(!connect.contains(&"-listen".to_string()));
        let url_at = connect.iter().position(|a| a == "-i").unwrap() + 1;
        assert_eq!(connect[url_at], "rtmp://127.0.0.1/live/stage");

        let listen = ffmpeg_args(&stream(true));
        assert!(listen.contains(&"-listen".to_string()));
        assert!(!listen.contains(&"-rw_timeout".to_string()));
    }
}
