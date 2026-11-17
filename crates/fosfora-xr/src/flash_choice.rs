//! The flash limit is the wearer's choice (board #5534): the setting's
//! words. Plain words and rules, so they build and test on the desktop.
//! `flash.rs` holds the limit itself and what turns it on and off
//! (`flash::decide`).
//!
//! **The setting.** "Limit flashing", on or off, saved in `hand_menu.json`
//! as `"limit_flashing"` (`palm_panel::MenuFile`). A file without the
//! field means the wearer has not chosen, and the limit is on.
//!
//! **The strobe** ([`strobe_may_run`]). `debug.fosfora.flash 1`, the
//! measurement strobe, runs only with the limit turned off by its knob.
//! The wearer's "Limit flashing: off" alone never starts it.

use crate::flash::By;

/// The setting's name in `hand_menu.json`.
pub const FIELD: &str = "limit_flashing";

/// The hand menu's row: what the setting does now.
pub fn row_label(on: bool) -> &'static str {
    if on {
        "Limit flashing: on"
    } else {
        "Limit flashing: off"
    }
}

/// Whether `debug.fosfora.flash`'s beat strobe may run: only with the
/// limit turned off by its knob, never by the wearer's setting alone.
pub fn strobe_may_run(limit_on: bool, by: By) -> bool {
    !limit_on && by == By::Knob
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_setting_s_words_say_what_it_does_now() {
        assert_eq!(row_label(true), "Limit flashing: on");
        assert_eq!(row_label(false), "Limit flashing: off");
        assert_eq!(FIELD, "limit_flashing");
    }

    #[test]
    fn the_strobe_runs_only_with_the_limit_off_by_its_knob() {
        assert!(strobe_may_run(false, By::Knob));
        for (on, by) in [
            (false, By::Wearer),
            (true, By::Wearer),
            (true, By::Knob),
            (true, By::NotAsked),
        ] {
            assert!(!strobe_may_run(on, by), "{on} {by:?}");
        }
        // As `app.rs` decides it: the wearer's "Limit flashing: off" alone
        // starts no strobe; the knob at 0 does, whatever is saved.
        let may = |knob, setting| {
            let (on, by) = crate::flash::decide(knob, setting);
            strobe_may_run(on, by)
        };
        assert!(!may(None, Some(false)));
        assert!(!may(Some("1"), Some(false)));
        assert!(may(Some("0"), Some(false)));
        assert!(may(Some("0"), Some(true)));
        assert!(may(Some("0"), None));
    }
}
