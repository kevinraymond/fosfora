pub mod colors;
pub mod dark;
pub mod light;
pub mod tokens;

use egui::Visuals;
use serde::{Deserialize, Serialize};

use colors::ThemeColors;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    HighContrast,
    Midnight,
    Ember,
    Neon,
}

impl ThemeMode {
    pub const ALL: &[ThemeMode] = &[
        ThemeMode::Dark,
        ThemeMode::Light,
        ThemeMode::HighContrast,
        ThemeMode::Midnight,
        ThemeMode::Ember,
        ThemeMode::Neon,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            ThemeMode::Dark => "Dark",
            ThemeMode::Light => "Light",
            ThemeMode::HighContrast => "High Contrast",
            ThemeMode::Midnight => "Midnight",
            ThemeMode::Ember => "Ember",
            ThemeMode::Neon => "Neon",
        }
    }

    pub fn visuals(&self) -> Visuals {
        match self {
            ThemeMode::Dark => dark::dark_visuals(),
            ThemeMode::Midnight => midnight_visuals(),
            ThemeMode::Ember => ember_visuals(),
            ThemeMode::Neon => neon_visuals(),
            ThemeMode::Light => light::light_visuals(),
            ThemeMode::HighContrast => high_contrast_visuals(),
        }
    }

    pub fn colors(&self) -> ThemeColors {
        match self {
            ThemeMode::Dark => ThemeColors::dark(),
            ThemeMode::Light => ThemeColors::light(),
            ThemeMode::HighContrast => ThemeColors::high_contrast(),
            ThemeMode::Midnight => ThemeColors::midnight(),
            ThemeMode::Ember => ThemeColors::ember(),
            ThemeMode::Neon => ThemeColors::neon(),
        }
    }

    #[allow(dead_code)]
    pub fn toggle(&self) -> Self {
        match self {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
            _ => ThemeMode::Dark,
        }
    }
}

fn midnight_visuals() -> Visuals {
    use egui::{Color32, Stroke};

    let mut v = dark::dark_visuals();

    // Deep navy panels
    v.panel_fill = Color32::from_rgb(0x0E, 0x12, 0x1C);
    v.window_fill = Color32::from_rgb(0x0E, 0x12, 0x1C);
    v.extreme_bg_color = Color32::from_rgb(0x08, 0x0C, 0x14);

    v.widgets.noninteractive.bg_fill = Color32::from_rgb(0x14, 0x1A, 0x28);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xB0, 0xC0, 0xD0));
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x2A, 0x38, 0x50));

    v.widgets.inactive.bg_fill = Color32::from_rgb(0x1A, 0x24, 0x36);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xC0, 0xD0, 0xE0));
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x30, 0x40, 0x58));

    v.widgets.hovered.bg_fill = Color32::from_rgb(0x22, 0x30, 0x48);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(0xD0, 0xE0, 0xF0));
    v.widgets.hovered.bg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(0x50, 0x90, 0xD0));

    v.widgets.active.bg_fill = Color32::from_rgb(0x28, 0x3A, 0x54);
    v.widgets.active.fg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(0xE0, 0xF0, 0xFF));
    v.widgets.active.bg_stroke = Stroke::new(2.0_f32, Color32::from_rgb(0x50, 0x90, 0xD0));

    v.selection.bg_fill = Color32::from_rgb(0x50, 0x90, 0xD0).gamma_multiply(0.4);
    v.selection.stroke = Stroke::new(1.5_f32, Color32::from_rgb(0x50, 0x90, 0xD0));

    v.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x2A, 0x38, 0x50));

    v
}

fn ember_visuals() -> Visuals {
    use egui::{Color32, Stroke};

    let mut v = dark::dark_visuals();

    // Warm charcoal panels
    v.panel_fill = Color32::from_rgb(0x1A, 0x14, 0x10);
    v.window_fill = Color32::from_rgb(0x1A, 0x14, 0x10);
    v.extreme_bg_color = Color32::from_rgb(0x10, 0x0C, 0x08);

    v.widgets.noninteractive.bg_fill = Color32::from_rgb(0x24, 0x1C, 0x16);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xD0, 0xC0, 0xB0));
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x40, 0x30, 0x20));

    v.widgets.inactive.bg_fill = Color32::from_rgb(0x2C, 0x22, 0x1A);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xE0, 0xD0, 0xC0));
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x50, 0x38, 0x28));

    v.widgets.hovered.bg_fill = Color32::from_rgb(0x38, 0x2A, 0x20);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(0xF0, 0xE0, 0xD0));
    v.widgets.hovered.bg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(0xE0, 0x90, 0x30));

    v.widgets.active.bg_fill = Color32::from_rgb(0x44, 0x30, 0x22);
    v.widgets.active.fg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(0xFF, 0xF0, 0xE0));
    v.widgets.active.bg_stroke = Stroke::new(2.0_f32, Color32::from_rgb(0xE0, 0x90, 0x30));

    v.selection.bg_fill = Color32::from_rgb(0xE0, 0x90, 0x30).gamma_multiply(0.4);
    v.selection.stroke = Stroke::new(1.5_f32, Color32::from_rgb(0xE0, 0x90, 0x30));

    v.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x40, 0x30, 0x20));

    v
}

fn neon_visuals() -> Visuals {
    use egui::{Color32, Stroke};

    let mut v = dark::dark_visuals();

    // Very dark purple-black panels
    v.panel_fill = Color32::from_rgb(0x0C, 0x0A, 0x14);
    v.window_fill = Color32::from_rgb(0x0C, 0x0A, 0x14);
    v.extreme_bg_color = Color32::from_rgb(0x06, 0x04, 0x0C);

    v.widgets.noninteractive.bg_fill = Color32::from_rgb(0x14, 0x10, 0x20);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xC0, 0xB0, 0xD0));
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x30, 0x20, 0x44));

    v.widgets.inactive.bg_fill = Color32::from_rgb(0x1A, 0x14, 0x2A);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xD0, 0xC0, 0xE0));
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x3A, 0x28, 0x50));

    v.widgets.hovered.bg_fill = Color32::from_rgb(0x24, 0x1C, 0x38);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(0xE0, 0xD0, 0xF0));
    v.widgets.hovered.bg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(0xFF, 0x50, 0xC0));

    v.widgets.active.bg_fill = Color32::from_rgb(0x2C, 0x22, 0x44);
    v.widgets.active.fg_stroke = Stroke::new(1.5_f32, Color32::from_rgb(0xF0, 0xE0, 0xFF));
    v.widgets.active.bg_stroke = Stroke::new(2.0_f32, Color32::from_rgb(0xFF, 0x50, 0xC0));

    v.selection.bg_fill = Color32::from_rgb(0xFF, 0x50, 0xC0).gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.5_f32, Color32::from_rgb(0xFF, 0x50, 0xC0));

    v.window_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x30, 0x20, 0x44));

    v
}

fn high_contrast_visuals() -> Visuals {
    use egui::{Color32, Stroke};

    let mut v = dark::dark_visuals();

    // Override for maximum contrast
    v.panel_fill = Color32::from_rgb(0x0A, 0x0A, 0x0A);
    v.window_fill = Color32::from_rgb(0x0A, 0x0A, 0x0A);
    v.extreme_bg_color = Color32::BLACK;
    v.override_text_color = Some(Color32::WHITE);

    v.widgets.noninteractive.bg_fill = Color32::from_rgb(0x11, 0x11, 0x11);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0xCC, 0xCC, 0xCC));
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x66, 0x66, 0x66));

    v.widgets.inactive.bg_fill = Color32::from_rgb(0x1A, 0x1A, 0x1A);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x66, 0x66, 0x66));

    v.widgets.hovered.bg_fill = Color32::from_rgb(0x28, 0x28, 0x28);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    v.widgets.hovered.bg_stroke = Stroke::new(2.0_f32, Color32::from_rgb(0x55, 0xAA, 0xFF));

    v.widgets.active.bg_fill = Color32::from_rgb(0x33, 0x33, 0x33);
    v.widgets.active.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    v.widgets.active.bg_stroke = Stroke::new(2.0_f32, Color32::from_rgb(0x55, 0xAA, 0xFF));

    v.selection.bg_fill = Color32::from_rgb(0x55, 0xAA, 0xFF).gamma_multiply(0.5);
    v.selection.stroke = Stroke::new(2.0_f32, Color32::from_rgb(0x55, 0xAA, 0xFF));

    v.window_stroke = Stroke::new(2.0_f32, Color32::from_rgb(0x66, 0x66, 0x66));

    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_mode_all_count() {
        assert_eq!(ThemeMode::ALL.len(), 6);
    }

    #[test]
    fn theme_mode_display_names() {
        for mode in ThemeMode::ALL {
            assert!(!mode.display_name().is_empty());
        }
    }

    #[test]
    fn theme_mode_toggle() {
        assert_eq!(ThemeMode::Dark.toggle(), ThemeMode::Light);
        assert_eq!(ThemeMode::Light.toggle(), ThemeMode::Dark);
        assert_eq!(ThemeMode::HighContrast.toggle(), ThemeMode::Dark);
    }

    #[test]
    fn theme_mode_default() {
        assert_eq!(ThemeMode::default(), ThemeMode::Dark);
    }

    #[test]
    fn theme_mode_serde_roundtrip() {
        for mode in ThemeMode::ALL {
            let json = serde_json::to_string(mode).unwrap();
            let m2: ThemeMode = serde_json::from_str(&json).unwrap();
            assert_eq!(*mode, m2);
        }
    }

    // ---- Additional tests ----

    #[test]
    fn theme_mode_exact_display_names() {
        assert_eq!(ThemeMode::Dark.display_name(), "Dark");
        assert_eq!(ThemeMode::Light.display_name(), "Light");
        assert_eq!(ThemeMode::HighContrast.display_name(), "High Contrast");
        assert_eq!(ThemeMode::Midnight.display_name(), "Midnight");
        assert_eq!(ThemeMode::Ember.display_name(), "Ember");
        assert_eq!(ThemeMode::Neon.display_name(), "Neon");
    }

    #[test]
    fn theme_mode_colors_dispatch_all() {
        // Ensure colors() doesn't panic for any variant
        for mode in ThemeMode::ALL {
            let _colors = mode.colors();
        }
    }

    #[test]
    fn theme_mode_visuals_dispatch_all() {
        // Ensure visuals() doesn't panic for any variant
        for mode in ThemeMode::ALL {
            let _visuals = mode.visuals();
        }
    }

    #[test]
    fn theme_mode_vj_toggle_returns_dark() {
        assert_eq!(ThemeMode::Midnight.toggle(), ThemeMode::Dark);
        assert_eq!(ThemeMode::Ember.toggle(), ThemeMode::Dark);
        assert_eq!(ThemeMode::Neon.toggle(), ThemeMode::Dark);
    }

    #[test]
    fn theme_mode_all_contains_all_variants() {
        assert!(ThemeMode::ALL.contains(&ThemeMode::Dark));
        assert!(ThemeMode::ALL.contains(&ThemeMode::Light));
        assert!(ThemeMode::ALL.contains(&ThemeMode::HighContrast));
        assert!(ThemeMode::ALL.contains(&ThemeMode::Midnight));
        assert!(ThemeMode::ALL.contains(&ThemeMode::Ember));
        assert!(ThemeMode::ALL.contains(&ThemeMode::Neon));
    }
}
