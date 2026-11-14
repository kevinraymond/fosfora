//! Interface sounds (board #5460): short cues that tell the wearer a
//! press, a gesture or the voice window registered. Bare hands give no
//! click and no haptic, so a cue is how a wearer knows a press counted,
//! and it says the microphone is open to someone who can't see the label.
//!
//! **Synthesized, no files.** [`Bank::new`] makes the six cues once at
//! launch, mono at [`RATE`]: plucked notes from E major (a sine with a few
//! softer partials, a quick attack, an exponential decay, and a short fade
//! so each ends on zero). [`Cue`] has each one's notes, length and level.
//!
//! **Never in the analysis.** The music's output callback hands each frame
//! toward the analysis tap first and adds the cues to the speaker's frames
//! afterwards ([`output_block`]), so the tap holds the music alone and a
//! cue never flashes the room. While the analysis runs on the microphones,
//! which would hear a cue, the app sets the gain to 0 ([`gain`]).
//!
//! **Nothing on the audio thread waits.** The frame loop asks for a cue
//! with [`play`]: one bit per cue in an atomic, the time of the ask in
//! another. The callback swaps the bits out at its next buffer and plays
//! them from its own [`Mixer`], which allocates, locks and logs nothing. A
//! cue asked while no output stream runs is dropped, and so is one a
//! stream picks up more than [`MAX_AGE_MS`] after the ask, rather than
//! sounding late.
//!
//! **Under full scale.** While a cue sounds, each sum goes through a soft
//! knee above [`KNEE`] that never reaches past 1.0 ([`limit`]). The music
//! alone is left as it was.
//!
//! **Whenever a stream runs.** The cues ride the music's stream while it
//! plays and their own stream (`playback::CueOut`, at [`ALONE`] of the gain
//! with no music to mask them) while it is stopped.

use std::collections::VecDeque;
use std::f64::consts::{PI, TAU};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

/// The rate the cues are made at (Hz), the Quest 3's output rate. A stream
/// at another rate reads them resampled.
pub const RATE: u32 = 48_000;
/// The gain while `debug.fosfora.uisound` is unset: the press peaks at
/// -14 dBFS, the other cues at -16.5 dBFS, over the music.
pub const DEFAULT_GAIN: f32 = 0.5;
/// The cues' own stream plays them at this much of the gain (-6 dB): with
/// the music stopped nothing masks them.
pub const ALONE: f32 = 0.5;
/// Above this the sum of the music and a cue bends toward full scale.
pub const KNEE: f32 = 0.85;
/// A cue a stream picks up later than this after its ask is dropped (ms).
pub const MAX_AGE_MS: u64 = 300;
/// Cues one stream plays at once; a fifth takes the slot of the one
/// furthest along.
const VOICES: usize = 4;
/// Each cue's last this many seconds fade to zero.
const FADE_S: f64 = 0.004;

// The notes (Hz).
const B5: f32 = 987.77;
const E6: f32 = 1318.51;
const FS6: f32 = 1479.98;
const GS6: f32 = 1661.22;
const B6: f32 = 1975.53;

/// A sine with a little of its octave and twelfth.
const SOFT: &[(f32, f32)] = &[(1.0, 1.0), (2.0, 0.15), (3.0, 0.05)];
/// A sine alone: the duller timbre.
const PURE: &[(f32, f32)] = &[(1.0, 1.0)];
/// A sine with an inharmonic partial at 2.76 times, a woodblock's.
const WOOD: &[(f32, f32)] = &[(1.0, 1.0), (2.76, 0.3)];

/// One plucked note of a cue: where it starts and its pitch (s, Hz), its
/// attack and decay times (s), and its partials as (multiple of the pitch,
/// amplitude). A partial decays faster by its multiple.
struct Note {
    at: f32,
    hz: f32,
    attack: f32,
    decay: f32,
    partials: &'static [(f32, f32)],
}

const fn note(at: f32, hz: f32, attack: f32, decay: f32, partials: &'static [(f32, f32)]) -> Note {
    Note {
        at,
        hz,
        attack,
        decay,
        partials,
    }
}

/// A cue's length (ms), its peak at a gain of 1, and its notes.
struct Spec {
    ms: u32,
    level: f32,
    notes: &'static [Note],
}

/// Each cue's notes, length and level, in [`Cue::ALL`]'s order.
static SPECS: [Spec; 6] = [
    // Press: a woodblock tick.
    Spec {
        ms: 30,
        level: 0.4,
        notes: &[note(0.0, B6, 0.0005, 0.006, WOOD)],
    },
    // Listening: rising.
    Spec {
        ms: 120,
        level: 0.3,
        notes: &[
            note(0.0, E6, 0.003, 0.025, SOFT),
            note(0.05, B6, 0.003, 0.025, SOFT),
        ],
    },
    // Closed: falling.
    Spec {
        ms: 120,
        level: 0.3,
        notes: &[
            note(0.0, B6, 0.003, 0.025, SOFT),
            note(0.05, E6, 0.003, 0.025, SOFT),
        ],
    },
    // Understood.
    Spec {
        ms: 160,
        level: 0.3,
        notes: &[note(0.0, GS6, 0.004, 0.05, SOFT)],
    },
    // Missed: lower, a sine alone, a softer attack.
    Spec {
        ms: 180,
        level: 0.3,
        notes: &[note(0.0, B5, 0.01, 0.055, PURE)],
    },
    // Armed: the same note twice.
    Spec {
        ms: 110,
        level: 0.3,
        notes: &[
            note(0.0, FS6, 0.002, 0.018, SOFT),
            note(0.06, FS6, 0.002, 0.018, SOFT),
        ],
    },
];

/// The interface sounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// A menu press, or a thumb swipe or an editor pinch that stepped
    /// something: a 30 ms B6 tick with a woodblock partial.
    Press,
    /// The voice window opened: E6 then B6, rising, 120 ms.
    Listening,
    /// The voice window closed: B6 then E6, falling, 120 ms.
    Closed,
    /// A sentence was understood, or the editor's second hold applied: a
    /// soft G#6, 160 ms.
    Understood,
    /// A sentence was not understood: a lower, duller B5 (a sine alone, a
    /// slower attack), 180 ms.
    Missed,
    /// The room editor armed its class cycle (a second hold applies it):
    /// F#6 twice, 110 ms.
    Armed,
}

impl Cue {
    /// Every cue, in the bank's order.
    pub const ALL: [Self; 6] = [
        Self::Press,
        Self::Listening,
        Self::Closed,
        Self::Understood,
        Self::Missed,
        Self::Armed,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn bit(self) -> u32 {
        1 << self.index()
    }

    /// For the log.
    pub fn name(self) -> &'static str {
        match self {
            Self::Press => "press",
            Self::Listening => "listening",
            Self::Closed => "closed",
            Self::Understood => "understood",
            Self::Missed => "missed",
            Self::Armed => "armed",
        }
    }

    fn spec(self) -> &'static Spec {
        &SPECS[self.index()]
    }

    /// The cue's samples at `rate`, mono, its peak at its level.
    fn synth(self, rate: u32) -> Vec<f32> {
        let spec = self.spec();
        let rate_f = f64::from(rate.max(1));
        let n = (u64::from(spec.ms) * u64::from(rate) / 1000) as usize;
        let mut out = vec![0.0f64; n];
        for note in spec.notes {
            let start = (f64::from(note.at) * rate_f).round() as usize;
            let (attack, decay) = (f64::from(note.attack), f64::from(note.decay));
            for (k, s) in out.iter_mut().enumerate().skip(start) {
                let t = (k - start) as f64 / rate_f;
                let rise = if t < attack {
                    0.5 - 0.5 * (PI * t / attack).cos()
                } else {
                    1.0
                };
                let v: f64 = note
                    .partials
                    .iter()
                    .map(|&(mult, amp)| {
                        let m = f64::from(mult);
                        f64::from(amp)
                            * (-t * m / decay).exp()
                            * (TAU * f64::from(note.hz) * m * t).sin()
                    })
                    .sum();
                *s += rise * v;
            }
        }
        // The tail fades to zero, the last sample exactly.
        let fade = ((FADE_S * rate_f) as usize).clamp(1, n.max(1));
        for k in 0..fade.min(n) {
            out[n - 1 - k] *= 0.5 - 0.5 * (PI * k as f64 / fade as f64).cos();
        }
        let peak = out.iter().fold(0.0f64, |m, s| m.max(s.abs()));
        let scale = if peak > 0.0 {
            f64::from(spec.level) / peak
        } else {
            0.0
        };
        out.iter().map(|s| (s * scale) as f32).collect()
    }
}

/// The cues' samples, made once at launch.
pub struct Bank {
    rate: u32,
    cues: [Vec<f32>; Cue::ALL.len()],
}

impl Bank {
    /// Every cue at `rate` (Hz).
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            cues: Cue::ALL.map(|c| c.synth(rate)),
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// `cue`'s samples, mono.
    pub fn samples(&self, cue: Cue) -> &[f32] {
        &self.cues[cue.index()]
    }
}

/// The time the cues' asks are stamped with: set when the bank is
/// installed, before any stream runs, so the audio thread only reads it.
static EPOCH: OnceLock<Instant> = OnceLock::new();

/// Milliseconds on the cues' clock.
pub fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// What the frame loop shares with the output callbacks: the cues asked
/// for, the gain, how many streams mix the cues, and the bank. Atomics
/// only, and the bank is set once before any stream starts.
pub struct Sounds {
    /// One bit per cue asked for and not yet picked up.
    asked: AtomicU32,
    /// When the last cue was asked for ([`now_ms`]).
    asked_at: AtomicU64,
    /// The gain, `f32` bits.
    gain: AtomicU32,
    /// Output streams that mix the cues, running now.
    streams: AtomicU32,
    bank: OnceLock<Bank>,
}

impl Default for Sounds {
    fn default() -> Self {
        Self::new()
    }
}

impl Sounds {
    pub const fn new() -> Self {
        Self {
            asked: AtomicU32::new(0),
            asked_at: AtomicU64::new(0),
            gain: AtomicU32::new(DEFAULT_GAIN.to_bits()),
            streams: AtomicU32::new(0),
            bank: OnceLock::new(),
        }
    }

    /// The cues to play; the first bank installed stays. Also starts the
    /// clock, so no callback is the first to read it.
    pub fn install(&self, bank: Bank) {
        let _ = now_ms();
        let _ = self.bank.set(bank);
    }

    pub fn bank(&self) -> Option<&Bank> {
        self.bank.get()
    }

    /// Ask for `cue` now; false when it was dropped ([`Sounds::play_at`]).
    pub fn play(&self, cue: Cue) -> bool {
        self.play_at(cue, now_ms())
    }

    /// Ask for `cue` at `now_ms`: the next buffer of a running stream
    /// plays it. With no stream running it is dropped (false), so it can't
    /// sound late when one starts.
    pub fn play_at(&self, cue: Cue, now_ms: u64) -> bool {
        if self.streams.load(Ordering::Acquire) == 0 {
            return false;
        }
        self.asked_at.store(now_ms, Ordering::Relaxed);
        self.asked.fetch_or(cue.bit(), Ordering::Release);
        true
    }

    /// The cues asked for since the last take, none if they are older
    /// than [`MAX_AGE_MS`] at `now_ms`.
    fn take(&self, now_ms: u64) -> u32 {
        let bits = self.asked.swap(0, Ordering::Acquire);
        let at = self.asked_at.load(Ordering::Relaxed);
        if bits == 0 || now_ms.saturating_sub(at) > MAX_AGE_MS {
            0
        } else {
            bits
        }
    }

    /// The gain, 0 to 1 (0 silences the cues).
    pub fn set_gain(&self, gain: f32) {
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    /// A stream that mixes the cues is starting: they are asked for while
    /// the returned guard lives.
    pub fn live(&self) -> Live<'_> {
        self.streams.fetch_add(1, Ordering::AcqRel);
        Live(self)
    }
}

/// A running stream that mixes the cues ([`Sounds::live`]).
pub struct Live<'a>(&'a Sounds);

impl Drop for Live<'_> {
    fn drop(&mut self) {
        self.0.streams.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The app's cues.
pub static SOUNDS: Sounds = Sounds::new();

/// Ask for `cue` on the app's speakers (from the frame loop; logged at the
/// debug level, so an unworn run shows which cues fired).
pub fn play(cue: Cue) {
    if SOUNDS.play(cue) {
        log::debug!("ui sound: {}", cue.name());
    } else {
        log::debug!("ui sound: {} dropped, no stream runs", cue.name());
    }
}

/// A cue playing: which, and where, in bank frames (32.32 fixed point).
#[derive(Debug, Clone, Copy)]
struct Voice {
    cue: Cue,
    pos: u64,
}

/// One stream's cues in flight. The callback owns it.
pub struct Mixer {
    voices: [Option<Voice>; VOICES],
    /// This stream's share of the gain ([`ALONE`] on the cues' own).
    scale: f32,
}

impl Mixer {
    pub const fn new(scale: f32) -> Self {
        Self {
            voices: [None; VOICES],
            scale,
        }
    }

    /// Whether a cue is playing.
    pub fn sounding(&self) -> bool {
        self.voices.iter().any(Option::is_some)
    }

    fn start(&mut self, cue: Cue) {
        let slot = match self.voices.iter().position(Option::is_none) {
            Some(free) => free,
            None => self
                .voices
                .iter()
                .enumerate()
                .max_by_key(|(_, v)| v.map_or(0, |v| v.pos))
                .map_or(0, |(i, _)| i),
        };
        self.voices[slot] = Some(Voice { cue, pos: 0 });
    }

    /// Start the cues asked for since the last buffer, and add the playing
    /// ones to `out` (`channels` interleaved at `out_rate`), each frame a
    /// cue sounds in through [`limit`]: on the first channel of a mono
    /// stream, the first two of any other. Frames after the last cue ends
    /// are left as they were. No allocation, lock or log.
    pub fn mix(
        &mut self,
        sounds: &Sounds,
        out: &mut [f32],
        channels: usize,
        out_rate: u32,
        now_ms: u64,
    ) {
        let asked = sounds.take(now_ms);
        let Some(bank) = sounds.bank() else {
            return;
        };
        let gain = sounds.gain() * self.scale;
        if gain <= 0.0 || channels == 0 {
            // Silenced: what was playing stops, and what was asked for
            // is not kept for later.
            self.voices = [None; VOICES];
            return;
        }
        for cue in Cue::ALL {
            if asked & cue.bit() != 0 {
                self.start(cue);
            }
        }
        if !self.sounding() {
            return;
        }
        let step = (f64::from(bank.rate()) / f64::from(out_rate.max(1)) * 4_294_967_296.0) as u64;
        let used = channels.min(2);
        for frame in out.chunks_exact_mut(channels) {
            let mut sum = 0.0f32;
            let mut any = false;
            for slot in &mut self.voices {
                let Some(v) = slot.as_mut() else {
                    continue;
                };
                let table = bank.samples(v.cue);
                let i = (v.pos >> 32) as usize;
                if i >= table.len() {
                    *slot = None;
                    continue;
                }
                let frac = (v.pos & 0xffff_ffff) as f32 / 4_294_967_296.0;
                let a = table[i];
                let b = table.get(i + 1).copied().unwrap_or(0.0);
                sum += a + (b - a) * frac;
                v.pos += step;
                any = true;
            }
            if !any {
                break;
            }
            let s = sum * gain;
            for x in &mut frame[..used] {
                *x = limit(*x + s);
            }
        }
    }
}

/// `x` through the soft knee: unchanged up to [`KNEE`], bent above it
/// toward full scale, which it never passes.
pub fn limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh())
            .min(1.0)
            .copysign(x)
    }
}

/// One buffer of the music's output callback, in the order that keeps the
/// analysis clean: each frame `next` gives (the clip's stereo) goes to the
/// speaker's `out` (averaged on a mono stream, the extra channels of a
/// wider one silent) and, as it was, into `line`, the delay line toward
/// the tap, while it holds fewer than `cap` samples. Only then does
/// `mixer` add the cues to `out`. Nothing reads `out` back, so the tap
/// gets the music alone.
pub fn output_block(
    out: &mut [f32],
    channels: usize,
    line: &mut VecDeque<f32>,
    cap: usize,
    mut next: impl FnMut() -> (f32, f32),
    mixer: &mut Mixer,
    sounds: &Sounds,
    out_rate: u32,
    now_ms: u64,
) {
    if channels == 0 {
        return;
    }
    for frame in out.chunks_exact_mut(channels) {
        let (l, r) = next();
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
        if line.len() < cap {
            line.push_back(l);
            line.push_back(r);
        }
    }
    mixer.mix(sounds, out, channels, out_rate, now_ms);
}

/// The cues' gain: `debug.fosfora.uisound` (`knob`) from 0 to 1, clamped,
/// [`DEFAULT_GAIN`] while it is unset or not a number, and 0 while the
/// analysis is on the microphones (`mic`), which would hear the cues.
pub fn gain(knob: Option<&str>, mic: bool) -> f32 {
    if mic {
        return 0.0;
    }
    knob_value(knob).map_or(DEFAULT_GAIN, |g| g.clamp(0.0, 1.0))
}

fn knob_value(knob: Option<&str>) -> Option<f32> {
    knob.and_then(|v| v.trim().parse::<f32>().ok())
        .filter(|g| g.is_finite())
}

/// The log line for the gain [`gain`] gives.
pub fn gain_line(knob: Option<&str>, mic: bool) -> String {
    let g = gain(knob, mic);
    if mic {
        "ui sound: off while the analysis is on the microphones".to_owned()
    } else if knob_value(knob).is_some() {
        format!("ui sound: gain {g:.2} (debug.fosfora.uisound)")
    } else {
        format!("ui sound: gain {g:.2} (default)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bank-installed `Sounds` with one stream running.
    fn sounds() -> (&'static Sounds, Live<'static>) {
        let s: &'static Sounds = Box::leak(Box::new(Sounds::new()));
        s.install(Bank::new(RATE));
        let live = s.live();
        (s, live)
    }

    /// A clip for the music: two detuned sines, stereo, at `level`.
    fn music(frames: usize, level: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let t = i as f32 / RATE as f32;
                [
                    level * (TAU as f32 * 220.0 * t).sin(),
                    level * (TAU as f32 * 331.0 * t).sin(),
                ]
            })
            .collect()
    }

    /// The music callback run over `clip` in `block`-frame buffers at
    /// `channels`, `cue` asked for before the first: the speaker's frames
    /// and what went into the delay line toward the tap.
    fn run(
        sounds: &Sounds,
        clip: &[f32],
        channels: usize,
        block: usize,
        cue: Option<Cue>,
    ) -> (Vec<f32>, Vec<f32>) {
        let mut mixer = Mixer::new(1.0);
        let mut line = VecDeque::new();
        let mut speaker = Vec::new();
        let frames = clip.len() / 2;
        let mut at = 0;
        if let Some(c) = cue {
            sounds.play_at(c, 0);
        }
        while at < frames {
            let n = block.min(frames - at);
            let mut out = vec![0.0f32; n * channels];
            let next = || {
                let f = (clip[2 * at], clip[2 * at + 1]);
                at += 1;
                f
            };
            output_block(
                &mut out,
                channels,
                &mut line,
                usize::MAX,
                next,
                &mut mixer,
                sounds,
                RATE,
                0,
            );
            speaker.extend_from_slice(&out);
        }
        (speaker, line.into_iter().collect())
    }

    fn bits(v: &[f32]) -> Vec<u32> {
        v.iter().map(|x| x.to_bits()).collect()
    }

    #[test]
    fn each_cue_has_its_length_and_peak() {
        let bank = Bank::new(RATE);
        let want: [(Cue, usize, f32); 6] = [
            (Cue::Press, 30, 0.4),
            (Cue::Listening, 120, 0.3),
            (Cue::Closed, 120, 0.3),
            (Cue::Understood, 160, 0.3),
            (Cue::Missed, 180, 0.3),
            (Cue::Armed, 110, 0.3),
        ];
        for (cue, ms, level) in want {
            let s = bank.samples(cue);
            assert_eq!(s.len(), ms * 48, "{}", cue.name());
            let peak = s.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert_close!(peak, level, "{}", cue.name());
            // Starts and ends on silence: no click at either end.
            assert!(s[0].abs() < 1e-3, "{}: {}", cue.name(), s[0]);
            assert_eq!(s[s.len() - 1].abs().to_bits(), 0, "{}", cue.name());
        }
        // At the default gain the press peaks at -14 dBFS, the others at
        // -16.5.
        for cue in Cue::ALL {
            let peak = bank.samples(cue).iter().fold(0.0f32, |m, x| m.max(x.abs())) * DEFAULT_GAIN;
            assert!(
                (0.15..=0.2 + 1e-6).contains(&peak),
                "{}: {peak}",
                cue.name()
            );
        }
    }

    #[test]
    fn the_cues_differ_where_they_should() {
        let bank = Bank::new(RATE);
        // Zero crossings per second, near the pitch of a note.
        let rate_of = |s: &[f32]| {
            let crossings = s
                .windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count();
            crossings as f32 / 2.0 / (s.len() as f32 / RATE as f32)
        };
        // The window's cues: E6 up to B6 when it opens, down when it
        // closes (the first 40 ms against 60 to 100 ms).
        let (head, tail) = (0..40 * 48, 60 * 48..100 * 48);
        let open = bank.samples(Cue::Listening);
        let closed = bank.samples(Cue::Closed);
        let (lo, hi) = (rate_of(&open[head.clone()]), rate_of(&open[tail.clone()]));
        assert!(
            (lo - E6).abs() < 40.0 && (hi - B6).abs() < 40.0,
            "{lo} -> {hi}"
        );
        let (hi, lo) = (rate_of(&closed[head]), rate_of(&closed[tail]));
        assert!(
            (hi - B6).abs() < 40.0 && (lo - E6).abs() < 40.0,
            "{hi} -> {lo}"
        );
        // The miss sits lower than the understood note.
        let (yes, no) = (
            rate_of(bank.samples(Cue::Understood)),
            rate_of(bank.samples(Cue::Missed)),
        );
        assert!(no < yes * 0.7, "missed {no} Hz, understood {yes} Hz");
    }

    #[test]
    fn the_frames_toward_the_tap_are_bit_identical_with_a_cue() {
        let (sounds, _live) = sounds();
        let clip = music(RATE as usize / 2, 0.3);
        for channels in [1, 2, 4] {
            for cue in Cue::ALL {
                let (_, quiet_tap) = run(sounds, &clip, channels, 192, None);
                let (_, cued_tap) = run(sounds, &clip, channels, 192, Some(cue));
                assert_eq!(quiet_tap.len(), clip.len());
                assert_eq!(
                    bits(&quiet_tap),
                    bits(&cued_tap),
                    "{} at {channels} ch",
                    cue.name()
                );
                // And the tap holds the clip exactly.
                assert_eq!(bits(&cued_tap), bits(&clip));
            }
        }
    }

    #[test]
    fn the_speaker_frames_carry_the_cue_and_only_while_it_plays() {
        let (sounds, _live) = sounds();
        let bank = sounds.bank().unwrap();
        let clip = music(RATE as usize / 2, 0.3);
        for channels in [1, 2, 4] {
            for cue in Cue::ALL {
                let (quiet, _) = run(sounds, &clip, channels, 192, None);
                let (cued, _) = run(sounds, &clip, channels, 192, Some(cue));
                let table = bank.samples(cue);
                let gain = sounds.gain();
                for (k, (q, c)) in quiet
                    .chunks_exact(channels)
                    .zip(cued.chunks_exact(channels))
                    .enumerate()
                {
                    let added = table.get(k).copied().unwrap_or(0.0) * gain;
                    // The music stays under the knee, so the sum is plain.
                    assert_close!(c[0] - q[0], added, "{} frame {k}", cue.name());
                    if channels > 1 {
                        assert_close!(c[1] - q[1], added, "{} frame {k}", cue.name());
                    }
                    for ch in 2..channels {
                        assert_eq!(c[ch].to_bits(), 0.0f32.to_bits());
                    }
                    if k >= table.len() {
                        assert_eq!(bits(c), bits(q), "{} after its end", cue.name());
                    }
                }
            }
        }
    }

    #[test]
    fn the_sum_never_passes_full_scale() {
        let (sounds, _live) = sounds();
        sounds.set_gain(1.0);
        // A full-scale square and one with overs (a decoder can give them).
        for level in [1.0f32, 1.5] {
            let clip: Vec<f32> = (0..RATE as usize / 4)
                .flat_map(|i| {
                    let v = if (i / 40) % 2 == 0 { level } else { -level };
                    [v, -v]
                })
                .collect();
            for cue in Cue::ALL {
                let (out, tap) = run(sounds, &clip, 2, 192, Some(cue));
                let len = sounds.bank().unwrap().samples(cue).len();
                // While the cue plays, nothing passes full scale.
                assert!(
                    out[..len * 2].iter().all(|x| x.abs() <= 1.0),
                    "{} over {level}",
                    cue.name()
                );
                assert_eq!(bits(&tap), bits(&clip));
            }
        }
        assert_close!(limit(0.5), 0.5);
        assert_close!(limit(-KNEE), -KNEE);
        assert!(limit(1.2) < 1.0 && limit(1.2) > KNEE);
        assert!(limit(-100.0) >= -1.0);
        assert!(limit(f32::MAX) <= 1.0);
    }

    #[test]
    fn a_cue_runs_across_buffers_and_at_another_rate() {
        let (sounds, _live) = sounds();
        let len = sounds.bank().unwrap().samples(Cue::Understood).len();
        // Small buffers: the cue spans dozens of callbacks, same frames.
        let clip = vec![0.0f32; RATE as usize / 2 * 2];
        let (small, _) = run(sounds, &clip, 2, 64, Some(Cue::Understood));
        let (large, _) = run(sounds, &clip, 2, 4096, Some(Cue::Understood));
        assert_eq!(bits(&small), bits(&large));
        let last = small.chunks_exact(2).rposition(|f| f[0] != 0.0).unwrap();
        assert!(last < len && last > len - 300, "{last} of {len}");
        // A stream at 44.1 kHz reads the cue resampled to the same length
        // in time.
        let mut mixer = Mixer::new(1.0);
        let mut out = vec![0.0f32; 44_100];
        sounds.play_at(Cue::Understood, 0);
        mixer.mix(sounds, &mut out, 1, 44_100, 0);
        let last = out.iter().rposition(|x| *x != 0.0).unwrap();
        let want = len * 44_100 / 48_000;
        assert!(last.abs_diff(want) < 300, "{last} vs {want}");
    }

    #[test]
    fn several_cues_at_once_all_play() {
        let (sounds, _live) = sounds();
        let bank = sounds.bank().unwrap();
        let mut mixer = Mixer::new(1.0);
        sounds.play_at(Cue::Press, 0);
        sounds.play_at(Cue::Armed, 0);
        let mut out = vec![0.0f32; 2000];
        mixer.mix(sounds, &mut out, 1, RATE, 0);
        let g = sounds.gain();
        for (k, x) in out.iter().enumerate().take(1000) {
            let want = (bank.samples(Cue::Press).get(k).copied().unwrap_or(0.0)
                + bank.samples(Cue::Armed)[k])
                * g;
            assert_close!(*x, want, "frame {k}");
        }
        // A cue asked again while it plays starts a second voice.
        let mut mixer = Mixer::new(1.0);
        sounds.play_at(Cue::Press, 0);
        let mut out = vec![0.0f32; 100];
        mixer.mix(sounds, &mut out, 1, RATE, 0);
        sounds.play_at(Cue::Press, 0);
        let mut out = vec![0.0f32; 100];
        mixer.mix(sounds, &mut out, 1, RATE, 0);
        assert_eq!(mixer.voices.iter().flatten().count(), 2);
    }

    #[test]
    fn gain_zero_silences_and_drops_what_was_asked() {
        let (sounds, _live) = sounds();
        let clip = music(4800, 0.3);
        sounds.set_gain(0.0);
        let (quiet, _) = run(sounds, &clip, 2, 192, None);
        let (cued, _) = run(sounds, &clip, 2, 192, Some(Cue::Listening));
        assert_eq!(bits(&quiet), bits(&cued));
        // Asked while silenced, it does not play once the gain is back.
        sounds.play_at(Cue::Press, 0);
        let mut mixer = Mixer::new(1.0);
        let mut out = vec![0.0f32; 512];
        mixer.mix(sounds, &mut out, 2, RATE, 0);
        sounds.set_gain(0.5);
        mixer.mix(sounds, &mut out, 2, RATE, 0);
        assert!(out.iter().all(|x| x.to_bits() == 0));
        // A cue playing when the gain goes to 0 stops.
        sounds.play_at(Cue::Understood, 0);
        mixer.mix(sounds, &mut out, 2, RATE, 0);
        assert!(mixer.sounding());
        sounds.set_gain(0.0);
        mixer.mix(sounds, &mut out, 2, RATE, 0);
        assert!(!mixer.sounding());
    }

    #[test]
    fn the_cues_own_stream_plays_them_at_half() {
        let (sounds, _live) = sounds();
        let mut music_mix = Mixer::new(1.0);
        let mut alone_mix = Mixer::new(ALONE);
        let mut a = vec![0.0f32; 1000];
        let mut b = vec![0.0f32; 1000];
        sounds.play_at(Cue::Press, 0);
        music_mix.mix(sounds, &mut a, 1, RATE, 0);
        sounds.play_at(Cue::Press, 0);
        alone_mix.mix(sounds, &mut b, 1, RATE, 0);
        for (x, y) in a.iter().zip(&b) {
            assert_close!(*y, *x * ALONE);
        }
    }

    #[test]
    fn a_cue_with_no_stream_or_picked_up_late_is_dropped() {
        let s: &'static Sounds = Box::leak(Box::new(Sounds::new()));
        s.install(Bank::new(RATE));
        // No stream running: the ask goes nowhere, and a stream that
        // starts later does not play it.
        assert!(!s.play_at(Cue::Missed, 0));
        let live = s.live();
        let mut mixer = Mixer::new(1.0);
        let mut out = vec![0.0f32; 256];
        mixer.mix(s, &mut out, 2, RATE, 0);
        assert!(!mixer.sounding());
        // Running, but the buffer comes too late: dropped.
        s.play_at(Cue::Missed, 1_000);
        mixer.mix(s, &mut out, 2, RATE, 1_000 + MAX_AGE_MS + 1);
        assert!(!mixer.sounding());
        // In time: plays.
        s.play_at(Cue::Missed, 2_000);
        mixer.mix(s, &mut out, 2, RATE, 2_000 + MAX_AGE_MS);
        assert!(mixer.sounding());
        drop(live);
        assert!(!s.play_at(Cue::Press, 3_000));
        assert_eq!(s.take(3_000), 0);
    }

    #[test]
    fn the_knob_is_a_gain_from_0_to_1() {
        assert_close!(gain(None, false), DEFAULT_GAIN);
        assert_close!(gain(Some("0"), false), 0.0);
        assert_close!(gain(Some("1"), false), 1.0);
        assert_close!(gain(Some(" 0.25 "), false), 0.25);
        assert_close!(gain(Some("3"), false), 1.0);
        assert_close!(gain(Some("-1"), false), 0.0);
        assert_close!(gain(Some("loud"), false), DEFAULT_GAIN);
        assert_close!(gain(Some("NaN"), false), DEFAULT_GAIN);
        // The microphones would hear the cues.
        assert_close!(gain(None, true), 0.0);
        assert_close!(gain(Some("1"), true), 0.0);
        assert_eq!(gain_line(None, false), "ui sound: gain 0.50 (default)");
        assert_eq!(
            gain_line(Some("0.8"), false),
            "ui sound: gain 0.80 (debug.fosfora.uisound)"
        );
        assert_eq!(
            gain_line(Some("0.8"), true),
            "ui sound: off while the analysis is on the microphones"
        );
    }

    /// The cues to listen to without the headset: each at the default gain
    /// as it plays over the music, then at [`ALONE`] of it as the cues' own
    /// stream plays it, 0.6 s apart, as a 48 kHz 16-bit WAV.
    #[test]
    #[ignore = "writes a file: UI_SOUND_WAV=<file.wav>"]
    fn write_the_cues_to_ui_sound_wav() {
        let path = std::env::var("UI_SOUND_WAV")
            .unwrap_or_else(|_| panic!("set UI_SOUND_WAV to the file to write"));
        let bank = Bank::new(RATE);
        let gap = vec![0.0f32; RATE as usize * 6 / 10];
        let mut mono = gap.clone();
        for scale in [1.0, ALONE] {
            for cue in Cue::ALL {
                mono.extend(bank.samples(cue).iter().map(|s| s * DEFAULT_GAIN * scale));
                mono.extend_from_slice(&gap);
            }
        }
        let data: Vec<u8> = mono
            .iter()
            .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
            .collect();
        let mut wav = Vec::with_capacity(44 + data.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&RATE.to_le_bytes());
        wav.extend_from_slice(&(RATE * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        std::fs::write(&path, wav).unwrap_or_else(|e| panic!("{path}: {e}"));
        println!(
            "{path}: {}, at gain {DEFAULT_GAIN} then {ALONE} of it",
            Cue::ALL.map(Cue::name).join(", ")
        );
    }
}
