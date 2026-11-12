//! Dev-only downbeat-tracker instrumentation (#4499).
//!
//! When the `FOSFORA_DOWNBEAT_SIDECAR` env var names a file, the hop chain appends one
//! JSONL record per beat the downbeat tracker scores (`"k":"b"`): the hop timestamp, the
//! settled beat's event time (what `/downbeat` carries when this beat is the "one"), the
//! grid index, the tempo lock and silence flags, the three accent cues the beat wrote into
//! the PREVIOUS beat's ring entry, the scoring branch it took (and `fb`, whether the
//! challenger was the low-confidence 4/4 fallback), the top candidate, and the lock after
//! it. The cues and the contrast travel as raw f32 bit patterns (`*_b`) beside a
//! readable value, so an offline replay of the tracker can be checked bit for bit.
//!
//! With the variable unset nothing is collected and the signal dump is byte-identical.
//! Never set it in live use: the writer does file I/O on the analysis thread.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};

use super::downbeat::DownbeatDebug;

pub struct DownbeatSidecar {
    out: BufWriter<File>,
    path: String,
}

impl DownbeatSidecar {
    /// Build the writer iff `FOSFORA_DOWNBEAT_SIDECAR` is set; any create/open failure is
    /// loud, as for the other sidecars.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var("FOSFORA_DOWNBEAT_SIDECAR").ok()?;
        let file = super::sidecar_file::create_sidecar("FOSFORA_DOWNBEAT_SIDECAR", &path);
        Some(Self {
            out: BufWriter::new(file),
            path,
        })
    }

    pub fn record(
        &mut self,
        timestamp: f64,
        beat_time: Option<f64>,
        beat_locked: bool,
        loud_silent: bool,
        bpm: f32,
        d: &DownbeatDebug,
    ) {
        let mut line = String::new();
        let bt = beat_time.map_or_else(|| "null".to_owned(), |t| format!("{t}"));
        let _ = write!(
            line,
            "{{\"k\":\"b\",\"ts\":{timestamp},\"bt\":{bt},\"gi\":{},\"lk\":{},\"ls\":{},\
             \"bpm\":{bpm},\"path\":\"{}\",\"fb\":{}",
            d.idx,
            u8::from(beat_locked),
            u8::from(loud_silent),
            d.path,
            u8::from(d.fallback),
        );
        if let Some([fl, cc, lr]) = d.filled {
            let _ = write!(
                line,
                ",\"fl\":{fl},\"cc\":{cc},\"lr\":{lr},\"fl_b\":{},\"cc_b\":{},\"lr_b\":{}",
                fl.to_bits(),
                cc.to_bits(),
                lr.to_bits()
            );
        }
        if let Some((m, p, s)) = d.best {
            let _ = write!(
                line,
                ",\"bm\":{m},\"bp\":{p},\"bs\":{s},\"bs_b\":{}",
                s.to_bits()
            );
        }
        let (cm, cp) = d.candidate.map_or((0, 0), |c| c);
        let _ = writeln!(
            line,
            ",\"m\":{},\"p\":{},\"cm\":{cm},\"cp\":{cp},\"cn\":{},\"db\":{}}}",
            d.meter,
            d.phase,
            d.candidate_count,
            u8::from(d.downbeat),
        );
        // As loud as a failed create: a full disk otherwise truncates the file, the
        // process exits 0, and the bench keeps it as a whole track (#106).
        if let Err(e) = self.out.write_all(line.as_bytes()) {
            panic!(
                "FOSFORA_DOWNBEAT_SIDECAR: write to {} failed: {e}",
                self.path
            );
        }
    }
}

impl Drop for DownbeatSidecar {
    fn drop(&mut self) {
        if let Err(e) = self.out.flush() {
            assert!(
                std::thread::panicking(),
                "FOSFORA_DOWNBEAT_SIDECAR: flush to {} failed: {e}",
                self.path
            );
        }
    }
}
