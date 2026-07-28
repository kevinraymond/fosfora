// Pegboard (Lite-Brite) particle simulation.
// Re-makes the loaded image/video/webcam frame as a backlit peg toy: source samples
// snap to a square screen lattice, an ordered dither decides whether each hole carries
// a peg, and the peg's colour is quantized to an eight-colour tray. Kicks pop a subset
// of pegs out of their holes; a spring re-seats them.
// Structs, bindings, and helpers are in particle_lib.wgsl (auto-prepended).

fn unpack_rgba(packed: f32) -> vec4f {
    let bits = bitcast<u32>(packed);
    return vec4f(
        f32(bits & 0xFFu) / 255.0,
        f32((bits >> 8u) & 0xFFu) / 255.0,
        f32((bits >> 16u) & 0xFFu) / 255.0,
        f32((bits >> 24u) & 0xFFu) / 255.0,
    );
}

fn luminance(c: vec3f) -> f32 {
    return dot(c, vec3f(0.299, 0.587, 0.114));
}

// --- Peg tray ---

// Eight classic peg colours. There is deliberately no black: an unlit hole *is* the
// black, which is what makes the dither carry tone instead of the palette carrying it.
fn peg_colour(i: u32) -> vec3f {
    switch i {
        case 0u: { return vec3f(1.00, 0.13, 0.12); } // red
        case 1u: { return vec3f(1.00, 0.45, 0.05); } // orange
        case 2u: { return vec3f(1.00, 0.88, 0.15); } // yellow
        case 3u: { return vec3f(0.15, 0.85, 0.25); } // green
        case 4u: { return vec3f(0.10, 0.45, 1.00); } // blue
        case 5u: { return vec3f(0.55, 0.20, 0.95); } // violet
        case 6u: { return vec3f(1.00, 0.40, 0.70); } // pink
        default: { return vec3f(1.00, 1.00, 1.00); } // white
    }
}

// Nearest tray colour under a luminance-weighted metric. Chroma is compared after
// normalising out brightness, so a dark red still reads as red rather than as blue.
fn snap_to_tray(col: vec3f) -> vec3f {
    let norm = col / max(luminance(col), 0.08);
    var best = 0u;
    var best_d = 1e9;
    for (var i = 0u; i < 8u; i++) {
        let p = peg_colour(i);
        let pn = p / max(luminance(p), 0.08);
        let d = pn - norm;
        // Luminance-weighted squared distance
        let wd = d * vec3f(0.299, 0.587, 0.114);
        let dist = dot(wd, d);
        if dist < best_d {
            best_d = dist;
            best = i;
        }
    }
    return peg_colour(best);
}

// Bayer 4x4 ordered-dither threshold in (0,1). Bit-interleave form: every value
// 0..15 appears exactly once over the 4x4 tile, so it is a true dispersed-dot matrix.
fn bayer4(px: u32, py: u32) -> f32 {
    let x = px & 3u;
    let y = py & 3u;
    let a = x ^ y;
    let v = ((a >> 1u) & 1u) * 8u
          + ((y >> 1u) & 1u) * 4u
          + (a & 1u) * 2u
          + (y & 1u);
    return (f32(v) + 0.5) / 16.0;
}

@compute @workgroup_size(256)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let idx = gid.x;
    if idx >= u.max_particles {
        return;
    }

    let home = aux[idx].home;
    let home_pos = home.xy;
    let home_color = unpack_rgba(home.z);

    // Skip transparent samples (padding beyond the sampled image)
    if home_color.a < 0.01 {
        var dead = read_particle(idx);
        dead.pos_life = vec4f(99.0, 99.0, 0.0, 0.0);
        dead.color = vec4f(0.0);
        write_particle(idx, dead);
        return;
    }

    let peg_rows       = clamp(param(0u), 8.0, 160.0); // lattice rows down the screen
    let peg_fill       = param(1u);  // peg diameter as a fraction of the cell
    let tray_strength  = param(2u);  // 0 = true source colour, 1 = full 8-colour snap
    let dither_amount  = param(3u);  // ordered-dither spread
    let peg_threshold  = param(4u);  // luminance above which a hole carries a peg
    let backlight      = param(5u);  // lamp behind the board
    let pop_force      = param(6u);  // kick impulse that lifts pegs out
    let polarity       = param(7u);  // 0 = pegs in the shadows (photos), 1 = pegs in the light (art on black)

    // Empty holes are always faintly visible: seeing the unfilled lattice is most of what
    // makes the thing read as a pegboard rather than as a halftone.
    let hole_glow = 0.22;

    // --- Lattice ---
    // Cells are square *on screen*: particle positions are NDC, so the x step has to be
    // divided by the viewport aspect or the peg grid comes out stretched.
    let cell_y = 2.0 / peg_rows;
    let cell_x = cell_y / aspect();
    let cell = vec2f(cell_x, cell_y);
    let cid = floor(home_pos / cell);
    let peg_center = (cid + 0.5) * cell;

    // Lattice id as unsigned, biased so negative cells stay in range.
    let bx = u32(i32(cid.x) + 512);
    let by = u32(i32(cid.y) + 512);

    // --- Dither, then quantize ---
    // The offset is applied to luminance *before* the peg/no-peg decision, so shading
    // comes out of dither texture rather than out of brightness. Brilliance widens the
    // spread slightly, which makes the shading shimmer on hats without moving any peg.
    let bay = bayer4(bx, by);
    let spread = dither_amount * (1.0 + u.brilliance * 0.6);
    let dith = (bay - 0.5) * spread;

    // Which end of the range carries a peg is a property of the source, not of the effect:
    // the bundled art is a bright subject on black, while a photograph is usually a darker
    // subject on a lighter ground and wants the opposite.
    let lum = mix(1.0 - luminance(home_color.rgb), luminance(home_color.rgb), polarity);
    let lit = (lum + dith) > peg_threshold;

    let tray = snap_to_tray(home_color.rgb);
    let peg_rgb = mix(home_color.rgb, tray, tray_strength);

    // --- Pop-out ritual ---
    // Only a fraction of the board leaves its holes on any one hit, and the fraction
    // grows with the kick, so a soft beat ripples and a hard one showers.
    var p = read_particle(idx);

    if p.pos_life.w <= 0.0 {
        let slot = emit_claim();
        if slot < u.emit_count {
            p.pos_life = vec4f(peg_center, 0.0, 1.0);
            p.vel_size = vec4f(0.0, 0.0, 0.0, u.initial_size);
            p.color = vec4f(peg_rgb, 1.0);
            p.flags = vec4f(0.0, u.lifetime, 0.0, 0.0);
            write_particle(idx, p);
            mark_alive(idx);
        } else {
            write_particle(idx, p);
        }
        return;
    }

    var pos = p.pos_life.xy;
    var vel = p.vel_size.xy;
    let dt = u.delta_time;

    if u.beat > 0.5 {
        // Deliberately a small minority of the board. At a quarter of the pegs the lattice
        // stops reading as a lattice — the picture dissolves into scattered dots and the
        // toy is gone — so the fraction stays low and only a hard kick opens it up.
        let pop_fraction = 0.05 + u.kick * 0.22;
        if hash(f32(idx) * 1.913 + floor(u.time * 4.0)) < pop_fraction {
            let seed = f32(idx) * 5.77 + u.time;
            let spray = vec2f(hash(seed) - 0.5, hash(seed + 3.1) - 0.5);
            // Biased outward from the board centre so the shower opens up
            let outward = normalize(peg_center + vec2f(0.0001));
            vel += (outward * 0.45 + spray) * pop_force * (0.6 + u.kick);
        }
    }

    // Gravity only bites once a peg is actually out of its hole.
    let offset = pos - peg_center;
    let out_of_hole = clamp(length(offset) / max(cell_y, 1e-4), 0.0, 1.0);
    vel.y -= 1.9 * out_of_hole * dt;

    // Seat spring + frame-rate-independent damping. Stiff enough that a peg is back in its
    // hole well inside one beat at club tempo; a slack spring leaves the whole board still
    // in the air when the next kick lands and it never re-forms.
    vel += (peg_center - pos) * 45.0 * dt;
    vel *= pow(0.80, dt * 60.0);

    pos += vel * dt;

    // --- Shading ---
    // A lit peg is a lamp seen through coloured plastic; an unlit one is just the hole.
    var rgb: vec3f;
    var alpha: f32;
    if lit {
        let lamp = backlight * (0.72 + u.bass * 0.45);
        rgb = peg_rgb * lamp;
        alpha = 1.0;
    } else {
        rgb = vec3f(0.05, 0.05, 0.06);
        alpha = hole_glow;
    }

    // Pegs in flight catch the lamp edge-on and read slightly brighter.
    rgb *= 1.0 + out_of_hole * 0.5;

    p.pos_life = vec4f(pos, 0.0, 1.0);
    p.vel_size = vec4f(vel, 0.0, cell_y * 0.5 * peg_fill);
    p.color = vec4f(rgb, alpha);
    p.flags.x += dt;
    if p.flags.x >= p.flags.y {
        p.flags.x = 0.0;
    }

    write_particle(idx, p);
    mark_alive(idx);
}
