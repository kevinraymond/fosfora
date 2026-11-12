//! Tempo-synced LFOs as binding sources (#129).
//!
//! Trama oscillators could already follow the beat, but only for trama node
//! params; a layer or effect param needed the external `smart_lfo.py` bridge.
//! These sources put the same curves on the binding bus:
//!
//! ```text
//! lfo.{shape}.{division}            lfo.sine.bar
//! lfo.{shape}.{division}.{degrees}  lfo.saw.beat.90   (phase offset, 0–359°)
//! ```
//!
//! Shapes: `sine`, `triangle`, `saw`, `square`, `sh` (sample and hold, a new
//! level each cycle). Divisions: `4bar`, `2bar`, `bar`, `half`, `beat`, `8th`,
//! `16th`. Values are 0..1. Phase comes from the beat clock
//! (`beat_index + beat_phase`), the clock trama's BeatSync oscillators read and
//! with the same 4/4 assumption, so an LFO here and one on a trama node agree;
//! like them, it holds still while the tempo is not locked.
//!
//! Every value is a pure function of the clock — sample and hold draws its
//! level from a hash of the cycle number — so the offline renderer replays
//! the same curve, and nothing is kept per binding.

use std::sync::LazyLock;

use crate::trama::modulation::{BeatDiv, Wave, splitmix64, wave};

use super::sources::SourceSnapshot;
use super::types::{Binding, SourceRaw};

/// Key prefix of every LFO source.
pub const PREFIX: &str = "lfo.";

/// An LFO waveform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoShape {
    Sine,
    Triangle,
    Saw,
    Square,
    SampleHold,
}

impl LfoShape {
    pub const ALL: [LfoShape; 5] = [
        LfoShape::Sine,
        LfoShape::Triangle,
        LfoShape::Saw,
        LfoShape::Square,
        LfoShape::SampleHold,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LfoShape::Sine => "sine",
            LfoShape::Triangle => "triangle",
            LfoShape::Saw => "saw",
            LfoShape::Square => "square",
            LfoShape::SampleHold => "sh",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LfoShape::Sine => "Sine",
            LfoShape::Triangle => "Triangle",
            LfoShape::Saw => "Saw",
            LfoShape::Square => "Square",
            LfoShape::SampleHold => "S&H",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        LfoShape::ALL.into_iter().find(|v| v.as_str() == s)
    }
}

fn division_key(div: BeatDiv) -> &'static str {
    match div {
        BeatDiv::FourBars => "4bar",
        BeatDiv::TwoBars => "2bar",
        BeatDiv::Bar => "bar",
        BeatDiv::Half => "half",
        BeatDiv::Beat => "beat",
        BeatDiv::Eighth => "8th",
        BeatDiv::Sixteenth => "16th",
    }
}

fn parse_division(s: &str) -> Option<BeatDiv> {
    BeatDiv::ALL.into_iter().find(|d| division_key(*d) == s)
}

/// A parsed LFO source key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LfoSpec {
    pub shape: LfoShape,
    pub division: BeatDiv,
    /// Phase offset in whole degrees, 0..360.
    pub degrees: u16,
}

impl LfoSpec {
    /// The source key this spec is written as. An offset of 0 is omitted.
    pub fn key(&self) -> String {
        let base = format!(
            "{PREFIX}{}.{}",
            self.shape.as_str(),
            division_key(self.division)
        );
        if self.degrees == 0 {
            base
        } else {
            format!("{base}.{}", self.degrees)
        }
    }

    /// "Sine 1 bar", "Saw 1/4 +90°".
    pub fn label(&self) -> String {
        let base = format!("{} {}", self.shape.label(), self.division.label());
        if self.degrees == 0 {
            base
        } else {
            format!("{base} +{}\u{00b0}", self.degrees)
        }
    }

    /// The LFO's value, 0..1, at `beat_clock` (beats, `beat_index + beat_phase`).
    pub fn value(&self, beat_clock: f32) -> f32 {
        let cycles = f64::from(beat_clock) / f64::from(self.division.beats())
            + f64::from(self.degrees) / 360.0;
        let eff = cycles.rem_euclid(1.0) as f32;
        let bipolar = match self.shape {
            LfoShape::Sine => wave(Wave::Sine, eff),
            LfoShape::Triangle => wave(Wave::Triangle, eff),
            LfoShape::Saw => wave(Wave::Saw, eff),
            LfoShape::Square => wave(Wave::Square, eff),
            LfoShape::SampleHold => return held_level(cycles.floor() as i64, self.division),
        };
        ((bipolar + 1.0) * 0.5).clamp(0.0, 1.0)
    }
}

/// Sample and hold's level for one cycle, 0..1: a hash of the cycle number
/// and the division, so each division gets its own sequence and a replay
/// draws the same levels.
fn held_level(cycle: i64, division: BeatDiv) -> f32 {
    let mut state = (cycle as u64) ^ (u64::from(division.beats().to_bits()) << 32);
    let bits = (splitmix64(&mut state) >> 40) as u32;
    bits as f32 / 16_777_216.0
}

/// Parse an `lfo.…` source key. `None` for anything else, including an offset
/// of 360° or more.
pub fn parse(key: &str) -> Option<LfoSpec> {
    let mut parts = key.strip_prefix(PREFIX)?.split('.');
    let shape = LfoShape::parse(parts.next()?)?;
    let division = parse_division(parts.next()?)?;
    let degrees = match parts.next() {
        None => 0,
        Some(d) => d.parse::<u16>().ok().filter(|d| *d < 360)?,
    };
    if parts.next().is_some() {
        return None;
    }
    Some(LfoSpec {
        shape,
        division,
        degrees,
    })
}

/// Every shape at every division, without offset: what the picker lists and
/// the bus publishes each frame. Keys are built once, so publishing them
/// allocates nothing.
static BASE: LazyLock<Vec<(String, LfoSpec)>> = LazyLock::new(|| {
    LfoShape::ALL
        .into_iter()
        .flat_map(|shape| {
            BeatDiv::ALL.into_iter().map(move |division| LfoSpec {
                shape,
                division,
                degrees: 0,
            })
        })
        .map(|spec| (spec.key(), spec))
        .collect()
});

/// The base LFO source keys, in picker order (shape, then slowest division
/// first).
pub fn base_keys() -> impl Iterator<Item = &'static str> {
    BASE.iter().map(|(k, _)| k.as_str())
}

/// Publish the base LFOs at `beat_clock`, plus any offset variant a binding in
/// `bindings` reads, so a bound `lfo.saw.beat.90` has a value and a meter like
/// any other source. Only the offset variants allocate a key, and only while
/// bound.
pub fn collect_into(map: &mut SourceSnapshot, beat_clock: f32, bindings: &[Binding]) {
    for (key, spec) in BASE.iter() {
        let v = spec.value(beat_clock);
        map.insert(key.as_str().into(), (v, SourceRaw::Value(v)));
    }
    for b in bindings {
        if !b.source.starts_with(PREFIX) || map.contains_key(b.source.as_str()) {
            continue;
        }
        if let Some(spec) = parse(&b.source) {
            let v = spec.value(beat_clock);
            map.insert(b.source.clone().into(), (v, SourceRaw::Value(v)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(key: &str) -> LfoSpec {
        parse(key).unwrap_or_else(|| panic!("{key} should parse"))
    }

    #[test]
    fn keys_round_trip() {
        for (key, s) in BASE.iter() {
            assert_eq!(parse(key).as_ref(), Some(s));
            assert_eq!(&s.key(), key);
        }
        assert_eq!(BASE.len(), 35);
        let off = spec("lfo.saw.beat.90");
        assert_eq!(off.degrees, 90);
        assert_eq!(off.key(), "lfo.saw.beat.90");
        assert_eq!(off.label(), "Saw 1/4 +90\u{00b0}");
        assert_eq!(spec("lfo.sine.bar").label(), "Sine 1 bar");
    }

    #[test]
    fn malformed_keys_do_not_parse() {
        for key in [
            "lfo.sine",
            "lfo.cosine.bar",
            "lfo.sine.3bar",
            "lfo.sine.bar.360",
            "lfo.sine.bar.-90",
            "lfo.sine.bar.90.1",
            "audio.lfo.sine.bar",
            "lfo.sine.bar.",
        ] {
            assert!(parse(key).is_none(), "{key} parsed");
        }
    }

    #[test]
    fn shapes_follow_the_beat_clock() {
        let saw = spec("lfo.saw.beat");
        assert!(saw.value(10.0).abs() < 1e-6, "a saw starts each beat at 0");
        assert!((saw.value(10.5) - 0.5).abs() < 1e-6);
        assert!((saw.value(10.99) - 0.99).abs() < 1e-4);

        let bar = spec("lfo.saw.bar");
        assert!(
            (bar.value(2.0) - 0.5).abs() < 1e-6,
            "half way through a 4-beat bar"
        );

        let sine = spec("lfo.sine.half");
        assert!((sine.value(0.0) - 0.5).abs() < 1e-6);
        assert!(
            (sine.value(0.5) - 1.0).abs() < 1e-6,
            "peak a quarter of the way"
        );

        let tri = spec("lfo.triangle.beat");
        assert!(tri.value(0.0).abs() < 1e-6);
        assert!((tri.value(0.5) - 1.0).abs() < 1e-6);

        let sq = spec("lfo.square.8th");
        assert!((sq.value(0.1) - 1.0).abs() < 1e-6);
        assert!(sq.value(0.3).abs() < 1e-6);
    }

    #[test]
    fn a_phase_offset_shifts_the_curve() {
        let a = spec("lfo.sine.bar");
        let b = spec("lfo.sine.bar.90");
        // 90° of a 4-beat cycle is one beat.
        for clock in [0.0, 1.3, 7.9] {
            assert!((b.value(clock) - a.value(clock + 1.0)).abs() < 1e-5);
        }
    }

    #[test]
    fn sample_and_hold_holds_for_a_cycle_and_replays() {
        let sh = spec("lfo.sh.beat");
        let level = sh.value(4.1);
        assert!((0.0..1.0).contains(&level));
        assert_eq!(sh.value(4.9), level, "held through the beat");
        let levels: Vec<f32> = (0..16).map(|b| sh.value(b as f32 + 0.5)).collect();
        assert!(
            levels.windows(2).any(|w| w[0] != w[1]),
            "a new level each cycle"
        );
        let again: Vec<f32> = (0..16).map(|b| sh.value(b as f32 + 0.5)).collect();
        assert_eq!(levels, again, "deterministic");
    }

    #[test]
    fn every_value_stays_in_unit_range() {
        for (_, s) in BASE.iter() {
            for i in 0..200 {
                let v = s.value(i as f32 * 0.137 - 3.0);
                assert!((0.0..=1.0).contains(&v), "{s:?} at {i}: {v}");
            }
        }
    }

    #[test]
    fn collect_publishes_the_base_set_and_bound_offsets_only() {
        use super::super::types::{BindingScope, BindingTarget};
        let bound = |source: &str| Binding {
            id: "b_001".into(),
            name: String::new(),
            enabled: true,
            scope: BindingScope::Global,
            source: source.into(),
            target: BindingTarget::Unset,
            transforms: Vec::new(),
        };
        let mut snap = SourceSnapshot::new();
        collect_into(
            &mut snap,
            1.0,
            &[
                bound("lfo.saw.beat.90"),
                bound("lfo.nope"),
                bound("audio.kick"),
            ],
        );
        assert_eq!(snap.len(), 36);
        let (v, _) = snap["lfo.saw.beat.90"];
        assert!((v - 0.25).abs() < 1e-6);
        assert!(!snap.contains_key("lfo.nope"));
    }
}
