//! Dev-only structure-path instrumentation (#2080).
//!
//! When the `FOSFORA_STRUCTURE_SIDECAR` env var names a file, the hop chain appends one
//! JSONL record per decimated tick carrying the boundary detector's raw inputs — the
//! 27-dim timbre fingerprint plus the fields the section *label* machine reads — so
//! offline sweeps (`bench/sweep_structure.py`) can replay the boundary back-end
//! (normalization, peak-picking, dwell, the label state machine's bar gates) against the
//! production front-end's exact output, instead of trusting a Python replica of the FFT.
//!
//! Two feature sources, matching production exactly and for the same reason each one is
//! read there:
//!
//! - the fingerprint comes from the **pre-normalization** snapshot, because the adaptive
//!   percentile window would otherwise have flattened the very dynamics being measured
//!   (`audio::structure`, and `analyze::structure_offline::segment` for the same reason);
//! - the label-machine fields come from the **final smoothed** features, because that is
//!   what `signal::section::HeuristicSectionEstimator::process` is handed on the wire.
//!
//! Never set the variable in live use: the writer does file I/O on the analysis thread.
//! The bench drives it per track via `--signal-dump`, where the "audio thread" is an
//! offline file loop.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};

use super::features::AudioFeatures;

/// Fingerprint dimension: 7 bands + MFCC 1..=8 + 12 chroma. Deliberately identical to
/// `analyze::structure_offline::FP_DIM` — the two paths must agree on what "sounds the
/// same" means, and the offline segmenter is the reference implementation.
pub const FP_DIM: usize = 27;

/// Records per second. Matches the structure tracker's internal decimation, so a replay
/// sees exactly the tick grid the detector runs on.
const TICK_HZ: f64 = 10.0;

pub struct StructureSidecar {
    out: BufWriter<File>,
    last_tick: f64,
}

impl StructureSidecar {
    /// Build the writer iff `FOSFORA_STRUCTURE_SIDECAR` is set; any create/open failure is
    /// loud — a sweep silently missing tracks would corrupt the comparison.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var("FOSFORA_STRUCTURE_SIDECAR").ok()?;
        let file = File::create(&path)
            .unwrap_or_else(|e| panic!("FOSFORA_STRUCTURE_SIDECAR: cannot create {path}: {e}"));
        Some(Self {
            out: BufWriter::new(file),
            last_tick: -1.0,
        })
    }

    /// `pre_norm` feeds the fingerprint, `live` feeds the label machine. Self-decimates to
    /// [`TICK_HZ`] on the same "first call at or past the interval" rule the tracker uses.
    pub fn record(&mut self, timestamp: f64, pre_norm: &AudioFeatures, live: &AudioFeatures) {
        if self.last_tick >= 0.0 && timestamp - self.last_tick < 1.0 / TICK_HZ {
            return;
        }
        self.last_tick = timestamp;

        let fp = fingerprint(pre_norm);
        let mut line = format!(
            "{{\"ts\":{timestamp:.4},\"loud_pre\":{:.5e},\"loud_s\":{:.5e},\"buildup\":{:.4},\"drop\":{:.1},\"bar_index\":{:.1},\"bar_phase\":{:.4},\"downbeat\":{:.1},\"sub_bass\":{:.5e},\"bass\":{:.5e},\"fp\":[",
            pre_norm.loudness_s,
            live.loudness_s,
            live.buildup,
            live.drop,
            live.bar_index,
            live.bar_phase,
            live.downbeat,
            live.sub_bass,
            live.bass,
        );
        for (i, v) in fp.iter().enumerate() {
            if i > 0 {
                line.push(',');
            }
            let _ = write!(line, "{v:.5e}");
        }
        line.push_str("]}\n");
        let _ = self.out.write_all(line.as_bytes());
    }
}

impl Drop for StructureSidecar {
    fn drop(&mut self) {
        let _ = self.out.flush();
    }
}

/// The raw (un-normalized) 27-dim fingerprint. The replay applies the unit-norm itself, so
/// a sweep can try alternative weightings of the chroma block against the timbre block
/// without a rebuild.
fn fingerprint(f: &AudioFeatures) -> [f32; FP_DIM] {
    let mut v = [0.0f32; FP_DIM];
    v[0] = f.sub_bass;
    v[1] = f.bass;
    v[2] = f.low_mid;
    v[3] = f.mid;
    v[4] = f.upper_mid;
    v[5] = f.presence;
    v[6] = f.brilliance;
    v[7..15].copy_from_slice(&f.mfcc[1..9]);
    v[15..27].copy_from_slice(&f.chroma);
    v
}
