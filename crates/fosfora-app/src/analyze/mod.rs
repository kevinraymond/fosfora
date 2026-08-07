//! Offline song analysis (#2027): `--analyze <file>`, no GPU and no window.
//!
//! Runs the *production* per-hop chain ([`crate::audio::hop::HopAnalyzer`]) over a decoded
//! file, faster than realtime, so the numbers it reports are the numbers the bindings would
//! see live. What it adds over the live path is **lookahead** — the whole song is in hand
//! before anything is decided, which is what the parked Movements work (#1488) never had:
//!
//! * no cold start — every section is known before any is labelled;
//! * no material-dependent boundary lag — boundaries land on the novelty peak itself;
//! * no running-max saturation — features are ranged over the whole song, not a causal ~4 s
//!   percentile window.

pub mod decode;
pub mod report;
pub mod schema_dump;
pub mod structure_offline;
pub mod validate;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::audio::hop::HopAnalyzer;
use crate::audio::structure::StructureConfig;
use crate::audio::{ANALYSIS_HOP, AudioFeatures, TempoConfig};
use crate::settings::BandScale;

/// Per-hop analysis output, in hop order.
pub struct HopStream {
    pub sample_rate: f32,
    pub source_channels: usize,
    pub duration_secs: f64,
    /// Sample-clock time of each hop, seconds.
    pub timestamps: Vec<f64>,
    /// Live-equivalent features: normalized by the causal percentile window, then smoothed.
    /// This is exactly what a binding would have read at that moment.
    pub live: Vec<AudioFeatures>,
    /// Pre-normalization, pre-smoothing features. See [`crate::audio::hop::HopOutput::pre_norm`]
    /// for the 13 fields that lag by one hop in this stream.
    pub raw: Vec<AudioFeatures>,
    /// Hop indices at which each pulse fired.
    pub beats: Vec<usize>,
    pub downbeats: Vec<usize>,
    pub drops: Vec<usize>,
}

impl HopStream {
    pub fn hop_hz(&self) -> f32 {
        self.sample_rate / ANALYSIS_HOP as f32
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    /// Hop indices converted to seconds on the sample clock.
    pub fn times_of(&self, hops: &[usize]) -> Vec<f64> {
        hops.iter().map(|&h| self.timestamps[h]).collect()
    }
}

/// Decode `path` and run every complete hop through the production chain.
pub fn analyze_file(path: &Path) -> Result<HopStream> {
    let audio = decode::decode_file(path)?;
    log::info!(
        "decoded {} — {:.1}s, {} Hz, {} source channel(s)",
        path.display(),
        audio.duration_secs(),
        audio.sample_rate,
        audio.source_channels,
    );
    Ok(drive(&audio))
}

/// Drive [`HopAnalyzer`] over decoded audio, handing every complete hop's full
/// [`crate::audio::hop::HopOutput`] to `f`. The offline scene renderer consumes
/// this directly — `HopOutput` carries `frame.mel`/`spectrum`/`dmfcc`, which
/// [`HopStream`] deliberately drops — while [`drive`] remains the analysis
/// entry point. One implementation of the mono fold and the sample clock.
pub fn drive_with(
    audio: &decode::DecodedAudio,
    mut f: impl FnMut(usize, f64, &crate::audio::hop::HopOutput),
) {
    let sample_rate = audio.sample_rate;
    // Defaults throughout: this is a measurement tool, so it must not inherit whatever the
    // operator happens to have left in the UI. The band scale matters — `Db` is the shipped
    // default and the one every detector was tuned against.
    let mut analyzer = HopAnalyzer::new(sample_rate, BandScale::Db, TempoConfig::default());
    let struct_cfg = StructureConfig::default();
    let tempo_cfg = TempoConfig::default();

    let hops = audio.frames() / ANALYSIS_HOP;
    let mut mono = vec![0.0f32; ANALYSIS_HOP];
    for h in 0..hops {
        let base = h * ANALYSIS_HOP;
        let stereo = &audio.interleaved[base * 2..(base + ANALYSIS_HOP) * 2];
        // Same mono fold the audio thread applies to the capture ring.
        for (m, s) in mono.iter_mut().zip(stereo.chunks_exact(2)) {
            *m = (s[0] + s[1]) * 0.5;
        }
        // Identical to `audio_thread`'s sample clock: hops are exactly ANALYSIS_HOP apart.
        let timestamp = ((base + ANALYSIS_HOP) as f64) / f64::from(sample_rate);

        let o = analyzer.process_hop(&mono, stereo, timestamp, struct_cfg, tempo_cfg, Vec::new());
        f(h, timestamp, &o);
    }
}

/// Drive [`HopAnalyzer`] over decoded audio into a [`HopStream`]. Split out so
/// tests can feed synthetic signal without touching the filesystem.
pub fn drive(audio: &decode::DecodedAudio) -> HopStream {
    let hops = audio.frames() / ANALYSIS_HOP;
    let mut out = HopStream {
        sample_rate: audio.sample_rate,
        source_channels: audio.source_channels,
        duration_secs: audio.duration_secs(),
        timestamps: Vec::with_capacity(hops),
        live: Vec::with_capacity(hops),
        raw: Vec::with_capacity(hops),
        beats: Vec::new(),
        downbeats: Vec::new(),
        drops: Vec::new(),
    };
    drive_with(audio, |h, timestamp, o| {
        if o.beat_fired {
            out.beats.push(h);
        }
        if o.downbeat_fired {
            out.downbeats.push(h);
        }
        if o.drop_fired {
            out.drops.push(h);
        }
        out.timestamps.push(timestamp);
        out.live.push(o.frame.features);
        out.raw.push(o.pre_norm);
    });
    out
}

/// `--analyze <file> [--out <path>] [--dense]` entry point. Returns the path written.
pub fn run(path: &Path, out: Option<&Path>, dense: bool) -> Result<PathBuf> {
    let stream = analyze_file(path)?;
    let sections = structure_offline::segment(&stream);
    let report = report::build(path, &stream, &sections, dense);

    let out_path = out.map_or_else(|| path.with_extension("analysis.json"), Path::to_path_buf);
    let json = serde_json::to_string_pretty(&report).context("serializing analysis")?;
    std::fs::write(&out_path, json).with_context(|| format!("writing {}", out_path.display()))?;
    Ok(out_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `HopStream` directly from synthetic features. This exercises the *new* offline
    /// logic (novelty → peak-pick → cluster → rank) without paying for the FFT chain, which the
    /// `audio_thread_golden_vector` test already pins.
    fn synthetic_stream(sections: &[(f32, [f32; 7], usize)], secs_each: f64) -> HopStream {
        const HOP_HZ: f32 = 86.13281;
        let sample_rate = HOP_HZ * ANALYSIS_HOP as f32;
        let per = (secs_each * f64::from(HOP_HZ)) as usize;

        let mut live = Vec::new();
        let mut raw = Vec::new();
        let mut timestamps = Vec::new();
        for (si, &(level, bands, chroma_root)) in sections.iter().enumerate() {
            for i in 0..per {
                let mut f = AudioFeatures {
                    sub_bass: bands[0],
                    bass: bands[1],
                    low_mid: bands[2],
                    mid: bands[3],
                    upper_mid: bands[4],
                    presence: bands[5],
                    brilliance: bands[6],
                    loudness_s: level,
                    rms: level,
                    ..Default::default()
                };
                f.chroma[chroma_root] = 1.0;
                // MFCC 1..=8 vary with the band shape so the timbre half of the fingerprint
                // carries signal too, not just chroma.
                for (k, m) in f.mfcc.iter_mut().enumerate().take(9).skip(1) {
                    *m = bands[k % 7] * 0.5;
                }
                live.push(f);
                raw.push(f);
                timestamps.push((si * per + i) as f64 / f64::from(HOP_HZ));
            }
        }
        HopStream {
            sample_rate,
            source_channels: 2,
            duration_secs: sections.len() as f64 * secs_each,
            timestamps,
            live,
            raw,
            beats: Vec::new(),
            downbeats: Vec::new(),
            drops: Vec::new(),
        }
    }

    const VERSE: (f32, [f32; 7], usize) = (0.35, [0.8, 0.7, 0.3, 0.2, 0.1, 0.1, 0.1], 0);
    const CHORUS: (f32, [f32; 7], usize) = (0.85, [0.2, 0.3, 0.4, 0.8, 0.7, 0.6, 0.5], 7);

    /// The headline claim of #2028: offline, a boundary lands ON the seam. The causal tracker
    /// reported the same seams +3.5..4.6 s late, by a margin that varied with the material.
    #[test]
    fn boundaries_land_on_the_seam_with_no_confirmation_lag() {
        let stream = synthetic_stream(&[VERSE, CHORUS, VERSE, CHORUS], 20.0);
        let seg = structure_offline::segment(&stream);

        let bounds: Vec<f64> = seg.sections.iter().skip(1).map(|s| s.start_secs).collect();
        assert_eq!(bounds.len(), 3, "expected 3 seams, got {bounds:?}");
        for (got, want) in bounds.iter().zip([20.0, 40.0, 60.0]) {
            assert!(
                (got - want).abs() < 1.0,
                "seam at {got:.2}s should be {want}s (offline lag must be ~0, not seconds)"
            );
        }
    }

    /// A' must resolve to A's identity and B' to B's — the recall that regressed on the harder
    /// track online, because a material-dependent lag leaked the next section's audio into the
    /// closing fingerprint.
    #[test]
    fn returning_sections_recall_their_identity() {
        let stream = synthetic_stream(&[VERSE, CHORUS, VERSE, CHORUS], 20.0);
        let seg = structure_offline::segment(&stream);
        assert_eq!(seg.sections.len(), 4);
        assert_eq!(seg.cluster_count, 2, "verse and chorus are two identities");
        assert_eq!(
            seg.sections[0].cluster, seg.sections[2].cluster,
            "A' recalls A"
        );
        assert_eq!(
            seg.sections[1].cluster, seg.sections[3].cluster,
            "B' recalls B"
        );
        assert_ne!(seg.sections[0].cluster, seg.sections[1].cluster);
    }

    /// `chorus_likeness` measured 0.118 of a possible 1.0 online. The offline rank must
    /// actually separate the two.
    #[test]
    fn energy_rank_separates_verse_from_chorus() {
        let stream = synthetic_stream(&[VERSE, CHORUS, VERSE, CHORUS], 20.0);
        let seg = structure_offline::segment(&stream);
        let verse = seg.sections[0].energy_rank.max(seg.sections[2].energy_rank);
        let chorus = seg.sections[1].energy_rank.min(seg.sections[3].energy_rank);
        assert!(
            chorus - verse > 0.5,
            "separation {:.3} is not a control (online managed 0.118)",
            chorus - verse
        );
    }

    /// Steady material has no structure. Reporting seams in it would be worse than reporting
    /// none, because a generated scene would cut on nothing.
    #[test]
    fn steady_material_yields_one_section_and_no_manufactured_contrast() {
        let stream = synthetic_stream(&[VERSE, VERSE, VERSE], 20.0);
        let seg = structure_offline::segment(&stream);
        assert_eq!(
            seg.cluster_count, 1,
            "one identity, got {}",
            seg.cluster_count
        );
        assert!(
            seg.sections.iter().all(|s| s.energy_rank == 0.0),
            "uniform level must report rank 0, not invented contrast"
        );
    }

    /// Cold start, the regime that killed the online version (#1977): a track short enough to
    /// hold one or two sections must still produce sane output rather than panicking or
    /// pinning every rank to an endpoint.
    #[test]
    fn one_and_two_section_songs_are_first_class() {
        for n in 1..=2 {
            let secs: Vec<_> = std::iter::repeat_n(VERSE, n).collect();
            let stream = synthetic_stream(&secs, 20.0);
            let seg = structure_offline::segment(&stream);
            assert!(!seg.sections.is_empty(), "n={n} produced no sections");
            assert!(seg.sections.iter().all(|s| s.energy_rank.is_finite()));
        }
    }

    /// THE cross-check: the offline driver must reproduce, sample for sample, what the live
    /// audio thread produces from the same signal. If this ever drifts, every number the tool
    /// reports stops describing the app the scene will actually run in.
    ///
    /// Asserted against the *same* pinned vector `audio_thread_golden_vector` uses, so the two
    /// paths are tied to one set of expected values rather than to each other.
    #[test]
    fn offline_driver_matches_the_live_audio_thread() {
        const SR: f32 = 44100.0;
        let signal = crate::audio::tests::golden_signal(SR, 4.0);
        let audio = decode::DecodedAudio {
            interleaved: signal,
            sample_rate: SR,
            source_channels: 2,
        };
        let stream = drive(&audio);
        assert_eq!(stream.len(), 344, "offline must produce the same hop count");

        for (hop, expected) in crate::audio::tests::GOLDEN_HOPS {
            let got = stream.live[*hop].as_slice();
            for (i, (&g, &e)) in got.iter().zip(expected.iter()).enumerate() {
                assert!(
                    (g - e).abs() < 1e-5,
                    "hop {hop} feature {i} ({}): offline {g}, live {e}",
                    crate::audio::schema::FEATURES[i].name,
                );
            }
        }
    }

    #[test]
    fn empty_stream_does_not_panic() {
        let stream = synthetic_stream(&[], 0.0);
        let seg = structure_offline::segment(&stream);
        assert!(seg.sections.is_empty());
    }

    /// Q1 Stage 4 acceptance gate: a beat's emitted time must sit ON the hit
    /// that caused it. Drives the REAL windowed analysis chain with a
    /// synthesized kick track and requires the median beat-to-hit offset
    /// inside ±25 ms.
    ///
    /// Pre-registered before the PLL scheduler lands, and ignored until then:
    /// today it fails at +156 ms — NOT window latency (measured onset-detection
    /// lag on these hits is +14 ms, and matched beats on the real-music dev
    /// subset sit at −6.6 ms median), but the current scheduler re-anchoring
    /// its grid onto late tail onsets of each hit, which is exactly what the
    /// Stage 4 rewrite removes. A window-latency constant was measured
    /// unnecessary and deliberately NOT added; this test would catch anyone
    /// re-adding one (median would go ≈ −46 ms).
    #[test]
    fn beat_times_land_on_the_click() {
        const SR: f32 = 44100.0;
        const SECS: f64 = 30.0;
        const PERIOD: f64 = 0.5; // 120 BPM

        let frames = (SECS * SR as f64) as usize;
        let mut interleaved = vec![0.0f32; frames * 2];
        // Bed: quiet tone (keeps momentary loudness above the −55 LUFS silence
        // gate) plus a deterministic noise floor. The floor matters: without it
        // the flux MAD between hits collapses, the adaptive onset threshold
        // bottoms out, and spurious tail onsets gate through continuously —
        // a regime real music never presents.
        let mut rng: u64 = 0x9E37_79B9_7F4A_7C15;
        for i in 0..frames {
            let t = i as f64 / f64::from(SR);
            rng = rng
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let noise = f64::from((rng >> 32) as u32) / (1u64 << 31) as f64 - 1.0;
            let bed = (0.04 * (2.0 * std::f64::consts::PI * 110.0 * t).sin() + 0.02 * noise) as f32;
            interleaved[i * 2] = bed;
            interleaved[i * 2 + 1] = bed;
        }
        // Hits: kick-shaped — a 60 Hz decaying sine with a 5 ms attack ramp plus
        // a short broadband transient, ~80 ms total. Sharp enough to have a
        // definite instant, shaped enough to exercise the real flux dynamics.
        let mut clicks = Vec::new();
        let mut tc = 1.0f64;
        while tc < SECS - 0.5 {
            clicks.push(tc);
            let c0 = (tc * f64::from(SR)) as usize;
            let n_hit = (0.080 * f64::from(SR)) as usize;
            let mut hrng: u64 = 0xDEAD_BEEF_CAFE_F00D;
            for j in 0..n_hit {
                let tj = j as f64 / f64::from(SR);
                let attack = (tj / 0.005).min(1.0);
                let body = (2.0 * std::f64::consts::PI * 60.0 * tj).sin() * (-tj / 0.030).exp();
                hrng = hrng
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let tnoise = (f64::from((hrng >> 32) as u32) / (1u64 << 31) as f64 - 1.0)
                    * (-tj / 0.008).exp();
                let s = (attack * (0.8 * body + 0.5 * tnoise)) as f32;
                interleaved[(c0 + j) * 2] += s;
                interleaved[(c0 + j) * 2 + 1] += s;
            }
            tc += PERIOD;
        }
        let audio = decode::DecodedAudio {
            interleaved,
            sample_rate: SR,
            source_channels: 2,
        };

        let mut beat_times = Vec::new();
        drive_with(&audio, |_h, _ts, o| {
            if o.beat_fired {
                beat_times.push(o.frame.beat_time.expect("fired beat must carry beat_time"));
            }
        });

        // Same convention as the bench: skip the acquisition head.
        let mut offsets: Vec<f64> = beat_times
            .iter()
            .filter(|&&b| b > 5.0)
            .map(|&b| {
                clicks
                    .iter()
                    .map(|&c| b - c)
                    .min_by(|a, b| a.abs().partial_cmp(&b.abs()).unwrap())
                    .unwrap()
            })
            .collect();
        assert!(
            offsets.len() >= 20,
            "scheduler failed to fire on a clean click track: {} beats after 5 s",
            offsets.len()
        );
        offsets.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = offsets[offsets.len() / 2];
        assert!(
            median.abs() <= 0.025,
            "median beat-to-click offset {:+.1} ms over {} beats — beats must land \
             on the click (|median| ≤ 25 ms)",
            median * 1000.0,
            offsets.len(),
        );
    }
}
