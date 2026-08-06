//! Decode a song file to interleaved stereo f32 at its native sample rate (#2027).
//!
//! Deliberately *not* resampled: every detector in the chain is parameterized by sample rate
//! at construction (`HopAnalyzer::new`), so feeding the file's own rate keeps the analysis
//! identical to what a live capture at that rate would produce, and avoids a resampler
//! becoming a second source of truth.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// A fully decoded song, interleaved L,R.
pub struct DecodedAudio {
    /// Interleaved stereo samples — always an even length, exactly 2 per frame. This is the
    /// same layout the capture ring hands the audio thread, so the hop slicing matches.
    pub interleaved: Vec<f32>,
    pub sample_rate: f32,
    /// Channel count of the *source*, before the stereo fold below. Reported so the analysis
    /// output can say whether the stereo field is real or synthesized from mono.
    pub source_channels: usize,
}

impl DecodedAudio {
    pub fn frames(&self) -> usize {
        self.interleaved.len() / 2
    }

    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate)
    }
}

/// Decode `path` in full. Mono is duplicated to both channels; anything above 2 channels keeps
/// the first two (the front pair in canonical order) rather than downmixing, so the A13 stereo
/// image stays the one the mix was authored with.
pub fn decode_file(path: &Path) -> Result<DecodedAudio> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .with_context(|| format!("probing {} (unsupported container?)", path.display()))?;

    let track = format
        .first_track_known_codec(TrackType::Audio)
        .ok_or_else(|| anyhow!("{} has no audio track with a known codec", path.display()))?;
    let track_id = track.id;
    let audio_params = match track.codec_params.as_ref() {
        Some(symphonia::core::codecs::CodecParameters::Audio(p)) => p.clone(),
        _ => bail!("{} track {track_id} is not audio", path.display()),
    };

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&audio_params, &AudioDecoderOptions::default())
        .context("no decoder for this codec")?;

    let mut interleaved: Vec<f32> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
    let mut sample_rate: Option<f32> = None;
    let mut source_channels = 0usize;

    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            // Clean end of stream.
            Ok(None) => break,
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(e).context("reading packet"),
        };
        if packet.track_id != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            // A single corrupt packet is skippable; the file is still usable.
            Err(SymphoniaError::DecodeError(_) | SymphoniaError::IoError(_)) => continue,
            Err(e) => return Err(e).context("decoding packet"),
        };

        append_stereo(
            &decoded,
            &mut interleaved,
            &mut scratch,
            &mut sample_rate,
            &mut source_channels,
        )?;
    }

    let sample_rate =
        sample_rate.ok_or_else(|| anyhow!("{} decoded to no audio", path.display()))?;
    if interleaved.is_empty() {
        bail!("{} decoded to zero samples", path.display());
    }

    Ok(DecodedAudio {
        interleaved,
        sample_rate,
        source_channels,
    })
}

/// Fold one decoded buffer onto the interleaved stereo output.
fn append_stereo(
    decoded: &GenericAudioBufferRef<'_>,
    out: &mut Vec<f32>,
    scratch: &mut Vec<f32>,
    sample_rate: &mut Option<f32>,
    source_channels: &mut usize,
) -> Result<()> {
    let spec = decoded.spec();
    let rate = spec.rate() as f32;
    let channels = spec.channels().count();
    if channels == 0 {
        return Ok(());
    }

    match *sample_rate {
        None => {
            *sample_rate = Some(rate);
            *source_channels = channels;
        }
        // A mid-stream rate change would silently corrupt the sample clock every timestamp is
        // derived from, so refuse rather than produce a plausible-looking wrong answer.
        Some(prev) if (prev - rate).abs() > f32::EPSILON => {
            bail!("sample rate changed mid-stream ({prev} -> {rate}); not supported")
        }
        Some(_) => {}
    }

    decoded.copy_to_vec_interleaved(scratch);
    match channels {
        1 => {
            out.reserve(scratch.len() * 2);
            for &s in scratch.iter() {
                out.push(s);
                out.push(s);
            }
        }
        _ => {
            out.reserve(scratch.len() / channels * 2);
            for frame in scratch.chunks_exact(channels) {
                out.push(frame[0]);
                out.push(frame[1]);
            }
        }
    }
    Ok(())
}
