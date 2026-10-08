//! The voice path, V1: the window and the text (board #3751,
//! `docs/xr/VOICE_DESIGN.md`). Hold the left fist and the headset listens;
//! release it (or [`MAX_S`] pass) and what was said is transcribed on the
//! headset and shown on the in-world label and in the log. Nothing acts on
//! the words yet (V2 is the grammar).
//!
//! **The pure part** (every target, desktop-tested): the push-to-talk window
//! ([`PushToTalk`]), the stream's 48 kHz stereo to whisper's 16 kHz mono
//! ([`mono_16k`]), the encoder window cut to the clip ([`audio_ctx`]), the
//! log and label text ([`heard_line`], [`label_text`]) and a tiny RIFF
//! reader for the unworn test path ([`parse_wav`]).
//!
//! **The window.** A fist opens it only once it has lasted [`DWELL_S`] (a
//! shorter one is a tracking glitch or a grab, not a press); release closes
//! it, and so does [`MAX_S`] of holding, after which the fist has to open
//! before it can press again. A closed window is [`Window::Closing`] until
//! its transcription returns ([`PushToTalk::finish`]): a fist then is
//! ignored, so two windows never overlap and one transcription runs at a
//! time.
//!
//! **The device part** (Android, `Voice`): whisper.cpp through
//! `whisper-rs` on one worker thread that loads `ggml-base.en.bin` at launch
//! and then transcribes each closed window (greedy, English, one segment,
//! no context, the encoder window cut to the clip: `MEASURED.md`,
//! "On-device speech to text"), so neither the load nor a transcription
//! ever runs on the frame thread. The window's audio is its own AAudio
//! input stream (the voice recognition preset, 48 kHz stereo, opened when
//! the window opens and closed when it closes), never the music path's.

/// Seconds a fist has to last before it opens the window.
pub const DWELL_S: f32 = 0.15;
/// The longest window (s): holding past it closes the window.
pub const MAX_S: f32 = 6.0;
/// whisper's input rate (Hz).
pub const WHISPER_RATE: u32 = 16_000;
/// The shortest clip whisper transcribes (s): whisper.cpp returns nothing
/// for input under 1 s, so a shorter window ("next") is padded with
/// silence to this length ([`pad_short`]).
pub const MIN_S: f32 = 1.1;
/// The encoder's frames per second of audio, its margin (64 frames,
/// 1.28 s), and its full 30 s window.
const CTX_PER_S: f32 = 50.0;
const CTX_MARGIN: i32 = 64;
const CTX_FULL: i32 = 1500;
/// What the label reads when the transcription is empty.
pub const NOTHING_LABEL: &str = "Didn't catch that";
/// What the label reads while the window is open, and while its
/// transcription runs.
pub const LISTENING_LABEL: &str = "Listening…";
pub const WAITING_LABEL: &str = "…";

/// The push-to-talk window.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Window {
    /// Not listening; a fist that lasts [`DWELL_S`] opens it.
    #[default]
    Closed,
    /// Listening for `since_s` seconds.
    Open { since_s: f32 },
    /// Closed, its transcription still running: fists are ignored.
    Closing,
}

/// What a step of the window did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// The window opened: start listening.
    Opened,
    /// The window closed after `seconds` of listening: transcribe.
    Closed { seconds: f32 },
}

/// The push-to-talk state machine: the left fist in, [`Event`]s out.
#[derive(Debug, Clone, Default)]
pub struct PushToTalk {
    window: Window,
    /// How long the current fist has lasted while closed (s).
    held_s: f32,
    /// [`MAX_S`] closed the window under a fist still held: it has to
    /// open before the next press counts.
    wait_release: bool,
}

impl PushToTalk {
    pub fn window(&self) -> Window {
        self.window
    }

    /// Advance by one frame of `dt` seconds with the fist this frame.
    pub fn step(&mut self, fist: bool, dt: f32) -> Option<Event> {
        let dt = dt.max(0.0);
        match self.window {
            Window::Closed => {
                if !fist {
                    self.held_s = 0.0;
                    self.wait_release = false;
                    return None;
                }
                if self.wait_release {
                    return None;
                }
                self.held_s += dt;
                if self.held_s >= DWELL_S - 1e-6 {
                    self.held_s = 0.0;
                    self.window = Window::Open { since_s: 0.0 };
                    return Some(Event::Opened);
                }
                None
            }
            Window::Open { since_s } => {
                if !fist {
                    self.window = Window::Closing;
                    return Some(Event::Closed { seconds: since_s });
                }
                let since_s = since_s + dt;
                if since_s >= MAX_S - 1e-6 {
                    self.window = Window::Closing;
                    self.wait_release = true;
                    return Some(Event::Closed { seconds: since_s });
                }
                self.window = Window::Open { since_s };
                None
            }
            Window::Closing => {
                // A fist here is ignored; a release still counts, so a
                // fist after it can press once the transcription is back.
                if !fist {
                    self.wait_release = false;
                }
                self.held_s = 0.0;
                None
            }
        }
    }

    /// The closed window's transcription returned (or never started): the
    /// next fist can open a window.
    pub fn finish(&mut self) {
        if self.window == Window::Closing {
            self.window = Window::Closed;
            self.held_s = 0.0;
        }
    }

    /// A transcription that no window started (the `voicefile` path) is
    /// running: closed becomes closing. False if a window is open.
    pub fn begin_closing(&mut self) -> bool {
        match self.window {
            Window::Closed => {
                self.window = Window::Closing;
                self.held_s = 0.0;
                true
            }
            Window::Closing => true,
            Window::Open { .. } => false,
        }
    }
}

/// Interleaved stereo at `rate` (Hz) to mono at [`WHISPER_RATE`]: the
/// channels averaged, then resampled by linear interpolation. Above 16 kHz
/// the mono signal first goes through a moving average as long as the rate
/// ratio (a box low-pass, three taps from 48 kHz), so that speech above
/// 8 kHz folds back into whisper's band attenuated rather than whole.
pub fn mono_16k(stereo: &[f32], rate: u32) -> Vec<f32> {
    let mono: Vec<f32> = stereo
        .chunks_exact(2)
        .map(|f| 0.5 * (f[0] + f[1]))
        .collect();
    if rate == WHISPER_RATE || mono.is_empty() || rate == 0 {
        return mono;
    }
    let step = rate as f64 / f64::from(WHISPER_RATE);
    let taps = step.round().max(1.0) as usize;
    let smooth: Vec<f32> = if taps > 1 {
        let half = taps / 2;
        (0..mono.len())
            .map(|i| {
                let lo = i.saturating_sub(half);
                let hi = (lo + taps).min(mono.len());
                mono[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
            })
            .collect()
    } else {
        mono
    };
    let out_len = (smooth.len() as f64 / step).floor() as usize;
    let last = smooth.len() - 1;
    (0..out_len)
        .map(|i| {
            let x = i as f64 * step;
            let i0 = (x.floor() as usize).min(last);
            let i1 = (i0 + 1).min(last);
            let frac = (x - i0 as f64) as f32;
            smooth[i0] + (smooth[i1] - smooth[i0]) * frac
        })
        .collect()
}

/// Pad a 16 kHz clip shorter than [`MIN_S`] with silence up to it.
pub fn pad_short(mono: &mut Vec<f32>) {
    let min = (MIN_S * WHISPER_RATE as f32).ceil() as usize;
    if mono.len() < min {
        mono.resize(min, 0.0);
    }
}

/// The encoder window for a clip of `seconds` (whisper's `audio_ctx`): the
/// clip plus 1.28 s at 50 frames a second, `((seconds + 1.28) / 0.02).ceil()`
/// written as `ceil(seconds * 50) + 64` so a whole number of frames does not
/// round up on float error; within `[64, 1500]` (1500 is whisper's whole
/// 30 s). The spike's rule: 0.16 to 0.43 s per 3 s clip on the Quest 3
/// against 1.05 to 1.46 s with the full window, every transcript right.
pub fn audio_ctx(seconds: f32) -> i32 {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    let frames = (seconds * CTX_PER_S).ceil().min(CTX_FULL as f32) as i32;
    (frames + CTX_MARGIN).clamp(CTX_MARGIN, CTX_FULL)
}

/// What whisper said, as words: trimmed, and empty when it is only a
/// non-speech annotation ("[BLANK_AUDIO]", "(wind blowing)", "[Music]").
pub fn spoken(text: &str) -> &str {
    let t = text.trim();
    let annotation = |open: char, close: char| {
        t.starts_with(open) && t.ends_with(close) && t[1..t.len() - 1].find([open, close]).is_none()
    };
    if t.len() >= 2 && (annotation('[', ']') || annotation('(', ')')) {
        ""
    } else {
        t
    }
}

/// The log line for a transcription of `ms` milliseconds:
/// `voice: heard "…" (412 ms)`, or `voice: heard nothing`.
pub fn heard_line(text: &str, ms: u32) -> String {
    let text = spoken(text);
    if text.is_empty() {
        "voice: heard nothing".to_owned()
    } else {
        format!("voice: heard \"{text}\" ({ms} ms)")
    }
}

/// The label for a transcription: the sentence, or [`NOTHING_LABEL`].
pub fn label_text(text: &str) -> String {
    let text = spoken(text);
    if text.is_empty() {
        NOTHING_LABEL.to_owned()
    } else {
        text.to_owned()
    }
}

/// A WAV file's samples as f32 in -1..1, for the `voicefile` path: RIFF
/// WAVE, PCM (format 1), mono, 16 kHz, 16-bit, the chunks walked in order
/// (a `LIST` before `data` is skipped).
pub fn parse_wav(bytes: &[u8]) -> Result<Vec<f32>, String> {
    let u16_at = |b: &[u8], i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_at = |b: &[u8], i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF WAVE file".to_owned());
    }
    let mut at = 12;
    let mut format = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32_at(bytes, at + 4) as usize;
        let body = at + 8;
        let end = body.saturating_add(len).min(bytes.len());
        match id {
            b"fmt " => {
                if end - body < 16 {
                    return Err("fmt chunk too short".to_owned());
                }
                format = Some((
                    u16_at(bytes, body),
                    u16_at(bytes, body + 2),
                    u32_at(bytes, body + 4),
                    u16_at(bytes, body + 14),
                ));
            }
            b"data" => {
                let Some((tag, channels, rate, bits)) = format else {
                    return Err("data before fmt".to_owned());
                };
                if (tag, channels, rate, bits) != (1, 1, WHISPER_RATE, 16) {
                    return Err(format!(
                        "need PCM mono 16 kHz 16-bit, got format {tag}, {channels} ch, {rate} Hz, {bits}-bit"
                    ));
                }
                return Ok(bytes[body..end]
                    .chunks_exact(2)
                    .map(|s| f32::from(i16::from_le_bytes([s[0], s[1]])) / 32768.0)
                    .collect());
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        at = body.saturating_add(len).saturating_add(len & 1);
    }
    Err("no data chunk".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 72.0;

    /// Step `ptt` with `fist` for `seconds`, collecting the events.
    fn hold(ptt: &mut PushToTalk, fist: bool, seconds: f32) -> Vec<Event> {
        let frames = (seconds / DT).round() as usize;
        (0..frames).filter_map(|_| ptt.step(fist, DT)).collect()
    }

    #[test]
    fn a_short_fist_is_nothing() {
        let mut ptt = PushToTalk::default();
        assert!(hold(&mut ptt, true, 0.1).is_empty());
        assert!(hold(&mut ptt, false, 0.5).is_empty());
        assert_eq!(ptt.window(), Window::Closed);
        // The dwell starts again after a release.
        assert!(hold(&mut ptt, true, 0.1).is_empty());
        assert_eq!(ptt.window(), Window::Closed);
    }

    #[test]
    fn a_held_fist_opens_after_the_dwell_and_closes_on_release() {
        let mut ptt = PushToTalk::default();
        let dwell_frames = (DWELL_S / DT).ceil() as usize;
        for _ in 0..dwell_frames - 1 {
            assert_eq!(ptt.step(true, DT), None);
        }
        assert_eq!(ptt.step(true, DT), Some(Event::Opened));
        assert!(hold(&mut ptt, true, 2.0).is_empty());
        assert!(matches!(ptt.window(), Window::Open { .. }));
        let Some(Event::Closed { seconds }) = ptt.step(false, DT) else {
            panic!("no close on release");
        };
        assert!((seconds - 2.0).abs() < 1e-3, "{seconds}");
        assert_eq!(ptt.window(), Window::Closing);
    }

    #[test]
    fn six_seconds_close_it_and_the_fist_must_open_first() {
        let mut ptt = PushToTalk::default();
        let events = hold(&mut ptt, true, DWELL_S + MAX_S + 1.0);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0], Event::Opened);
        let Event::Closed { seconds } = events[1] else {
            panic!("{events:?}");
        };
        assert!((seconds - MAX_S).abs() < DT, "{seconds}");
        ptt.finish();
        // Still the same fist: nothing.
        assert!(hold(&mut ptt, true, 1.0).is_empty());
        assert!(hold(&mut ptt, false, 0.1).is_empty());
        assert_eq!(hold(&mut ptt, true, 0.5), vec![Event::Opened]);
    }

    #[test]
    fn a_fist_while_closing_is_ignored_and_opens_after() {
        let mut ptt = PushToTalk::default();
        hold(&mut ptt, true, 1.0);
        assert!(matches!(ptt.step(false, DT), Some(Event::Closed { .. })));
        // A new fist while the transcription runs: nothing.
        assert!(hold(&mut ptt, false, 0.2).is_empty());
        assert!(hold(&mut ptt, true, 1.0).is_empty());
        assert_eq!(ptt.window(), Window::Closing);
        // The transcription is back: the fist, still held, opens after the
        // dwell.
        ptt.finish();
        assert_eq!(ptt.window(), Window::Closed);
        assert_eq!(hold(&mut ptt, true, 0.5), vec![Event::Opened]);
    }

    #[test]
    fn the_file_path_closes_only_a_closed_window() {
        let mut ptt = PushToTalk::default();
        assert!(ptt.begin_closing());
        assert_eq!(ptt.window(), Window::Closing);
        assert!(hold(&mut ptt, true, 1.0).is_empty());
        ptt.finish();
        hold(&mut ptt, false, 0.1);
        hold(&mut ptt, true, 0.5);
        assert!(!ptt.begin_closing());
    }

    fn sine(freq: f32, rate: u32, seconds: f32) -> Vec<f32> {
        let n = (rate as f32 * seconds) as usize;
        (0..n)
            .map(|i| (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    /// Upward zero crossings per second.
    fn frequency(signal: &[f32], rate: u32) -> f32 {
        let ups = signal
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        ups as f32 * rate as f32 / signal.len() as f32
    }

    #[test]
    fn mono_16k_takes_a_third_of_48k_at_the_same_frequency() {
        let left = sine(440.0, 48_000, 1.0);
        let stereo: Vec<f32> = left.iter().flat_map(|&s| [s, s]).collect();
        let out = mono_16k(&stereo, 48_000);
        assert_eq!(out.len(), left.len() / 3);
        let f = frequency(&out, WHISPER_RATE);
        assert!((f - 440.0).abs() < 2.0, "{f}");
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.95 && peak <= 1.0, "{peak}");
    }

    #[test]
    fn mono_16k_averages_the_channels_and_passes_16k_through() {
        let stereo = [1.0, 0.0, 0.5, -0.5, -1.0, -1.0];
        assert_close!(mono_16k(&stereo, WHISPER_RATE), [0.5, 0.0, -1.0]);
        assert!(mono_16k(&[], 48_000).is_empty());
        // 44.1 kHz too: the length scales by the ratio.
        let stereo = vec![0.25; 44_100 * 2];
        let out = mono_16k(&stereo, 44_100);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|s| (s - 0.25).abs() < 1e-6));
    }

    #[test]
    fn short_clips_are_padded_to_the_minimum() {
        let mut clip = vec![0.5; 8000];
        pad_short(&mut clip);
        assert_eq!(clip.len(), 17_600);
        assert_close!(clip[7999], 0.5);
        assert_close!(clip[8000], 0.0);
        let mut long = vec![0.5; 48_000];
        pad_short(&mut long);
        assert_eq!(long.len(), 48_000);
    }

    #[test]
    fn audio_ctx_is_the_clip_plus_the_margin_within_the_ends() {
        assert_eq!(audio_ctx(3.0), 214);
        assert_eq!(audio_ctx(0.0), 64);
        assert_eq!(audio_ctx(-1.0), 64);
        assert_eq!(audio_ctx(f32::NAN), 64);
        assert_eq!(audio_ctx(2.61), 195);
        assert_eq!(audio_ctx(MAX_S), 364);
        assert_eq!(audio_ctx(29.0), 1500);
        assert_eq!(audio_ctx(60.0), 1500);
    }

    #[test]
    fn heard_line_quotes_the_text_or_says_nothing() {
        assert_eq!(
            heard_line(" Next effect. ", 412),
            "voice: heard \"Next effect.\" (412 ms)"
        );
        assert_eq!(heard_line("", 90), "voice: heard nothing");
        assert_eq!(heard_line("  ", 90), "voice: heard nothing");
        assert_eq!(heard_line("[BLANK_AUDIO]", 90), "voice: heard nothing");
        assert_eq!(heard_line(" (wind blowing)", 90), "voice: heard nothing");
        assert_eq!(label_text("[Music]"), NOTHING_LABEL);
        assert_eq!(label_text(" Edit the room. "), "Edit the room.");
        // Brackets inside a sentence are words.
        assert_eq!(spoken("[a] and [b]"), "[a] and [b]");
    }

    /// A WAV file: `fmt ` with these fields, a padded odd-length `LIST`
    /// chunk, then `data`.
    fn wav(channels: u16, rate: u32, bits: u16, samples: &[i16]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        b.extend_from_slice(&(channels * 2).to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(b"LIST");
        b.extend_from_slice(&3u32.to_le_bytes());
        b.extend_from_slice(b"abc\0");
        b.extend_from_slice(b"data");
        b.extend_from_slice(&(samples.len() as u32 * 2).to_le_bytes());
        for s in samples {
            b.extend_from_slice(&s.to_le_bytes());
        }
        let len = (b.len() - 8) as u32;
        b[4..8].copy_from_slice(&len.to_le_bytes());
        b
    }

    #[test]
    fn the_wav_reader_takes_16k_mono_16_bit() {
        let samples = parse_wav(&wav(1, 16_000, 16, &[0, 16384, -32768, 32767])).unwrap();
        assert_close!(samples, [0.0, 0.5, -1.0, 32767.0 / 32768.0]);
    }

    #[test]
    fn the_wav_reader_refuses_what_whisper_cannot_take() {
        assert!(parse_wav(&wav(2, 16_000, 16, &[0, 0])).is_err());
        assert!(parse_wav(&wav(1, 48_000, 16, &[0])).is_err());
        assert!(parse_wav(b"RIFF\0\0\0\0WAVEjunk").is_err());
        assert!(parse_wav(b"not a wav").is_err());
        let mut no_data = wav(1, 16_000, 16, &[]);
        no_data.truncate(no_data.len() - 8);
        assert!(parse_wav(&no_data).is_err());
    }
}
