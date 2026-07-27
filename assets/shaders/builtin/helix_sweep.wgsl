// Helix sweep: rebuild the whole density volume each frame from a compact ring of
// per-frame audio slices, as the signed distance to a twisting ribbon swept along
// the Z axis.
//
// AXIS CONVENTION: +Z is NOW, -Z is the oldest retained slice. The camera flies
// down -Z, so history recedes ahead of the viewer.
//
// The ribbon is a closed shell whose radius, at each angle around the centreline,
// IS the spectrum: the 7 band energies drive angular lobes, so the cross-section
// literally is the frequency content of that moment. Twist rotates that profile
// along Z; a wandering centreline keeps it from reading as a straight tube.
//
// Whole-volume rewrite (one thread per voxel) rather than appending one Z-slice
// per frame: a moving write head leaves a wrap seam, and the twist could not
// evolve coherently along the length. At 128^3 this is ~2.1M invocations of cheap
// ALU, which is far from the frame budget.
//
// `HelixUniforms` is used by this shader alone, so it is declared inline (unlike
// VolUniforms / LatticeUniforms, which are shared preambles). Must byte-match the
// Rust `HelixUniforms`; `helix_uniforms_is_48_bytes` asserts the size.

struct HelixUniforms {
    grid_res: u32,
    slice_count: u32,
    head: u32, // ring index of the NEWEST slice
    time: f32,
    radius: f32,        // base cross-section radius (unit-cube units)
    thickness: f32,     // shell half-thickness; the ribbon is hollow
    twist_gain: f32,    // radians of profile rotation per unit Z
    spectrum_gain: f32, // how far the 7 bands deform the radius profile
    wander: f32,        // centreline excursion from the Z axis
    ripple_gain: f32,   // waveform min/max ripple on the shell
    hue_spread: f32,    // centroid -> aux hue spread
    _pad0: f32,
}

// One retained frame of audio. 4 vec4f = 64 B; the ring is slice_count of these.
struct HelixSlice {
    bands_lo: vec4f, // sub_bass, bass, low_mid, mid
    bands_hi: vec4f, // high_mid, presence, brilliance, rms
    path: vec4f,     // center_x, center_y, twist (accumulated), centroid01
    extra: vec4f,    // kick, wave_lo, wave_hi, unused
}

@group(0) @binding(0) var<uniform> h: HelixUniforms;
@group(0) @binding(1) var<storage, read> slices: array<HelixSlice>;
@group(0) @binding(2) var density_out: texture_storage_3d<r32float, write>;
@group(0) @binding(3) var aux_out: texture_storage_3d<r32float, write>;

const TAU: f32 = 6.28318530718;

// Ring lookup by AGE in slices (0 = newest). Wraps through the head.
fn slice_at(age: u32) -> HelixSlice {
    let n = max(h.slice_count, 1u);
    let a = min(age, n - 1u);
    let idx = (h.head + n - a) % n;
    return slices[idx];
}

// Linear blend between the two slices bracketing a fractional age, so the ribbon
// is continuous along Z instead of stepping once per retained frame.
fn slice_lerp(age_f: f32) -> HelixSlice {
    let a0 = u32(floor(max(age_f, 0.0)));
    let f = fract(max(age_f, 0.0));
    let s0 = slice_at(a0);
    let s1 = slice_at(a0 + 1u);
    var out: HelixSlice;
    out.bands_lo = mix(s0.bands_lo, s1.bands_lo, f);
    out.bands_hi = mix(s0.bands_hi, s1.bands_hi, f);
    out.extra = mix(s0.extra, s1.extra, f);
    // path.z is an accumulated angle: blend the SHORT arc so a wrap through
    // +/-PI does not spin the profile the long way round between two slices.
    var p = mix(s0.path, s1.path, f);
    let dtw = s1.path.z - s0.path.z;
    let wrapped = dtw - TAU * round(dtw / TAU);
    p.z = s0.path.z + wrapped * f;
    out.path = p;
    return out;
}

// Radius of the ribbon at `angle`, given a slice. The 7 bands become angular
// lobes 1..7, so a bass-heavy moment bulges in one broad lobe and a bright one
// ripples in seven fine ones.
fn profile_radius(s: HelixSlice, angle: f32) -> f32 {
    let b = array<f32, 7>(
        s.bands_lo.x, s.bands_lo.y, s.bands_lo.z, s.bands_lo.w,
        s.bands_hi.x, s.bands_hi.y, s.bands_hi.z,
    );
    var lobes = 0.0;
    for (var k = 0u; k < 7u; k++) {
        let harmonic = f32(k + 1u);
        // Each band gets its own phase offset so the lobes do not all peak at the
        // same angle and cancel into a plain circle.
        lobes += b[k] * cos(harmonic * angle + harmonic * 1.7) / harmonic;
    }
    // rms sets the overall calibre; kick punches it outward at that slice, so a
    // past beat stays visible in the geometry as it recedes.
    let rms = s.bands_hi.w;
    let kick = s.extra.x;
    let base = h.radius * (0.55 + 0.75 * rms + 0.5 * kick);
    return base * (1.0 + h.spectrum_gain * lobes);
}

@compute @workgroup_size(4, 4, 4)
fn cs_sweep(@builtin(global_invocation_id) gid: vec3<u32>) {
    let g = h.grid_res;
    if gid.x >= g || gid.y >= g || gid.z >= g {
        return;
    }

    // Voxel centre in the unit cube [-1,1]^3 the marcher assumes.
    let gf = f32(g);
    let p = (vec3f(gid) + vec3f(0.5)) / gf * 2.0 - vec3f(1.0);

    // +Z is now. t=1 at the newest slice, t=0 at the oldest.
    let t = (p.z + 1.0) * 0.5;
    let n = max(h.slice_count, 1u);
    let age_f = (1.0 - t) * f32(n - 1u);
    let s = slice_lerp(age_f);

    // Centreline: the slice's own wander, so the ribbon's path through space is a
    // record of the track rather than a fixed helix.
    let centre = s.path.xy * h.wander;
    let rel = p.xy - centre;
    let radius = length(rel);
    // Profile rotates with the accumulated twist plus a fixed rate along Z, so the
    // ribbon corkscrews even through steady material.
    let angle = atan2(rel.y, rel.x) - s.path.z - p.z * h.twist_gain;

    // `target` is a reserved WGSL keyword — hence `shell_r`.
    var shell_r = profile_radius(s, angle);
    // Onset/flux as a fine ripple on the shell — the transient detail the band
    // energies have already averaged away.
    //
    // The harmonic must stay a small INTEGER: it has to be periodic in `angle` or
    // the shell tears at the atan2 wrap, which rules out scaling it by radius to
    // hold arc-length constant. 9 keeps the arc wavelength near the shell around
    // 18 voxels at the default radius; the radial fade then kills what is left
    // near the axis, where any fixed harmonic is badly under-sampled. Both were
    // visible as a hard comb on the silhouette before.
    let ripple = (s.extra.y + s.extra.z) * 0.5;
    let radial_fade = smoothstep(0.0, h.radius * 0.6, radius);
    shell_r += h.ripple_gain * ripple * radial_fade * sin(angle * 9.0 + s.path.z);

    // Hollow shell: density peaks ON the surface and falls off either side, so the
    // camera flies through an open tube instead of into a solid rod.
    let dist = abs(radius - max(shell_r, 1e-4));
    let d = 1.0 - smoothstep(0.0, max(h.thickness, 1e-4), dist);

    textureStore(density_out, vec3i(gid), vec4f(d, 0.0, 0.0, 0.0));
    // aux carries the slice's spectral centroid as a 0..1 hue offset (the marcher
    // reads it as `aux - 0.5` scaled by age_influence), so colour is a property of
    // WHEN the material was heard, and travels with it down the tube.
    let hue = clamp(0.5 + (s.path.w - 0.5) * h.hue_spread, 0.0, 1.0);
    textureStore(aux_out, vec3i(gid), vec4f(hue, 0.0, 0.0, 0.0));
}
