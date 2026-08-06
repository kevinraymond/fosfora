// Lumen — display pass: shade the lit scene (full res).
//   feedback() = own previous frame (small blend to smooth the half-res field)
//   input0     = cascade 0 (finest radiance field, this frame)
//   input1     = scene (rgb emission, a occlusion), this frame
//
// Cascade 0 holds, per probe, 16 directional radiances in a 4x4 texel block. Averaging the 16
// directions gives the irradiance (light arriving from all around) at that probe; bilinear
// blending across the 4 nearest probes upsamples it smoothly to full res. Emitter cores are
// added straight from the scene so the lights themselves glow, and fog turns lit haze into
// volumetric shafts.

const BLOCK: i32 = 4;
const DIRS: f32 = 16.0;

// Mean radiance (irradiance) of one probe = average of its 16 direction texels. textureLoad,
// not the linear accessor, so directions inside the block aren't blurred together.
fn probe_irradiance(probe: vec2i, grid: vec2i) -> vec3f {
    let pc = clamp(probe, vec2i(0), grid - vec2i(1)) * BLOCK;
    var s = vec3f(0.0);
    for (var y = 0; y < BLOCK; y = y + 1) {
        for (var x = 0; x < BLOCK; x = x + 1) {
            s += textureLoad(input0_tex, pc + vec2i(x, y), 0).rgb;
        }
    }
    return s / DIRS;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let dims = vec2f(textureDimensions(prev_frame));
    let uv = frag_coord.xy / dims;

    let emitter_glow = param(2u);   // p2
    let bounce = param(3u);         // p3
    let fog_p = param(5u);          // p5
    let warmth = param(7u);         // p7
    let saturation = param(8u);     // p8
    let brightness = param(9u);     // p9

    // Bilinear gather of cascade-0 irradiance across the 4 nearest probes.
    let c0_dims = vec2i(textureDimensions(input0_tex));
    let grid = c0_dims / BLOCK;
    let fp = uv * vec2f(grid) - vec2f(0.5);
    let ip = vec2i(floor(fp));
    let fr = fract(fp);
    let i00 = probe_irradiance(ip + vec2i(0, 0), grid);
    let i10 = probe_irradiance(ip + vec2i(1, 0), grid);
    let i01 = probe_irradiance(ip + vec2i(0, 1), grid);
    let i11 = probe_irradiance(ip + vec2i(1, 1), grid);
    var irr = mix(mix(i00, i10, fr.x), mix(i01, i11, fr.x), fr.y);

    // Bounce gain — brilliance/presence brighten the bounced light.
    irr *= 0.28 + 1.4 * bounce + 0.5 * (u.presence + u.brilliance);

    // Emitter cores glow directly; fog makes the lit medium hazy (reveals the shafts).
    let scene = input1(uv);
    let emitters = scene.rgb * (0.4 + 2.4 * emitter_glow);
    let fog_amt = fog_p * mix(0.12, 1.1, 1.0 - u.flatness);
    var col = irr * (1.0 + 0.5 * fog_amt) + emitters;

    // Palette: warmth tilt + a gentle key-locked hue when the key is confident.
    col *= mix(vec3f(0.82, 0.94, 1.18), vec3f(1.20, 1.0, 0.78), warmth);
    let key_shift = (fosfora_key_hue(u.key_class, u.key_is_minor) - 0.5) * 0.2 * u.key_confidence;
    col = fosfora_hue_shift(col, key_shift);

    let luma = dot(col, vec3f(0.299, 0.587, 0.114));
    col = mix(vec3f(luma), col, 0.4 + 1.2 * saturation);
    col *= 0.18 + 0.85 * brightness;

    // Temporal smoothing over own history (mix, not add — self-limiting).
    col = mix(col, feedback(uv).rgb, 0.22);
    col = min(col, vec3f(2.5));
    return vec4f(col, 1.0);
}
