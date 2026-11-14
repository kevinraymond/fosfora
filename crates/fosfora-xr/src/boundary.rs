//! The boundary while roaming (board #5454).
//!
//! A stationary boundary is a circle about a meter across. A wearer who
//! stands up and walks leaves it, and the system stops the show with its
//! "moved too far" screen. `XR_META_boundary_visibility` lets an app ask
//! the system to hold the boundary back, and the system only agrees while
//! a passthrough layer is on screen, so the wearer still sees the room.
//!
//! The app asks in roam only. In roam the passthrough image never goes
//! under a quarter opacity and walking brings the whole room back
//! (`roam::ROAM_FLOOR`, `roam::walk_factor`), so the room is always
//! there to see. Seated, the room may be black, and the boundary stays.
//!
//! The openxr crate has no binding for this extension, so the constants
//! here are the header's (`XR_META_boundary_visibility`, extension 529,
//! revision 1) and the call goes through `xrGetInstanceProcAddr`
//! (`xr.rs`). The runtime lists the extension only for an app whose
//! manifest asks for `com.oculus.permission.BOUNDARY_VISIBILITY`.

/// The extension's name, as the runtime lists it.
pub const EXTENSION: &str = "XR_META_boundary_visibility";

/// `xrRequestBoundaryVisibilityMETA`, the one call.
pub const REQUEST_FN: &std::ffi::CStr = c"xrRequestBoundaryVisibilityMETA";

/// `XR_TYPE_SYSTEM_BOUNDARY_VISIBILITY_PROPERTIES_META`.
pub const TYPE_SYSTEM_PROPERTIES: i32 = 1_000_528_000;

/// `XR_BOUNDARY_VISIBILITY_NOT_SUPPRESSED_META`.
pub const NOT_SUPPRESSED: i32 = 1;
/// `XR_BOUNDARY_VISIBILITY_SUPPRESSED_META`.
pub const SUPPRESSED: i32 = 2;

/// `XR_BOUNDARY_VISIBILITY_SUPPRESSION_NOT_ALLOWED_META`: a success code,
/// the system heard the request and kept the boundary.
const SUPPRESSION_NOT_ALLOWED: i32 = 1_000_528_000;

/// The app asks again this often (s). The system can bring the boundary
/// back on its own (the passthrough layer left the screen, the wearer
/// opened a system menu), and the crate drops the event that says so.
pub const ASK_EVERY_S: f32 = 2.0;

/// The runtime's own bytes for the extension among the names the crate
/// does not know (`ExtensionSet::other`), to hand back when enabling it.
/// The crate keeps a name with or without its NUL.
pub fn offered(other: &[Vec<u8>]) -> Option<&[u8]> {
    other
        .iter()
        .map(Vec::as_slice)
        .find(|name| name.strip_suffix(&[0]).unwrap_or(name) == EXTENSION.as_bytes())
}

/// The enum value for a request.
pub fn value(suppressed: bool) -> i32 {
    if suppressed {
        SUPPRESSED
    } else {
        NOT_SUPPRESSED
    }
}

/// What the system said to a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// `XR_SUCCESS`: the system took the request.
    Taken,
    /// The system kept the boundary (no passthrough layer on screen).
    NotAllowed,
    /// Any other code, an error or a success the header does not name.
    Other(i32),
}

/// The reply for a raw `XrResult`.
pub fn reply(code: i32) -> Reply {
    match code {
        0 => Reply::Taken,
        SUPPRESSION_NOT_ALLOWED => Reply::NotAllowed,
        other => Reply::Other(other),
    }
}

/// Whether the boundary is held back now: in roam with passthrough on.
/// `knob` (`debug.fosfora.boundary 0|1`) is the wearer-free override for
/// tests, 1 holding it back seated too and 0 never.
pub fn wants(roam: bool, passthrough: bool, knob: Option<bool>) -> bool {
    passthrough && knob.unwrap_or(roam)
}

/// When to ask, and what changed. One per session.
#[derive(Debug, Default)]
pub struct Ask {
    /// The last request and its reply.
    last: Option<(bool, Reply)>,
    /// Seconds since the last request.
    since_s: f32,
}

impl Ask {
    /// A frame went by wanting `want`: `Some(want)` when a request is due,
    /// at once on a change and every [`ASK_EVERY_S`] after.
    pub fn due(&mut self, dt: f32, want: bool) -> Option<bool> {
        self.since_s += dt;
        match self.last {
            // Nothing asked yet and nothing wanted: the boundary is as the
            // system has it.
            None if !want => None,
            Some((asked, _)) if asked == want && self.since_s < ASK_EVERY_S => None,
            _ => Some(want),
        }
    }

    /// The request for `want` came back with `reply`: the line to log when
    /// it is news, none when it repeats the last one.
    pub fn heard(&mut self, want: bool, reply: Reply) -> Option<String> {
        self.since_s = 0.0;
        let news = self.last != Some((want, reply));
        self.last = Some((want, reply));
        news.then(|| line(want, reply))
    }
}

/// The log line for a request and its reply.
pub fn line(want: bool, reply: Reply) -> String {
    let asked = if want {
        "asked to hold it back"
    } else {
        "asked for it back"
    };
    match reply {
        Reply::Taken => format!("boundary: {asked} · the system took it"),
        Reply::NotAllowed => {
            format!("boundary: {asked} · not allowed (no passthrough layer on screen)")
        }
        Reply::Other(code) => format!("boundary: {asked} · the system answered {code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_is_found_with_or_without_its_nul() {
        let with = vec![
            b"XR_FB_other\0".to_vec(),
            b"XR_META_boundary_visibility\0".to_vec(),
        ];
        assert_eq!(offered(&with), Some(&b"XR_META_boundary_visibility\0"[..]));
        let without = vec![b"XR_META_boundary_visibility".to_vec()];
        assert_eq!(offered(&without), Some(&b"XR_META_boundary_visibility"[..]));
        let longer = vec![b"XR_META_boundary_visibility2\0".to_vec()];
        assert_eq!(offered(&longer), None);
        assert_eq!(offered(&[]), None);
    }

    #[test]
    fn the_boundary_is_held_back_in_roam_with_passthrough_only() {
        assert!(wants(true, true, None));
        assert!(!wants(false, true, None), "seated, the boundary stays");
        assert!(!wants(true, false, None), "no passthrough, no asking");
        assert!(
            wants(false, true, Some(true)),
            "the knob holds it back seated"
        );
        assert!(!wants(true, true, Some(false)), "the knob keeps it in roam");
        assert!(!wants(true, false, Some(true)), "never without passthrough");
    }

    #[test]
    fn replies_read_the_header_codes() {
        assert_eq!(reply(0), Reply::Taken);
        assert_eq!(reply(1_000_528_000), Reply::NotAllowed);
        assert_eq!(reply(-1), Reply::Other(-1));
        assert_eq!(value(true), 2);
        assert_eq!(value(false), 1);
    }

    #[test]
    fn it_asks_on_a_change_and_every_two_seconds_after() {
        let mut ask = Ask::default();
        // Seated from the start: nothing to ask.
        assert_eq!(ask.due(1.0, false), None);
        // Roam comes on: ask at once.
        assert_eq!(ask.due(0.014, true), Some(true));
        assert!(ask.heard(true, Reply::Taken).is_some());
        // Quiet until the interval is up.
        assert_eq!(ask.due(1.0, true), None);
        assert_eq!(ask.due(0.9, true), None);
        assert_eq!(ask.due(0.2, true), Some(true));
        assert_eq!(
            ask.heard(true, Reply::Taken),
            None,
            "the same reply is no news"
        );
        // The reply changes: news.
        assert_eq!(ask.due(2.1, true), Some(true));
        assert!(ask.heard(true, Reply::NotAllowed).is_some());
        // Roam goes off: ask for the boundary back at once.
        assert_eq!(ask.due(0.014, false), Some(false));
        assert!(ask.heard(false, Reply::Taken).is_some());
    }

    #[test]
    fn the_lines_say_what_was_asked_and_heard() {
        assert_eq!(
            line(true, Reply::Taken),
            "boundary: asked to hold it back · the system took it"
        );
        assert_eq!(
            line(false, Reply::Taken),
            "boundary: asked for it back · the system took it"
        );
        assert!(line(true, Reply::NotAllowed).contains("not allowed"));
        assert!(line(true, Reply::Other(-12)).contains("-12"));
    }
}
