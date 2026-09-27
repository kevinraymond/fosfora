pub mod colors;
pub mod custom;
pub mod palette;
pub mod tokens;

use serde::{Deserialize, Serialize};

use custom::CustomTheme;
use palette::Palette;

/// Which theme the interface uses (#3125). Four built in; any number more
/// from token files in the themes folder.
///
/// The aliases keep settings written before v2 loading: `settings.json` is
/// parsed whole, and one unknown theme name would reset every setting in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    Light,
    #[default]
    #[serde(alias = "Dark", alias = "Tritanopia")]
    Gray,
    #[serde(alias = "HighContrast")]
    Black,
    /// Whoever chose one of v1's colored or color-blindness themes chose
    /// color; this is the one colored theme left.
    #[serde(
        alias = "Midnight",
        alias = "Ember",
        alias = "Neon",
        alias = "Deuteranopia",
        alias = "Protanopia"
    )]
    BlueOrange,
    /// A token file in the themes folder, by file stem.
    Custom(String),
}

impl ThemeMode {
    pub const BUILT_IN: [ThemeMode; 4] = [
        ThemeMode::Light,
        ThemeMode::Gray,
        ThemeMode::Black,
        ThemeMode::BlueOrange,
    ];

    /// The name shown for the theme. A custom theme's is the name inside its
    /// file, or the file's stem when it has none.
    pub fn display_name(&self, custom: &[CustomTheme]) -> String {
        match self {
            ThemeMode::Light => "Light".into(),
            ThemeMode::Gray => "Gray".into(),
            ThemeMode::Black => "Black".into(),
            ThemeMode::BlueOrange => "Blue and orange".into(),
            ThemeMode::Custom(stem) => custom::find(custom, stem)
                .map(|t| t.name.clone())
                .unwrap_or_else(|| stem.clone()),
        }
    }

    pub fn built_in_palette(&self) -> Option<Palette> {
        match self {
            ThemeMode::Light => Some(Palette::LIGHT),
            ThemeMode::Gray => Some(Palette::GRAY),
            ThemeMode::Black => Some(Palette::BLACK),
            ThemeMode::BlueOrange => Some(Palette::BLUE_ORANGE),
            ThemeMode::Custom(_) => None,
        }
    }

    /// The colors to draw with. A custom theme whose file has gone falls back
    /// to Gray rather than failing.
    pub fn palette(&self, custom: &[CustomTheme]) -> Palette {
        match self {
            ThemeMode::Custom(stem) => custom::find(custom, stem)
                .map(|t| t.palette)
                .unwrap_or(Palette::GRAY),
            built_in => built_in.built_in_palette().unwrap_or(Palette::GRAY),
        }
    }

    /// A built-in theme by the name a theme file's `base` gives it: its
    /// display name or its settings name, in any case.
    pub fn built_in_named(name: &str) -> Option<ThemeMode> {
        let n: String = name
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect();
        Self::BUILT_IN.into_iter().find(|t| {
            let shown: String = t
                .display_name(&[])
                .chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect();
            shown == n || shown.replace("and", "") == n
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_themes_round_trip_through_settings() {
        for mode in ThemeMode::BUILT_IN {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(serde_json::from_str::<ThemeMode>(&json).unwrap(), mode);
        }
        let custom = ThemeMode::Custom("night shift".into());
        let json = serde_json::to_string(&custom).unwrap();
        assert_eq!(serde_json::from_str::<ThemeMode>(&json).unwrap(), custom);
    }

    #[test]
    fn v1_theme_names_still_load() {
        let old = |s: &str| serde_json::from_str::<ThemeMode>(&format!("\"{s}\"")).unwrap();
        assert_eq!(old("Dark"), ThemeMode::Gray);
        assert_eq!(old("Light"), ThemeMode::Light);
        assert_eq!(old("HighContrast"), ThemeMode::Black);
        assert_eq!(old("Midnight"), ThemeMode::BlueOrange);
        assert_eq!(old("Ember"), ThemeMode::BlueOrange);
        assert_eq!(old("Neon"), ThemeMode::BlueOrange);
    }

    #[test]
    fn default_is_gray() {
        assert_eq!(ThemeMode::default(), ThemeMode::Gray);
    }

    #[test]
    fn a_missing_custom_theme_draws_as_gray() {
        assert_eq!(ThemeMode::Custom("gone".into()).palette(&[]), Palette::GRAY);
    }

    #[test]
    fn a_base_is_found_by_either_name_in_any_case() {
        assert_eq!(ThemeMode::built_in_named("gray"), Some(ThemeMode::Gray));
        assert_eq!(ThemeMode::built_in_named("BLACK"), Some(ThemeMode::Black));
        assert_eq!(
            ThemeMode::built_in_named("Blue and orange"),
            Some(ThemeMode::BlueOrange)
        );
        assert_eq!(
            ThemeMode::built_in_named("BlueOrange"),
            Some(ThemeMode::BlueOrange)
        );
        assert_eq!(ThemeMode::built_in_named("Dark"), None);
    }
}
