//! The world sim's cut mask (board #3806): `debug.fosfora.simcut`, a
//! comma-separated list of the parts of `flux_xr_sim.wgsl`'s alive path to
//! skip, so a cost sweep can apportion the sim dispatch among them. A
//! diagnostic: the look may break under it, and the mask is 0 (nothing cut)
//! when the knob is unset. The XR app writes the mask into the obstacle
//! block's second header row, `.x` lane (aux[2].x, as u32 bits); the bits
//! match the shader's `XR_CUT_*` constants.

/// The cuttable parts, by knob name, in bit order (bit 0 first):
///
/// - `flow`: the two flow-field samples and the audio multiplier (flow
///   velocity 0, so no beat boost either);
/// - `turb`: the two turbulence noise samples and their velocity add;
/// - `boxes`: the collide's loop over the room boxes;
/// - `spheres`: the collide's loop over the hand spheres;
/// - `depth`: the collide with the live depth map;
/// - `lift`: the palm lift;
/// - `edge`: the respawn on leaving the volume and a free particle's reach
///   check (the particle just continues);
/// - `atomics`: the alive list's atomic, a plain store instead (the draw
///   then sees no particle alive);
/// - `path`: the whole alive path, the particle written back unchanged and
///   marked alive (the read, the write-back and the atomic alone).
pub const PARTS: [&str; 9] = [
    "flow", "turb", "boxes", "spheres", "depth", "lift", "edge", "atomics", "path",
];

/// The bit of the part named `name` (one of [`PARTS`]).
#[must_use]
pub fn bit(name: &str) -> Option<u32> {
    PARTS.iter().position(|&p| p == name).map(|i| 1 << i)
}

/// `all`: every part but `path`, which skips the rest anyway.
pub const ALL: u32 = (1 << (PARTS.len() - 1)) - 1;

/// The mask a `debug.fosfora.simcut` value asks for, and the names in it
/// that name no part (to warn about; the rest still apply). Names are
/// trimmed and case-insensitive; empty entries are ignored.
#[must_use]
pub fn parse(value: &str) -> (u32, Vec<String>) {
    let mut mask = 0;
    let mut unknown = Vec::new();
    for raw in value.split(',') {
        let name = raw.trim().to_ascii_lowercase();
        if name.is_empty() {
            continue;
        }
        if name == "all" {
            mask |= ALL;
        } else if let Some(b) = bit(&name) {
            mask |= b;
        } else {
            unknown.push(raw.trim().to_owned());
        }
    }
    (mask, unknown)
}

/// The part names set in `mask`, for the log ("none" for 0).
#[must_use]
pub fn describe(mask: u32) -> String {
    let names: Vec<&str> = PARTS
        .iter()
        .enumerate()
        .filter(|&(i, _)| mask & (1 << i) != 0)
        .map(|(_, &p)| p)
        .collect();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_cuts_nothing() {
        assert_eq!(parse(""), (0, vec![]));
        assert_eq!(parse(" , "), (0, vec![]));
        assert_eq!(describe(0), "none");
    }

    #[test]
    fn parts_are_bits_in_order() {
        assert_eq!(bit("flow"), Some(1));
        assert_eq!(bit("turb"), Some(2));
        assert_eq!(bit("boxes"), Some(4));
        assert_eq!(bit("spheres"), Some(8));
        assert_eq!(bit("depth"), Some(16));
        assert_eq!(bit("lift"), Some(32));
        assert_eq!(bit("edge"), Some(64));
        assert_eq!(bit("atomics"), Some(128));
        assert_eq!(bit("path"), Some(256));
        assert_eq!(bit("all"), None);
    }

    #[test]
    fn two_parts_give_two_bits() {
        assert_eq!(parse("flow,boxes"), (1 | 4, vec![]));
        assert_eq!(describe(1 | 4), "flow,boxes");
    }

    #[test]
    fn all_is_every_part_but_path() {
        let (mask, unknown) = parse("all");
        assert!(unknown.is_empty());
        assert_eq!(mask, 0xff);
        assert_eq!(mask & bit("path").unwrap(), 0);
        assert_eq!(parse("all,path").0, 0x1ff);
    }

    #[test]
    fn unknown_names_are_reported_and_the_rest_parses() {
        let (mask, unknown) = parse("flow,bogus,edge");
        assert_eq!(mask, 1 | 64);
        assert_eq!(unknown, vec!["bogus".to_owned()]);
    }

    /// The shader's `XR_CUT_*` constants are these bits.
    #[test]
    fn the_shader_reads_the_same_bits() {
        let sim = include_str!("../../../assets/xr/shaders/flux_xr_sim.wgsl");
        for (i, part) in PARTS.iter().enumerate() {
            let line = format!(
                "const XR_CUT_{}: u32 = {}u;",
                part.to_ascii_uppercase(),
                1u32 << i
            );
            assert!(sim.contains(&line), "flux_xr_sim.wgsl lacks `{line}`");
        }
    }

    #[test]
    fn whitespace_and_case_are_tolerated() {
        assert_eq!(parse(" Flow , TURB,\tDepth "), (1 | 2 | 16, vec![]));
    }
}
