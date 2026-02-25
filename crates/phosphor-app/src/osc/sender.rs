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
        self.send_float("/phosphor/audio/onset", f.onset);
        self.send_float("/phosphor/audio/beat", f.beat);
        self.send_float("/phosphor/audio/beat_phase", f.beat_phase);
        self.send_float("/phosphor/audio/bpm", f.bpm * 300.0); // raw BPM, not normalized
    }

    /// Send current state (active layer, effect name).
    pub fn send_state(&self, active_layer: usize, effect_name: &str) {
        self.send_int("/phosphor/state/layer", active_layer as i32);
        self.send_string("/phosphor/state/effect", effect_name);
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
