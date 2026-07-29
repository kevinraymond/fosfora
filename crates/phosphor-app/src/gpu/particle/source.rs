//! The active particle source, as one value.
//!
//! Particles can be fed by a still image, a video or GIF, a live webcam, or a 3D
//! model rastered to a frame. Which one is live used to be spread across four
//! fields — a `ParticleImageSource` enum plus three `Option<String>` paths — that
//! were mutually exclusive *by convention*: every writer had to remember to clear
//! its siblings. One of them (`static_model_path`) had no writer that cleared it
//! at all, so a picture loaded over a model left both set, and the panel kept
//! naming the model while showing the picture (#2011).
//!
//! [`ParticleSource`] is that state as a single field, so the disagreement is not
//! representable. Switching source is one assignment, and the only decision left
//! is which source a *preset* describes — settled once in
//! [`SourcePresetFields::resolve`] rather than at each restore site.

use super::emitter::EmitterDef;
use crate::media::types::DecodedFrame;

/// The active particle source. Exactly one, by construction (#2011).
///
/// Replaces `image_source` plus the `video_path` / `static_image_path` /
/// `static_model_path` trio. Built-in and user-picked images are both
/// [`ParticleSource::Image`]: nothing in the codebase distinguishes them except
/// the `raster_` file-name convention in [`super::source_loader`].
#[derive(Default)]
pub enum ParticleSource {
    /// No source loaded. What a particle system has before anything is sampled,
    /// and what an effect with a non-image emitter stays at.
    #[default]
    None,
    /// A still image — built-in or user-picked.
    Image { path: String },
    /// Pre-decoded video or GIF frames, played back and re-sampled per frame.
    #[cfg(feature = "video")]
    Video {
        path: String,
        frames: Vec<DecodedFrame>,
        delays_ms: Vec<u32>,
        playback: VideoPlayback,
    },
    /// Live webcam feed — frames arrive externally via
    /// [`super::system::ParticleSystem::update_webcam_frame`], which carries its
    /// own dimensions, so these are recorded for the panel rather than read back.
    #[cfg(feature = "webcam")]
    #[allow(dead_code)]
    Webcam { width: u32, height: u32 },
    /// A 3D mesh or splat capture, rastered to a frame and sampled like an image
    /// (#1993). The pose it was sampled at lives in `ParticleSystem::model_sample`,
    /// not here — a morph *target* sets that pose with no model source active.
    Model { path: String },
}

/// Transport state for a video source. Split out of the variant so callers that
/// only drive playback do not have to destructure the whole source.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoPlayback {
    pub current_frame: usize,
    pub frame_elapsed_ms: f64,
    pub playing: bool,
    pub looping: bool,
    pub speed: f32,
}

impl Default for VideoPlayback {
    fn default() -> Self {
        Self {
            current_frame: 0,
            frame_elapsed_ms: 0.0,
            playing: true,
            looping: true,
            speed: 1.0,
        }
    }
}

/// Which kind of source is live, for code that needs to branch without caring
/// about the payload. Carried to the UI in place of the old `source_type: String`,
/// so the panel matches instead of comparing string literals.
///
/// Not feature-gated even though `Video` and `Webcam` are: the panel and the
/// preset resolver have to name a kind this build cannot construct — a preset
/// naming a webcam still has to be recognised and declined, not silently read as
/// an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ParticleSourceKind {
    None,
    Image,
    Video,
    Webcam,
    Model,
}

impl ParticleSource {
    /// Which kind of source this is.
    pub fn kind(&self) -> ParticleSourceKind {
        match self {
            ParticleSource::None => ParticleSourceKind::None,
            ParticleSource::Image { .. } => ParticleSourceKind::Image,
            #[cfg(feature = "video")]
            ParticleSource::Video { .. } => ParticleSourceKind::Video,
            #[cfg(feature = "webcam")]
            ParticleSource::Webcam { .. } => ParticleSourceKind::Webcam,
            ParticleSource::Model { .. } => ParticleSourceKind::Model,
        }
    }

    /// The file this source came from, if it has one.
    pub fn path(&self) -> Option<&str> {
        match self {
            ParticleSource::Image { path } | ParticleSource::Model { path } => Some(path),
            #[cfg(feature = "video")]
            ParticleSource::Video { path, .. } => Some(path),
            #[cfg(feature = "webcam")]
            ParticleSource::Webcam { .. } => None,
            ParticleSource::None => None,
        }
    }

    /// What the panel shows under the source picker. The bare file name for
    /// anything with a path — a full path does not fit the panel, and the old
    /// code already took the file name for models while showing video sources
    /// their whole absolute path.
    pub fn display_name(&self) -> String {
        match self {
            ParticleSource::None => String::new(),
            #[cfg(feature = "webcam")]
            ParticleSource::Webcam { .. } => "webcam".to_string(),
            _ => match self.path() {
                Some(path) => std::path::Path::new(path)
                    .file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string()),
                None => String::new(),
            },
        }
    }

    /// True when the source produces no per-frame updates, so
    /// [`super::system::ParticleSystem::update_source`] can return early.
    pub fn is_static(&self) -> bool {
        !matches!(
            self.kind(),
            ParticleSourceKind::Video | ParticleSourceKind::Webcam
        )
    }

    /// Only called behind `feature = "webcam"`, so it is dead in a default build.
    #[allow(dead_code)]
    pub fn is_webcam(&self) -> bool {
        self.kind() == ParticleSourceKind::Webcam
    }

    /// Advance playback by dt seconds. Returns true if the current frame changed.
    #[allow(unused_variables)]
    pub fn advance(&mut self, dt_secs: f64) -> bool {
        match self {
            #[cfg(feature = "video")]
            ParticleSource::Video {
                frames,
                delays_ms,
                playback,
                ..
            } => {
                if !playback.playing || frames.is_empty() {
                    return false;
                }
                playback.frame_elapsed_ms += dt_secs * 1000.0 * (playback.speed as f64);
                let delay = delays_ms.get(playback.current_frame).copied().unwrap_or(33) as f64;
                if playback.frame_elapsed_ms >= delay {
                    playback.frame_elapsed_ms -= delay;
                    let next = playback.current_frame + 1;
                    if next >= frames.len() {
                        if playback.looping {
                            playback.current_frame = 0;
                        } else {
                            playback.playing = false;
                        }
                    } else {
                        playback.current_frame = next;
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// The current frame's raw RGBA data (video only).
    pub fn current_frame_data(&self) -> Option<&DecodedFrame> {
        match self {
            #[cfg(feature = "video")]
            ParticleSource::Video {
                frames, playback, ..
            } => frames.get(playback.current_frame),
            _ => None,
        }
    }

    #[allow(dead_code)]
    pub fn frame_count(&self) -> usize {
        match self {
            #[cfg(feature = "video")]
            ParticleSource::Video { frames, .. } => frames.len(),
            _ => 0,
        }
    }

    /// Mutable transport state (video only), for the panel's play/loop/speed
    /// controls. Dead in a build without `feature = "video"`, where no source can
    /// carry playback state at all.
    #[allow(dead_code)]
    pub fn playback_mut(&mut self) -> Option<&mut VideoPlayback> {
        match self {
            #[cfg(feature = "video")]
            ParticleSource::Video { playback, .. } => Some(playback),
            _ => None,
        }
    }

    /// Read-only transport state (video only).
    pub fn playback(&self) -> Option<&VideoPlayback> {
        match self {
            #[cfg(feature = "video")]
            ParticleSource::Video { playback, .. } => Some(playback),
            _ => None,
        }
    }

    /// Current position in seconds (0.0 when not a video).
    pub fn video_position_secs(&self) -> f64 {
        match self {
            #[cfg(feature = "video")]
            ParticleSource::Video {
                delays_ms,
                playback,
                ..
            } => {
                let ms: f64 = delays_ms
                    .iter()
                    .take(playback.current_frame)
                    .map(|d| *d as f64)
                    .sum();
                (ms + playback.frame_elapsed_ms) / 1000.0
            }
            _ => 0.0,
        }
    }

    /// Total duration in seconds (0.0 when not a video).
    pub fn video_duration_secs(&self) -> f64 {
        match self {
            #[cfg(feature = "video")]
            ParticleSource::Video { delays_ms, .. } => {
                delays_ms.iter().map(|d| *d as f64).sum::<f64>() / 1000.0
            }
            _ => 0.0,
        }
    }

    /// Seek to a time position (video only).
    #[allow(dead_code, unused_variables)]
    pub fn seek_to_secs(&mut self, target_secs: f64) {
        #[cfg(feature = "video")]
        if let ParticleSource::Video {
            delays_ms,
            playback,
            ..
        } = self
        {
            let target_ms = target_secs * 1000.0;
            let mut accumulated = 0.0f64;
            for (i, delay) in delays_ms.iter().enumerate() {
                let d = *delay as f64;
                if accumulated + d > target_ms {
                    playback.current_frame = i;
                    playback.frame_elapsed_ms = target_ms - accumulated;
                    return;
                }
                accumulated += d;
            }
            // Past end — clamp to last frame.
            if !delays_ms.is_empty() {
                playback.current_frame = delays_ms.len() - 1;
                playback.frame_elapsed_ms = 0.0;
            }
        }
    }

    /// The preset fields describing this source. The inverse of
    /// [`SourcePresetFields::resolve`].
    pub fn to_preset_fields(&self) -> SourcePresetFields {
        let mut fields = SourcePresetFields::default();
        match self {
            ParticleSource::None => {}
            ParticleSource::Image { path } => fields.image_path = Some(path.clone()),
            ParticleSource::Model { path } => fields.model_path = Some(path.clone()),
            #[cfg(feature = "video")]
            ParticleSource::Video { path, playback, .. } => {
                fields.video_path = Some(path.clone());
                fields.video_speed = Some(playback.speed);
                fields.video_looping = Some(playback.looping);
            }
            #[cfg(feature = "webcam")]
            ParticleSource::Webcam { .. } => fields.webcam = Some(true),
        }
        fields
    }
}

/// Mirror the active source into the declarative `emitter` fields a rebuilt
/// particle system reads back.
///
/// Every field is cleared first, so exactly one can be set. Previously only the
/// model path did any clearing, and a video left a stale `emitter.image` behind
/// for the panel to name (#2011).
///
/// Free function rather than a method on `ParticleSystem` so it is reachable
/// without a GPU: constructing a `ParticleSystem` needs a device, which is why
/// every test that builds one is `#[ignore]`d and never runs in CI.
pub fn sync_emitter(emitter: &mut EmitterDef, source: &ParticleSource) {
    emitter.image = String::new();
    emitter.model = String::new();
    emitter.video = String::new();
    match source {
        ParticleSource::None => {}
        ParticleSource::Image { path } => {
            // The emitter names a file inside assets/images/, so store the bare
            // name; an absolute path from the picker resolves anyway.
            emitter.image = std::path::Path::new(path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
        }
        ParticleSource::Model { path } => emitter.model = path.clone(),
        #[cfg(feature = "video")]
        ParticleSource::Video { path, .. } => emitter.video = path.clone(),
        #[cfg(feature = "webcam")]
        ParticleSource::Webcam { .. } => emitter.video = "webcam".to_string(),
    }
}

/// What to *load*, as opposed to what is loaded. A restored preset names a file;
/// turning it into a [`ParticleSource`] needs decoding (video) or the GPU (model),
/// so the two types stay separate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceSpec {
    Image(String),
    Video(String),
    Webcam,
    Model(String),
}

/// The source-describing fields of a saved layer preset, gathered into one value
/// so save and restore share a single definition of what they mean.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourcePresetFields {
    pub video_path: Option<String>,
    pub video_speed: Option<f32>,
    pub video_looping: Option<bool>,
    pub webcam: Option<bool>,
    pub image_path: Option<String>,
    pub model_path: Option<String>,
}

impl SourcePresetFields {
    /// Which single source this preset describes.
    ///
    /// The precedence — video, then webcam, then model, then image — is the one
    /// the sequentially-gated restore blocks already implemented. It has to stay
    /// total rather than assume one field is set, because presets written before
    /// the source became one field could record two at once: the model bug in
    /// #2011 saved `particle_image_path` and `particle_model_path` together, and
    /// those presets are already on disk.
    pub fn resolve(&self) -> Option<SourceSpec> {
        if let Some(path) = &self.video_path {
            return Some(SourceSpec::Video(path.clone()));
        }
        if self.webcam == Some(true) {
            return Some(SourceSpec::Webcam);
        }
        if let Some(path) = &self.model_path {
            return Some(SourceSpec::Model(path.clone()));
        }
        if let Some(path) = &self.image_path {
            return Some(SourceSpec::Image(path.clone()));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(path: &str) -> ParticleSource {
        ParticleSource::Image {
            path: path.to_string(),
        }
    }

    fn model(path: &str) -> ParticleSource {
        ParticleSource::Model {
            path: path.to_string(),
        }
    }

    #[cfg(feature = "video")]
    fn video(path: &str) -> ParticleSource {
        ParticleSource::Video {
            path: path.to_string(),
            frames: Vec::new(),
            delays_ms: vec![100, 100],
            playback: VideoPlayback::default(),
        }
    }

    /// Every variant, so a new one cannot be added without deciding what it is
    /// called and what it reports.
    fn all_sources() -> Vec<ParticleSource> {
        #[allow(unused_mut)]
        let mut v = vec![
            ParticleSource::None,
            image("/home/kevin/pics/raster_skull.png"),
            model("/home/kevin/models/skull.glb"),
        ];
        #[cfg(feature = "video")]
        v.push(video("/home/kevin/clips/loop.mp4"));
        #[cfg(feature = "webcam")]
        v.push(ParticleSource::Webcam {
            width: 640,
            height: 480,
        });
        v
    }

    #[test]
    fn every_source_reports_its_own_kind_and_name() {
        assert_eq!(ParticleSource::None.kind(), ParticleSourceKind::None);
        assert_eq!(
            image("/a/b/raster_skull.png").kind(),
            ParticleSourceKind::Image
        );
        assert_eq!(model("/a/b/skull.glb").kind(), ParticleSourceKind::Model);

        // The panel shows a file name, never a full path — the old code took the
        // file name for models but handed video its whole absolute path.
        assert_eq!(
            image("/home/kevin/pics/raster_skull.png").display_name(),
            "raster_skull.png"
        );
        assert_eq!(
            model("/home/kevin/models/skull.glb").display_name(),
            "skull.glb"
        );
        assert_eq!(ParticleSource::None.display_name(), "");

        #[cfg(feature = "video")]
        {
            assert_eq!(
                video("/home/kevin/clips/loop.mp4").kind(),
                ParticleSourceKind::Video
            );
            assert_eq!(
                video("/home/kevin/clips/loop.mp4").display_name(),
                "loop.mp4"
            );
        }
        #[cfg(feature = "webcam")]
        {
            let cam = ParticleSource::Webcam {
                width: 640,
                height: 480,
            };
            assert_eq!(cam.kind(), ParticleSourceKind::Webcam);
            assert_eq!(cam.display_name(), "webcam");
        }

        // No two variants report the same kind.
        let kinds: Vec<_> = all_sources().iter().map(|s| s.kind()).collect();
        for (i, a) in kinds.iter().enumerate() {
            for b in kinds.iter().skip(i + 1) {
                assert_ne!(a, b, "two variants share a kind");
            }
        }
    }

    /// #2011: switching source must retire the previous one, in every direction.
    /// The four hand-written `= None` clears this replaces covered only the
    /// transitions someone thought of; this walks all of them.
    #[test]
    fn switching_source_retires_the_previous_one() {
        let n = all_sources().len();
        for from_idx in 0..n {
            for to_idx in 0..n {
                let mut live = all_sources().swap_remove(from_idx);
                let from_name = live.display_name();

                let to = all_sources().swap_remove(to_idx);
                let to_kind = to.kind();
                let to_name = to.display_name();

                // The switch under test: one assignment replaces the whole
                // discriminator. No sibling field is left to forget.
                live = to;

                assert_eq!(live.kind(), to_kind);
                assert_eq!(live.display_name(), to_name);
                // Nothing of the previous source survives — including its name,
                // which is exactly what the panel kept showing when a stale model
                // path outlived the picture loaded over it (#2011).
                if from_name != to_name {
                    assert_ne!(live.display_name(), from_name);
                }

                // Nothing of the previous source can survive: there is one field,
                // so the assertions below are about the *derived* answers every
                // consumer reads — label, pose gate, transport gate.
                assert_eq!(live.is_webcam(), to_kind == ParticleSourceKind::Webcam);
                assert_eq!(
                    live.is_static(),
                    !matches!(
                        to_kind,
                        ParticleSourceKind::Video | ParticleSourceKind::Webcam
                    )
                );
                assert_eq!(
                    live.playback().is_some(),
                    to_kind == ParticleSourceKind::Video
                );
                assert_eq!(
                    live.path().is_some(),
                    matches!(
                        to_kind,
                        ParticleSourceKind::Image
                            | ParticleSourceKind::Model
                            | ParticleSourceKind::Video
                    )
                );
            }
        }
    }

    /// The regression that #2011 actually reported, at the level it happened.
    ///
    /// `apply_model_source` cleared `emitter.image` when a model won, but nothing
    /// did the reverse, so a picture loaded over a model left `emitter.model` set
    /// and the panel kept naming the model. Walk every ordered pair: after any
    /// switch, only the incoming source's emitter field is populated.
    #[test]
    fn switching_source_clears_the_previous_emitter_fields() {
        let n = all_sources().len();
        for from_idx in 0..n {
            for to_idx in 0..n {
                let from = all_sources().swap_remove(from_idx);
                let to = all_sources().swap_remove(to_idx);

                let mut emitter = EmitterDef::default();
                sync_emitter(&mut emitter, &from);
                sync_emitter(&mut emitter, &to);

                let populated = [
                    ("image", !emitter.image.is_empty()),
                    ("model", !emitter.model.is_empty()),
                    ("video", !emitter.video.is_empty()),
                ]
                .into_iter()
                .filter(|(_, set)| *set)
                .map(|(name, _)| name)
                .collect::<Vec<_>>();

                let expected: Vec<&str> = match to.kind() {
                    ParticleSourceKind::None => vec![],
                    ParticleSourceKind::Image => vec!["image"],
                    ParticleSourceKind::Model => vec!["model"],
                    // A webcam has no file, so it rides in `video` as "webcam" —
                    // the convention build_particle_system already reads.
                    ParticleSourceKind::Video | ParticleSourceKind::Webcam => vec!["video"],
                };
                assert_eq!(
                    populated,
                    expected,
                    "{:?} -> {:?} left {populated:?} set",
                    from.kind(),
                    to.kind()
                );
            }
        }
    }

    #[test]
    fn emitter_image_is_stored_as_a_bare_file_name() {
        let mut emitter = EmitterDef::default();
        sync_emitter(&mut emitter, &image("/home/kevin/pics/raster_skull.png"));
        assert_eq!(emitter.image, "raster_skull.png");
        // A model keeps its full path — it is resolved against assets/models/ only
        // when bare, and the picker hands over an absolute path.
        sync_emitter(&mut emitter, &model("/home/kevin/models/skull.glb"));
        assert_eq!(emitter.model, "/home/kevin/models/skull.glb");
        assert!(emitter.image.is_empty());
    }

    #[test]
    fn preset_fields_round_trip() {
        for source in all_sources() {
            let fields = source.to_preset_fields();

            // A source writes ONE path field. Without this, saving a model could
            // also stamp `particle_image_path` and put the two-sources-at-once
            // state (#2011) straight back into the preset file — `resolve()`
            // would still pick the model, so nothing else here would notice.
            let paths_written = [
                fields.video_path.is_some(),
                fields.webcam == Some(true),
                fields.image_path.is_some(),
                fields.model_path.is_some(),
            ]
            .iter()
            .filter(|set| **set)
            .count();
            let expected = usize::from(source.kind() != ParticleSourceKind::None);
            assert_eq!(
                paths_written,
                expected,
                "{:?} wrote {paths_written} source fields, expected {expected}",
                source.kind()
            );

            let resolved = fields.resolve();
            match source.kind() {
                ParticleSourceKind::None => assert_eq!(resolved, None),
                ParticleSourceKind::Image => {
                    assert_eq!(
                        resolved,
                        Some(SourceSpec::Image(source.path().unwrap().to_string()))
                    );
                }
                ParticleSourceKind::Model => {
                    assert_eq!(
                        resolved,
                        Some(SourceSpec::Model(source.path().unwrap().to_string()))
                    );
                }
                ParticleSourceKind::Video => {
                    assert_eq!(
                        resolved,
                        Some(SourceSpec::Video(source.path().unwrap().to_string()))
                    );
                }
                ParticleSourceKind::Webcam => assert_eq!(resolved, Some(SourceSpec::Webcam)),
            }
        }
    }

    /// A preset written before the source became one field could name an image
    /// and a model at once — that is the #2011 bug, persisted. It must resolve
    /// the same way the old sequentially-gated restore did: model wins, because
    /// the image block only ran when `particle_model_path` was absent.
    #[test]
    fn preset_with_both_image_and_model_resolves_to_model() {
        let fields = SourcePresetFields {
            image_path: Some("/pics/raster_skull.png".to_string()),
            model_path: Some("/models/skull.glb".to_string()),
            ..Default::default()
        };
        assert_eq!(
            fields.resolve(),
            Some(SourceSpec::Model("/models/skull.glb".to_string()))
        );
    }

    /// Full precedence, including the pairs the old restore never reached because
    /// its blocks ran in sequence rather than as one decision.
    #[test]
    fn preset_precedence_is_video_then_webcam_then_model_then_image() {
        let all = SourcePresetFields {
            video_path: Some("/clips/loop.mp4".to_string()),
            webcam: Some(true),
            model_path: Some("/models/skull.glb".to_string()),
            image_path: Some("/pics/raster_skull.png".to_string()),
            ..Default::default()
        };
        assert_eq!(
            all.resolve(),
            Some(SourceSpec::Video("/clips/loop.mp4".to_string()))
        );

        let no_video = SourcePresetFields {
            video_path: None,
            ..all.clone()
        };
        assert_eq!(no_video.resolve(), Some(SourceSpec::Webcam));

        let no_cam = SourcePresetFields {
            webcam: None,
            ..no_video.clone()
        };
        assert_eq!(
            no_cam.resolve(),
            Some(SourceSpec::Model("/models/skull.glb".to_string()))
        );

        let image_only = SourcePresetFields {
            model_path: None,
            ..no_cam.clone()
        };
        assert_eq!(
            image_only.resolve(),
            Some(SourceSpec::Image("/pics/raster_skull.png".to_string()))
        );

        assert_eq!(SourcePresetFields::default().resolve(), None);
    }

    /// `webcam: Some(false)` is not a webcam. The old restore compared against
    /// `Some(true)` explicitly; keep that, or a preset that recorded "not a
    /// webcam" would restore as one.
    #[test]
    fn webcam_false_is_not_a_webcam_source() {
        let fields = SourcePresetFields {
            webcam: Some(false),
            image_path: Some("/pics/raster_skull.png".to_string()),
            ..Default::default()
        };
        assert_eq!(
            fields.resolve(),
            Some(SourceSpec::Image("/pics/raster_skull.png".to_string()))
        );
    }

    #[test]
    fn no_source_has_no_name_and_no_path() {
        let none = ParticleSource::None;
        assert!(none.path().is_none());
        assert!(none.display_name().is_empty());
        assert!(none.is_static());
        assert!(!none.is_webcam());
        assert!(none.playback().is_none());
        assert_eq!(none.frame_count(), 0);
        assert!(none.current_frame_data().is_none());
        assert!((none.video_position_secs() - 0.0).abs() < 1e-10);
        assert!((none.video_duration_secs() - 0.0).abs() < 1e-10);
        assert_eq!(none.to_preset_fields(), SourcePresetFields::default());
    }

    #[test]
    fn a_still_source_never_advances() {
        for mut source in all_sources() {
            if source.kind() == ParticleSourceKind::Video {
                continue;
            }
            assert!(!source.advance(0.016), "{:?} advanced", source.kind());
        }
    }

    #[cfg(feature = "video")]
    #[test]
    fn video_advances_loops_and_seeks() {
        let frame = || DecodedFrame {
            data: vec![0u8; 4],
            width: 1,
            height: 1,
        };
        let mut src = ParticleSource::Video {
            path: "/clips/loop.mp4".to_string(),
            frames: vec![frame(), frame()],
            delays_ms: vec![100, 100],
            playback: VideoPlayback::default(),
        };
        assert!((src.video_duration_secs() - 0.2).abs() < 1e-9);

        // Under the frame delay: no change.
        assert!(!src.advance(0.05));
        assert_eq!(src.playback().unwrap().current_frame, 0);
        // Past it: next frame.
        assert!(src.advance(0.06));
        assert_eq!(src.playback().unwrap().current_frame, 1);
        // Past the end, looping: back to the start.
        assert!(src.advance(0.11));
        assert_eq!(src.playback().unwrap().current_frame, 0);

        src.seek_to_secs(0.15);
        assert_eq!(src.playback().unwrap().current_frame, 1);
        // Past the end clamps rather than running off.
        src.seek_to_secs(99.0);
        assert_eq!(src.playback().unwrap().current_frame, 1);
    }

    #[cfg(feature = "video")]
    #[test]
    fn video_preset_fields_carry_transport() {
        let src = ParticleSource::Video {
            path: "/clips/loop.mp4".to_string(),
            frames: Vec::new(),
            delays_ms: vec![100],
            playback: VideoPlayback {
                speed: 0.5,
                looping: false,
                ..Default::default()
            },
        };
        let fields = src.to_preset_fields();
        assert_eq!(fields.video_path.as_deref(), Some("/clips/loop.mp4"));
        assert_eq!(fields.video_speed, Some(0.5));
        assert_eq!(fields.video_looping, Some(false));
        assert!(fields.image_path.is_none());
        assert!(fields.model_path.is_none());
    }
}
