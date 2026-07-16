//! Minimal f32 → IEEE-754 half-precision (f16) conversion, shared by the GPU texture
//! uploaders that target `*16Float` formats (`particle::flow_field`, `audio_textures`).
//! Kept dependency-free (no `half` crate) to match the rest of the codebase.

/// Convert an `f32` to an IEEE 754 half-precision (f16) value, returned as its `u16` bit
/// pattern. Handles Inf/NaN, overflow → Inf, and underflow → zero/denormal.
pub fn f32_to_f16(val: f32) -> u16 {
    let bits = val.to_bits();
    let sign = (bits >> 31) & 1;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let frac = bits & 0x7FFFFF;

    if exp == 0xFF {
        // Inf or NaN
        return ((sign << 15) | 0x7C00 | (if frac != 0 { 0x200 } else { 0 })) as u16;
    }

    let new_exp = exp - 127 + 15;

    if new_exp >= 31 {
        // Overflow → Inf
        return ((sign << 15) | 0x7C00) as u16;
    }

    if new_exp <= 0 {
        // Underflow → zero or denorm
        if new_exp < -10 {
            return (sign << 15) as u16;
        }
        let frac = (frac | 0x800000) >> (1 - new_exp);
        return ((sign << 15) | (frac >> 13)) as u16;
    }

    ((sign << 15) | ((new_exp as u32) << 10) | (frac >> 13)) as u16
}
