//! The Signal emitter: frames in, `/fosfora/v1/` messages out.
//!
//! Consumes `AudioFrame`s at analysis rate (the sole consumer in headless mode —
//! no interpolator, no PLLs) and emits events the hop they fire, continuous
//! groups decimated on the sample clock, and on-change addresses immediately plus
//! a 1 Hz re-broadcast for late-joining receivers. No wall clock anywhere, so an
//! offline run over the same audio is bit-for-bit deterministic.

use rosc::OscType;

use crate::audio::AudioFrame;
use crate::audio::features::AudioFeatures;

use super::phrase::PhraseTracker;
use super::predict::DropPredictor;
use super::schema;
use super::section::{HeuristicSectionEstimator, SectionEstimator, SectionLabel};
use super::sink::SignalSink;

/// Derived onset edge: the SuperFlux envelope only jumps upward like this on a
/// real onset ([`crate::audio::beat::OnsetDetector`] exposes no boolean without an
/// ABI change, so the emitter re-derives one — documented in SIGNAL.md).
const ONSET_EDGE_DELTA: f32 = 0.20;
const ONSET_MIN_GAP_SECS: f64 = 0.06;
/// Kick-band onset hysteresis (the drums stem-proxy onset).
const KICK_ENTER: f32 = 0.5;
const KICK_REARM: f32 = 0.3;
const KICK_MIN_GAP_SECS: f64 = 0.10;
/// Key changes are only announced above this confidence.
const KEY_MIN_CONFIDENCE: f32 = 0.3;
/// On-change addresses are re-broadcast this often for late joiners.
const ANNOUNCE_INTERVAL_SECS: f64 = 1.0;

const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

#[derive(Debug, Clone, Copy)]
pub struct EmitCfg {
    pub tx_rate_hz: u32,
    pub feat_bus: bool,
    pub stems: bool,
}

pub struct SignalEmitter {
    cfg: EmitCfg,
    tx_interval: f64,
    next_tx: f64,

    section: Box<dyn SectionEstimator>,
    phrase: PhraseTracker,
    predict: DropPredictor,

    beat_total: u32,
    bar_total: u32,
    drop_total: u32,

    prev_onset: f32,
    last_onset_ts: f64,
    kick_armed: bool,
    last_kick_ts: f64,

    last_key: Option<(usize, bool)>,
    last_key_conf: f32,
    last_section: Option<SectionLabel>,
    last_section_conf: f32,
    last_phrase_len: Option<u32>,
    last_phrase_conf: f32,
    last_announce_ts: f64,

    feat_addrs: Vec<String>,
}

impl SignalEmitter {
    pub fn new(cfg: EmitCfg) -> Self {
        Self::with_estimator(cfg, Box::new(HeuristicSectionEstimator::new()))
    }

    pub fn with_estimator(cfg: EmitCfg, section: Box<dyn SectionEstimator>) -> Self {
        Self {
            cfg,
            tx_interval: 1.0 / f64::from(cfg.tx_rate_hz.clamp(1, 86)),
            next_tx: 0.0,
            section,
            phrase: PhraseTracker::new(),
            predict: DropPredictor::new(),
            beat_total: 0,
            bar_total: 0,
            drop_total: 0,
            prev_onset: 0.0,
            last_onset_ts: f64::NEG_INFINITY,
            kick_armed: true,
            last_kick_ts: f64::NEG_INFINITY,
            last_key: None,
            last_key_conf: 0.0,
            last_section: None,
            last_section_conf: 0.0,
            last_phrase_len: None,
            last_phrase_conf: 0.0,
            last_announce_ts: f64::NEG_INFINITY,
            feat_addrs: schema::feat_addresses(),
        }
    }

    /// Running (beat, downbeat, drop) totals — reconciled against
    /// [`crate::audio::PulseCounts`] by the live loop to detect frame loss.
    pub fn totals(&self) -> (u32, u32, u32) {
        (self.beat_total, self.bar_total, self.drop_total)
    }

    pub fn tier(&self) -> &'static str {
        self.section.tier()
    }

    /// Feed one analysis frame, in hop order.
    pub fn process_frame(&mut self, frame: &AudioFrame, sink: &mut dyn SignalSink) {
        let f = &frame.features;
        let ts = frame.timestamp;

        // ---- Events: emitted the hop they fire. Counts as args, so a receiver
        // can detect its own datagram loss from a gap.
        // Q1: /beat and /downbeat carry the beat's event time, which may sit before
        // this hop's timestamp once the scheduler fires at predicted instants.
        let beat_ts = frame.beat_time.unwrap_or(ts);
        if f.beat > 0.5 {
            self.beat_total += 1;
            sink.emit(beat_ts, schema::BEAT, &[OscType::Int(self.beat_total as i32)]);
        }
        if f.downbeat > 0.5 {
            self.bar_total += 1;
            sink.emit(beat_ts, schema::DOWNBEAT, &[OscType::Int(self.bar_total as i32)]);
        }
        if f.drop > 0.5 {
            self.drop_total += 1;
            sink.emit(ts, schema::DROP, &[OscType::Int(self.drop_total as i32)]);
            self.phrase.on_drop();
        }
        if f.onset - self.prev_onset >= ONSET_EDGE_DELTA
            && ts - self.last_onset_ts >= ONSET_MIN_GAP_SECS
        {
            self.last_onset_ts = ts;
            sink.emit(ts, schema::ONSET, &[OscType::Float(f.onset)]);
        }
        self.prev_onset = f.onset;
        if self.cfg.stems {
            if f.kick < KICK_REARM {
                self.kick_armed = true;
            }
            if self.kick_armed
                && f.kick >= KICK_ENTER
                && ts - self.last_kick_ts >= KICK_MIN_GAP_SECS
            {
                self.kick_armed = false;
                self.last_kick_ts = ts;
                sink.emit(ts, schema::STEM_DRUMS_ONSET, &[OscType::Float(f.kick)]);
            }
        }

        // ---- On change (+ 1 Hz re-broadcast below).
        let state = self.section.process(f, ts);
        if self.last_section != Some(state.label) {
            self.last_section = Some(state.label);
            self.last_section_conf = state.confidence;
            emit_section(sink, ts, state.label, state.confidence);
            self.phrase.on_section_change();
        } else {
            self.last_section_conf = state.confidence;
        }

        if f.key_confidence >= KEY_MIN_CONFIDENCE {
            let tonic = ((f.key_class * 11.0).round() as usize).min(11);
            let minor = f.key_is_minor > 0.5;
            if self.last_key != Some((tonic, minor)) {
                self.last_key = Some((tonic, minor));
                self.last_key_conf = f.key_confidence;
                emit_key(sink, ts, tonic, minor, f.key_confidence);
            } else {
                self.last_key_conf = f.key_confidence;
            }
        }

        let ph = self.phrase.process(f, ts);
        let predict = self.predict.process(f, &ph, ts);
        if self.last_phrase_len != Some(ph.len) {
            self.last_phrase_len = Some(ph.len);
            self.last_phrase_conf = ph.len_confidence;
            emit_phrase_len(sink, ts, ph.len, ph.len_confidence);
        } else {
            self.last_phrase_conf = ph.len_confidence;
        }

        // ---- 1 Hz re-broadcast of the on-change state, for late joiners.
        if ts - self.last_announce_ts >= ANNOUNCE_INTERVAL_SECS {
            self.last_announce_ts = ts;
            if let Some(label) = self.last_section {
                emit_section(sink, ts, label, self.last_section_conf);
            }
            if let Some((tonic, minor)) = self.last_key {
                emit_key(sink, ts, tonic, minor, self.last_key_conf);
            }
            if let Some(len) = self.last_phrase_len {
                emit_phrase_len(sink, ts, len, self.last_phrase_conf);
            }
        }

        // ---- Continuous group, decimated on the sample clock.
        if ts >= self.next_tx {
            // One message per tick even if frames stall briefly; never a burst.
            self.next_tx = (self.next_tx + self.tx_interval).max(ts - self.tx_interval);
            self.emit_continuous(ts, f, &ph, predict, sink);
        }
    }

    fn emit_continuous(
        &mut self,
        ts: f64,
        f: &AudioFeatures,
        ph: &super::phrase::PhraseState,
        predict: f32,
        sink: &mut dyn SignalSink,
    ) {
        sink.emit(ts, schema::BPM, &[OscType::Float(f.raw_bpm())]);
        sink.emit(ts, schema::BAR_PHASE, &[OscType::Float(f.bar_phase)]);
        sink.emit(ts, schema::BUILD, &[OscType::Float(f.buildup)]);
        sink.emit(ts, schema::ENERGY, &[OscType::Float(f.loudness_s)]);
        if self.cfg.stems {
            sink.emit(
                ts,
                schema::STEM_DRUMS_ENERGY,
                &[OscType::Float(f.percussive_energy)],
            );
            sink.emit(
                ts,
                schema::STEM_BASS_ENERGY,
                &[OscType::Float(0.5 * (f.sub_bass + f.bass))],
            );
            sink.emit(
                ts,
                schema::STEM_MELODY_ENERGY,
                &[OscType::Float(f.harmonic_energy)],
            );
        }
        sink.emit(
            ts,
            schema::PHRASE_BAR,
            &[OscType::Int(ph.bar_in_phrase as i32)],
        );
        sink.emit(
            ts,
            schema::PHRASE_BEATS_LEFT,
            &[OscType::Int(ph.beats_left as i32)],
        );
        sink.emit(ts, schema::PREDICT_DROP, &[OscType::Float(predict)]);

        if self.cfg.feat_bus {
            for (addr, value) in self.feat_addrs.iter().zip(f.as_slice()) {
                sink.emit(ts, addr, &[OscType::Float(*value)]);
            }
        }
    }

    /// Status heartbeat — caller-driven (1 Hz wall clock live, sample clock offline).
    pub fn emit_status(
        &mut self,
        ts: f64,
        uptime_secs: f64,
        device: &str,
        hop_hz: f64,
        sink: &mut dyn SignalSink,
    ) {
        sink.emit(ts, schema::STATUS_ONLINE, &[OscType::Int(1)]);
        sink.emit(
            ts,
            schema::STATUS_UPTIME,
            &[OscType::Float(uptime_secs as f32)],
        );
        sink.emit(
            ts,
            schema::STATUS_DEVICE,
            &[OscType::String(device.to_string())],
        );
        sink.emit(ts, schema::STATUS_HOP_HZ, &[OscType::Float(hop_hz as f32)]);
        sink.emit(
            ts,
            schema::STATUS_TIER,
            &[OscType::String(self.section.tier().to_string())],
        );
    }

    /// The clean-shutdown goodbye. Receivers should also treat >~3 s of status
    /// staleness as offline (a SIGKILL sends no goodbye).
    pub fn emit_offline(&mut self, ts: f64, sink: &mut dyn SignalSink) {
        sink.emit(ts, schema::STATUS_ONLINE, &[OscType::Int(0)]);
    }
}

fn emit_section(sink: &mut dyn SignalSink, ts: f64, label: SectionLabel, conf: f32) {
    sink.emit(
        ts,
        schema::SECTION,
        &[
            OscType::String(label.as_str().to_string()),
            OscType::Float(conf),
        ],
    );
}

fn emit_key(sink: &mut dyn SignalSink, ts: f64, tonic: usize, minor: bool, conf: f32) {
    let name = format!("{}{}", NOTE_NAMES[tonic], if minor { "m" } else { "" });
    sink.emit(
        ts,
        schema::KEY,
        &[OscType::String(name), OscType::Float(conf)],
    );
}

fn emit_phrase_len(sink: &mut dyn SignalSink, ts: f64, len: u32, conf: f32) {
    sink.emit(
        ts,
        schema::PHRASE_LEN,
        &[OscType::Int(len as i32), OscType::Float(conf)],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::sink::VecSink;
    use bytemuck::Zeroable;

    const HOP_SECS: f64 = 1.0 / 86.0;

    fn cfg() -> EmitCfg {
        EmitCfg {
            tx_rate_hz: 30,
            feat_bus: false,
            stems: true,
        }
    }

    fn frame(ts: f64, setup: impl FnOnce(&mut AudioFeatures)) -> AudioFrame {
        let mut f = AudioFeatures::zeroed();
        setup(&mut f);
        AudioFrame {
            features: f,
            spectrum: vec![0.0; 4].into_boxed_slice(),
            mel: vec![0.0; 4].into_boxed_slice(),
            dmfcc: [0.0; 13],
            timestamp: ts,
            phase_frozen: false,
            bar_duration: 2.0,
            beat_time: None,
        }
    }

    /// N beat pulses in produce exactly N /beat events out, with monotonic counts —
    /// the property the render-thread OSC path never had (30 Hz cap ate pulses).
    #[test]
    fn every_beat_pulse_becomes_exactly_one_event() {
        let mut em = SignalEmitter::new(cfg());
        let mut sink = VecSink::new();
        let mut ts = 0.0;
        let mut beats = 0;
        for hop in 0..860 {
            let on_beat = hop % 43 == 0; // ~2 Hz beats at 86 Hz hops
            if on_beat {
                beats += 1;
            }
            em.process_frame(
                &frame(ts, |f| f.beat = if on_beat { 1.0 } else { 0.0 }),
                &mut sink,
            );
            ts += HOP_SECS;
        }
        let got = sink.at(schema::BEAT);
        assert_eq!(got.len(), beats);
        let counts: Vec<i32> = got
            .iter()
            .map(|(_, _, args)| match args.as_slice() {
                [OscType::Int(n)] => *n,
                other => panic!("beat args {other:?}"),
            })
            .collect();
        assert_eq!(counts, (1..=beats as i32).collect::<Vec<_>>());
        assert_eq!(em.totals().0, beats as u32);
    }

    /// Continuous addresses decimate to the configured rate; events don't.
    #[test]
    fn continuous_group_decimates_to_tx_rate() {
        let mut em = SignalEmitter::new(EmitCfg {
            tx_rate_hz: 10,
            feat_bus: false,
            stems: true,
        });
        let mut sink = VecSink::new();
        let mut ts = 0.0;
        for _ in 0..860 {
            // 10 s of frames
            em.process_frame(&frame(ts, |f| f.buildup = 0.5), &mut sink);
            ts += HOP_SECS;
        }
        let n = sink.at(schema::BUILD).len();
        assert!((95..=105).contains(&n), "10 Hz over 10 s → ~100, got {n}");
    }

    /// Key announcements: once on change (confidence-gated), plus the 1 Hz
    /// re-broadcast for late joiners.
    #[test]
    fn key_emits_on_change_and_rebroadcasts() {
        let mut em = SignalEmitter::new(cfg());
        let mut sink = VecSink::new();
        let mut ts = 0.0;
        // 0.5 s of A minor…
        for _ in 0..43 {
            em.process_frame(
                &frame(ts, |f| {
                    f.key_class = 9.0 / 11.0;
                    f.key_is_minor = 1.0;
                    f.key_confidence = 0.8;
                }),
                &mut sink,
            );
            ts += HOP_SECS;
        }
        // …then 2 s of F# major.
        for _ in 0..172 {
            em.process_frame(
                &frame(ts, |f| {
                    f.key_class = 6.0 / 11.0;
                    f.key_is_minor = 0.0;
                    f.key_confidence = 0.8;
                }),
                &mut sink,
            );
            ts += HOP_SECS;
        }
        let keys: Vec<&str> = sink
            .at(schema::KEY)
            .iter()
            .map(|(_, _, args)| match args.as_slice() {
                [OscType::String(s), OscType::Float(_)] => s.as_str(),
                other => panic!("key args {other:?}"),
            })
            .collect();
        assert!(keys.starts_with(&["Am"]), "{keys:?}");
        assert!(keys.contains(&"F#"), "{keys:?}");
        // Change events: exactly one Am→F# switch; the rest are re-broadcasts.
        let switches = keys.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(switches, 1, "{keys:?}");
        // Re-broadcast cadence: ~1 Hz over 2.5 s → at least 2 messages total.
        assert!(keys.len() >= 2 && keys.len() <= 6, "{keys:?}");
    }

    /// The onset edge detector fires once per envelope jump, not per hop.
    #[test]
    fn onset_edge_fires_once_per_jump() {
        let mut em = SignalEmitter::new(cfg());
        let mut sink = VecSink::new();
        let mut ts = 0.0;
        // Envelope: jump to 0.8, hold, decay, jump again.
        let envelope: Vec<f32> = (0..100)
            .map(|i| match i {
                10 => 0.8,
                11..=30 => 0.8 - (i - 10) as f32 * 0.02,
                60 => 0.9,
                61..=80 => 0.9 - (i - 60) as f32 * 0.02,
                _ => 0.1,
            })
            .collect();
        for v in envelope {
            em.process_frame(&frame(ts, |f| f.onset = v), &mut sink);
            ts += HOP_SECS;
        }
        assert_eq!(sink.at(schema::ONSET).len(), 2);
    }

    #[test]
    fn stems_off_silences_stem_addresses() {
        let mut em = SignalEmitter::new(EmitCfg {
            tx_rate_hz: 30,
            feat_bus: false,
            stems: false,
        });
        let mut sink = VecSink::new();
        let mut ts = 0.0;
        for _ in 0..258 {
            em.process_frame(
                &frame(ts, |f| {
                    f.kick = 0.9;
                    f.percussive_energy = 0.7;
                }),
                &mut sink,
            );
            ts += HOP_SECS;
        }
        assert!(sink.at(schema::STEM_DRUMS_ENERGY).is_empty());
        assert!(sink.at(schema::STEM_DRUMS_ONSET).is_empty());
        assert!(
            !sink.at(schema::ENERGY).is_empty(),
            "non-stem traffic continues"
        );
    }

    /// The feat bus is opt-in and covers all 83 slots per tick when on.
    #[test]
    fn feat_bus_emits_all_slots_when_enabled() {
        let mut em = SignalEmitter::new(EmitCfg {
            tx_rate_hz: 10,
            feat_bus: true,
            stems: false,
        });
        let mut sink = VecSink::new();
        em.process_frame(&frame(0.0, |f| f.rms = 0.5), &mut sink);
        let feat_msgs = sink
            .msgs
            .iter()
            .filter(|(_, a, _)| a.starts_with("/fosfora/v1/feat/"))
            .count();
        assert_eq!(feat_msgs, crate::audio::features::NUM_FEATURES);
    }

    /// End-to-end over the real analysis chain: the golden synthetic signal through
    /// the real audio thread, frames into the emitter — beat events must match the
    /// engine's own pulse counters exactly.
    #[test]
    fn golden_signal_beats_reconcile_with_pulse_counters() {
        let signal = crate::audio::tests::golden_signal(44100.0, 4.0);
        let (frames, counts) = crate::audio::tests::run_audio_thread_collecting(&signal, 44100.0);
        let mut em = SignalEmitter::new(cfg());
        let mut sink = VecSink::new();
        for fr in &frames {
            em.process_frame(fr, &mut sink);
        }
        let (beats, bars, drops) = em.totals();
        assert_eq!(beats, counts.beat, "emitter saw every beat pulse");
        assert_eq!(bars, counts.downbeat);
        assert_eq!(drops, counts.drop);
        assert_eq!(sink.at(schema::BEAT).len(), counts.beat as usize);
        assert!(!sink.at(schema::SECTION).is_empty(), "section announced");
    }
}
