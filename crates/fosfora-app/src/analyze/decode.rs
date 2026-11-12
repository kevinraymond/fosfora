//! Decode a song file to interleaved stereo f32 at the analysis rate (#2027).
//!
//! Not resampled beyond what live capture does: every detector in the chain is parameterized
//! by sample rate at construction (`HopAnalyzer::new`), and above 88.2 kHz the same
//! [`decimate`](crate::audio::decimate) stage the audio thread runs halves the rate (#53). So
//! a file analyzes identically to a live capture at its rate, with one source of truth.

use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// A fully decoded song, interleaved L,R.
pub struct DecodedAudio {
    /// Interleaved stereo samples — always an even length, exactly 2 per frame. This is the
    /// same layout the capture ring hands the audio thread, so the hop slicing matches.
    pub interleaved: Vec<f32>,
    /// The analysis rate: the file's own rate, halved while above 88.2 kHz (#53).
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

/// What a song file's container says about it, read without decoding any audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioProbe {
    /// Playable frames in the track, when the container states or can estimate it. `None` for
    /// a stream with no length in its headers; the caller then has only the file size to go by.
    pub frames: Option<u64>,
    /// The file's own sample rate (before the analysis decimation `decode_file` applies).
    pub sample_rate: Option<u32>,
}

impl AudioProbe {
    /// The track's length, when both the frame count and the rate are known.
    pub fn duration_secs(&self) -> Option<f64> {
        let rate = self.sample_rate.filter(|&r| r > 0)?;
        Some(self.frames? as f64 / f64::from(rate))
    }
}

/// Read `path`'s length from its container without decoding it, so a caller that holds a whole
/// decoded song in memory (`decode_file` costs 8 bytes per frame) can refuse one that is too long
/// before paying for it. Opens the file and reads its headers only.
pub fn probe_file(path: &Path) -> Result<AudioProbe> {
    let format = open_format(path)?;
    let track = format
        .first_track_known_codec(TrackType::Audio)
        .ok_or_else(|| anyhow!("{} has no audio track with a known codec", path.display()))?;
    let sample_rate = match track.codec_params.as_ref() {
        Some(symphonia::core::codecs::CodecParameters::Audio(p)) => p.sample_rate,
        _ => bail!("{} track {} is not audio", path.display(), track.id),
    };
    Ok(AudioProbe {
        frames: track.num_frames,
        sample_rate,
    })
}

/// Open `path` and find its container, hinted by the file's extension.
fn open_format(path: &Path) -> Result<Box<dyn FormatReader>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .with_context(|| format!("probing {} (unsupported container?)", path.display()))
}

/// Decode `path` in full. Mono is duplicated to both channels; anything above 2 channels keeps
/// the first two (the front pair in canonical order) rather than downmixing, so the A13 stereo
/// image stays the one the mix was authored with.
pub fn decode_file(path: &Path) -> Result<DecodedAudio> {
    let mut format = open_format(path)?;

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

    let mut decimator = crate::audio::decimate::StereoDecimator::new(sample_rate as u32);
    if decimator.is_active() {
        let mut out = Vec::with_capacity(interleaved.len());
        decimator.process(&interleaved, &mut out);
        interleaved = out;
    }

    Ok(DecodedAudio {
        interleaved,
        sample_rate: crate::audio::decimate::analysis_rate(sample_rate as u32) as f32,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// One silent MPEG-1 Layer III frame: 128 kbps, 44.1 kHz, mono, no CRC, no padding
    /// (144 * 128000 / 44100 = 417 bytes). All-zero side info and main data decode to silence.
    fn silent_mp3_frame() -> Vec<u8> {
        let mut f = vec![0u8; 417];
        f[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0xC0]);
        f
    }

    /// An ID3v2.4 tag holding one APIC (cover art) frame of `art` bytes, the way tagged
    /// downloads carry their artwork in front of the audio.
    fn id3v2_with_art(art: usize) -> Vec<u8> {
        let syncsafe = |n: usize| {
            [
                (n >> 21) & 0x7F,
                (n >> 14) & 0x7F,
                (n >> 7) & 0x7F,
                n & 0x7F,
            ]
            .map(|b| b as u8)
        };
        // encoding, MIME "image/jpeg\0", picture type 3 (front cover), empty description.
        let mut body = vec![0u8];
        body.extend_from_slice(b"image/jpeg\0");
        body.extend_from_slice(&[3, 0]);
        body.resize(body.len() + art, 0);
        let mut frame = b"APIC".to_vec();
        frame.extend_from_slice(&syncsafe(body.len()));
        frame.extend_from_slice(&[0, 0]);
        frame.extend_from_slice(&body);
        let mut tag = b"ID3\x04\x00\x00".to_vec();
        tag.extend_from_slice(&syncsafe(frame.len()));
        tag.extend_from_slice(&frame);
        tag
    }

    fn decode_bytes(name: &str, bytes: &[u8]) -> Result<DecodedAudio> {
        let path = std::env::temp_dir().join(format!("fosfora_{}_{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let out = decode_file(&path);
        let _ = std::fs::remove_file(&path);
        out
    }

    /// #4416: an MP3 whose tag (cover art) runs past the probe's 1 MB scan still decodes. The
    /// probe can only skip a leading tag when the ID3v2 reader is compiled in; without it the
    /// probe scans the tag byte by byte for an MP3 frame and gives up at 1 MB ("reached probe
    /// limit"), which ~5% of the beat model's FMA corpus hit.
    #[test]
    fn an_mp3_behind_a_large_id3v2_tag_decodes() {
        let frames = 200;
        let mut bytes = id3v2_with_art(1_500_000);
        for _ in 0..frames {
            bytes.extend_from_slice(&silent_mp3_frame());
        }
        let audio = decode_bytes("big_tag.mp3", &bytes).expect("a tagged MP3 decodes");
        assert_eq!(audio.source_channels, 1);
        let want = (frames * 1152) as f64 / 44100.0;
        assert!(
            (audio.duration_secs() - want).abs() < 0.1,
            "decoded {:.3} s, want ~{want:.3} s",
            audio.duration_secs()
        );
    }

    fn with_temp_file<T>(name: &str, bytes: &[u8], f: impl FnOnce(&Path) -> T) -> T {
        let path = std::env::temp_dir().join(format!("fosfora_{}_{name}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let out = f(&path);
        let _ = std::fs::remove_file(&path);
        out
    }

    /// A 16-bit PCM WAV of `frames` silent stereo frames at `rate`.
    fn silent_wav(frames: u32, rate: u32) -> Vec<u8> {
        let data = frames * 4;
        let mut w = b"RIFF".to_vec();
        w.extend_from_slice(&(36 + data).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes()); // PCM
        w.extend_from_slice(&2u16.to_le_bytes()); // channels
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 4).to_le_bytes()); // byte rate
        w.extend_from_slice(&4u16.to_le_bytes()); // block align
        w.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data.to_le_bytes());
        w.resize(w.len() + data as usize, 0);
        w
    }

    /// The probe reads a WAV's length from its header, and it is the length the decode gives.
    #[test]
    fn the_probe_reads_a_wav_length_the_decode_agrees_with() {
        let bytes = silent_wav(24_000, 48_000);
        let (probe, audio) = with_temp_file("probe.wav", &bytes, |p| {
            (
                probe_file(p).expect("probes"),
                decode_file(p).expect("decodes"),
            )
        });
        assert_eq!(probe.sample_rate, Some(48_000));
        assert_eq!(probe.frames, Some(24_000));
        assert_eq!(probe.duration_secs(), Some(0.5));
        assert_eq!(audio.frames(), 24_000);
    }

    /// A constant-bitrate MP3 with no length header: the container reader estimates the length
    /// from the file's size, which is what lets a caller refuse a long, small file undecoded.
    #[test]
    fn the_probe_estimates_a_headerless_mp3() {
        let frames = 200;
        let bytes: Vec<u8> = (0..frames).flat_map(|_| silent_mp3_frame()).collect();
        let probe = with_temp_file("probe.mp3", &bytes, |p| probe_file(p).expect("probes"));
        assert_eq!(probe.sample_rate, Some(44_100));
        let want = f64::from(frames * 1152) / 44_100.0;
        let secs = probe.duration_secs().expect("an estimated length");
        assert!(
            (secs - want).abs() < 0.2,
            "probed {secs:.3} s, want ~{want:.3} s"
        );
    }

    /// A file that is not audio is an error, not a zero length.
    #[test]
    fn the_probe_refuses_what_is_not_audio() {
        let err = with_temp_file("probe.txt", b"not a song", probe_file);
        assert!(err.is_err());
    }
}
