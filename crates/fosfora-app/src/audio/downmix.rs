//! Surround-to-stereo downmix for capture devices with more than two channels (#67).
//!
//! The capture ring carries interleaved L/R. Keeping only channels 0/1 of a 5.1 or 7.1 source
//! drops the centre channel, which is where film and most mixes put vocals and dialogue. This
//! folds every channel into the pair with the ITU-R BS.775 Lo/Ro coefficients: fronts at unity,
//! centre and surrounds at −3 dB, LFE omitted.

/// −3 dB.
const MINUS_3DB: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Speaker positions, in the bit order of a `WAVEFORMATEXTENSIBLE` channel mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Speaker {
    FrontLeft,
    FrontRight,
    FrontCenter,
    Lfe,
    BackLeft,
    BackRight,
    FrontLeftOfCenter,
    FrontRightOfCenter,
    BackCenter,
    SideLeft,
    SideRight,
    /// Top/height or unknown positions: not folded in.
    Other,
}

impl Speaker {
    fn from_mask_bit(bit: u32) -> Self {
        match bit {
            0 => Self::FrontLeft,
            1 => Self::FrontRight,
            2 => Self::FrontCenter,
            3 => Self::Lfe,
            4 => Self::BackLeft,
            5 => Self::BackRight,
            6 => Self::FrontLeftOfCenter,
            7 => Self::FrontRightOfCenter,
            8 => Self::BackCenter,
            9 => Self::SideLeft,
            10 => Self::SideRight,
            _ => Self::Other,
        }
    }

    /// `(to_left, to_right)` gain.
    fn gains(self) -> (f32, f32) {
        match self {
            Self::FrontLeft | Self::FrontLeftOfCenter => (1.0, 0.0),
            Self::FrontRight | Self::FrontRightOfCenter => (0.0, 1.0),
            Self::FrontCenter | Self::BackCenter => (MINUS_3DB, MINUS_3DB),
            Self::BackLeft | Self::SideLeft => (MINUS_3DB, 0.0),
            Self::BackRight | Self::SideRight => (0.0, MINUS_3DB),
            Self::Lfe | Self::Other => (0.0, 0.0),
        }
    }
}

/// Per-channel `(to_left, to_right)` gains for one device's channel layout.
#[derive(Debug, Clone)]
pub struct Downmix {
    gains: Box<[(f32, f32)]>,
}

impl Downmix {
    fn from_speakers(channels: usize, speakers: impl IntoIterator<Item = Speaker>) -> Self {
        let mut gains: Vec<(f32, f32)> = speakers
            .into_iter()
            .chain(std::iter::repeat(Speaker::Other))
            .take(channels)
            .map(Speaker::gains)
            .collect();
        // Mono and stereo pass through untouched, whatever the layout claims.
        match channels {
            0 => {}
            1 => gains[0] = (1.0, 1.0),
            2 => gains.copy_from_slice(&[(1.0, 0.0), (0.0, 1.0)]),
            _ => {}
        }
        Self {
            gains: gains.into_boxed_slice(),
        }
    }

    /// From a `WAVEFORMATEXTENSIBLE` `dwChannelMask`: channels appear in ascending bit order.
    /// A zero mask carries no layout, so it falls back to [`Downmix::wave_default`].
    pub fn from_channel_mask(channels: usize, mask: u32) -> Self {
        if mask == 0 {
            return Self::wave_default(channels);
        }
        let speakers = (0..32)
            .filter(|bit| mask & (1 << bit) != 0)
            .map(Speaker::from_mask_bit);
        Self::from_speakers(channels, speakers)
    }

    /// The conventional order for a channel count on Windows and macOS: FL FR FC LFE BL BR SL SR.
    pub fn wave_default(channels: usize) -> Self {
        let layout: &[Speaker] = match channels {
            3 => &[
                Speaker::FrontLeft,
                Speaker::FrontRight,
                Speaker::FrontCenter,
            ],
            4 => &[
                Speaker::FrontLeft,
                Speaker::FrontRight,
                Speaker::BackLeft,
                Speaker::BackRight,
            ],
            5 => &[
                Speaker::FrontLeft,
                Speaker::FrontRight,
                Speaker::FrontCenter,
                Speaker::BackLeft,
                Speaker::BackRight,
            ],
            7 => &[
                Speaker::FrontLeft,
                Speaker::FrontRight,
                Speaker::FrontCenter,
                Speaker::Lfe,
                Speaker::BackCenter,
                Speaker::SideLeft,
                Speaker::SideRight,
            ],
            _ => &[
                Speaker::FrontLeft,
                Speaker::FrontRight,
                Speaker::FrontCenter,
                Speaker::Lfe,
                Speaker::BackLeft,
                Speaker::BackRight,
                Speaker::SideLeft,
                Speaker::SideRight,
            ],
        };
        Self::from_speakers(channels, layout.iter().copied())
    }

    /// ALSA's order, which puts the rear pair before the centre: FL FR RL RR FC LFE SL SR.
    pub fn alsa_default(channels: usize) -> Self {
        let layout: &[Speaker] = match channels {
            3 => &[
                Speaker::FrontLeft,
                Speaker::FrontRight,
                Speaker::FrontCenter,
            ],
            _ => &[
                Speaker::FrontLeft,
                Speaker::FrontRight,
                Speaker::BackLeft,
                Speaker::BackRight,
                Speaker::FrontCenter,
                Speaker::Lfe,
                Speaker::SideLeft,
                Speaker::SideRight,
            ],
        };
        Self::from_speakers(channels, layout.iter().copied())
    }

    /// The layout a cpal device with no channel map most likely has on this platform.
    pub fn host_default(channels: usize) -> Self {
        if cfg!(target_os = "linux") {
            Self::alsa_default(channels)
        } else {
            Self::wave_default(channels)
        }
    }

    pub fn channels(&self) -> usize {
        self.gains.len()
    }

    /// Fold one interleaved frame (`frame.len() == channels()`) into `(left, right)`.
    pub fn frame(&self, frame: impl Fn(usize) -> f32) -> (f32, f32) {
        let mut l = 0.0;
        let mut r = 0.0;
        for (ch, &(gl, gr)) in self.gains.iter().enumerate() {
            if gl == 0.0 && gr == 0.0 {
                continue;
            }
            let s = frame(ch);
            l += gl * s;
            r += gr * s;
        }
        (l, r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mix(d: &Downmix, frame: &[f32]) -> (f32, f32) {
        d.frame(|ch| frame[ch])
    }

    fn close(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn mono_and_stereo_pass_through() {
        assert_eq!(mix(&Downmix::host_default(1), &[0.5]), (0.5, 0.5));
        assert_eq!(mix(&Downmix::host_default(2), &[0.25, -0.5]), (0.25, -0.5));
        // A mask naming a lone centre speaker is still plain mono.
        assert_eq!(mix(&Downmix::from_channel_mask(1, 0x4), &[0.5]), (0.5, 0.5));
    }

    #[test]
    fn wave_5_1_keeps_the_centre_and_drops_lfe() {
        let d = Downmix::wave_default(6);
        // Centre only: both sides at −3 dB. This is what used to vanish.
        let c = mix(&d, &[0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert!(close(c, (MINUS_3DB, MINUS_3DB)), "{c:?}");
        // LFE only: omitted.
        assert_eq!(mix(&d, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0]), (0.0, 0.0));
        // Surrounds fold to their own side at −3 dB.
        let s = mix(&d, &[0.0, 0.0, 0.0, 0.0, 1.0, -1.0]);
        assert!(close(s, (MINUS_3DB, -MINUS_3DB)), "{s:?}");
    }

    #[test]
    fn alsa_5_1_finds_the_centre_at_index_4() {
        let d = Downmix::alsa_default(6);
        let c = mix(&d, &[0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        assert!(close(c, (MINUS_3DB, MINUS_3DB)), "{c:?}");
        assert_eq!(mix(&d, &[0.0, 0.0, 0.0, 0.0, 0.0, 1.0]), (0.0, 0.0));
    }

    #[test]
    fn channel_mask_decides_the_order() {
        // KSAUDIO_SPEAKER_7POINT1_SURROUND: FL FR FC LFE BL BR SL SR.
        let d = Downmix::from_channel_mask(8, 0x63F);
        let side = mix(&d, &[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        assert!(close(side, (MINUS_3DB, 0.0)), "{side:?}");
        // 2.1 (FL FR LFE): the third channel is the LFE, not a centre.
        let d = Downmix::from_channel_mask(3, 0xB);
        assert_eq!(mix(&d, &[0.0, 0.0, 1.0]), (0.0, 0.0));
        // Zero mask falls back to the default order.
        let d = Downmix::from_channel_mask(6, 0);
        let c = mix(&d, &[0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert!(close(c, (MINUS_3DB, MINUS_3DB)), "{c:?}");
    }

    #[test]
    fn extra_channels_beyond_the_layout_are_ignored() {
        let d = Downmix::wave_default(10);
        assert_eq!(d.channels(), 10);
        let frame = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0];
        assert_eq!(mix(&d, &frame), (0.0, 0.0));
    }
}
