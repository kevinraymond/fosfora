//! Dev-only beat-path instrumentation (#4153).
//!
//! When the `FOSFORA_BEAT_SIDECAR` env var names a file, the hop chain appends one
//! JSONL record per tempo update (`"k":"u"`: the autocorrelation winner, its comb
//! evidence and harmonics, the folded measurement, the filter, lock, anchor and
//! scheduler state) and one per state transition (`"k":"e"`: seed, lock, anchor and
//! grid events), so an offline pass can see WHY the grid went where it went instead
//! of inferring it from the beats alone. #4234 adds the phase arbiter: one `"k":"w"`
//! record per event pushed into its window (`src` kick / act, `t` its time) and one
//! `"k":"a"` per vote at a grid instant (both windows' on / half / total counts, the
//! quarter bins since #4402, the vote, the streak after it, and whether and how far the
//! grid moved), so a replay of the arbiter can be checked against what the scheduler
//! counted.
//!
//! With the variable unset nothing is collected and the signal dump is byte-identical.
//! Never set it in live use: the writer does file I/O on the analysis thread. The
//! bench drives it per track via `--signal-dump`, where the "audio thread" is an
//! offline file loop.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};

use super::beat::BeatDebug;

pub struct BeatSidecar {
    out: BufWriter<File>,
    path: String,
}

/// A JSON number, or `null` for anything JSON cannot carry.
fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.5}")
    } else {
        "null".to_owned()
    }
}

fn opt(v: Option<f64>) -> String {
    v.map_or_else(|| "null".to_owned(), num)
}

impl BeatSidecar {
    /// Build the writer iff `FOSFORA_BEAT_SIDECAR` is set; any create/open failure is
    /// loud — a sweep silently missing tracks would corrupt the comparison.
    pub fn from_env() -> Option<Self> {
        let path = std::env::var("FOSFORA_BEAT_SIDECAR").ok()?;
        let file = super::sidecar_file::create_sidecar("FOSFORA_BEAT_SIDECAR", &path);
        Some(Self {
            out: BufWriter::new(file),
            path,
        })
    }

    pub fn record(&mut self, timestamp: f64, dbg: BeatDebug) {
        let mut line = String::new();
        if dbg.ran {
            let t = &dbg.tempo;
            let _ = writeln!(
                line,
                "{{\"k\":\"u\",\"ts\":{timestamp:.4},\"raw_bpm\":{},\"conf\":{},\"comb\":{},\
                 \"peak\":{},\"floor\":{},\"c2\":{},\"c3\":{},\"c4\":{},\"meas_bpm\":{},\
                 \"filt_bpm\":{},\"support\":{},\"locked\":{},\"seeded\":{},\"anchor_bpm\":{},\
                 \"challenge\":{},\"pub_bpm\":{},\"mode\":\"{}\",\"on_sup\":{},\"kick_sup\":{},\
                 \"grid_n\":{}}}",
                num(t.raw_bpm),
                num(t.conf),
                num(t.comb),
                num(t.peak),
                num(t.floor),
                opt(t.c2),
                opt(t.c3),
                opt(t.c4),
                num(t.meas_bpm),
                num(t.filt_bpm),
                num(t.support),
                u8::from(t.locked),
                u8::from(t.seeded),
                opt(t.anchor_bpm),
                num(t.challenge),
                num(t.pub_bpm),
                dbg.mode,
                num(dbg.on_sup),
                num(dbg.kick_sup),
                dbg.grid_n,
            );
        }
        for (src, t) in &dbg.arb_events {
            let _ = writeln!(
                line,
                "{{\"k\":\"w\",\"ts\":{timestamp:.4},\"src\":\"{src}\",\"t\":{t:.9}}}"
            );
        }
        for v in &dbg.arb_votes {
            let _ = writeln!(
                line,
                "{{\"k\":\"a\",\"ts\":{timestamp:.4},\"t\":{:.9},\"p\":{:.9},\"on\":{},\
                 \"half\":{},\"n\":{},\"lon\":{},\"lhalf\":{},\"ln\":{},\"q1\":{},\"q3\":{},\
                 \"lq1\":{},\"lq3\":{},\"vote\":{},\"streak\":{},\"shift\":{},\"sk\":{}}}",
                v.now,
                v.period,
                v.short.0,
                v.short.1,
                v.short.2,
                v.long.0,
                v.long.1,
                v.long.2,
                v.quarter.0,
                v.quarter.1,
                v.lquarter.0,
                v.lquarter.1,
                u8::from(v.vote),
                v.streak,
                u8::from(v.shift),
                v.offset,
            );
        }
        for (ev, extra) in &dbg.events {
            let _ = write!(line, "{{\"k\":\"e\",\"ts\":{timestamp:.4},\"ev\":\"{ev}\"");
            if extra.is_finite() {
                let _ = write!(line, ",\"str\":{extra:.5}");
            }
            line.push_str("}\n");
        }
        if line.is_empty() {
            return;
        }
        // As loud as a failed create: a full disk otherwise truncates the file, the
        // process exits 0, and the bench keeps it as a whole track (#106).
        if let Err(e) = self.out.write_all(line.as_bytes()) {
            panic!("FOSFORA_BEAT_SIDECAR: write to {} failed: {e}", self.path);
        }
    }
}

impl Drop for BeatSidecar {
    fn drop(&mut self) {
        if let Err(e) = self.out.flush() {
            // Already unwinding: a second panic would abort, and the first one
            // has made the run fail anyway.
            assert!(
                std::thread::panicking(),
                "FOSFORA_BEAT_SIDECAR: flush to {} failed: {e}",
                self.path
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::beat::{ArbVote, TempoDebug};

    fn sample(ran: bool, events: Vec<(&'static str, f64)>) -> BeatDebug {
        BeatDebug {
            ran,
            tempo: TempoDebug {
                raw_bpm: 128.0,
                c2: Some(0.5),
                anchor_bpm: None,
                ..TempoDebug::default()
            },
            mode: "locked",
            on_sup: 0.75,
            kick_sup: 0.5,
            grid_n: 12,
            events,
            arb_events: vec![],
            arb_votes: vec![],
        }
    }

    /// One "u" line per ACF run, one "e" line per event, and nothing for a hop
    /// with neither; non-finite extras carry no "str".
    #[test]
    fn lines_match_the_hop() {
        let path = std::env::temp_dir().join(format!("beat_sidecar_{}.jsonl", std::process::id()));
        {
            let mut sc = BeatSidecar {
                out: BufWriter::new(File::create(&path).unwrap()),
                path: path.display().to_string(),
            };
            sc.record(0.0, sample(false, vec![]));
            sc.record(
                1.0,
                sample(true, vec![("lock", f64::NAN), ("acq_fire", 0.25)]),
            );
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].starts_with("{\"k\":\"u\",\"ts\":1.0000,\"raw_bpm\":128.00000,"));
        assert!(lines[0].contains("\"c2\":0.50000,\"c3\":null,"));
        assert!(lines[0].contains("\"anchor_bpm\":null,"));
        assert!(lines[0].ends_with(
            "\"mode\":\"locked\",\"on_sup\":0.75000,\"kick_sup\":0.50000,\"grid_n\":12}"
        ));
        assert_eq!(lines[1], "{\"k\":\"e\",\"ts\":1.0000,\"ev\":\"lock\"}");
        assert_eq!(
            lines[2],
            "{\"k\":\"e\",\"ts\":1.0000,\"ev\":\"acq_fire\",\"str\":0.25000}"
        );
    }

    /// #4234: one "w" line per window push and one "a" line per vote, before the events.
    #[test]
    fn arbiter_lines_carry_both_windows() {
        let path =
            std::env::temp_dir().join(format!("beat_sidecar_arb_{}.jsonl", std::process::id()));
        {
            let mut sc = BeatSidecar {
                out: BufWriter::new(File::create(&path).unwrap()),
                path: path.display().to_string(),
            };
            let mut d = sample(false, vec![("arb_shift", f64::NAN)]);
            d.arb_events = vec![("kick", 2.5), ("act", 2.465)];
            d.arb_votes = vec![ArbVote {
                now: 3.0,
                period: 0.5,
                short: (1, 5, 7),
                long: (3, 14, 20),
                quarter: (0, 1),
                lquarter: (2, 1),
                vote: true,
                streak: 0,
                shift: true,
                offset: 0.5,
            }];
            sc.record(3.0, d);
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            [
                "{\"k\":\"w\",\"ts\":3.0000,\"src\":\"kick\",\"t\":2.500000000}",
                "{\"k\":\"w\",\"ts\":3.0000,\"src\":\"act\",\"t\":2.465000000}",
                "{\"k\":\"a\",\"ts\":3.0000,\"t\":3.000000000,\"p\":0.500000000,\"on\":1,\"half\":5,\
                 \"n\":7,\"lon\":3,\"lhalf\":14,\"ln\":20,\"q1\":0,\"q3\":1,\"lq1\":2,\"lq3\":1,\
                 \"vote\":1,\"streak\":0,\"shift\":1,\"sk\":0.5}",
                "{\"k\":\"e\",\"ts\":3.0000,\"ev\":\"arb_shift\"}",
            ],
            "{text}"
        );
    }

    /// A full disk must fail the run, not leave a truncated file behind an exit 0.
    #[cfg(target_os = "linux")]
    #[test]
    #[should_panic(expected = "FOSFORA_BEAT_SIDECAR: write to /dev/full failed")]
    fn a_failed_write_is_loud() {
        let mut sc = BeatSidecar {
            out: BufWriter::new(File::create("/dev/full").unwrap()),
            path: "/dev/full".to_owned(),
        };
        // Enough lines to overflow the BufWriter and reach the device.
        for hop in 0..1000 {
            sc.record(f64::from(hop), sample(true, vec![]));
        }
    }
}
