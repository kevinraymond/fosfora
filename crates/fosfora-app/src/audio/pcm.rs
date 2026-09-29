//! Interleaved PCM byte decoding for the WASAPI loopback backend.
//!
//! Pure byte-to-f32 conversion with no Windows dependency, kept out of `wasapi_capture` so
//! every mix format it can be handed is unit-tested on every platform, not only on a Windows
//! host.

use super::downmix::Downmix;

/// Decode one channel of one interleaved frame to f32. Out-of-range offsets read as 0, as
/// does a format with no branch here.
fn decode_sample(
    data: &[u8],
    frame_start: usize,
    ch: usize,
    is_float: bool,
    bits_per_sample: u16,
) -> f32 {
    let bytes = usize::from(bits_per_sample / 8);
    let offset = frame_start + ch * bytes;
    let Some(b) = data.get(offset..offset + bytes) else {
        return 0.0;
    };
    match (is_float, bits_per_sample) {
        (true, 32) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        // Integer PCM in a 32-bit container, including 24-bit samples left-justified in it
        // (#70: this used to fall through to 0.0, so such devices captured silence).
        (false, 32) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        (false, 24) => {
            // Sign-extend from 24 bits by placing the sample in the top of an i32.
            (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0
        }
        (false, 16) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32_768.0,
        _ => 0.0,
    }
}

/// Convert raw interleaved audio bytes to interleaved `L,R` stereo f32.
///
/// A13 (#1464): the ring carries stereo and the analysis thread derives the mono mix. Mono is
/// duplicated to both sides, stereo passes through, and more channels fold into the pair through
/// `downmix` (#67). Output is always even-length, upholding the capture ring's L/R parity
/// invariant.
pub fn convert_to_stereo_f32(
    data: &[u8],
    downmix: &Downmix,
    is_float: bool,
    bits_per_sample: u16,
    frame_bytes: usize,
) -> Vec<f32> {
    let num_frames = data.len() / frame_bytes;
    let mut stereo = Vec::with_capacity(num_frames * 2);

    for i in 0..num_frames {
        let frame_start = i * frame_bytes;
        let (l, r) =
            downmix.frame(|ch| decode_sample(data, frame_start, ch, is_float, bits_per_sample));
        stereo.push(l);
        stereo.push(r);
    }

    stereo
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode `frames` of `[L, R]` with `enc`, which writes one sample's bytes.
    fn encode(frames: &[[f32; 2]], enc: impl Fn(f32) -> Vec<u8>) -> Vec<u8> {
        frames
            .iter()
            .flat_map(|f| f.iter().flat_map(|&x| enc(x)))
            .collect()
    }

    const FRAMES: [[f32; 2]; 3] = [[0.5, -0.25], [-1.0, 0.75], [0.0, 0.125]];

    fn assert_decodes(bytes: &[u8], is_float: bool, bits: u16, tol: f32) {
        let frame_bytes = 2 * usize::from(bits / 8);
        let got = convert_to_stereo_f32(
            bytes,
            &Downmix::wave_default(2),
            is_float,
            bits,
            frame_bytes,
        );
        let want: Vec<f32> = FRAMES.iter().flatten().copied().collect();
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(&want) {
            assert!(
                (g - w).abs() <= tol,
                "{bits}-bit float={is_float}: got {g}, want {w}"
            );
        }
    }

    #[test]
    fn decodes_f32() {
        let bytes = encode(&FRAMES, |x| x.to_le_bytes().to_vec());
        assert_decodes(&bytes, true, 32, 0.0);
    }

    #[test]
    fn decodes_i32() {
        let bytes = encode(&FRAMES, |x| {
            ((f64::from(x) * 2_147_483_648.0).clamp(i32::MIN as f64, i32::MAX as f64) as i32)
                .to_le_bytes()
                .to_vec()
        });
        assert_decodes(&bytes, false, 32, 1e-6);
    }

    #[test]
    fn decodes_i24() {
        let bytes = encode(&FRAMES, |x| {
            let v = (x * 8_388_608.0).clamp(-8_388_608.0, 8_388_607.0) as i32;
            v.to_le_bytes()[..3].to_vec()
        });
        assert_decodes(&bytes, false, 24, 1e-6);
    }

    #[test]
    fn decodes_i16() {
        let bytes = encode(&FRAMES, |x| {
            ((x * 32_768.0).clamp(-32_768.0, 32_767.0) as i16)
                .to_le_bytes()
                .to_vec()
        });
        assert_decodes(&bytes, false, 16, 1e-4);
    }

    #[test]
    fn mono_duplicates_and_surround_folds_into_the_pair() {
        let mono: Vec<u8> = [0.5f32, -0.5]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        assert_eq!(
            convert_to_stereo_f32(&mono, &Downmix::wave_default(1), true, 32, 4),
            [0.5, 0.5, -0.5, -0.5]
        );

        let quad: Vec<u8> = [0.1f32, 0.2, 0.3, 0.4]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        // Quad (FL FR BL BR): the rear pair joins its side at −3 dB instead of being dropped.
        let got = convert_to_stereo_f32(&quad, &Downmix::wave_default(4), true, 32, 16);
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let want = [0.1 + h * 0.3, 0.2 + h * 0.4];
        assert!(
            got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-6),
            "{got:?}"
        );
    }

    #[test]
    fn unknown_format_and_short_data_read_silence() {
        assert_eq!(
            convert_to_stereo_f32(&[0xff; 16], &Downmix::wave_default(2), false, 8, 2),
            vec![0.0; 16]
        );
        assert_eq!(decode_sample(&[0xff; 3], 0, 1, false, 16), 0.0);
    }
}
