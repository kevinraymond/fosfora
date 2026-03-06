use std::collections::HashMap;
use std::path::PathBuf;
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, TryRecvError, bounded};

use crate::media::decoder::MediaSource;
use crate::preset::Preset;

/// Request sent to the background decode thread.
pub struct PresetDecodeRequest {
    pub preset_index: usize,
    pub preset: Preset,
    /// (layer_index, media_path) pairs to decode.
    pub media_jobs: Vec<(usize, PathBuf)>,
    pub generation: u64,
}

/// Result of decoding a single media file.
pub enum MediaDecodeResult {
    Ok(MediaSource),
    Err(String),
}

/// Result sent back from the background thread.
pub struct PresetDecodeResult {
    pub preset_index: usize,
    pub preset: Preset,
    /// layer_index → decoded media (or error).
    pub decoded_media: HashMap<usize, MediaDecodeResult>,
    pub generation: u64,
}

/// Loading state visible to UI.
#[derive(Clone)]
pub enum PresetLoadingState {
    Idle,
    Loading {
        preset_name: String,
        preset_index: usize,
    },
}

/// Manages background preset decoding with generation-based cancellation.
pub struct PresetLoader {
    request_tx: Option<Sender<PresetDecodeRequest>>,
    result_rx: Receiver<PresetDecodeResult>,
    generation: u64,
    pub state: PresetLoadingState,
    _thread: Option<JoinHandle<()>>,
}

impl PresetLoader {
    pub fn new() -> Self {
        let (request_tx, request_rx) = bounded::<PresetDecodeRequest>(1);
        let (result_tx, result_rx) = bounded::<PresetDecodeResult>(1);

        let handle = thread::Builder::new()
            .name("phosphor-preset-loader".into())
            .spawn(move || {
                Self::decode_thread(request_rx, result_tx);
            })
            .expect("failed to spawn preset loader thread");

        Self {
            request_tx: Some(request_tx),
            result_rx,
            generation: 0,
            state: PresetLoadingState::Idle,
            _thread: Some(handle),
        }
    }

    /// Submit a new decode request. Bumps generation to cancel any in-flight work.
    pub fn request_load(
        &mut self,
        preset_index: usize,
        preset: Preset,
        media_jobs: Vec<(usize, PathBuf)>,
        preset_name: String,
    ) {
        self.generation += 1;
        let current_gen = self.generation;

        self.state = PresetLoadingState::Loading {
            preset_name,
            preset_index,
        };

        let request = PresetDecodeRequest {
            preset_index,
            preset,
            media_jobs,
            generation: current_gen,
        };

        if let Some(ref tx) = self.request_tx {
            match tx.try_send(request) {
                Ok(()) => {}
                Err(err) => {
                    // Channel full — drain stale result and yield to let decode thread proceed
                    let _ = self.result_rx.try_recv();
                    thread::yield_now();
                    // Retry with the recovered request; if still full, generation counter
                    // ensures the stale request will be discarded by the decode thread.
                    let _ = tx.try_send(err.into_inner());
                }
            }
        }
    }

    /// Poll for a completed decode result. Returns Some if a result matching
    /// the current generation is available.
    pub fn try_recv(&mut self) -> Option<PresetDecodeResult> {
        match self.result_rx.try_recv() {
            Ok(result) => {
                if result.generation == self.generation {
                    self.state = PresetLoadingState::Idle;
                    Some(result)
                } else {
                    // Stale result from cancelled request — discard
                    log::debug!(
                        "Discarded stale preset decode result (gen {} vs current {})",
                        result.generation,
                        self.generation
                    );
                    None
                }
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                log::warn!("Preset loader thread disconnected");
                self.state = PresetLoadingState::Idle;
                None
            }
        }
    }

    /// Background thread: receive requests, decode media, send results.
    fn decode_thread(
        request_rx: Receiver<PresetDecodeRequest>,
        result_tx: Sender<PresetDecodeResult>,
    ) {
        loop {
            // Block waiting for next request
            let mut request = match request_rx.recv() {
                Ok(r) => r,
                Err(_) => {
                    // Channel closed — App is shutting down
                    log::debug!("Preset loader thread exiting (channel closed)");
                    return;
                }
            };

            // Before starting work, drain any newer request that arrived
            loop {
                match request_rx.try_recv() {
                    Ok(newer) => {
                        log::debug!(
                            "Preset loader: skipping gen {} for newer gen {}",
                            request.generation,
                            newer.generation
                        );
                        request = newer;
                    }
                    Err(_) => break,
                }
            }

            'decode: loop {
                let mut decoded_media = HashMap::new();
                let mut cancelled = false;

                let num_jobs = request.media_jobs.len();
                for job_idx in 0..num_jobs {
                    // Check for cancellation between jobs
                    if let Ok(newer) = request_rx.try_recv() {
                        log::debug!(
                            "Preset loader: cancelled gen {} mid-decode, starting gen {}",
                            request.generation,
                            newer.generation
                        );
                        request = newer;
                        cancelled = true;
                        break;
                    }

                    let (layer_idx, ref path) = request.media_jobs[job_idx];
                    log::info!("Decoding media for layer {}: {}", layer_idx, path.display());
                    let result = match crate::media::decoder::load_media(path) {
                        Ok(source) => MediaDecodeResult::Ok(source),
                        Err(e) => {
                            log::warn!(
                                "Failed to decode media '{}' for preset: {}",
                                path.display(),
                                e
                            );
                            MediaDecodeResult::Err(e)
                        }
                    };
                    decoded_media.insert(layer_idx, result);
                }

                if cancelled {
                    continue 'decode;
                }

                let result = PresetDecodeResult {
                    preset_index: request.preset_index,
                    preset: request.preset,
                    decoded_media,
                    generation: request.generation,
                };

                // Send result (if channel full, old result is stale — try to drain it)
                if result_tx.try_send(result).is_err() {
                    // Result channel full — this shouldn't happen often since main thread
                    // drains each frame, but handle it gracefully
                    log::debug!("Preset loader: result channel full, dropping result");
                }
                break;
            }
        }
    }
}

impl Drop for PresetLoader {
    fn drop(&mut self) {
        // Drop the sender to signal thread exit
        self.request_tx.take();
        // Join the thread
        if let Some(handle) = self._thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_state_starts_idle() {
        let loader = PresetLoader::new();
        assert!(matches!(loader.state, PresetLoadingState::Idle));
    }

    #[test]
    fn generation_increments() {
        let mut loader = PresetLoader::new();
        assert_eq!(loader.generation, 0);

        let preset = Preset {
            layers: vec![],
            active_layer: 0,
            postprocess: Default::default(),
        };

        loader.request_load(0, preset.clone(), vec![], "Test".into());
        assert_eq!(loader.generation, 1);

        loader.request_load(1, preset, vec![], "Test2".into());
        assert_eq!(loader.generation, 2);
    }

    #[test]
    fn request_sets_loading_state() {
        let mut loader = PresetLoader::new();
        let preset = Preset {
            layers: vec![],
            active_layer: 0,
            postprocess: Default::default(),
        };

        loader.request_load(3, preset, vec![], "My Preset".into());

        match &loader.state {
            PresetLoadingState::Loading {
                preset_name,
                preset_index,
            } => {
                assert_eq!(preset_name, "My Preset");
                assert_eq!(*preset_index, 3);
            }
            _ => panic!("Expected Loading state"),
        }
    }

    #[test]
    fn empty_media_jobs_returns_result() {
        let mut loader = PresetLoader::new();
        let preset = Preset {
            layers: vec![],
            active_layer: 0,
            postprocess: Default::default(),
        };

        loader.request_load(0, preset, vec![], "Empty".into());

        // Give thread time to process
        std::thread::sleep(std::time::Duration::from_millis(50));

        let result = loader.try_recv();
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.preset_index, 0);
        assert!(result.decoded_media.is_empty());
        assert!(matches!(loader.state, PresetLoadingState::Idle));
    }

    #[test]
    fn missing_file_returns_error() {
        let mut loader = PresetLoader::new();
        let preset = Preset {
            layers: vec![],
            active_layer: 0,
            postprocess: Default::default(),
        };

        let jobs = vec![(0, PathBuf::from("/nonexistent/fake_image.png"))];
        loader.request_load(0, preset, jobs, "Bad".into());

        std::thread::sleep(std::time::Duration::from_millis(100));

        let result = loader.try_recv();
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.decoded_media.len(), 1);
        assert!(matches!(
            result.decoded_media.get(&0),
            Some(MediaDecodeResult::Err(_))
        ));
    }

    #[test]
    fn stale_result_discarded() {
        let mut loader = PresetLoader::new();
        let preset = Preset {
            layers: vec![],
            active_layer: 0,
            postprocess: Default::default(),
        };

        // Send first request
        loader.request_load(0, preset.clone(), vec![], "First".into());
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Bump generation without sending (simulates a rapid request cycle)
        loader.generation = 999;

        // The result from gen 1 should be discarded
        let result = loader.try_recv();
        assert!(result.is_none());
    }
}
