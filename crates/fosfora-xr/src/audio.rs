//! Live audio on the headset (S6): the core's `AudioSystem` over cpal's
//! AAudio backend, polled once per frame for the render side.
//!
//! The core owns the capture stream, the ring buffer and the `fosfora-audio`
//! analysis thread; this module only turns its per-frame accessors into the
//! `HopOutput` shape the headless scene renderer and the S5 sim consume, and
//! logs what the S6 gate needs (tempo lock, pulses, health).

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use fosfora_app::audio::capture::RingBuffer;
use fosfora_app::audio::hop::HopOutput;
use fosfora_app::audio::{AudioFeatures, AudioFrame, AudioSystem, StructureConfig, TempoControl};
use fosfora_app::gpu::audio_textures::WAVEFORM_PEEK;
use fosfora_app::settings::BandScale;
use log::{error, info};

/// Seconds between status lines in logcat.
const STATUS_PERIOD_S: f32 = 2.0;

pub struct LiveAudio {
    system: AudioSystem,
    /// An XR-owned capture stream (`mic_with`), kept alive with the system.
    _stream: Option<cpal::Stream>,
    /// A raw AAudio input stream (`mic_aaudio`), likewise.
    _aaudio: Option<ndk::audio::AudioStream>,
    features: AudioFeatures,
    waveform: Vec<f32>,
    started: Instant,
    since_status: f32,
    first_lock: Option<f32>,
    /// Beats seen, as counted by the analysis thread (never misses one even
    /// when the render loop runs slower than hops arrive).
    beats: u32,
}

impl LiveAudio {
    /// Open the default input device (the headset microphones through
    /// AAudio; needs `RECORD_AUDIO` granted) and start analysis.
    pub fn mic() -> Self {
        let system = AudioSystem::new();
        info!(
            "audio: capture at {} Hz, {:?}",
            system.sample_rate,
            system.indicator()
        );
        Self::wrap(system, None)
    }

    /// Analysis over samples the caller produces (the file playback tap):
    /// `ring` gets interleaved stereo at `sample_rate`, `callback_count`
    /// is bumped by the producer per delivery.
    pub fn from_ring(
        ring: Arc<RingBuffer>,
        sample_rate: u32,
        callback_count: Arc<AtomicU64>,
    ) -> Self {
        let system = AudioSystem::from_ring(
            ring,
            sample_rate,
            callback_count,
            BandScale::default(),
            Arc::new(Mutex::new(StructureConfig::default())),
            Arc::new(Mutex::new(TempoControl::default())),
        );
        info!("audio: analysis over an external ring at {sample_rate} Hz");
        Self::wrap(system, None)
    }

    /// The microphones through an XR-owned cpal input stream with an
    /// explicit sample format and rate (S6 diagnosis: the core's default
    /// F32 config reads as silence on the Quest), converted to f32 stereo
    /// into a ring the core analyzes.
    pub fn mic_with(format: cpal::SampleFormat, sample_rate: u32) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no default input device"))?;
        let config = cpal::StreamConfig {
            channels: 2,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let ring = Arc::new(RingBuffer::new());
        let callback_count = Arc::new(AtomicU64::new(0));
        let (ring_cb, count_cb) = (ring.clone(), callback_count.clone());
        let err = |e| error!("mic stream error: {e}");
        let stream = match format {
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    push_stereo(
                        &ring_cb,
                        &count_cb,
                        data.iter().map(|&s| f32::from(s) / 32768.0),
                    );
                },
                err,
                None,
            ),
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config,
                move |data: &[f32], _| push_stereo(&ring_cb, &count_cb, data.iter().copied()),
                err,
                None,
            ),
            other => return Err(anyhow!("unsupported mic format {other:?}")),
        }
        .context("build_input_stream")?;
        stream.play().context("mic stream.play")?;
        info!("audio: XR mic stream {format:?} at {sample_rate} Hz, 2 ch");
        let system = AudioSystem::from_ring(
            ring,
            sample_rate,
            callback_count,
            BandScale::default(),
            Arc::new(Mutex::new(StructureConfig::default())),
            Arc::new(Mutex::new(TempoControl::default())),
        );
        Ok(Self::wrap(system, Some(stream)))
    }

    /// The microphones through a raw AAudio input stream with an explicit
    /// input preset (cpal cannot set one; AAudio's default is voice
    /// recognition, which the Quest processes for speech). 16-bit stereo at
    /// `sample_rate`, converted to f32 into a ring the core analyzes.
    pub fn mic_aaudio(
        preset: ndk::audio::AudioInputPreset,
        sample_rate: u32,
        low_latency: bool,
    ) -> Result<Self> {
        use ndk::audio::{
            AudioCallbackResult, AudioDirection, AudioFormat, AudioPerformanceMode,
            AudioStreamBuilder,
        };
        let ring = Arc::new(RingBuffer::new());
        let callback_count = Arc::new(AtomicU64::new(0));
        let (ring_cb, count_cb) = (ring.clone(), callback_count.clone());
        let stream = AudioStreamBuilder::new()
            .context("AAudio builder")?
            .direction(AudioDirection::Input)
            .format(AudioFormat::PCM_I16)
            .channel_count(2)
            .sample_rate(i32::try_from(sample_rate).unwrap_or(48_000))
            .input_preset(preset)
            .performance_mode(if low_latency {
                AudioPerformanceMode::LowLatency
            } else {
                AudioPerformanceMode::None
            })
            .data_callback(Box::new(move |_stream, data, frames| {
                let n = usize::try_from(frames).unwrap_or(0) * 2;
                // SAFETY: AAudio hands the callback `frames` frames of the
                // stream's format (PCM_I16, 2 channels, as requested and
                // confirmed after open); the buffer is valid for the call.
                let samples = unsafe { std::slice::from_raw_parts(data.cast::<i16>(), n) };
                push_stereo(
                    &ring_cb,
                    &count_cb,
                    samples.iter().map(|&s| f32::from(s) / 32768.0),
                );
                AudioCallbackResult::Continue
            }))
            .error_callback(Box::new(|_stream, e| error!("AAudio input error: {e:?}")))
            .open_stream()
            .context("AAudio open_stream")?;
        let actual_rate = stream.sample_rate();
        let actual_channels = stream.channel_count();
        let actual_format = stream.format();
        info!(
            "audio: AAudio mic {preset:?} · {actual_rate} Hz · {actual_channels} ch · {actual_format:?} · preset now {:?} · perf {:?}",
            stream.input_preset(),
            stream.performance_mode()
        );
        if actual_channels != 2 || actual_format != AudioFormat::PCM_I16 {
            return Err(anyhow!(
                "AAudio opened {actual_channels} ch {actual_format:?}, need 2 ch PCM_I16"
            ));
        }
        stream.request_start().context("AAudio request_start")?;
        let system = AudioSystem::from_ring(
            ring,
            u32::try_from(actual_rate).unwrap_or(sample_rate),
            callback_count,
            BandScale::default(),
            Arc::new(Mutex::new(StructureConfig::default())),
            Arc::new(Mutex::new(TempoControl::default())),
        );
        let mut live = Self::wrap(system, None);
        live._aaudio = Some(stream);
        Ok(live)
    }

    fn wrap(system: AudioSystem, stream: Option<cpal::Stream>) -> Self {
        Self {
            system,
            _stream: stream,
            _aaudio: None,
            features: AudioFeatures::default(),
            waveform: vec![0.0; WAVEFORM_PEEK],
            started: Instant::now(),
            since_status: 0.0,
            first_lock: None,
            beats: 0,
        }
    }

    /// Advance one render frame: interpolated features (held when no new hop
    /// arrived), the latest spectrum, mel column and dMFCC, and whether a
    /// beat fired since the last call.
    pub fn frame(&mut self, dt: f32, ts: f64) -> HopOutput {
        if let Some(f) = self.system.latest_features(dt) {
            self.features = f;
        }
        let pulses = self.system.pulse_counts();
        let beat_fired = pulses.beat != self.beats;
        self.beats = pulses.beat;
        let f = self.features;

        if self.first_lock.is_none() && f.raw_bpm() > 0.0 {
            let t = self.started.elapsed().as_secs_f32();
            self.first_lock = Some(t);
            info!(
                "audio: tempo first lock at {t:.1} s: {:.1} BPM",
                f.raw_bpm()
            );
        }
        self.since_status += dt;
        if self.since_status >= STATUS_PERIOD_S {
            self.since_status = 0.0;
            let health = self.system.poll_health();
            // Raw input level, to tell "silent room" from "no samples".
            self.system.recording_ring.peek_latest(&mut self.waveform);
            let peak = self.waveform.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            info!(
                "audio: peak {peak:.4} · bpm {:.1} · beat_strength {:.2} · rms {:.3} · bass {:.2} · onset {:.2} · centroid {:.2} · beats {} downbeats {} · {:?}{}",
                f.raw_bpm(),
                f.beat_strength,
                f.rms,
                f.bass,
                f.onset,
                f.centroid,
                pulses.beat,
                pulses.downbeat,
                self.system.indicator(),
                health.map(|h| format!(" · {h}")).unwrap_or_default()
            );
        }

        HopOutput {
            frame: AudioFrame {
                features: f,
                spectrum: self.system.latest_spectrum().into(),
                mel: self.system.latest_mel().into(),
                dmfcc: *self.system.latest_dmfcc(),
                timestamp: ts,
                phase_frozen: false,
                bar_duration: f.beat_period_secs().map_or(2.0, |p| 4.0 * f64::from(p)),
                beat_time: None,
                section_boundary: None,
            },
            beat_fired,
            downbeat_fired: f.downbeat > 0.5,
            drop_fired: false,
            pre_norm: f,
        }
    }

    /// The newest `WAVEFORM_PEEK` mono samples, for the waveform texture.
    pub fn waveform(&mut self) -> &[f32] {
        self.system.recording_ring.peek_latest(&mut self.waveform);
        &self.waveform
    }
}

/// Push already-stereo interleaved samples into the ring in fixed chunks
/// (no allocation on the callback thread) and count the delivery.
fn push_stereo(ring: &RingBuffer, count: &AtomicU64, samples: impl Iterator<Item = f32>) {
    let mut chunk = [0.0f32; 512];
    let mut n = 0;
    for s in samples {
        chunk[n] = s;
        n += 1;
        if n == chunk.len() {
            ring.push(&chunk);
            n = 0;
        }
    }
    if n > 0 {
        ring.push(&chunk[..n]);
    }
    count.fetch_add(1, Ordering::Relaxed);
}
