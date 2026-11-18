//! The hand menu's Music row (board #3472): the music started and
//! stopped from the palm while the headset is worn, whatever
//! `debug.fosfora.audio` the app launched with. Plain data, so the row's
//! words and the source each state puts the analysis on build and test on
//! the desktop; `app.rs` owns the playback (`playback.rs`) and the analysis
//! (`audio.rs`). Board #5385: the row plays the wearer's own music
//! folders (`playlist.rs`, `player.rs`). Board #5528: while they are
//! empty it plays the tracks the app ships with ([`BUNDLED`]), as it
//! plays the wearer's. Board #5533: each bundled track names a theme
//! ([`Bundled::theme`]), which the room takes while it plays
//! (`themes::Bringing`, `lanes::RoomLanes::bring_theme`). The wearer's own
//! music brings none.
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
/// order the Music row plays them: the first tracks of the playlist, the
/// wearer's own after them (`playlist::bundled`, `playlist::listed`). A
/// title is the file name's, by the rule for the wearer's files
/// (`playlist::title`). A new track is a new entry here, with the theme it
/// brings, and its file in `assets/xr/audio/`. Nothing else names a track:
/// replacing one is an edit of this list.
pub const BUNDLED: &[Bundled] = &[
    Bundled::new("Let_the_Night_Come_In.ogg", Theme::Aurora),
    Bundled::new("Turn_the_Walls_to_Stone.ogg", Theme::Flint),
    Bundled::new("High_Meadow.ogg", Theme::Contour),
];

/// One track the app ships with: its file's name and the theme the room
/// takes while it plays (board #5533).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bundled {
    pub file: &'static str,
    pub theme: Theme,
}

impl Bundled {
    const fn new(file: &'static str, theme: Theme) -> Self {
        Self { file, theme }
    }
}

/// The bundled tracks' file names, in the list's order
/// (`playlist::bundled` takes them).
pub fn bundled_files() -> Vec<&'static str> {
    BUNDLED.iter().map(|b| b.file).collect()
}

/// The theme the bundled track in the file called `file_name` brings;
/// `None` for any other name.
pub fn bundled_theme(file_name: &str) -> Option<Theme> {
    BUNDLED
        .iter()
        .find(|b| b.file == file_name)
        .map(|b| b.theme)
}

/// The `debug.fosfora.tracktheme` knob (board #5533): 0 and the bundled
/// tracks bring no theme, so a theme under test stays; unset or 1 they do.
pub const TRACK_THEME_KNOB: &str = "debug.fosfora.tracktheme";

use crate::themes::Theme;

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

/// Whether to ask for the storage permission now (board #5549). The app
/// does not ask at launch: the bundled tracks play without it, and the
/// system's dialog speaks of photos and videos. It asks once a run, when
/// the wearer goes for music the list does not hold: a step round an end
/// of the list (`playlist::wraps`) or "scan my music" (`wants_more`),
/// while the shared folder is there and unread (`blocked`) and this run
/// has not asked (`asked`). `debug.fosfora.ask 0` turns the ask off for
/// unworn runs, at the call.
pub fn asks_storage(wants_more: bool, blocked: bool, asked: bool) -> bool {
    wants_more && blocked && !asked
}

/// What the label says before the system's storage dialog comes up
/// (board #5555): why it is coming and where the songs go. The dialog
/// itself speaks of "media files, including audio, photos and videos" and
/// says nothing of either, and it covers the label once it is up, so the
/// label shows first, for its reading time ([`StorageAsk::due`]).
pub const ASK_STORAGE_LABEL: &str =
    "To add your own music,\nallow storage next.\nSongs go in the headset's\nMusic folder.";

/// What the label says when storage was granted and the music folders
/// hold no songs: the step that asked went round the end, and this is why.
pub const NO_OWN_SONGS_LABEL: &str = "No songs in the headset's Music folder yet.";

/// How many polls in a row (one a second) the session has the wearer's
/// focus with the permission still missing before a [`StorageAsk`] counts
/// as unanswered. Two: the system's dialog takes the focus within a
/// fraction of a second of the ask, so one poll can land before it, and a
/// grant shows on the first poll after it closes.
pub const UNANSWERED_POLLS: u32 = 2;

/// The ask for the storage permission, from the step or the "scan my
/// music" that started it to the wearer's answer (board #5555).
///
/// It tells first: [`ASK_STORAGE_LABEL`] shows for its reading time, and
/// only then is the system's dialog asked for ([`Self::due`]), so the
/// wearer knows what the dialog is for before it covers the view.
///
/// A step that started it is owed ([`Self::step`]): the grant makes it
/// once the wearer's songs are in the list, where it lands on their first
/// (or last) one, and a "no" or a dialog that never came makes it round
/// the end of the list ([`Self::unanswered`]). Either way the song changes
/// on the one "next song", and the label names the song it changed to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StorageAsk {
    by: isize,
    tell_s: f32,
    asked: bool,
    focused_polls: u32,
}

impl StorageAsk {
    /// `by`: the step that started it (1 next, -1 previous), 0 for "scan
    /// my music".
    pub fn new(by: isize) -> Self {
        Self {
            by,
            tell_s: crate::label::read_s(ASK_STORAGE_LABEL),
            asked: false,
            focused_polls: 0,
        }
    }

    /// The step owed, `None` for "scan my music".
    pub fn step(self) -> Option<isize> {
        (self.by != 0).then_some(self.by)
    }

    /// Whether the system's dialog was asked for yet.
    pub fn asked(self) -> bool {
        self.asked
    }

    /// Once a frame while it tells: true on the frame the label has had
    /// its reading time, which is when to ask the system. Once.
    pub fn due(&mut self, dt: f32) -> bool {
        if self.asked {
            return false;
        }
        self.tell_s -= dt;
        self.asked = self.tell_s <= 0.0;
        self.asked
    }

    /// One poll with the permission still missing, `focused` while the
    /// session has the wearer's focus (the dialog takes it). True once the
    /// ask went unanswered: [`UNANSWERED_POLLS`] focused polls in a row
    /// after the dialog was asked for.
    pub fn unanswered(&mut self, focused: bool) -> bool {
        self.focused_polls = if focused && self.asked {
            self.focused_polls + 1
        } else {
            0
        };
        self.focused_polls >= UNANSWERED_POLLS
    }
}

/// The label for a step made, in the voice reply's words: "next: High
/// Meadow", "previous: High Meadow".
pub fn step_reply(by: isize, title: &str) -> String {
    format!("{}: {title}", if by < 0 { "previous" } else { "next" })
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
    fn storage_is_asked_once_when_the_wearer_wants_more_than_the_list_holds() {
        // A step round an end, or "scan my music", with the shared folder
        // unread and nothing asked yet.
        assert!(asks_storage(true, true, false));
        // A step inside the list asks nothing.
        assert!(!asks_storage(false, true, false));
        // The folder is read already, or there is none.
        assert!(!asks_storage(true, false, false));
        // Once a run: a wearer who said no is not asked at every wrap.
        assert!(!asks_storage(true, true, true));
        // The label says what the dialog is for and where the songs go,
        // on the lines it is written in, whole.
        assert!(ASK_STORAGE_LABEL.contains("your own music"));
        assert!(ASK_STORAGE_LABEL.contains("Music folder"));
        let shown = crate::label::layout(ASK_STORAGE_LABEL);
        assert_eq!(
            shown.lines,
            ASK_STORAGE_LABEL.lines().collect::<Vec<_>>(),
            "no line wraps"
        );
        // The longest answer after a grant: no songs, and the song the
        // step went round to.
        let none = format!(
            "{NO_OWN_SONGS_LABEL}\n{}",
            step_reply(1, "Let the Night Come In")
        );
        let shown = crate::label::layout(&none);
        assert!(
            shown.lines.len() <= crate::label::MAX_LINES,
            "{:?}",
            shown.lines
        );
        assert!(
            !shown.lines.concat().contains('\u{2026}'),
            "{:?}",
            shown.lines
        );
    }

    #[test]
    fn the_ask_tells_first_and_the_step_that_started_it_is_owed() {
        // Board #5555. The label shows for its reading time before the
        // dialog is asked for, and the ask is made once.
        let mut ask = StorageAsk::new(1);
        assert_eq!((ask.step(), ask.asked()), (Some(1), false));
        let frames = (crate::label::read_s(ASK_STORAGE_LABEL) * 72.0).ceil() as usize;
        assert!(frames > 3 * 72, "{frames}: long enough to read two lines");
        let due: Vec<bool> = (0..frames + 72).map(|_| ask.due(1.0 / 72.0)).collect();
        assert_eq!(due.iter().filter(|d| **d).count(), 1);
        let at = due.iter().position(|d| *d).unwrap();
        assert!(at + 2 >= frames && at <= frames, "{at} of {frames}");
        assert!(ask.asked());
        // The dialog comes up after one focused poll and holds the focus
        // for eight seconds: nothing is decided, and the first poll back
        // sees the grant before this is asked again.
        assert!(!ask.unanswered(true));
        for _ in 0..8 {
            assert!(!ask.unanswered(false));
        }
        // A "no": back in focus with the permission still missing.
        assert!(!ask.unanswered(true));
        assert!(ask.unanswered(true));
        // While it tells, focused polls decide nothing: the dialog has not
        // been asked for.
        let mut ask = StorageAsk::new(-1);
        for _ in 0..5 {
            assert!(!ask.unanswered(true));
        }
        // No dialog at all (the system stopped showing it): two polls.
        assert!(ask.due(60.0));
        assert!(!ask.unanswered(true));
        assert!(ask.unanswered(true));
        // "scan my music" owes no step.
        assert_eq!(StorageAsk::new(0).step(), None);
        // The label is the voice reply's for the same step.
        assert_eq!(step_reply(1, "High Meadow"), "next: High Meadow");
        assert_eq!(step_reply(-1, "High Meadow"), "previous: High Meadow");
    }

    #[test]
    fn the_bundled_tracks_are_files_the_player_takes_and_the_test_clip_is_not_one() {
        use crate::playlist;
        assert!(!BUNDLED.is_empty());
        let files = bundled_files();
        assert!(!files.contains(&CLIP));
        let titles: Vec<String> = files.iter().map(|n| playlist::title(n)).collect();
        for (name, title) in files.iter().zip(&titles) {
            assert!(playlist::is_music(std::path::Path::new(name)), "{name}");
            // A title the row shows and the voice can name, one track each.
            assert!(!crate::intent::normalize(title).is_empty(), "{name}");
            assert_eq!(titles.iter().filter(|t| *t == title).count(), 1, "{name}");
        }
        // Each is in the folder the APK's audio is staged from, so the
        // list and the files cannot drift apart.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/xr/audio");
        let (tracks, missing) = playlist::bundled(&dir, &files);
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

    /// Board #5533: every bundled track brings a theme, found by its
    /// file's name; no other file brings one. Nothing here names a track,
    /// a title or how many there are.
    #[test]
    fn every_bundled_track_brings_a_theme_and_no_other_file_does() {
        for b in BUNDLED {
            assert_eq!(bundled_theme(b.file), Some(b.theme), "{}", b.file);
            assert!(Theme::ALL.contains(&b.theme), "{}", b.file);
        }
        // One entry per file, so a file brings one theme.
        let files = bundled_files();
        for f in &files {
            assert_eq!(files.iter().filter(|g| *g == f).count(), 1, "{f}");
        }
        assert_eq!(bundled_theme(CLIP), None);
        assert_eq!(bundled_theme("a song of the wearer's.ogg"), None);
        assert_eq!(bundled_theme(""), None);
        // The first track is the one the app's look was made for: a fresh
        // room starts on the theme it brings.
        assert_eq!(BUNDLED[0].theme, Theme::Aurora);
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
