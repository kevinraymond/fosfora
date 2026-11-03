//! Tempo prior configuration, presets, and the manual octave/tap/sync-offset controls.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// A7 (#1458): tempo prior configuration + manual octave/tap overrides
// ---------------------------------------------------------------------------

/// Lowest/highest BPM the tempo estimator will report. Manual octave shifts and tap
/// tempo are rejected when they would land outside this window.
pub const BPM_MIN: f64 = 40.0;
pub const BPM_MAX: f64 = 300.0;

/// Auto-prior adaptation (A7 #1458). The rate is per tempo update — the estimator runs
/// one every 6 frames (~14 Hz), so 0.001 is a ~70s time constant: slow enough that a
/// transient mis-lock can't drag the prior with it. Bounds are tighter than
/// `BPM_MIN`/`BPM_MAX`: the *prior centre* has no business out at 40 or 300.
pub(super) const AUTO_PRIOR_RATE: f64 = 0.001;
pub(super) const AUTO_PRIOR_MIN_CONFIDENCE: f64 = 0.5;
pub(super) const AUTO_PRIOR_MIN_BPM: f64 = 60.0;
pub(super) const AUTO_PRIOR_MAX_BPM: f64 = 200.0;

/// Bounds on the prior width, in octaves. Zero would make the prior a delta function that
/// rejects every candidate; the upper bound is already effectively "no opinion".
pub(super) const MIN_PRIOR_SIGMA: f64 = 0.05;
pub(super) const MAX_PRIOR_SIGMA: f64 = 4.0;

/// Prior centre in log2 space, clamped to the range the estimator can actually report.
pub(super) fn prior_center_log2(bpm: f32) -> f64 {
    (bpm as f64).clamp(BPM_MIN, BPM_MAX).log2()
}

/// User-tunable tempo prior (A7 #1458). The estimator scores metrical-ratio candidates
/// with a log-Gaussian centred on `prior_center_bpm`, so this is what decides whether a
/// 172 BPM DnB track reads as 172 or folds to 86.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TempoConfig {
    /// Centre of the log-Gaussian tempo prior, in BPM.
    pub prior_center_bpm: f32,
    /// Prior width in octaves (log2 BPM). Small = a strong opinion about the octave.
    pub prior_sigma: f32,
    /// When set, the estimator slowly walks `prior_center_bpm` toward the tempo it is
    /// actually locking onto. The audio thread publishes the adapted value back into the
    /// shared config, so the UI reads it live and it freezes in place when auto is off.
    pub auto_prior: bool,
}

impl Default for TempoConfig {
    fn default() -> Self {
        // The pre-A7 hardcoded values — upgrading users get identical detection until
        // they pick a preset. 150 is deliberate, not a stray: benched against 120, 130
        // and 120/σ0.75 (#78), every lower center halved fast EDM (GiantSteps Acc1
        // 69.9% → 53.6–60.1%) for at most +2.5 points on Ballroom. Slow material is
        // what the genre presets are for.
        Self {
            prior_center_bpm: 150.0,
            prior_sigma: 1.0,
            auto_prior: false,
        }
    }
}

/// Genre presets for the tempo prior (A7 #1458).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempoPreset {
    Neutral,
    Wide,
    House,
    DrumAndBass,
    HipHop,
    Ambient,
}

impl TempoPreset {
    pub const ALL: &[TempoPreset] = &[
        TempoPreset::Neutral,
        TempoPreset::Wide,
        TempoPreset::House,
        TempoPreset::DrumAndBass,
        TempoPreset::HipHop,
        TempoPreset::Ambient,
    ];

    /// (centre BPM, sigma in octaves).
    pub fn values(self) -> (f32, f32) {
        match self {
            Self::Neutral => (150.0, 1.0),
            Self::Wide => (140.0, 1.2),
            Self::House => (127.0, 0.35),
            Self::DrumAndBass => (172.0, 0.3),
            Self::HipHop => (90.0, 0.4),
            Self::Ambient => (70.0, 0.6),
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Neutral => "Neutral \u{00b7} 150",
            Self::Wide => "Wide \u{00b7} 140",
            Self::House => "House \u{00b7} 127",
            Self::DrumAndBass => "Drum & Bass \u{00b7} 172",
            Self::HipHop => "Hip-hop \u{00b7} 90",
            Self::Ambient => "Ambient \u{00b7} 70",
        }
    }

    /// The preset matching this config exactly, or `None` when the user has hand-tuned
    /// the sliders. Keeps the config the single source of truth — no preset field to
    /// drift out of sync with the values it names.
    #[expect(
        clippy::float_cmp,
        reason = "presets write their values verbatim; any other value is hand-tuned"
    )]
    pub fn from_config(cfg: &TempoConfig) -> Option<TempoPreset> {
        Self::ALL.iter().copied().find(|p| {
            let (c, s) = p.values();
            cfg.prior_center_bpm == c && cfg.prior_sigma == s
        })
    }
}

/// One-shot tempo override from the UI / MIDI / OSC (A7 #1458). Queued in
/// [`TempoControl::pending`] and drained by the audio thread each hop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TempoCommand {
    /// Force the reported tempo up (+1) or down (-1) an octave.
    ShiftOctave(i32),
    /// Lock onto a tapped tempo, in BPM (averaged UI-side — see `audio_panel`).
    Tap(f64),
}

/// Range of the user A/V sync offset, ms (#80). Positive fires the beat grid early; negative
/// holds the whole analysis output back.
pub const SYNC_OFFSET_MIN_MS: f32 = -250.0;
pub const SYNC_OFFSET_MAX_MS: f32 = 200.0;

/// The furthest ahead of its grid instant a locked beat is emitted, as a fraction of the
/// period (#80). The scheduler only looks at instant N+1 once instant N has settled, so a
/// lead approaching a whole period would emit late; under half a period also keeps every
/// emission clear of the `lead_emitted` half-period match.
pub(super) const MAX_LEAD_PERIOD_FRAC: f64 = 0.45;

/// Shared tempo state: live config plus a small command mailbox, both behind one mutex
/// the audio thread locks once per hop (the #1510 pattern, extended with the mailbox the
/// A7 note called for). Cloned into the audio thread and threaded through `switch_device`,
/// so user tuning survives a device change.
#[derive(Debug, Default)]
pub struct TempoControl {
    pub config: TempoConfig,
    pending: Vec<TempoCommand>,
    /// #80: the user A/V sync offset, ms. Kept beside `config` rather than in it: it is a
    /// property of the rig, not of the tempo prior, and Ableton Link swaps `config` out and
    /// back while it follows a session.
    sync_offset_ms: f32,
}

impl TempoControl {
    pub fn new(config: TempoConfig) -> Self {
        Self {
            config,
            pending: Vec::new(),
            sync_offset_ms: 0.0,
        }
    }

    /// [`Self::new`] with the saved A/V sync offset (#80).
    pub fn with_sync_offset(config: TempoConfig, sync_offset_ms: f32) -> Self {
        let mut ctl = Self::new(config);
        ctl.set_sync_offset_ms(sync_offset_ms);
        ctl
    }

    /// Set the A/V sync offset (#80), clamped to
    /// [`SYNC_OFFSET_MIN_MS`]..[`SYNC_OFFSET_MAX_MS`]; a non-finite value reads as 0.
    pub fn set_sync_offset_ms(&mut self, ms: f32) {
        self.sync_offset_ms = if ms.is_finite() {
            ms.clamp(SYNC_OFFSET_MIN_MS, SYNC_OFFSET_MAX_MS)
        } else {
            0.0
        };
    }

    pub fn sync_offset_ms(&self) -> f32 {
        self.sync_offset_ms
    }

    /// Queue a command for the audio thread. Bounded so a stalled/absent audio thread
    /// can't grow this without limit — dropping the oldest keeps the newest intent.
    pub fn push(&mut self, cmd: TempoCommand) {
        const MAX_PENDING: usize = 16;
        if self.pending.len() >= MAX_PENDING {
            self.pending.remove(0);
        }
        self.pending.push(cmd);
    }

    pub fn drain(&mut self) -> Vec<TempoCommand> {
        std::mem::take(&mut self.pending)
    }
}
