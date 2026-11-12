//! The Music row's player (board #3472, #5385): the wearer's tracks from
//! the two music folders (`playlist.rs`), or the bundled clip while they
//! are empty, played through one output stream (`playback.rs`) with the
//! analysis on its tap (`music.rs` has the row's words and where the
//! analysis goes).
//!
//! **Off the frame loop.** A track is decoded whole (`Clip::decode`, about
//! 23 MB a minute as 48 kHz stereo f32) on the music thread, which also
//! walks the folders and frees the clips the frame loop lets go of. The
//! frame loop only polls it. A step the wearer asks for decodes the new
//! track while the old one plays on, and the playback swaps it in when it
//! is ready; the old clip is freed once swapped. A track the caps refuse
//! (`playlist::too_large`, `too_long`), or one that does not decode, is
//! logged and skipped, in the direction the wearer was going.
//!
//! **Play through.** With two tracks or more, the next one decodes
//! [`PRELOAD_S`] before the playing one ends and the callback swaps it in
//! where the last one ends. One track, or the bundled clip, loops.
//!
//! **Remembered.** The current track and the last play or stop are saved
//! in `music.json` beside `hand_menu.json` (`playlist::Saved`), so a launch
//! resumes the wearer's music. `debug.fosfora.file`, when set, is played
//! in place of the bundled clip and of the saved track until the wearer
//! steps to a track.
//!
//! Streaming decode is a later step: [`decode_track`] is the one place a track
//! becomes a [`Clip`], and the playback takes any `Arc<Clip>`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use anyhow::Result;
use log::{error, info, warn};

use crate::audio::LiveAudio;
use crate::playback::{Clip, Playback, PlaybackOptions};
use crate::playlist::{self, Folder, Playlist, Saved, Scan};

/// How long before the playing track ends the next one starts decoding
/// (s): a 15-minute track decodes in well under that on the desktop; the
/// device's figure goes in MEASURED.md.
const PRELOAD_S: f32 = 30.0;

/// A clip from `path`: `click` or `click:<bpm>` a 30 s click track, a
/// `.f32` file raw 48 kHz stereo f32 (the measurement knobs'), anything
/// else the core's decoder.
pub fn load_clip(path: &Path) -> Result<Clip> {
    if let Some(bpm) = path
        .to_str()
        .and_then(|p| p.strip_prefix("click"))
        .map(|rest| rest.trim_start_matches(':').parse::<f32>().unwrap_or(120.0))
    {
        Ok(Clip::click(bpm, 30.0, 48_000))
    } else if path.extension().is_some_and(|e| e == "f32") {
        Clip::from_raw_f32_stereo(path, 48_000)
    } else {
        Clip::decode(path)
    }
}

/// A track for the player: refused before the decode when it is larger
/// than a track within the length cap can be, and after it when it is
/// longer than the cap. A decoder that panics is a refusal too, so the
/// music thread lives on.
///
/// The length is known only once the whole file is decoded: the core's
/// decoder has no probe yet, so a long file small enough to pass the size
/// check (a two-hour MP3) is decoded in full before it is refused.
fn decode_track(path: &Path) -> Result<Clip, String> {
    if let Some(why) = std::fs::metadata(path)
        .ok()
        .and_then(|m| playlist::too_large(m.len()))
    {
        return Err(why);
    }
    let clip = std::panic::catch_unwind(|| load_clip(path))
        .map_err(|_| "the decoder panicked".to_owned())?
        .map_err(|e| format!("{e:#}"))?;
    let secs = clip_secs(&clip);
    match playlist::too_long(secs) {
        Some(why) => Err(why),
        None => Ok(clip),
    }
}

fn clip_secs(clip: &Clip) -> f32 {
    clip.samples.len() as f32 / 2.0 / clip.sample_rate.max(1) as f32
}

/// The title the row shows for `path`.
fn title_of(path: &Path) -> String {
    playlist::title(&path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    ))
}

/// Work for the music thread.
enum Job {
    /// Decode a track, unless a newer decode was asked for meanwhile.
    Decode { id: u64, path: PathBuf },
    /// Walk the music folders.
    Scan(Vec<(Folder, PathBuf)>),
    /// Free a clip off the frame loop.
    Free(Arc<Clip>),
}

/// What the music thread did.
enum Done {
    Decoded {
        id: u64,
        path: PathBuf,
        clip: Result<Clip, String>,
        took: Duration,
    },
    Scanned(Scan),
}

/// The music thread: its job queue, its results and the newest decode
/// asked for (an older one not started yet is skipped).
struct Worker {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    latest: Arc<AtomicU64>,
}

impl Worker {
    fn start() -> std::io::Result<Self> {
        let (jobs, inbox) = std::sync::mpsc::channel::<Job>();
        let (outbox, done) = std::sync::mpsc::channel::<Done>();
        let latest = Arc::new(AtomicU64::new(0));
        let newest = latest.clone();
        std::thread::Builder::new()
            .name("fosfora-music".to_owned())
            .spawn(move || {
                for job in inbox {
                    let result = match job {
                        Job::Decode { id, .. } if id < newest.load(Ordering::Relaxed) => continue,
                        Job::Decode { id, path } => {
                            let started = Instant::now();
                            let clip = decode_track(&path);
                            Done::Decoded {
                                id,
                                path,
                                clip,
                                took: started.elapsed(),
                            }
                        }
                        Job::Scan(roots) => {
                            let roots: Vec<(Folder, &Path)> =
                                roots.iter().map(|(f, p)| (*f, p.as_path())).collect();
                            Done::Scanned(playlist::scan(&roots))
                        }
                        Job::Free(clip) => {
                            drop(clip);
                            continue;
                        }
                    };
                    if outbox.send(result).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self { jobs, done, latest })
    }
}

/// The two music folders: the shared one (read only with the storage
/// permission) and the app's own.
#[derive(Debug, Clone, Default)]
pub struct Folders {
    pub shared: Option<PathBuf>,
    pub own: Option<PathBuf>,
    /// The storage permission is granted: the shared folder is scanned.
    pub shared_readable: bool,
}

impl Folders {
    fn roots(&self) -> Vec<(Folder, PathBuf)> {
        let shared = self
            .shared
            .clone()
            .filter(|_| self.shared_readable)
            .map(|p| (Folder::Shared, p));
        let own = self.own.clone().map(|p| (Folder::App, p));
        shared.into_iter().chain(own).collect()
    }
}

/// A decoded clip and where it came from.
struct Held {
    path: PathBuf,
    clip: Arc<Clip>,
    /// How long its decode took.
    took: Duration,
}

/// The decode the player waits for: its job, the track, and whether it
/// swaps in at once (a step the wearer asked for) or where the playing
/// one ends (the next track of a playlist playing through).
struct Decoding {
    id: u64,
    path: PathBuf,
    now: bool,
}

pub struct Music {
    /// The launch `debug.fosfora.audio` (where a stop puts the analysis).
    source: String,
    options: PlaybackOptions,
    folders: Folders,
    playlist: Playlist,
    /// What plays while the folders are empty, looped: the bundled clip,
    /// or `debug.fosfora.file`.
    fallback: PathBuf,
    fallback_title: String,
    /// `debug.fosfora.file` is the fallback: it plays at launch over the
    /// folders, until the wearer steps to a track.
    pinned: bool,
    /// The row is on the fallback, not on a track.
    on_fallback: bool,
    /// The clip on the speakers, or kept from the last stop so a play
    /// starts at once where it stopped.
    held: Option<Held>,
    /// The clip queued in the playback, until the callback swaps it in.
    queued: Option<Held>,
    decoding: Option<Decoding>,
    next_id: u64,
    /// Tracks refused or failed this launch (until a rescan), skipped.
    bad: HashSet<PathBuf>,
    /// The way the wearer last stepped (1 or -1): a refused track is
    /// skipped this way.
    direction: isize,
    worker: Option<Worker>,
    /// Declared before `analysis` so the stream stops feeding the tap
    /// before its analysis thread is joined.
    pub(crate) playback: Option<Playback>,
    /// The analysis on the tap (`None` with `loop`, whose analysis stays
    /// on the microphones).
    pub(crate) analysis: Option<LiveAudio>,
    /// Swaps of the playback seen so far.
    swaps_seen: u32,
    /// Where the next play starts, in clip frames.
    resume: usize,
    /// The row's state: play asked for (the clip may still be decoding).
    want: bool,
    saved_file: PathBuf,
    saved: Saved,
}

impl Music {
    /// The player with no tracks yet, on `fallback` (`pinned`: it is
    /// `debug.fosfora.file`).
    pub fn new(
        fallback: PathBuf,
        pinned: bool,
        source: &str,
        options: PlaybackOptions,
        folders: Folders,
        saved_file: PathBuf,
        saved: Saved,
    ) -> Self {
        let worker = Worker::start()
            .inspect_err(|e| error!("music: no music thread: {e}"))
            .ok();
        Self {
            source: source.to_owned(),
            options,
            folders,
            playlist: Playlist::default(),
            fallback_title: title_of(&fallback),
            fallback,
            pinned,
            on_fallback: true,
            held: None,
            queued: None,
            decoding: None,
            next_id: 1,
            bad: HashSet::new(),
            direction: 1,
            worker,
            playback: None,
            analysis: None,
            swaps_seen: 0,
            resume: 0,
            want: false,
            saved_file,
            saved,
        }
    }

    /// Walk the folders now (the launch, before the frame loop) and start
    /// on the saved track when it is still there, else the first. Returns
    /// the scan and whether the saved track was found, for the launch line.
    pub fn scan_at_launch(&mut self) -> (Scan, bool) {
        let roots = self.folders.roots();
        let refs: Vec<(Folder, &Path)> = roots.iter().map(|(f, p)| (*f, p.as_path())).collect();
        let scan = playlist::scan(&refs);
        for (dir, why) in &scan.unreadable {
            warn!("music: {} unreadable: {why}", dir.display());
        }
        self.playlist = Playlist::new(scan.tracks.clone());
        let resumed = match self
            .saved
            .track
            .as_deref()
            .and_then(|p| self.playlist.index_of(p))
        {
            Some(i) => {
                self.playlist.select(i);
                true
            }
            None => false,
        };
        self.on_fallback = self.pinned || self.playlist.is_empty();
        (scan, resumed)
    }

    /// The selected track's title, `None` on the fallback.
    pub fn track_title(&self) -> Option<&str> {
        self.playlist
            .current_track()
            .filter(|_| !self.on_fallback)
            .map(|t| t.title.as_str())
    }

    /// What the Music row shows: the track's title, or the fallback's.
    pub fn title(&self) -> &str {
        self.track_title().unwrap_or(&self.fallback_title)
    }

    /// The tracks as the voice grammar names them.
    pub fn tracks(&self) -> crate::intent::Tracks<'_> {
        crate::intent::Tracks {
            titles: self.playlist.titles(),
            current: self.playlist.current().filter(|_| !self.on_fallback),
            playing: self.want,
        }
    }

    /// Take over a playback the launch source started (`file`, `loop`)
    /// with the clip it plays and its tap's analysis, if any: the row
    /// starts on "Music: stop".
    pub fn adopt(
        &mut self,
        playback: Playback,
        clip: Arc<Clip>,
        analysis: Option<LiveAudio>,
        path: PathBuf,
    ) {
        info!(
            "{}",
            crate::music::log_line(true, &self.source, &file_name(&path))
        );
        self.on_fallback = path == self.fallback || self.playlist.index_of(&path).is_none();
        self.held = Some(Held {
            path,
            clip,
            took: Duration::ZERO,
        });
        self.swaps_seen = playback.swaps();
        self.playback = Some(playback);
        self.analysis = analysis;
        self.want = true;
    }

    /// Whether play is asked for (the row's state).
    pub fn wanted(&self) -> bool {
        self.want
    }

    /// Whether the music is on the speakers.
    pub fn playing(&self) -> bool {
        self.playback.is_some()
    }

    /// The path the row is on.
    fn selected(&self) -> PathBuf {
        match self.playlist.current_track() {
            Some(t) if !self.on_fallback => t.path.clone(),
            _ => self.fallback.clone(),
        }
    }

    /// Whether the clip from `path` loops: the fallback, or a playlist of
    /// one.
    fn loops(&self, path: &Path) -> bool {
        path == self.fallback || self.playlist.len() <= 1
    }

    /// The launch's play, from `music::plays_at_launch`: not saved (the
    /// `debug.fosfora.music` knob is not the wearer's press).
    pub fn play_at_launch(&mut self) {
        self.play(true);
    }

    /// The Music row's press, or the voice's: play (from the kept clip at
    /// once, else after its decode) or stop. Saved.
    pub fn set(&mut self, on: bool) {
        self.play(on);
        self.save();
    }

    fn play(&mut self, on: bool) {
        self.want = on;
        if on {
            if self.playback.is_some() {
                return;
            }
            let path = self.selected();
            if self.held.as_ref().is_some_and(|h| h.path == path) {
                self.start();
            } else if self.decoding.as_ref().is_none_or(|d| d.path != path) {
                self.decode(path, true);
            }
            return;
        }
        if let Some(p) = self.playback.take() {
            self.settle(&p);
            self.resume = p.resume_frame();
            if let Some(q) = p.unqueue() {
                self.free(q);
            }
            if let Some(r) = p.take_retired() {
                self.free(r);
            }
            if let Some(q) = self.queued.take() {
                self.free(q.clip);
            }
        }
        // The next track's decode ahead of the end is nobody's now (a
        // step's is the next play's).
        self.decoding = self.decoding.take().filter(|d| d.now);
        self.analysis = None;
        info!(
            "{}",
            crate::music::log_line(false, &self.source, &file_name(&self.selected()))
        );
    }

    /// The Music row's `<` (-1) or `>` (1), or the voice's "next song":
    /// the next track not refused yet, wrapping. Playing, it decodes while
    /// the old one plays on and swaps in when ready; stopped, it is
    /// selected for the next play, from its top.
    pub fn step(&mut self, by: isize) {
        let n = self.playlist.len();
        let from = if self.on_fallback {
            None
        } else {
            self.playlist.current()
        };
        let mut at = playlist::stepped(n, from, by);
        for _ in 0..n {
            match at.and_then(|i| self.playlist.track(i)) {
                Some(t) if self.bad.contains(&t.path) => at = playlist::stepped(n, at, by),
                _ => break,
            }
        }
        match at {
            Some(i) => {
                self.direction = by.signum();
                self.go(i);
            }
            None => info!("music: no tracks to step through (put files in Music/)"),
        }
    }

    /// The voice's "play {song}": the playlist's track `i`, playing.
    pub fn play_track(&mut self, i: usize) {
        if i >= self.playlist.len() {
            return;
        }
        self.direction = 1;
        self.go(i);
        if !self.want {
            self.set(true);
        }
    }

    /// Make track `i` the row's: decoded to swap in at once while playing,
    /// else kept for the next play, from its top.
    fn go(&mut self, i: usize) {
        self.playlist.select(i);
        self.on_fallback = false;
        self.resume = 0;
        let path = self.selected();
        if self.want {
            self.decode(path, true);
        } else if let Some(h) = self.held.take_if(|h| h.path != path) {
            self.free(h.clip);
        }
        self.save();
    }

    /// The voice's "scan my music", and the storage permission's grant:
    /// walk the folders on the music thread (`poll` takes the result).
    pub fn rescan(&mut self) {
        let roots = self.folders.roots();
        info!(
            "music: scanning {}",
            roots
                .iter()
                .map(|(_, p)| p.display().to_string())
                .collect::<Vec<_>>()
                .join(" and ")
        );
        if let Some(w) = &self.worker {
            let _ = w.jobs.send(Job::Scan(roots));
        }
    }

    /// The storage permission arrived: the shared folder joins the scan.
    pub fn storage_granted(&mut self) {
        self.folders.shared_readable = true;
        self.rescan();
    }

    /// Ask the music thread for the clip at `path`; `now`: it swaps in at
    /// once when it is ready, else where the playing one ends.
    fn decode(&mut self, path: PathBuf, now: bool) {
        let Some(w) = &self.worker else {
            error!("music: no music thread to decode {}", path.display());
            self.want = false;
            return;
        };
        let id = self.next_id;
        self.next_id += 1;
        w.latest.store(id, Ordering::Relaxed);
        info!("music: decoding \"{}\"", title_of(&path));
        if w.jobs
            .send(Job::Decode {
                id,
                path: path.clone(),
            })
            .is_err()
        {
            error!("music: the music thread is gone");
            self.want = false;
            return;
        }
        self.decoding = Some(Decoding { id, path, now });
    }

    /// Hand `clip` to the music thread to free (a decoded track is about
    /// 100 MB; freeing it would stall the frame loop).
    fn free(&self, clip: Arc<Clip>) {
        match &self.worker {
            Some(w) => {
                let _ = w.jobs.send(Job::Free(clip));
            }
            None => drop(clip),
        }
    }

    /// Once per frame: the music thread's results, the swaps the playback
    /// made, and the next track's decode ahead of the playing one's end. A
    /// failed decode or start turns play back off (`wanted`), which the row
    /// shows.
    pub fn poll(&mut self) {
        if let Some(p) = self.playback.take() {
            self.settle(&p);
            self.playback = Some(p);
        }
        while let Some(done) = self.worker.as_ref().and_then(|w| w.done.try_recv().ok()) {
            match done {
                Done::Scanned(scan) => self.scanned(scan),
                Done::Decoded {
                    id,
                    path,
                    clip,
                    took,
                } => self.decoded(id, path, clip, took),
            }
        }
        let next = self.playback.as_ref().and_then(|p| {
            let path = self.held.as_ref()?.path.clone();
            (!self.loops(&path)
                && self.queued.is_none()
                && self.decoding.is_none()
                && p.remaining_secs() < PRELOAD_S)
                .then_some(path)
        });
        if let Some(path) = next
            && let Some(i) = self.next_good(&path, 1)
            && let Some(t) = self.playlist.track(i)
        {
            let path = t.path.clone();
            self.decode(path, false);
        }
    }

    /// Catch up with `p`'s swaps (the queued clip took over: it is the
    /// row's now, the old one is freed) and free what it swapped out.
    fn settle(&mut self, p: &Playback) {
        while self.swaps_seen < p.swaps() {
            self.swaps_seen += 1;
            if let Some(q) = self.queued.take() {
                self.took_over(q, p);
            }
        }
        if let Some(r) = p.take_retired() {
            self.free(r);
        }
    }

    /// `q` took over from the held clip in `p`.
    fn took_over(&mut self, q: Held, p: &Playback) {
        if let Some(i) = self.playlist.index_of(&q.path) {
            self.playlist.select(i);
            self.on_fallback = false;
        } else {
            self.on_fallback = true;
        }
        p.set_looping(self.loops(&q.path));
        info!(
            "{}",
            playlist::start_line(&title_of(&q.path), clip_secs(&q.clip), q.took.as_secs_f32())
        );
        if let Some(old) = self.held.replace(q) {
            self.free(old.clip);
        }
        self.save();
    }

    /// The track after `path` in `by`'s direction that is not refused,
    /// wrapping; `None` when every one is.
    fn next_good(&self, path: &Path, by: isize) -> Option<usize> {
        let n = self.playlist.len();
        let mut at = self.playlist.index_of(path);
        for _ in 0..n {
            at = playlist::stepped(n, at, by);
            if let Some(t) = at.and_then(|i| self.playlist.track(i))
                && !self.bad.contains(&t.path)
            {
                return at;
            }
        }
        None
    }

    fn decoded(&mut self, id: u64, path: PathBuf, clip: Result<Clip, String>, took: Duration) {
        let Some(d) = self.decoding.take_if(|d| d.id == id) else {
            // A newer decode was asked for: this one is nobody's.
            if let Ok(clip) = clip {
                self.free(Arc::new(clip));
            }
            return;
        };
        let clip = match clip {
            Ok(clip) => Arc::new(clip),
            Err(why) => {
                warn!("music: skipping \"{}\": {why}", title_of(&path));
                self.skip(&path, d.now);
                return;
            }
        };
        let held = Held { path, clip, took };
        let Some(p) = self.playback.take() else {
            if let Some(old) = self.held.replace(held) {
                self.free(old.clip);
            }
            if self.want {
                self.start();
            }
            return;
        };
        self.settle(&p);
        // Swapped in at once, it loops as it will once in; queued for the
        // end, the playing clip must play through to reach it.
        p.set_looping(d.now && self.loops(&held.path));
        match (p.queue(held.clip.clone(), d.now), self.queued.take()) {
            // The one queued before was never swapped in: both references
            // to it go.
            (Some(old), q) => {
                self.free(old);
                if let Some(q) = q {
                    self.free(q.clip);
                }
            }
            // The slot was empty: the one queued before was swapped in
            // since `settle` looked.
            (None, Some(q)) => {
                self.swaps_seen += 1;
                self.took_over(q, &p);
            }
            (None, None) => {}
        }
        self.queued = Some(held);
        self.playback = Some(p);
    }

    /// `path` was refused: move on past it the way the wearer was going
    /// (for a step: selected; for the track after the playing one: queued
    /// at its end), or back to the fallback when no track plays at all.
    fn skip(&mut self, path: &Path, now: bool) {
        if path == self.fallback {
            error!("music: the fallback clip does not play");
            self.want = false;
            return;
        }
        self.bad.insert(path.to_path_buf());
        let by = if now { self.direction } else { 1 };
        match self.next_good(path, by) {
            Some(i) => {
                if now {
                    self.playlist.select(i);
                    self.save();
                }
                if let Some(t) = self.playlist.track(i) {
                    let next = t.path.clone();
                    self.decode(next, now);
                }
            }
            None => {
                warn!("music: no track in the folders plays: the bundled clip");
                self.on_fallback = true;
                if self.want {
                    self.decode(self.fallback.clone(), true);
                }
            }
        }
    }

    fn scanned(&mut self, scan: Scan) {
        for (dir, why) in &scan.unreadable {
            warn!("music: {} unreadable: {why}", dir.display());
        }
        info!(
            "music: {} tracks ({} in Music, {} in the app folder){}",
            scan.tracks.len(),
            scan.count(Folder::Shared),
            scan.count(Folder::App),
            if scan.full { ", the cap" } else { "" }
        );
        self.bad.clear();
        self.playlist.replace(scan.tracks);
        // The folders had nothing and now have tracks: the first takes
        // over from the bundled clip.
        if self.on_fallback && !self.pinned && !self.playlist.is_empty() {
            self.on_fallback = false;
            self.resume = 0;
            if self.want {
                let path = self.selected();
                self.decode(path, true);
            }
            self.save();
        }
        if let (Some(p), Some(h)) = (&self.playback, &self.held) {
            p.set_looping(self.loops(&h.path));
        }
    }

    /// Start the held clip where the last stop left it, the analysis on
    /// its tap (as the `file` source does at launch).
    fn start(&mut self) {
        let Some(held) = &self.held else {
            return;
        };
        let (path, clip, took) = (held.path.clone(), held.clip.clone(), held.took);
        let tap = Arc::new(fosfora_app::audio::capture::RingBuffer::new());
        let options = PlaybackOptions {
            start_frame: self.resume,
            looping: self.loops(&path),
            ..self.options
        };
        match Playback::start(clip.clone(), tap.clone(), options) {
            Ok(p) => {
                if self.resume > 0 {
                    info!(
                        "music: from {:.1} s into the clip",
                        self.resume as f32 / clip.sample_rate.max(1) as f32
                    );
                } else {
                    info!(
                        "{}",
                        playlist::start_line(
                            &title_of(&path),
                            clip_secs(&clip),
                            took.as_secs_f32()
                        )
                    );
                }
                let analysis = crate::music::tap_analyzed(&self.source)
                    .then(|| LiveAudio::from_ring(tap, p.output_rate(), p.callback_count()))
                    .transpose();
                self.analysis = match analysis {
                    Ok(a) => a,
                    Err(e) => {
                        // Playback without its analysis would play to effects that
                        // never hear it; treat it like a failed start.
                        error!("music: analysis over the tap: {e:#}");
                        self.want = false;
                        return;
                    }
                };
                self.swaps_seen = p.swaps();
                self.playback = Some(p);
                info!(
                    "{}",
                    crate::music::log_line(true, &self.source, &file_name(&path))
                );
            }
            Err(e) => {
                error!("music: starting playback: {e:#}");
                self.want = false;
            }
        }
    }

    /// Save the row's track (not the fallback) and its play or stop in
    /// `music.json`, when they changed.
    fn save(&mut self) {
        let now = Saved {
            track: self
                .playlist
                .current_track()
                .map(|t| t.path.clone())
                .or_else(|| self.saved.track.clone()),
            playing: Some(self.want),
        };
        if now == self.saved {
            return;
        }
        match std::fs::write(&self.saved_file, now.to_json()) {
            Ok(()) => self.saved = now,
            Err(e) => warn!("music: saving {}: {e}", self.saved_file.display()),
        }
    }
}

/// The file name of `path`, for the log.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The saved state from `path` (`playlist::Saved`); nothing saved when it
/// is missing or broken.
pub fn load_saved(path: &Path) -> Saved {
    std::fs::read_to_string(path)
        .map(|s| Saved::parse(&s))
        .unwrap_or_default()
}
