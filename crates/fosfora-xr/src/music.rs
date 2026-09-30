//! The hand menu's Music row (board #3472): the bundled clip started and
//! stopped from the palm while the headset is worn, whatever
//! `debug.fosfora.audio` the app launched with. Plain data, so the row's
//! words and the source each state puts the analysis on build and test on
//! the desktop; `app.rs` owns the playback (`playback.rs`) and the analysis
//! (`audio.rs`).
//!
//! Playing, the clip loops on the speakers and the analysis follows its
//! tap, as `debug.fosfora.audio file` does from launch. Stopped, the
//! analysis goes back to what the app launched with: the synthetic groove,
//! or the microphones, whose stream stays open across a play so a stop is
//! instant. `loop` is the exception: it measures the microphones hearing
//! the clip, so its analysis stays on them either way.

/// The bundled clip the row plays (`assets/xr/audio/`, staged under the
/// app's assets dir as `audio/`).
pub const CLIP: &str = "ember_glow_excerpt.ogg";

/// What the analysis runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Analysis {
    /// The synthetic 120 BPM groove.
    Synth,
    /// The microphones the launch source opened.
    Mic,
    /// The playing clip's tap.
    Tap,
}

impl Analysis {
    /// For the log lines: "on its tap", "back on synth".
    pub fn name(self) -> &'static str {
        match self {
            Self::Synth => "synth",
            Self::Mic => "the microphones",
            Self::Tap => "its tap",
        }
    }
}

/// Whether the music plays at launch: `file` and `loop` start the clip as
/// before, and the `debug.fosfora.music` knob (`knob`) starts it over any
/// other source.
pub fn plays_at_launch(audio_source: &str, knob: bool) -> bool {
    knob || matches!(audio_source, "file" | "loop")
}

/// Whether the launch source keeps microphones open (what a stop goes back
/// to; else the synthetic groove).
fn has_mic(audio_source: &str) -> bool {
    matches!(audio_source, "mic" | "micxr" | "aaudio" | "loop")
}

/// What the analysis runs on with the music `playing` or not, for the
/// app's launch `audio_source`.
pub fn analysis(playing: bool, audio_source: &str) -> Analysis {
    if audio_source == "loop" {
        Analysis::Mic
    } else if playing {
        Analysis::Tap
    } else if has_mic(audio_source) {
        Analysis::Mic
    } else {
        Analysis::Synth
    }
}

/// Whether the clip's tap feeds an analysis of its own (not with `loop`,
/// whose analysis stays on the microphones).
pub fn tap_analyzed(audio_source: &str) -> bool {
    analysis(true, audio_source) == Analysis::Tap
}

/// The row's button: what a press does, the state in the words.
pub fn label(playing: bool) -> &'static str {
    if playing {
        "Music: stop"
    } else {
        "Music: play"
    }
}

/// The log line for the music turning `playing` or not, `clip` its file
/// name.
pub fn log_line(playing: bool, audio_source: &str, clip: &str) -> String {
    let (now, before) = (
        analysis(playing, audio_source),
        analysis(!playing, audio_source),
    );
    let whereto = match (playing, now == before) {
        (_, true) => format!("stays on {}", now.name()),
        (true, false) => format!("on {}", now.name()),
        (false, false) => format!("back on {}", now.name()),
    };
    if playing {
        format!("music: playing {clip} (the analysis {whereto})")
    } else {
        format!("music: stopped (the analysis {whereto})")
    }
}

/// The debug panel header's audio source: the launch source's name, or
/// the clip's tap while the music drives the analysis.
pub fn header(playing: bool, audio_source: &str) -> &str {
    match analysis(playing, audio_source) {
        Analysis::Tap => "music tap",
        Analysis::Synth => "synth",
        Analysis::Mic => audio_source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_says_what_a_press_does() {
        assert_eq!(label(false), "Music: play");
        assert_eq!(label(true), "Music: stop");
    }

    #[test]
    fn file_and_loop_start_playing_and_the_knob_starts_any_source() {
        for source in ["file", "loop"] {
            assert!(plays_at_launch(source, false), "{source}");
        }
        for source in ["synth", "mic", "micxr", "aaudio", "anything"] {
            assert!(!plays_at_launch(source, false), "{source}");
            assert!(plays_at_launch(source, true), "{source}");
        }
    }

    #[test]
    fn a_play_moves_the_analysis_to_the_tap_and_a_stop_back_to_the_launch_source() {
        // Synth and file launch with no microphones: a stop falls back to
        // the synthetic groove (file's own clip was the music).
        for source in ["synth", "file", "unknown"] {
            assert_eq!(analysis(true, source), Analysis::Tap, "{source}");
            assert_eq!(analysis(false, source), Analysis::Synth, "{source}");
            assert!(tap_analyzed(source));
        }
        // The microphones stay open across a play and take over on a stop.
        for source in ["mic", "micxr", "aaudio"] {
            assert_eq!(analysis(true, source), Analysis::Tap, "{source}");
            assert_eq!(analysis(false, source), Analysis::Mic, "{source}");
            assert!(tap_analyzed(source));
        }
        // The acoustic loopback analyzes the microphones hearing the clip.
        assert_eq!(analysis(true, "loop"), Analysis::Mic);
        assert_eq!(analysis(false, "loop"), Analysis::Mic);
        assert!(!tap_analyzed("loop"));
    }

    #[test]
    fn the_log_lines_name_the_clip_and_where_the_analysis_goes() {
        assert_eq!(
            log_line(true, "synth", CLIP),
            "music: playing ember_glow_excerpt.ogg (the analysis on its tap)"
        );
        assert_eq!(
            log_line(false, "synth", CLIP),
            "music: stopped (the analysis back on synth)"
        );
        assert_eq!(
            log_line(false, "aaudio", CLIP),
            "music: stopped (the analysis back on the microphones)"
        );
        assert_eq!(
            log_line(true, "loop", CLIP),
            "music: playing ember_glow_excerpt.ogg (the analysis stays on the microphones)"
        );
    }

    #[test]
    fn the_header_names_what_the_analysis_runs_on() {
        assert_eq!(header(true, "synth"), "music tap");
        assert_eq!(header(false, "synth"), "synth");
        assert_eq!(header(false, "file"), "synth");
        assert_eq!(header(false, "micxr"), "micxr");
        assert_eq!(header(true, "loop"), "loop");
    }
}
