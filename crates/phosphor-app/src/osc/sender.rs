use std::net::UdpSocket;

use rosc::{OscMessage, OscPacket, OscType};

use crate::audio::features::AudioFeatures;

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
