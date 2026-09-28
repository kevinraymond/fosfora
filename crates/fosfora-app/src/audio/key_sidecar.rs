//! Dev-only key-path instrumentation (#2079).
//!
//! When the `FOSFORA_KEY_SIDECAR` env var names a file, the hop chain appends one JSONL
//! record every [`DECIMATE`]th hop with the key path's raw inputs — the 61 pre-fold
//! semitone energies, `harmonic_ratio` and the perceptual-silence flag — so offline
//! sweeps (`bench/analyze_key.py --sweep`) can replay the key *back-end* (profiles,
//! fold floor, EMA, gates, hysteresis, the emit gate) against the production
//! *front-end*'s exact output, instead of trusting a Python replica of the CQT.
//!
//! Never set the variable in live use: the writer does file I/O on the analysis thread.
//! The bench drives it per track via `--signal-dump`, where the "audio thread" is an
//! offline file loop.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};

use super::chroma::{BassObs, N_SEMITONES};

/// Record every Nth hop: ~21.5 Hz at the 86 Hz hop rate — ample for a 12 s key EMA,
/// and a quarter of the bytes.
const DECIMATE: u32 = 4;

pub struct KeySidecar {
    out: BufWriter<File>,
    path: String,
    hops_seen: u32,
}

impl KeySidecar {
    /// Build the writer iff `FOSFORA_KEY_SIDECAR` is set; any create/open failure is
    /// loud — a sweep silently missing tracks would corrupt the comparison.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var("FOSFORA_KEY_SIDECAR").ok()?;
        let file = File::create(&path)
            .unwrap_or_else(|e| panic!("FOSFORA_KEY_SIDECAR: cannot create {path}: {e}"));
        Some(Self {
            out: BufWriter::new(file),
            path,
            hops_seen: 0,
        })
    }

    pub fn record(
        &mut self,
        timestamp: f64,
        e61: &[f32; N_SEMITONES],
        bass: Option<BassObs>,
        harmonic_ratio: f32,
        loud_silent: bool,
    ) {
        self.hops_seen = self.hops_seen.wrapping_add(1);
        if self.hops_seen % DECIMATE != 1 {
            return;
        }
        let bass_json = match bass {
            Some(b) => format!("{{\"pc\":{},\"mag\":{:.5e}}}", b.pc, b.mag),
            None => "null".to_owned(),
        };
        let mut line = format!(
            "{{\"ts\":{timestamp:.4},\"hr\":{harmonic_ratio:.4},\"silent\":{},\"bass\":{bass_json},\"e61\":[",
            u8::from(loud_silent)
        );
        for (i, v) in e61.iter().enumerate() {
            if i > 0 {
                line.push(',');
            }
            let _ = write!(line, "{v:.5e}");
        }
        line.push_str("]}\n");
        // As loud as a failed create: a full disk otherwise truncates the file, the
        // process exits 0, and the bench keeps it as a whole track (#106).
        if let Err(e) = self.out.write_all(line.as_bytes()) {
            panic!("FOSFORA_KEY_SIDECAR: write to {} failed: {e}", self.path);
        }
    }
}

impl Drop for KeySidecar {
    fn drop(&mut self) {
        if let Err(e) = self.out.flush() {
            // Already unwinding: a second panic would abort, and the first one
            // has made the run fail anyway.
            assert!(
                std::thread::panicking(),
                "FOSFORA_KEY_SIDECAR: flush to {} failed: {e}",
                self.path
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full disk must fail the run, not leave a truncated file behind an exit 0.
    #[cfg(target_os = "linux")]
    #[test]
    #[should_panic(expected = "FOSFORA_KEY_SIDECAR: write to /dev/full failed")]
    fn a_failed_write_is_loud() {
        let mut sc = KeySidecar {
            out: BufWriter::new(File::create("/dev/full").unwrap()),
            path: "/dev/full".to_owned(),
            hops_seen: 0,
        };
        let e61 = [0.0; N_SEMITONES];
        // Enough lines to overflow the BufWriter and reach the device.
        for hop in 0..1000 {
            sc.record(f64::from(hop), &e61, None, 0.5, false);
        }
    }
}
