/// Global keyboard shortcut definitions.
pub struct Shortcuts;

impl Shortcuts {
    pub const TOGGLE_UI: &str = "D";
    pub const FULLSCREEN: &str = "F";
    pub const QUIT: &str = "Esc";
    pub const CYCLE_PANELS: &str = "F6";
    pub const SLIDER_STEP: &str = "Arrow";
    pub const SLIDER_LARGE_STEP: &str = "Shift+Arrow";
    pub const SLIDER_MIN: &str = "Home";
    pub const SLIDER_MAX: &str = "End";

    pub fn legend() -> &'static [(&'static str, &'static str)] {
        &[
            ("D", "Toggle UI"),
            ("F", "Fullscreen"),
            ("Esc", "Quit"),
            ("Tab", "Next widget"),
            ("Shift+Tab", "Prev widget"),
            ("F6", "Cycle panels"),
            ("Arrow", "Adjust slider (1%)"),
            ("Shift+Arrow", "Adjust slider (10%)"),
            ("Home/End", "Min/Max"),
        ]
    }
}
