//! The `analysis.json` schema (#2027) — what an offline pass says about a song.
//!
//! Consumed by a scene generator, but useful on its own as a "what is this song made of"
//! report. Versioned from the start because a generator will be pinned against it.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::audio::schema;

use super::HopStream;
use super::structure_offline::{Section, Segmentation};

/// Bumped on any breaking change to the shape below.
pub const ANALYSIS_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Analysis {
    pub version: u32,
    pub source: SourceInfo,
    pub global: Global,
    pub sections: Vec<SectionReport>,
    pub events: Events,
    /// Per-hop feature streams. Present only with `--dense`; a 4-minute song is ~20k hops ×
    /// 81 features, so this is tens of MB of JSON and most consumers want the summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<Frames>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    pub path: String,
    pub duration_secs: f64,
    pub sample_rate: f32,
    /// Channels in the file. 1 means the stereo field below is synthesized, not measured.
    pub source_channels: usize,
    /// Analysis frames per second (`sample_rate / 512`).
    pub hop_hz: f32,
    pub hop_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Global {
    /// Median BPM over hops where the estimator was confident enough to report one.
    pub bpm: f32,
    pub beat_count: usize,
    pub downbeat_count: usize,
    /// Modal key across the song: pitch class 0..11, minor flag, and the share of hops
    /// agreeing with it.
    pub key_class: u32,
    pub key_is_minor: bool,
    pub key_agreement: f32,
    /// Mean short-term loudness over sounding hops, 0..1 (the schema's 0..1 LUFS mapping).
    pub loudness_s_mean: f32,
    pub cluster_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionReport {
    pub index: usize,
    pub start_secs: f64,
    pub end_secs: f64,
    pub duration_secs: f64,
    /// Sections sharing a label are the same musical section returning.
    pub label: String,
    pub cluster: usize,
    pub energy: f32,
    /// 0..1 across the whole song — the offline replacement for `chorus_likeness`.
    pub energy_rank: f32,
    /// 27-dim identity: 7 bands + MFCC 1..=8 + 12 chroma, unit-normalized.
    pub fingerprint: Vec<f32>,
    /// Mean of a handful of named features over the section, for a generator to read without
    /// having to understand the fingerprint.
    pub descriptors: Descriptors,
    /// p10/p50/p90 of each live feature over the section's hop span, keyed by feature name
    /// (no `audio.` prefix). Calibration data for remap input ranges (#2037): a binding reads
    /// exactly these values at runtime, so a remap ranged by them tracks the song instead of
    /// pinning on a compressed master. The mfcc rows are skipped, mirroring the generator's
    /// curated source list. Additive since version 1; absent in older files.
    #[serde(default)]
    pub percentiles: BTreeMap<String, [f32; 3]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Descriptors {
    pub rms: f32,
    pub centroid: f32,
    pub percussive_energy: f32,
    pub harmonic_ratio: f32,
    pub buildup: f32,
    pub stereo_width: f32,
    pub onset_density: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Events {
    pub beats_secs: Vec<f64>,
    pub downbeats_secs: Vec<f64>,
    pub drops_secs: Vec<f64>,
    /// Section boundaries, seconds. These land *on* the novelty peak — offline there is no
    /// confirmation lag to subtract (finding #2028).
    pub boundaries_secs: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frames {
    /// Feature names in slot order, so a consumer never has to hard-code the layout.
    pub feature_names: Vec<String>,
    pub timestamps: Vec<f64>,
    /// Live-equivalent: causally normalized then smoothed, i.e. what a binding read.
    pub live: Vec<Vec<f32>>,
    /// Pre-normalization, pre-smoothing.
    pub raw: Vec<Vec<f32>>,
    /// Novelty curve at `novelty_hz`, absolute 0..1.
    pub novelty: Vec<f32>,
    pub novelty_hz: f32,
}

pub fn build(path: &Path, stream: &HopStream, seg: &Segmentation, dense: bool) -> Analysis {
    Analysis {
        version: ANALYSIS_VERSION,
        source: SourceInfo {
            path: path.display().to_string(),
            duration_secs: stream.duration_secs,
            sample_rate: stream.sample_rate,
            source_channels: stream.source_channels,
            hop_hz: stream.hop_hz(),
            hop_count: stream.len(),
        },
        global: global(stream, seg),
        sections: seg
            .sections
            .iter()
            .enumerate()
            .map(|(i, s)| section_report(i, s, stream))
            .collect(),
        events: Events {
            beats_secs: stream.times_of(&stream.beats),
            downbeats_secs: stream.times_of(&stream.downbeats),
            drops_secs: stream.times_of(&stream.drops),
            boundaries_secs: seg.sections.iter().skip(1).map(|s| s.start_secs).collect(),
        },
        frames: dense.then(|| Frames {
            feature_names: schema::FEATURES
                .iter()
                .map(|f| f.name.to_string())
                .collect(),
            timestamps: stream.timestamps.clone(),
            live: stream.live.iter().map(|f| f.as_slice().to_vec()).collect(),
            raw: stream.raw.iter().map(|f| f.as_slice().to_vec()).collect(),
            novelty: seg.novelty.clone(),
            novelty_hz: seg.novelty_hz,
        }),
    }
}

fn global(stream: &HopStream, seg: &Segmentation) -> Global {
    // BPM is stored as bpm/300; only count hops where the estimator actually reported one.
    let mut bpms: Vec<f32> = stream
        .live
        .iter()
        .map(|f| f.bpm * 300.0)
        .filter(|b| *b > 1.0)
        .collect();
    bpms.sort_by(f32::total_cmp);
    let bpm = if bpms.is_empty() {
        0.0
    } else {
        bpms[bpms.len() / 2]
    };

    // Modal key over hops the detector was confident about, so a few ambiguous frames at a
    // fade cannot outvote the body of the song.
    let mut votes = [0usize; 24];
    let mut voted = 0usize;
    for f in &stream.live {
        if f.key_confidence < 0.5 {
            continue;
        }
        let class = (f.key_class * 11.0).round().clamp(0.0, 11.0) as usize;
        let minor = usize::from(f.key_is_minor > 0.5);
        votes[minor * 12 + class] += 1;
        voted += 1;
    }
    let best = votes
        .iter()
        .enumerate()
        .max_by_key(|&(_, v)| *v)
        .map_or(0, |(i, _)| i);
    let agreement = if voted == 0 {
        0.0
    } else {
        votes[best] as f32 / voted as f32
    };

    let sounding: Vec<f32> = stream
        .live
        .iter()
        .map(|f| f.loudness_s)
        .filter(|l| *l > 0.05)
        .collect();
    let loudness_s_mean = if sounding.is_empty() {
        0.0
    } else {
        sounding.iter().sum::<f32>() / sounding.len() as f32
    };

    Global {
        bpm,
        beat_count: stream.beats.len(),
        downbeat_count: stream.downbeats.len(),
        key_class: (best % 12) as u32,
        key_is_minor: best >= 12,
        key_agreement: agreement,
        loudness_s_mean,
        cluster_count: seg.cluster_count,
    }
}

fn section_report(index: usize, s: &Section, stream: &HopStream) -> SectionReport {
    // Hop range covering this section on the sample clock.
    let lo = stream
        .timestamps
        .partition_point(|&t| t < s.start_secs)
        .min(stream.len().saturating_sub(1));
    let hi = stream
        .timestamps
        .partition_point(|&t| t < s.end_secs)
        .max(lo + 1);
    let span = &stream.live[lo..hi.min(stream.len())];

    let mean = |f: fn(&crate::audio::AudioFeatures) -> f32| {
        if span.is_empty() {
            0.0
        } else {
            span.iter().map(f).sum::<f32>() / span.len() as f32
        }
    };
    // Onsets per second, from the fired-beat list rather than the continuous onset envelope.
    let beats_here = stream.beats.iter().filter(|&&h| h >= lo && h < hi).count();
    let dur = (s.end_secs - s.start_secs).max(1e-3);

    SectionReport {
        percentiles: span_percentiles(span),
        index,
        start_secs: s.start_secs,
        end_secs: s.end_secs,
        duration_secs: dur,
        label: s.label.clone(),
        cluster: s.cluster,
        energy: s.energy,
        energy_rank: s.energy_rank,
        fingerprint: s.fingerprint.to_vec(),
        descriptors: Descriptors {
            rms: mean(|f| f.rms),
            centroid: mean(|f| f.centroid),
            percussive_energy: mean(|f| f.percussive_energy),
            harmonic_ratio: mean(|f| f.harmonic_ratio),
            buildup: mean(|f| f.buildup),
            stereo_width: mean(|f| f.stereo_width),
            onset_density: (beats_here as f64 / dur) as f32,
        },
    }
}

/// p10/p50/p90 per live feature over a hop span, by slot against [`schema::FEATURES`].
fn span_percentiles(span: &[crate::audio::AudioFeatures]) -> BTreeMap<String, [f32; 3]> {
    let mut out = BTreeMap::new();
    for (slot, fdef) in schema::FEATURES.iter().enumerate() {
        if fdef.name.starts_with("mfcc.") {
            continue;
        }
        let mut vals: Vec<f32> = span.iter().map(|f| f.as_slice()[slot]).collect();
        vals.sort_by(f32::total_cmp);
        // Nearest-rank on the sorted span; an empty span reports zeros, matching the
        // descriptor means' guard.
        let pick = |p: f64| {
            if vals.is_empty() {
                0.0
            } else {
                vals[((vals.len() - 1) as f64 * p).round() as usize]
            }
        };
        out.insert(fdef.name.to_string(), [pick(0.10), pick(0.50), pick(0.90)]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioFeatures;

    fn features_with(slot: usize, value: f32) -> AudioFeatures {
        let mut f = AudioFeatures::default();
        f.as_slice_mut()[slot] = value;
        f
    }

    fn slot_of(name: &str) -> usize {
        schema::FEATURES
            .iter()
            .position(|f| f.name == name)
            .unwrap()
    }

    #[test]
    fn percentiles_of_a_constant_span_are_that_constant() {
        let rms = slot_of("rms");
        let span: Vec<AudioFeatures> = (0..50).map(|_| features_with(rms, 0.4)).collect();
        let p = span_percentiles(&span);
        assert_eq!(p["rms"], [0.4, 0.4, 0.4]);
    }

    #[test]
    fn percentiles_of_a_ramp_are_ordered_and_span_the_ramp() {
        let rms = slot_of("rms");
        let span: Vec<AudioFeatures> = (0..101)
            .map(|i| features_with(rms, i as f32 / 100.0))
            .collect();
        let p = p_of(&span, "rms");
        assert!(p[0] <= p[1] && p[1] <= p[2], "unordered: {p:?}");
        assert!(
            (p[0] - 0.10).abs() < 0.02,
            "p10 off a uniform ramp: {}",
            p[0]
        );
        assert!(
            (p[1] - 0.50).abs() < 0.02,
            "p50 off a uniform ramp: {}",
            p[1]
        );
        assert!(
            (p[2] - 0.90).abs() < 0.02,
            "p90 off a uniform ramp: {}",
            p[2]
        );
    }

    #[test]
    fn mfcc_family_is_excluded_and_curated_features_are_present() {
        let p = span_percentiles(&[AudioFeatures::default()]);
        assert!(p.keys().all(|k| !k.starts_with("mfcc.")));
        for key in ["rms", "sub_bass", "buildup", "stereo_width", "chroma.0"] {
            assert!(p.contains_key(key), "missing {key}");
        }
    }

    #[test]
    fn empty_span_reports_zeros_not_a_panic() {
        let p = span_percentiles(&[]);
        assert_eq!(p["rms"], [0.0, 0.0, 0.0]);
        assert!(!p.is_empty());
    }

    fn p_of(span: &[AudioFeatures], name: &str) -> [f32; 3] {
        span_percentiles(span)[name]
    }
}
