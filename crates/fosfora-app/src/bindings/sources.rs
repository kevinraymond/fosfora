use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::audio::features::AudioFeatures;
#[cfg(feature = "desktop")]
use crate::midi::MidiSystem;
#[cfg(feature = "desktop")]
use crate::osc::OscSystem;

use super::types::SourceRaw;

/// A source ID (e.g. "audio.kick"). Borrowed from a static table for every
/// audio source, so filling a frame's snapshot allocates no key strings; only
/// the open-ended MIDI, OSC and WebSocket names are owned (#149).
pub type SourceKey = Cow<'static, str>;

/// A snapshot of all source values for one frame.
/// Key: source ID (e.g. "audio.kick"), Value: (normalized 0-1, raw diagnostics).
///
/// The bus keeps one and refills it in place each frame, so its table is
/// allocated once rather than per frame (#149).
pub type SourceSnapshot = HashMap<SourceKey, (f32, SourceRaw)>;

/// Keys for the indexed audio families, built once. Sized past the largest
/// family each feeds (13 MFCC/delta-MFCC, 12 chroma, 64 mel); an index past the
/// table still works, with an owned key.
static MFCC_KEYS: LazyLock<Vec<String>> = LazyLock::new(|| indexed_keys("audio.mfcc.", 13));
static CHROMA_KEYS: LazyLock<Vec<String>> = LazyLock::new(|| indexed_keys("audio.chroma.", 12));
static MEL_KEYS: LazyLock<Vec<String>> = LazyLock::new(|| indexed_keys("audio.mel.", 128));
static DMFCC_KEYS: LazyLock<Vec<String>> = LazyLock::new(|| indexed_keys("audio.dmfcc.", 13));

fn indexed_keys(prefix: &str, n: usize) -> Vec<String> {
    (0..n).map(|i| format!("{prefix}{i}")).collect()
}

fn indexed_key(table: &'static [String], prefix: &str, i: usize) -> SourceKey {
    table.get(i).map_or_else(
        || Cow::Owned(format!("{prefix}{i}")),
        |k| Cow::Borrowed(k.as_str()),
    )
}

/// Insert one plain-valued source.
fn put(map: &mut SourceSnapshot, key: impl Into<SourceKey>, val: f32) {
    map.insert(key.into(), (val, SourceRaw::Value(val)));
}

/// Circle-of-fifths key hue for the `audio.key_hue` derived source (Chromatica #1477).
///
/// Mirrors `fosfora_key_hue()` in `assets/shaders/lib/palette.wgsl`: recover the integer
/// pitch class from `key_class` (= tonic/11), map it through the circle of fifths so adjacent
/// keys sit a perfect fifth apart, nudge minor keys cooler. The scalar is then eased toward a
/// neutral 0.5 as `key_confidence` falls, so an unsure key doesn't swing every bound layer's
/// color. The base hue is the shared source of truth with the shader helper; the confidence
/// ease is binding-only (a shader reads `u.key_confidence` directly).
fn key_hue(key_class: f32, is_minor: f32, key_confidence: f32) -> f32 {
    let pc = (key_class.clamp(0.0, 1.0) * 11.0).round();
    let hue = ((pc * 7.0) % 12.0 / 12.0 - is_minor * 0.08).rem_euclid(1.0);
    let conf = key_confidence.clamp(0.0, 1.0);
    0.5 + (hue - 0.5) * conf
}

/// Collect audio features into a fresh snapshot. The per-frame path uses
/// [`collect_audio_into`] on the bus's reused one.
pub fn collect_audio(features: &AudioFeatures) -> SourceSnapshot {
    let mut map = HashMap::with_capacity(96);
    collect_audio_into(&mut map, features);
    map
}

/// Collect audio features into `map`.
pub fn collect_audio_into(map: &mut SourceSnapshot, features: &AudioFeatures) {
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
        put(map, key, val);
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
        put(map, key, val);
    }

    // MFCC (13 coefficients)
    for (i, &val) in features.mfcc.iter().enumerate() {
        put(map, indexed_key(&MFCC_KEYS, "audio.mfcc.", i), val);
    }

    // Chroma (12 pitch classes)
    for (i, &val) in features.chroma.iter().enumerate() {
        put(map, indexed_key(&CHROMA_KEYS, "audio.chroma.", i), val);
    }

    // Dominant chroma
    put(map, "audio.dominant_chroma", features.dominant_chroma);

    // Derived: key-locked hue on the circle of fifths (Chromatica #1477). Lets any layer's
    // color param bind to the song's key. Mirrors fosfora_key_hue() in lib/palette.wgsl.
    let kh = key_hue(
        features.key_class,
        features.key_is_minor,
        features.key_confidence,
    );
    put(map, "audio.key_hue", kh);

    // Audio features added by the batched ABI bumps #1505 ("v2") and #1629 ("v3").
    // Every detector below has landed; the "0.0 until each detector lands" note this
    // comment used to carry outlived them by a release and is why these 28 sat here,
    // collected every frame, listed in neither picker, until #1896.
    let reserved = [
        // v2 (#1505)
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
        // v3 (#1629)
        ("audio.percussive_energy", features.percussive_energy),
        ("audio.harmonic_energy", features.harmonic_energy),
        ("audio.harmonic_ratio", features.harmonic_ratio),
        ("audio.pitch", features.pitch),
        ("audio.pitch_confidence", features.pitch_confidence),
        ("audio.contrast_0", features.contrast_0),
        ("audio.contrast_1", features.contrast_1),
        ("audio.contrast_2", features.contrast_2),
        ("audio.contrast_3", features.contrast_3),
        ("audio.contrast_4", features.contrast_4),
        ("audio.contrast_5", features.contrast_5),
        ("audio.contrast_mean", features.contrast_mean),
        ("audio.timbre_flux", features.timbre_flux),
        // A13b per-band pan (#1801). These shipped with uniform fields and picker entries but
        // no collector, so all seven read a permanent 0.0 in the binding matrix — bind one and
        // it never moves. Caught by `every_picker_audio_source_is_actually_collected`.
        ("audio.band_pan_sub_bass", features.band_pan_sub_bass),
        ("audio.band_pan_bass", features.band_pan_bass),
        ("audio.band_pan_low_mid", features.band_pan_low_mid),
        ("audio.band_pan_mid", features.band_pan_mid),
        ("audio.band_pan_upper_mid", features.band_pan_upper_mid),
        ("audio.band_pan_presence", features.band_pan_presence),
        ("audio.band_pan_brilliance", features.band_pan_brilliance),
        // Tempo trust (#81).
        ("audio.tempo_confidence", features.tempo_confidence),
        ("audio.beat_locked", features.beat_locked),
    ];
    for (key, val) in reserved {
        put(map, key, val);
    }
}

/// Collect mel-spectrogram bands into source snapshot as `audio.mel.N` (A1b, #1512).
///
/// `mel` is the newest A17 mel column ([`crate::audio::AudioSystem::latest_mel`]), already
/// dB-normalized to 0..1 per band — so the values drop straight in with no schema/normalizer
/// involvement (mel bands are not part of the `AudioFeatures` ABI). Empty slice yields no
/// sources.
pub fn collect_mel_bands(mel: &[f32]) -> SourceSnapshot {
    let mut map = HashMap::with_capacity(mel.len());
    collect_mel_bands_into(&mut map, mel);
    map
}

/// [`collect_mel_bands`] into an existing snapshot.
pub fn collect_mel_bands_into(map: &mut SourceSnapshot, mel: &[f32]) {
    for (i, &val) in mel.iter().enumerate() {
        put(map, indexed_key(&MEL_KEYS, "audio.mel.", i), val);
    }
}

/// Collect delta-MFCC slopes into source snapshot as `audio.dmfcc.N` (A16 #1467).
///
/// `dmfcc` is the newest [`crate::audio::AudioSystem::latest_dmfcc`] — raw bipolar per-coefficient
/// slopes, not part of the `AudioFeatures` ABI (bindings-only, to save the uniform budget), so the
/// values drop straight in with no schema/normalizer involvement (the binding graph range-maps).
pub fn collect_dmfcc_bands(dmfcc: &[f32; 13]) -> SourceSnapshot {
    let mut map = HashMap::with_capacity(dmfcc.len());
    collect_dmfcc_bands_into(&mut map, dmfcc);
    map
}

/// [`collect_dmfcc_bands`] into an existing snapshot.
pub fn collect_dmfcc_bands_into(map: &mut SourceSnapshot, dmfcc: &[f32; 13]) {
    for (i, &val) in dmfcc.iter().enumerate() {
        put(map, indexed_key(&DMFCC_KEYS, "audio.dmfcc.", i), val);
    }
}

/// Collect MIDI CC values into source snapshot.
#[cfg(feature = "desktop")]
pub fn collect_midi_into(map: &mut SourceSnapshot, midi: &MidiSystem) {
    for (&(cc, channel), &(raw_value, ref device_name)) in &midi.last_cc_values {
        // Sanitize device name for use as key segment
        let device = sanitize_device_name(device_name);
        let key = format!("midi.{device}.cc.{channel}.{cc}");
        let normalized = f32::from(raw_value) / 127.0;
        map.insert(key.into(), (normalized, SourceRaw::MidiCc(raw_value)));
    }
}

/// Collect OSC values into source snapshot.
#[cfg(feature = "desktop")]
pub fn collect_osc_into(map: &mut SourceSnapshot, osc: &OscSystem) {
    for (address, &value) in &osc.last_raw_values {
        put(map, format!("osc.{address}"), value);
    }
}

/// Collect WebSocket binding values into source snapshot.
pub fn collect_websocket_into(map: &mut SourceSnapshot, ws_values: &HashMap<String, f32>) {
    for (key, &value) in ws_values {
        put(map, format!("ws.{key}"), value);
    }
}

/// Sanitize device name: replace spaces and dots with underscores.
#[cfg(feature = "desktop")]
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
        // 7 bands + 13 scalars + 13 mfcc + 12 chroma + 1 dominant + 1 key_hue + 35 reserved
        // (28 from the v2/v3 tails + the 7 A13b band pans that were listed but uncollected)
        // + 2 tempo-trust slots (#81)
        assert_eq!(snap.len(), 84);
        assert!(snap.contains_key("audio.tempo_confidence"));
        assert!(snap.contains_key("audio.beat_locked"));
        assert!(snap.contains_key("audio.band_pan_sub_bass"));
        assert!(snap.contains_key("audio.band_pan_brilliance"));
        assert!(snap.contains_key("audio.kick"));
        assert!(snap.contains_key("audio.band.0"));
        assert!(snap.contains_key("audio.mfcc.12"));
        assert!(snap.contains_key("audio.chroma.11"));
        assert!(snap.contains_key("audio.dominant_chroma"));
        // Derived circle-of-fifths key hue (Chromatica #1477).
        assert!(snap.contains_key("audio.key_hue"));
        // Reserved v2 tail (#1505)
        assert!(snap.contains_key("audio.loudness_m"));
        assert!(snap.contains_key("audio.downbeat"));
        assert!(snap.contains_key("audio.bar_phase"));
        assert!(snap.contains_key("audio.drop"));
        // Reserved v3 tail (#1629)
        assert!(snap.contains_key("audio.percussive_energy"));
        assert!(snap.contains_key("audio.harmonic_ratio"));
        assert!(snap.contains_key("audio.pitch"));
        assert!(snap.contains_key("audio.contrast_0"));
        assert!(snap.contains_key("audio.timbre_flux"));
    }

    #[test]
    fn test_key_hue_circle_of_fifths() {
        // C major (key_class 0) at full confidence -> hue 0.0.
        assert!((key_hue(0.0, 0.0, 1.0) - 0.0).abs() < 1e-6);
        // G (tonic 7 -> key_class 7/11) is a fifth up: one step around the wheel = 1/12.
        assert!((key_hue(7.0 / 11.0, 0.0, 1.0) - 1.0 / 12.0).abs() < 1e-5);
        // Unsure key eases toward the neutral 0.5 so bound layers don't swing.
        assert!((key_hue(7.0 / 11.0, 0.0, 0.0) - 0.5).abs() < 1e-6);
        // Output always stays in 0..1.
        for pc in 0..12u32 {
            let h = key_hue(pc as f32 / 11.0, 1.0, 1.0);
            assert!((0.0..=1.0).contains(&h), "hue {h} out of range for pc {pc}");
        }
    }

    #[test]
    fn test_collect_mel_bands() {
        // 64-band A17 column -> audio.mel.0..63 (A1b, #1512).
        let mel = [0.0f32; 64];
        let mut snap = SourceSnapshot::new();
        collect_mel_bands_into(&mut snap, &mel);
        assert_eq!(snap.len(), 64);
        assert!(snap.contains_key("audio.mel.0"));
        assert!(snap.contains_key("audio.mel.63"));
        assert!(!snap.contains_key("audio.mel.64"));
        // Empty column yields no sources (before the first audio frame arrives).
        let mut empty = SourceSnapshot::new();
        collect_mel_bands_into(&mut empty, &[]);
        assert!(empty.is_empty());
    }

    #[test]
    fn test_collect_dmfcc_bands() {
        // 13 delta-MFCC slopes -> audio.dmfcc.0..12 (A16, #1467), bindings-only.
        let dmfcc = [0.0f32; 13];
        let mut snap = SourceSnapshot::new();
        collect_dmfcc_bands_into(&mut snap, &dmfcc);
        assert_eq!(snap.len(), 13);
        assert!(snap.contains_key("audio.dmfcc.0"));
        assert!(snap.contains_key("audio.dmfcc.12"));
        assert!(!snap.contains_key("audio.dmfcc.13"));
    }

    #[test]
    #[cfg(feature = "desktop")]
    fn test_sanitize_device_name() {
        assert_eq!(sanitize_device_name("MPD218"), "MPD218");
        assert_eq!(sanitize_device_name("My Device 2.0"), "My_Device_2_0");
    }

    #[test]
    fn test_collect_websocket() {
        let mut ws = HashMap::new();
        ws.insert("mediapipe.left_thumb_x".to_string(), 0.5);
        let mut snap = SourceSnapshot::new();
        collect_websocket_into(&mut snap, &ws);
        assert_eq!(snap.len(), 1);
        let (val, raw) = snap.get("ws.mediapipe.left_thumb_x").unwrap();
        assert!((*val - 0.5).abs() < 1e-5);
        assert!((raw.numeric() - 0.5).abs() < 1e-5);
    }

    /// Every audio-derived key is borrowed from a static table, so a frame's
    /// snapshot allocates no key strings for them (#149).
    #[test]
    fn audio_keys_are_borrowed_not_allocated() {
        let mut snap = SourceSnapshot::new();
        collect_audio_into(&mut snap, &AudioFeatures::default());
        collect_mel_bands_into(&mut snap, &[0.0; 64]);
        collect_dmfcc_bands_into(&mut snap, &[0.0; 13]);
        assert_eq!(snap.len(), 84 + 64 + 13);
        for key in snap.keys() {
            assert!(matches!(key, Cow::Borrowed(_)), "{key} was allocated");
        }
        // Refilling keeps the table: clear() retains capacity, and the same
        // keys land in the same slots.
        let cap = snap.capacity();
        snap.clear();
        collect_audio_into(&mut snap, &AudioFeatures::default());
        assert_eq!(snap.capacity(), cap);
    }

    #[test]
    fn raw_values_format_on_demand() {
        assert_eq!(SourceRaw::Value(0.25).to_string(), "0.250");
        assert_eq!(SourceRaw::MidiCc(64).to_string(), "CC 64/127");
        assert!((SourceRaw::MidiCc(127).numeric() - 127.0).abs() < 1e-9);
    }
}
