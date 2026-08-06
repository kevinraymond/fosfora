/// A decoded frame ready for GPU upload.
pub struct DecodedFrame {
    pub data: Vec<u8>, // RGBA8
    pub width: u32,
    pub height: u32,
}

/// Playback direction for media layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayDirection {
    Forward,
    Reverse,
    PingPong,
}

/// Transport state for media playback.
#[derive(Debug, Clone)]
pub struct TransportState {
    pub playing: bool,
    pub looping: bool,
    pub speed: f32,
    pub direction: PlayDirection,
    /// Current position in seconds (for video) or frame index (for GIF).
    #[allow(dead_code)]
    pub position: f64,
    /// Total duration in seconds (for video) or total frames (for GIF as f64).
    pub duration: f64,
}

impl Default for TransportState {
    fn default() -> Self {
        Self {
            playing: true,
            looping: true,
            speed: 1.0,
            direction: PlayDirection::Forward,
            position: 0.0,
            duration: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_state_defaults() {
        let t = TransportState::default();
        assert!(t.playing);
        assert!(t.looping);
        assert!((t.speed - 1.0).abs() < 1e-6);
        assert_eq!(t.direction, PlayDirection::Forward);
        assert!((t.position - 0.0).abs() < 1e-10);
        assert!((t.duration - 0.0).abs() < 1e-10);
    }

    #[test]
    fn play_direction_equality() {
        assert_eq!(PlayDirection::Forward, PlayDirection::Forward);
        assert_ne!(PlayDirection::Forward, PlayDirection::Reverse);
        assert_ne!(PlayDirection::Reverse, PlayDirection::PingPong);
    }

    #[test]
    fn decoded_frame_constructable() {
        let frame = DecodedFrame {
            data: vec![255, 0, 0, 255],
            width: 1,
            height: 1,
        };
        assert_eq!(frame.data.len(), 4);
        assert_eq!(frame.width, 1);
    }
}
