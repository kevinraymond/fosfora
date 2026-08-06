use serde::{Deserialize, Serialize};

/// Output resolution for capture targets (NDI, recording).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputResolution {
    #[default]
    Match,
    Res720p,
    Res1080p,
    Res4K,
    Res8K,
}

impl OutputResolution {
    pub const ALL: &[OutputResolution] = &[
        OutputResolution::Match,
        OutputResolution::Res720p,
        OutputResolution::Res1080p,
        OutputResolution::Res4K,
        OutputResolution::Res8K,
    ];

    pub fn dimensions(self, window_w: u32, window_h: u32) -> (u32, u32) {
        match self {
            OutputResolution::Match => (window_w, window_h),
            OutputResolution::Res720p => (1280, 720),
            OutputResolution::Res1080p => (1920, 1080),
            OutputResolution::Res4K => (3840, 2160),
            OutputResolution::Res8K => (7680, 4320),
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            OutputResolution::Match => "Match Window",
            OutputResolution::Res720p => "720p",
            OutputResolution::Res1080p => "1080p",
            OutputResolution::Res4K => "4K",
            OutputResolution::Res8K => "8K",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_resolution_match_passes_through() {
        assert_eq!(OutputResolution::Match.dimensions(800, 600), (800, 600));
        assert_eq!(OutputResolution::Match.dimensions(1920, 1080), (1920, 1080));
    }

    #[test]
    fn output_resolution_fixed() {
        assert_eq!(OutputResolution::Res720p.dimensions(800, 600), (1280, 720));
        assert_eq!(
            OutputResolution::Res1080p.dimensions(800, 600),
            (1920, 1080)
        );
        assert_eq!(OutputResolution::Res4K.dimensions(800, 600), (3840, 2160));
        assert_eq!(OutputResolution::Res8K.dimensions(800, 600), (7680, 4320));
    }

    #[test]
    fn output_resolution_all_count() {
        assert_eq!(OutputResolution::ALL.len(), 5);
    }

    #[test]
    fn output_resolution_default_is_match() {
        assert_eq!(OutputResolution::default(), OutputResolution::Match);
    }

    #[test]
    fn output_resolution_serde_roundtrip() {
        for r in OutputResolution::ALL {
            let json = serde_json::to_string(r).unwrap();
            let r2: OutputResolution = serde_json::from_str(&json).unwrap();
            assert_eq!(*r, r2);
        }
    }

    #[test]
    fn output_resolution_exact_display_names() {
        assert_eq!(OutputResolution::Match.display_name(), "Match Window");
        assert_eq!(OutputResolution::Res720p.display_name(), "720p");
        assert_eq!(OutputResolution::Res1080p.display_name(), "1080p");
        assert_eq!(OutputResolution::Res4K.display_name(), "4K");
        assert_eq!(OutputResolution::Res8K.display_name(), "8K");
    }
}
