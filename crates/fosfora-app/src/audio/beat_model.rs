//! Beat activation (#4163): the probability that a beat landed [`BeatActivation::lag_hops`]
//! hops ago, scored every analysis hop by a small gradient-boosted tree ensemble over the
//! raw evidence the detectors already compute: the kick model's band frame and probability,
//! the SuperFlux onset flux, the perceptual-silence flag and the pre-normalization features,
//! for this hop and a few before it. It is evidence for the beat grid (the phase arbiter's
//! votes), not a beat tracker: the grid still predicts beats, the activation steers phase.
//!
//! The ensemble is trained by `scripts/beat_model/train.py` on the hop sidecar's dumps
//! (`FOSFORA_HOP_SIDECAR`, the same columns this module reads) and exported as
//! `beat_model.json`: flat node arrays like `kick_model.json`, plus the input map (which
//! column at which hop offset feeds each ensemble input), the target lag and the picker the
//! training chose. The runtime resolves the input names onto the detector chain's slots at
//! load, so a retrained model with other columns needs no code change, and a name that
//! does not exist fails at the first hop, loudly.
//!
//! Causal by construction: every offset is at or before this hop, and hops before the start
//! of the stream read as zero, exactly as the training rows were built.

use std::sync::OnceLock;

use serde::Deserialize;

use super::beat::ONSET_FLUX_NAMES;
use super::features::AudioFeatures;
use super::kick_model::{FRAME_NAMES, N_FRAME};
use super::schema::FEATURES;

/// One hop's raw evidence, as the hop chain has it once the onset detector has run.
pub struct RawEvidence<'a> {
    /// The kick model's input frame for this hop (`kick_model::FRAME_NAMES`).
    pub kick_frame: &'a [f32; N_FRAME],
    /// The kick model's raw probability, before the silence gate and the `kick` mapping.
    pub kick_probability: f32,
    /// `BeatDetector::raw_onset_flux`: combined, low, mid, high.
    pub onset_flux: [f64; 4],
    /// The pre-normalization feature snapshot.
    pub pre_norm: &'a AudioFeatures,
    /// The A10 perceptual-silence gate.
    pub loud_silent: bool,
}

/// The causal peak picker the training chose for turning the activation into events:
/// fire at a hop whose activation is at least `thr`, is the maximum of the last `k` hops,
/// and comes at least `k` hops after the previous fire.
#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Picker {
    pub k: u32,
    pub thr: f32,
}

#[derive(Deserialize)]
struct InputDef {
    name: String,
    offset: i32,
}

/// `beat_model.json` as `scripts/beat_model/train.py --export` writes it (format 2).
#[derive(Deserialize)]
struct ModelFile {
    format: u32,
    kind: String,
    lag_hops: u32,
    inputs: Vec<InputDef>,
    n_features: usize,
    baseline: f32,
    picker: Picker,
    roots: Vec<u32>,
    feature: Vec<u16>,
    threshold: Vec<f32>,
    left: Vec<u32>,
    right: Vec<u32>,
    value: Vec<f32>,
    leaf: Vec<u8>,
}

/// Where an input column lives in the detector chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    KickFrame(usize),
    OnsetFlux(usize),
    KickProbability,
    PreNorm(usize),
    Silent,
}

/// `kf.<band feature>`, `sf.<partition>`, `kp`, `pn.<AudioFeatures slot>`, `sil`: the
/// names the hop sidecar's header gives the columns, which is what the training saw.
fn resolve(name: &str) -> Option<Column> {
    if let Some(n) = name.strip_prefix("kf.") {
        return FRAME_NAMES
            .iter()
            .position(|&f| f == n)
            .map(Column::KickFrame);
    }
    if let Some(n) = name.strip_prefix("sf.") {
        return ONSET_FLUX_NAMES
            .iter()
            .position(|&f| f == n)
            .map(Column::OnsetFlux);
    }
    if let Some(n) = name.strip_prefix("pn.") {
        return FEATURES
            .iter()
            .position(|d| d.name == n)
            .map(Column::PreNorm);
    }
    match name {
        "kp" => Some(Column::KickProbability),
        "sil" => Some(Column::Silent),
        _ => None,
    }
}

fn read(column: Column, ev: &RawEvidence) -> f32 {
    match column {
        Column::KickFrame(i) => ev.kick_frame[i],
        Column::OnsetFlux(i) => ev.onset_flux[i] as f32,
        Column::KickProbability => ev.kick_probability,
        Column::PreNorm(i) => ev.pre_norm.as_slice()[i],
        Column::Silent => f32::from(u8::from(ev.loud_silent)),
    }
}

struct Model {
    file: ModelFile,
    /// The distinct columns the inputs read, in first-use order: one ring slot each.
    columns: Vec<Column>,
    /// Per ensemble input: (ring slot, hops back).
    gather: Vec<(usize, usize)>,
    /// The deepest look-back any input needs.
    max_back: usize,
}

impl Model {
    fn load(json: &str) -> Model {
        let file: ModelFile = serde_json::from_str(json).expect("beat_model.json parses");
        assert_eq!(file.format, 2, "beat_model.json format");
        assert_eq!(file.kind, "beat_activation", "beat_model.json kind");
        assert_eq!(
            file.n_features,
            file.inputs.len(),
            "beat_model.json input count"
        );
        let n = file.feature.len();
        assert!(
            [
                file.threshold.len(),
                file.left.len(),
                file.right.len(),
                file.value.len(),
                file.leaf.len()
            ]
            .iter()
            .all(|&l| l == n),
            "beat_model.json node arrays"
        );
        let mut columns: Vec<Column> = Vec::new();
        let mut gather = Vec::with_capacity(file.inputs.len());
        let mut max_back = 0usize;
        for input in &file.inputs {
            let column = resolve(&input.name).unwrap_or_else(|| {
                panic!("beat_model.json input {:?} names no column", input.name)
            });
            assert!(
                input.offset <= 0,
                "beat_model.json input {} looks ahead ({})",
                input.name,
                input.offset
            );
            let back = (-input.offset) as usize;
            max_back = max_back.max(back);
            let slot = match columns.iter().position(|&c| c == column) {
                Some(s) => s,
                None => {
                    columns.push(column);
                    columns.len() - 1
                }
            };
            gather.push((slot, back));
        }
        Model {
            file,
            columns,
            gather,
            max_back,
        }
    }

    fn probability(&self, x: &[f32]) -> f32 {
        let f = &self.file;
        let mut raw = f.baseline;
        for &root in &f.roots {
            let mut i = root as usize;
            while f.leaf[i] == 0 {
                i = if x[f.feature[i] as usize] <= f.threshold[i] {
                    f.left[i]
                } else {
                    f.right[i]
                } as usize;
            }
            raw += f.value[i];
        }
        1.0 / (1.0 + (-raw).exp())
    }
}

fn model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| Model::load(include_str!("beat_model.json")))
}

/// Per-hop beat activation. Owns the recent hops' column values; call [`process`]
/// exactly once per hop, silent or not, so the history stays aligned.
///
/// [`process`]: BeatActivation::process
pub struct BeatActivation {
    /// `max_back + 1` rows of column values; `head` is this hop's row.
    ring: Vec<Vec<f32>>,
    head: usize,
    /// Hops seen so far; offsets reaching before the first hop read as zero.
    seen: usize,
    x: Vec<f32>,
}

impl Default for BeatActivation {
    fn default() -> Self {
        Self::new()
    }
}

impl BeatActivation {
    pub fn new() -> Self {
        let m = model();
        Self {
            ring: vec![vec![0.0; m.columns.len()]; m.max_back + 1],
            head: 0,
            seen: 0,
            x: vec![0.0; m.gather.len()],
        }
    }

    /// Hops between a beat and the hop whose activation reports it: a peak at hop `h`
    /// says the beat was at `h - lag_hops`.
    pub fn lag_hops() -> u32 {
        model().file.lag_hops
    }

    pub fn picker() -> Picker {
        model().file.picker
    }

    /// Score this hop: P(a beat landed `lag_hops` hops ago), 0..1.
    pub fn process(&mut self, ev: &RawEvidence) -> f32 {
        let m = model();
        let len = self.ring.len();
        self.head = (self.head + 1) % len;
        for (k, &column) in m.columns.iter().enumerate() {
            self.ring[self.head][k] = read(column, ev);
        }
        self.seen += 1;
        for (j, &(slot, back)) in m.gather.iter().enumerate() {
            self.x[j] = if back < self.seen {
                self.ring[(self.head + len - back) % len][slot]
            } else {
                0.0
            };
        }
        m.probability(&self.x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_names_resolve_to_the_runtime_slots() {
        assert_eq!(resolve("kf.flux_30_60"), Some(Column::KickFrame(0)));
        assert_eq!(resolve("kf.level_30_60"), Some(Column::KickFrame(11)));
        assert_eq!(resolve("kf.low_peakiness"), Some(Column::KickFrame(23)));
        assert_eq!(resolve("sf.combined"), Some(Column::OnsetFlux(0)));
        assert_eq!(resolve("sf.high"), Some(Column::OnsetFlux(3)));
        assert_eq!(resolve("kp"), Some(Column::KickProbability));
        assert_eq!(resolve("sil"), Some(Column::Silent));
        assert_eq!(resolve("pn.sub_bass"), Some(Column::PreNorm(0)));
        assert_eq!(resolve("pn.kick"), Some(Column::PreNorm(8)));
        assert_eq!(resolve("pn.mfcc.0"), Some(Column::PreNorm(20)));
        assert_eq!(resolve("pn.beat_locked"), Some(Column::PreNorm(84)));
        for bad in ["kf.flux_0_30", "sf.all", "pn.nothing", "kick", "", "kp@-1"] {
            assert_eq!(resolve(bad), None, "{bad}");
        }
    }

    /// The embedded model names columns that exist, looks only backwards, and is causal
    /// to a bounded depth.
    #[test]
    fn the_embedded_model_loads() {
        let m = model();
        assert_eq!(m.gather.len(), m.file.n_features);
        assert!(m.max_back <= 16, "looks back {} hops", m.max_back);
        assert!(m.file.lag_hops <= 4, "lag {}", m.file.lag_hops);
        assert!(!m.columns.is_empty() && !m.file.roots.is_empty());
        assert!(m.file.picker.k >= 1 && (0.0..1.0).contains(&m.file.picker.thr));
    }

    /// An input named `col@-k` is the value `col` had k hops ago, zero before the stream
    /// started: the layout the training rows were built in.
    #[test]
    fn history_is_gathered_by_offset_from_zero() {
        let m = model();
        let frame = [0.0f32; N_FRAME];
        let pre_norm = AudioFeatures::default();
        let mut act = BeatActivation::new();
        // Feed hops whose kick probability counts up; every other column stays 0.
        let (slot, _) = m.gather[m
            .file
            .inputs
            .iter()
            .position(|i| i.name == "kp")
            .expect("the model reads kp")];
        for hop in 1..=6u32 {
            act.process(&RawEvidence {
                kick_frame: &frame,
                kick_probability: hop as f32 * 0.1,
                onset_flux: [0.0; 4],
                pre_norm: &pre_norm,
                loud_silent: false,
            });
            for (j, (s, back)) in m.gather.iter().enumerate() {
                if *s != slot {
                    continue;
                }
                let want = if (*back as u32) < hop {
                    (hop - *back as u32) as f32 * 0.1
                } else {
                    0.0
                };
                assert_eq!(
                    act.x[j], want,
                    "hop {hop}, input {} ({back} back)",
                    m.file.inputs[j].name
                );
            }
        }
    }

    /// The whole runtime against the Python evaluation of the same model on the same
    /// dump: every hop's activation, from the first (zero history) on. The fixture dump
    /// also carries the activation the binary wrote; it must be this code's (to the ulp
    /// the platform's `exp` allows), or the fixture is stale against the embedded model
    /// and needs regenerating.
    #[test]
    fn golden_activation_matches_python() {
        #[derive(Deserialize)]
        struct Rec {
            sil: u8,
            kp: f32,
            kf: Vec<f32>,
            sf: Vec<f32>,
            pn: Vec<f32>,
            act: f32,
        }
        #[derive(Deserialize)]
        struct Golden {
            act: Vec<f32>,
        }
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/beat_model/fixtures/"
        );
        let dump = std::fs::read_to_string(format!("{dir}click_200hops.jsonl")).unwrap();
        let golden: Golden = serde_json::from_str(
            &std::fs::read_to_string(format!("{dir}click_200hops.golden.json")).unwrap(),
        )
        .unwrap();
        let mut act = BeatActivation::new();
        let mut n = 0;
        for line in dump.lines().skip(1) {
            let r: Rec = serde_json::from_str(line).unwrap();
            let mut frame = [0.0f32; N_FRAME];
            frame.copy_from_slice(&r.kf);
            let mut pre_norm = AudioFeatures::default();
            pre_norm.as_slice_mut().copy_from_slice(&r.pn);
            let a = act.process(&RawEvidence {
                kick_frame: &frame,
                kick_probability: r.kp,
                onset_flux: [
                    f64::from(r.sf[0]),
                    f64::from(r.sf[1]),
                    f64::from(r.sf[2]),
                    f64::from(r.sf[3]),
                ],
                pre_norm: &pre_norm,
                loud_silent: r.sil != 0,
            });
            assert!(
                (a - golden.act[n]).abs() < 1e-5,
                "hop {n}: runtime {a} vs python {}",
                golden.act[n]
            );
            // The binary's own value, within an ulp or two: f32::exp is the platform's libm,
            // and macOS arm64 rounds it differently from Linux. A stale fixture against a
            // re-embedded model differs by orders of magnitude more than this.
            assert!(
                (a - r.act).abs() < 1e-5,
                "hop {n}: the fixture's activation {} is not this model's {a} (regenerate it)",
                r.act
            );
            n += 1;
        }
        assert_eq!(n, golden.act.len());
        assert!(n >= 100);
    }

    /// Per-hop cost against the kick model, at full clock: the round's cap is 3x.
    #[test]
    #[ignore = "timing; run alone with --nocapture"]
    fn cost_per_hop_vs_kick_model() {
        use super::super::kick_model::KickDetector;
        use std::time::Instant;
        let n = 20_000;
        let mut lcg = 0x1234_5678u32;
        let mut rnd = || {
            lcg = lcg.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (lcg >> 8) as f32 / 16_777_216.0
        };
        let large: Vec<Vec<f32>> = (0..64)
            .map(|_| (0..2049).map(|_| rnd() * 0.01).collect())
            .collect();
        let medium: Vec<Vec<f32>> = (0..64)
            .map(|_| (0..513).map(|_| rnd() * 0.01).collect())
            .collect();
        let mut kick = KickDetector::new(44100.0, 4096, 1024);
        let t = Instant::now();
        let mut acc = 0.0;
        for i in 0..n {
            acc += kick.process(
                &large[i % 64],
                &large[(i + 1) % 64],
                &medium[i % 64],
                &medium[(i + 1) % 64],
                2e-3,
            );
        }
        let kick_us = t.elapsed().as_secs_f64() * 1e6 / n as f64;

        let frames: Vec<[f32; N_FRAME]> = (0..64).map(|_| std::array::from_fn(|_| rnd())).collect();
        let pre: Vec<AudioFeatures> = (0..64)
            .map(|_| {
                let mut f = AudioFeatures::default();
                for v in f.as_slice_mut() {
                    *v = rnd();
                }
                f
            })
            .collect();
        let mut act = BeatActivation::new();
        let t = Instant::now();
        for i in 0..n {
            acc += act.process(&RawEvidence {
                kick_frame: &frames[i % 64],
                kick_probability: rnd(),
                onset_flux: [f64::from(rnd()); 4],
                pre_norm: &pre[i % 64],
                loud_silent: false,
            });
        }
        let beat_us = t.elapsed().as_secs_f64() * 1e6 / n as f64;
        let m = model();
        println!(
            "kick model {kick_us:.2} us/hop ({} trees); beat model {beat_us:.2} us/hop ({} trees, {} nodes, {} inputs, {} columns, {} back) ratio {:.2} (acc {acc})",
            300,
            m.file.roots.len(),
            m.file.feature.len(),
            m.gather.len(),
            m.columns.len(),
            m.max_back,
            beat_us / kick_us
        );
        assert!(
            beat_us <= 3.0 * kick_us,
            "beat model {beat_us:.2} us vs kick {kick_us:.2} us"
        );
    }
}
