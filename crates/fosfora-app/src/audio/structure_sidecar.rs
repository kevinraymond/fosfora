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
//! It also carries the drop state machine's per-tick internals ([`DropTrace`], #2211). Those
//! come from neither source above: `update_drop` reads the **pre-normalization**
//! `loudness_m` and `sub_bass`, which nothing on the wire carries (`/energy` is *short-term*
//! loudness; `feat/sub_bass` has been adaptively re-ranged). The tracker hands over the
//! conjuncts it actually evaluated, so a replay is checking the detector rather than a
//! lookalike built from proxies.
//!
//! v3 adds `d_kick` / `d_perc` / `d_hratio` for the same reason and from the same source
//! (#2299). They are *candidate* conjuncts, not evaluated ones — `update_drop` does not read
//! them yet — but it is handed `pre_norm`, so those are the values a new conjunct would see.
//! The wire's `/stem/drums/energy` and `/stem/melody/energy` are the live counterparts of
//! two of them and are **adaptively re-ranged** (`audio::schema`): a percentile-ranged
//! feature saturates while the signal is stable, which is precisely the failure mode that
//! made `subbass_ref` useless as a gate (#2300). Sweeping the ranged proxy would measure
//! the ranger, not the music.
//!
//! v4 opens up the build-up logistic itself (#2370). Every round before it could replay
//! everything *downstream* of `buildup_logistic` and nothing inside it, because only the
//! output (`d_build`) was recorded — which is why the five weights have sat at their original
//! values while the arm constants on top of them were swept four times. The four clamped terms
//! now ride along with their raw ingredients, so a sweep can re-derive each term at any gain
//! (`d_f_*` proves it re-derived them correctly) and reconstruct `d_build` exactly. The meta
//! line grows a `consts` object for the same reason: `CENTROID_RISE_GAIN`, `BUILD_TAU` and the
//! rest are module constants rather than live config, and a Python replay mirroring them by
//! hand is a second copy of a number waiting to drift out of step with this one.
//!
//! Never set the variable in live use: the writer does file I/O on the analysis thread.
//! The bench drives it per track via `--signal-dump`, where the "audio thread" is an
//! offline file loop.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};

use super::features::AudioFeatures;
use super::loudness::{LUFS_SPAN_LU, TREND_RANGE_LU};
use super::structure::{
    BUILD_TAU, CENTROID_RISE_GAIN, DropTrace, ONSET_FAST_SECONDS, ONSET_RISE_GAIN, SLOPE_SECONDS,
    STRUCTURE_TICK_HZ, StructureConfig,
};

/// Fingerprint dimension: 7 bands + MFCC 1..=8 + 12 chroma. Deliberately identical to
/// `analyze::structure_offline::FP_DIM` — the two paths must agree on what "sounds the
/// same" means, and the offline segmenter is the reference implementation.
pub const FP_DIM: usize = 27;

/// Records per second. Matches the structure tracker's internal decimation, so a replay
/// sees exactly the tick grid the detector runs on.
const TICK_HZ: f64 = 10.0;

/// Sidecar schema version. v1 was fingerprint + label-machine fields only; v2 adds the
/// drop-machine trace and the leading `meta` line; v3 adds the HPSS trio the precision
/// round needs (#2299); v4 adds the build-up logistic's own inputs and the `consts` object
/// (#2370). Readers must skip the meta line and may ignore unknown fields, so a v1 reader
/// keeps working on a v2 file once it does — and every bump since has stayed additive for
/// exactly that reason: the Harmonix corpus is 374 v2 sidecars nobody is re-dumping.
const SCHEMA_VERSION: u32 = 4;

pub struct StructureSidecar {
    out: BufWriter<File>,
    last_tick: f64,
    /// The meta line is written on the first `record`, because the tracker's live config is
    /// not known at construction (it arrives per hop from the shared `Arc<Mutex<_>>`).
    wrote_meta: bool,
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
            wrote_meta: false,
        })
    }

    /// `pre_norm` feeds the fingerprint, `live` feeds the label machine, `trace` is the drop
    /// machine's own account of the tick. Self-decimates to [`TICK_HZ`] on the same "first
    /// call at or past the interval" rule the tracker uses — `trace.tick_index` lets a reader
    /// verify that alignment rather than assume it.
    pub fn record(
        &mut self,
        timestamp: f64,
        pre_norm: &AudioFeatures,
        live: &AudioFeatures,
        trace: &DropTrace,
        cfg: &StructureConfig,
    ) {
        if self.last_tick >= 0.0 && timestamp - self.last_tick < 1.0 / TICK_HZ {
            return;
        }
        self.last_tick = timestamp;

        if !self.wrote_meta {
            self.wrote_meta = true;
            let meta = format!(
                "{{\"meta\":1,\"schema\":{SCHEMA_VERSION},\"tick_hz\":{TICK_HZ},\"cfg\":{{\
\"buildup_bias\":{},\"buildup_w_loud\":{},\"buildup_w_centroid\":{},\"buildup_w_onset\":{},\
\"buildup_w_subbass\":{},\"drop_arm_buildup\":{},\"drop_arm_sustain\":{},\"drop_arm_hold\":{},\
\"drop_loud_jump\":{},\"drop_baseline_seconds\":{},\"drop_subbass_return\":{},\
\"drop_refractory\":{}}},\"consts\":{{\"centroid_rise_gain\":{},\"onset_rise_gain\":{},\
\"build_tau\":{},\"slope_seconds\":{},\"onset_fast_seconds\":{},\"trend_range_lu\":{},\
\"lufs_span_lu\":{},\"tick_hz\":{}}}}}\n",
                cfg.buildup_bias,
                cfg.buildup_w_loud,
                cfg.buildup_w_centroid,
                cfg.buildup_w_onset,
                cfg.buildup_w_subbass,
                cfg.drop_arm_buildup,
                cfg.drop_arm_sustain,
                cfg.drop_arm_hold,
                cfg.drop_loud_jump,
                cfg.drop_baseline_seconds,
                cfg.drop_subbass_return,
                cfg.drop_refractory,
                // Not config: module constants the logistic is built from. Written so a
                // replay reads them rather than restating them (#2370).
                CENTROID_RISE_GAIN,
                ONSET_RISE_GAIN,
                BUILD_TAU,
                SLOPE_SECONDS,
                ONSET_FAST_SECONDS,
                TREND_RANGE_LU,
                LUFS_SPAN_LU,
                STRUCTURE_TICK_HZ,
            );
            let _ = self.out.write_all(meta.as_bytes());
        }

        let fp = fingerprint(pre_norm);
        let mut line = format!(
            "{{\"ts\":{timestamp:.4},\"loud_pre\":{:.5e},\"loud_s\":{:.5e},\"buildup\":{:.4},\"drop\":{:.1},\"bar_index\":{:.1},\"bar_phase\":{:.4},\"downbeat\":{:.1},\"sub_bass\":{:.5e},\"bass\":{:.5e},\
\"d_tick\":{},\"d_t\":{:.4},\"d_loud_m\":{:.5e},\"d_sub\":{:.5e},\"d_sub_ref\":{:.5e},\"d_build\":{:.5e},\
\"d_high\":{:.4},\"d_base\":{:.5e},\"d_jump\":{:.5e},\"d_ring\":{},\"d_armed\":{},\"d_subret\":{},\
\"d_refrac\":{},\"d_fired\":{},\"d_kick\":{:.5e},\"d_perc\":{:.5e},\"d_hratio\":{:.5e},\
\"d_f_loud\":{:.5e},\"d_f_cent\":{:.5e},\"d_f_onset\":{:.5e},\"d_f_subgone\":{:.5e},\
\"d_loud_trend\":{:.5e},\"d_loud_s\":{:.5e},\"d_cent\":{:.5e},\"d_cent_slow\":{:.5e},\
\"d_onset_fast\":{:.5e},\"d_onset_slow\":{:.5e},\"d_sub_slow\":{:.5e},\"fp\":[",
            pre_norm.loudness_s,
            live.loudness_s,
            live.buildup,
            live.drop,
            live.bar_index,
            live.bar_phase,
            live.downbeat,
            live.sub_bass,
            live.bass,
            trace.tick_index,
            trace.tick_time,
            trace.loud_m,
            trace.sub_bass,
            trace.subbass_ref,
            trace.buildup,
            trace.high_duration,
            trace.baseline,
            trace.jump,
            trace.ring_len,
            u8::from(trace.armed),
            u8::from(trace.sub_returning),
            u8::from(trace.in_refractory),
            u8::from(trace.fired),
            pre_norm.kick,
            pre_norm.percussive_energy,
            pre_norm.harmonic_ratio,
            // v4 (#2370): from the TRACE, not from `pre_norm`. The trio above are candidate
            // conjuncts nothing evaluates, so the record-time snapshot is the honest source
            // for them; these are values the tick actually computed, and taking them off a
            // re-read here would be measuring a lookalike.
            trace.build.f_loud,
            trace.build.f_centroid,
            trace.build.f_onset,
            trace.build.f_subbass_gone,
            trace.build.loud_trend,
            trace.build.loud_s,
            trace.build.centroid,
            trace.build.centroid_slow,
            trace.build.onset_fast,
            trace.build.onset_slow,
            trace.build.subbass_slow,
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
