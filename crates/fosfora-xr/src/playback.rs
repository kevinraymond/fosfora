//! Test-track playback on the headset (S6): a decoded stereo clip looped
//! through cpal's AAudio output, with every played frame also pushed into a
//! tap ring so the analysis chain can follow what the speakers play rather
//! than what the microphones pick up.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use fosfora_app::audio::capture::RingBuffer;
use log::{error, info};

/// A clip in memory: interleaved stereo f32 at `sample_rate`.
pub struct Clip {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl Clip {
    /// Decode a song file with the core's decoder (`decode` feature:
    /// symphonia; OGG Vorbis for the bundled track). Stereo interleaved at
    /// the file's own rate.
    pub fn decode(path: &std::path::Path) -> Result<Self> {
        let audio = fosfora_app::decode::decode_file(path)
            .with_context(|| format!("decoding {}", path.display()))?;
        info!(
            "playback: {} · {:.1} s at {} Hz ({} source channels)",
            path.display(),
            audio.interleaved.len() as f32 / 2.0 / audio.sample_rate,
            audio.sample_rate,
            audio.source_channels
        );
        Ok(Self {
            samples: audio.interleaved,
            sample_rate: audio.sample_rate as u32,
        })
    }

    /// Raw interleaved stereo little-endian f32 (what `ffmpeg -f f32le -ac 2`
    /// writes), the interim format until the bundled OGG is decoded on
    /// device.
    pub fn from_raw_f32_stereo(path: &std::path::Path, sample_rate: u32) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        if bytes.len() % 8 != 0 {
            return Err(anyhow!("{}: not whole stereo f32 frames", path.display()));
        }
        let samples: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        info!(
            "playback: {} · {:.1} s at {sample_rate} Hz stereo",
            path.display(),
            samples.len() as f32 / 2.0 / sample_rate as f32
        );
        Ok(Self {
            samples,
            sample_rate,
        })
    }
}

struct Shared {
    clip: Clip,
    /// The rate the output stream was opened with.
    output_rate: u32,
    /// Deliveries into the tap, for the analysis watchdog.
    callback_count: Arc<AtomicU64>,
    /// Playhead in clip frames, fixed point with 32 fractional bits, so the
    /// callback can resample when the output rate differs from the clip's.
    pos: AtomicU64,
    frames_played: AtomicUsize,
    tap: Arc<RingBuffer>,
}

pub struct Playback {
    _stream: cpal::Stream,
    shared: Arc<Shared>,
}

impl Playback {
    /// Start looping `clip` on the default output device. `tap` receives the
    /// played stereo frames at the output rate.
    pub fn start(clip: Clip, tap: Arc<RingBuffer>) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default output device"))?;
        let config = device
            .default_output_config()
            .context("default_output_config")?;
        let sample_rate = config.sample_rate();
        let channels = usize::from(config.channels());
        info!(
            "playback: output {sample_rate} Hz, {channels} ch, {:?}",
            config.sample_format()
        );
        let shared = Arc::new(Shared {
            clip,
            output_rate: sample_rate,
            callback_count: Arc::new(AtomicU64::new(0)),
            pos: AtomicU64::new(0),
            frames_played: AtomicUsize::new(0),
            tap,
        });
        let cb = shared.clone();
        let stream = device
            .build_output_stream(
                &config.into(),
                move |out: &mut [f32], _| fill(&cb, out, channels),
                |e| error!("playback stream error: {e}"),
                None,
            )
            .context("build_output_stream")?;
        stream.play().context("stream.play")?;
        Ok(Self {
            _stream: stream,
            shared,
        })
    }

    /// The output stream's rate (the tap ring's rate).
    pub fn output_rate(&self) -> u32 {
        self.shared.output_rate
    }

    /// Tap deliveries counter, shared with the analysis side.
    pub fn callback_count(&self) -> Arc<AtomicU64> {
        self.shared.callback_count.clone()
    }

    /// Frames handed to the output so far.
    pub fn frames_played(&self) -> usize {
        self.shared.frames_played.load(Ordering::Relaxed)
    }

    /// Playhead position in the clip, seconds.
    pub fn position_secs(&self) -> f32 {
        let pos = self.shared.pos.load(Ordering::Relaxed);
        (pos >> 32) as f32 / self.shared.clip.sample_rate as f32
    }
}

/// Output callback: linear-interpolated, looping read of the clip into
/// `out`, and the same frames into the tap ring. No allocation: the tap is
/// fed in fixed chunks. Only this callback advances `pos`.
fn fill(shared: &Shared, out: &mut [f32], channels: usize) {
    const CHUNK: usize = 256;
    let clip = &shared.clip;
    let frames = clip.samples.len() / 2;
    if frames < 2 || channels == 0 {
        out.fill(0.0);
        return;
    }
    // Clip frames per output frame, 32.32 fixed point.
    let step = ((f64::from(clip.sample_rate) / f64::from(shared.output_rate.max(1)))
        * 4_294_967_296.0) as u64;
    let mut pos = shared.pos.load(Ordering::Relaxed);
    let wrap = (frames as u64) << 32;
    let mut tap = [0.0f32; CHUNK * 2];
    let mut tap_len = 0;
    for frame in out.chunks_exact_mut(channels) {
        let i = (pos >> 32) as usize;
        let frac = (pos & 0xffff_ffff) as f32 / 4_294_967_296.0;
        let j = if i + 1 < frames { i + 1 } else { 0 };
        let l = clip.samples[2 * i] + (clip.samples[2 * j] - clip.samples[2 * i]) * frac;
        let r =
            clip.samples[2 * i + 1] + (clip.samples[2 * j + 1] - clip.samples[2 * i + 1]) * frac;
        match channels {
            1 => frame[0] = (l + r) * 0.5,
            _ => {
                frame[0] = l;
                frame[1] = r;
                for extra in &mut frame[2..] {
                    *extra = 0.0;
                }
            }
        }
        tap[tap_len] = l;
        tap[tap_len + 1] = r;
        tap_len += 2;
        if tap_len == tap.len() {
            shared.tap.push(&tap);
            tap_len = 0;
        }
        pos += step;
        if pos >= wrap {
            pos -= wrap;
        }
    }
    if tap_len > 0 {
        shared.tap.push(&tap[..tap_len]);
    }
    shared.pos.store(pos, Ordering::Relaxed);
    shared
        .frames_played
        .fetch_add(out.len() / channels, Ordering::Relaxed);
    shared.callback_count.fetch_add(1, Ordering::Relaxed);
}
