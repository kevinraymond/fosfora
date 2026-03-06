use crossbeam_channel::{Receiver, Sender};
use midir::MidiInputConnection;

use super::types::{MidiMessage, MidiMsgType};

/// Parse raw MIDI bytes into a MidiMessage.
fn parse_midi_bytes(data: &[u8]) -> Option<MidiMessage> {
    if data.len() < 3 {
        return None;
    }
    let status = data[0] & 0xF0;
    let channel = (data[0] & 0x0F) + 1; // 1-indexed channels
    let number = data[1];
    let value = data[2];

    match status {
        0xB0 => Some(MidiMessage {
            msg_type: MidiMsgType::Cc,
            channel,
            number,
            value,
        }),
        0x90 => {
            // Note On (velocity 0 = Note Off)
            Some(MidiMessage {
                msg_type: MidiMsgType::Note,
                channel,
                number,
                value,
            })
        }
        0x80 => {
            // Note Off → value 0
            Some(MidiMessage {
                msg_type: MidiMsgType::Note,
                channel,
                number,
                value: 0,
            })
        }
        _ => None,
    }
}

/// RAII wrapper around a midir connection. Drop closes the port.
pub struct MidiPort {
    _connection: MidiInputConnection<()>,
    pub port_name: String,
}

impl MidiPort {
    /// Open a MIDI port by name. Returns the port handle, a receiver for parsed messages,
    /// and a receiver for MIDI clock bytes (0xF8/0xFA/0xFB/0xFC system realtime).
    pub fn open(port_name: &str) -> anyhow::Result<(Self, Receiver<MidiMessage>, Receiver<u8>)> {
        let midi_in = midir::MidiInput::new("phosphor")?;
        let ports = midi_in.ports();

        let port = ports
            .iter()
            .find(|p| {
                midi_in
                    .port_name(p)
                    .map(|n| n == port_name)
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("MIDI port '{}' not found", port_name))?;

        let (tx, rx): (Sender<MidiMessage>, Receiver<MidiMessage>) = crossbeam_channel::bounded(64);
        let (clock_tx, clock_rx): (Sender<u8>, Receiver<u8>) = crossbeam_channel::bounded(256);

        // midir manages its own callback thread internally
        let connection = midi_in
            .connect(
                port,
                "phosphor-midi",
                move |_timestamp, data, _| {
                    // System realtime messages (1-byte, 0xF8..0xFF)
                    if !data.is_empty() && data[0] >= 0xF8 {
                        let _ = clock_tx.try_send(data[0]);
                        return;
                    }
                    if let Some(msg) = parse_midi_bytes(data) {
                        let _ = tx.try_send(msg); // drop if full
                    }
                },
                (),
            )
            .map_err(|e| anyhow::anyhow!("Failed to connect MIDI port: {e}"))?;

        Ok((
            Self {
                _connection: connection,
                port_name: port_name.to_string(),
            },
            rx,
            clock_rx,
        ))
    }

    /// List all available MIDI input port names.
    pub fn list_ports() -> Vec<String> {
        let Ok(midi_in) = midir::MidiInput::new("phosphor-enumerate") else {
            return Vec::new();
        };
        midi_in
            .ports()
            .iter()
            .filter_map(|p| midi_in.port_name(p).ok())
            .collect()
    }
}
