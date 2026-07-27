use std::net::UdpSocket;

use rosc::{OscMessage, OscPacket, OscType};

use crate::audio::PulseCounts;
use crate::audio::features::AudioFeatures;

/// Addresses for the pulse totals, in the order [`OscSender::send_pulse_counts`] emits them
/// and matching the field order of [`PulseCounts`].
///
/// Flat `_count` suffixes rather than `/beat/count`: `/phosphor/audio/beat` is already a leaf,
/// and a node that is both leaf and container trips OSC tools that build an address tree.
const PULSE_COUNT_ADDRS: [&str; 3] = [
    "/phosphor/audio/beat_count",
    "/phosphor/audio/downbeat_count",
    "/phosphor/audio/drop_count",
];

/// Fire-and-forget OSC sender over UDP.
pub struct OscSender {
    socket: Option<UdpSocket>,
    target: String,
}

impl OscSender {
    pub fn new() -> Self {
        Self {
            socket: None,
            target: String::new(),
        }
    }

    /// Configure the sender to target host:port. Binds an ephemeral local port.
    pub fn configure(&mut self, host: &str, port: u16) {
        self.target = format!("{host}:{port}");
        match UdpSocket::bind("0.0.0.0:0") {
            Ok(sock) => {
                let _ = sock.set_nonblocking(true);
                self.socket = Some(sock);
                log::info!("OSC sender configured: target {}", self.target);
            }
            Err(e) => {
                log::error!("Failed to bind OSC sender socket: {e}");
                self.socket = None;
            }
        }
    }

    /// Disable the sender.
    #[allow(dead_code)]
    pub fn disable(&mut self) {
        self.socket = None;
    }

    /// Send all audio features as OSC messages.
    pub fn send_audio(&self, f: &AudioFeatures) {
        // 7 bands
        self.send_float("/phosphor/audio/bands/sub_bass", f.sub_bass);
        self.send_float("/phosphor/audio/bands/bass", f.bass);
        self.send_float("/phosphor/audio/bands/low_mid", f.low_mid);
        self.send_float("/phosphor/audio/bands/mid", f.mid);
        self.send_float("/phosphor/audio/bands/upper_mid", f.upper_mid);
        self.send_float("/phosphor/audio/bands/presence", f.presence);
        self.send_float("/phosphor/audio/bands/brilliance", f.brilliance);
        // Aggregates + beat
        self.send_float("/phosphor/audio/rms", f.rms);
        self.send_float("/phosphor/audio/kick", f.kick);
        // Spectral shape (A4 #1455): now level-invariant / log-axis and worth emitting.
        self.send_float("/phosphor/audio/centroid", f.centroid);
        self.send_float("/phosphor/audio/flux", f.flux);
        self.send_float("/phosphor/audio/flatness", f.flatness);
        self.send_float("/phosphor/audio/rolloff", f.rolloff);
        self.send_float("/phosphor/audio/bandwidth", f.bandwidth);
        self.send_float("/phosphor/audio/zcr", f.zcr);
        self.send_float("/phosphor/audio/onset", f.onset);
        self.send_float("/phosphor/audio/beat", f.beat);
        self.send_float("/phosphor/audio/beat_phase", f.beat_phase);
        self.send_float("/phosphor/audio/bpm", f.bpm * 300.0); // raw BPM, not normalized
        // A11 key (#1462): pitch-class index (×11 → 0..11), minor flag, confidence.
        self.send_float("/phosphor/audio/key/class", f.key_class * 11.0);
        self.send_float("/phosphor/audio/key/is_minor", f.key_is_minor);
        self.send_float("/phosphor/audio/key/confidence", f.key_confidence);
        // A12 downbeat (#1463): bar's "one" trigger, bar sawtooth, beat index in the bar.
        self.send_float("/phosphor/audio/downbeat", f.downbeat);
        self.send_float("/phosphor/audio/bar_phase", f.bar_phase);
        self.send_float("/phosphor/audio/beat_in_bar", f.beat_in_bar);
        // A10 loudness (#1461): momentary / short-term loudness + rising trend.
        self.send_float("/phosphor/audio/loudness_m", f.loudness_m);
        self.send_float("/phosphor/audio/loudness_s", f.loudness_s);
        self.send_float("/phosphor/audio/loudness_trend", f.loudness_trend);
        // A18 structure (#1469): section-boundary novelty, build-up, drop trigger.
        self.send_float("/phosphor/audio/section_novelty", f.section_novelty);
        self.send_float("/phosphor/audio/buildup", f.buildup);
        self.send_float("/phosphor/audio/drop", f.drop);
        // A13 stereo (#1464): balance (-1=L..+1=R), mid/side width 0..1, L/R correlation (-1..+1).
        // pan/corr are stored remapped to 0..1; emit them bipolar, matching the key/bpm convention.
        self.send_float("/phosphor/audio/pan", f.pan * 2.0 - 1.0);
        self.send_float("/phosphor/audio/stereo_width", f.stereo_width);
        self.send_float("/phosphor/audio/stereo_corr", f.stereo_corr * 2.0 - 1.0);
        // A13b per-band pan (#1801): where each band sits in the image. Emitted bipolar like `pan`.
        self.send_float(
            "/phosphor/audio/band_pan/sub_bass",
            f.band_pan_sub_bass * 2.0 - 1.0,
        );
        self.send_float("/phosphor/audio/band_pan/bass", f.band_pan_bass * 2.0 - 1.0);
        self.send_float(
            "/phosphor/audio/band_pan/low_mid",
            f.band_pan_low_mid * 2.0 - 1.0,
        );
        self.send_float("/phosphor/audio/band_pan/mid", f.band_pan_mid * 2.0 - 1.0);
        self.send_float(
            "/phosphor/audio/band_pan/upper_mid",
            f.band_pan_upper_mid * 2.0 - 1.0,
        );
        self.send_float(
            "/phosphor/audio/band_pan/presence",
            f.band_pan_presence * 2.0 - 1.0,
        );
        self.send_float(
            "/phosphor/audio/band_pan/brilliance",
            f.band_pan_brilliance * 2.0 - 1.0,
        );
        // A14 HPSS (#1465): percussive / harmonic energies (0..1) and their balance (0..1).
        self.send_float("/phosphor/audio/percussive_energy", f.percussive_energy);
        self.send_float("/phosphor/audio/harmonic_energy", f.harmonic_energy);
        self.send_float("/phosphor/audio/harmonic_ratio", f.harmonic_ratio);
        // A15 pitch (#1466): normalized log-frequency f0 (0..1), the same in real Hz (de-normalized
        // like bpm/key), and the YIN periodicity confidence (0..1).
        self.send_float("/phosphor/audio/pitch", f.pitch);
        self.send_float(
            "/phosphor/audio/pitch_hz",
            crate::audio::pitch::norm_to_hz(f.pitch),
        );
        self.send_float("/phosphor/audio/pitch_confidence", f.pitch_confidence);
        // A16 spectral contrast (#1467): per-octave peak-vs-valley tonality (0..1) + mean.
        self.send_float("/phosphor/audio/contrast_0", f.contrast_0);
        self.send_float("/phosphor/audio/contrast_1", f.contrast_1);
        self.send_float("/phosphor/audio/contrast_2", f.contrast_2);
        self.send_float("/phosphor/audio/contrast_3", f.contrast_3);
        self.send_float("/phosphor/audio/contrast_4", f.contrast_4);
        self.send_float("/phosphor/audio/contrast_5", f.contrast_5);
        self.send_float("/phosphor/audio/contrast_mean", f.contrast_mean);
        // A16 timbre dynamics (#1467): L2 of the delta-MFCC (coeffs 1..12), adaptively normalized.
        self.send_float("/phosphor/audio/timbre_flux", f.timbre_flux);
    }

    /// Send the running beat / downbeat / drop totals (#1976).
    ///
    /// A sibling of [`Self::send_audio`] rather than part of it: the counts live on the audio
    /// engine, not in [`AudioFeatures`].
    ///
    /// `/phosphor/audio/{beat,downbeat,drop}` next to these are 1-frame pulses, and this sender
    /// is rate-limited below the render rate — so most of them never reach the wire (measured:
    /// 1 of 4 caught on one capture). **External tools should watch these totals, not the
    /// pulses.** Two properties they rely on:
    ///
    /// - Watch for the value *changing*, not increasing. `AudioEngine::reconfigure` installs a
    ///   fresh counter on a device switch, so it resets to 0.
    /// - The delta between two ticks is how many events fell in that window, so nothing is
    ///   lost — only the individual timing within the window, which 30 Hz could not carry
    ///   anyway.
    ///
    /// Sent as floats to keep every `/phosphor/audio/*` address one type; exact to 2^24 pulses
    /// (~97 days of continuous 2 Hz beats).
    pub fn send_pulse_counts(&self, c: &PulseCounts) {
        for (addr, count) in PULSE_COUNT_ADDRS.iter().zip([c.beat, c.downbeat, c.drop]) {
            self.send_float(addr, count as f32);
        }
    }

    /// Send current state (active layer, effect name).
    pub fn send_state(&self, active_layer: usize, effect_name: &str) {
        self.send_int("/phosphor/state/layer", active_layer as i32);
        self.send_string("/phosphor/state/effect", effect_name);
    }

    /// Send timeline state.
    pub fn send_timeline(&self, active: bool, cue_index: usize, cue_count: usize, progress: f32) {
        self.send_int("/phosphor/state/timeline/active", active as i32);
        self.send_int("/phosphor/state/timeline/cue_index", cue_index as i32);
        self.send_int("/phosphor/state/timeline/cue_count", cue_count as i32);
        self.send_float("/phosphor/state/timeline/transition_progress", progress);
    }

    fn send_float(&self, addr: &str, value: f32) {
        self.send_packet(addr, vec![OscType::Float(value)]);
    }

    fn send_int(&self, addr: &str, value: i32) {
        self.send_packet(addr, vec![OscType::Int(value)]);
    }

    fn send_string(&self, addr: &str, value: &str) {
        self.send_packet(addr, vec![OscType::String(value.to_string())]);
    }

    fn send_packet(&self, addr: &str, args: Vec<OscType>) {
        let Some(ref socket) = self.socket else {
            return;
        };
        let packet = OscPacket::Message(OscMessage {
            addr: addr.to_string(),
            args,
        });
        match rosc::encoder::encode(&packet) {
            Ok(bytes) => {
                let _ = socket.send_to(&bytes, &self.target);
            }
            Err(e) => {
                log::debug!("OSC encode error: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Bind a receiver on an ephemeral loopback port and a sender aimed at it.
    fn loopback() -> (UdpSocket, OscSender) {
        let rx = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
        rx.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let port = rx.local_addr().expect("local addr").port();
        let mut tx = OscSender::new();
        tx.configure("127.0.0.1", port);
        (rx, tx)
    }

    /// Receive `n` messages and return them as (address, args) pairs, in arrival order.
    /// UDP on loopback does not reorder in practice, and each `send_float` is one datagram.
    fn recv_messages(rx: &UdpSocket, n: usize) -> Vec<(String, Vec<OscType>)> {
        let mut out = Vec::with_capacity(n);
        let mut buf = [0u8; 1024];
        for _ in 0..n {
            let (len, _) = rx.recv_from(&mut buf).expect("recv");
            let (_, packet) = rosc::decoder::decode_udp(&buf[..len]).expect("decode");
            match packet {
                OscPacket::Message(m) => out.push((m.addr, m.args)),
                OscPacket::Bundle(_) => panic!("expected a message, got a bundle"),
            }
        }
        out
    }

    /// The addresses and the float type are the wire contract external tools bind against
    /// (#1976) — a typo or a retype here is silent at runtime, so pin both.
    #[test]
    fn pulse_counts_round_trip_addresses_and_values() {
        let (rx, tx) = loopback();
        tx.send_pulse_counts(&PulseCounts {
            beat: 412,
            downbeat: 103,
            drop: 2,
        });

        let got = recv_messages(&rx, 3);
        let expected = [
            ("/phosphor/audio/beat_count", 412.0),
            ("/phosphor/audio/downbeat_count", 103.0),
            ("/phosphor/audio/drop_count", 2.0),
        ];
        for ((addr, args), (want_addr, want_value)) in got.iter().zip(expected) {
            assert_eq!(addr, want_addr);
            assert_eq!(args.as_slice(), &[OscType::Float(want_value)]);
        }
    }

    /// The const table drives the emit loop, so it must stay aligned with the field order of
    /// `PulseCounts` — swap two fields and every consumer silently reads the wrong counter.
    #[test]
    fn pulse_count_addrs_match_field_order() {
        let (rx, tx) = loopback();
        // Distinct values so a permuted table shows up as a mismatch rather than passing.
        tx.send_pulse_counts(&PulseCounts {
            beat: 1,
            downbeat: 2,
            drop: 3,
        });

        let got = recv_messages(&rx, 3);
        let addrs: Vec<&str> = got.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(addrs, PULSE_COUNT_ADDRS);
        let values: Vec<f32> = got
            .iter()
            .map(|(_, args)| match args.as_slice() {
                [OscType::Float(v)] => *v,
                other => panic!("expected one float, got {other:?}"),
            })
            .collect();
        assert_eq!(values, vec![1.0, 2.0, 3.0]);
    }

    /// An unconfigured sender must stay a no-op rather than panic — `send_state` calls this
    /// unconditionally once TX is enabled, and `configure` leaves `socket: None` if the bind
    /// failed.
    #[test]
    fn unconfigured_sender_is_a_noop() {
        let tx = OscSender::new();
        tx.send_pulse_counts(&PulseCounts::default());
    }
}
