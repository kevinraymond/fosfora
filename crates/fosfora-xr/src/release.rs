//! What a launch shows: the developer's surface or the finished one
//! (board #4204).
//!
//! The debug and the release APK carry the same native library, so the
//! library cannot tell them apart at build time. The APK can: the release
//! build type sets `isDebuggable = false`, and Android reports it in
//! `ApplicationInfo.flags` ([`FLAG_DEBUGGABLE`]), which `app.rs` reads
//! through JNI once at launch. A debuggable APK shows the developer
//! surface, as it always has; any other shows the release surface. The
//! `debug.fosfora.devmenu 0|1` knob forces either one on either build, so
//! the release look can be checked on the debug APK.
//!
//! The release surface:
//!
//! - The hand menu without its developer rows: the Debug panel and the
//!   Pitcher toggles, and the "All: none" cell (`panel_grid::menu`). The
//!   Limit flashing row stays (board #5534): it is the wearer's setting. The
//!   debug panel stays off whatever `hand_menu.json` saved, and the file
//!   keeps that flag ([`debug_panel`]). The voice commands that reach the
//!   hidden rows do nothing there ([`voice_reaches`]).
//! - Voice transcripts and the agent's replies at the debug log level,
//!   which a release launch does not log ([`app_log_level`]).
//! - No room saved under `debug/` unless the `debug.fosfora.anchors` knob
//!   asks (`anchor_replay::saves`).
//!
//! Both surfaces show the wearer a short label when the session ends on an
//! error the app cannot recover from ([`fatal_text`]).
//!
//! Plain data and decisions, so they build and test on the desktop.

/// `ApplicationInfo.FLAG_DEBUGGABLE`: set when the manifest's application
/// is debuggable, which the Gradle debug build type makes it.
pub const FLAG_DEBUGGABLE: i32 = 1 << 1;
/// The knob that forces a surface: `1` the developer's, `0` the release
/// one; unset or any other value follows the build.
pub const KNOB: &str = "debug.fosfora.devmenu";
/// How long the fatal label shows before the app exits (s).
pub const FATAL_S: f32 = 4.0;
// Long enough to read, short enough not to hold a dead app up.
const _: () = assert!(FATAL_S >= 3.0 && FATAL_S <= 6.0);
/// The label's words for a voice command that reaches a row the release
/// surface hides.
pub const HIDDEN_REPLY: &str = "That isn't available";

/// The APK the library runs in, from its `ApplicationInfo.flags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    Debug,
    Release,
    /// The flags could not be read (the JNI call failed).
    Unknown,
}

impl Build {
    /// The build `ApplicationInfo.flags` names.
    pub fn from_flags(flags: i32) -> Self {
        if flags & FLAG_DEBUGGABLE != 0 {
            Self::Debug
        } else {
            Self::Release
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Release => "release",
            Self::Unknown => "unknown",
        }
    }
}

/// What the app shows the wearer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// The developer's: the debug panel and the developer rows in the hand
    /// menu, transcripts in the log, the room saved for replay.
    Developer,
    /// The finished app.
    Release,
}

impl Surface {
    /// The surface for `build`, unless `knob` (`debug.fosfora.devmenu`)
    /// forces one. A build that could not be read shows the release
    /// surface: a wearer never sees the developer's menu by accident, and
    /// the knob still gives it back.
    pub fn at_launch(build: Build, knob: Option<&str>) -> Self {
        match knob.map(str::trim) {
            Some("1") => Self::Developer,
            Some("0") => Self::Release,
            _ if build == Build::Debug => Self::Developer,
            _ => Self::Release,
        }
    }

    pub fn developer(self) -> bool {
        self == Self::Developer
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Developer => "developer",
            Self::Release => "release",
        }
    }
}

/// The launch's log line: the build, the surface, and the knob when it
/// decided. "build: debug APK · developer surface";
/// "build: debug APK · release surface (debug.fosfora.devmenu 0)".
pub fn launch_line(build: Build, knob: Option<&str>, surface: Surface) -> String {
    let forced = match knob.map(str::trim) {
        Some(v @ ("0" | "1")) => format!(" ({KNOB} {v})"),
        _ => String::new(),
    };
    format!(
        "build: {} APK \u{b7} {} surface{forced}",
        build.name(),
        surface.name()
    )
}

/// The debug panel at launch: whether it is on, and what to save to
/// `hand_menu.json` (`None`: leave the file alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebugPanel {
    pub on: bool,
    pub save: Option<bool>,
}

/// The debug panel at launch on `surface`, from the flag `hand_menu.json`
/// saved and the `debug.fosfora.hud` knob. The developer surface does as
/// before: the knob, when set to `0` or `1`, decides and is saved; else
/// the saved flag. The release surface has no debug panel: it is off, the
/// knob is ignored, and nothing is saved, so the flag a developer launch
/// saved is still there for the next one.
pub fn debug_panel(surface: Surface, saved: bool, knob: Option<&str>) -> DebugPanel {
    if surface == Surface::Release {
        return DebugPanel {
            on: false,
            save: None,
        };
    }
    match knob.map(str::trim) {
        Some("1") => DebugPanel {
            on: true,
            save: Some(true),
        },
        Some("0") => DebugPanel {
            on: false,
            save: Some(false),
        },
        _ => DebugPanel {
            on: saved,
            save: None,
        },
    }
}

/// Whether a voice command for `intent` acts on `surface`: everything on
/// the developer's; on the release surface not the rows it hides, the
/// Pitcher toggle and "All: none".
pub fn voice_reaches(surface: Surface, intent: &crate::intent::Intent) -> bool {
    use crate::intent::Intent;
    surface.developer() || !matches!(intent, Intent::Pitcher(_) | Intent::AllNone)
}

/// The app's own log level on `surface`: debug on the developer's, so the
/// voice transcripts and the agent's replies show; info on the release
/// one, which leaves them out. Other crates log at info on both.
pub fn app_log_level(surface: Surface) -> log::LevelFilter {
    match surface {
        Surface::Developer => log::LevelFilter::Debug,
        Surface::Release => log::LevelFilter::Info,
    }
}

/// Why the app stopped, in the wearer's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The runtime lost the session or the instance.
    SessionLost,
    /// The runtime ran out of memory.
    OutOfMemory,
    /// Any other runtime error.
    Headset,
    /// An error of the app's own.
    App,
}

impl Stop {
    /// The stop for an OpenXR result by its name (`XrResult`'s Debug,
    /// "ERROR_SESSION_LOST").
    pub fn of_xr(name: &str) -> Self {
        match name {
            "ERROR_SESSION_LOST" | "ERROR_INSTANCE_LOST" | "SESSION_LOSS_PENDING" => {
                Self::SessionLost
            }
            "ERROR_OUT_OF_MEMORY" => Self::OutOfMemory,
            _ => Self::Headset,
        }
    }

    /// One plain reason, short enough for the label's one line.
    pub fn reason(self) -> &'static str {
        match self {
            Self::SessionLost => "the session was lost",
            Self::OutOfMemory => "out of memory",
            Self::Headset => "a headset error",
            Self::App => "an app error",
        }
    }
}

/// The fatal label: "Fosfora stopped: the session was lost".
pub fn fatal_text(stop: Stop) -> String {
    format!("Fosfora stopped: {}", stop.reason())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::Intent;

    /// Board #4198: the APK's NOTICE (`scripts/xr/notice.py`) names every
    /// crate it lists at a version Cargo.lock has, so a crate bumped or
    /// dropped since it was made fails here. A crate added since is caught
    /// by `notice.py --check`, which `run.sh build` runs: only cargo knows
    /// which crates the headset's library links. It also names every part
    /// the APK ships besides the crates.
    #[test]
    fn the_notice_lists_crates_cargo_lock_has_and_every_part() {
        const NOTICE: &str = include_str!("../../../assets/xr/NOTICE");
        const LOCK: &str = include_str!("../../../Cargo.lock");
        // Cargo.lock's packages, as "name version".
        let mut locked = std::collections::HashSet::new();
        let mut name = None;
        for line in LOCK.lines() {
            if let Some(n) = line.strip_prefix("name = ") {
                name = Some(n.trim_matches('"'));
            } else if let (Some(v), Some(n)) = (line.strip_prefix("version = "), name.take()) {
                locked.insert(format!("{n} {}", v.trim_matches('"')));
            }
        }
        assert!(locked.len() > 300, "{}", locked.len());
        // The crate table, a row per crate, "name version  license", after
        // the section's words and up to the next section.
        let (head, rest) = NOTICE
            .split_once("Rust crates in the app's library (")
            .expect("the crates' section");
        let (count, rest) = rest.split_once(')').expect("its count");
        let count: usize = count.parse().expect("a count");
        let (_, rest) = rest
            .split_once("https://crates.io/crates/<name>/<version>.\n\n")
            .expect("the table");
        let table = rest.split("\n\n").next().expect("the rows");
        let rows: Vec<&str> = table.lines().collect();
        assert_eq!(rows.len(), count);
        for row in &rows {
            let mut words = row.split_whitespace();
            let (n, v) = (words.next().unwrap(), words.next().unwrap());
            assert!(
                locked.contains(&format!("{n} {v}")),
                "{n} {v} is not in Cargo.lock: run scripts/xr/notice.py"
            );
            assert!(words.next().is_some(), "{row}: no license");
        }
        for part in [
            "Fosfora",
            "whisper.cpp",
            "Whisper base.en",
            "voice model",
            "ONNX Runtime",
            "OpenXR loader",
            "libc++",
            "Ember Glow",
        ] {
            assert!(head.contains(part), "{part}");
        }
        for text in ["The MIT license", "The Apache License 2.0"] {
            assert_eq!(NOTICE.matches(text).count(), 1, "{text}");
        }
        assert!(NOTICE.contains("Version 2.0, January 2004"));
        assert!(NOTICE.contains("Permission is hereby granted, free of charge"));
    }

    #[test]
    fn the_debuggable_flag_names_the_build() {
        assert_eq!(Build::from_flags(0), Build::Release);
        assert_eq!(Build::from_flags(FLAG_DEBUGGABLE), Build::Debug);
        // Other flags change nothing (FLAG_HAS_CODE, FLAG_ALLOW_BACKUP).
        assert_eq!(Build::from_flags(0x4 | 0x8000), Build::Release);
        assert_eq!(Build::from_flags(0x4 | 0x8000 | 0x2), Build::Debug);
    }

    #[test]
    fn the_build_picks_the_surface_and_the_knob_overrides_it() {
        assert_eq!(Surface::at_launch(Build::Debug, None), Surface::Developer);
        assert_eq!(Surface::at_launch(Build::Release, None), Surface::Release);
        assert_eq!(Surface::at_launch(Build::Unknown, None), Surface::Release);
        for build in [Build::Debug, Build::Release, Build::Unknown] {
            assert_eq!(Surface::at_launch(build, Some("1")), Surface::Developer);
            assert_eq!(Surface::at_launch(build, Some("0")), Surface::Release);
            assert_eq!(Surface::at_launch(build, Some(" 0 ")), Surface::Release);
            // Any other value follows the build.
            assert_eq!(
                Surface::at_launch(build, Some("yes")),
                Surface::at_launch(build, None)
            );
            assert_eq!(
                Surface::at_launch(build, Some("")),
                Surface::at_launch(build, None)
            );
        }
    }

    #[test]
    fn the_launch_line_names_the_build_the_surface_and_the_knob() {
        assert_eq!(
            launch_line(Build::Debug, None, Surface::Developer),
            "build: debug APK \u{b7} developer surface"
        );
        assert_eq!(
            launch_line(Build::Debug, Some("0"), Surface::Release),
            "build: debug APK \u{b7} release surface (debug.fosfora.devmenu 0)"
        );
        assert_eq!(
            launch_line(Build::Release, Some("1"), Surface::Developer),
            "build: release APK \u{b7} developer surface (debug.fosfora.devmenu 1)"
        );
        assert_eq!(
            launch_line(Build::Unknown, Some("x"), Surface::Release),
            "build: unknown APK \u{b7} release surface"
        );
    }

    #[test]
    fn the_release_surface_ignores_the_saved_debug_panel_and_saves_nothing() {
        for saved in [false, true] {
            for knob in [None, Some("0"), Some("1"), Some("x")] {
                assert_eq!(
                    debug_panel(Surface::Release, saved, knob),
                    DebugPanel {
                        on: false,
                        save: None
                    },
                    "saved {saved} knob {knob:?}"
                );
            }
        }
    }

    #[test]
    fn the_developer_surface_keeps_the_saved_debug_panel_and_its_knob() {
        for saved in [false, true] {
            assert_eq!(
                debug_panel(Surface::Developer, saved, None),
                DebugPanel {
                    on: saved,
                    save: None
                }
            );
            assert_eq!(
                debug_panel(Surface::Developer, saved, Some("1")),
                DebugPanel {
                    on: true,
                    save: Some(true)
                }
            );
            assert_eq!(
                debug_panel(Surface::Developer, saved, Some("0")),
                DebugPanel {
                    on: false,
                    save: Some(false)
                }
            );
        }
    }

    #[test]
    fn voice_reaches_the_hidden_rows_on_the_developer_surface_only() {
        for hidden in [
            Intent::Pitcher(true),
            Intent::Pitcher(false),
            Intent::AllNone,
        ] {
            assert!(voice_reaches(Surface::Developer, &hidden));
            assert!(!voice_reaches(Surface::Release, &hidden));
        }
        for shown in [
            Intent::NextEffect,
            Intent::Cloud(false),
            Intent::EditRoom(true),
            Intent::Music(true),
            Intent::Recenter,
            Intent::Rescan,
            Intent::Roam(true),
            // Board #5534: the flash limit is the wearer's setting, on
            // both surfaces, either way.
            Intent::FlashLimit(true),
            Intent::FlashLimit(false),
            // Board #5575: the Tips control too.
            Intent::Tips(true),
            Intent::Tips(false),
        ] {
            assert!(voice_reaches(Surface::Developer, &shown));
            assert!(voice_reaches(Surface::Release, &shown));
        }
    }

    #[test]
    fn the_release_surface_logs_the_app_at_info() {
        assert_eq!(app_log_level(Surface::Developer), log::LevelFilter::Debug);
        assert_eq!(app_log_level(Surface::Release), log::LevelFilter::Info);
        // A debug line (the transcripts) passes on the developer surface
        // only.
        assert!(log::Level::Debug <= app_log_level(Surface::Developer));
        assert!(log::Level::Debug > app_log_level(Surface::Release));
        assert!(log::Level::Info <= app_log_level(Surface::Release));
    }

    #[test]
    fn the_fatal_label_gives_one_plain_reason_on_one_short_line() {
        assert_eq!(Stop::of_xr("ERROR_SESSION_LOST"), Stop::SessionLost);
        assert_eq!(Stop::of_xr("ERROR_INSTANCE_LOST"), Stop::SessionLost);
        assert_eq!(Stop::of_xr("SESSION_LOSS_PENDING"), Stop::SessionLost);
        assert_eq!(Stop::of_xr("ERROR_OUT_OF_MEMORY"), Stop::OutOfMemory);
        assert_eq!(Stop::of_xr("ERROR_RUNTIME_FAILURE"), Stop::Headset);
        assert_eq!(
            fatal_text(Stop::SessionLost),
            "Fosfora stopped: the session was lost"
        );
        for stop in [
            Stop::SessionLost,
            Stop::OutOfMemory,
            Stop::Headset,
            Stop::App,
        ] {
            let text = fatal_text(stop);
            assert!(text.starts_with("Fosfora stopped: "), "{text}");
            // One line, no longer than the scan label's longest
            // ("No room found. Rescan from the hand menu", 40).
            assert_eq!(crate::label::lines(&text).len(), 1);
            assert!(text.chars().count() <= 40, "{text}");
        }
    }
}
