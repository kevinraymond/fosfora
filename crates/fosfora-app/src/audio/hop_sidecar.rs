//! Dev-only per-hop evidence dump for the beat-likelihood model (#4163).
//!
//! When the `FOSFORA_HOP_SIDECAR` env var names a file, the hop chain appends one JSONL
//! record per analysis hop with the RAW evidence the detectors read on that hop: the kick
//! model's band features and its probability, the SuperFlux onset flux (combined and per
//! partition), the perceptual-silence flag and the pre-normalization `AudioFeatures`
//! snapshot, plus the beat the grid fired on this hop if any. The feature bus a
//! `--signal-dump` emits is post-normalizer and post-smoother; a model that runs INSIDE
//! the detector chain never sees that form, so it is trained on this instead.
//!
//! Format (v2): the first line is a header (`"k":"h"`) that names every column in order,
//! then one record (`"k":"r"`) per hop, `"i"` counting from 0; `act` is the runtime's own
//! beat activation for the hop. Numbers are the shortest
//! round-trip f32, timestamps six decimals, non-finite values `null`.
//! `scripts/beat_model/hopdump.py` is the reader and must stay in step with this file.
//!
//! The `pn` snapshot is taken before the beat, downbeat and structure blocks fill their
//! slots, so those 17 carry the PREVIOUS hop's value (see `HopOutput::pre_norm`); the
//! reader drops them from model input anyway, since on pseudo-labeled data they are the
//! label.
//!
//! With the variable unset nothing is collected and the signal dump is byte-identical.
//! Never set it in live use: the writer does file I/O on the analysis thread. The bench
//! drives it per track via `--signal-dump`, where the "audio thread" is an offline file
//! loop.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};

use super::beat::ONSET_FLUX_NAMES;
use super::features::AudioFeatures;
use super::kick_model::{FRAME_NAMES, N_FRAME};
use super::schema::FEATURES;

/// What one hop's record carries. Everything is this hop's value as the detectors
/// computed it, nothing smoothed or held.
pub struct HopEvidence<'a> {
    /// Sample-clock time of the hop, seconds.
    pub timestamp: f64,
    /// The A10 perceptual-silence gate the onset detector and `kick` honor.
    pub loud_silent: bool,
    /// The kick model's raw probability, before the silence gate and the `kick` mapping.
    pub kick_probability: f32,
    /// The kick model's input frame for this hop (`kick_model::FRAME_NAMES`).
    pub kick_frame: &'a [f32; N_FRAME],
    /// `BeatDetector::raw_onset_flux`: combined, low, mid, high.
    pub onset_flux: [f64; 4],
    /// The pre-normalization feature snapshot.
    pub pre_norm: &'a AudioFeatures,
    /// The runtime's beat activation for this hop (`beat_model.rs`), so an offline pass
    /// can check its own evaluation of the model against the app's.
    pub beat_activation: f32,
    /// The event time of the beat the grid fired on this hop, if it fired one.
    pub beat_time: Option<f64>,
}

pub struct HopSidecar {
    out: BufWriter<File>,
    path: String,
    sample_rate: f32,
    /// Records written so far; the next record's `"i"`.
    hops: u64,
}

/// Append `v` as a JSON number: the shortest decimal that round-trips, or `null`.
fn push_num(line: &mut String, v: f32) {
    if v.is_finite() {
        let _ = write!(line, "{v}");
    } else {
        line.push_str("null");
    }
}

fn push_list(line: &mut String, values: impl IntoIterator<Item = f32>) {
    for (n, v) in values.into_iter().enumerate() {
        if n > 0 {
            line.push(',');
        }
        push_num(line, v);
    }
}

fn push_names(line: &mut String, names: impl IntoIterator<Item = &'static str>) {
    for (n, name) in names.into_iter().enumerate() {
        if n > 0 {
            line.push(',');
        }
        let _ = write!(line, "\"{name}\"");
    }
}

impl HopSidecar {
    /// Build the writer iff `FOSFORA_HOP_SIDECAR` is set; any create/open failure is
    /// loud, since a sweep silently missing tracks would corrupt the dataset.
    pub fn from_env(sample_rate: f32) -> Option<Self> {
        let path = std::env::var("FOSFORA_HOP_SIDECAR").ok()?;
        let file = super::sidecar_file::create_sidecar("FOSFORA_HOP_SIDECAR", &path);
        Some(Self::new(file, path, sample_rate))
    }

    pub(crate) fn new(file: File, path: String, sample_rate: f32) -> Self {
        Self {
            out: BufWriter::new(file),
            path,
            sample_rate,
            hops: 0,
        }
    }

    fn header(&self) -> String {
        let mut line = String::with_capacity(2048);
        let _ = write!(
            line,
            "{{\"k\":\"h\",\"v\":2,\"sr\":{},\"hop\":{},\"kf\":[",
            self.sample_rate,
            super::ANALYSIS_HOP
        );
        push_names(&mut line, FRAME_NAMES);
        line.push_str("],\"sf\":[");
        push_names(&mut line, ONSET_FLUX_NAMES);
        line.push_str("],\"pn\":[");
        push_names(&mut line, FEATURES.iter().map(|d| d.name));
        line.push_str("]}\n");
        line
    }

    pub fn record(&mut self, ev: &HopEvidence) {
        let mut line = if self.hops == 0 {
            self.header()
        } else {
            String::with_capacity(1536)
        };
        let _ = write!(
            line,
            "{{\"k\":\"r\",\"i\":{},\"ts\":{:.6},\"sil\":{},\"kp\":",
            self.hops,
            ev.timestamp,
            u8::from(ev.loud_silent)
        );
        push_num(&mut line, ev.kick_probability);
        line.push_str(",\"kf\":[");
        push_list(&mut line, ev.kick_frame.iter().copied());
        line.push_str("],\"sf\":[");
        push_list(&mut line, ev.onset_flux.iter().map(|&v| v as f32));
        line.push_str("],\"pn\":[");
        push_list(&mut line, ev.pre_norm.as_slice().iter().copied());
        line.push_str("],\"act\":");
        push_num(&mut line, ev.beat_activation);
        line.push_str(",\"bt\":");
        match ev.beat_time {
            Some(t) if t.is_finite() => {
                let _ = write!(line, "{t:.6}");
            }
            _ => line.push_str("null"),
        }
        line.push_str("}\n");
        self.hops += 1;
        // As loud as a failed create: a full disk otherwise truncates the file, the
        // process exits 0, and the bench keeps it as a whole track (#106).
        if let Err(e) = self.out.write_all(line.as_bytes()) {
            panic!("FOSFORA_HOP_SIDECAR: write to {} failed: {e}", self.path);
        }
    }
}

impl Drop for HopSidecar {
    fn drop(&mut self) {
        if let Err(e) = self.out.flush() {
            // Already unwinding: a second panic would abort, and the first one
            // has made the run fail anyway.
            assert!(
                std::thread::panicking(),
                "FOSFORA_HOP_SIDECAR: flush to {} failed: {e}",
                self.path
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::features::NUM_FEATURES;

    fn evidence<'a>(
        frame: &'a [f32; N_FRAME],
        pre_norm: &'a AudioFeatures,
        beat_time: Option<f64>,
    ) -> HopEvidence<'a> {
        HopEvidence {
            timestamp: 512.0 / 44100.0,
            loud_silent: false,
            kick_probability: 0.25,
            kick_frame: frame,
            onset_flux: [0.5, 0.75, 0.125, 0.0],
            pre_norm,
            beat_activation: 0.5,
            beat_time,
        }
    }

    /// The header names every column in the order the records carry them, each hop is one
    /// record counted from 0, a non-finite feature is `null`, and `bt` is the beat time or
    /// `null`.
    #[test]
    fn header_names_every_column_then_one_record_per_hop() {
        let path = std::env::temp_dir().join(format!("hop_sidecar_{}.jsonl", std::process::id()));
        let frame: [f32; N_FRAME] = std::array::from_fn(|i| i as f32 * 0.5);
        let pre_norm = AudioFeatures {
            sub_bass: 0.125,
            flux: f32::NAN,
            ..AudioFeatures::default()
        };
        {
            let mut sc = HopSidecar::new(
                File::create(&path).unwrap(),
                path.display().to_string(),
                44100.0,
            );
            sc.record(&evidence(&frame, &pre_norm, None));
            sc.record(&evidence(&frame, &pre_norm, Some(1.25)));
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");

        let header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(header["k"], "h");
        assert_eq!(header["v"], 2);
        assert_eq!(header["sr"], 44100);
        assert_eq!(header["hop"], 512);
        let names = |key: &str| -> Vec<String> {
            header[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(names("kf"), FRAME_NAMES);
        assert_eq!(names("sf"), ONSET_FLUX_NAMES);
        let pn = names("pn");
        assert_eq!(pn.len(), NUM_FEATURES);
        assert_eq!(
            (pn[0].as_str(), pn[8].as_str(), pn[84].as_str()),
            ("sub_bass", "kick", "beat_locked")
        );

        assert!(
            lines[1].starts_with(
                "{\"k\":\"r\",\"i\":0,\"ts\":0.011610,\"sil\":0,\"kp\":0.25,\"kf\":[0,0.5,1,"
            ),
            "{}",
            lines[1]
        );
        assert!(
            lines[1].contains("],\"sf\":[0.5,0.75,0.125,0],\"pn\":[0.125,"),
            "{}",
            lines[1]
        );
        assert!(
            lines[1].ends_with("],\"act\":0.5,\"bt\":null}"),
            "{}",
            lines[1]
        );
        let rec: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        let pn = rec["pn"].as_array().unwrap();
        assert_eq!(pn.len(), NUM_FEATURES);
        assert!(pn[10].is_null(), "NaN flux must be null, got {}", pn[10]);
        assert_eq!(rec["kf"].as_array().unwrap().len(), N_FRAME);

        assert!(
            lines[2].starts_with("{\"k\":\"r\",\"i\":1,"),
            "{}",
            lines[2]
        );
        assert!(lines[2].ends_with(",\"bt\":1.250000}"), "{}", lines[2]);
    }

    /// A full disk must fail the run, not leave a truncated file behind an exit 0.
    #[cfg(target_os = "linux")]
    #[test]
    #[should_panic(expected = "FOSFORA_HOP_SIDECAR: write to /dev/full failed")]
    fn a_failed_write_is_loud() {
        let frame = [0.0f32; N_FRAME];
        let pre_norm = AudioFeatures::default();
        let mut sc = HopSidecar::new(
            File::create("/dev/full").unwrap(),
            "/dev/full".to_owned(),
            44100.0,
        );
        // Enough records to overflow the BufWriter and reach the device.
        for _ in 0..100 {
            sc.record(&evidence(&frame, &pre_norm, None));
        }
    }
}
