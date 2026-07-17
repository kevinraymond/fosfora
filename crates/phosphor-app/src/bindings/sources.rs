use std::collections::HashMap;

use crate::audio::features::AudioFeatures;
use crate::midi::MidiSystem;
use crate::osc::OscSystem;

use super::types::SourceRaw;

/// A snapshot of all source values for one frame.
/// Key: source ID (e.g. "audio.kick"), Value: (normalized 0-1, raw diagnostics).
pub type SourceSnapshot = HashMap<String, (f32, SourceRaw)>;

/// Collect audio features into source snapshot.
pub fn collect_audio(features: &AudioFeatures) -> SourceSnapshot {
    let mut map = HashMap::with_capacity(61);

    let raw = |v: f32| SourceRaw {
        display: format!("{:.3}", v),
        numeric: v as f64,
    };

    // Frequency bands (indices 0-6)
    let bands = [
        ("audio.band.0", features.sub_bass),
        ("audio.band.1", features.bass),
        ("audio.band.2", features.low_mid),
        ("audio.band.3", features.mid),
        ("audio.band.4", features.upper_mid),
        ("audio.band.5", features.presence),
        ("audio.band.6", features.brilliance),
    ];
    for (key, val) in bands {
        map.insert(key.to_string(), (val, raw(val)));
    }

    // Aggregates + spectral + beat
    let scalars = [
        ("audio.rms", features.rms),
        ("audio.kick", features.kick),
        ("audio.onset", features.onset),
        ("audio.beat", features.beat),
        ("audio.beat_phase", features.beat_phase),
        ("audio.bpm", features.bpm),
        ("audio.beat_strength", features.beat_strength),
        ("audio.centroid", features.centroid),
        ("audio.flux", features.flux),
        ("audio.flatness", features.flatness),
        ("audio.rolloff", features.rolloff),
        ("audio.bandwidth", features.bandwidth),
        ("audio.zcr", features.zcr),
    ];
    for (key, val) in scalars {
        map.insert(key.to_string(), (val, raw(val)));
    }

    // MFCC (13 coefficients)
    for (i, &val) in features.mfcc.iter().enumerate() {
        map.insert(format!("audio.mfcc.{i}"), (val, raw(val)));
    }

    // Chroma (12 pitch classes)
    for (i, &val) in features.chroma.iter().enumerate() {
        map.insert(format!("audio.chroma.{i}"), (val, raw(val)));
    }

    // Dominant chroma
    map.insert(
        "audio.dominant_chroma".to_string(),
        (features.dominant_chroma, raw(features.dominant_chroma)),
    );

    // Reserved audio features (batched ABI bump #1505) — 0.0 until each detector
    // lands, but exposed as sources now so bindings can target them ahead of time.
    let reserved = [
        ("audio.loudness_m", features.loudness_m),
        ("audio.loudness_s", features.loudness_s),
        ("audio.loudness_trend", features.loudness_trend),
        ("audio.key_class", features.key_class),
        ("audio.key_is_minor", features.key_is_minor),
        ("audio.key_confidence", features.key_confidence),
        ("audio.downbeat", features.downbeat),
        ("audio.bar_phase", features.bar_phase),
        ("audio.beat_in_bar", features.beat_in_bar),
        ("audio.pan", features.pan),
        ("audio.stereo_width", features.stereo_width),
        ("audio.stereo_corr", features.stereo_corr),
        ("audio.section_novelty", features.section_novelty),
        ("audio.buildup", features.buildup),
        ("audio.drop", features.drop),
    ];
    for (key, val) in reserved {
        map.insert(key.to_string(), (val, raw(val)));
    }

    map
}

/// Collect mel-spectrogram bands into source snapshot as `audio.mel.N` (A1b, #1512).
///
/// `mel` is the newest A17 mel column ([`crate::audio::AudioSystem::latest_mel`]), already
/// dB-normalized to 0..1 per band — so the values drop straight in with no schema/normalizer
/// involvement (mel bands are not part of the `AudioFeatures` ABI). Empty slice yields no
/// sources.
pub fn collect_mel_bands(mel: &[f32]) -> SourceSnapshot {
    let raw = |v: f32| SourceRaw {
        display: format!("{:.3}", v),
        numeric: v as f64,
    };
    let mut map = HashMap::with_capacity(mel.len());
    for (i, &val) in mel.iter().enumerate() {
        map.insert(format!("audio.mel.{i}"), (val, raw(val)));
    }
    map
}

/// Collect MIDI CC values into source snapshot.
pub fn collect_midi(midi: &MidiSystem) -> SourceSnapshot {
    let mut map = HashMap::with_capacity(midi.last_cc_values.len());

    for (&(cc, channel), &(raw_value, ref device_name)) in &midi.last_cc_values {
        // Sanitize device name for use as key segment
        let device = sanitize_device_name(device_name);
        let key = format!("midi.{device}.cc.{channel}.{cc}");
        let normalized = raw_value as f32 / 127.0;
        map.insert(
            key,
            (
                normalized,
                SourceRaw {
                    display: format!("CC {raw_value}/127"),
                    numeric: raw_value as f64,
                },
            ),
        );
    }

    map
}

/// Collect OSC values into source snapshot.
pub fn collect_osc(osc: &OscSystem) -> SourceSnapshot {
    let mut map = HashMap::with_capacity(osc.last_raw_values.len());

    for (address, &value) in &osc.last_raw_values {
        let key = format!("osc.{address}");
        map.insert(
            key,
            (
                value,
                SourceRaw {
                    display: format!("{:.3}", value),
                    numeric: value as f64,
                },
            ),
        );
    }

    map
}

/// Collect WebSocket binding values into source snapshot.
pub fn collect_websocket(ws_values: &HashMap<String, f32>) -> SourceSnapshot {
    let mut map = HashMap::with_capacity(ws_values.len());

    for (key, &value) in ws_values {
        let source_key = format!("ws.{key}");
        map.insert(
            source_key,
            (
                value,
                SourceRaw {
                    display: format!("{:.3}", value),
                    numeric: value as f64,
                },
            ),
        );
    }

    map
}

/// Sanitize device name: replace spaces and dots with underscores.
fn sanitize_device_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_audio() {
        let features = AudioFeatures::default();
        let snap = collect_audio(&features);
        // 7 bands + 13 scalars + 13 mfcc + 12 chroma + 1 dominant + 15 reserved = 61
        assert_eq!(snap.len(), 61);
        assert!(snap.contains_key("audio.kick"));
        assert!(snap.contains_key("audio.band.0"));
        assert!(snap.contains_key("audio.mfcc.12"));
        assert!(snap.contains_key("audio.chroma.11"));
        assert!(snap.contains_key("audio.dominant_chroma"));
        // Reserved tail (#1505)
        assert!(snap.contains_key("audio.loudness_m"));
        assert!(snap.contains_key("audio.downbeat"));
        assert!(snap.contains_key("audio.bar_phase"));
        assert!(snap.contains_key("audio.drop"));
    }

    #[test]
    fn test_collect_mel_bands() {
        // 64-band A17 column -> audio.mel.0..63 (A1b, #1512).
        let mel = [0.0f32; 64];
        let snap = collect_mel_bands(&mel);
        assert_eq!(snap.len(), 64);
        assert!(snap.contains_key("audio.mel.0"));
        assert!(snap.contains_key("audio.mel.63"));
        assert!(!snap.contains_key("audio.mel.64"));
        // Empty column yields no sources (before the first audio frame arrives).
        assert!(collect_mel_bands(&[]).is_empty());
    }

    #[test]
    fn test_sanitize_device_name() {
        assert_eq!(sanitize_device_name("MPD218"), "MPD218");
        assert_eq!(sanitize_device_name("My Device 2.0"), "My_Device_2_0");
    }

    #[test]
    fn test_collect_websocket() {
        let mut ws = HashMap::new();
        ws.insert("mediapipe.left_thumb_x".to_string(), 0.5);
        let snap = collect_websocket(&ws);
        assert_eq!(snap.len(), 1);
        let (val, raw) = snap.get("ws.mediapipe.left_thumb_x").unwrap();
        assert!((*val - 0.5).abs() < 1e-5);
        assert!((raw.numeric - 0.5).abs() < 1e-5);
    }
}
