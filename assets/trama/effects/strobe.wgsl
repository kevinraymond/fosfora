/*! trama
{
  "name": "Strobe",
  "id": "strobe",
  "kind": "effect",
  "inputs": 1,
  "params": [
    { "type": "Float", "name": "per_bar",  "default": 4.0,  "min": 1.0,  "max": 16.0 },
    { "type": "Float", "name": "duty",     "default": 0.25, "min": 0.05, "max": 0.95 },
    { "type": "Float", "name": "amount",   "default": 1.0,  "min": 0.0,  "max": 1.0 },
    { "type": "Color", "name": "color",    "default": [1.0, 1.0, 1.0, 1.0] },
    { "type": "Bool",  "name": "invert",   "default": false },
    { "type": "Bool",  "name": "on_kick",  "default": false },
    { "type": "Bool",  "name": "on_onset", "default": false }
  ],
  "integers": ["per_bar"]
}
*/
// Strobe — flashes locked to the music. By default it fires `per_bar` times a
// bar on the tempo grid (4 is every beat, 8 every eighth, 3 a triplet feel),
// each flash lasting `duty` of the gap to the next. `on_kick` and `on_onset`
// fire on the kick drum or on any hit instead of the grid. With no tempo
// locked and neither of those on, it stays dark rather than freezing lit.
//
// A flash covers the picture with `color` (its alpha is how opaque the
// flash is), so black makes it a blackout strobe. `invert` flashes the
// negative of the picture instead. `amount` scales the whole thing, and is
// the parameter to modulate for a fade-in.
//
// Like every flash in Fosfora, this goes through the photosensitivity flash
// limiter on the final output: past the limit set in Settings, flashes are
// dimmed, so `per_bar` 16 at a fast tempo will not hit the screen at full
// strength unless the limiter is off.
//
// Param slots: per_bar 0, duty 1, amount 2, color 3–6 (four scalars),
// invert 7, on_kick 8, on_onset 9.

fn strobe_gate() -> f32 {
    let on_kick = param(8u) > 0.5;
    let on_onset = param(9u) > 0.5;
    if on_kick || on_onset {
        let kick = select(0.0, step(0.5, u.kick), on_kick);
        let hit = select(0.0, step(0.5, u.onset), on_onset);
        return max(kick, hit);
    }
    if u.bpm <= 0.0 {
        return 0.0;
    }
    // The bar clock never runs backwards, so the grid stays put across bars.
    let clock = (u.bar_index + u.bar_phase) * max(round(param(0u)), 1.0);
    return select(0.0, 1.0, fract(clock) < param(1u));
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let c = input0(uv);
    let k = strobe_gate() * clamp(param(2u), 0.0, 1.0);
    if k <= 0.0 {
        return c;
    }

    if param(7u) > 0.5 {
        // Same negative as the Invert node: unpremultiplied, HDR clamped,
        // alpha untouched.
        if c.a <= 1e-4 {
            return c;
        }
        let rgb = c.rgb / c.a;
        let inv = vec3f(1.0) - clamp(rgb, vec3f(0.0), vec3f(1.0));
        return vec4f(mix(rgb, inv, k) * c.a, c.a);
    }

    // Premultiplied "over": the flash color laid on top of the picture.
    let flash = vec4f(param(3u), param(4u), param(5u), param(6u));
    let f = vec4f(flash.rgb * flash.a, flash.a) * k;
    return f + c * (1.0 - f.a);
}
