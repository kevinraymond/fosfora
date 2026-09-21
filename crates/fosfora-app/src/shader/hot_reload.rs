use std::collections::HashMap;
use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender};
use notify_debouncer_mini::{DebouncedEventKind, Debouncer, new_debouncer};

use crate::effect::loader::assets_dir;

/// Which consumer a changed file belongs to.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    /// A layer-stack shader under `assets/shaders`.
    Shader,
    /// A `.pfx` effect definition.
    Pfx,
    /// A trama effect file, `assets/trama/effects/*.wgsl`.
    Trama,
    Ignore,
}

/// Classify a changed path. By LOCATION first, then by extension: a trama
/// effect is a `.wgsl` too, and routed by extension alone it lands in the
/// layer-stack reload path, which matches it against no pass and drops it
/// without a word.
fn route(path: &std::path::Path) -> Route {
    let ext = |e: &str| path.extension().is_some_and(|x| x == e);
    // `Path::ends_with` compares whole components, so this holds whether
    // notify reports the path absolute or relative to the working directory.
    if path
        .parent()
        .is_some_and(|dir| dir.ends_with("trama/effects"))
    {
        return if ext("wgsl") {
            Route::Trama
        } else {
            Route::Ignore
        };
    }
    if ext("wgsl") {
        Route::Shader
    } else if ext("pfx") {
        Route::Pfx
    } else {
        Route::Ignore
    }
}

/// A file's modification time and size, or `None` if it does not exist. Read
/// with `stat`, which — unlike opening the file — is not itself an event.
type Stamp = Option<(std::time::SystemTime, u64)>;

fn stamp(path: &std::path::Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// Record the stamp of every file under `dir`, so that the reads the app does
/// while STARTING (loading every shader and effect it is about to watch) are
/// recognized as reads.
fn seed(dir: &std::path::Path, recursive: bool, seen: &mut HashMap<PathBuf, Stamp>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(Result::ok).map(|e| e.path()) {
        if path.is_dir() {
            if recursive {
                seed(&path, true, seen);
            }
        } else if route(&path) != Route::Ignore {
            seen.insert(path.clone(), stamp(&path));
        }
    }
}

pub struct ShaderWatcher {
    /// `None` when no watcher could be created at all: hot reload is off and
    /// the channels below never deliver.
    _debouncer: Option<Debouncer<notify::RecommendedWatcher>>,
    receiver: Receiver<PathBuf>,
    pfx_receiver: Receiver<PathBuf>,
    trama_receiver: Receiver<PathBuf>,
    /// The stamp each file had when it was last reported. notify subscribes
    /// to inotify OPEN and the mini-debouncer flattens every event to
    /// "changed", so merely READING a watched file is reported as a change —
    /// and a consumer that reloads on change reads the file. An event whose
    /// stamp has not moved is a read, and is dropped here for all three
    /// routes.
    seen: HashMap<PathBuf, Stamp>,
    /// What could not be watched, for a one-time note in the status bar.
    /// Taken by the first frame.
    degraded: Option<String>,
}

impl ShaderWatcher {
    /// Never fails. Hot reload is a development convenience, and a watch that
    /// cannot be set up (typically the OS file-watch limit, exhausted by an
    /// editor watching a large tree elsewhere on the machine) used to abort
    /// app startup with no window (#2667). Now it is logged, noted once in the
    /// status bar, and the app runs without hot reload for that directory.
    pub fn new() -> Self {
        Self::watching(
            &assets_dir().join("shaders"),
            &assets_dir().join("effects"),
            &crate::trama::effect::trama_effects_dir(),
        )
    }

    /// The watcher over explicit directories, so a test can point it at a
    /// scratch tree instead of the real assets.
    fn watching(
        shader_dir: &std::path::Path,
        effects_dir: &std::path::Path,
        trama_dir: &std::path::Path,
    ) -> Self {
        // Absolute, all three. In the dev workflow `assets_dir()` is the
        // RELATIVE path `assets`, while notify reports absolute paths — so
        // stamps seeded under the relative spelling were never found, and
        // every file the app read while starting was reported once as new.
        let absolute = |p: &std::path::Path| std::path::absolute(p).unwrap_or(p.to_path_buf());
        let (shader_dir, effects_dir, trama_dir) = (
            &absolute(shader_dir),
            &absolute(effects_dir),
            &absolute(trama_dir),
        );
        let (tx, rx): (Sender<PathBuf>, Receiver<PathBuf>) = crossbeam_channel::unbounded();
        let (pfx_tx, pfx_rx): (Sender<PathBuf>, Receiver<PathBuf>) = crossbeam_channel::unbounded();
        let (trama_tx, trama_rx): (Sender<PathBuf>, Receiver<PathBuf>) =
            crossbeam_channel::unbounded();

        let debouncer = new_debouncer(
            std::time::Duration::from_millis(100),
            move |res: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
                if let Ok(events) = res {
                    for event in events {
                        if event.kind == DebouncedEventKind::Any {
                            let path = event.path.clone();
                            let _ = match route(&path) {
                                Route::Shader => tx.send(path),
                                Route::Pfx => pfx_tx.send(path),
                                Route::Trama => trama_tx.send(path),
                                Route::Ignore => Ok(()),
                            };
                        }
                    }
                }
            },
        );

        let mut failed: Vec<String> = Vec::new();
        let debouncer = match debouncer {
            Ok(mut debouncer) => {
                for (dir, mode, what) in [
                    (shader_dir, notify::RecursiveMode::Recursive, "shader"),
                    (effects_dir, notify::RecursiveMode::Recursive, ".pfx"),
                    (
                        trama_dir,
                        notify::RecursiveMode::NonRecursive,
                        "trama effect",
                    ),
                ] {
                    if !dir.exists() {
                        continue;
                    }
                    match debouncer.watcher().watch(dir, mode) {
                        Ok(()) => log::info!("Watching {} for {what} changes", dir.display()),
                        Err(e) => {
                            log::warn!(
                                "{what} files will not hot-reload: cannot watch {}: {e}",
                                dir.display()
                            );
                            failed.push(format!("{e}"));
                        }
                    }
                }
                Some(debouncer)
            }
            Err(e) => {
                log::warn!("Hot reload is off: cannot create a file watcher: {e}");
                failed.push(format!("{e}"));
                None
            }
        };
        // One line for the status bar. The reasons are usually all the same
        // (the watch limit), so name the first and leave the rest to the log.
        let degraded = failed
            .first()
            .map(|reason| format!("Hot reload off: {reason}"));

        let mut seen = HashMap::new();
        seed(shader_dir, true, &mut seen);
        seed(effects_dir, true, &mut seen);
        seed(trama_dir, false, &mut seen);

        Self {
            _debouncer: debouncer,
            receiver: rx,
            pfx_receiver: pfx_rx,
            trama_receiver: trama_rx,
            seen,
            degraded,
        }
    }

    /// The note for the status bar if some directory could not be watched,
    /// once.
    pub fn take_degraded_notice(&mut self) -> Option<String> {
        self.degraded.take()
    }

    /// Unique paths from `receiver` whose file really changed: created,
    /// written, replaced or deleted since it was last reported.
    fn drain(receiver: &Receiver<PathBuf>, seen: &mut HashMap<PathBuf, Stamp>) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        while let Ok(path) = receiver.try_recv() {
            if paths.contains(&path) {
                continue;
            }
            let now = stamp(&path);
            // A path never seen before is a new file. (If notify ever spells
            // a path differently from the seeding walk, the cost is one
            // spurious report per file, not a loop.)
            if seen.insert(path.clone(), now) != Some(now) {
                paths.push(path);
            }
        }
        paths
    }

    /// Changed layer-stack shaders (`.wgsl` under `assets/shaders`).
    pub fn drain_changes(&mut self) -> Vec<PathBuf> {
        Self::drain(&self.receiver, &mut self.seen)
    }

    /// Changed trama effect files.
    pub fn drain_trama_changes(&mut self) -> Vec<PathBuf> {
        Self::drain(&self.trama_receiver, &mut self.seen)
    }

    /// Changed `.pfx` effect definitions.
    pub fn drain_pfx_changes(&mut self) -> Vec<PathBuf> {
        Self::drain(&self.pfx_receiver, &mut self.seen)
    }
}

#[cfg(test)]
mod tests {
    use super::{Route, ShaderWatcher, route};
    use std::path::Path;

    // Run: cargo test -p fosfora-app -- reading_a_watched_file_is_not_a_change
    //
    // notify subscribes to inotify OPEN, and the mini-debouncer flattens every
    // event to "changed" — so READING a watched file reports a change. A
    // consumer that reloads on change reads the file, which reports a change:
    // the first build of trama hot reload recompiled all four effects every
    // 100 ms from launch, forever, lit by the registry's own initial load.
    // Linux only: that is where OPEN is an event.
    #[cfg(target_os = "linux")]
    #[test]
    fn reading_a_watched_file_is_not_a_change() {
        let root = std::env::temp_dir().join(format!("fosfora-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (shaders, effects, trama) = (
            root.join("shaders"),
            root.join("effects"),
            root.join("trama/effects"),
        );
        for d in [&shaders, &effects, &trama] {
            std::fs::create_dir_all(d).unwrap();
        }
        let file = trama.join("hue_drift.wgsl");
        std::fs::write(&file, "one").unwrap();
        // Reach the tree through a RELATIVE path, as the dev workflow does
        // (`assets/…`): notify reports absolute paths, and stamps seeded under
        // any other spelling are never found.
        let up: std::path::PathBuf = std::env::current_dir()
            .unwrap()
            .components()
            .skip(1)
            .map(|_| "..")
            .collect();
        let relative = |abs: &Path| up.join(abs.strip_prefix("/").unwrap());
        assert!(relative(&trama).is_relative() && relative(&trama).is_dir());
        let mut w =
            ShaderWatcher::watching(&relative(&shaders), &relative(&effects), &relative(&trama));

        // Poll the way the app does. `quiet` waits out the whole window (the
        // debounce is 100 ms); `next` returns as soon as something arrives.
        let mut poll = |ms: u64, stop_early: bool| {
            let mut seen = Vec::new();
            let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            while std::time::Instant::now() < until && (seen.is_empty() || !stop_early) {
                seen.extend(w.drain_trama_changes());
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            seen
        };
        let none = Vec::<std::path::PathBuf>::new();

        // What the registry does at startup, and what every reload does.
        for _ in 0..3 {
            std::fs::read_to_string(&file).unwrap();
        }
        assert_eq!(poll(400, false), none, "a read is not a change");

        std::fs::write(&file, "two, and longer").unwrap();
        let reported = poll(3000, true);
        assert_eq!(reported.len(), 1, "a write is: {reported:?}");
        assert!(reported[0].ends_with("trama/effects/hue_drift.wgsl"));
        std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            poll(400, false),
            none,
            "reading it back is not, and the write is not reported twice"
        );

        std::fs::remove_file(&file).unwrap();
        assert_eq!(poll(3000, true).len(), 1, "a delete is a change");

        // A file that has never existed before. The stamp filter keys on path,
        // so a new one has nothing seeded and must come through — this is what
        // makes `xtask new-effect` land in a running app instead of needing a
        // restart, and `TramaRegistry::reload_file` treats an unknown id as an
        // addition rather than a swap.
        let fresh = trama.join("brand_new.wgsl");
        std::fs::write(&fresh, "created while the app was running").unwrap();
        let reported = poll(3000, true);
        assert_eq!(
            reported.len(),
            1,
            "a newly created effect is a change: {reported:?}"
        );
        assert!(reported[0].ends_with("trama/effects/brand_new.wgsl"));

        std::fs::remove_dir_all(&root).unwrap();
    }

    // Run: cargo test -p fosfora-app -- a_directory_that_cannot_be_watched_is_not_fatal
    //
    // #2667: a VS Code watcher held 64,758 of the machine's 65,536 inotify
    // watches, the shader watch failed, and `?` took app startup down with it
    // — no window, exit 0. The failure injected here is a real one (an
    // unreadable directory, EACCES from inotify_add_watch), not a mock.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_directory_that_cannot_be_watched_is_not_fatal() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("fosfora-nowatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (shaders, effects, trama) = (
            root.join("shaders"),
            root.join("effects"),
            root.join("trama/effects"),
        );
        for d in [&shaders, &effects, &trama] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::set_permissions(&shaders, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads through any mode bits; the watch would succeed and the
        // test would prove nothing.
        if std::fs::read_dir(&shaders).is_ok() {
            std::fs::set_permissions(&shaders, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::remove_dir_all(&root).unwrap();
            eprintln!("skipped: running with permission to read a mode-000 directory");
            return;
        }

        let mut w = ShaderWatcher::watching(&shaders, &effects, &trama);
        std::fs::set_permissions(&shaders, std::fs::Permissions::from_mode(0o755)).unwrap();

        let notice = w.take_degraded_notice();
        assert!(
            notice
                .as_deref()
                .is_some_and(|n| n.starts_with("Hot reload off")),
            "the failed watch is reported for the status bar: {notice:?}"
        );
        assert_eq!(w.take_degraded_notice(), None, "once");

        // The directories that COULD be watched still hot-reload.
        let file = trama.join("hue_drift.wgsl");
        std::fs::write(&file, "one").unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut reported = Vec::new();
        while reported.is_empty() && std::time::Instant::now() < until {
            reported.extend(w.drain_trama_changes());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(reported.len(), 1, "trama still watched: {reported:?}");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_trama_effect_is_routed_by_where_it_lives() {
        for p in [
            "assets/trama/effects/hue_drift.wgsl",
            "/home/x/fosfora/assets/trama/effects/hue_drift.wgsl",
        ] {
            assert_eq!(route(Path::new(p)), Route::Trama, "{p}");
        }
        // Everything that was routed before still is.
        assert_eq!(
            route(Path::new("assets/shaders/lib/noise.wgsl")),
            Route::Shader
        );
        assert_eq!(route(Path::new("assets/effects/aurora.pfx")), Route::Pfx);
        // An editor's swap file next to a trama effect is nobody's.
        assert_eq!(
            route(Path::new("assets/trama/effects/.hue_drift.wgsl.swp")),
            Route::Ignore
        );
        assert_eq!(
            route(Path::new("assets/trama/effects/notes.txt")),
            Route::Ignore
        );
    }
}
