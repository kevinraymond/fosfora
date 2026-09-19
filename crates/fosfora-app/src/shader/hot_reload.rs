use std::path::PathBuf;

use anyhow::Result;
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

pub struct ShaderWatcher {
    _debouncer: Debouncer<notify::RecommendedWatcher>,
    receiver: Receiver<PathBuf>,
    pfx_receiver: Receiver<PathBuf>,
    trama_receiver: Receiver<PathBuf>,
}

impl ShaderWatcher {
    pub fn new() -> Result<Self> {
        let (tx, rx): (Sender<PathBuf>, Receiver<PathBuf>) = crossbeam_channel::unbounded();
        let (pfx_tx, pfx_rx): (Sender<PathBuf>, Receiver<PathBuf>) = crossbeam_channel::unbounded();
        let (trama_tx, trama_rx): (Sender<PathBuf>, Receiver<PathBuf>) =
            crossbeam_channel::unbounded();

        let mut debouncer = new_debouncer(
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
        )?;

        // Watch assets/shaders for .wgsl changes
        let shader_dir = assets_dir().join("shaders");
        if shader_dir.exists() {
            debouncer
                .watcher()
                .watch(&shader_dir, notify::RecursiveMode::Recursive)?;
            log::info!("Watching {} for shader changes", shader_dir.display());
        }

        // Watch assets/effects for .pfx changes
        let effects_dir = assets_dir().join("effects");
        if effects_dir.exists() {
            debouncer
                .watcher()
                .watch(&effects_dir, notify::RecursiveMode::Recursive)?;
            log::info!("Watching {} for .pfx changes", effects_dir.display());
        }

        // trama effect files. NOT fatal if it cannot be watched: hot reload is
        // a convenience, and the two watches above taking the whole app down
        // when they fail is its own bug.
        let trama_dir = crate::trama::effect::trama_effects_dir();
        if trama_dir.exists() {
            match debouncer
                .watcher()
                .watch(&trama_dir, notify::RecursiveMode::NonRecursive)
            {
                Ok(()) => log::info!("Watching {} for trama effects", trama_dir.display()),
                Err(e) => log::warn!(
                    "trama effects will not hot-reload: cannot watch {}: {e}",
                    trama_dir.display()
                ),
            }
        }

        Ok(Self {
            _debouncer: debouncer,
            receiver: rx,
            pfx_receiver: pfx_rx,
            trama_receiver: trama_rx,
        })
    }

    /// Drain all pending .wgsl change events and return the unique paths.
    pub fn drain_changes(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        while let Ok(path) = self.receiver.try_recv() {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    }

    /// Drain all pending trama effect file changes and return the unique paths.
    pub fn drain_trama_changes(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        while let Ok(path) = self.trama_receiver.try_recv() {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    }

    /// Drain all pending .pfx change events and return the unique paths.
    pub fn drain_pfx_changes(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        while let Ok(path) = self.pfx_receiver.try_recv() {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::{Route, route};
    use std::path::Path;

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
