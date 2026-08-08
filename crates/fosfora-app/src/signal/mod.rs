//! Signal — the analysis engine as a product: a headless mode that broadcasts
//! Fosfora's musical understanding (beats, bars, sections, phrase position, drop
//! prediction, stem-proxy energies, the raw feature bus) over versioned OSC
//! addresses for TouchDesigner / Resolume / QLC+ / grandMA-class consumers.
//!
//! The contract lives in `docs/SIGNAL.md` and `schema.rs`. Positioning guardrails
//! (program addendum): every stateful signal carries a confidence value, and
//! Signal only ever *informs* the operator's rig — it triggers nothing itself.

pub mod clock;
pub mod emitter;
pub mod phrase;
pub mod predict;
pub mod schema;
pub mod section;
pub mod sink;
pub mod types;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};

use crate::audio::beat::TempoControl;
use crate::audio::{ANALYSIS_HOP, AudioSystem};
use crate::settings::SettingsConfig;

use emitter::{EmitCfg, SignalEmitter};
use sink::UdpSink;
use types::SignalConfig;

/// CLI overrides for `--signal` / `--signal-dump`. Merged over the loaded
/// `signal.json`; never saved back.
#[derive(Debug, Default)]
pub struct SignalCliArgs {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub rate: Option<u32>,
    pub feat_bus: bool,
    pub no_stems: bool,
    pub device: Option<String>,
}

fn merged_config(args: &SignalCliArgs) -> SignalConfig {
    let mut cfg = SignalConfig::load();
    if let Some(h) = &args.host {
        cfg.host = h.clone();
    }
    if let Some(p) = args.port {
        cfg.port = p;
    }
    if let Some(r) = args.rate {
        cfg.tx_rate_hz = r;
    }
    if args.feat_bus {
        cfg.feat_bus = true;
    }
    if args.no_stems {
        cfg.stems = false;
    }
    cfg
}

fn emit_cfg(cfg: &SignalConfig) -> EmitCfg {
    EmitCfg {
        tx_rate_hz: cfg.tx_rate_hz,
        feat_bus: cfg.feat_bus,
        stems: cfg.stems,
    }
}

/// `--signal`: the live headless analysis-broadcast loop. No window, no GPU —
/// just the audio engine and a UDP socket.
pub fn run(args: &SignalCliArgs) -> Result<()> {
    let cfg = merged_config(args);

    // Unlike --analyze (a measurement tool pinned to defaults), Signal is a rig
    // mode: honor the operator's saved device, band scale and detector tuning.
    let settings = SettingsConfig::load();
    let device = args.device.clone().or(settings.audio_device);
    let tuning = Arc::new(Mutex::new(settings.structure_tuning));
    let tempo = Arc::new(Mutex::new(TempoControl::new(settings.tempo)));
    let audio = AudioSystem::new_with_device(device.as_deref(), settings.band_scale, tuning, tempo);
    if !audio.active {
        return Err(anyhow!(
            "no audio input available{}",
            audio
                .last_error
                .as_deref()
                .map(|e| format!(": {e}"))
                .unwrap_or_default()
        ));
    }
    let hop_hz = f64::from(audio.sample_rate) / ANALYSIS_HOP as f64;
    log::info!(
        "signal: broadcasting /fosfora/v1 to {}:{} — device \"{}\", {:.1} Hz hops, {} Hz continuous{}{}",
        cfg.host,
        cfg.port,
        audio.device_name,
        hop_hz,
        cfg.tx_rate_hz.clamp(1, 86),
        if cfg.feat_bus { ", feat bus ON" } else { "" },
        if cfg.stems { "" } else { ", stems off" },
    );

    let rx = audio.frame_receiver();
    let mut sink = UdpSink::new(&cfg.host, cfg.port);
    let mut em = SignalEmitter::new(emit_cfg(&cfg));

    #[cfg(feature = "link")]
    let mut link_sys = {
        let lc = crate::link::LinkConfig::load();
        if lc.enabled {
            log::info!(
                "signal: Ableton Link on — {:?} mode, quantum {}, start/stop sync {}",
                lc.mode,
                lc.quantum_clamped(),
                if lc.start_stop_sync { "on" } else { "off" },
            );
        }
        crate::link::LinkSystem::new(lc)
    };
    // Last emitted (peers, centi-BPM, playing) — the on-change dedup key.
    #[cfg(feature = "link")]
    let mut link_prev: Option<Option<(u64, i64, bool)>> = None;
    #[cfg(feature = "link")]
    let mut link_last_drive = Instant::now();
    #[cfg(feature = "link")]
    let mut link_last_bpm = 0.0f32;

    let running = Arc::new(AtomicBool::new(true));
    {
        let running = running.clone();
        ctrlc::set_handler(move || running.store(false, Ordering::SeqCst))
            .context("install ctrl-c handler")?;
    }

    let started = Instant::now();
    let mut last_status = Instant::now();
    let mut last_ts = 0.0f64;
    while running.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(frame) => {
                last_ts = frame.timestamp;
                em.process_frame(&frame, &mut sink);
                #[cfg(feature = "link")]
                {
                    link_last_bpm = frame.features.raw_bpm();
                }
                // Drain any backlog before blocking again: the bounded(4) channel
                // gives ~43 ms of slack at 93.75 Hz, and the 1 Hz status/log tick
                // below can eat most of that (measured: 2 dropped frames in a 58 s
                // live run without this).
                while let Ok(frame) = rx.try_recv() {
                    last_ts = frame.timestamp;
                    em.process_frame(&frame, &mut sink);
                    #[cfg(feature = "link")]
                    {
                        link_last_bpm = frame.features.raw_bpm();
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(anyhow!("audio thread exited unexpectedly"));
            }
        }
        #[cfg(feature = "link")]
        {
            let dt = link_last_drive.elapsed().as_secs_f32();
            link_last_drive = Instant::now();
            let tick = link_sys.drive(audio.tempo(), link_last_bpm, dt);
            // Emit on change; the 1 Hz status tick below re-broadcasts for
            // late joiners (same policy as the on-change addresses).
            let snap = tick.map(|t| (t.peers, (t.tempo * 100.0).round() as i64, t.playing));
            if link_prev != Some(snap) {
                emit_link(&mut sink, last_ts, link_sys.config.enabled, tick);
                link_prev = Some(snap);
            }
        }
        if last_status.elapsed() >= Duration::from_secs(1) {
            last_status = Instant::now();
            #[cfg(feature = "link")]
            emit_link(
                &mut sink,
                last_ts,
                link_sys.config.enabled,
                link_sys.last_tick(),
            );
            em.emit_status(
                last_ts,
                started.elapsed().as_secs_f64(),
                &audio.device_name,
                hop_hz,
                &mut sink,
            );
            let (beats, bars, drops) = em.totals();
            let engine = audio.pulse_counts();
            // The emitter is the sole frame consumer, so gaps mean dropped frames.
            if engine.beat > beats || engine.downbeat > bars || engine.drop > drops {
                log::warn!(
                    "signal: pulse gap — engine {}b/{}d/{}x vs emitted {beats}b/{bars}d/{drops}x",
                    engine.beat,
                    engine.downbeat,
                    engine.drop
                );
            }
            log::info!(
                "signal: {beats} beats, {bars} bars, {drops} drops | tier {} | -> {}:{}",
                em.tier(),
                cfg.host,
                cfg.port
            );
        }
    }

    em.emit_offline(last_ts, &mut sink);
    log::info!("signal: clean shutdown ({} beats total)", em.totals().0);
    Ok(())
}

/// `/fosfora/v1/link/*` — live-loop telemetry only (SIGNAL.md). Deliberately
/// not part of [`SignalEmitter`]: Link state is wall-clock network state, and
/// the emitter's offline byte-determinism guarantee must hold, so
/// `--signal-dump` never sees these addresses.
#[cfg(feature = "link")]
fn emit_link(
    sink: &mut dyn sink::SignalSink,
    ts: f64,
    enabled: bool,
    tick: Option<crate::link::LinkTick>,
) {
    use rosc::OscType;
    use schema::{LINK_ENABLED, LINK_PEERS, LINK_PLAYING, LINK_TEMPO};
    sink.emit(ts, LINK_ENABLED, &[OscType::Int(i32::from(enabled))]);
    if let Some(t) = tick {
        sink.emit(
            ts,
            LINK_PEERS,
            &[OscType::Int(t.peers.min(i32::MAX as u64) as i32)],
        );
        sink.emit(ts, LINK_TEMPO, &[OscType::Float(t.tempo as f32)]);
        sink.emit(ts, LINK_PLAYING, &[OscType::Int(i32::from(t.playing))]);
    }
}

/// Dump-mode config: built-in defaults plus CLI flags, never the operator's
/// `signal.json`. Same policy as `--analyze` (a measurement tool must not
/// inherit whatever tx_rate/feat_bus/stems the rig config happens to hold) —
/// the benchmark harness scores these dumps, so two machines given the same
/// file and flags must produce the same bytes.
#[cfg(feature = "analyze")]
fn dump_config(args: &SignalCliArgs) -> SignalConfig {
    let mut cfg = SignalConfig::default();
    if let Some(r) = args.rate {
        cfg.tx_rate_hz = r;
    }
    if args.feat_bus {
        cfg.feat_bus = true;
    }
    if args.no_stems {
        cfg.stems = false;
    }
    cfg
}

/// Everything after decode: meta line, emitter loop, sample-clock status
/// heartbeat. Split from [`run_dump`] so byte determinism is testable against
/// an in-memory writer with no file I/O. Returns the emitter totals.
#[cfg(feature = "analyze")]
fn dump_stream<W: std::io::Write>(
    audio: &crate::analyze::decode::DecodedAudio,
    source: &str,
    cfg: &SignalConfig,
    jsonl: &mut sink::JsonlSink<W>,
) -> (u32, u32, u32) {
    let hop_hz = f64::from(audio.sample_rate) / ANALYSIS_HOP as f64;
    jsonl.write_meta(
        source,
        audio.sample_rate as u32,
        hop_hz,
        cfg.tx_rate_hz.clamp(1, 86),
    );

    let mut em = SignalEmitter::new(emit_cfg(cfg));
    let mut next_status = 0.0f64;
    crate::analyze::drive_with(audio, |_hop, ts, out| {
        em.process_frame(&out.frame, jsonl);
        if ts >= next_status {
            next_status = ts + 1.0;
            em.emit_status(ts, ts, source, hop_hz, jsonl);
        }
    });
    em.totals()
}

/// `--signal-dump <audio>`: the same emitter driven by the offline decoder,
/// writing the JSONL event log instead of UDP. Deterministic — decimation and
/// the status heartbeat both ride the sample clock, and the config is pinned
/// to defaults + flags (see [`dump_config`]).
#[cfg(feature = "analyze")]
pub fn run_dump(
    input: &std::path::Path,
    out: Option<&std::path::Path>,
    args: &SignalCliArgs,
) -> Result<()> {
    use std::io::Write;

    let cfg = dump_config(args);
    let audio = crate::analyze::decode::decode_file(input)
        .with_context(|| format!("decode {}", input.display()))?;
    let source = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| input.display().to_string());

    let default_out = input.with_extension("signal.jsonl");
    let (mut jsonl, out_desc): (sink::JsonlSink<Box<dyn Write>>, String) = match out {
        Some(p) if p.as_os_str() == "-" => (
            sink::JsonlSink::new(Box::new(std::io::stdout().lock())),
            "stdout".to_string(),
        ),
        other => {
            let path = other.unwrap_or(default_out.as_path());
            let file = std::fs::File::create(path)
                .with_context(|| format!("create {}", path.display()))?;
            (
                sink::JsonlSink::new(Box::new(std::io::BufWriter::new(file))),
                path.display().to_string(),
            )
        }
    };
    let (beats, bars, drops) = dump_stream(&audio, &source, &cfg, &mut jsonl);
    eprintln!(
        "signal-dump: {} — {beats} beats, {bars} bars, {drops} drops -> {out_desc}",
        input.display()
    );
    Ok(())
}

#[cfg(all(test, feature = "analyze"))]
mod tests {
    use super::*;

    fn golden_audio() -> crate::analyze::decode::DecodedAudio {
        crate::analyze::decode::DecodedAudio {
            interleaved: crate::audio::tests::golden_signal(44100.0, 8.0),
            sample_rate: 44100.0,
            source_channels: 2,
        }
    }

    /// The harness contract: identical input + config → byte-identical JSONL.
    /// 8 s crosses multiple status ticks, decimation boundaries and the 1 Hz
    /// re-broadcast, so every timing mechanism in the emitter is exercised.
    #[test]
    fn dump_bytes_are_deterministic() {
        let audio = golden_audio();
        let cfg = SignalConfig::default();

        let run = || {
            let mut jsonl = sink::JsonlSink::new(Vec::new());
            let totals = dump_stream(&audio, "golden.wav", &cfg, &mut jsonl);
            (jsonl.into_inner(), totals)
        };
        let (a, totals_a) = run();
        let (b, totals_b) = run();

        assert_eq!(totals_a, totals_b);
        assert!(totals_a.0 > 0, "golden signal must produce beats");
        let text = String::from_utf8(a.clone()).unwrap();
        let first = text.lines().next().unwrap();
        assert!(
            first.contains(r#""meta":1"#),
            "first line is the meta record"
        );
        assert!(
            text.contains("/fosfora/v1/beat"),
            "dump must contain beat events"
        );
        assert_eq!(a, b, "dump bytes must be identical across runs");
    }

    /// Dump mode never reads signal.json: defaults + CLI flags only.
    #[test]
    fn dump_config_is_defaults_plus_flags() {
        let d = dump_config(&SignalCliArgs::default());
        let def = SignalConfig::default();
        assert_eq!(d.tx_rate_hz, def.tx_rate_hz);
        assert_eq!(d.feat_bus, def.feat_bus);
        assert_eq!(d.stems, def.stems);

        let d = dump_config(&SignalCliArgs {
            rate: Some(10),
            feat_bus: true,
            no_stems: true,
            ..Default::default()
        });
        assert_eq!(d.tx_rate_hz, 10);
        assert!(d.feat_bus);
        assert!(!d.stems);
    }
}
