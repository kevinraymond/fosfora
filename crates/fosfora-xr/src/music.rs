//! The hand menu's Music row (board #3472): the music started and
//! stopped from the palm while the headset is worn, whatever
//! `debug.fosfora.audio` the app launched with. Plain data, so the row's
//! words and the source each state puts the analysis on build and test on
//! the desktop; `app.rs` owns the playback (`playback.rs`) and the analysis
//! (`audio.rs`). Board #5385: the row plays the wearer's own music
//! folders (`playlist.rs`, `player.rs`). Board #5528: while they are
//! empty it plays the tracks the app ships with ([`BUNDLED`]), as it
//! plays the wearer's.
//!
//! Playing, the music is on the speakers (the tracks play through; one
//! track, or the test clip, loops) and the analysis follows its
//! tap, as `debug.fosfora.audio file` does from launch. Stopped, the
//! analysis goes back to what the app launched with: the synthetic groove,
//! or the microphones, whose stream stays open across a play so a stop is
//! instant. `loop` is the exception: it measures the microphones hearing
//! the clip, so its analysis stays on them either way.

/// The test clip (`assets/xr/audio/`, staged under the app's assets dir
/// as `audio/`): what the measurement sources `debug.fosfora.audio file`
/// and `loop` play, a known 140 BPM. It is never in the Music row's
/// playlist; the row falls back to it only when no track plays at all.
pub const CLIP: &str = "ember_glow_excerpt.ogg";

/// The tracks the app ships with (board #5528), beside [`CLIP`], in the
/// order the Music row plays them: the playlist while the wearer's music
/// folders hold no track (`playlist::bundled`, `playlist::listed`). A
/// title is the file name's, by the rule for the wearer's files
/// (`playlist::title`). A new track is a new name here and its file in
/// `assets/xr/audio/`.
pub const BUNDLED: &[&str] = &["Let_the_Night_Come_In.ogg", "Slow_Blue.ogg"];

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

/// Whether the music is on the speakers at launch. The `debug.fosfora.music`
/// knob (`knob`, `None` unset) decides when it is set: on over any source,
/// off over every one, `file` and `loop` included. Unset, the music plays
/// unless the launch source is a microphone (`mic`, `micxr`, `aaudio`):
/// `file` and `loop` are the test clip, and `synth`, the default, is no
/// real source; before board #4191 a first launch on it heard nothing and
/// saw a synthetic beat until the wearer found the Music row. A microphone
/// is chosen on purpose, to measure what it hears, and the music on the
/// speakers would be in it. Board #5385: over the other sources the
/// wearer's last play or stop decides (`saved`, from `music.json`; `None`
/// before the first press), so a launch resumes the music, or stays quiet
/// after a stop.
pub fn plays_at_launch(audio_source: &str, knob: Option<bool>, saved: Option<bool>) -> bool {
    let mic = matches!(audio_source, "mic" | "micxr" | "aaudio");
    knob.unwrap_or_else(|| !mic && saved.unwrap_or(true))
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

/// What Music `<` `>` and a spoken "next song" say with no song to step
/// to (board #5461): where songs go, or, with the shared `Music/` folder
/// unread for want of the storage permission (`blocked`), that permission.
/// The press did nothing visible before, and the voice's reply read like a
/// path ("no songs: put files in Music/"). Board #5528: with the bundled
/// tracks there is a song to step to, so these words show only when none
/// of those plays either.
pub fn no_songs(blocked: bool) -> &'static str {
    if blocked {
        NO_MUSIC_ACCESS_LABEL
    } else {
        NO_SONGS_LABEL
    }
}

/// [`no_songs`]' words: no songs anywhere it can read, and the shared
/// folder unread.
pub const NO_SONGS_LABEL: &str = "No songs on the headset. Put them in its Music folder.";
pub const NO_MUSIC_ACCESS_LABEL: &str =
    "Can't read the Music folder. Allow music files in Settings.";

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
    fn the_bundled_tracks_are_files_the_player_takes_and_the_test_clip_is_not_one() {
        use crate::playlist;
        assert!(!BUNDLED.is_empty());
        assert!(!BUNDLED.contains(&CLIP));
        let titles: Vec<String> = BUNDLED.iter().map(|n| playlist::title(n)).collect();
        for (name, title) in BUNDLED.iter().zip(&titles) {
            assert!(playlist::is_music(std::path::Path::new(name)), "{name}");
            // A title the row shows and the voice can name, one track each.
            assert!(!crate::intent::normalize(title).is_empty(), "{name}");
            assert_eq!(titles.iter().filter(|t| *t == title).count(), 1, "{name}");
        }
        // Each is in the folder the APK's audio is staged from, so the
        // list and the files cannot drift apart.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/xr/audio");
        let (tracks, missing) = playlist::bundled(&dir, BUNDLED);
        assert_eq!(missing, Vec::<String>::new());
        assert_eq!(tracks.len(), BUNDLED.len());
        assert!(dir.join(CLIP).is_file());
        // The first track is the first of the playlist with empty folders.
        let list = playlist::Playlist::new(playlist::listed(Vec::new(), &tracks, false));
        assert_eq!(
            list.current_track().map(|t| t.title.as_str()),
            Some(titles[0].as_str())
        );
    }

    #[test]
    fn the_row_says_what_a_press_does() {
        assert_eq!(label(false), "Music: play");
        assert_eq!(label(true), "Music: stop");
    }

    #[test]
    fn unset_the_clip_plays_at_launch_unless_a_microphone_was_chosen() {
        // The clip sources, and synth (the default) or an unknown name,
        // which run the synthetic groove: no real source to keep quiet for.
        for source in ["file", "loop", "synth", "anything"] {
            assert!(plays_at_launch(source, None, None), "{source}");
        }
        // A microphone is a measurement setup: the clip would be in it.
        for source in ["mic", "micxr", "aaudio"] {
            assert!(!plays_at_launch(source, None, None), "{source}");
        }
    }

    #[test]
    fn the_knob_set_decides_for_every_source() {
        for source in [
            "synth", "file", "loop", "mic", "micxr", "aaudio", "anything",
        ] {
            for saved in [None, Some(true), Some(false)] {
                assert!(plays_at_launch(source, Some(true), saved), "{source}");
                assert!(!plays_at_launch(source, Some(false), saved), "{source}");
            }
        }
    }

    #[test]
    fn unset_the_wearers_last_press_decides_but_not_over_a_microphone() {
        for source in ["synth", "file", "loop"] {
            assert!(plays_at_launch(source, None, Some(true)), "{source}");
            assert!(!plays_at_launch(source, None, Some(false)), "{source}");
        }
        for source in ["mic", "micxr", "aaudio"] {
            assert!(!plays_at_launch(source, None, Some(true)), "{source}");
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
