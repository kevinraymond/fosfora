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
    let mut map = HashMap::with_capacity(46);

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

    map
}

/// Collect MIDI CC values into source snapshot.
pub fn collect_midi(midi: &MidiSystem) -> SourceSnapshot {
    let mut map = SourceSnapshot::new();

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
    let mut map = SourceSnapshot::new();

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
    let mut map = SourceSnapshot::new();

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
        .map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_audio() {
        let features = AudioFeatures::default();
        let snap = collect_audio(&features);
        // 7 bands + 13 scalars + 13 mfcc + 12 chroma + 1 dominant = 46
        assert_eq!(snap.len(), 46);
        assert!(snap.contains_key("audio.kick"));
        assert!(snap.contains_key("audio.band.0"));
        assert!(snap.contains_key("audio.mfcc.12"));
        assert!(snap.contains_key("audio.chroma.11"));
        assert!(snap.contains_key("audio.dominant_chroma"));
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
