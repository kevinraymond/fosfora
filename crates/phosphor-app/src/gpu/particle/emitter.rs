use serde::{Deserialize, Serialize};

/// Emitter shape and position definition for .pfx files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmitterDef {
    #[serde(default = "default_shape")]
    pub shape: String,
    #[serde(default)]
    pub radius: f32,
    #[serde(default)]
    pub position: [f32; 2],
    /// Image file path (relative to assets/images/) for "image" shape emitters.
    #[serde(default)]
    pub image: String,
    /// Video file path (relative to assets/videos/) or "webcam" for live source.
    /// Only used with "image" shape emitters.
    #[serde(default)]
    pub video: String,
}

impl Default for EmitterDef {
    fn default() -> Self {
        Self {
            shape: "point".to_string(),
            radius: 0.0,
            position: [0.0, 0.0],
            image: String::new(),
            video: String::new(),
        }
    }
}

fn default_shape() -> String {
    "point".to_string()
}

impl EmitterDef {
    /// Convert shape string to GPU enum value.
    pub fn shape_index(&self) -> u32 {
        match self.shape.as_str() {
            "point" => 0,
            "ring" => 1,
            "line" => 2,
            "screen" => 3,
            "image" => 4,
            "disc" => 5,
            "cone" => 6,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_index_values() {
        let cases = [
            ("point", 0),
            ("ring", 1),
            ("line", 2),
            ("screen", 3),
            ("image", 4),
            ("disc", 5),
            ("cone", 6),
            ("unknown", 0),
        ];
        for (shape, expected) in cases {
            let e = EmitterDef {
                shape: shape.to_string(),
                ..Default::default()
            };
            assert_eq!(e.shape_index(), expected, "shape={shape}");
        }
    }

    #[test]
    fn emitter_def_defaults() {
        let e = EmitterDef::default();
        assert_eq!(e.shape, "point");
        assert_eq!(e.radius, 0.0);
        assert_eq!(e.position, [0.0, 0.0]);
        assert!(e.image.is_empty());
        assert!(e.video.is_empty());
    }

    #[test]
    fn emitter_def_serde_roundtrip() {
        let e = EmitterDef {
            shape: "ring".into(),
            radius: 0.5,
            position: [0.1, 0.2],
            image: "test.png".into(),
            video: String::new(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let e2: EmitterDef = serde_json::from_str(&json).unwrap();
        assert_eq!(e2.shape, "ring");
        assert!((e2.radius - 0.5).abs() < 1e-6);
        assert_eq!(e2.image, "test.png");
        assert!(e2.video.is_empty());
    }

    #[test]
    fn emitter_def_serde_with_video() {
        let json = r#"{"shape":"image","image":"logo.png","video":"clip.mp4"}"#;
        let e: EmitterDef = serde_json::from_str(json).unwrap();
        assert_eq!(e.shape, "image");
        assert_eq!(e.image, "logo.png");
        assert_eq!(e.video, "clip.mp4");
    }

    #[test]
    fn emitter_def_serde_video_defaults() {
        // Old-format JSON without video field should still parse
        let json = r#"{"shape":"image","image":"logo.png"}"#;
        let e: EmitterDef = serde_json::from_str(json).unwrap();
        assert!(e.video.is_empty());
    }
}
