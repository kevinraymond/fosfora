//! The wearer's own music (board #5385): the files in the two music
//! folders as one playlist that the hand menu's Music row and the voice
//! step through. Board #5528, #5546: the tracks the app ships with come
//! first in it, and the wearer's own follow ([`bundled`], [`listed`]).
//!
//! The folders are the shared `Music/` folder, which needs the storage
//! permission (`permissions::storage`), and the app's own external files
//! folder `Android/data/<package>/files/Music`, which needs none and is
//! reachable over USB. Both are scanned at launch, when the permission
//! arrives, and when the wearer asks.
//!
//! Plain data and a directory walk, so the list, its order, the stepping,
//! the lookup by spoken name and the saved state build and test on the
//! desktop. `player.rs` decodes and plays.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// The file extensions the core's decoder reads (`decode` feature:
/// symphonia with MP3, Vorbis in OGG, FLAC, WAV, and AAC in MP4),
/// compared without case.
pub const EXTENSIONS: [&str; 5] = ["mp3", "ogg", "flac", "wav", "m4a"];
/// How many folders below a music folder a scan descends (an artist and
/// an album folder, with room to spare).
pub const MAX_DEPTH: usize = 4;
/// The most tracks a scan lists, both folders together; the walk stops
/// there.
pub const MAX_TRACKS: usize = 2000;
/// The longest track the player takes (minutes). A decoded track is held
/// whole in memory, about 23 MB a minute as 48 kHz stereo f32, so a
/// longer one is refused and skipped.
pub const MAX_TRACK_MIN: u32 = 15;
/// The densest audio the folders hold in practice: uncompressed 96 kHz
/// 24-bit stereo, in bytes a second. A file larger than the length cap at
/// this rate is longer than the cap whatever its format, so it is refused
/// without decoding it.
const DENSEST_BYTES_PER_S: u64 = 96_000 * 2 * 3;
/// The file the last track and the play state are saved in, under the
/// config dir beside `hand_menu.json`.
pub const SAVED_FILE: &str = "music.json";

/// Which folder a track is in, for the launch line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Folder {
    /// The shared `Music/` folder.
    Shared,
    /// The app's own external files folder.
    App,
    /// The app's assets: a track the app ships with (board #5528).
    Bundled,
}

/// One file of the playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub path: PathBuf,
    /// What the Music row and the voice call it ([`title`]).
    pub title: String,
    pub folder: Folder,
}

/// A track's title from its file name, without reading its tags: the
/// extension dropped, underscores as spaces, a leading track number with
/// its separator dropped ("01 - ", "1. ", "03 "), whitespace collapsed.
/// A name that is only a number keeps it.
pub fn title(file_name: &str) -> String {
    let stem = match file_name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && is_music_ext(ext) => stem,
        _ => file_name,
    };
    let spaced = stem.replace('_', " ");
    let words = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let kept = words(&spaced);
    let without = words(strip_track_number(&spaced));
    if without.is_empty() { kept } else { without }
}

/// `s` without a leading track number: one to three digits followed by
/// `.`, `-` or `)` (spaces around it allowed), or by a space when the
/// number has a leading zero ("01 Song"). "99 Luftballons" keeps its
/// number.
fn strip_track_number(s: &str) -> &str {
    let t = s.trim_start();
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return s;
    }
    let rest = &t[digits..];
    let after = rest.trim_start();
    if let Some(stripped) = after.strip_prefix(['.', '-', ')']) {
        return stripped.trim_start();
    }
    if t.starts_with('0') && rest.starts_with(' ') {
        return after;
    }
    s
}

fn is_music_ext(ext: &str) -> bool {
    EXTENSIONS.iter().any(|e| e.eq_ignore_ascii_case(ext))
}

/// Whether `path` names a file the player takes, by its extension.
pub fn is_music(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_music_ext)
}

/// `a` against `b` as a person sorts names: case folded, a run of digits
/// by its value ("track 2" before "track 10"), the raw text breaking ties.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let take = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut run = String::new();
                    while let Some(c) = it.next_if(char::is_ascii_digit) {
                        run.push(c);
                    }
                    run
                };
                let (n, m) = (take(&mut x), take(&mut y));
                let (n, m) = (n.trim_start_matches('0'), m.trim_start_matches('0'));
                let ord = n.len().cmp(&m.len()).then_with(|| n.cmp(m));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(c), Some(d)) => {
                let ord = c.to_lowercase().cmp(d.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                x.next();
                y.next();
            }
        }
    }
}

/// What a scan found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    /// The shared folder's tracks, then the app folder's, each folder's in
    /// [`natural_cmp`] order by subfolder, then by file name.
    pub tracks: Vec<Track>,
    /// The walk stopped at [`MAX_TRACKS`].
    pub full: bool,
    /// Folders the walk could not read, with why.
    pub unreadable: Vec<(PathBuf, String)>,
}

impl Scan {
    /// How many tracks came from `folder`.
    pub fn count(&self, folder: Folder) -> usize {
        self.tracks.iter().filter(|t| t.folder == folder).count()
    }
}

/// Walk `roots` (the shared folder and the app folder, either absent) for
/// music files, at most [`MAX_DEPTH`] folders down and [`MAX_TRACKS`] in
/// all. Hidden files and folders are skipped, and a link to a folder is
/// not followed (a loop would never end). A root that does not exist
/// lists nothing.
pub fn scan(roots: &[(Folder, &Path)]) -> Scan {
    let mut out = Scan::default();
    for &(folder, root) in roots {
        if !root.is_dir() {
            continue;
        }
        let mut found: Vec<(Vec<String>, String, PathBuf)> = Vec::new();
        let left = MAX_TRACKS.saturating_sub(out.tracks.len());
        walk(root, &mut Vec::new(), &mut found, left, &mut out);
        found.sort_by(|a, b| {
            let dirs =
                a.0.iter()
                    .zip(&b.0)
                    .map(|(x, y)| natural_cmp(x, y))
                    .find(|o| *o != Ordering::Equal)
                    .unwrap_or_else(|| a.0.len().cmp(&b.0.len()));
            dirs.then_with(|| natural_cmp(&a.1, &b.1))
        });
        out.tracks
            .extend(found.into_iter().map(|(_, name, path)| Track {
                title: title(&name),
                path,
                folder,
            }));
    }
    out
}

/// One folder of the walk: its music files into `found` (the subfolders
/// on the way, the file name, the path), its subfolders after.
fn walk(
    dir: &Path,
    rel: &mut Vec<String>,
    found: &mut Vec<(Vec<String>, String, PathBuf)>,
    left: usize,
    out: &mut Scan,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            out.unreadable.push((dir.to_path_buf(), e.to_string()));
            return;
        }
    };
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let file = kind.is_file()
            || (kind.is_symlink() && std::fs::metadata(&path).is_ok_and(|m| m.is_file()));
        if file && is_music(&path) {
            if found.len() >= left {
                out.full = true;
                return;
            }
            found.push((rel.clone(), name, path));
        } else if kind.is_dir() && rel.len() < MAX_DEPTH {
            subdirs.push((name, path));
        }
    }
    for (name, path) in subdirs {
        rel.push(name);
        walk(&path, rel, found, left, out);
        rel.pop();
        if out.full {
            return;
        }
    }
}

/// The tracks the app ships with (board #5528): `names` in `dir`, in
/// `names`' order, and the names with no file there, which the caller
/// logs. The order is the list's, never the folder's.
pub fn bundled(dir: &Path, names: &[&str]) -> (Vec<Track>, Vec<String>) {
    let mut missing = Vec::new();
    let tracks = names
        .iter()
        .filter_map(|name| {
            let path = dir.join(name);
            if path.is_file() {
                Some(Track {
                    title: title(name),
                    path,
                    folder: Folder::Bundled,
                })
            } else {
                missing.push((*name).to_owned());
                None
            }
        })
        .collect();
    (tracks, missing)
}

/// The Music row's tracks for what a scan `found` (board #5528, #5546):
/// the `bundled` ones first, in their order, then the wearer's own. A
/// first launch opens on the first bundled track whatever the folders
/// hold, and the wearer's music is a step away in the same list.
/// `only_bundled` (`debug.fosfora.bundled 1`) is the bundled ones alone.
pub fn listed(found: Vec<Track>, bundled: &[Track], only_bundled: bool) -> Vec<Track> {
    let mut tracks = bundled.to_vec();
    if !only_bundled {
        tracks.extend(found);
    }
    tracks
}

/// Whether the first track of a rescan's playlist takes over from what
/// the Music row is on, at once while the music plays and at the next
/// play otherwise. From the test clip (`on_clip`) it does, unless
/// `debug.fosfora.file` put the row there (`pinned`). From a track it does
/// only when that track is `gone` from the new playlist (board #5546). A
/// rescan that adds the wearer's tracks after the bundled ones, or takes
/// them away, leaves a track that is still there playing.
pub fn takes_over(on_clip: bool, pinned: bool, gone: bool) -> bool {
    if on_clip { !pinned } else { gone }
}

/// What a spoken name found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    One(usize),
    /// Several titles hold the name (their indices); the reply names them.
    Several(Vec<usize>),
    Nothing,
}

/// The track `spoken` names among `titles`, both folded alike by the
/// voice grammar's normalizer (`intent::normalize`: case, punctuation and
/// the fillers dropped, number words as digits). A title that is the name
/// whole wins (the first of equal ones); else the titles holding the
/// name's words in a row, one of them a match and several an ambiguity.
pub fn find<S: AsRef<str>>(titles: &[S], spoken: &str) -> Found {
    let said = crate::intent::normalize(spoken);
    if said.is_empty() {
        return Found::Nothing;
    }
    let folded: Vec<Vec<String>> = titles
        .iter()
        .map(|t| crate::intent::normalize(t.as_ref()))
        .collect();
    if let Some(i) = folded.iter().position(|t| *t == said) {
        return Found::One(i);
    }
    let holding: Vec<usize> = folded
        .iter()
        .enumerate()
        .filter(|(_, t)| t.windows(said.len()).any(|w| w == said.as_slice()))
        .map(|(i, _)| i)
        .collect();
    match holding.as_slice() {
        [] => Found::Nothing,
        [one] => Found::One(*one),
        _ => Found::Several(holding),
    }
}

/// Of `n` tracks, the one `by` steps from `current` (1 next, -1
/// previous), wrapping at both ends; from no current track, the first for
/// a step forward and the last for a step back. `None` when there are
/// none.
pub fn stepped(n: usize, current: Option<usize>, by: isize) -> Option<usize> {
    if n == 0 {
        return None;
    }
    Some(match current {
        Some(i) => (i as isize + by).rem_euclid(n as isize) as usize,
        None if by < 0 => n - 1,
        None => 0,
    })
}

/// The tracks, their titles, and the one the Music row shows.
#[derive(Debug, Clone, Default)]
pub struct Playlist {
    tracks: Vec<Track>,
    titles: Vec<String>,
    current: Option<usize>,
}

impl Playlist {
    /// The tracks, the first one current.
    pub fn new(tracks: Vec<Track>) -> Self {
        let titles = tracks.iter().map(|t| t.title.clone()).collect();
        let current = (!tracks.is_empty()).then_some(0);
        Self {
            tracks,
            titles,
            current,
        }
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// How many of the tracks are the ones the app ships with ([`listed`]
    /// puts them first).
    pub fn bundled_len(&self) -> usize {
        self.tracks
            .iter()
            .filter(|t| t.folder == Folder::Bundled)
            .count()
    }

    /// How many of the tracks are the wearer's own.
    pub fn own_len(&self) -> usize {
        self.tracks.len() - self.bundled_len()
    }

    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// The titles in order, for the voice grammar's vocabulary.
    pub fn titles(&self) -> &[String] {
        &self.titles
    }

    pub fn current(&self) -> Option<usize> {
        self.current
    }

    pub fn track(&self, i: usize) -> Option<&Track> {
        self.tracks.get(i)
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.current.and_then(|i| self.tracks.get(i))
    }

    /// Make track `i` current (ignored past the end).
    pub fn select(&mut self, i: usize) {
        if i < self.tracks.len() {
            self.current = Some(i);
        }
    }

    /// The track `by` steps from the current one ([`stepped`]).
    pub fn peek(&self, by: isize) -> Option<usize> {
        stepped(self.tracks.len(), self.current, by)
    }

    /// Step `by` tracks ([`Self::peek`]) and make that one current.
    pub fn step(&mut self, by: isize) -> Option<usize> {
        let i = self.peek(by)?;
        self.current = Some(i);
        Some(i)
    }

    /// The index of the track at `path`.
    pub fn index_of(&self, path: &Path) -> Option<usize> {
        self.tracks.iter().position(|t| t.path == path)
    }

    /// The track `spoken` names ([`find`]).
    pub fn find(&self, spoken: &str) -> Found {
        find(&self.titles, spoken)
    }

    /// A fresh scan's tracks in place of these, the current track kept by
    /// its path when it is still there, else the first. Returns whether
    /// the row stayed on its track (true as well when it was on none).
    pub fn replace(&mut self, tracks: Vec<Track>) -> bool {
        let was = self.current_track().map(|t| t.path.clone());
        *self = Self::new(tracks);
        let Some(path) = was else {
            return true;
        };
        match self.index_of(&path) {
            Some(i) => {
                self.current = Some(i);
                true
            }
            None => false,
        }
    }
}

/// Why a file is refused before its decode: larger than any track within
/// [`MAX_TRACK_MIN`] can be (`bytes`, its size).
pub fn too_large(bytes: u64) -> Option<String> {
    let most = u64::from(MAX_TRACK_MIN) * 60 * DENSEST_BYTES_PER_S;
    (bytes > most).then(|| {
        format!(
            "{} MB is more than a {MAX_TRACK_MIN}-minute track can be",
            bytes / 1_000_000
        )
    })
}

/// Why a decoded track is refused: longer than [`MAX_TRACK_MIN`] (`secs`,
/// its length).
pub fn too_long(secs: f32) -> Option<String> {
    (secs > MAX_TRACK_MIN as f32 * 60.0)
        .then(|| format!("{} is over the {MAX_TRACK_MIN}-minute cap", clock(secs)))
}

/// A length as `m:ss`.
pub fn clock(secs: f32) -> String {
    let s = secs.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// `text` cut to `max` characters, the last one an ellipsis when cut.
pub fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// The last track and whether it was playing, `music.json` under the
/// config dir. A missing or broken file, or a missing field, is nothing
/// saved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Saved {
    pub track: Option<PathBuf>,
    pub playing: Option<bool>,
}

impl Saved {
    pub fn parse(json: &str) -> Self {
        let value = serde_json::from_str::<serde_json::Value>(json).unwrap_or_default();
        Self {
            track: value
                .get("track")
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from),
            playing: value.get("playing").and_then(serde_json::Value::as_bool),
        }
    }

    /// What to save with the row on `current` (`None` on the test clip,
    /// which keeps the track saved before) and play asked for or not. With
    /// `debug.fosfora.bundled` (`only_bundled`) the track saved before
    /// stays too: the knob's playlist is not the wearer's choice.
    #[must_use]
    pub fn next(&self, current: Option<&Track>, only_bundled: bool, playing: bool) -> Self {
        Self {
            track: current
                .filter(|_| !only_bundled)
                .map(|t| t.path.clone())
                .or_else(|| self.track.clone()),
            playing: Some(playing),
        }
    }

    /// `{"track":"/storage/emulated/0/Music/a.mp3","playing":true}`, the
    /// fields saved so far.
    pub fn to_json(&self) -> String {
        let mut json = serde_json::json!({});
        if let Some(t) = &self.track {
            json["track"] = t.to_string_lossy().into_owned().into();
        }
        if let Some(p) = self.playing {
            json["playing"] = p.into();
        }
        json.to_string()
    }
}

/// "1 track", "14 tracks".
fn tracks_n(n: usize) -> String {
    if n == 1 {
        "1 track".to_owned()
    } else {
        format!("{n} tracks")
    }
}

/// The launch line: how many tracks each folder gave (`scan`), the
/// bundled tracks of the playlist (`list`) by name, whether they come
/// before the wearer's, stand alone because the folders are empty, or
/// because `debug.fosfora.bundled` says so (`only_bundled`), and which
/// track the Music row starts on (whether it was the saved one and
/// whether it plays). With no track at all, that the test clip stands in.
pub fn launch_line(
    scan: &Scan,
    list: &Playlist,
    only_bundled: bool,
    resumed: bool,
    playing: bool,
) -> String {
    let n = scan.tracks.len();
    let found = format!(
        "{} ({} in Music, {} in the app folder){}",
        tracks_n(n),
        scan.count(Folder::Shared),
        scan.count(Folder::App),
        if scan.full {
            format!(", the first {MAX_TRACKS}")
        } else {
            String::new()
        },
    );
    let Some(title) = list.current_track().map(|t| t.title.as_str()) else {
        let none = if only_bundled {
            format!("debug.fosfora.bundled 1 over {found}, and no bundled track")
        } else {
            "no tracks found and no bundled track".to_owned()
        };
        return format!(
            "music: {none}, {} the test clip (put files in Music/)",
            if playing { "playing" } else { "stopped on" }
        );
    };
    let start = match (playing, resumed) {
        (true, true) => format!("resuming \"{title}\""),
        (true, false) => format!("playing \"{title}\""),
        (false, _) => format!("\"{title}\", stopped"),
    };
    if list.bundled_len() == 0 {
        return format!("music: {found} \u{b7} {start}");
    }
    let names = list
        .tracks()
        .iter()
        .filter(|t| t.folder == Folder::Bundled)
        .map(|t| format!("\"{}\"", t.title))
        .collect::<Vec<_>>()
        .join(", ");
    if only_bundled {
        return format!(
            "music: debug.fosfora.bundled 1 over {found} \u{b7} bundled: {names} \u{b7} {start}"
        );
    }
    if list.own_len() == 0 {
        return format!(
            "music: no tracks in the music folders \u{b7} bundled: {names} \u{b7} {start}"
        );
    }
    format!("music: {found} \u{b7} after the bundled: {names} \u{b7} {start}")
}

/// The line for a track that starts: its title, length and decode time.
pub fn start_line(title: &str, secs: f32, decode_s: f32) -> String {
    format!(
        "music: playing \"{title}\" \u{b7} {} \u{b7} decoded in {decode_s:.1} s",
        clock(secs)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(name: &str) -> Track {
        Track {
            path: PathBuf::from(format!("/m/{name}")),
            title: title(name),
            folder: Folder::Shared,
        }
    }

    fn list(names: &[&str]) -> Playlist {
        Playlist::new(names.iter().map(|n| track(n)).collect())
    }

    /// Two tracks as the app ships them.
    fn shipped() -> Vec<Track> {
        ["First_Song.ogg", "Second.ogg"]
            .iter()
            .map(|n| Track {
                path: PathBuf::from(format!("/assets/audio/{n}")),
                title: title(n),
                folder: Folder::Bundled,
            })
            .collect()
    }

    #[test]
    fn a_title_is_the_file_name_without_its_extension_and_track_number() {
        for (name, want) in [
            ("One More Time.mp3", "One More Time"),
            (
                "01 - Daft Punk - One More Time.flac",
                "Daft Punk - One More Time",
            ),
            ("1. Intro.ogg", "Intro"),
            ("03 Harder Better.MP3", "Harder Better"),
            ("07)  Spaced   out.wav", "Spaced out"),
            ("my_song_name.m4a", "my song name"),
            ("99 Luftballons.mp3", "99 Luftballons"),
            ("1999.mp3", "1999"),
            ("01.mp3", "01"),
            ("02 - .mp3", "02 -"),
            ("notes.txt", "notes.txt"),
            ("archive.tar.ogg", "archive.tar"),
            (".hidden.mp3", ".hidden"),
        ] {
            assert_eq!(title(name), want, "{name}");
        }
    }

    #[test]
    fn only_the_decoders_extensions_are_music_in_any_case() {
        for name in ["a.mp3", "a.OGG", "a.Flac", "a.wav", "a.m4a", "dir/b.MP3"] {
            assert!(is_music(Path::new(name)), "{name}");
        }
        for name in [
            "a.txt",
            "a.mp4",
            "a.opus",
            "a.jpg",
            "mp3",
            "a.mp3.part",
            "a",
        ] {
            assert!(!is_music(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn names_sort_as_people_read_them() {
        let mut names = vec![
            "Track 10.mp3",
            "track 2.mp3",
            "Track 1.mp3",
            "b.mp3",
            "A.mp3",
            "track 02.mp3",
            "10.mp3",
            "9.mp3",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(
            names,
            [
                "9.mp3",
                "10.mp3",
                "A.mp3",
                "b.mp3",
                "Track 1.mp3",
                "track 02.mp3",
                "track 2.mp3",
                "Track 10.mp3",
            ]
        );
        assert_eq!(natural_cmp("x", "x"), Ordering::Equal);
        assert_eq!(natural_cmp("a", "ab"), Ordering::Less);
    }

    #[test]
    fn steps_wrap_at_both_ends() {
        let mut p = list(&["a.mp3", "b.mp3", "c.mp3"]);
        assert_eq!(p.current(), Some(0));
        assert_eq!(p.peek(-1), Some(2));
        assert_eq!(p.step(1), Some(1));
        assert_eq!(p.step(1), Some(2));
        assert_eq!(p.step(1), Some(0));
        assert_eq!(p.step(-1), Some(2));
        assert_eq!(p.current_track().map(|t| t.title.as_str()), Some("c"));
        // One track steps onto itself.
        let mut one = list(&["only.ogg"]);
        assert_eq!((one.step(1), one.step(-1)), (Some(0), Some(0)));
        // None steps nowhere.
        let mut none = Playlist::default();
        assert_eq!(
            (none.step(1), none.peek(-1), none.current()),
            (None, None, None)
        );
        none.select(3);
        assert_eq!(none.current(), None);
    }

    #[test]
    fn a_rescan_keeps_the_current_track_by_its_path() {
        let mut p = list(&["a.mp3", "b.mp3", "c.mp3"]);
        p.select(2);
        p.replace(vec![track("0.mp3"), track("a.mp3"), track("c.mp3")]);
        assert_eq!(p.current(), Some(2));
        assert_eq!(p.titles(), ["0", "a", "c"]);
        // Gone: the first.
        p.replace(vec![track("x.mp3"), track("y.mp3")]);
        assert_eq!(p.current(), Some(0));
        p.replace(Vec::new());
        assert_eq!(p.current(), None);
        assert_eq!(p.index_of(Path::new("/m/x.mp3")), None);
    }

    #[test]
    fn a_spoken_name_finds_its_track() {
        let p = list(&[
            "01 - One More Time.mp3",
            "One.flac",
            "Harder, Better, Faster, Stronger.ogg",
            "Don't Stop Me Now.mp3",
            "Stop and Stare.mp3",
            "Twenty One Pilots - Heathens.mp3",
        ]);
        for (spoken, want) in [
            // Whole, folded: case, punctuation, the apostrophe.
            ("one more time", Found::One(0)),
            ("ONE MORE TIME!", Found::One(0)),
            ("harder better faster stronger", Found::One(2)),
            ("dont stop me now", Found::One(3)),
            ("Don't stop me now.", Found::One(3)),
            // A title that is the name whole beats one holding it.
            ("one", Found::One(1)),
            // A unique run of words.
            ("faster", Found::One(2)),
            ("heathens", Found::One(5)),
            // Number words read as the normalizer reads them, both sides.
            ("twenty one pilots", Found::One(5)),
            ("21 pilots", Found::One(5)),
            // Several hold it: an ambiguity naming them.
            ("stop", Found::Several(vec![3, 4])),
            // Nothing holds it, or nothing was said.
            ("stairway", Found::Nothing),
            ("on", Found::Nothing),
            ("", Found::Nothing),
            ("the", Found::Nothing),
        ] {
            assert_eq!(p.find(spoken), want, "\"{spoken}\"");
        }
        // Equal titles in both folders: the first.
        assert_eq!(find(&["Song", "Song"], "song"), Found::One(0));
    }

    #[test]
    fn the_saved_state_round_trips_and_reads_nothing_when_broken() {
        for saved in [
            Saved {
                track: Some(PathBuf::from("/storage/emulated/0/Music/a b.mp3")),
                playing: Some(true),
            },
            Saved {
                track: None,
                playing: Some(false),
            },
            Saved::default(),
        ] {
            assert_eq!(Saved::parse(&saved.to_json()), saved);
        }
        assert_eq!(Saved::default().to_json(), "{}");
        for broken in [
            "",
            "{nope",
            r#"{"track": 3, "playing": "yes"}"#,
            r#"{"track": ""}"#,
        ] {
            assert_eq!(Saved::parse(broken), Saved::default(), "{broken}");
        }
    }

    #[test]
    fn the_caps_refuse_what_cannot_fit_in_memory() {
        // 15 minutes of 96 kHz 24-bit stereo is the most a track can be.
        let most = 15 * 60 * 96_000 * 2 * 3;
        assert_eq!(too_large(most), None);
        assert!(too_large(most + 1).is_some_and(|w| w.contains("15-minute")));
        // A 128 kbps MP3 of 15 minutes is far under it.
        assert_eq!(too_large(15 * 60 * 16_000), None);
        assert_eq!(too_long(15.0 * 60.0), None);
        assert_eq!(
            too_long(18.0 * 60.0 + 20.4).as_deref(),
            Some("18:20 is over the 15-minute cap")
        );
        assert_eq!(clock(222.0), "3:42");
        assert_eq!(clock(59.6), "1:00");
        assert_eq!(clock(-1.0), "0:00");
    }

    #[test]
    fn a_long_title_is_cut_with_an_ellipsis() {
        assert_eq!(truncate("Short", 10), "Short");
        assert_eq!(truncate("Exactly 10", 10), "Exactly 10");
        assert_eq!(truncate("One More Time", 6), "One M\u{2026}");
        // A cut right after a space drops the space.
        assert_eq!(truncate("One More Time", 5), "One\u{2026}");
        assert_eq!(truncate("abc", 0), "\u{2026}");
    }

    #[test]
    fn the_launch_line_counts_each_folder_and_names_the_start() {
        let mut scan = Scan {
            tracks: (0..14)
                .map(|i| Track {
                    folder: if i < 12 { Folder::Shared } else { Folder::App },
                    ..track(&format!("{i}.mp3"))
                })
                .collect(),
            ..Scan::default()
        };
        // Without a bundled track (none in the assets): the folders' alone.
        let own = |scan: &Scan| Playlist::new(listed(scan.tracks.clone(), &[], false));
        // The wearer's after the bundled ones, the launch on the first
        // bundled track, or on the saved one.
        let mut both = Playlist::new(listed(scan.tracks.clone(), &shipped(), false));
        assert_eq!(
            launch_line(&scan, &both, false, false, true),
            "music: 14 tracks (12 in Music, 2 in the app folder) \u{b7} after the bundled: \"First Song\", \"Second\" \u{b7} playing \"First Song\""
        );
        both.select(2);
        assert_eq!(
            launch_line(&scan, &both, false, true, false),
            "music: 14 tracks (12 in Music, 2 in the app folder) \u{b7} after the bundled: \"First Song\", \"Second\" \u{b7} \"0\", stopped"
        );
        assert_eq!(
            launch_line(&scan, &own(&scan), false, true, true),
            "music: 14 tracks (12 in Music, 2 in the app folder) \u{b7} resuming \"0\""
        );
        assert_eq!(
            launch_line(&scan, &own(&scan), false, false, true),
            "music: 14 tracks (12 in Music, 2 in the app folder) \u{b7} playing \"0\""
        );
        assert_eq!(
            launch_line(&scan, &own(&scan), false, true, false),
            "music: 14 tracks (12 in Music, 2 in the app folder) \u{b7} \"0\", stopped"
        );
        // The knob: the bundled tracks by name, over what the folders hold.
        let forced = Playlist::new(listed(scan.tracks.clone(), &shipped(), true));
        assert_eq!(
            launch_line(&scan, &forced, true, false, true),
            "music: debug.fosfora.bundled 1 over 14 tracks (12 in Music, 2 in the app folder) \u{b7} bundled: \"First Song\", \"Second\" \u{b7} playing \"First Song\""
        );
        scan.full = true;
        assert!(
            launch_line(&scan, &own(&scan), false, true, true).contains(", the first 2000 \u{b7}")
        );
        scan.tracks.truncate(1);
        assert!(
            launch_line(&scan, &own(&scan), false, false, true).starts_with("music: 1 track (1 in")
        );
        // Empty folders: the bundled tracks by name, the saved one resumed.
        let empty = Scan::default();
        let mut bundled = Playlist::new(listed(Vec::new(), &shipped(), false));
        assert_eq!(
            launch_line(&empty, &bundled, false, false, true),
            "music: no tracks in the music folders \u{b7} bundled: \"First Song\", \"Second\" \u{b7} playing \"First Song\""
        );
        bundled.select(1);
        assert_eq!(
            launch_line(&empty, &bundled, false, true, false),
            "music: no tracks in the music folders \u{b7} bundled: \"First Song\", \"Second\" \u{b7} \"Second\", stopped"
        );
        // No track at all: the test clip stands in.
        let none = Playlist::default();
        assert_eq!(
            launch_line(&empty, &none, false, false, true),
            "music: no tracks found and no bundled track, playing the test clip (put files in Music/)"
        );
        assert_eq!(
            launch_line(&empty, &none, false, false, false),
            "music: no tracks found and no bundled track, stopped on the test clip (put files in Music/)"
        );
        assert_eq!(
            launch_line(&scan, &none, true, false, true),
            "music: debug.fosfora.bundled 1 over 1 track (1 in Music, 0 in the app folder), the first 2000, and no bundled track, playing the test clip (put files in Music/)"
        );
        assert_eq!(
            start_line("Song", 222.4, 1.84),
            "music: playing \"Song\" \u{b7} 3:42 \u{b7} decoded in 1.8 s"
        );
    }

    #[test]
    fn with_empty_folders_the_playlist_is_the_bundled_tracks_in_the_lists_order() {
        let p = Playlist::new(listed(Vec::new(), &shipped(), false));
        assert_eq!((p.bundled_len(), p.own_len()), (2, 0));
        assert_eq!(p.titles(), ["First Song", "Second"]);
        assert_eq!(p.current(), Some(0));
        // Whatever the list's length: one, or three in an order no sort
        // gives.
        let three: Vec<Track> = ["b.ogg", "c.ogg", "a.ogg"]
            .iter()
            .map(|n| Track {
                folder: Folder::Bundled,
                ..track(n)
            })
            .collect();
        for n in 1..=three.len() {
            let p = Playlist::new(listed(Vec::new(), &three[..n], false));
            assert_eq!(p.titles(), &["b", "c", "a"][..n]);
        }
        // No bundled track either: nothing.
        let none = Playlist::new(listed(Vec::new(), &[], false));
        assert!(none.is_empty() && none.bundled_len() == 0);
    }

    #[test]
    fn the_wearers_tracks_follow_the_bundled_ones_in_one_list() {
        let own = vec![track("mine.mp3"), track("yours.flac")];
        let p = Playlist::new(listed(own.clone(), &shipped(), false));
        assert_eq!(p.titles(), ["First Song", "Second", "mine", "yours"]);
        assert_eq!((p.bundled_len(), p.own_len()), (2, 2));
        // The launch is on the first bundled track whatever the folders
        // hold, and a step back from it is the wearer's last.
        assert_eq!(p.current(), Some(0));
        assert_eq!(p.peek(-1), Some(3));
        // The bundled ones keep their places, the wearer's their order.
        let folders: Vec<Folder> = p.tracks().iter().map(|t| t.folder).collect();
        assert_eq!(
            folders,
            [
                Folder::Bundled,
                Folder::Bundled,
                Folder::Shared,
                Folder::Shared
            ]
        );
        // Every title is there for the voice, bundled or not.
        assert_eq!(p.find("yours"), Found::One(3));
        assert_eq!(p.find("second"), Found::One(1));
        // With no bundled track in the assets the list is the wearer's.
        let p = Playlist::new(listed(own.clone(), &[], false));
        assert_eq!(p.titles(), ["mine", "yours"]);
        assert_eq!((p.bundled_len(), p.own_len()), (0, 2));
        // The knob: the bundled ones alone, whatever the folders hold.
        let p = Playlist::new(listed(own, &shipped(), true));
        assert_eq!(p.titles(), ["First Song", "Second"]);
        assert_eq!(p.own_len(), 0);
    }

    #[test]
    fn the_wearers_tracks_arriving_and_leaving_keep_the_row_on_its_track() {
        // Empty at launch, on the second bundled track.
        let mut p = Playlist::new(listed(Vec::new(), &shipped(), false));
        p.select(1);
        // The wearer's tracks arrive (the permission, "scan my music"):
        // they follow the bundled ones and the row stays where it is.
        assert!(p.replace(listed(
            vec![track("a.mp3"), track("b.mp3")],
            &shipped(),
            false,
        )));
        assert_eq!(
            (p.titles(), p.current()),
            (
                &["First Song", "Second", "a", "b"].map(String::from)[..],
                Some(1)
            )
        );
        // A rescan among their tracks keeps the current one by its path.
        p.select(3);
        assert!(p.replace(listed(
            vec![track("0.mp3"), track("a.mp3"), track("b.mp3")],
            &shipped(),
            false,
        )));
        assert_eq!(p.current_track().map(|t| t.title.as_str()), Some("b"));
        assert_eq!(p.current(), Some(4));
        // The folders empty while the row is on one of theirs: that track
        // is gone, and the row is on the first bundled one.
        assert!(!p.replace(listed(Vec::new(), &shipped(), false)));
        assert_eq!((p.bundled_len(), p.own_len()), (2, 0));
        assert_eq!(
            p.current_track().map(|t| t.title.as_str()),
            Some("First Song")
        );
        // The folders empty while the row is on a bundled track: it stays.
        p.replace(listed(vec![track("a.mp3")], &shipped(), false));
        p.select(1);
        assert!(p.replace(listed(Vec::new(), &shipped(), false)));
        assert_eq!(p.current(), Some(1));
        // A list with no current track has none to lose.
        assert!(Playlist::default().replace(shipped()));
    }

    #[test]
    fn a_rescan_takes_over_only_from_the_clip_or_when_the_rows_track_is_gone() {
        // On a track that is still in the list: it plays on, whatever the
        // scan added or took away around it.
        assert!(!takes_over(false, false, false));
        // The row's track is gone from the list.
        assert!(takes_over(false, false, true));
        assert!(takes_over(false, true, true));
        // On the test clip: the first track takes over, unless
        // debug.fosfora.file put the row there.
        assert!(takes_over(true, false, false));
        assert!(takes_over(true, false, true));
        assert!(!takes_over(true, true, false));
        assert!(!takes_over(true, true, true));
    }

    #[test]
    fn a_bundled_track_is_saved_as_the_wearers_are_but_not_under_the_knob() {
        let before = Saved {
            track: Some(PathBuf::from("/m/mine.mp3")),
            playing: Some(false),
        };
        let bundled = shipped();
        // With empty folders the bundled track the row is on is saved,
        // and a launch finds it again by its path.
        let saved = before.next(Some(&bundled[1]), false, true);
        assert_eq!(saved.track.as_deref(), Some(bundled[1].path.as_path()));
        assert_eq!(saved.playing, Some(true));
        let again = Playlist::new(listed(Vec::new(), &bundled, false));
        assert_eq!(again.index_of(saved.track.as_deref().unwrap()), Some(1));
        // The wearer's own saved track is found after the bundled ones.
        let both = Playlist::new(listed(vec![track("mine.mp3")], &bundled, false));
        assert_eq!(both.index_of(before.track.as_deref().unwrap()), Some(2));
        // The knob keeps the wearer's saved track; the play or stop is
        // still theirs.
        let knob = before.next(Some(&bundled[1]), true, true);
        assert_eq!(knob.track, before.track);
        assert_eq!(knob.playing, Some(true));
        // On the test clip the track saved before stays.
        assert_eq!(before.next(None, false, false), before);
    }

    /// A folder tree under the temp dir, removed on drop.
    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str, files: &[&str]) -> Self {
            let root =
                std::env::temp_dir().join(format!("fosfora-playlist-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            for f in files {
                let path = root.join(f);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, b"x").unwrap();
            }
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_scan_lists_both_folders_folder_then_name() {
        let shared = Tree::new(
            "shared",
            &[
                "z top.mp3",
                "Album 10/02 b.mp3",
                "Album 10/01 a.mp3",
                "album 9/x.flac",
                "album 9/cover.jpg",
                "album 9/Disc 1/1.ogg",
                ".trash/gone.mp3",
                "a/b/c/d/deep.mp3",
                "a/b/c/d/e/too deep.mp3",
                "notes.txt",
            ],
        );
        let app = Tree::new("app", &["mine.wav", "Zed.M4A"]);
        let missing = std::env::temp_dir().join("fosfora-playlist-none");
        let scan = scan(&[
            (Folder::Shared, &shared.0),
            (Folder::App, &app.0),
            (Folder::App, &missing),
        ]);
        let names: Vec<&str> = scan.tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(
            names,
            [
                // The root's files before any subfolder's, then the
                // subfolders in natural order, depth first.
                "z top", "deep", "x", "1", "a", "b", // shared
                "mine", "Zed", // app
            ]
        );
        assert_eq!(
            (scan.count(Folder::Shared), scan.count(Folder::App)),
            (6, 2)
        );
        assert!(!scan.full && scan.unreadable.is_empty());
        assert!(scan.tracks[2].path.ends_with("album 9/x.flac"));
    }

    #[test]
    fn a_missing_bundled_file_is_left_out_and_named() {
        let dir = Tree::new(
            "bundled",
            &["Slow_Burn.ogg", "01 - Opener.ogg", "other.ogg"],
        );
        let names = ["Slow_Burn.ogg", "Gone.ogg", "01 - Opener.ogg"];
        let (tracks, missing) = bundled(&dir.0, &names);
        // The list's order, not the folder's, and only the list's files.
        let titles: Vec<&str> = tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Slow Burn", "Opener"]);
        assert!(tracks.iter().all(|t| t.folder == Folder::Bundled));
        assert_eq!(tracks[0].path, dir.0.join("Slow_Burn.ogg"));
        assert_eq!(missing, ["Gone.ogg"]);
        // None of them there: no track, every name missing, no panic.
        let nowhere = std::env::temp_dir().join("fosfora-playlist-no-assets");
        let (tracks, missing) = bundled(&nowhere, &names);
        assert!(tracks.is_empty());
        assert_eq!(missing, names);
        assert_eq!(bundled(&dir.0, &[]), (Vec::new(), Vec::new()));
    }

    #[test]
    fn a_scan_stops_at_the_track_cap() {
        let names: Vec<String> = (0..MAX_TRACKS + 5).map(|i| format!("{i}.mp3")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let tree = Tree::new("cap", &refs);
        let scan = scan(&[(Folder::Shared, &tree.0), (Folder::App, &tree.0)]);
        assert_eq!(scan.tracks.len(), MAX_TRACKS);
        assert!(scan.full);
    }
}
