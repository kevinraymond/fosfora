//! The modulation half of the Parameter/Modulation/Uniform triple.
//!
//! Every Float parameter can carry one modulation slot (flat — no
//! mod-of-mod): an oscillator or an audio feature, scaled by a bipolar
//! `amount` into the parameter's span, combined with the manual base value
//! by a mode, clamped to the parameter's range, then slewed by a dt-correct
//! one-pole. The manual base is what will serialize (M3); everything in
//! [`ModState`] is runtime-only and replays deterministically — oscillator
//! RNG streams are seeded from node id + param name, never the wall clock.
//!
//! Resolution runs exactly once per frame, from `App::update`. The executor
//! only *reads* the cached results ([`apply_resolved`]), so the dissolve
//! path's second execute per frame cannot double-advance oscillators, and
//! the inspector's ghost indicator reads the same values after the fact.

use crate::params::{ParamDef, ParamStore, ParamValue};

use super::audio::{AudioFeature, AudioView};
use super::node::NodeId;

/// How a modulation signal combines with the manual base value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModMode {
    /// `base + signal · amount · span` — bipolar wobble around the slider.
    Add,
    /// `base · (1 + signal · amount)` — relative scaling. Deliberate
    /// deviation from the handoff's literal `base · m` (with `m` already
    /// scaled into the span): that is dimensionally param-units² and pins
    /// the value to ~0 whenever the signal rests. Recorded in DECISIONS.md.
    Multiply,
    /// Crossfade the base toward the signal mapped into the param's range,
    /// by `|amount|`; negative amounts invert the signal first.
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OscShape {
    Sine,
    Saw,
    Square,
    Triangle,
    /// New random level each cycle.
    SampleHold,
    /// Random walk: a fresh target each cycle, slewed toward with
    /// τ = period/3 — smooth bounded wander.
    Drift,
}

impl OscShape {
    pub const ALL: [OscShape; 6] = [
        OscShape::Sine,
        OscShape::Saw,
        OscShape::Square,
        OscShape::Triangle,
        OscShape::SampleHold,
        OscShape::Drift,
    ];
}

/// Musical divisions for beat-synced rates. 4/4 is assumed in v1 (the bar
/// clock exists for a later meter-aware upgrade); `beats()` is the cycle
/// length in beats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeatDiv {
    FourBars,
    TwoBars,
    Bar,
    Half,
    Beat,
    Eighth,
    Sixteenth,
}

impl BeatDiv {
    pub fn beats(self) -> f32 {
        match self {
            BeatDiv::FourBars => 16.0,
            BeatDiv::TwoBars => 8.0,
            BeatDiv::Bar => 4.0,
            BeatDiv::Half => 2.0,
            BeatDiv::Beat => 1.0,
            BeatDiv::Eighth => 0.5,
            BeatDiv::Sixteenth => 0.25,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BeatDiv::FourBars => "4 bars",
            BeatDiv::TwoBars => "2 bars",
            BeatDiv::Bar => "1 bar",
            BeatDiv::Half => "1/2",
            BeatDiv::Beat => "1/4",
            BeatDiv::Eighth => "1/8",
            BeatDiv::Sixteenth => "1/16",
        }
    }

    pub const ALL: [BeatDiv; 7] = [
        BeatDiv::FourBars,
        BeatDiv::TwoBars,
        BeatDiv::Bar,
        BeatDiv::Half,
        BeatDiv::Beat,
        BeatDiv::Eighth,
        BeatDiv::Sixteenth,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OscRate {
    Hz(f32),
    /// Phase derived from the continuous beat clock; freezes with it when
    /// the tempo detector is unlocked (a fallback tempo would snap phase
    /// the moment lock lands — Hz rates are the "always moving" option).
    BeatSync(BeatDiv),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Osc {
    pub shape: OscShape,
    pub rate: OscRate,
    /// Phase offset in cycles, 0..1.
    pub phase: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModSource {
    Oscillator(Osc),
    Audio(AudioFeature),
}

/// One parameter's modulation slot — the serializable configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Modulation {
    pub source: ModSource,
    /// Depth, -1..=1, scaling into the parameter's `(max - min)` span.
    pub amount: f32,
    pub mode: ModMode,
    /// 0..=1 knob mapped to a one-pole τ = smoothing² · 2 s (quadratic for
    /// fine control in the busy 0–200 ms region); 0 snaps.
    pub smoothing: f32,
}

/// Runtime-only state beside a [`Modulation`]. Never serialized; replays
/// deterministically from the (node, param) seed.
#[derive(Debug, Clone)]
pub struct ModState {
    /// Hz-rate phase accumulator in cycles (`phase += rate · dt` — dt is the
    /// clamped frame dt; wall-clock time diverges from summed dt on hitches).
    phase: f32,
    /// Previous effective phase, for cycle-wrap detection (S&H/Drift fire).
    prev_phase: f32,
    /// splitmix64 stream, seeded from node id + param name.
    rng: u64,
    /// S&H output level / Drift target, in [-1, 1].
    held: f32,
    /// Drift's slewed output.
    drift: f32,
    /// One-pole state; `None` until the first resolve, which snaps (a newly
    /// assigned modulation must not slew in from an unrelated value).
    smoothed: Option<f32>,
    /// Scalar offset of the param in the packed `[f32; 16]`, recomputed each
    /// resolve; `None` = inert (non-Float, unknown name, or beyond the cap).
    pub slot: Option<u8>,
    /// The value the uniform gets and the inspector ghost shows.
    pub resolved: f32,
}

impl ModState {
    fn seeded(node: NodeId, param: &str) -> Self {
        let mut rng = seed_for(node, param);
        // Pre-sample so S&H has a level before its first cycle wrap.
        let held = rand_bipolar(&mut rng);
        Self {
            phase: 0.0,
            prev_phase: 0.0,
            rng,
            held,
            drift: 0.0,
            smoothed: None,
            slot: None,
            resolved: 0.0,
        }
    }
}

/// A parameter name paired with its modulation config + runtime state.
/// Lives on `NodeInstance` so it dies with the node and survives rewires.
#[derive(Debug, Clone)]
pub struct ParamMod {
    pub param: String,
    pub config: Modulation,
    pub state: ModState,
}

impl ParamMod {
    pub fn new(node: NodeId, param: impl Into<String>, config: Modulation) -> Self {
        let param = param.into();
        let state = ModState::seeded(node, &param);
        Self {
            param,
            config,
            state,
        }
    }
}

/// Resolve every modulation on one node: advance oscillator state by `dt`,
/// combine with the base value, clamp, slew, and cache `slot`/`resolved`.
/// Zero allocations — called for every node every frame.
pub fn resolve_node(params: &ParamStore, mods: &mut [ParamMod], dt: f32, view: &AudioView) {
    for m in mods {
        let Some((slot, min, max, base)) = locate_float(params, &m.param) else {
            m.state.slot = None;
            continue;
        };
        let span = max - min;
        if span <= 0.0 {
            m.state.slot = None;
            continue;
        }
        m.state.slot = Some(slot);

        let (signal, bipolar) = match &m.config.source {
            ModSource::Oscillator(osc) => (advance_osc(osc, &mut m.state, dt, view), true),
            ModSource::Audio(feature) => (view.signal(*feature), false),
        };
        let amount = m.config.amount.clamp(-1.0, 1.0);
        let target = match m.config.mode {
            ModMode::Add => base + signal * amount * span,
            ModMode::Multiply => base * (1.0 + signal * amount),
            ModMode::Replace => {
                let u_raw = if bipolar {
                    (signal + 1.0) * 0.5
                } else {
                    signal
                };
                let u = if amount < 0.0 { 1.0 - u_raw } else { u_raw };
                base + (min + span * u - base) * amount.abs()
            }
        };
        let target = target.clamp(min, max);

        let smoothing = m.config.smoothing.clamp(0.0, 1.0);
        m.state.resolved = if smoothing <= 0.0 {
            m.state.smoothed = Some(target);
            target
        } else {
            let tau = smoothing * smoothing * 2.0;
            let alpha = 1.0 - (-dt / tau.max(0.001)).exp();
            let smoothed = m.state.smoothed.get_or_insert(target);
            *smoothed += alpha * (target - *smoothed);
            *smoothed
        };
    }
}

/// Overlay resolved values onto a packed params buffer. Touches only the
/// slots that carry a live modulation, so unmodulated params keep the
/// same-frame base values the caller just packed.
pub fn apply_resolved(buf: &mut [f32; 16], mods: &[ParamMod]) {
    for m in mods {
        if let Some(slot) = m.state.slot {
            buf[slot as usize] = m.state.resolved;
        }
    }
}

/// Find a Float param by name: its scalar slot in the packed buffer
/// (declaration order, same walk as `ParamStore::pack_to_buffer`), range,
/// and current base value.
fn locate_float(params: &ParamStore, name: &str) -> Option<(u8, f32, f32, f32)> {
    let mut offset = 0usize;
    for def in &params.defs {
        if def.name() == name {
            if let ParamDef::Float {
                default, min, max, ..
            } = def
            {
                if offset >= 16 {
                    return None;
                }
                let base = match params.get(name) {
                    Some(ParamValue::Float(v)) => *v,
                    _ => *default,
                };
                return Some((offset as u8, *min, *max, base));
            }
            return None;
        }
        offset += def_width(def);
    }
    None
}

/// Scalar slots a def occupies in the packed buffer.
fn def_width(def: &ParamDef) -> usize {
    match def {
        ParamDef::Float { .. } | ParamDef::Bool { .. } => 1,
        ParamDef::Point2D { .. } => 2,
        ParamDef::Color { .. } => 4,
    }
}

/// Advance one oscillator by `dt` and return its bipolar sample.
fn advance_osc(osc: &Osc, state: &mut ModState, dt: f32, view: &AudioView) -> f32 {
    let eff = match osc.rate {
        OscRate::Hz(hz) => {
            state.phase = (state.phase + hz.max(0.0) * dt).fract();
            (state.phase + osc.phase).fract()
        }
        OscRate::BeatSync(div) => (view.beat_clock() / div.beats() + osc.phase).fract(),
    };
    let wrapped = eff < state.prev_phase;
    state.prev_phase = eff;

    match osc.shape {
        OscShape::Sine => (eff * std::f32::consts::TAU).sin(),
        OscShape::Saw => 2.0 * eff - 1.0,
        OscShape::Square => {
            if eff < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        OscShape::Triangle => 1.0 - 4.0 * (eff - 0.5).abs(),
        OscShape::SampleHold => {
            if wrapped {
                state.held = rand_bipolar(&mut state.rng);
            }
            state.held
        }
        OscShape::Drift => {
            if wrapped {
                state.held = rand_bipolar(&mut state.rng);
            }
            let period = match osc.rate {
                OscRate::Hz(hz) if hz > 0.0 => 1.0 / hz,
                OscRate::Hz(_) => f32::INFINITY,
                // Unlocked tempo: hold, consistent with the frozen phase.
                OscRate::BeatSync(div) => view
                    .beat_period_secs()
                    .map_or(f32::INFINITY, |p| p * div.beats()),
            };
            if period.is_finite() {
                let tau = (period / 3.0).max(0.001);
                let alpha = 1.0 - (-dt / tau).exp();
                state.drift += alpha * (state.held - state.drift);
            }
            state.drift.clamp(-1.0, 1.0)
        }
    }
}

/// House splitmix64 (see `gpu/particle/splat_source.rs`) — deterministic,
/// no dependency.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform in [-1, 1) from the top 24 bits of a splitmix64 draw.
fn rand_bipolar(state: &mut u64) -> f32 {
    let bits = (splitmix64(state) >> 40) as u32;
    (bits as f32 / 16_777_216.0) * 2.0 - 1.0
}

/// FNV-1a over the param name, mixed with the node id: stable per
/// (node, param), so S&H/Drift streams replay identically run to run.
fn seed_for(node: NodeId, param: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in param.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^ node.0.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::features::AudioFeatures;

    fn float_store(name: &str, default: f32, min: f32, max: f32) -> ParamStore {
        let mut store = ParamStore::new();
        store.load_from_defs(&[ParamDef::Float {
            name: name.into(),
            default,
            min,
            max,
        }]);
        store
    }

    /// A view whose `Rms` signal reads exactly `level`.
    fn rms_view(level: f32) -> AudioView {
        let mut view = AudioView::default();
        let features = AudioFeatures {
            rms: level,
            ..Default::default()
        };
        view.update(0.016, &features, &[]);
        view
    }

    fn audio_mod(mode: ModMode, amount: f32, smoothing: f32) -> Modulation {
        Modulation {
            source: ModSource::Audio(AudioFeature::Rms),
            amount,
            mode,
            smoothing,
        }
    }

    fn resolve_one(store: &ParamStore, m: &mut ParamMod, dt: f32, view: &AudioView) -> f32 {
        resolve_node(store, std::slice::from_mut(m), dt, view);
        m.state.resolved
    }

    #[test]
    fn apply_add_offsets_and_clamps() {
        let store = float_store("speed", 0.5, 0.0, 1.0);
        let view = rms_view(1.0);
        let mut m = ParamMod::new(NodeId(1), "speed", audio_mod(ModMode::Add, 0.2, 0.0));
        assert!((resolve_one(&store, &mut m, 0.016, &view) - 0.7).abs() < 1e-6);
        m.config.amount = 0.9; // 0.5 + 0.9 → clamps at max
        assert!((resolve_one(&store, &mut m, 0.016, &view) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn apply_multiply_is_relative_and_clamps() {
        let store = float_store("speed", 0.5, 0.0, 1.0);
        let mut m = ParamMod::new(NodeId(1), "speed", audio_mod(ModMode::Multiply, 1.0, 0.0));
        // Signal at rest → base unchanged (the property literal base·m loses).
        assert!((resolve_one(&store, &mut m, 0.016, &rms_view(0.0)) - 0.5).abs() < 1e-6);
        // Full signal doubles the base.
        assert!((resolve_one(&store, &mut m, 0.016, &rms_view(1.0)) - 1.0).abs() < 1e-6);
        // Negative amount at full signal zeroes it.
        m.config.amount = -1.0;
        assert!(resolve_one(&store, &mut m, 0.016, &rms_view(1.0)).abs() < 1e-6);
    }

    #[test]
    fn apply_replace_blends_base_toward_mapped_signal() {
        let store = float_store("speed", 0.2, 0.0, 1.0);
        let mut m = ParamMod::new(NodeId(1), "speed", audio_mod(ModMode::Replace, 0.5, 0.0));
        // lerp(0.2, 1.0, 0.5) = 0.6
        assert!((resolve_one(&store, &mut m, 0.016, &rms_view(1.0)) - 0.6).abs() < 1e-6);
        m.config.amount = 1.0;
        assert!((resolve_one(&store, &mut m, 0.016, &rms_view(1.0)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn negative_amount_inverts() {
        let store = float_store("speed", 0.5, 0.0, 1.0);
        // Replace with amount -1 at signal 0.8 → target 1 - 0.8 = 0.2.
        let mut m = ParamMod::new(NodeId(1), "speed", audio_mod(ModMode::Replace, -1.0, 0.0));
        assert!((resolve_one(&store, &mut m, 0.016, &rms_view(0.8)) - 0.2).abs() < 1e-6);
        // Add with a negative amount pushes below the base.
        let mut m = ParamMod::new(NodeId(1), "speed", audio_mod(ModMode::Add, -0.4, 0.0));
        assert!((resolve_one(&store, &mut m, 0.016, &rms_view(1.0)) - 0.1).abs() < 1e-6);
    }

    #[test]
    fn zero_span_param_is_inert() {
        let store = float_store("fixed", 0.5, 0.5, 0.5);
        let mut m = ParamMod::new(NodeId(1), "fixed", audio_mod(ModMode::Add, 1.0, 0.0));
        resolve_node(&store, std::slice::from_mut(&mut m), 0.016, &rms_view(1.0));
        assert!(m.state.slot.is_none());
    }

    #[test]
    fn smoothing_zero_snaps() {
        let store = float_store("speed", 0.0, 0.0, 1.0);
        let mut m = ParamMod::new(NodeId(1), "speed", audio_mod(ModMode::Replace, 1.0, 0.0));
        assert!((resolve_one(&store, &mut m, 0.001, &rms_view(1.0)) - 1.0).abs() < 1e-6);
        assert!(resolve_one(&store, &mut m, 0.001, &rms_view(0.0)).abs() < 1e-6);
    }

    #[test]
    fn first_resolve_snaps_even_with_smoothing() {
        let store = float_store("speed", 0.0, 0.0, 1.0);
        let mut m = ParamMod::new(NodeId(1), "speed", audio_mod(ModMode::Replace, 1.0, 0.9));
        // No slew-in from an unrelated initial value: first resolve = target.
        assert!((resolve_one(&store, &mut m, 0.001, &rms_view(0.8)) - 0.8).abs() < 1e-6);
    }

    #[test]
    fn smoothing_is_dt_correct() {
        let store = float_store("speed", 0.0, 0.0, 1.0);
        let cfg = audio_mod(ModMode::Replace, 1.0, 0.5);
        let view = rms_view(1.0);

        let mut two = ParamMod::new(NodeId(1), "speed", cfg);
        resolve_one(&store, &mut two, 0.0, &rms_view(0.0)); // seed smoothed at 0
        resolve_one(&store, &mut two, 0.1, &view);
        let two_steps = resolve_one(&store, &mut two, 0.1, &view);

        let mut one = ParamMod::new(NodeId(1), "speed", cfg);
        resolve_one(&store, &mut one, 0.0, &rms_view(0.0));
        let one_step = resolve_one(&store, &mut one, 0.2, &view);

        assert!(
            (two_steps - one_step).abs() < 1e-5,
            "{two_steps} vs {one_step}"
        );
    }

    #[test]
    fn osc_shapes_hit_known_phase_values() {
        let view = AudioView::default();
        let cases = [
            (OscShape::Sine, 1.0),     // sin(2π·0.25) = 1
            (OscShape::Saw, -0.5),     // 2·0.25 − 1
            (OscShape::Square, 1.0),   // first half-cycle high
            (OscShape::Triangle, 0.0), // 1 − 4·|0.25 − 0.5|
        ];
        for (shape, expected) in cases {
            let mut state = ModState::seeded(NodeId(1), "p");
            let osc = Osc {
                shape,
                rate: OscRate::Hz(1.0),
                phase: 0.0,
            };
            let v = advance_osc(&osc, &mut state, 0.25, &view);
            assert!((v - expected).abs() < 1e-5, "{shape:?}: {v} != {expected}");
        }
    }

    #[test]
    fn osc_phase_accumulates_rate_dt_and_wraps() {
        let view = AudioView::default();
        let mut state = ModState::seeded(NodeId(1), "p");
        let osc = Osc {
            shape: OscShape::Saw,
            rate: OscRate::Hz(1.0),
            phase: 0.0,
        };
        let a = advance_osc(&osc, &mut state, 0.6, &view); // phase 0.6
        let b = advance_osc(&osc, &mut state, 0.6, &view); // wraps to 0.2
        assert!((a - (2.0 * 0.6 - 1.0)).abs() < 1e-5);
        assert!((b - (2.0 * 0.2 - 1.0)).abs() < 1e-4);
    }

    #[test]
    fn sample_hold_fires_on_wrap_and_is_deterministic_per_node_param() {
        let view = AudioView::default();
        let sequence = |node: NodeId| -> Vec<f32> {
            let mut state = ModState::seeded(node, "speed");
            let osc = Osc {
                shape: OscShape::SampleHold,
                rate: OscRate::Hz(1.0),
                phase: 0.0,
            };
            (0..8)
                .map(|_| advance_osc(&osc, &mut state, 0.6, &view))
                .collect()
        };
        let a = sequence(NodeId(1));
        let b = sequence(NodeId(1));
        assert_eq!(a, b, "same (node, param) must replay identically");
        // Effective phase runs 0.6, 0.2, 0.8, 0.4, 0.0, … — the second step
        // wraps (resample), the third holds.
        assert_ne!(a[0], a[1], "wrap must resample");
        assert_eq!(a[1], a[2], "no wrap must hold");
        // A different node id draws a different stream.
        assert_ne!(a, sequence(NodeId(2)));
    }

    #[test]
    fn drift_bounded_and_continuous() {
        let view = AudioView::default();
        let mut state = ModState::seeded(NodeId(7), "warp");
        let osc = Osc {
            shape: OscShape::Drift,
            rate: OscRate::Hz(2.0),
            phase: 0.0,
        };
        let mut prev = advance_osc(&osc, &mut state, 0.016, &view);
        for _ in 0..1000 {
            let v = advance_osc(&osc, &mut state, 0.016, &view);
            assert!(v.abs() <= 1.0, "drift escaped: {v}");
            assert!((v - prev).abs() < 0.2, "drift jumped: {prev} → {v}");
            prev = v;
        }
    }

    /// A view at 120 BPM with the beat clock at `clock` beats.
    fn beat_view(clock: f32) -> AudioView {
        let mut view = AudioView::default();
        let features = AudioFeatures {
            beat_index: clock.floor(),
            beat_phase: clock.fract(),
            bpm: 120.0 / 300.0,
            ..Default::default()
        };
        view.update(0.016, &features, &[]);
        view
    }

    #[test]
    fn beat_sync_phase_follows_beat_clock_per_division() {
        let mut state = ModState::seeded(NodeId(1), "p");
        let osc = Osc {
            shape: OscShape::Saw,
            rate: OscRate::BeatSync(BeatDiv::Half),
            phase: 0.0,
        };
        // Clock 2.5 beats over a 2-beat cycle → phase 0.25 → saw −0.5.
        let v = advance_osc(&osc, &mut state, 0.016, &beat_view(2.5));
        assert!((v - (-0.5)).abs() < 1e-5, "{v}");
        // A bar-long cycle at the same clock sits at phase 2.5/4.
        let osc_bar = Osc {
            rate: OscRate::BeatSync(BeatDiv::Bar),
            ..osc
        };
        let v = advance_osc(&osc_bar, &mut state, 0.016, &beat_view(2.5));
        assert!((v - (2.0 * (2.5 / 4.0) - 1.0)).abs() < 1e-5, "{v}");
    }

    #[test]
    fn beat_sync_freezes_when_unlocked() {
        // Unlocked: the PLL holds beat_phase, so the derived phase (and any
        // S&H stream keyed on its wraps) holds too.
        let mut view = AudioView::default();
        let features = AudioFeatures {
            beat_index: 3.0,
            beat_phase: 0.4,
            bpm: 0.0,
            ..Default::default()
        };
        view.update(0.016, &features, &[]);
        let mut state = ModState::seeded(NodeId(1), "p");
        let osc = Osc {
            shape: OscShape::Saw,
            rate: OscRate::BeatSync(BeatDiv::Beat),
            phase: 0.0,
        };
        let a = advance_osc(&osc, &mut state, 0.5, &view);
        let b = advance_osc(&osc, &mut state, 0.5, &view);
        assert_eq!(a, b, "frozen clock must freeze the oscillator");
    }

    #[test]
    fn resolve_writes_slot_and_resolved_for_float() {
        let store = float_store("speed", 0.5, 0.0, 1.0);
        let mut m = ParamMod::new(NodeId(1), "speed", audio_mod(ModMode::Add, 0.0, 0.0));
        resolve_node(&store, std::slice::from_mut(&mut m), 0.016, &rms_view(0.0));
        assert_eq!(m.state.slot, Some(0));
        assert!((m.state.resolved - 0.5).abs() < 1e-6);
    }

    #[test]
    fn resolve_slot_respects_mixed_def_offsets() {
        let mut store = ParamStore::new();
        store.load_from_defs(&[
            ParamDef::Color {
                name: "tint".into(),
                default: [1.0; 4],
            },
            ParamDef::Float {
                name: "speed".into(),
                default: 0.5,
                min: 0.0,
                max: 1.0,
            },
        ]);
        let mut m = ParamMod::new(NodeId(1), "speed", audio_mod(ModMode::Add, 0.0, 0.0));
        resolve_node(&store, std::slice::from_mut(&mut m), 0.016, &rms_view(0.0));
        assert_eq!(m.state.slot, Some(4), "Color occupies slots 0-3");
    }

    #[test]
    fn resolve_skips_missing_and_non_float() {
        let mut store = ParamStore::new();
        store.load_from_defs(&[ParamDef::Color {
            name: "tint".into(),
            default: [1.0; 4],
        }]);
        let view = rms_view(1.0);
        let mut on_color = ParamMod::new(NodeId(1), "tint", audio_mod(ModMode::Add, 1.0, 0.0));
        resolve_node(&store, std::slice::from_mut(&mut on_color), 0.016, &view);
        assert!(on_color.state.slot.is_none());
        let mut on_ghost = ParamMod::new(NodeId(1), "ghost", audio_mod(ModMode::Add, 1.0, 0.0));
        resolve_node(&store, std::slice::from_mut(&mut on_ghost), 0.016, &view);
        assert!(on_ghost.state.slot.is_none());
    }

    #[test]
    fn apply_resolved_touches_only_modulated_slots() {
        let store = float_store("speed", 0.5, 0.0, 1.0);
        let mut m = ParamMod::new(NodeId(1), "speed", audio_mod(ModMode::Replace, 1.0, 0.0));
        resolve_node(&store, std::slice::from_mut(&mut m), 0.016, &rms_view(1.0));

        let mut buf = [7.0f32; 16];
        apply_resolved(&mut buf, std::slice::from_ref(&m));
        assert!((buf[0] - 1.0).abs() < 1e-6, "modulated slot overlaid");
        assert!(buf[1..].iter().all(|v| *v == 7.0), "other slots untouched");

        // A slot-less mod (unknown param) must leave the buffer alone.
        let ghost = ParamMod::new(NodeId(1), "ghost", audio_mod(ModMode::Add, 1.0, 0.0));
        let mut buf2 = [7.0f32; 16];
        apply_resolved(&mut buf2, std::slice::from_ref(&ghost));
        assert!(buf2.iter().all(|v| *v == 7.0));
    }

    #[test]
    fn resolve_then_double_apply_is_identical() {
        // The dissolve path executes the graph twice per frame; both
        // executes must see identical values because apply only reads.
        let store = float_store("speed", 0.5, 0.0, 1.0);
        let mut m = ParamMod::new(NodeId(1), "speed", audio_mod(ModMode::Add, 0.5, 0.3));
        resolve_node(&store, std::slice::from_mut(&mut m), 0.016, &rms_view(0.8));
        let resolved = m.state.resolved;
        let mut first = [0.0f32; 16];
        let mut second = [0.0f32; 16];
        apply_resolved(&mut first, std::slice::from_ref(&m));
        apply_resolved(&mut second, std::slice::from_ref(&m));
        assert_eq!(first, second);
        assert_eq!(m.state.resolved, resolved, "apply must not advance state");
    }

    #[test]
    fn orphan_nodes_still_resolve() {
        use super::super::effect::EffectId;
        use crate::trama::graph::NodeGraph;
        use crate::trama::node::NodeKind;
        // An unwired node's modulation still resolves via params_iter_mut,
        // so its oscillator phase stays warm across rewires.
        let mut g = NodeGraph::new_with_output();
        let s = g.add_node(
            NodeKind::Source {
                effect: EffectId("s".into()),
            },
            0,
            &[ParamDef::Float {
                name: "x".into(),
                default: 0.0,
                min: 0.0,
                max: 1.0,
            }],
        );
        g.set_modulation(s, "x", Some(audio_mod(ModMode::Replace, 1.0, 0.0)))
            .unwrap();
        let view = rms_view(0.9);
        for node in g.params_iter_mut() {
            resolve_node(node.params, node.mods, 0.016, &view);
        }
        let node = g.params_mut(s).unwrap();
        let m = &node.mods[0];
        assert_eq!(m.state.slot, Some(0));
        assert!((m.state.resolved - 0.9).abs() < 1e-6);
    }

    /// I8's heap half, verified: after warmup, one frame of trama's CPU
    /// work — audio-view fold, modulation resolve for every node, and the
    /// executor's per-node uniform prep (pack + overlay) — performs ZERO
    /// heap allocations. The GPU probes hold the other half (no texture or
    /// resource creation in steady state); wgpu's own command encoding is
    /// outside trama's control and outside this claim.
    #[test]
    fn steady_state_frame_cpu_work_allocates_nothing() {
        use super::super::effect::EffectId;
        use crate::gpu::ShaderUniforms;
        use crate::trama::graph::NodeGraph;
        use crate::trama::node::NodeKind;

        let float = |name: &str| ParamDef::Float {
            name: name.into(),
            default: 0.5,
            min: 0.0,
            max: 1.0,
        };
        let eff = |g: &mut NodeGraph, id: &str, inputs: u8, defs: &[ParamDef]| {
            g.add_node(
                NodeKind::Effect {
                    effect: EffectId(id.into()),
                },
                inputs,
                defs,
            )
        };

        // Motion-echo shape with every modulation source family in play:
        // audio feature, Hz oscillator, and SampleHold (the RNG path).
        let mut g = NodeGraph::new_with_output();
        let src = g.add_node(
            NodeKind::Source {
                effect: EffectId("noise".into()),
            },
            0,
            &[float("scale"), float("speed")],
        );
        let mix = eff(&mut g, "mix", 2, &[float("amount")]);
        let tr = eff(&mut g, "transform", 1, &[float("scale"), float("rotate")]);
        let fb = g.add_node(NodeKind::Feedback, 1, &[]);
        let out = g.output_node();
        g.connect(src, mix, 0).unwrap();
        g.connect(mix, tr, 0).unwrap();
        g.connect(tr, fb, 0).unwrap();
        g.connect(fb, mix, 1).unwrap();
        g.connect(mix, out, 0).unwrap();
        g.set_modulation(src, "speed", Some(audio_mod(ModMode::Add, 0.5, 0.3)))
            .unwrap();
        let osc = |shape: OscShape| Modulation {
            source: ModSource::Oscillator(Osc {
                shape,
                rate: OscRate::Hz(1.7),
                phase: 0.25,
            }),
            amount: 0.8,
            mode: ModMode::Replace,
            smoothing: 0.2,
        };
        g.set_modulation(tr, "rotate", Some(osc(OscShape::Sine)))
            .unwrap();
        g.set_modulation(mix, "amount", Some(osc(OscShape::SampleHold)))
            .unwrap();

        let features = AudioFeatures {
            rms: 0.6,
            bass: 0.8,
            ..Default::default()
        };
        let mel = vec![0.3f32; 64];
        let template = ShaderUniforms::zeroed();
        let mut view = AudioView::default();

        let frame = |g: &mut NodeGraph, view: &mut AudioView| {
            // TramaSystem::update's steady-state body...
            view.update(1.0 / 60.0, &features, &mel);
            for node in g.params_iter_mut() {
                resolve_node(node.params, node.mods, 1.0 / 60.0, view);
            }
            // ...and the executor's per-step CPU prep (write_buffer's
            // payload construction; the GPU call itself is out of scope).
            for node in g.params_iter_mut() {
                let mut u = template;
                u.params = node.params.pack_to_buffer();
                apply_resolved(&mut u.params, node.mods);
                std::hint::black_box(&u);
            }
        };

        // Warmup: first resolves snap smoothers and seed S&H state.
        frame(&mut g, &mut view);
        frame(&mut g, &mut view);
        let (allocs, ()) = crate::test_alloc::count_allocs(|| {
            for _ in 0..10 {
                frame(&mut g, &mut view);
            }
        });
        assert_eq!(
            allocs, 0,
            "steady-state frame CPU work must not allocate (I8)"
        );
    }
}
