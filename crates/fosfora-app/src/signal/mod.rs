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
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                return Err(anyhow!("audio thread exited unexpectedly"));
            }
        }
        if last_status.elapsed() >= Duration::from_secs(1) {
            last_status = Instant::now();
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

/// `--signal-dump <audio>`: the same emitter driven by the offline decoder,
/// writing the JSONL event log instead of UDP. Deterministic — decimation and
/// the status heartbeat both ride the sample clock.
#[cfg(feature = "analyze")]
pub fn run_dump(
    input: &std::path::Path,
    out: Option<&std::path::Path>,
    args: &SignalCliArgs,
) -> Result<()> {
    use std::io::Write;

    let cfg = merged_config(args);
    let audio = crate::analyze::decode::decode_file(input)
        .with_context(|| format!("decode {}", input.display()))?;
    let hop_hz = f64::from(audio.sample_rate) / ANALYSIS_HOP as f64;
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
    jsonl.write_meta(
        &source,
        audio.sample_rate as u32,
        hop_hz,
        cfg.tx_rate_hz.clamp(1, 86),
    );

    let mut em = SignalEmitter::new(emit_cfg(&cfg));
    let mut next_status = 0.0f64;
    crate::analyze::drive_with(&audio, |_hop, ts, out| {
        em.process_frame(&out.frame, &mut jsonl);
        if ts >= next_status {
            next_status = ts + 1.0;
            em.emit_status(ts, ts, &source, hop_hz, &mut jsonl);
        }
    });
    let (beats, bars, drops) = em.totals();
    eprintln!(
        "signal-dump: {} — {beats} beats, {bars} bars, {drops} drops -> {out_desc}",
        input.display()
    );
    Ok(())
}
