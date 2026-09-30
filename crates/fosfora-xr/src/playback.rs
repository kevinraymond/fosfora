//! Test-track playback on the headset (S6): a decoded stereo clip looped
//! through a raw AAudio output stream, with every played frame also pushed
//! into a tap ring so the analysis chain can follow what the speakers play
//! rather than what the microphones pick up.
//!
//! The tap runs **behind** the output by the stream's own latency (board
//! #3253). Filmed in S6, the beat flash led the sound by ~115 ms: the tap
//! was fed as the frames were handed to AAudio, some 190 ms before the
//! speaker emitted them, while the analysis and the render loop needed only
//! ~45 ms. AAudio reports where the DAC is (`AAudioStream_getTimestamp`), so
//! the callback estimates the output latency from that, holds the played
//! frames in a delay line and hands them to the tap when the speaker is
//! about to play them, less the detection time. cpal's output path could
//! not do this: its callback timestamps mix two clocks and hide the stream.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use fosfora_app::audio::capture::RingBuffer;
use log::{error, info};
use ndk::audio::{
    AudioCallbackResult, AudioContentType, AudioDirection, AudioFormat, AudioPerformanceMode,
    AudioStream, AudioStreamBuilder, AudioUsage, Clockid,
};

/// From a sample entering the analysis ring to the photon: the beat pulse
/// reaches the render loop 20–44 ms after the frame is handed to AAudio
/// (median 32; the app's own log against the click track), and the flash
/// reaches the lens ~44 ms after that (phone film, Sep 28: flash 41 ms
/// after the sound with a 36 ms output, so 36 + 41 − 32). The tap runs this
/// much less than the output latency behind the speaker, so the flash and
/// the sound coincide at the lens.
const ANALYSIS_TO_PHOTON_MS: f32 = 76.0;
/// Output latency the low-latency stream is given by default, by buffer
/// size: `ANALYSIS_TO_PHOTON_MS` plus a margin, so the tap delay above is
/// small but positive. AAudio's own low-latency buffer (1536 frames, 36 ms
/// to the speaker) is shorter than the analysis-and-display chain, which
/// left the flash 41 ms behind the sound with no delay to trade.
const TARGET_OUTPUT_MS: f32 = 84.0;
/// The output latency is estimated once per this many played frames (5 per
/// second at 48 kHz), whatever size the callbacks come in.
const ESTIMATE_EVERY_FRAMES: usize = 9_600;
// The estimate is averaged over the whole run. On the Quest 3's default
// (legacy) output the raw estimate saw-tooths between 137 and 215 ms over
// ~4 s (the timestamps' position runs ~2 % ahead of the frames handed
// over, then snaps back), so a 5 s median still swung ±20 ms and stepped the
// delay every 2 s; the run's mean settles within a few ms after two teeth,
// and the beat pulses land at its value. The low-latency (MMAP) output
// reports a constant 36 ms and needs none of this. The stream is torn down
// if the output device changes (AAudio disconnects it), so a run-long mean
// never goes stale.
/// The tap delay moves only when the smoothed estimate differs from it by
/// more than this, and at most once per `STEP_INTERVAL_FRAMES`, so the
/// analysis is not stuttered by noise (each step drops or repeats that much
/// audio in the tap).
const DELAY_STEP_MS: f32 = 15.0;
const STEP_INTERVAL_FRAMES: usize = 96_000;
/// Delay line capacity, frames: 2 s at 48 kHz.
const DELAY_CAP_FRAMES: usize = 96_000;

/// A clip in memory: interleaved stereo f32 at `sample_rate`.
pub struct Clip {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl Clip {
    /// A synthetic click track for the audio-to-photon measurement: a 2 ms
    /// burst of 1 kHz every beat at `bpm`, `secs` long, stereo at `rate`.
    /// Sharp enough for a phone recording to time, and the core's beat
    /// tracker locks to it within a few beats.
    pub fn click(bpm: f32, secs: f32, rate: u32) -> Self {
        let frames = (secs * rate as f32) as usize;
        let period = (60.0 / bpm * rate as f32) as usize;
        let burst = rate as usize * 2 / 1000;
        let mut samples = vec![0.0f32; frames * 2];
        for i in 0..frames {
            let k = i % period;
            if k < burst {
                let env = 1.0 - k as f32 / burst as f32;
                let v = 0.8 * env * (i as f32 / rate as f32 * 1000.0 * std::f32::consts::TAU).sin();
                samples[2 * i] = v;
                samples[2 * i + 1] = v;
            }
        }
        info!("playback: click track {bpm} BPM, {secs} s at {rate} Hz");
        Self {
            samples,
            sample_rate: rate,
        }
    }

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

/// How the output stream is opened and how far the tap runs behind it.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlaybackOptions {
    /// Ask AAudio for its low-latency performance mode. On the Quest 3 that
    /// is an MMAP stream with 192-frame bursts and a constant, exactly
    /// reported 36 ms of output latency; the default mode is a legacy
    /// stream with 1922-frame bursts and 140–215 ms that the timestamps
    /// report only roughly. The app defaults to low latency.
    pub low_latency: bool,
    /// A fixed tap delay in ms instead of the estimate from the stream's
    /// timestamps (a knob for the latency measurement; 0 = the old
    /// behavior, tap fed as the frames are handed to AAudio).
    pub tap_delay_ms: Option<f32>,
    /// Output buffer in ms of audio (rounded up to bursts, capped by the
    /// stream's capacity); `None` = `TARGET_OUTPUT_MS` in low-latency mode
    /// and AAudio's default otherwise, `Some(0.0)` = AAudio's default.
    pub buffer_ms: Option<f32>,
    /// The clip frame the playhead starts on (wrapped to the clip's
    /// length): the hand menu's Music row resumes where it stopped
    /// ([`Playback::resume_frame`]). 0 = the top.
    pub start_frame: usize,
}

struct Shared {
    /// Shared with the caller, so a stopped clip is kept without a copy
    /// and started again without a decode.
    clip: Arc<Clip>,
    /// The rate the output stream opened with (set before it starts).
    output_rate: AtomicU32,
    /// Deliveries into the tap, for the analysis watchdog.
    callback_count: Arc<AtomicU64>,
    /// Frames played since the last latency estimate and since the last
    /// delay step (the cadences below; the tap counter above is the
    /// watchdog's and stays one per callback).
    frames_since_estimate: AtomicUsize,
    frames_since_step: AtomicUsize,
    /// Playhead in clip frames, fixed point with 32 fractional bits, so the
    /// callback can resample when the output rate differs from the clip's.
    pos: AtomicU64,
    frames_played: AtomicUsize,
    tap: Arc<RingBuffer>,
    /// Latest output latency estimate, in hundredths of a millisecond
    /// (0 = none yet).
    latency_cms: AtomicU32,
    /// Played frames the callback holds back before handing them to the
    /// tap.
    delay_frames: AtomicUsize,
    /// A knob-fixed delay: no estimation.
    fixed_delay: bool,
    /// Played frames not yet handed to the tap (interleaved). Only the
    /// callback touches it, so the lock is never contended.
    delay_line: Mutex<VecDeque<f32>>,
    /// Sum and count of the latency estimates (ms) so far; callback-only,
    /// like the delay line.
    estimates: Mutex<(f64, u32)>,
}

pub struct Playback {
    _stream: AudioStream,
    shared: Arc<Shared>,
}

impl Playback {
    /// Start looping `clip` on the default output device from
    /// `options.start_frame`. `tap` receives the played stereo frames at
    /// the output rate, `options.tap_delay_ms` or the estimated output
    /// latency (less `ANALYSIS_TO_PHOTON_MS`) behind the speaker.
    ///
    /// Nothing here is bound to a thread or to the session's state: the
    /// launch path calls it on `android_main`'s thread before the first
    /// frame, the Music row from the frame loop on the same thread. It
    /// blocks while AAudio opens and starts the stream.
    pub fn start(clip: Arc<Clip>, tap: Arc<RingBuffer>, options: PlaybackOptions) -> Result<Self> {
        let requested_rate = clip.sample_rate;
        let clip_frames = clip.samples.len() / 2;
        let start = if clip_frames > 0 {
            options.start_frame % clip_frames
        } else {
            0
        };
        let shared = Arc::new(Shared {
            clip,
            output_rate: AtomicU32::new(requested_rate),
            callback_count: Arc::new(AtomicU64::new(0)),
            frames_since_estimate: AtomicUsize::new(ESTIMATE_EVERY_FRAMES),
            frames_since_step: AtomicUsize::new(STEP_INTERVAL_FRAMES),
            pos: AtomicU64::new((start as u64) << 32),
            frames_played: AtomicUsize::new(0),
            tap,
            latency_cms: AtomicU32::new(0),
            delay_frames: AtomicUsize::new(0),
            fixed_delay: options.tap_delay_ms.is_some(),
            delay_line: Mutex::new(VecDeque::with_capacity(DELAY_CAP_FRAMES * 2)),
            estimates: Mutex::new((0.0, 0)),
        });
        let cb = shared.clone();
        let stream = AudioStreamBuilder::new()
            .context("AAudio builder")?
            .direction(AudioDirection::Output)
            .format(AudioFormat::PCM_Float)
            .channel_count(2)
            .sample_rate(i32::try_from(requested_rate).unwrap_or(48_000))
            .usage(AudioUsage::Media)
            .content_type(AudioContentType::Music)
            .performance_mode(if options.low_latency {
                AudioPerformanceMode::LowLatency
            } else {
                AudioPerformanceMode::None
            })
            // Room for the buffer size below (a request; the stream reports
            // what it got).
            .buffer_capacity_in_frames(
                (TARGET_OUTPUT_MS.max(options.buffer_ms.unwrap_or(0.0)) / 1000.0
                    * requested_rate as f32
                    * 2.0) as i32,
            )
            .data_callback(Box::new(move |stream, data, frames| {
                let channels = usize::try_from(stream.channel_count()).unwrap_or(2).max(1);
                let n = usize::try_from(frames).unwrap_or(0) * channels;
                // SAFETY: AAudio hands the callback a buffer of `frames`
                // frames in the stream's format, PCM_Float with
                // `channel_count` channels (checked after open), writable
                // for the duration of the call.
                let out = unsafe { std::slice::from_raw_parts_mut(data.cast::<f32>(), n) };
                fill(&cb, stream, out, channels);
                AudioCallbackResult::Continue
            }))
            .error_callback(Box::new(|_stream, e| {
                error!("playback stream error: {e:?}");
            }))
            .open_stream()
            .context("AAudio open_stream (output)")?;
        let rate = u32::try_from(stream.sample_rate()).unwrap_or(requested_rate);
        let channels = stream.channel_count();
        let format = stream.format();
        info!(
            "playback: AAudio output {rate} Hz · {channels} ch · {format:?} · perf {:?} · burst {} · buffer {} frames",
            stream.performance_mode(),
            stream.frames_per_burst(),
            stream.buffer_size_in_frames()
        );
        if format != AudioFormat::PCM_Float || channels < 1 {
            return Err(anyhow!(
                "AAudio opened {channels} ch {format:?}, need PCM_Float"
            ));
        }
        shared.output_rate.store(rate, Ordering::Relaxed);
        // Buffer size: enough audio in flight that the speaker is behind the
        // analysis-and-display chain, so the tap delay can line them up.
        let buffer_ms = match options.buffer_ms {
            Some(ms) => ms,
            None if options.low_latency => TARGET_OUTPUT_MS,
            None => 0.0,
        };
        if buffer_ms > 0.0 {
            let burst = u32::try_from(stream.frames_per_burst()).unwrap_or(1).max(1);
            let want = ((buffer_ms / 1000.0 * rate as f32) as u32).div_ceil(burst) * burst;
            let want = i32::try_from(want)
                .unwrap_or(i32::MAX)
                .min(stream.buffer_capacity_in_frames());
            // AAudio returns the size it set (positive) on success; ndk 0.9
            // maps every non-zero result to an error, so a positive
            // "error" is the success path. The stream's own getter is the
            // truth either way.
            let result = stream.set_buffer_size_in_frames(want);
            let got = stream.buffer_size_in_frames();
            match result {
                Ok(_) | Err(ndk::audio::AudioError::__Unknown(1..)) => info!(
                    "playback: buffer {want} frames asked ({buffer_ms:.0} ms), {got} set, capacity {}",
                    stream.buffer_capacity_in_frames()
                ),
                Err(e) => {
                    error!("playback: set_buffer_size_in_frames({want}): {e:?}, buffer {got}");
                }
            }
        }
        if let Some(ms) = options.tap_delay_ms {
            let frames = ((ms.max(0.0) / 1000.0 * rate as f32) as usize).min(DELAY_CAP_FRAMES);
            shared.delay_frames.store(frames, Ordering::Relaxed);
            info!("playback: tap delay fixed at {ms:.0} ms ({frames} frames)");
        }
        stream.request_start().context("AAudio request_start")?;
        Ok(Self {
            _stream: stream,
            shared,
        })
    }

    /// The output stream's rate (the tap ring's rate).
    pub fn output_rate(&self) -> u32 {
        self.shared.output_rate.load(Ordering::Relaxed)
    }

    /// Tap deliveries counter, shared with the analysis side.
    pub fn callback_count(&self) -> Arc<AtomicU64> {
        self.shared.callback_count.clone()
    }

    /// Frames handed to the output so far.
    pub fn frames_played(&self) -> usize {
        self.shared.frames_played.load(Ordering::Relaxed)
    }

    /// Playhead position in the clip, seconds: the frame being handed to
    /// AAudio now, `latency_ms` ahead of the speaker.
    pub fn position_secs(&self) -> f32 {
        let pos = self.shared.pos.load(Ordering::Relaxed);
        (pos >> 32) as f32 / self.shared.clip.sample_rate as f32
    }

    /// The clip frame to start again from after this playback stops: the
    /// playhead less the output latency (the frames handed to AAudio but
    /// not yet played, which closing the stream discards), wrapped.
    pub fn resume_frame(&self) -> usize {
        let frames = self.shared.clip.samples.len() / 2;
        if frames == 0 {
            return 0;
        }
        let head = (self.shared.pos.load(Ordering::Relaxed) >> 32) as usize;
        let behind = (self.latency_ms() / 1000.0 * self.shared.clip.sample_rate as f32) as usize;
        (head + frames - behind % frames) % frames
    }

    /// The clip this plays.
    pub fn clip(&self) -> &Arc<Clip> {
        &self.shared.clip
    }

    /// Latest estimate of the output latency (handed to AAudio → speaker),
    /// ms; 0 before the first estimate.
    pub fn latency_ms(&self) -> f32 {
        self.shared.latency_cms.load(Ordering::Relaxed) as f32 / 100.0
    }

    /// Output underruns so far (AAudio's count): the price of a small
    /// low-latency buffer under GPU load.
    pub fn xruns(&self) -> i32 {
        self._stream.x_run_count()
    }

    /// How far the tap runs behind the frames handed to AAudio, ms.
    pub fn tap_delay_ms(&self) -> f32 {
        self.shared.delay_frames.load(Ordering::Relaxed) as f32 / self.output_rate() as f32 * 1000.0
    }
}

/// CLOCK_MONOTONIC now, in nanoseconds: the clock AAudio's timestamps use
/// when asked with `Clockid::Monotonic`. `std::time::Instant` is the same
/// clock on Android but does not expose its value.
fn monotonic_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `clock_gettime` writes one `timespec` through a valid pointer
    // to a local; CLOCK_MONOTONIC always exists on Android.
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut ts) };
    if rc != 0 {
        return 0;
    }
    // Both fields are `i64` on aarch64 Android (the only target this module
    // builds for).
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

/// Output latency now: how long until the frame being handed to AAudio
/// reaches the speaker, from the stream's last DAC timestamp. `None` when
/// the stream has no timestamp yet (the first bursts) or the numbers do not
/// make sense.
fn output_latency_s(stream: &AudioStream, frames_in_flight: i64) -> Option<f64> {
    let ts = stream.timestamp(Clockid::Monotonic).ok()?;
    let now = monotonic_ns();
    if ts.time_nanoseconds <= 0 || now <= 0 {
        return None;
    }
    let rate = f64::from(stream.sample_rate().max(1));
    // The DAC was at `frame_position` at `time_nanoseconds`; the frame we
    // hand over now is `frames_written` (+ this buffer) frames later, so it
    // plays that many frames after the timestamp, less the time since.
    let ahead = (stream.frames_written() + frames_in_flight - ts.frame_position) as f64 / rate
        - (now - ts.time_nanoseconds) as f64 * 1e-9;
    (0.0..2.0).contains(&ahead).then_some(ahead)
}

/// Output callback: linear-interpolated, looping read of the clip into
/// `out`, the same frames into the delay line, and the frames whose time
/// has come from the delay line into the tap. No allocation: the tap is fed
/// in fixed chunks and the delay line was sized up front. Only this
/// callback advances `pos`.
fn fill(shared: &Shared, stream: &AudioStream, out: &mut [f32], channels: usize) {
    const CHUNK: usize = 256;
    let clip = &shared.clip;
    let frames = clip.samples.len() / 2;
    let output_rate = u32::try_from(stream.sample_rate()).unwrap_or(0).max(1);
    if frames < 2 || channels == 0 {
        out.fill(0.0);
        return;
    }
    // Clip frames per output frame, 32.32 fixed point.
    let step = ((f64::from(clip.sample_rate) / f64::from(output_rate)) * 4_294_967_296.0) as u64;
    let mut pos = shared.pos.load(Ordering::Relaxed);
    let wrap = (frames as u64) << 32;
    let mut line = shared
        .delay_line
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        if line.len() < DELAY_CAP_FRAMES * 2 {
            line.push_back(l);
            line.push_back(r);
        }
        pos += step;
        if pos >= wrap {
            pos -= wrap;
        }
    }
    shared.pos.store(pos, Ordering::Relaxed);
    let out_frames = out.len() / channels;
    shared
        .frames_played
        .fetch_add(out_frames, Ordering::Relaxed);

    // The output latency, on the first callback and then periodically.
    let since_estimate = shared
        .frames_since_estimate
        .fetch_add(out_frames, Ordering::Relaxed)
        + out_frames;
    let since_step = shared
        .frames_since_step
        .fetch_add(out_frames, Ordering::Relaxed)
        + out_frames;
    if !shared.fixed_delay && since_estimate >= ESTIMATE_EVERY_FRAMES {
        shared.frames_since_estimate.store(0, Ordering::Relaxed);
        if let Some(ahead) = output_latency_s(stream, out_frames as i64) {
            // Run-long mean: a single estimate jitters by a burst or more.
            let mut est = shared
                .estimates
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            est.0 += ahead * 1000.0;
            est.1 += 1;
            let first = est.1 == 1;
            let latency_ms = (est.0 / f64::from(est.1)) as f32;
            drop(est);
            shared
                .latency_cms
                .store((latency_ms * 100.0) as u32, Ordering::Relaxed);
            let target_ms = (latency_ms - ANALYSIS_TO_PHOTON_MS).max(0.0);
            let target = ((target_ms / 1000.0 * output_rate as f32) as usize).min(DELAY_CAP_FRAMES);
            let current = shared.delay_frames.load(Ordering::Relaxed);
            let current_ms = current as f32 / output_rate as f32 * 1000.0;
            // The first estimate always sets the delay (from 0); later ones
            // move it only through the hysteresis and the interval.
            if first
                || ((target_ms - current_ms).abs() > DELAY_STEP_MS
                    && since_step >= STEP_INTERVAL_FRAMES)
            {
                shared.frames_since_step.store(0, Ordering::Relaxed);
                shared.delay_frames.store(target, Ordering::Relaxed);
                info!(
                    "playback: output latency {latency_ms:.1} ms (raw {:.1}) · tap delay {current_ms:.0} -> {target_ms:.0} ms",
                    ahead * 1000.0
                );
            }
        }
    }

    // Hand the frames whose speaker time has come to the tap.
    let hold = shared.delay_frames.load(Ordering::Relaxed) * 2;
    let mut tap = [0.0f32; CHUNK * 2];
    while line.len() > hold {
        let n = (line.len() - hold).min(tap.len());
        for slot in &mut tap[..n] {
            *slot = line.pop_front().unwrap_or(0.0);
        }
        shared.tap.push(&tap[..n]);
    }
    drop(line);
    shared.callback_count.fetch_add(1, Ordering::Relaxed);
}
