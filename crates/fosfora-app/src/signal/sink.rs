//! Where Signal messages go: UDP for a live rig, JSONL for offline dumps (and the
//! Workstream C benchmark harness), a plain Vec for tests. The emitter only ever
//! sees the trait, so live and offline runs share one code path.

use rosc::OscType;

use crate::osc::sender::OscSender;

pub trait SignalSink {
    /// `ts` is sample-clock seconds ([`crate::audio::AudioFrame::timestamp`]).
    /// The UDP sink ignores it; the JSONL sink records it.
    fn emit(&mut self, ts: f64, addr: &str, args: &[OscType]);
}

/// Fire-and-forget UDP, reusing the OSC sender's socket mechanics.
pub struct UdpSink {
    sender: OscSender,
}

impl UdpSink {
    pub fn new(host: &str, port: u16) -> Self {
        let mut sender = OscSender::new();
        sender.configure(host, port);
        Self { sender }
    }
}

impl SignalSink for UdpSink {
    fn emit(&mut self, _ts: f64, addr: &str, args: &[OscType]) {
        self.sender.send_message(addr, args.to_vec());
    }
}

/// One JSON object per line. The record shapes are frozen — they are the
/// `--signal-dump` contract the benchmark harness scores against:
///
/// ```json
/// {"meta":1,"schema":"/fosfora/v1","source":"song.flac","sample_rate":44100,"hop_hz":86.13,"tx_rate_hz":30}
/// {"ts":12.345,"addr":"/fosfora/v1/beat","args":[{"i":23}]}
/// ```
///
/// Args are single-key objects — `i` (int32), `f` (float32), `s` (string) —
/// exactly the OSC type tags.
pub struct JsonlSink<W: std::io::Write> {
    w: W,
}

impl<W: std::io::Write> JsonlSink<W> {
    pub fn new(w: W) -> Self {
        Self { w }
    }

    /// The meta record, written once as the first line (distinguished by its `meta` key).
    pub fn write_meta(&mut self, source: &str, sample_rate: u32, hop_hz: f64, tx_rate_hz: u32) {
        let line = serde_json::json!({
            "meta": 1,
            "schema": super::schema::PREFIX,
            "source": source,
            "sample_rate": sample_rate,
            "hop_hz": hop_hz,
            "tx_rate_hz": tx_rate_hz,
        });
        let _ = writeln!(self.w, "{line}");
    }

    #[cfg(test)]
    pub fn into_inner(self) -> W {
        self.w
    }
}

fn arg_json(arg: &OscType) -> serde_json::Value {
    match arg {
        OscType::Int(v) => serde_json::json!({ "i": v }),
        // Route the f32 through its shortest round-trip decimal so 0.82f32 dumps as
        // 0.82, not 0.8199999928474426 (f32→f64 widening drags the binary tail along).
        OscType::Float(v) => {
            let clean: f64 = format!("{v}").parse().unwrap_or(f64::from(*v));
            serde_json::json!({ "f": clean })
        }
        OscType::String(v) => serde_json::json!({ "s": v }),
        // The emitter only produces i/f/s; anything else is a bug worth surfacing
        // in the dump rather than silently dropping.
        other => serde_json::json!({ "unsupported": format!("{other:?}") }),
    }
}

impl<W: std::io::Write> SignalSink for JsonlSink<W> {
    fn emit(&mut self, ts: f64, addr: &str, args: &[OscType]) {
        let args: Vec<serde_json::Value> = args.iter().map(arg_json).collect();
        let line = serde_json::json!({ "ts": ts, "addr": addr, "args": args });
        let _ = writeln!(self.w, "{line}");
    }
}

/// Test sink: records everything.
#[cfg(test)]
pub struct VecSink {
    pub msgs: Vec<(f64, String, Vec<OscType>)>,
}

#[cfg(test)]
impl VecSink {
    pub fn new() -> Self {
        Self { msgs: Vec::new() }
    }

    /// All messages sent to `addr`, in order.
    pub fn at(&self, addr: &str) -> Vec<&(f64, String, Vec<OscType>)> {
        self.msgs.iter().filter(|(_, a, _)| a == addr).collect()
    }
}

#[cfg(test)]
impl SignalSink for VecSink {
    fn emit(&mut self, ts: f64, addr: &str, args: &[OscType]) {
        self.msgs.push((ts, addr.to_string(), args.to_vec()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::time::Duration;

    /// The exact serialized lines are the harness contract — pin them byte-for-byte.
    #[test]
    fn jsonl_lines_are_pinned() {
        let mut sink = JsonlSink::new(Vec::new());
        sink.write_meta("song.flac", 44100, 86.13, 30);
        sink.emit(12.5, "/fosfora/v1/beat", &[OscType::Int(23)]);
        sink.emit(
            12.75,
            "/fosfora/v1/section",
            &[OscType::String("build".into()), OscType::Float(0.82)],
        );

        let out = String::from_utf8(sink.into_inner()).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines[0],
            r#"{"hop_hz":86.13,"meta":1,"sample_rate":44100,"schema":"/fosfora/v1","source":"song.flac","tx_rate_hz":30}"#
        );
        assert_eq!(
            lines[1],
            r#"{"addr":"/fosfora/v1/beat","args":[{"i":23}],"ts":12.5}"#
        );
        assert_eq!(
            lines[2],
            r#"{"addr":"/fosfora/v1/section","args":[{"s":"build"},{"f":0.82}],"ts":12.75}"#
        );
    }

    /// Full addresses go on the wire untouched — the OSC TX prefix must not apply.
    #[test]
    fn udp_sink_round_trips_full_addresses() {
        let rx = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let port = rx.local_addr().unwrap().port();

        let mut sink = UdpSink::new("127.0.0.1", port);
        sink.emit(
            0.0,
            "/fosfora/v1/section",
            &[OscType::String("drop".into()), OscType::Float(0.9)],
        );

        let mut buf = [0u8; 512];
        let (len, _) = rx.recv_from(&mut buf).expect("recv");
        let (_, packet) = rosc::decoder::decode_udp(&buf[..len]).expect("decode");
        match packet {
            rosc::OscPacket::Message(m) => {
                assert_eq!(m.addr, "/fosfora/v1/section");
                assert_eq!(
                    m.args,
                    vec![OscType::String("drop".into()), OscType::Float(0.9)]
                );
            }
            other => panic!("expected message, got {other:?}"),
        }
    }
}
