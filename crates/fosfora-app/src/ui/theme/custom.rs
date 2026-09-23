//! Custom themes (#3125): token files in `<config>/themes/`.
//!
//! A file names a built-in theme to start from and the colors it changes:
//!
//! ```json
//! { "name": "Night shift", "base": "Black", "colors": { "text": "#F4E9D8" } }
//! ```
//!
//! Colors it leaves out come from the base (Gray when it names none). A file
//! with mistakes still loads; what was wrong is listed under the theme in
//! Appearance, so the fix is visible from inside the app.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ThemeMode;
use super::palette::Palette;

#[derive(Debug, Default, Serialize, Deserialize)]
struct ThemeFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<String>,
    #[serde(default)]
    colors: BTreeMap<String, String>,
}

/// One theme file, read.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomTheme {
    /// The file's name without `.json`: what settings remember it by.
    pub stem: String,
    /// The name to show.
    pub name: String,
    pub palette: Palette,
    /// What was wrong with the file, then which colors are hard to read.
    pub problems: Vec<String>,
}

pub fn themes_dir() -> PathBuf {
    crate::paths::config_root().join("themes")
}

const PUBLISHED: &str = "fosfora_custom_themes";

/// Hand the theme list to the panels, which read it with [`published`]. It
/// stays until the next call: the list only changes on a reload.
pub fn publish(ctx: &egui::Context, themes: &[CustomTheme]) {
    let list: std::sync::Arc<[CustomTheme]> = themes.into();
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(PUBLISHED), list));
}

pub fn published(ctx: &egui::Context) -> std::sync::Arc<[CustomTheme]> {
    ctx.data(|d| d.get_temp(egui::Id::new(PUBLISHED)))
        .unwrap_or_else(|| std::sync::Arc::from(Vec::new()))
}

/// Show `dir` in the system's file manager, creating it first so there is
/// something to show.
pub fn reveal(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(target_os = "windows")]
    let opener = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let opener = "xdg-open";
    std::process::Command::new(opener)
        .arg(dir)
        .spawn()
        .map(|_| ())
}

pub fn find<'a>(themes: &'a [CustomTheme], stem: &str) -> Option<&'a CustomTheme> {
    themes.iter().find(|t| t.stem == stem)
}

/// Read one theme file's text.
fn parse(stem: &str, text: &str) -> CustomTheme {
    let mut problems = Vec::new();
    let file: ThemeFile = serde_json::from_str(text).unwrap_or_else(|e| {
        problems.push(format!("not a theme file: {e}"));
        ThemeFile::default()
    });
    let base = match file.base.as_deref() {
        None => ThemeMode::Gray,
        Some(b) => ThemeMode::built_in_named(b).unwrap_or_else(|| {
            problems.push(format!(
                "base \"{b}\" is not a built-in theme (Light, Gray, Black, Blue and orange)"
            ));
            ThemeMode::Gray
        }),
    };
    let mut palette = base.palette(&[]);
    problems.extend(palette.apply(&file.colors));
    problems.extend(palette.contrast_problems());
    CustomTheme {
        stem: stem.to_string(),
        name: file
            .name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| stem.to_string()),
        palette,
        problems,
    }
}

/// Every `.json` in `dir`, by name. A missing folder is no themes.
pub fn load_dir(dir: &Path) -> Vec<CustomTheme> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut themes: Vec<CustomTheme> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("json"))
        })
        .filter_map(|p| {
            let stem = p.file_stem()?.to_str()?.to_string();
            let theme = match std::fs::read_to_string(&p) {
                Ok(text) => parse(&stem, &text),
                Err(e) => CustomTheme {
                    stem: stem.clone(),
                    name: stem,
                    palette: Palette::GRAY,
                    problems: vec![format!("could not be read: {e}")],
                },
            };
            for problem in &theme.problems {
                log::warn!("theme {}: {problem}", p.display());
            }
            Some(theme)
        })
        .collect();
    themes.sort_by_key(|t| t.name.to_lowercase());
    themes
}

/// Write `palette` as a new theme file named after `name`, every color
/// spelled out so the file shows everything there is to change. Never
/// overwrites: a taken name gets a number. Returns the file written.
pub fn write_new(dir: &Path, name: &str, palette: &Palette) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let base: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = if base.is_empty() {
        "theme".to_string()
    } else {
        base
    };
    let path = (1..10_000)
        .map(|n| match n {
            1 => dir.join(format!("{base}.json")),
            n => dir.join(format!("{base}-{n}.json")),
        })
        .find(|p| !p.exists())
        .ok_or_else(|| std::io::Error::other("every name for this theme is taken"))?;
    let file = ThemeFile {
        name: Some(name.to_string()),
        base: None,
        colors: palette.to_map(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_changes_only_the_colors_it_names() {
        let t = parse(
            "night",
            r##"{"name":"Night shift","base":"Black","colors":{"accent":"#56B4E9"}}"##,
        );
        assert_eq!(t.name, "Night shift");
        assert_eq!(t.palette.accent.r(), 0x56);
        assert_eq!(t.palette.bg, Palette::BLACK.bg);
        assert!(t.problems.is_empty(), "{:?}", t.problems);
    }

    #[test]
    fn a_file_without_a_base_starts_from_gray_and_takes_its_stem_as_name() {
        let t = parse("mine", r#"{"colors":{}}"#);
        assert_eq!(t.name, "mine");
        assert_eq!(t.palette, Palette::GRAY);
    }

    #[test]
    fn mistakes_and_unreadable_colors_are_listed_not_fatal() {
        let t = parse(
            "bad",
            r##"{"base":"Sepia","colors":{"text":"#3A3F45","bgg":"#000000"}}"##,
        );
        // Unknown base, unknown token, and text now as dark as Gray's ground.
        assert!(t.problems.iter().any(|p| p.contains("Sepia")));
        assert!(t.problems.iter().any(|p| p.contains("bgg")));
        assert!(t.problems.iter().any(|p| p.starts_with("text on bg")));

        let t = parse("broken", "{ not json");
        assert_eq!(t.palette, Palette::GRAY);
        assert!(t.problems[0].starts_with("not a theme file"));
    }

    #[test]
    fn a_written_theme_reads_back_the_same_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("fosfora-themes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = write_new(&dir, "Blue and orange copy", &Palette::BLUE_ORANGE).unwrap();
        let b = write_new(&dir, "Blue and orange copy", &Palette::BLUE_ORANGE).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.file_name().unwrap(), "blue-and-orange-copy.json");
        let themes = load_dir(&dir);
        assert_eq!(themes.len(), 2);
        assert_eq!(themes[0].palette, Palette::BLUE_ORANGE);
        assert_eq!(themes[0].name, "Blue and orange copy");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_folder_is_no_themes() {
        assert!(load_dir(Path::new("/nonexistent/fosfora/themes")).is_empty());
    }
}
