//! The `/fosfora/v1/` address table — Signal's wire contract.
//!
//! These strings are the product: external rigs patch against them, so `/v1/` is
//! frozen. Additive changes (new addresses) are allowed; changing an existing
//! address's type or semantics requires a side-by-side `/fosfora/v2/`. The full
//! contract, including emission semantics and units, lives in `docs/SIGNAL.md`.

use crate::audio::schema::FEATURES;

pub const PREFIX: &str = "/fosfora/v1";

// Events (emitted per hop, the moment they fire).
pub const BEAT: &str = "/fosfora/v1/beat"; // i running beat count
pub const DOWNBEAT: &str = "/fosfora/v1/downbeat"; // i running bar count
pub const DROP: &str = "/fosfora/v1/drop"; // i running drop count
pub const ONSET: &str = "/fosfora/v1/onset"; // f strength
// Q4 (#2080): f confidence 0..1 + f age in seconds. Additive, so still v1. Fires on a
// confirmed novelty peak whether or not the /section LABEL changed — a chorus following a
// verse is a boundary even when both read "steady". The age is the detector's own fixed
// latency (kernel centring + peak confirmation), not an estimate: subtract it to place the
// boundary in musical time.
pub const SECTION_BOUNDARY: &str = "/fosfora/v1/section/boundary";
pub const STEM_DRUMS_ONSET: &str = "/fosfora/v1/stem/drums/onset"; // f kick-band strength

// Continuous (decimated to the configured TX rate).
pub const BPM: &str = "/fosfora/v1/bpm"; // f real BPM
pub const BAR_PHASE: &str = "/fosfora/v1/bar_phase"; // f 0..1
pub const BUILD: &str = "/fosfora/v1/build"; // f 0..1
pub const ENERGY: &str = "/fosfora/v1/energy"; // f 0..1 (short-term loudness)
pub const STEM_DRUMS_ENERGY: &str = "/fosfora/v1/stem/drums/energy"; // f 0..1 (HPSS proxy)
pub const STEM_BASS_ENERGY: &str = "/fosfora/v1/stem/bass/energy"; // f 0..1 (band proxy)
pub const STEM_MELODY_ENERGY: &str = "/fosfora/v1/stem/melody/energy"; // f 0..1 (HPSS proxy)
pub const PHRASE_BAR: &str = "/fosfora/v1/phrase/bar"; // i bar within phrase, 1-based
pub const PHRASE_BEATS_LEFT: &str = "/fosfora/v1/phrase/beats_left"; // i to next boundary
pub const PREDICT_DROP: &str = "/fosfora/v1/predict/drop"; // f confidence 0..1

// On change (plus a 1 Hz re-broadcast for late-joining receivers).
pub const KEY: &str = "/fosfora/v1/key"; // s "Am"/"F#" + f confidence
pub const SECTION: &str = "/fosfora/v1/section"; // s label + f confidence
pub const PHRASE_LEN: &str = "/fosfora/v1/phrase/len"; // i 8|16|32 + f confidence

// Status (1 Hz). Split scalars, not JSON: OSC-native consumers can't parse args.
pub const STATUS_ONLINE: &str = "/fosfora/v1/status/online"; // i 1 (0 on clean shutdown)
pub const STATUS_UPTIME: &str = "/fosfora/v1/status/uptime"; // f seconds
pub const STATUS_DEVICE: &str = "/fosfora/v1/status/device"; // s
pub const STATUS_HOP_HZ: &str = "/fosfora/v1/status/hop_hz"; // f
pub const STATUS_TIER: &str = "/fosfora/v1/status/tier"; // s e.g. "heuristic-v1"

// Link session telemetry (cargo feature `link`; live loop only — never in
// offline dumps, which must stay deterministic). On change + 1 Hz status tick.
#[cfg(feature = "link")]
pub const LINK_ENABLED: &str = "/fosfora/v1/link/enabled"; // i 0|1
#[cfg(feature = "link")]
pub const LINK_PEERS: &str = "/fosfora/v1/link/peers"; // i
#[cfg(feature = "link")]
pub const LINK_TEMPO: &str = "/fosfora/v1/link/tempo"; // f session BPM
#[cfg(feature = "link")]
pub const LINK_PLAYING: &str = "/fosfora/v1/link/playing"; // i 0|1 (start/stop sync)

// Reserved, documented, not emitted in v1: /fosfora/v1/chord,
// /fosfora/v1/stem/bass/onset, /fosfora/v1/stem/melody/onset. Further
// /fosfora/v1/link/* additions stay additive-only.

/// The opt-in raw feature bus: one address per `AudioFeatures` slot, named from
/// the canonical table in `audio::schema` — zero mapping logic, stable by
/// construction. Values are the normalized 0..1 slots verbatim.
pub fn feat_addresses() -> Vec<String> {
    FEATURES
        .iter()
        .map(|d| format!("{PREFIX}/feat/{}", d.name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::features::NUM_FEATURES;

    /// Every address is part of the frozen wire contract — a typo or rename here is
    /// a silent breaking change for every patched rig, so pin the exact strings.
    #[test]
    fn v1_addresses_are_pinned() {
        let pinned: [(&str, &str); 22] = [
            (BEAT, "/fosfora/v1/beat"),
            (DOWNBEAT, "/fosfora/v1/downbeat"),
            (DROP, "/fosfora/v1/drop"),
            (ONSET, "/fosfora/v1/onset"),
            (STEM_DRUMS_ONSET, "/fosfora/v1/stem/drums/onset"),
            (BPM, "/fosfora/v1/bpm"),
            (BAR_PHASE, "/fosfora/v1/bar_phase"),
            (BUILD, "/fosfora/v1/build"),
            (ENERGY, "/fosfora/v1/energy"),
            (STEM_DRUMS_ENERGY, "/fosfora/v1/stem/drums/energy"),
            (STEM_BASS_ENERGY, "/fosfora/v1/stem/bass/energy"),
            (STEM_MELODY_ENERGY, "/fosfora/v1/stem/melody/energy"),
            (PHRASE_BAR, "/fosfora/v1/phrase/bar"),
            (PHRASE_BEATS_LEFT, "/fosfora/v1/phrase/beats_left"),
            (PREDICT_DROP, "/fosfora/v1/predict/drop"),
            (KEY, "/fosfora/v1/key"),
            (SECTION, "/fosfora/v1/section"),
            (SECTION_BOUNDARY, "/fosfora/v1/section/boundary"),
            (PHRASE_LEN, "/fosfora/v1/phrase/len"),
            (STATUS_ONLINE, "/fosfora/v1/status/online"),
            (STATUS_UPTIME, "/fosfora/v1/status/uptime"),
            (STATUS_TIER, "/fosfora/v1/status/tier"),
        ];
        for (actual, want) in pinned {
            assert_eq!(actual, want);
        }
        assert_eq!(STATUS_DEVICE, "/fosfora/v1/status/device");
        assert_eq!(STATUS_HOP_HZ, "/fosfora/v1/status/hop_hz");
        #[cfg(feature = "link")]
        {
            assert_eq!(LINK_ENABLED, "/fosfora/v1/link/enabled");
            assert_eq!(LINK_PEERS, "/fosfora/v1/link/peers");
            assert_eq!(LINK_TEMPO, "/fosfora/v1/link/tempo");
            assert_eq!(LINK_PLAYING, "/fosfora/v1/link/playing");
        }
    }

    /// The feat bus derives from the canonical feature table; its size and shape
    /// follow the ABI, and no name may produce an illegal OSC address.
    #[test]
    fn feat_bus_covers_every_slot_with_legal_addresses() {
        let addrs = feat_addresses();
        assert_eq!(addrs.len(), NUM_FEATURES);
        for a in &addrs {
            assert!(a.starts_with("/fosfora/v1/feat/"), "{a}");
            // OSC address-pattern specials and whitespace are illegal in a literal address.
            for bad in [' ', '#', '*', ',', '?', '[', ']', '{', '}'] {
                assert!(!a.contains(bad), "{a} contains {bad:?}");
            }
        }
        assert!(addrs.contains(&"/fosfora/v1/feat/sub_bass".to_string()));
        assert!(addrs.contains(&"/fosfora/v1/feat/mfcc.0".to_string()));
        assert!(addrs.contains(&"/fosfora/v1/feat/beat_index".to_string()));
    }
}
