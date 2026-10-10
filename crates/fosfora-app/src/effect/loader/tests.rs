// The launch effect was "Phosphor" until the app was renamed; presets,
// bindings and loop specs saved before say so. Each rename must point at
// an effect that ships, and no shipped effect may still use an old name,
// or the old saves would find nothing (or the wrong thing).
#[test]
fn renamed_effects_point_at_shipped_effects() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/effects");
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "pfx"))
        .map(|e| {
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap();
            v["name"].as_str().unwrap().to_string()
        })
        .collect();
    assert!(
        names.iter().any(|n| n == LAUNCH_EFFECT),
        "{LAUNCH_EFFECT} does not ship"
    );
    let starter = crate::ui::tour::STARTER_EFFECT;
    assert!(
        names.iter().any(|n| n == starter),
        "the Layers tour's {starter} does not ship"
    );
    for (old, new) in RENAMED_EFFECTS {
        assert!(
            names.iter().any(|n| n == new),
            "{old} -> {new}: {new} does not ship"
        );
        assert!(
            !names.iter().any(|n| n == old),
            "an effect is still called {old}"
        );
        assert_eq!(current_effect_name(old), *new);
    }
    assert_eq!(current_effect_name("Aurora"), "Aurora");
}

use super::*;
use crate::gpu::test_gpu::{gpu_guard, test_gpu};

// Multi-pass graph (#1481): the injected WGSL declares one texture+sampler pair
// and one accessor per input, at bindings 7+2i / 8+2i, and nothing when there
// are no inputs.
#[test]
fn input_bindings_wgsl_shape() {
    assert!(build_input_bindings(0).is_empty());

    let two = build_input_bindings(2);
    // input0 at 7/8, input1 at 9/10, each with a raw texture, sampler, accessor.
    assert!(two.contains("@binding(7) var input0_tex: texture_2d<f32>;"));
    assert!(two.contains("@binding(8) var input0_sampler: sampler;"));
    assert!(two.contains("fn input0(uv: vec2f) -> vec4f"));
    assert!(two.contains("@binding(9) var input1_tex: texture_2d<f32>;"));
    assert!(two.contains("@binding(10) var input1_sampler: sampler;"));
    assert!(two.contains("fn input1(uv: vec2f) -> vec4f"));

    // The inputs-aware preamble carries the uniform block AND the input decls;
    // input_count 0 is byte-identical to the plain library preamble.
    let loader = EffectLoader::for_test("");
    let frag = "@fragment fn fs_main() -> @location(0) vec4f { return vec4f(0.0); }";
    assert_eq!(
        loader.prepend_library_with_inputs(frag, 0),
        loader.prepend_library(frag),
    );
    let with_one = loader.prepend_library_with_inputs(frag, 1);
    assert!(with_one.contains("PhosphorUniforms"));
    assert!(with_one.contains("fn input0(uv: vec2f) -> vec4f"));
}

fn make_effect(author: &str) -> PfxEffect {
    serde_json::from_str(&format!(
        r#"{{"name":"Test","author":"{}","shader":"test.wgsl"}}"#,
        author
    ))
    .unwrap()
}

#[test]
fn is_builtin_true_for_fosfora_author() {
    assert!(EffectLoader::is_builtin(&make_effect("Fosfora")));
}

#[test]
fn is_builtin_false_for_user_author() {
    assert!(!EffectLoader::is_builtin(&make_effect("User")));
    assert!(!EffectLoader::is_builtin(&make_effect("")));
}

#[test]
fn prepend_library_without_uniforms() {
    let loader = EffectLoader::for_test("// lib code\n");
    let source = "fn main() {}";
    let result = loader.prepend_library(source);
    // Should contain UNIFORM_BLOCK, lib, and source
    assert!(result.contains("PhosphorUniforms"));
    assert!(result.contains("// lib code"));
    assert!(result.contains("fn main() {}"));
}

#[test]
fn prepend_library_with_existing_uniforms() {
    let loader = EffectLoader::for_test("// lib code\n");
    let source = "struct PhosphorUniforms { time: f32 }\nfn main() {}";
    let result = loader.prepend_library(source);
    // Should NOT double-prepend UNIFORM_BLOCK
    let count = result.matches("PhosphorUniforms").count();
    assert_eq!(count, 1); // only the one in source
    assert!(result.contains("// lib code"));
}

// The #1855 regression: a shader that merely *mentions* the struct name in a
// comment used to have its uniform block suppressed, and failed in production
// only ("no definition in scope for identifier: u") because the compile probes
// concatenated the block unconditionally.
#[test]
fn prepend_library_injects_despite_comment_mention() {
    let loader = EffectLoader::for_test("// lib code\n");
    for source in [
        "// reads u.time from PhosphorUniforms\nfn main() {}",
        "/* PhosphorUniforms is injected for us */\nfn main() {}",
        "/* outer /* PhosphorUniforms */ still a comment */\nfn main() {}",
        "fn main() {} // see struct PhosphorUniforms in loader.rs",
    ] {
        assert!(
            !declares_uniform_struct(source),
            "a comment mention must not read as a declaration: {source:?}"
        );
        assert!(
            loader.prepend_library(source).contains("var<uniform> u:"),
            "uniform block must still be injected for: {source:?}"
        );
    }
}

#[test]
fn declares_uniform_struct_matches_real_declarations_only() {
    for src in [
        "struct PhosphorUniforms { time: f32 }",
        "struct   PhosphorUniforms\n{\n  time: f32,\n}",
        "fn main() {}\nstruct PhosphorUniforms {}",
    ] {
        assert!(declares_uniform_struct(src), "should match: {src:?}");
    }
    for src in [
        "struct PhosphorUniformsExtra { time: f32 }", // different type
        "struct MyPhosphorUniforms { time: f32 }",    // different type
        "mystruct PhosphorUniforms {}",               // `struct` not a token
        "structPhosphorUniforms {}",                  // no separator
        "let x = PhosphorUniforms;",                  // a use, not a declaration
        "",
    ] {
        assert!(!declares_uniform_struct(src), "should not match: {src:?}");
    }
}

#[test]
fn strip_wgsl_comments_preserves_code_and_multibyte() {
    // The em dash is multi-byte: byte-oriented stripping must not split it.
    let src = "fn a() {} // drop — this\n/* and\nthis */fn b() {}";
    let stripped = strip_wgsl_comments(src);
    assert!(stripped.contains("fn a() {}"));
    assert!(stripped.contains("fn b() {}"));
    assert!(!stripped.contains("drop"));
    assert!(!stripped.contains("and"));
    // Newlines inside comments survive, so line numbers still line up.
    assert_eq!(src.matches('\n').count(), stripped.matches('\n').count());
}

// Production injects the uniform block, so no shipped shader may declare the
// struct itself — that would be a duplicate declaration at load. This one
// invariant replaces the per-effect `!contains("PhosphorUniforms")` asserts
// that used to work around the #1855 trap, and covers shaders nobody thought
// to add an assert for.
//
// `default.wgsl` is the deliberate exception: it is the reference copy of the
// ABI kept in sync with UNIFORM_BLOCK and gpu/uniforms.rs, and it is the one
// file the suppression branch of prepend_library exists to serve. Asserting
// it *does* declare keeps that branch covered by a real shader.
/// The WGSL `PhosphorUniforms` (both copies: `UNIFORM_BLOCK` and `default.wgsl`) must lay
/// the overlay-clock block out exactly as `gpu::uniforms::ShaderUniforms` does. #81 turned
/// its two pad slots into `tempo_confidence`/`beat_locked`; a WGSL copy that kept the pads,
/// or swapped the two, would read the wrong value with no validation error.
#[test]
fn uniform_block_layout_matches_rust() {
    use crate::gpu::uniforms::ShaderUniforms;
    use std::mem::offset_of;
    let default_wgsl = include_str!("../../../../../assets/shaders/default.wgsl");
    for (name, src) in [
        ("UNIFORM_BLOCK", UNIFORM_BLOCK),
        ("default.wgsl", default_wgsl),
    ] {
        let module = naga::front::wgsl::parse_str(src).expect("parses");
        let (members, span) = module
            .types
            .iter()
            .find_map(|(_, ty)| match &ty.inner {
                naga::TypeInner::Struct { members, span }
                    if ty.name.as_deref() == Some("PhosphorUniforms") =>
                {
                    Some((members.clone(), *span))
                }
                _ => None,
            })
            .expect("declares PhosphorUniforms");
        let offset = |field: &str| {
            members
                .iter()
                .find(|m| m.name.as_deref() == Some(field))
                .unwrap_or_else(|| panic!("{name}: no member {field}"))
                .offset as usize
        };
        assert_eq!(
            span as usize,
            std::mem::size_of::<ShaderUniforms>(),
            "{name} size"
        );
        assert_eq!(
            offset("bar_index"),
            offset_of!(ShaderUniforms, bar_index),
            "{name}"
        );
        assert_eq!(
            offset("beat_index"),
            offset_of!(ShaderUniforms, beat_index),
            "{name}"
        );
        assert_eq!(
            offset("tempo_confidence"),
            offset_of!(ShaderUniforms, tempo_confidence),
            "{name}"
        );
        assert_eq!(
            offset("beat_locked"),
            offset_of!(ShaderUniforms, beat_locked),
            "{name}"
        );
    }
}

#[test]
fn only_default_wgsl_declares_the_uniform_struct() {
    let shaders = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shaders");
    let mut checked = 0;
    let mut saw_default = false;
    for entry in std::fs::read_dir(&shaders).expect("assets/shaders must exist") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("wgsl") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("readable shader");
        let declares = declares_uniform_struct(&src);
        if path.file_name().and_then(|n| n.to_str()) == Some("default.wgsl") {
            assert!(declares, "default.wgsl must keep its reference ABI copy");
            assert!(
                !EffectLoader::for_test("")
                    .prepend_library(&src)
                    .contains(UNIFORM_BLOCK),
                "default.wgsl declares the struct — injection must be suppressed"
            );
            saw_default = true;
        } else {
            assert!(
                !declares,
                "{} declares PhosphorUniforms — production injects it, so this \
                 would be a duplicate declaration at load",
                path.display()
            );
        }
        checked += 1;
    }
    assert!(saw_default, "default.wgsl not found");
    assert!(checked > 40, "only {checked} shaders scanned — path wrong?");
}

// Discovery silently drops a .pfx that fails to deserialize
// (scan_effects_directory warn-logs and moves on), so a schema typo in a
// shipped builtin would just make it vanish from the browser. Parse the
// real file in CI instead.
#[test]
fn tide_pfx_parses_as_builtin() {
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/tide.pfx"))
            .expect("tide.pfx must deserialize");
    assert!(EffectLoader::is_builtin(&effect));
    assert_eq!(effect.inputs.len(), 8); // exactly the 8 compute param slots
    let particles = effect.particles.expect("tide is a particle effect");
    assert_eq!(particles.render_mode, "billboard"); // trails need billboard
    assert!(particles.trail_length >= 2); // ribbon renderer enable gate
    assert!(particles.max_scaled_count <= 300_000); // quality scaler cap
}

// Generic offscreen preview for ANY particle effect, through the production
// ParticleSystem — the repo previously had only splat- and frost-specific
// probes, so a change to a shared sim helper had no cheap before/after.
//
// Renders the particle layer alone (no bg pass, no obstacle, no image
// source), which is exactly the layer where spawn-distribution changes show.
// Prints a per-render SIGNATURE (mean + quadrant means + alive count) so an
// A/B can be diffed numerically instead of by eye.
//
// PARTICLE_PFX=vessel,tesla  selects effects (default: every particle .pfx)
// PARTICLE_PNG_DIR=/path     where PNGs land (default /tmp)
// Run: cargo test -p fosfora-app -- --ignored particle_effect_previews --nocapture
#[test]
#[ignore = "requires a GPU/software adapter; writes PNGs"]
fn particle_effect_previews() {
    use crate::gpu::frame_capture::FrameCapture;
    use crate::gpu::particle::ParticleSystem;

    let out_dir = std::env::var("PARTICLE_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");

    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let libs = format!("{}\n{plib}", probe_libs());

    let wanted: Option<Vec<String>> = std::env::var("PARTICLE_PFX")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect());

    let mut names: Vec<String> = std::fs::read_dir(root.join("effects"))
        .expect("effects dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .collect();
    names.sort();
    if let Some(w) = &wanted {
        names.retain(|n| w.contains(n));
        assert!(!names.is_empty(), "PARTICLE_PFX matched no .pfx: {w:?}");
    }

    let _guard = gpu_guard();
    let (device, queue) = test_gpu();
    let (w, h) = (640u32, 360u32);
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;

    // Silence matters here: Vessel's degenerate emit gate showed up as a
    // fountain that ran with no audio at all, so "idle" is a real probe.
    struct State {
        name: &'static str,
        rms: f32,
        onset_every: u32,
    }
    let states = [
        State {
            name: "idle",
            rms: 0.02,
            onset_every: 0,
        },
        State {
            name: "groove",
            rms: 0.55,
            onset_every: 15,
        },
    ];

    let frames = 120u32;
    let dt = 1.0 / 60.0;
    let mut rendered = 0usize;

    for name in &names {
        let json = std::fs::read_to_string(root.join("effects").join(format!("{name}.pfx")))
            .expect("read .pfx");
        let effect: PfxEffect = serde_json::from_str(&json).expect("parse .pfx");
        let Some(mut def) = effect.particles.clone() else {
            continue;
        };
        if def.compute_shader.is_empty() {
            continue;
        }
        // Splat needs an uploaded cloud to show anything — it has its own probe.
        if def.splat.is_some() {
            continue;
        }
        let sim = match std::fs::read_to_string(root.join("shaders").join(&def.compute_shader)) {
            Ok(s) => s,
            Err(e) => panic!("{name}: missing sim {}: {e}", def.compute_shader),
        };
        // Probe-sized: the 2M-particle effects would dominate runtime and the
        // distribution artefacts are visible far below full count.
        def.max_count = def.max_count.min(200_000);
        def.max_scaled_count = 0;

        // Interaction effects read the spatial hash (group 3), whose grid
        // constants the production loader patches into particle_lib for the
        // actual particle count — mirror that or the pipeline layout mismatches.
        let sim_src = if def.interaction || def.interaction_3d {
            use crate::gpu::particle::spatial_hash::{grid_dims, grid_dims_3d};
            let (gw, gh) = grid_dims(def.max_count, def.grid_max);
            let gd = if def.interaction_3d {
                grid_dims_3d(def.max_count, def.grid_max)
            } else {
                1
            };
            let patched = libs
                .replace(
                    "const SH_GRID_W: u32 = 40u;",
                    &format!("const SH_GRID_W: u32 = {gw}u;"),
                )
                .replace(
                    "const SH_GRID_H: u32 = 40u;",
                    &format!("const SH_GRID_H: u32 = {gh}u;"),
                )
                .replace(
                    "const SH_GRID_D: u32 = 1u;",
                    &format!("const SH_GRID_D: u32 = {gd}u;"),
                );
            format!("{patched}\n{sim}")
        } else {
            format!("{libs}\n{sim}")
        };

        // Param slots 0–7 from the .pfx defaults, so the probe renders each
        // effect as shipped rather than at an arbitrary setting.
        let mut params = [0.0f32; 8];
        for (i, p) in effect.inputs.iter().take(8).enumerate() {
            params[i] = match p {
                crate::params::ParamDef::Float { default, .. } => *default,
                crate::params::ParamDef::Bool { default: true, .. } => 1.0,
                _ => 0.0,
            };
        }

        let mut ps = ParticleSystem::new(&device, &queue, fmt, &def, &sim_src, def.interaction);
        if def.trail_length >= 2 {
            ps.setup_trails(&device, fmt, def.trail_length, def.trail_width);
        }

        // Image emitters need their source SAMPLED before they show anything.
        // ParticleSystem::new does not do it — app.rs does, right after — so
        // without this the probe rendered Raster, Morph, Pegboard, Etch and
        // Lantern as a flat frame and still printed a clean-looking signature,
        // identical for every one of them and identical across audio states.
        // A blank probe that reports success is worse than no probe.
        if def.emitter.shape == "image" && !def.emitter.image.is_empty() {
            let sample_def =
                def.image_sample
                    .clone()
                    .unwrap_or(crate::gpu::particle::types::ImageSampleDef {
                        mode: "grid".to_string(),
                        threshold: 0.1,
                        scale: 1.0,
                    });
            let path = root.join("images").join(&def.emitter.image);
            match crate::gpu::particle::image_source::sample_image(
                &path,
                &sample_def,
                def.max_count,
            ) {
                Ok(aux) => {
                    assert!(!aux.is_empty(), "{name}: image sampled to zero particles");
                    ps.upload_aux_data(&device, &queue, &aux);
                    ps.store_current_aux(aux);
                }
                Err(e) => panic!("{name}: sampling '{}': {e}", def.emitter.image),
            }
        }

        for s in &states {
            for f in 0..frames {
                ps.poll_counter_readback();
                ps.update_uniforms(dt, f as f32 * dt, [w as f32, h as f32], 0.0);
                ps.uniforms.rms = s.rms;
                ps.uniforms.centroid = 0.5;
                ps.uniforms.onset = if s.onset_every > 0 && f % s.onset_every == 0 {
                    0.7
                } else {
                    0.0
                };
                ps.uniforms.beat = if s.onset_every > 0 && f % s.onset_every == 0 {
                    1.0
                } else {
                    0.0
                };
                ps.uniforms.buildup = if s.onset_every > 0 { 0.5 } else { 0.0 };
                ps.uniforms.effect_params = params;

                let is_last = f == frames - 1;
                let mut fc =
                    is_last.then(|| FrameCapture::new(&device, w, h, fmt, "particle-capture"));

                let mut enc = device.create_command_encoder(&Default::default());
                ps.dispatch(&mut enc, &queue);
                let target = fc.as_ref().map(|fc| &fc.view);
                if let Some(view) = target {
                    {
                        let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("particle-preview-bg"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: 0.01,
                                        g: 0.01,
                                        b: 0.015,
                                        a: 1.0,
                                    }),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });
                    }
                    ps.render(&mut enc, &queue, view);
                }
                if let Some(fc) = fc.as_ref() {
                    fc.copy_to_staging(&mut enc);
                }
                queue.submit([enc.finish()]);
                ps.request_counter_readback();
                ps.flip();

                if let Some(fc) = fc.as_mut() {
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                    fc.request_map();
                    let data = loop {
                        device
                            .poll(wgpu::PollType::Wait {
                                submission_index: None,
                                timeout: None,
                            })
                            .unwrap();
                        if let Some(d) = fc.take_mapped_data(&device) {
                            break d;
                        }
                    };
                    // Signature: overall mean plus quadrant means. A spawn
                    // distribution that shifts (clustered → uniform) moves the
                    // quadrant spread even when the overall mean barely budges.
                    let lum = |i: usize| {
                        (data[i] as f64 + data[i + 1] as f64 + data[i + 2] as f64) / 765.0
                    };
                    let mut q = [0f64; 4];
                    let mut qn = [0f64; 4];
                    for y in 0..h as usize {
                        for x in 0..w as usize {
                            let qi =
                                (y >= h as usize / 2) as usize * 2 + (x >= w as usize / 2) as usize;
                            q[qi] += lum((y * w as usize + x) * 4);
                            qn[qi] += 1.0;
                        }
                    }
                    for i in 0..4 {
                        q[i] /= qn[i];
                    }
                    let mean = (q[0] + q[1] + q[2] + q[3]) / 4.0;
                    let path = format!("{out_dir}/{name}_{}.png", s.name);
                    image::RgbaImage::from_raw(w, h, data)
                        .expect("raw->image")
                        .save(&path)
                        .expect("save png");
                    println!(
                        "SIG {name:<14} {:<7} mean={mean:.5} q=[{:.5} {:.5} {:.5} {:.5}]",
                        s.name, q[0], q[1], q[2], q[3]
                    );
                    rendered += 1;
                }
            }
        }
    }

    assert!(rendered > 0, "no particle effects rendered");
    eprintln!("rendered {rendered} previews into {out_dir}");
}

// Image-sourced effects (Raster, Morph, Pegboard, Etch) render NOTHING in
// `particle_effect_previews`: aux is uploaded by the app after a source loads, not by
// `ParticleSystem::new`, so the probe's aux buffer is all zeros and every particle
// takes the `home_color.a < 0.01` early-out. That probe reported the clear colour
// exactly (mean 0.10850, all four quadrants identical) for Raster and Morph for as
// long as they have shipped, which reads as "covered" and is not.
//
// This probe closes that hole: it samples a real built-in image into aux, uploads it,
// and renders the effect's background pass and its particles into the same target so
// the composed picture is what gets captured. The background matters here — Etch draws
// dark ink on light powder, and against the particle probe's near-black clear it would
// be invisible even once the particles worked.
//
// Run: cargo test -p fosfora-app -- --ignored media_effect_previews --nocapture
#[test]
#[ignore = "requires a GPU/software adapter; writes PNGs"]
fn media_effect_previews() {
    use crate::gpu::frame_capture::FrameCapture;
    use crate::gpu::particle::ParticleSystem;
    use crate::gpu::pipeline::ShaderPipeline;
    use crate::gpu::uniforms::{ShaderUniforms, UniformBuffer};

    let out_dir = std::env::var("MEDIA_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");

    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let libs = format!("{}\n{plib}", probe_libs());

    // Every .pfx whose emitter samples an image is in scope.
    let wanted: Option<Vec<String>> = std::env::var("MEDIA_PFX")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect());
    let mut names: Vec<String> = std::fs::read_dir(root.join("effects"))
        .expect("effects dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .collect();
    names.sort();
    if let Some(wnt) = &wanted {
        names.retain(|n| wnt.contains(n));
        assert!(!names.is_empty(), "MEDIA_PFX matched no .pfx: {wnt:?}");
    }

    let _guard = gpu_guard();
    let (device, queue) = test_gpu();
    let (w, h) = (640u32, 360u32);
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;

    let mk_target = |label: &str| {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        (tex, view)
    };

    let mk_audio = |label: &str, format: wgpu::TextureFormat| {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        tex.create_view(&Default::default())
    };
    let waveform = mk_audio("media-waveform", wgpu::TextureFormat::Rg16Float);
    let spectrum = mk_audio("media-spectrum", wgpu::TextureFormat::R16Float);
    let spectrogram = mk_audio("media-spectrogram", wgpu::TextureFormat::R8Unorm);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    // Silence is a real state for these effects: the picture must be legible with no
    // audio at all, because the source image is the subject and audio only animates it.
    struct State {
        name: &'static str,
        rms: f32,
        bass: f32,
        onset_every: u32,
        /// Frame on which `u.drop` pulses, or 0 for never. The A18 drop is one frame
        /// wide, and Etch hangs its whole erase ritual off it — without a state that
        /// fires it, a broken shake-clean would look exactly like a working one.
        drop_at: u32,
    }
    let states = [
        State {
            name: "idle",
            rms: 0.02,
            bass: 0.02,
            onset_every: 0,
            drop_at: 0,
        },
        State {
            name: "groove",
            rms: 0.55,
            bass: 0.6,
            onset_every: 15,
            drop_at: 0,
        },
        State {
            name: "drop",
            rms: 0.55,
            bass: 0.6,
            onset_every: 15,
            drop_at: 200,
        },
    ];

    let frames = 240u32;
    let dt = 1.0 / 60.0;
    let mut rendered = 0usize;

    for name in &names {
        let json = std::fs::read_to_string(root.join("effects").join(format!("{name}.pfx")))
            .expect("read .pfx");
        let effect: PfxEffect = serde_json::from_str(&json).expect("parse .pfx");
        let Some(mut def) = effect.particles.clone() else {
            continue;
        };
        if def.compute_shader.is_empty() || def.emitter.shape != "image" {
            continue;
        }
        // Probe-sized, and not only for runtime: Raster's two million particles come
        // alive on an emit budget that needs about ten seconds to fill, so at full
        // count the capture is the top 40% of the image and nothing else — which reads
        // as a broken effect rather than as a probe that stopped too early.
        def.max_count = def.max_count.min(200_000);
        def.max_scaled_count = 0;

        let sim = std::fs::read_to_string(root.join("shaders").join(&def.compute_shader))
            .unwrap_or_else(|e| panic!("{name}: missing sim {}: {e}", def.compute_shader));
        let sim_src = format!("{libs}\n{sim}");

        // Source aux exactly as the app would: the emitter's built-in image through the
        // production sampler, at the effect's own count and sampling mode. MEDIA_IMAGE
        // points the whole sweep at one file instead — the legibility of these effects
        // depends entirely on the source, so being able to aim them at a photograph
        // rather than at the bundled subject-on-black art is the check that matters.
        let img_path = match std::env::var("MEDIA_IMAGE") {
            Ok(p) => std::path::PathBuf::from(p),
            Err(_) => root.join("images").join(&def.emitter.image),
        };
        let sample_def =
            def.image_sample
                .clone()
                .unwrap_or(crate::gpu::particle::types::ImageSampleDef {
                    mode: "grid".to_string(),
                    threshold: 0.1,
                    scale: 1.0,
                });
        // MEDIA_MODEL aims the same sweep at a 3D model (#1993), which reaches these
        // effects through a render-to-frame rather than a decoder. Worth running both
        // ways: a model's silhouette and tonal distribution are nothing like the
        // bundled art's, and an effect can read well on one and not the other.
        let (src_label, aux) = match std::env::var("MEDIA_MODEL") {
            Ok(m) => {
                let path = std::path::PathBuf::from(m);
                let aux = crate::gpu::particle::model_source::sample_model(
                    &device,
                    &queue,
                    &path,
                    &sample_def,
                    &def.model_sample.clone().unwrap_or_default(),
                    def.max_count,
                )
                .unwrap_or_else(|e| panic!("{name}: sample {}: {e}", path.display()));
                (path.display().to_string(), aux)
            }
            Err(_) => {
                let aux = crate::gpu::particle::image_source::sample_image(
                    &img_path,
                    &sample_def,
                    def.max_count,
                )
                .unwrap_or_else(|e| panic!("{name}: sample {}: {e}", img_path.display()));
                (img_path.display().to_string(), aux)
            }
        };
        assert!(
            !aux.is_empty(),
            "{name}: sampling {src_label} produced no aux — the probe would be blind",
        );

        // Background pass (if the effect has one), built through the production preamble.
        let bg = effect.passes.first().map(|p| {
            let src = std::fs::read_to_string(root.join("shaders").join(&p.shader))
                .unwrap_or_else(|e| panic!("{name}: missing pass {}: {e}", p.shader));
            ShaderPipeline::new(&device, fmt, &probe_preamble(&src), None, 0)
                .unwrap_or_else(|e| panic!("{name}: bg pipeline: {e}"))
        });

        let mut params = [0.0f32; 16];
        for (i, p) in effect.inputs.iter().take(8).enumerate() {
            params[i] = match p {
                crate::params::ParamDef::Float { default, .. } => *default,
                crate::params::ParamDef::Bool { default: true, .. } => 1.0,
                _ => 0.0,
            };
        }
        // MEDIA_PARAMS=7:0,2:0.5 overrides slots by index, so a setting can be checked
        // without editing the .pfx defaults out from under the shipped look.
        if let Ok(spec) = std::env::var("MEDIA_PARAMS") {
            for kv in spec.split(',').filter(|s| !s.trim().is_empty()) {
                let (k, v) = kv.split_once(':').expect("MEDIA_PARAMS wants slot:value");
                let slot: usize = k.trim().parse().expect("MEDIA_PARAMS slot");
                assert!(slot < 8, "MEDIA_PARAMS slot {slot} is out of range");
                params[slot] = v.trim().parse().expect("MEDIA_PARAMS value");
            }
        }

        for s in &states {
            let targets = [mk_target("media-ping"), mk_target("media-pong")];
            let ubuf = UniformBuffer::new(&device);
            let bind_groups: Vec<_> = targets
                .iter()
                .map(|(_, view)| {
                    ubuf.create_bind_group(
                        &device,
                        &bg.as_ref().expect("bg pass").bind_group_layout,
                        view,
                        &sampler,
                        &waveform,
                        &spectrum,
                        &spectrogram,
                        &sampler,
                        &[],
                    )
                })
                .collect();

            let mut ps = ParticleSystem::new(&device, &queue, fmt, &def, &sim_src, def.interaction);
            ps.upload_aux_data(&device, &queue, &aux);

            let mut fu = ShaderUniforms::zeroed();
            fu.resolution = [w as f32, h as f32];
            fu.params = params;

            let mut src = 0usize;
            for f in 0..frames {
                let is_last = f == frames - 1;
                let beat = s.onset_every > 0 && f % s.onset_every == 0;
                let drop = if s.drop_at > 0 && f == s.drop_at {
                    1.0
                } else {
                    0.0
                };

                ps.poll_counter_readback();
                ps.update_uniforms(dt, f as f32 * dt, [w as f32, h as f32], 0.0);
                ps.uniforms.rms = s.rms;
                ps.uniforms.bass = s.bass;
                ps.uniforms.brilliance = s.bass * 0.5;
                ps.uniforms.centroid = 0.5;
                ps.uniforms.onset = if beat { 0.7 } else { 0.0 };
                ps.uniforms.beat = if beat { 1.0 } else { 0.0 };
                ps.uniforms.kick = if beat { 0.6 } else { 0.0 };
                ps.uniforms.drop = drop;
                ps.uniforms.effect_params = [
                    params[0], params[1], params[2], params[3], params[4], params[5], params[6],
                    params[7],
                ];

                fu.time = f as f32 * dt;
                fu.delta_time = dt;
                fu.rms = s.rms;
                fu.bass = s.bass;
                fu.brilliance = s.bass * 0.5;
                fu.beat = if beat { 1.0 } else { 0.0 };
                fu.kick = if beat { 0.6 } else { 0.0 };
                fu.onset = if beat { 0.7 } else { 0.0 };
                fu.drop = drop;
                ubuf.update(&queue, &fu);

                let mut fc =
                    is_last.then(|| FrameCapture::new(&device, w, h, fmt, "media-capture"));
                let dst = 1 - src;
                let target: &wgpu::TextureView = fc.as_ref().map_or(&targets[dst].1, |fc| &fc.view);

                let mut enc = device.create_command_encoder(&Default::default());
                ps.dispatch(&mut enc, &queue);
                {
                    // Background first, reading the previous frame as feedback.
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("media-preview-bg"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: target,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    pass.set_pipeline(&bg.as_ref().expect("bg pass").pipeline);
                    pass.set_bind_group(0, &bind_groups[src], &[]);
                    pass.draw(0..3, 0..1);
                }
                // Particles compose on top of the background, same as production.
                ps.render(&mut enc, &queue, target);
                if let Some(fc) = fc.as_ref() {
                    fc.copy_to_staging(&mut enc);
                }
                queue.submit([enc.finish()]);
                ps.request_counter_readback();
                ps.flip();
                src = dst;

                if let Some(fc) = fc.as_mut() {
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                    fc.request_map();
                    let data = loop {
                        device
                            .poll(wgpu::PollType::Wait {
                                submission_index: None,
                                timeout: None,
                            })
                            .unwrap();
                        if let Some(d) = fc.take_mapped_data(&device) {
                            break d;
                        }
                    };
                    let lum = |i: usize| {
                        (data[i] as f64 + data[i + 1] as f64 + data[i + 2] as f64) / 765.0
                    };
                    // Quadrant means plus a coverage count. Coverage is the metric that
                    // actually fails when an effect draws nothing: a blank frame has a
                    // plausible mean but near-zero spread against its own average.
                    let mut q = [0f64; 4];
                    let mut qn = [0f64; 4];
                    let mut sum = 0f64;
                    let mut sum_sq = 0f64;
                    for y in 0..h as usize {
                        for x in 0..w as usize {
                            let l = lum((y * w as usize + x) * 4);
                            let qi =
                                (y >= h as usize / 2) as usize * 2 + (x >= w as usize / 2) as usize;
                            q[qi] += l;
                            qn[qi] += 1.0;
                            sum += l;
                            sum_sq += l * l;
                        }
                    }
                    for i in 0..4 {
                        q[i] /= qn[i];
                    }
                    let n = (w * h) as f64;
                    let mean = sum / n;
                    let sd = (sum_sq / n - mean * mean).max(0.0).sqrt();
                    let path = format!("{out_dir}/{name}_{}.png", s.name);
                    image::RgbaImage::from_raw(w, h, data)
                        .expect("raw->image")
                        .save(&path)
                        .expect("save png");
                    println!(
                        "SIG {name:<10} {:<7} mean={mean:.5} sd={sd:.5} q=[{:.5} {:.5} {:.5} {:.5}]",
                        s.name, q[0], q[1], q[2], q[3]
                    );
                    // A flat frame is the exact failure this probe exists to catch.
                    assert!(
                        sd > 0.01,
                        "{name} [{}]: frame is flat (sd={sd:.5}) — nothing drew",
                        s.name
                    );
                    rendered += 1;
                }
            }
        }
    }

    assert!(rendered > 0, "no image-sourced effects rendered");
    eprintln!("rendered {rendered} media previews into {out_dir}");
}

// The f32-spacing half of the degenerate-hash finding, provable on the CPU:
// decorrelated draws must NEVER be taken as hash(x), hash(x + 1.0), because
// at realistic seeds the offset rounds away entirely and both calls return
// the same number.
#[test]
fn float_offset_seeds_collapse_at_particle_scale() {
    // seed_base = u.seed + f32(idx) * 17.31, the fosfora/builtin convention,
    // at the 2,000,000 particles those effects actually ship.
    let seed_base = 30_000.0f32 + 2_000_000.0f32 * 17.31;
    assert!(
        seed_base > 33_554_432.0, // 2^25, where the f32 ULP reaches 4.0
        "test premise moved: seed_base = {seed_base}"
    );
    assert_eq!(
        seed_base + 1.0,
        seed_base,
        "the +1.0 offset must be shown to vanish — this is why rand_vec2 \
         returns x == y and why 5-draw emitters collapse to one value"
    );
    assert_eq!(seed_base + 2.0, seed_base);

    // The integer path keeps every draw distinct at the same scale.
    let idx = 2_000_000u32;
    let a = uhash_ref(idx);
    let b = uhash_ref(idx ^ 0x9e37_79b9);
    assert_ne!(a, b, "XOR-salted draws must stay independent");
}

// Mirror of the library's uhash, used by the statistical probes below. The
// test immediately after this one asserts it has not drifted from the WGSL.
fn uhash_ref(x: u32) -> u32 {
    let mut h = x;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h
}

// Guards uhash_ref (and the inline copy in the GPU probe) against drifting
// away from the shipped WGSL, which is the thing actually under test.
#[test]
fn particle_lib_exports_the_integer_hash() {
    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    for needle in [
        "fn uhash(x: u32) -> u32 {",
        "h = h ^ (h >> 16u);",
        "h = h * 0x7feb352du;",
        "h = h ^ (h >> 15u);",
        "h = h * 0x846ca68bu;",
        "fn uhash_f(x: u32) -> f32 {",
        "return f32(uhash(x)) / 4294967296.0;",
    ] {
        assert!(
            plib.contains(needle),
            "particle_lib.wgsl no longer contains `{needle}` — the integer \
             hash moved or changed; update uhash_ref and the GPU probe"
        );
    }
    // The duplicated per-effect copies were folded into the library; a new
    // one creeping back in would be a redefinition error at load, but this
    // catches it at test time with a clearer message.
    let shaders = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shaders");
    for sim in [
        "cleave_sim",
        "tide_sim",
        "ascend_sim",
        "panorama_sim",
        "splat_sim",
    ] {
        let src = std::fs::read_to_string(shaders.join(format!("{sim}.wgsl"))).unwrap();
        assert!(
            !src.contains("fn uhash(x: u32) -> u32 {"),
            "{sim}.wgsl redefines uhash — it comes from particle_lib now"
        );
    }
}

// Statistical probe on the REAL GPU: the integer hash must be uniform and
// decorrelated over a contiguous index band at the magnitudes particle sims
// actually reach. This is the guard on the fix.
//
// Deliberately one-sided: it does NOT assert that fract-sin misbehaves.
// sin() accuracy is a driver/hardware property (lavapipe's is accurate,
// the RTX fast path is not), so demanding the bug reproduce would be flaky.
// The fract-sin numbers are printed for the record instead.
// Run: cargo test -p fosfora-app -- --ignored integer_hash_is_uniform_at_particle_scale
#[test]
#[ignore = "requires a GPU/software adapter"]
fn integer_hash_is_uniform_at_particle_scale() {
    const N: u32 = 65536;
    const BASE: u32 = 1_000_000; // a contiguous band deep in a 2M-particle sim
    const SEED: f32 = 30_000.0; // u.seed is time*1000 % 65536

    let _guard = gpu_guard();
    let (device, queue) = test_gpu();

    let shader = r#"
struct Params { seed: f32, base: u32, pad0: u32, pad1: u32 };
@group(0) @binding(0) var<uniform> pr: Params;
@group(0) @binding(1) var<storage, read_write> out_sin: array<f32>;
@group(0) @binding(2) var<storage, read_write> out_int: array<f32>;

fn uhash(x: u32) -> u32 {
    var h = x;
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return h;
}
fn uhash_f(x: u32) -> f32 { return f32(uhash(x)) / 4294967296.0; }
fn hash(n: f32) -> f32 { return fract(sin(n) * 43758.5453123); }

@compute @workgroup_size(64)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let i = gid.x;
    if i >= arrayLength(&out_sin) { return; }
    let idx = pr.base + i;
    // The exact expression vessel_sim used before the fix.
    out_sin[i] = hash(pr.seed + f32(idx) * 3.7);
    out_int[i] = uhash_f(idx + uhash(u32(pr.seed * 256.0)));
}
"#;

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("hash-probe"),
        source: wgpu::ShaderSource::Wgsl(shader.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("hash-probe"),
        layout: None,
        module: &module,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });

    let bytes = (N as u64) * 4;
    let mk = || {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    };
    let (buf_sin, buf_int) = (mk(), mk());
    let params = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut pbytes = [0u8; 16];
    pbytes[0..4].copy_from_slice(&SEED.to_ne_bytes());
    pbytes[4..8].copy_from_slice(&BASE.to_ne_bytes());
    queue.write_buffer(&params, 0, &pbytes);

    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: buf_sin.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: buf_int.as_entire_binding(),
            },
        ],
    });

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.dispatch_workgroups(N / 64, 1, 1);
    }
    let stage_sin = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let stage_int = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    enc.copy_buffer_to_buffer(&buf_sin, 0, &stage_sin, 0, bytes);
    enc.copy_buffer_to_buffer(&buf_int, 0, &stage_int, 0, bytes);
    queue.submit([enc.finish()]);

    let read = |b: &wgpu::Buffer| -> Vec<f32> {
        b.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        let v = bytemuck::cast_slice::<u8, f32>(&b.slice(..).get_mapped_range()).to_vec();
        b.unmap();
        v
    };
    let sins = read(&stage_sin);
    let ints = read(&stage_int);

    let stats = |v: &[f32]| {
        let n = v.len() as f64;
        let mean = v.iter().map(|&x| x as f64).sum::<f64>() / n;
        let below_05 = v.iter().filter(|&&x| x < 0.05).count() as f64 / n;
        let tiny = v.iter().filter(|&&x| x < 1e-4).count() as f64 / n;
        (mean, below_05, tiny)
    };
    let (sm, sb, st) = stats(&sins);
    let (im, ib, it) = stats(&ints);
    eprintln!(
        "fract-sin: mean={sm:.4} p(<0.05)={sb:.4} p(<1e-4)={st:.6}\n\
         integer  : mean={im:.4} p(<0.05)={ib:.4} p(<1e-4)={it:.6}\n\
         (uniform expects 0.5 / 0.05 / 0.0001)"
    );

    // The integer hash must be uniform at this scale.
    assert!(
        (im - 0.5).abs() < 0.01,
        "integer hash mean should be 0.5, got {im}"
    );
    assert!(
        (ib - 0.05).abs() < 0.01,
        "integer hash should put 5% below 0.05, got {ib}"
    );
    assert!(
        it < 0.001,
        "integer hash near-zero tail should stay ~1e-4, got {it} — this is \
         the band that made gates fire unconditionally"
    );

    // ...and adjacent indices must be independent, since sims hash idx and
    // idx+1 for values that must not correlate.
    let pairs = ints.len() / 2;
    let corr = {
        let (mut sx, mut sy, mut sxy, mut sxx, mut syy) = (0f64, 0f64, 0f64, 0f64, 0f64);
        for i in 0..pairs {
            let (x, y) = (ints[i * 2] as f64, ints[i * 2 + 1] as f64);
            sx += x;
            sy += y;
            sxy += x * y;
            sxx += x * x;
            syy += y * y;
        }
        let n = pairs as f64;
        (sxy - sx * sy / n) / (((sxx - sx * sx / n) * (syy - sy * sy / n)).sqrt())
    };
    assert!(
        corr.abs() < 0.02,
        "adjacent indices should be uncorrelated, got r={corr}"
    );
}

// Sweep: every particle sim a shipped .pfx names, compiled through the
// production compute concatenation. The per-effect probes below cover only
// 8 effects, so a library change (a new helper, a renamed function) could
// break the other dozen sims with nothing failing until launch —
// all_effect_pass_shaders_compile deliberately skips compute sims.
//
// Auto-layout (`layout: None`) is what makes this cheap: pipeline creation
// still forces full validation of bindings and the entry point without the
// harness having to build any particle buffers.
// Run: cargo test -p fosfora-app -- --ignored all_particle_sim_shaders_compile
#[test]
#[ignore = "requires a GPU/software adapter"]
fn all_particle_sim_shaders_compile() {
    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let libs = format!("{}\n{plib}", probe_libs());

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let mut entries: Vec<_> = std::fs::read_dir(root.join("effects"))
        .expect("effects dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
        .collect();
    entries.sort();

    // Several effects share a sim (e.g. the lattice family); compile once each.
    let mut seen = std::collections::BTreeSet::new();
    for path in &entries {
        let json = std::fs::read_to_string(path).expect("read .pfx");
        let effect: PfxEffect = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("{}: bad JSON: {e}", path.display()));
        if let Some(p) = &effect.particles {
            if !p.compute_shader.is_empty() {
                seen.insert(p.compute_shader.clone());
            }
        }
    }
    assert!(
        seen.len() > 15,
        "suspiciously few particle sims found ({}) — did .pfx discovery change?",
        seen.len()
    );

    let _guard = gpu_guard();
    let (device, _queue) = test_gpu();

    let mut failures: Vec<String> = Vec::new();
    for rel in &seen {
        let src_path = root.join("shaders").join(rel);
        let src = match std::fs::read_to_string(&src_path) {
            Ok(s) => s,
            Err(e) => {
                failures.push(format!("missing sim {}: {e}", src_path.display()));
                continue;
            }
        };
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(rel),
            source: wgpu::ShaderSource::Wgsl(format!("{libs}\n{src}").into()),
        });
        let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(rel),
            layout: None,
            module: &module,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(err) = pollster::block_on(device.pop_error_scope()) {
            failures.push(format!("{rel}: {err:?}"));
        }
    }

    eprintln!("compiled {} particle sims", seen.len());
    assert!(
        failures.is_empty(),
        "particle sims failed to compile:\n{}",
        failures.join("\n")
    );
}

/// Every shipped `.pfx`, parsed, in file order; a file that fails to parse is
/// reported as a failure rather than skipped.
fn shipped_pfx_or_failures(failures: &mut Vec<String>) -> Vec<PfxEffect> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/effects");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("effects dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "pfx"))
        .collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|path| {
            let json = std::fs::read_to_string(path).expect("read .pfx");
            serde_json::from_str(&json)
                .map_err(|e| failures.push(format!("{}: bad JSON: {e}", path.display())))
                .ok()
        })
        .collect()
}

// CPU twin of the ignored GPU sweep `all_effect_pass_shaders_compile` (#162):
// every pass of every shipped .pfx, assembled exactly as the app compiles it
// (library + input bindings + fullscreen vertex shader), through naga's WGSL
// front end and validator. Runs in CI with no adapter, so a library change
// that breaks a pass shader fails here rather than at launch.
#[test]
fn all_effect_pass_shaders_validate_on_cpu() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let loader = EffectLoader::for_test(&probe_libs());
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for effect in shipped_pfx_or_failures(&mut failures) {
        for pass in effect.normalized_passes() {
            let src_path = root.join("shaders").join(&pass.shader);
            let Ok(src) = std::fs::read_to_string(&src_path) else {
                failures.push(format!(
                    "{} pass '{}': missing {}",
                    effect.name,
                    pass.name,
                    src_path.display()
                ));
                continue;
            };
            let full = format!(
                "{}\n{}",
                crate::gpu::fullscreen_quad::FULLSCREEN_TRIANGLE_VS,
                loader.prepend_library_with_inputs(&src, pass.input_count())
            );
            if let Err(e) = crate::trama::effect::validate_wgsl(&full) {
                failures.push(format!(
                    "{} pass '{}' ({}): {e}",
                    effect.name, pass.name, pass.shader
                ));
            }
            checked += 1;
        }
    }
    assert!(
        checked > 40,
        "suspiciously few pass shaders found ({checked})"
    );
    assert!(
        failures.is_empty(),
        "{} of {checked} pass shaders failed naga validation:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// naga validation for a particle sim. `trama::effect::validate_wgsl` uses the
/// strictest baseline capabilities, which lack SHADER_FLOAT16_IN_FLOAT32 —
/// the capability behind core WGSL's `unpack2x16float` (splat_sim's packed SH
/// coefficients). wgpu grants it on every Vulkan/Metal/DX12 device, so it is
/// added here rather than failing a shader that runs everywhere we ship.
fn validate_sim_wgsl(src: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(src).map_err(|e| e.emit_to_string(src))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default() | naga::valid::Capabilities::SHADER_FLOAT16_IN_FLOAT32,
    )
    .validate(&module)
    .map(|_| ())
    .map_err(|e| e.emit_to_string(src))
}

// CPU twin of `all_particle_sim_shaders_compile` (#162): every particle sim a
// shipped .pfx names, concatenated as that GPU probe does, through naga.
#[test]
fn all_particle_sim_shaders_validate_on_cpu() {
    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let libs = format!("{}\n{plib}", probe_libs());
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let mut failures = Vec::new();
    let seen: std::collections::BTreeSet<String> = shipped_pfx_or_failures(&mut failures)
        .into_iter()
        .filter_map(|e| e.particles.map(|p| p.compute_shader))
        .filter(|s| !s.is_empty())
        .collect();
    assert!(
        seen.len() > 15,
        "suspiciously few particle sims found ({})",
        seen.len()
    );
    for rel in &seen {
        let src_path = root.join("shaders").join(rel);
        let Ok(src) = std::fs::read_to_string(&src_path) else {
            failures.push(format!("missing sim {}", src_path.display()));
            continue;
        };
        if let Err(e) = validate_sim_wgsl(&format!("{libs}\n{src}")) {
            failures.push(format!("{rel}: {e}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} particle sims failed naga validation:\n{}",
        failures.len(),
        seen.len(),
        failures.join("\n")
    );
}

// Compile probe for the Tide sim + bg shaders through the production
// concatenation (lib_source = noise + palette, then particle_lib for
// compute). Catches WGSL errors without launching the app.
// Run: cargo test -p fosfora-app -- --ignored tide_shaders_compile
#[test]
#[ignore = "requires a GPU/software adapter"]
fn tide_shaders_compile() {
    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let sim = include_str!("../../../../../assets/shaders/tide_sim.wgsl");
    let bg = include_str!("../../../../../assets/shaders/tide_bg.wgsl");

    let _guard = gpu_guard();
    let (device, _queue) = test_gpu();

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let sim_src = format!("{}\n{plib}\n{sim}", probe_libs());
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("tide-sim-probe"),
        source: wgpu::ShaderSource::Wgsl(sim_src.into()),
    });
    // Pipeline creation forces full validation (entry point, bindings).
    let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("tide-sim-probe"),
        layout: None,
        module: &module,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "tide_sim.wgsl failed validation: {err:?}");

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let bg_src = probe_preamble(bg);
    let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("tide-bg-probe"),
        source: wgpu::ShaderSource::Wgsl(bg_src.into()),
    });
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "tide_bg.wgsl failed validation: {err:?}");
}

// Same guard as tide_pfx_parses_as_builtin: discovery silently drops a
// .pfx that fails to deserialize, so parse the real file in CI.
#[test]
fn vessel_pfx_parses_as_builtin() {
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/vessel.pfx"))
            .expect("vessel.pfx must deserialize");
    assert!(EffectLoader::is_builtin(&effect));
    assert_eq!(effect.inputs.len(), 8); // exactly the 8 compute param slots
    let particles = effect.particles.expect("vessel is a particle effect");
    assert_eq!(particles.render_mode, "billboard"); // trails need billboard
    assert!(particles.trail_length >= 2); // ribbon renderer enable gate
    assert!(particles.max_scaled_count <= 300_000); // quality scaler cap
}

// Compile probe for the Vessel sim + bg shaders. Unlike Tide's probe this
// includes the sdf lib in the sim concatenation — Vessel's fallback
// amphora uses fosfora_sd_segment2 (production lib_source is
// noise + palette + sdf + tonemap, see LIBRARY_FILES). Also a
// pre-launch check that the WGSL ParticleUniforms mirror matches the
// Rust layout (896 bytes since the #1800 ABI bump).
// Run: cargo test -p fosfora-app -- --ignored vessel_shaders_compile
#[test]
#[ignore = "requires a GPU/software adapter"]
fn vessel_shaders_compile() {
    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let sim = include_str!("../../../../../assets/shaders/vessel_sim.wgsl");
    let bg = include_str!("../../../../../assets/shaders/vessel_bg.wgsl");

    let _guard = gpu_guard();
    let (device, _queue) = test_gpu();

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let sim_src = format!("{}\n{plib}\n{sim}", probe_libs());
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vessel-sim-probe"),
        source: wgpu::ShaderSource::Wgsl(sim_src.into()),
    });
    // Pipeline creation forces full validation (entry point, bindings).
    let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("vessel-sim-probe"),
        layout: None,
        module: &module,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "vessel_sim.wgsl failed validation: {err:?}");

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let bg_src = probe_preamble(bg);
    let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("vessel-bg-probe"),
        source: wgpu::ShaderSource::Wgsl(bg_src.into()),
    });
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "vessel_bg.wgsl failed validation: {err:?}");
}

// Same guard as tide_pfx_parses_as_builtin: discovery silently drops a
// .pfx that fails to deserialize, so parse the real file in CI.
#[test]
fn cleave_pfx_parses_as_builtin() {
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/cleave.pfx"))
            .expect("cleave.pfx must deserialize");
    assert!(EffectLoader::is_builtin(&effect));
    assert_eq!(effect.inputs.len(), 8); // exactly the 8 compute param slots
    let particles = effect.particles.expect("cleave is a particle effect");
    assert_eq!(particles.render_mode, "billboard"); // trails need billboard
    assert!(particles.trail_length >= 2); // ribbon renderer enable gate
    assert!(particles.max_scaled_count <= 300_000); // quality scaler cap
}

// Compile probe for the Cleave sim + bg shaders (no sdf lib — Cleave uses
// no SDF helpers). Also validates the two-cohort sim's atomicAdd on
// counters[3] (the shard emission sub-budget) against the particle_lib
// binding layout.
// Run: cargo test -p fosfora-app -- --ignored cleave_shaders_compile
#[test]
#[ignore = "requires a GPU/software adapter"]
fn cleave_shaders_compile() {
    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let sim = include_str!("../../../../../assets/shaders/cleave_sim.wgsl");
    let bg = include_str!("../../../../../assets/shaders/cleave_bg.wgsl");

    let _guard = gpu_guard();
    let (device, _queue) = test_gpu();

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let sim_src = format!("{}\n{plib}\n{sim}", probe_libs());
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("cleave-sim-probe"),
        source: wgpu::ShaderSource::Wgsl(sim_src.into()),
    });
    // Pipeline creation forces full validation (entry point, bindings).
    let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("cleave-sim-probe"),
        layout: None,
        module: &module,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "cleave_sim.wgsl failed validation: {err:?}");

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let bg_src = probe_preamble(bg);
    let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("cleave-bg-probe"),
        source: wgpu::ShaderSource::Wgsl(bg_src.into()),
    });
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "cleave_bg.wgsl failed validation: {err:?}");
}

// Helix is a volume effect like Lattice: no compute shader, so
// `particle_effect_previews` skips it and `gpu::helix::helix_render_previews`
// is its probe. What still has to hold here is that the .pfx loads as a
// builtin and its background pass exists — every particle effect needs at
// least one pass, and Helix's whole render is the ray marcher compositing
// over it.
#[test]
fn helix_pfx_parses_as_builtin() {
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/helix.pfx"))
            .expect("helix.pfx must deserialize");
    assert!(EffectLoader::is_builtin(&effect));
    assert_eq!(effect.passes.len(), 1, "Helix needs its background pass");
    // The performance knobs live in `inputs`, not the contextual panel — that
    // is what puts them in the Parameters panel and on the binding bus. Moving
    // one back into the `helix` def block would silently make it unbindable.
    assert_eq!(
        effect.inputs.len(),
        crate::gpu::helix::HELIX_PARAM_NAMES.len()
    );
    let particles = effect.particles.expect("helix is a particle effect");
    assert!(
        particles.helix.is_some(),
        "the helix def block is what turns the effect on"
    );
    assert!(
        particles.compute_shader.is_empty(),
        "Helix renders a volume, not particles — a sim shader would be dead weight"
    );
}

// Compile probe for the Helix background pass through the production
// concatenation. `helix_bg.wgsl` reads `u.resolution`, so it only compiles if
// the uniform block is actually injected — this is what would catch the #1855
// trap turning the backdrop into a load error at runtime.
// Run: cargo test -p fosfora-app -- --ignored helix_shaders_compile
#[test]
#[ignore = "requires a GPU/software adapter"]
fn helix_shaders_compile() {
    let bg = include_str!("../../../../../assets/shaders/helix_bg.wgsl");

    let _guard = gpu_guard();
    let (device, _queue) = test_gpu();

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let src = probe_preamble(bg);
    let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("helix-bg-probe"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "helix_bg.wgsl failed validation: {err:?}");
}

/// The probes compile shaders against an embedded copy of the library set,
/// because they run with no assets directory. That copy has to stay equal to
/// what the app actually prepends: adding one library to `LIB_FILENAMES` once
/// failed seven probes at once with "unknown identifier", because each carried
/// its own hand-written `noise + palette` list. Pin the two together so the
/// next library is a one-line change, not a scavenger hunt.
/// INV-B source lint, running in plain CI (no GPU): every effect declaring
/// `loop: "phase_locked"` must be a pure function of the uniform block —
/// no feedback passes, no previous-frame inputs, no particle system, and no
/// wall-clock uniforms in the (comment-stripped) shader source. The GPU
/// determinism probe (pass_executor.rs) proves bit-identity; this catches
/// violations without an adapter.
#[test]
fn phase_locked_effects_are_pure_functions_of_uniforms() {
    use crate::effect::format::LoopMode;

    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let mut checked = 0usize;
    for effect in shipped_effects_for_test() {
        if effect.loop_mode != LoopMode::PhaseLocked {
            continue;
        }
        checked += 1;
        let name = &effect.name;
        assert!(
            effect.particles.is_none(),
            "{name}: phase_locked effects cannot carry a particle system (state)"
        );
        for pass in effect.normalized_passes() {
            assert!(
                !pass.feedback,
                "{name}/{}: phase_locked forbids feedback passes",
                pass.name
            );
            assert!(
                pass.prev_inputs.is_empty(),
                "{name}/{}: phase_locked forbids previous-frame inputs",
                pass.name
            );
            let src_path = dir.join("shaders").join(&pass.shader);
            let src = std::fs::read_to_string(&src_path)
                .unwrap_or_else(|e| panic!("{name}: cannot read {}: {e}", src_path.display()));
            let code = strip_wgsl_comments(&src);
            // Note this is a denylist over the RAW effect source, so a lib helper
            // that reads wall-clock uniforms hides behind its own name and has to
            // be added here by hand — as frame_decay/frame_gain (#1986) and the
            // chrono_keep pair are below.
            for token in [
                "feedback(",
                "u.time",
                "u.delta_time",
                "u.frame_index",
                "u.scroll_phase",
                "frame_steps(",
                "frame_decay(",
                "frame_decay3(",
                "frame_gain(",
                "chrono_keep",
            ] {
                assert!(
                    !code.contains(token),
                    "{name}/{}: phase_locked forbids `{token}` — all motion must derive \
                     from beat/bar phases and counters",
                    pass.shader
                );
            }
        }
        assert!(
            effect.alpha,
            "{name}: the shipped phase_locked family is the overlay family — alpha: true"
        );
    }
    assert!(
        checked >= 4,
        "expected the four overlay effects to be phase_locked, found {checked} — \
         did the glob or the metadata rot?"
    );
}

/// The override only counts before the first lookup; after it, the resolved
/// directory stays put and the caller gets its path back.
#[test]
fn set_assets_dir_after_first_use_is_refused() {
    let resolved = assets_dir().to_path_buf();
    let late = PathBuf::from("/nonexistent/fosfora-assets");
    assert_eq!(set_assets_dir(late.clone()), Err(late));
    assert_eq!(assets_dir(), resolved);
}

#[test]
fn probe_libs_match_production() {
    let embedded: Vec<&str> = LIB_SOURCES.iter().map(|(name, _)| *name).collect();
    assert_eq!(
        embedded, LIB_FILENAMES,
        "LIB_SOURCES must mirror LIB_FILENAMES exactly, in order"
    );
    let concat = probe_libs();
    for (name, src) in LIB_SOURCES {
        assert!(!src.is_empty(), "{name} embedded empty");
        assert!(
            concat.contains(src.trim()),
            "{name} missing from probe_libs()"
        );
    }
}

/// #1986: the retention helpers must measure trail length in SECONDS, not frames.
/// This mirrors `frame_steps`/`frame_decay`/`frame_gain` from lib/chronoflow.wgsl in
/// Rust so the invariants run in plain CI — the WGSL is the shipping copy, so any
/// change there has to land here too.
#[test]
fn frame_decay_measures_trail_length_in_seconds() {
    fn frame_steps(dt: f32) -> f32 {
        if dt <= 0.0 {
            return 1.0;
        }
        (dt * 60.0).clamp(1e-4, 2.0)
    }
    fn frame_decay(keep60: f32, dt: f32) -> f32 {
        let n = frame_steps(dt);
        if n == 1.0 {
            return keep60;
        }
        keep60.clamp(0.0, 1.0).powf(n)
    }
    fn frame_gain(gain60: f32, keep60: f32, dt: f32) -> f32 {
        let k = keep60.clamp(0.0, 1.0);
        let d = 1.0 - k;
        if d < 1e-5 {
            return gain60 * frame_steps(dt);
        }
        gain60 * (1.0 - frame_decay(k, dt)) / d
    }
    // The per-channel gain (#2376) and the spatial-diffusion rate (#2350).
    // Neither had a Rust mirror until Array/Cascade/Vessel started depending
    // on them for their 60 fps identity.
    fn frame_gain3(gain60: f32, keep60: f32, dt: f32) -> f32 {
        let k = keep60.clamp(0.0, 1.0);
        let d = 1.0 - k;
        let n = frame_steps(dt);
        let decayed = if n == 1.0 { k } else { k.powf(n) };
        if d < 1e-5 {
            return gain60 * n;
        }
        gain60 * (1.0 - decayed) / d
    }
    fn frame_diffuse(rate60: f32, dt: f32) -> f32 {
        let d = rate60.clamp(0.0, 1.0);
        if frame_steps(dt) == 1.0 {
            return d;
        }
        1.0 - frame_decay(1.0 - d, dt)
    }

    // 1. Exact identity at 60 fps, so every shipped look is preserved untouched.
    //    This relies on (1.0f32 / 60.0) * 60.0 being bit-exactly 1.0.
    let dt60 = 1.0f32 / 60.0;
    assert_eq!(
        frame_steps(dt60),
        1.0,
        "(1/60)*60 must be exactly 1.0 in f32"
    );
    for k in [0.72f32, 0.82, 0.90, 0.95, 0.98, 0.999] {
        assert_eq!(
            frame_decay(k, dt60),
            k,
            "60 fps must not change retention {k}"
        );
        assert_eq!(frame_gain(0.5, k, dt60), 0.5, "60 fps must not change gain");
        assert_eq!(
            frame_gain3(0.5, k, dt60),
            0.5,
            "60 fps must not change the per-channel gain"
        );
    }
    // frame_diffuse guards its own identity with an explicit early return
    // rather than evaluating 1-(1-d): that round trip is NOT the identity in
    // f32 (0.12 comes back as 0.12000000476837158) and moved Polycephalum
    // 0.7% at 60 fps before the guard was added.
    for d in [0.0025f32, 0.12, 0.16, 0.4, 1.0] {
        assert_eq!(
            frame_diffuse(d, dt60),
            d,
            "60 fps must not change the diffusion rate {d}"
        );
    }
    // A convergent warp uv' = mix(uv, c, w) composes as 1-(1-w)^n, so the
    // displacement after one wall-clock second must not depend on the frame
    // rate — the invariant Cascade's and Array's centre-pull now rely on.
    for w in [0.0025f32, 0.01, 0.05] {
        let reference = 1.0 - (1.0f32 - w).powi(60);
        for fps in [30u32, 50, 120, 240] {
            let dt = 1.0 / fps as f32;
            let after = 1.0 - (1.0 - frame_diffuse(w, dt)).powi(fps as i32);
            assert!(
                (after / reference - 1.0).abs() < 1e-3,
                "convergent warp w={w} reached {after} after 1 s at {fps} fps, \
                 expected {reference}"
            );
        }
    }

    // 2. The point of the fix: the fraction surviving one WALL-CLOCK SECOND is the
    //    same at every frame rate from 30 up. With a per-frame constant these are
    //    k^30 versus k^240 — a factor of 1e18 at k = 0.82.
    for k in [0.72f32, 0.82, 0.95, 0.98] {
        let reference = k.powi(60);
        for fps in [30u32, 50, 60, 120, 144, 240] {
            let dt = 1.0 / fps as f32;
            let after_one_second = frame_decay(k, dt).powi(fps as i32);
            let rel = (after_one_second - reference).abs() / reference;
            assert!(
                rel < 1e-3,
                "k={k} at {fps} fps retained {after_one_second:e} after 1 s, \
                 expected {reference:e} (relative error {rel:e})"
            );
        }
    }

    // 3. Steady state a/(1-k) is preserved wherever both helpers apply (the linear
    //    blends and Chromatica's additive loop), so plateau brightness holds.
    for k in [0.3f32, 0.72, 0.9, 0.98] {
        for a in [0.5f32, 1.0] {
            let reference = a / (1.0 - k);
            for fps in [30u32, 60, 120, 240] {
                let dt = 1.0 / fps as f32;
                let steady = frame_gain(a, k, dt) / (1.0 - frame_decay(k, dt));
                assert!(
                    (steady / reference - 1.0).abs() < 1e-3,
                    "steady state moved at {fps} fps (k={k}, a={a}): \
                     {steady} vs {reference}"
                );
            }
        }
    }

    // 4. The bound. dt is clamped to 0.05 upstream, so an unbounded exponent reaches
    //    3 and one stalled frame takes 0.82 -> 0.55 — a new single-frame darkening on
    //    exactly the hitches this absorbs, which is what forced the #1983 revert.
    //    Capped at two normal frames, the same frame only reaches 0.672.
    for k in [0.72f32, 0.82, 0.90] {
        let hitched = frame_decay(k, 0.05);
        assert!(
            hitched >= k * k - 1e-6,
            "a hitched frame decayed past the two-frame bound: {k} -> {hitched}"
        );
        assert!(
            hitched > k * k * k,
            "the bound is not actually clamping (unbounded would be {})",
            k * k * k
        );
    }

    // 5. Below 30 fps the bound deliberately wins over exactness: trails run long
    //    rather than flashing dark. Documented here so the trade is not mistaken
    //    for a regression.
    let dt24 = 1.0f32 / 24.0;
    assert_eq!(frame_steps(dt24), 2.0, "24 fps must saturate the bound");

    // 6. Degenerate inputs stay benign.
    assert_eq!(
        frame_decay(0.82, 0.0),
        0.82,
        "dt <= 0 must behave as authored"
    );
    assert_eq!(frame_decay(0.0, 1.0 / 120.0), 0.0, "k = 0 must stay 0");
    assert_eq!(
        frame_gain(1.0, 0.0, 1.0 / 30.0),
        1.0,
        "k = 0 leaves gain alone"
    );
}

/// `polycephalum_diffuse.wgsl` is a standalone pipeline compiled with `layout: None`,
/// so wgpu derives the layout from the shader and a WGSL struct that has drifted from
/// the Rust one still validates — every field past the drift then reads a shifted
/// offset and the trail field quietly misbehaves. Pin the two field lists together.
#[test]
fn trail_field_uniforms_wgsl_matches_rust() {
    use crate::gpu::particle::types::TrailFieldUniforms;
    const WGSL: &str = include_str!("../../../../../assets/shaders/polycephalum_diffuse.wgsl");
    // Mirrors TrailFieldUniforms in gpu/particle/types.rs, in declaration order.
    const EXPECTED: &[&str] = &[
        "grid_w: u32",
        "grid_h: u32",
        "channels: u32",
        "deposit_scale: f32",
        "decay: f32",
        "diffuse: f32",
        "time: f32",
        "delta_time: f32",
    ];

    let src = strip_wgsl_comments(WGSL);
    let start = src
        .find("struct TrailUniforms")
        .expect("TrailUniforms struct not found");
    let end = src[start..].find('}').expect("unterminated TrailUniforms");
    let fields: Vec<String> = src[start..start + end]
        .lines()
        .skip(1)
        .map(|l| {
            l.trim()
                .trim_end_matches(',')
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|l| !l.is_empty())
        .collect();

    assert_eq!(
        fields, EXPECTED,
        "polycephalum_diffuse.wgsl TrailUniforms has drifted from Rust \
         TrailFieldUniforms — update both, or the diffuse pass reads shifted offsets"
    );
    assert_eq!(
        std::mem::size_of::<TrailFieldUniforms>(),
        32,
        "TrailFieldUniforms is documented as 32 bytes"
    );
}

/// #1986: a shader that multiplies its own history by a per-FRAME constant measures
/// trail length in frames, so the same preset is a different effect at 30 fps and at
/// 240. Every shader reading `feedback()` must therefore route its retention through
/// a delta-time-aware helper — `frame_decay`/`frame_decay3`/`frame_gain`, or the
/// older `chrono_keep` pair — unless it is exempted below with the reason why.
///
/// This fires on the NEXT feedback effect someone writes, which is the moment the
/// bug would otherwise come back.
#[test]
fn feedback_shaders_correct_retention_for_frame_time() {
    // Each entry records why the shader has no per-frame retention to correct.
    const EXEMPT: &[(&str, &str)] = &[
        (
            "chronoflow_velocity.wgsl",
            "carries its own pow(0.90, dt * 60.0)",
        ),
        (
            "etch_bg.wgsl",
            "retention is exactly 1.0 by design — nothing decays per frame",
        ),
        (
            "lumen_cascade.wgsl",
            "feedback read for target size only; cascades are recomputed each frame",
        ),
        (
            "lumen_scene.wgsl",
            "feedback read for target size only; never blended",
        ),
        (
            "protea_mass.wgsl",
            "Flow Lenia mass field is conserved, not decayed",
        ),
        (
            "frost_phase.wgsl",
            "an integrator, not an image: adds rate * delta_time, which is already \
             frame-time correct, and nothing decays",
        ),
        (
            "tunnel_phase.wgsl",
            "an integrator, not an image: adds rate * delta_time, which is already \
             frame-time correct, and nothing decays",
        ),
        (
            "sumi_pressure.wgsl",
            "feedback is the previous Jacobi iterate, not frame history",
        ),
        (
            "fluvid_pressure.wgsl",
            "feedback is the previous SOR iterate, not frame history",
        ),
        (
            "fluvid_rd.wgsl",
            "a reaction-diffusion integrator, not an image: nothing decays, and its \
             step size is scaled by frame_steps() so the chemistry advances the same \
             amount per second at any frame rate",
        ),
    ];

    // CARGO_MANIFEST_DIR, not assets_dir(): under `cargo test` the CWD is the package
    // root, so assets_dir() falls through to its CWD-relative fallback.
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shaders");
    let mut checked = 0usize;
    let mut offenders = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "wgsl"))
        .collect();
    entries.sort();

    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let src = strip_wgsl_comments(&std::fs::read_to_string(&path).unwrap());
        if !src.contains("feedback(") {
            continue;
        }
        if let Some((_, why)) = EXEMPT.iter().find(|(n, _)| *n == name) {
            assert!(!why.is_empty(), "{name}: exemption needs a reason");
            continue;
        }
        checked += 1;
        let corrected = [
            "frame_decay(",
            "frame_decay3(",
            "frame_gain(",
            "chrono_keep",
        ]
        .iter()
        .any(|h| src.contains(h));
        if !corrected {
            offenders.push(name);
        }
    }

    assert!(
        offenders.is_empty(),
        "these shaders read feedback() but never correct retention for frame time \
         (#1986) — wrap the per-frame constant in frame_decay()/frame_decay3(), or \
         add an EXEMPT entry saying why none is needed: {offenders:?}"
    );
    assert!(
        checked >= 25,
        "expected the feedback family to be ~27 shaders, found {checked} — has the \
         sweep stopped finding them?"
    );
}

/// Etch's pen lives in a compute shader and its powder lives in a fragment shader, and
/// the two share no buffer — so they agree on when the board is shaken clean only by
/// computing the same function of `u.time` independently. If one copy drifts, the pen
/// starts redrawing before (or long after) the powder re-coats, and nothing fails: the
/// effect just quietly stops erasing properly. Pin the two texts together.
///
/// The helper is deliberately duplicated rather than hoisted into a shared lib: adding
/// a file to `LIB_FILENAMES` couples `assets/` to the binary, and `assets/` is live
/// shared state that every running build reads (#1983).
#[test]
fn etch_clear_cycle_matches() {
    const SIM: &str = include_str!("../../../../../assets/shaders/etch_sim.wgsl");
    const BG: &str = include_str!("../../../../../assets/shaders/etch_bg.wgsl");

    // Extract from the shake-seconds constant through the end of etch_clearing().
    let extract = |src: &str, which: &str| -> String {
        let start = src
            .find("const ETCH_SHAKE_SECS")
            .unwrap_or_else(|| panic!("{which}: ETCH_SHAKE_SECS not found"));
        let body = src
            .find("fn etch_clearing")
            .unwrap_or_else(|| panic!("{which}: etch_clearing not found"));
        let end = src[body..]
            .find("\n}")
            .unwrap_or_else(|| panic!("{which}: etch_clearing has no close"));
        src[start..body + end + 2].split_whitespace().collect()
    };

    assert_eq!(
        extract(SIM, "etch_sim.wgsl"),
        extract(BG, "etch_bg.wgsl"),
        "etch_clearing must be byte-identical in etch_sim.wgsl and etch_bg.wgsl"
    );
}

// Same guard as tide_pfx_parses_as_builtin.
#[test]
fn frost_pfx_parses_as_builtin() {
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/frost.pfx"))
            .expect("frost.pfx must deserialize");
    assert!(EffectLoader::is_builtin(&effect));
    assert_eq!(effect.inputs.len(), 9); // 8 floats + drift Point2D = 10 slots
    assert!(effect.particles.is_none()); // pure fragment + feedback effect
}

// Compile probe for the Frost fragment shader through the production
// concatenation (UNIFORM_BLOCK + noise + palette). Fragment-only effect,
// so no compute-pipeline step.
// Run: cargo test -p fosfora-app -- --ignored frost_shaders_compile
#[test]
#[ignore = "requires a GPU/software adapter"]
fn frost_shaders_compile() {
    // Two passes since #2984: `phase` integrates the wander rate and `main`
    // reads it as input0, so main needs one input binding to compile.
    let loader = EffectLoader::for_test(&probe_libs());
    let sources = [
        (
            "frost_phase.wgsl",
            loader.prepend_library_with_inputs(
                include_str!("../../../../../assets/shaders/frost_phase.wgsl"),
                0,
            ),
        ),
        (
            "frost.wgsl",
            loader.prepend_library_with_inputs(
                include_str!("../../../../../assets/shaders/frost.wgsl"),
                1,
            ),
        ),
    ];

    let _guard = gpu_guard();
    let (device, _queue) = test_gpu();

    for (name, src) in sources {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frost-probe"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "{name} failed validation: {err:?}");
    }
}

// Offscreen render probe for Frost's two material states: run the real
// fragment pipeline with synthetic audio uniforms (tonal vs noisy) through
// 90 feedback frames and capture PNGs. Guards against a black screen or a
// feedback blowout and asserts the crystal and sand states actually differ.
// Run: FROST_PNG_DIR=/path cargo test -p fosfora-app -- --ignored frost_render_previews
#[test]
#[ignore = "requires a GPU/software adapter; writes PNGs"]
fn frost_render_previews() {
    use crate::gpu::frame_capture::FrameCapture;
    use crate::gpu::pipeline::ShaderPipeline;
    use crate::gpu::uniforms::{ShaderUniforms, UniformBuffer};

    let out_dir = std::env::var("FROST_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let _guard = gpu_guard();
    let (device, queue) = test_gpu();

    // Production concatenation: uniform block + libs + effect fragment, with
    // the one input the main pass reads since #2984 — the wander phase.
    let frost = include_str!("../../../../../assets/shaders/frost.wgsl");
    let fragment_source =
        EffectLoader::for_test(&probe_libs()).prepend_library_with_inputs(frost, 1);

    let (w, h) = (960u32, 540u32);
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let pipeline =
        ShaderPipeline::new(&device, fmt, &fragment_source, None, 1).expect("frost pipeline");

    // Ping-pong pair for the feedback loop.
    let mk_target = |label: &str| {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        (tex, view)
    };
    let targets = [mk_target("frost-ping"), mk_target("frost-pong")];

    // 1x1 placeholder audio textures matching the production bindings.
    let mk_audio = |label: &str, format: wgpu::TextureFormat| {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        tex.create_view(&Default::default())
    };
    let waveform = mk_audio("frost-waveform", wgpu::TextureFormat::Rg16Float);
    let spectrum = mk_audio("frost-spectrum", wgpu::TextureFormat::R16Float);
    let spectrogram = mk_audio("frost-spectrogram", wgpu::TextureFormat::R8Unorm);
    // The `phase` pass's output, zeroed: the cells hold still at phase 0.
    // This probe is about the crystal/sand material states, not the wander,
    // which frost_wander_is_integrated_not_multiplied_by_uptime covers
    // through the real two-pass graph.
    let phase = mk_audio("frost-phase", wgpu::TextureFormat::Rgba16Float);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let ubuf = UniformBuffer::new(&device);
    let bind_groups: Vec<_> = targets
        .iter()
        .map(|(_, view)| {
            ubuf.create_bind_group(
                &device,
                &pipeline.bind_group_layout,
                view,
                &sampler,
                &waveform,
                &spectrum,
                &spectrogram,
                &sampler,
                &[(&phase, &sampler)],
            )
        })
        .collect();

    struct ProbeState {
        name: &'static str,
        flatness: f32,
        zcr: f32,
        bandwidth: f32,
        bass: f32,
        centroid: f32,
        rms: f32,
        /// Onset / kick applied on the final (captured) frame only.
        onset: f32,
        kick: f32,
    }
    let state = |name, flatness, zcr, bandwidth, bass, centroid, rms, onset, kick| ProbeState {
        name,
        flatness,
        zcr,
        bandwidth,
        bass,
        centroid,
        rms,
        onset,
        kick,
    };
    let states = [
        state("crystal", 0.05, 0.02, 0.20, 0.30, 0.60, 0.40, 0.0, 0.0),
        state("mid", 0.50, 0.20, 0.45, 0.35, 0.50, 0.45, 0.0, 0.0),
        state("sand", 0.92, 0.45, 0.70, 0.40, 0.40, 0.50, 0.0, 0.0),
        state(
            "crystal_shatter",
            0.05,
            0.02,
            0.20,
            0.30,
            0.60,
            0.40,
            1.0,
            0.8,
        ),
    ];
    let frames = 90u32;
    let dt = 1.0 / 60.0;
    let mut means = std::collections::HashMap::new();

    for s in states {
        let name = s.name;
        let mut u = ShaderUniforms::zeroed();
        u.resolution = [w as f32, h as f32];
        u.delta_time = dt;
        u.feedback_decay = 0.88;
        u.params = [
            0.5, 0.0, 0.5, 0.5, 0.6, 0.6, 0.5, 1.0, 0.0, -0.6, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ];
        u.flatness = s.flatness;
        u.zcr = s.zcr;
        u.bandwidth = s.bandwidth;
        u.sub_bass = s.bass * 0.7;
        u.bass = s.bass;
        u.centroid = s.centroid;
        u.rms = s.rms;

        let mut src = 0usize;
        for f in 0..frames {
            u.time = f as f32 * dt;
            u.frame_index = f as f32;
            u.beat_phase = (u.time * 2.0).fract();
            if f == frames - 1 {
                u.onset = s.onset;
                u.kick = s.kick;
            }
            ubuf.update(&queue, &u);
            let dst = 1 - src;
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("frost-preview-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &targets[dst].1,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&pipeline.pipeline);
                pass.set_bind_group(0, &bind_groups[src], &[]);
                pass.draw(0..3, 0..1);
            }
            queue.submit([enc.finish()]);
            src = dst;
        }

        // Re-render the final frame into the capture target and read it back.
        let mut fc = FrameCapture::new(&device, w, h, fmt, "frost-capture");
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frost-capture-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &fc.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, &bind_groups[1 - src], &[]);
            pass.draw(0..3, 0..1);
        }
        fc.copy_to_staging(&mut enc);
        queue.submit([enc.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        fc.request_map();
        let data = loop {
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            if let Some(d) = fc.take_mapped_data(&device) {
                break d;
            }
        };

        let mean = data.iter().map(|&b| b as f64).sum::<f64>() / (data.len() as f64 * 255.0);
        means.insert(name, mean);
        let path = format!("{out_dir}/frost_{name}.png");
        image::RgbaImage::from_raw(w, h, data)
            .expect("raw->image")
            .save(&path)
            .expect("save png");
        eprintln!("wrote {path} (mean {mean:.4})");

        // Not black, not blown out.
        assert!(mean > 0.005, "{name} rendered near-black (mean {mean:.4})");
        assert!(mean < 0.90, "{name} blew out (mean {mean:.4})");
    }

    // The two material states must actually look different.
    let diff = (means["crystal"] - means["sand"]).abs();
    assert!(
        diff > 0.01,
        "crystal and sand states are indistinguishable (means {:?})",
        means
    );
}

// Same guard as frost_pfx_parses_as_builtin: discovery silently drops a .pfx
// that fails to deserialize.
#[test]
fn chromatica_pfx_parses_as_builtin() {
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/chromatica.pfx"))
            .expect("chromatica.pfx must deserialize");
    assert!(EffectLoader::is_builtin(&effect));
    assert_eq!(effect.inputs.len(), 12); // 12 float params
    assert!(effect.particles.is_none()); // pure fragment + feedback effect
}

// Compile probe for the Chromatica fragment shader through the production
// concatenation. It uses fosfora_sd_segment2, so sdf.wgsl must be in the
// concat (production prepends it via LIB_FILENAMES; the compile probe must too).
// Run: cargo test -p fosfora-app -- --ignored chromatica_shaders_compile
#[test]
#[ignore = "requires a GPU/software adapter"]
fn chromatica_shaders_compile() {
    let chromatica = include_str!("../../../../../assets/shaders/chromatica.wgsl");

    let _guard = gpu_guard();
    let (device, _queue) = test_gpu();

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let src = probe_preamble(chromatica);
    let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("chromatica-probe"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "chromatica.wgsl failed validation: {err:?}");
}

// Same silent-drop guard, for the first true multi-pass pass-graph effect (#1481).
// Also pins the two pieces of infra the solver depends on: the pressure pass's
// Jacobi `iterations` and the cross-pass `prev_inputs` edge that cuts the
// velocity→divergence→pressure→velocity cycle.
#[test]
fn sumi_pfx_parses_as_builtin() {
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/sumi.pfx"))
            .expect("sumi.pfx must deserialize");
    assert!(EffectLoader::is_builtin(&effect));
    assert_eq!(effect.inputs.len(), 10); // 10 float param sliders
    assert!(effect.particles.is_none()); // pure fragment pass-graph effect
    let passes = effect.normalized_passes();
    assert_eq!(passes.len(), 4);
    let pressure = passes.iter().find(|p| p.name == "pressure").unwrap();
    assert_eq!(pressure.iterations, 24);
    assert!(pressure.feedback);
    let divergence = passes.iter().find(|p| p.name == "divergence").unwrap();
    assert!(!divergence.feedback);
    assert_eq!(divergence.prev_inputs, vec!["velocity".to_string()]);
    let velocity = passes.iter().find(|p| p.name == "velocity").unwrap();
    assert_eq!(velocity.inputs, vec!["pressure".to_string()]);
    assert_eq!(velocity.prev_inputs, vec!["dye".to_string()]);

    // Mirror PassExecutor::new's resolver so a mistyped pass name in the .pfx fails
    // in CI, not only in the ignored GPU probe: every `inputs` entry must name an
    // EARLIER pass; every `prev_inputs` entry must name some FEEDBACK pass.
    for (idx, pass) in passes.iter().enumerate() {
        for name in &pass.inputs {
            assert!(
                passes[..idx].iter().any(|p| &p.name == name),
                "pass '{}' input '{name}' must name an earlier pass",
                pass.name
            );
        }
        for name in &pass.prev_inputs {
            assert!(
                passes.iter().any(|p| &p.name == name && p.feedback),
                "pass '{}' prev_input '{name}' must name a feedback pass",
                pass.name
            );
        }
    }
}

// Compile probe for all four Sumi pass shaders through the real pipeline path
// (ShaderPipeline::new = reflection layout + render-pipeline creation), each with
// its production input_count so the injected input0/input1 bindings validate.
// Run: cargo test -p fosfora-app -- --ignored sumi_shaders_compile
#[test]
#[ignore = "requires a GPU/software adapter"]
fn sumi_shaders_compile() {
    use crate::gpu::pipeline::ShaderPipeline;

    let libs = probe_libs();
    let loader = EffectLoader::for_test(&libs);

    let _guard = gpu_guard();
    let (device, _queue) = test_gpu();
    let fmt = wgpu::TextureFormat::Rgba16Float;

    // (name, source, input_count) — velocity reads pressure + prev dye (2).
    let cases = [
        (
            "sumi_divergence",
            include_str!("../../../../../assets/shaders/sumi_divergence.wgsl"),
            1usize,
        ),
        (
            "sumi_pressure",
            include_str!("../../../../../assets/shaders/sumi_pressure.wgsl"),
            1,
        ),
        (
            "sumi_velocity",
            include_str!("../../../../../assets/shaders/sumi_velocity.wgsl"),
            2,
        ),
        (
            "sumi_dye",
            include_str!("../../../../../assets/shaders/sumi_dye.wgsl"),
            1,
        ),
    ];
    for (name, shader, count) in cases {
        let src = loader.prepend_library_with_inputs(shader, count);
        ShaderPipeline::new(&device, fmt, &src, None, count)
            .unwrap_or_else(|e| panic!("{name}.wgsl failed to compile: {e}"));
    }
}

// Offscreen render probe: drive the real fragment pipeline with synthetic
// chroma/key uniforms (C major, A minor, silence, edges-off) through feedback
// frames and capture PNGs. Guards against a black screen or a feedback blowout,
// and asserts a lit chord is brighter than silence and the consonance edges add light.
// Run: CHROMATICA_PNG_DIR=/path cargo test -p fosfora-app --release -- --ignored chromatica_render_previews
#[test]
#[ignore = "requires a GPU/software adapter; writes PNGs"]
fn chromatica_render_previews() {
    use crate::gpu::frame_capture::FrameCapture;
    use crate::gpu::pipeline::ShaderPipeline;
    use crate::gpu::uniforms::{ShaderUniforms, UniformBuffer};

    let out_dir = std::env::var("CHROMATICA_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let _guard = gpu_guard();
    let (device, queue) = test_gpu();

    // Production concatenation: uniform block + libs (incl. sdf) + effect fragment.
    let chromatica = include_str!("../../../../../assets/shaders/chromatica.wgsl");
    let fragment_source = probe_preamble(chromatica);

    let (w, h) = (960u32, 540u32);
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let pipeline =
        ShaderPipeline::new(&device, fmt, &fragment_source, None, 0).expect("chromatica pipeline");

    // Ping-pong pair for the feedback loop.
    let mk_target = |label: &str| {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        (tex, view)
    };
    let targets = [mk_target("chroma-ping"), mk_target("chroma-pong")];

    // 1x1 placeholder audio textures matching the production bindings.
    let mk_audio = |label: &str, format: wgpu::TextureFormat| {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        tex.create_view(&Default::default())
    };
    let waveform = mk_audio("chroma-waveform", wgpu::TextureFormat::Rg16Float);
    let spectrum = mk_audio("chroma-spectrum", wgpu::TextureFormat::R16Float);
    let spectrogram = mk_audio("chroma-spectrogram", wgpu::TextureFormat::R8Unorm);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });

    let ubuf = UniformBuffer::new(&device);
    let bind_groups: Vec<_> = targets
        .iter()
        .map(|(_, view)| {
            ubuf.create_bind_group(
                &device,
                &pipeline.bind_group_layout,
                view,
                &sampler,
                &waveform,
                &spectrum,
                &spectrogram,
                &sampler,
                &[],
            )
        })
        .collect();

    struct ProbeState {
        name: &'static str,
        chroma: [f32; 12],
        key_class: f32,
        key_is_minor: f32,
        key_confidence: f32,
        dominant_chroma: f32,
        edges: f32,
    }
    // Chord chroma vectors (C=0 .. B=11).
    let c_major = {
        let mut c = [0.0f32; 12];
        c[0] = 1.0; // C
        c[4] = 0.85; // E
        c[7] = 0.9; // G
        c
    };
    let a_minor = {
        let mut c = [0.0f32; 12];
        c[9] = 1.0; // A
        c[0] = 0.85; // C
        c[4] = 0.8; // E
        c
    };
    let states = [
        ProbeState {
            name: "c_major",
            chroma: c_major,
            key_class: 0.0,
            key_is_minor: 0.0,
            key_confidence: 0.9,
            dominant_chroma: 0.0,
            edges: 1.0,
        },
        ProbeState {
            name: "a_minor",
            chroma: a_minor,
            key_class: 9.0 / 11.0,
            key_is_minor: 1.0,
            key_confidence: 0.85,
            dominant_chroma: 9.0 / 11.0,
            edges: 1.0,
        },
        ProbeState {
            name: "no_edges",
            chroma: c_major,
            key_class: 0.0,
            key_is_minor: 0.0,
            key_confidence: 0.9,
            dominant_chroma: 0.0,
            edges: 0.0,
        },
        ProbeState {
            name: "silence",
            chroma: [0.0; 12],
            key_class: 0.0,
            key_is_minor: 0.0,
            key_confidence: 0.0,
            dominant_chroma: 0.0,
            edges: 1.0,
        },
    ];
    let frames = 60u32;
    let dt = 1.0 / 60.0;
    let mut means = std::collections::HashMap::new();

    for s in states {
        let name = s.name;
        let mut u = ShaderUniforms::zeroed();
        u.resolution = [w as f32, h as f32];
        u.delta_time = dt;
        u.feedback_decay = 0.88;
        // ring_spacing, bloom_gain, arc_thickness, rotation_speed, palette_shift,
        // minor_droop, feedback_amount, interval_edges, consonance_gain, glow,
        // edge_spin, edge_breath
        u.params = [
            0.4, 0.6, 0.5, 0.4, 0.0, 0.6, 0.5, s.edges, 0.6, 0.6, 0.65, 0.5, 0.0, 0.0, 0.0, 0.0,
        ];
        u.chroma = s.chroma;
        u.key_class = s.key_class;
        u.key_is_minor = s.key_is_minor;
        u.key_confidence = s.key_confidence;
        u.dominant_chroma = s.dominant_chroma;
        u.bandwidth = 0.4;

        let mut src = 0usize;
        for f in 0..frames {
            u.time = f as f32 * dt;
            u.frame_index = f as f32;
            u.beat_phase = (u.time * 2.0).fract();
            ubuf.update(&queue, &u);
            let dst = 1 - src;
            let mut enc = device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("chroma-preview-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &targets[dst].1,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&pipeline.pipeline);
                pass.set_bind_group(0, &bind_groups[src], &[]);
                pass.draw(0..3, 0..1);
            }
            queue.submit([enc.finish()]);
            src = dst;
        }

        // Re-render the final frame into the capture target and read it back.
        let mut fc = FrameCapture::new(&device, w, h, fmt, "chroma-capture");
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("chroma-capture-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &fc.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&pipeline.pipeline);
            pass.set_bind_group(0, &bind_groups[1 - src], &[]);
            pass.draw(0..3, 0..1);
        }
        fc.copy_to_staging(&mut enc);
        queue.submit([enc.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        fc.request_map();
        let data = loop {
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            if let Some(d) = fc.take_mapped_data(&device) {
                break d;
            }
        };

        let mean = data.iter().map(|&b| b as f64).sum::<f64>() / (data.len() as f64 * 255.0);
        means.insert(name, mean);
        let path = format!("{out_dir}/chromatica_{name}.png");
        image::RgbaImage::from_raw(w, h, data)
            .expect("raw->image")
            .save(&path)
            .expect("save png");
        eprintln!("wrote {path} (mean {mean:.4})");

        // Not blown out.
        assert!(mean < 0.90, "{name} blew out (mean {mean:.4})");
    }

    // A lit chord is not black.
    assert!(means["c_major"] > 0.005, "c_major near-black");
    assert!(means["a_minor"] > 0.005, "a_minor near-black");
    // A chord is brighter than silence (the rings bloom).
    assert!(
        means["c_major"] > means["silence"] + 0.002,
        "chord not brighter than silence (means {means:?})"
    );
    // The consonance edges add light: edges-on is brighter than edges-off.
    assert!(
        means["c_major"] > means["no_edges"],
        "consonance edges added no light (means {means:?})"
    );
}

// Same guard as tide_pfx_parses_as_builtin, plus the frozen Splat param
// ABI: the sim reads slots 0–7 by index and the CPU driver reads 8–11
// (app.rs forwards them into splat_ui_params), so count and order are
// load-bearing.
#[test]
fn splat_pfx_parses_as_builtin() {
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/splat.pfx"))
            .expect("splat.pfx must deserialize");
    assert!(EffectLoader::is_builtin(&effect));
    assert_eq!(effect.inputs.len(), 13); // 8 sim slots + 4 CPU camera slots + roundness
    let particles = effect.particles.expect("splat is a particle effect");
    assert_eq!(particles.render_mode, "compute"); // splats need the raster
    assert_eq!(particles.blend, "oit"); // weighted-average OIT resolve
    assert!(particles.trail_length < 2); // trails share group 2 — forbidden
    let splat = particles.splat.expect("splat def block required");
    assert!(splat.source.starts_with("demo:"));
    assert!(particles.max_scaled_count <= 3_000_000); // go/no-go budget
    assert!((particles.emit_rate - 0.0).abs() < f32::EPSILON); // persistent slots, no emission
}

// Compile probe for the Splat sim + bg through the production
// concatenation. The sim declares its own @group(2) @binding(1) splat
// static buffer next to the lib's unconditional @group(2) @binding(0)
// trail declaration — this probe is the pre-launch check that naga
// accepts that coexistence (auto layout only materializes statically
// used bindings) and that the 896-byte uniform mirror still matches.
// Run: cargo test -p fosfora-app -- --ignored splat_shaders_compile
#[test]
#[ignore = "requires a GPU/software adapter"]
fn splat_shaders_compile() {
    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let sim = include_str!("../../../../../assets/shaders/splat_sim.wgsl");
    let bg = include_str!("../../../../../assets/shaders/splat_bg.wgsl");
    let resolve = include_str!("../../../../../assets/shaders/builtin/compute_raster_resolve.wgsl");

    let _guard = gpu_guard();
    let (device, _queue) = test_gpu();

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let sim_src = format!("{}\n{plib}\n{sim}", probe_libs());
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("splat-sim-probe"),
        source: wgpu::ShaderSource::Wgsl(sim_src.into()),
    });
    let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("splat-sim-probe"),
        layout: None,
        module: &module,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "splat_sim.wgsl failed validation: {err:?}");

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let bg_src = probe_preamble(bg);
    let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("splat-bg-probe"),
        source: wgpu::ShaderSource::Wgsl(bg_src.into()),
    });
    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "splat_bg.wgsl failed validation: {err:?}");

    // The patched resolve (OIT mode 2 branch) must still validate.
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("splat-resolve-probe"),
        source: wgpu::ShaderSource::Wgsl(resolve.into()),
    });
    let err = pollster::block_on(device.pop_error_scope());
    assert!(
        err.is_none(),
        "compute_raster_resolve.wgsl failed validation: {err:?}"
    );
}

// Offscreen render probe (frost_render_previews pattern) driving the
// PRODUCTION ParticleSystem: a procedural torus-knot scene (no asset
// download — CI never needs a real capture) uploaded via
// upload_splat_cloud, simulated + rasterized through the "oit" resolve
// for four synthetic audio states, PNGs captured and sanity-asserted.
// A wrapped i32 accumulator (overflow) shows up as garbage colors and
// fails the mean bound; a broken projection/OIT renders black.
// Run: cargo test -p fosfora-app -- --ignored splat_render_previews
// PNGs land in $SPLAT_PNG_DIR (default /tmp).
#[test]
#[ignore = "requires a GPU/software adapter; writes PNGs"]
fn splat_render_previews() {
    use crate::gpu::frame_capture::FrameCapture;
    use crate::gpu::particle::ParticleSystem;
    use crate::gpu::particle::splat::generate_test_scene;

    let out_dir = std::env::var("SPLAT_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let _guard = gpu_guard();
    let (device, queue) = test_gpu();

    // Production sim concatenation + the shipped .pfx def, probe-sized.
    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let sim = include_str!("../../../../../assets/shaders/splat_sim.wgsl");
    let sim_src = format!("{}\n{plib}\n{sim}", probe_libs());
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/splat.pfx")).unwrap();
    let mut def = effect.particles.unwrap();
    // Real-scene override: SPLAT_PLY=/path/to/scene.ply runs the production
    // decode path (parse + cull + normalize) instead of the synthetic
    // torus-knot, so tuning happens against actual capture geometry (the
    // synthetic scene is too thin/front-facing to reproduce dense-figure
    // artefacts). SPLAT_CAM_DIST overrides the orbit radius (default 1.6 =
    // whole figure; smaller = zoom in).
    let ply_path = std::env::var("SPLAT_PLY").ok();
    let env_f = |k: &str, d: f32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(d)
    };
    let cam_dist = env_f("SPLAT_CAM_DIST", 1.6);
    let splat_scale = env_f("SPLAT_SCALE", 1.0);
    let opacity_gain = env_f("SPLAT_OPACITY", 1.0);
    let exposure = env_f("SPLAT_EXPOSURE", 0.33);
    // SPLAT_SORT=0 forces the OIT fallback for an A/B against the sorted path.
    if let Ok(v) = std::env::var("SPLAT_SORT") {
        if let Some(splat) = def.splat.as_mut() {
            splat.sort = v != "0";
        }
    }
    if ply_path.is_some() {
        def.max_count = 1_000_000; // keep every splat of a ~800k capture
    } else {
        // 60k: above TILED_THRESHOLD once alive, so early frames exercise
        // the direct path and steady state exercises the tiled path.
        def.max_count = 60_000;
    }
    def.max_scaled_count = 0;

    // SPLAT_W/SPLAT_H override the probe resolution — the 8px-cap regression
    // only shows at real res (1920×1080), not the 960×540 default.
    let env_u = |k: &str, d: u32| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(d)
    };
    let (w, h) = (env_u("SPLAT_W", 960), env_u("SPLAT_H", 540));
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    // Use the effect's own scene transform (incl. the Y-down→Y-up 180°-X flip)
    // so the offscreen A/B exercises the real load path, not an unrotated one.
    let scene_scale = def.splat.as_ref().map_or(1.0, |s| s.scene_scale);
    // SPLAT_FAR_CLIP overrides the .pfx far-field cull (0 = keep everything).
    let far_clip = def.splat.as_ref().map_or(0.0, |s| s.far_clip);
    // SPLAT_ROT=x,y,z overrides the .pfx Euler offsets — the SH path is the
    // one thing that reads the scene rotation back out (it un-rotates the
    // view direction), so testing it needs the rotation to be a variable.
    let scene_rot = match std::env::var("SPLAT_ROT") {
        Ok(v) => {
            let e: Vec<f32> = v.split(',').filter_map(|c| c.trim().parse().ok()).collect();
            assert_eq!(e.len(), 3, "SPLAT_ROT wants three comma-separated degrees");
            [e[0], e[1], e[2]]
        }
        Err(_) => def
            .splat
            .as_ref()
            .map_or([0.0, 0.0, 0.0], |s| s.rotation_degrees),
    };
    let mut ps = ParticleSystem::new(&device, &queue, fmt, &def, &sim_src, false);
    ps.resize_compute_raster(&device, w, h);
    let mut cloud = if let Some(p) = ply_path.as_ref() {
        use std::sync::atomic::{AtomicBool, AtomicU8};
        let prog = AtomicU8::new(0);
        let cancel = AtomicBool::new(false);
        crate::gpu::particle::splat_source::load_splat_file(
            std::path::Path::new(p),
            1_000_000,
            crate::gpu::particle::splat_source::SceneOptions {
                scene_scale,
                rotation_degrees: scene_rot,
                far_clip: env_f("SPLAT_FAR_CLIP", far_clip),
            },
            &prog,
            &cancel,
        )
        .expect("load SPLAT_PLY")
    } else {
        generate_test_scene(50_000)
    };
    // SPLAT_SH=0 drops a capture to DC only — the A/B that isolates the
    // view-dependent contribution at identical geometry (#1862).
    if std::env::var("SPLAT_SH").is_ok_and(|v| v == "0") {
        cloud.sh_degree = 0;
        cloud.sh = Vec::new();
    }
    eprintln!(
        "scene splats: {} (SH degree {})",
        cloud.count, cloud.sh_degree
    );
    ps.upload_splat_cloud(&device, &queue, &cloud);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("splat-preview-target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    // Stand-in for the bg pass: the plate's base vignette color.
    let bg_clear = wgpu::Color {
        r: 0.02,
        g: 0.022,
        b: 0.032,
        a: 1.0,
    };

    struct ProbeState {
        name: &'static str,
        rms: f32,
        centroid: f32,
        focus: f32,
        onset_every: u32, // 0 = never
        drop_at: u32,     // frame index, u32::MAX = never
    }
    let states = [
        ProbeState {
            name: "idle",
            rms: 0.1,
            centroid: 0.4,
            focus: 0.5,
            onset_every: 0,
            drop_at: u32::MAX,
        },
        ProbeState {
            name: "groove",
            rms: 0.5,
            centroid: 0.5,
            focus: 0.5,
            onset_every: 15,
            drop_at: u32::MAX,
        },
        ProbeState {
            name: "drop_explode",
            rms: 0.6,
            centroid: 0.5,
            focus: 0.5,
            onset_every: 15,
            drop_at: 60,
        },
        ProbeState {
            name: "defocus",
            rms: 0.3,
            centroid: 1.0,
            focus: 1.0,
            onset_every: 0,
            drop_at: u32::MAX,
        },
    ];

    let frames = 90u32;
    let dt = 1.0 / 60.0;
    let mut captures: std::collections::HashMap<&str, Vec<u8>> = std::collections::HashMap::new();

    for s in &states {
        for f in 0..frames {
            ps.poll_counter_readback();
            ps.update_uniforms(dt, f as f32 * dt, [w as f32, h as f32], 0.0);
            ps.uniforms.rms = s.rms;
            ps.uniforms.centroid = s.centroid;
            ps.uniforms.onset = if s.onset_every > 0 && f % s.onset_every == 0 {
                0.7
            } else {
                0.0
            };
            ps.uniforms.drop = if f == s.drop_at { 1.0 } else { 0.0 };
            ps.uniforms.buildup = if s.drop_at != u32::MAX && f < s.drop_at {
                f as f32 / s.drop_at as f32
            } else {
                0.0
            };
            // Frozen param slots 0–7 (sim) — see splat.pfx.
            ps.uniforms.effect_params = [
                0.8,
                0.75,
                0.5,
                s.focus,
                splat_scale,
                opacity_gain,
                0.3,
                exposure,
            ];
            // Slots 8–12 (CPU driver): orbit, distance, pitch, focal bias,
            // roundness. SPLAT_ORBIT=0 + SPLAT_YAW/SPLAT_PITCH freezes a
            // viewing angle; SPLAT_ROUNDNESS morphs shard→sphere.
            ps.splat_ui_params = [
                env_f("SPLAT_ORBIT", 0.3),
                cam_dist,
                env_f("SPLAT_PITCH", 0.15),
                0.0,
                env_f("SPLAT_ROUNDNESS", 0.0),
            ];
            ps.update_splat_driver();
            if std::env::var("SPLAT_YAW").is_ok() {
                ps.uniforms.cam_yaw = env_f("SPLAT_YAW", 0.0);
            }

            // On the final frame render into the capture target instead
            // (its texture is RENDER_ATTACHMENT | COPY_SRC, not COPY_DST).
            let is_last = f == frames - 1;
            let mut fc = is_last.then(|| FrameCapture::new(&device, w, h, fmt, "splat-capture"));
            let frame_view = fc.as_ref().map_or(&target_view, |fc| &fc.view);

            let mut enc = device.create_command_encoder(&Default::default());
            ps.dispatch(&mut enc, &queue);
            {
                // Clear to the bg color; ps.render composites (LoadOp::Load).
                let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("splat-preview-bg"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: frame_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(bg_clear),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            }
            ps.render(&mut enc, &queue, frame_view);
            if let Some(fc) = fc.as_ref() {
                fc.copy_to_staging(&mut enc);
            }
            queue.submit([enc.finish()]);
            ps.request_counter_readback();
            ps.flip();

            if let Some(fc) = fc.as_mut() {
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                fc.request_map();
                let data = loop {
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                    if let Some(d) = fc.take_mapped_data(&device) {
                        break d;
                    }
                };
                let mean =
                    data.iter().map(|&b| b as f64).sum::<f64>() / (data.len() as f64 * 255.0);
                let path = format!("{out_dir}/splat_{}.png", s.name);
                image::RgbaImage::from_raw(w, h, data.clone())
                    .expect("raw->image")
                    .save(&path)
                    .expect("save png");
                eprintln!("wrote {path} (mean {mean:.4})");

                // Sanity guards apply to the synthetic scene only; a real
                // SPLAT_PLY render (possibly zoomed) legitimately fills the
                // corner and varies in mean — it is a debug capture.
                if ply_path.is_none() {
                    // Not black (scene visible), not blown out (an i32
                    // accumulator wrap reads as garbage brightness).
                    assert!(
                        mean > 0.004,
                        "{} rendered near-black (mean {mean:.4})",
                        s.name
                    );
                    assert!(mean < 0.90, "{} blew out (mean {mean:.4})", s.name);
                    // Background must show through empty space: the scene is
                    // centered, so the top-left corner is bg-only (the 0.02
                    // linear clear ≈ 40/255 in sRGB — allow slack).
                    assert!(
                        data[0] < 70 && data[1] < 70 && data[2] < 70,
                        "{}: corner not background ({:?})",
                        s.name,
                        &data[0..4]
                    );
                }
                captures.insert(s.name, data);
            }
        }
    }

    // The drop must visibly shatter the scene vs. the same state without
    // it (mean absolute per-pixel difference). Synthetic scene only.
    if ply_path.is_none() {
        let a = &captures["groove"];
        let b = &captures["drop_explode"];
        let diff = a
            .iter()
            .zip(b.iter())
            .map(|(&x, &y)| (x as f64 - y as f64).abs())
            .sum::<f64>()
            / (a.len() as f64 * 255.0);
        assert!(
            diff > 0.003,
            "drop_explode is indistinguishable from groove (mean |Δ| {diff:.5})"
        );
    }
}

// Headless wall-clock perf run for the #1800 go/no-go gate (≥60 FPS at
// 1–3M splats): 600 frames of the production dispatch+raster+resolve at
// 1080p, GPU-bound via a blocking poll per frame. Reports mean / p99
// frame time. Splat count via SPLAT_PERF_COUNT (default 1_000_000).
// Run: SPLAT_PERF_COUNT=3000000 cargo test -p fosfora-app --release -- --ignored --nocapture splat_perf_600_frames
#[test]
#[ignore = "requires a GPU; perf measurement, run --release"]
fn splat_perf_600_frames() {
    use crate::gpu::particle::ParticleSystem;
    use crate::gpu::particle::splat::generate_test_scene;

    let count: u32 = std::env::var("SPLAT_PERF_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);

    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let sim = include_str!("../../../../../assets/shaders/splat_sim.wgsl");
    let sim_src = format!("{}\n{plib}\n{sim}", probe_libs());
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/splat.pfx")).unwrap();
    let mut def = effect.particles.unwrap();
    def.max_count = count;
    def.max_scaled_count = 0;

    let _guard = gpu_guard();
    let (device, queue) = test_gpu();

    let (w, h) = (1920u32, 1080u32);
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut ps = ParticleSystem::new(&device, &queue, fmt, &def, &sim_src, false);
    ps.resize_compute_raster(&device, w, h);
    // SPLAT_PLY measures a REAL capture instead of the procedural knot —
    // needed for the two things the synthetic scene cannot show: the cost of
    // view-dependent SH (the knot has none) and the close-zoom overdraw the
    // sorted path's 1024 px radius cap allows (#1862).
    let cloud = match std::env::var("SPLAT_PLY") {
        Ok(p) => {
            use std::sync::atomic::{AtomicBool, AtomicU8};
            eprintln!("loading {p}…");
            let (prog, cancel) = (AtomicU8::new(0), AtomicBool::new(false));
            let scene_rot = def
                .splat
                .as_ref()
                .map_or([0.0, 0.0, 0.0], |s| s.rotation_degrees);
            crate::gpu::particle::splat_source::load_splat_file(
                std::path::Path::new(&p),
                count,
                crate::gpu::particle::splat_source::SceneOptions {
                    rotation_degrees: scene_rot,
                    far_clip: def.splat.as_ref().map_or(0.0, |s| s.far_clip),
                    ..Default::default()
                },
                &prog,
                &cancel,
            )
            .expect("load SPLAT_PLY")
        }
        Err(_) => {
            eprintln!("generating {count} procedural splats…");
            generate_test_scene(count as usize)
        }
    };
    if std::env::var("SPLAT_SH").is_ok_and(|v| v == "0") {
        // A/B the SH evaluation cost at identical geometry.
        let mut c = cloud;
        c.sh_degree = 0;
        c.sh = Vec::new();
        ps.upload_splat_cloud(&device, &queue, &c);
    } else {
        ps.upload_splat_cloud(&device, &queue, &cloud);
    }

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("splat-perf-target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: fmt,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());

    let frames = 600u32;
    let dt = 1.0 / 60.0;
    let mut times_ms: Vec<f64> = Vec::with_capacity(frames as usize);
    for f in 0..frames {
        ps.poll_counter_readback();
        ps.update_uniforms(dt, f as f32 * dt, [w as f32, h as f32], 0.0);
        ps.uniforms.rms = 0.5;
        ps.uniforms.onset = if f % 20 == 0 { 0.7 } else { 0.0 };
        ps.uniforms.drop = if f % 240 == 100 { 1.0 } else { 0.0 }; // periodic worst-case explode
        let scale_ov: f32 = std::env::var("SPLAT_PERF_SCALE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);
        let focus_ov: f32 = std::env::var("SPLAT_PERF_FOCUS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.5);
        ps.uniforms.effect_params = [0.8, 0.75, 0.5, focus_ov, scale_ov, 1.0, 0.3, 0.33];
        // SPLAT_CAM_DIST < 1.6 zooms in — the r_cap overdraw stress case.
        let dist: f32 = std::env::var("SPLAT_CAM_DIST")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.6);
        ps.splat_ui_params = [0.3, dist, 0.15, 0.0, 0.0];
        ps.update_splat_driver();

        let t0 = std::time::Instant::now();
        let mut enc = device.create_command_encoder(&Default::default());
        ps.dispatch(&mut enc, &queue);
        ps.render(&mut enc, &queue, &view);
        queue.submit([enc.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        ps.request_counter_readback();
        ps.flip();
        if f >= 30 {
            times_ms.push(ms); // skip warm-up (pipeline compiles, first tiled frames)
        }
        if f % 100 == 0 {
            eprintln!("  frame {f}: {ms:.2} ms, alive {}", ps.alive_count);
        }
    }
    times_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = times_ms.iter().sum::<f64>() / times_ms.len() as f64;
    let p99 = times_ms[(times_ms.len() as f64 * 0.99) as usize - 1];
    let max = times_ms.last().unwrap();
    eprintln!(
        "splat perf @ {count} splats, 1080p: mean {mean:.2} ms ({:.0} FPS), p99 {p99:.2} ms, max {max:.2} ms",
        1000.0 / mean
    );
}

// Panorama + Ascend (#1801): the two MIR-pack effects. Same guard the
// other particle effects carry — the .pfx must parse as a builtin.
#[test]
fn panorama_pfx_parses_as_builtin() {
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/panorama.pfx"))
            .expect("panorama.pfx must deserialize");
    assert!(EffectLoader::is_builtin(&effect));
    assert_eq!(effect.inputs.len(), 8);
    let particles = effect.particles.expect("panorama is a particle effect");
    assert_eq!(particles.compute_shader, "panorama_sim.wgsl");
}

#[test]
fn ascend_pfx_parses_as_builtin() {
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/ascend.pfx"))
            .expect("ascend.pfx must deserialize");
    assert!(EffectLoader::is_builtin(&effect));
    assert_eq!(effect.inputs.len(), 8);
    let particles = effect.particles.expect("ascend is a particle effect");
    assert_eq!(particles.compute_shader, "ascend_sim.wgsl");
}

/// Compile probe for both #1801 effects through the production concatenation.
/// Run: cargo test -p fosfora-app -- --ignored mir_pack_shaders_compile
#[test]
#[ignore = "requires a GPU/software adapter"]
fn mir_pack_shaders_compile() {
    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");

    let _guard = gpu_guard();
    let (device, _queue) = test_gpu();

    let sims = [
        (
            "panorama_sim.wgsl",
            include_str!("../../../../../assets/shaders/panorama_sim.wgsl"),
        ),
        (
            "ascend_sim.wgsl",
            include_str!("../../../../../assets/shaders/ascend_sim.wgsl"),
        ),
    ];
    for (name, sim) in sims {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(name),
            source: wgpu::ShaderSource::Wgsl(format!("{}\n{plib}\n{sim}", probe_libs()).into()),
        });
        // Pipeline creation forces full validation (entry point, bindings).
        let _ = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(name),
            layout: None,
            module: &module,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "{name} failed validation: {err:?}");
    }

    let bgs = [
        (
            "panorama_bg.wgsl",
            include_str!("../../../../../assets/shaders/panorama_bg.wgsl"),
        ),
        (
            "ascend_bg.wgsl",
            include_str!("../../../../../assets/shaders/ascend_bg.wgsl"),
        ),
    ];
    for (name, bg) in bgs {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(name),
            source: wgpu::ShaderSource::Wgsl(probe_preamble(bg).into()),
        });
        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "{name} failed validation: {err:?}");
    }
}

/// Offscreen render of the reworked Ascend ridgeline (#1441) under a few
/// synthetic spectra, so the terrain shape can be eyeballed without the app.
/// Renders only the particle pass (no bg feedback / bloom) — enough to read
/// the ridge silhouette and confirm the bands sculpt it. Writes PNGs to
/// ASCEND_PNG_DIR (default /tmp); the asserts only guard not-black / not-blown.
/// Run: ASCEND_PNG_DIR=/some/dir cargo test -p fosfora-app --release -- --ignored ascend_render_previews
#[test]
#[ignore = "requires a GPU/software adapter; writes PNGs"]
fn ascend_render_previews() {
    use crate::audio::features::AudioFeatures;
    use crate::gpu::frame_capture::FrameCapture;
    use crate::gpu::particle::ParticleSystem;

    let out_dir = std::env::var("ASCEND_PNG_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let _guard = gpu_guard();
    let (device, queue) = test_gpu();

    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let sim = include_str!("../../../../../assets/shaders/ascend_sim.wgsl");
    let sim_src = format!("{}\n{plib}\n{sim}", probe_libs());
    let effect: PfxEffect =
        serde_json::from_str(include_str!("../../../../../assets/effects/ascend.pfx")).unwrap();
    let mut def = effect.particles.unwrap();
    def.max_scaled_count = 0; // keep max_count as authored

    // The 8 sim params in .pfx order (altitude, relief, shimmer, flow, hue,
    // glow, baseline, trail_decay).
    let params: [f32; 8] = [0.7, 0.6, 0.45, 0.4, 0.5, 0.55, 0.3, 0.84];

    let (w, h) = (960u32, 540u32);
    let fmt = wgpu::TextureFormat::Rgba8UnormSrgb;

    // Each scene is a band spectrum + brightness, chosen to show the ridge
    // respond: bass sinks it low-left, bright lifts a right-leaning range,
    // full raises the whole massif.
    let bands = |v: [f32; 7]| v;
    let scenes: [(&str, [f32; 7], f32, f32); 3] = [
        // name, [sub_bass..brilliance], rolloff, rms
        (
            "bass",
            bands([0.85, 0.7, 0.35, 0.15, 0.08, 0.05, 0.03]),
            0.18,
            0.6,
        ),
        (
            "bright",
            bands([0.05, 0.1, 0.2, 0.35, 0.55, 0.8, 0.9]),
            0.85,
            0.6,
        ),
        (
            "full",
            bands([0.5, 0.55, 0.5, 0.6, 0.5, 0.55, 0.5]),
            0.5,
            0.7,
        ),
    ];

    for (name, b, rolloff, rms) in scenes {
        let mut ps = ParticleSystem::new(&device, &queue, fmt, &def, &sim_src, false);

        let feat = AudioFeatures {
            sub_bass: b[0],
            bass: b[1],
            low_mid: b[2],
            mid: b[3],
            upper_mid: b[4],
            presence: b[5],
            brilliance: b[6],
            rolloff,
            rms,
            bandwidth: 0.4,
            centroid: rolloff,
            zcr: 0.2,
            ..Default::default()
        };

        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ascend-preview-target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: fmt,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&Default::default());

        // ~2.5 s of frames at 60 Hz so the field fully populates and settles
        // onto the ridge (lifetime is 3 s).
        let frames = 150u32;
        for f in 0..frames {
            let time = f as f32 / 60.0;
            ps.update_uniforms(1.0 / 60.0, time, [w as f32, h as f32], 0.0);
            ps.update_audio(&feat);
            ps.uniforms.effect_params = params;

            let is_last = f == frames - 1;
            let mut fc = is_last.then(|| FrameCapture::new(&device, w, h, fmt, "ascend-capture"));
            let frame_view = fc.as_ref().map_or(&target_view, |fc| &fc.view);

            let mut enc = device.create_command_encoder(&Default::default());
            ps.dispatch(&mut enc, &queue);
            {
                // Clear to near-black; ps.render composites additively (LoadOp::Load).
                let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ascend-preview-clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: frame_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.01,
                                g: 0.01,
                                b: 0.02,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            }
            ps.render(&mut enc, &queue, frame_view);
            if let Some(fc) = fc.as_ref() {
                fc.copy_to_staging(&mut enc);
            }
            queue.submit([enc.finish()]);
            ps.flip();

            if let Some(fc) = fc.as_mut() {
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                fc.request_map();
                let data = loop {
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                    if let Some(d) = fc.take_mapped_data(&device) {
                        break d;
                    }
                };
                let mean =
                    data.iter().map(|&x| x as f64).sum::<f64>() / (data.len() as f64 * 255.0);
                let path = format!("{out_dir}/ascend_{name}.png");
                image::RgbaImage::from_raw(w, h, data.clone())
                    .expect("raw->image")
                    .save(&path)
                    .expect("save png");
                eprintln!("wrote {path} (mean {mean:.4})");
                assert!(mean > 0.001, "{name} rendered near-black (mean {mean:.4})");
                assert!(mean < 0.80, "{name} blew out (mean {mean:.4})");
            }
        }
    }
}

/// Proves the Rust `ParticleUniforms` and the WGSL mirror in `particle_lib.wgsl` agree at the
/// **field** level, not just in total size.
///
/// The `*_shaders_compile` probes above all create pipelines with `layout: None`, so wgpu
/// derives the layout *from the shader* — a WGSL struct that has drifted smaller than the Rust
/// one still validates, and every sim then reads shifted offsets. That is the failure mode the
/// A13b bump (896 -> 944 B, #1801) could introduce silently, and Panorama is about to read
/// `band_pan` directly.
///
/// Writes a distinctive value into one field per block, runs a real dispatch that copies them
/// out through the production `particle_lib` accessors, and reads them back. `splat_sh_degree`
/// is asserted alongside the new fields specifically to pin the "append, never insert"
/// invariant (#1505): if the new tail had been spliced in mid-struct, it would move.
///
/// Run: cargo test -p fosfora-app -- --ignored particle_uniforms_wgsl_layout_matches_rust
#[test]
#[ignore = "requires a GPU/software adapter"]
fn particle_uniforms_wgsl_layout_matches_rust() {
    use crate::gpu::particle::types::ParticleUniforms;
    use wgpu::util::DeviceExt;

    // Production concatenation order — particle_lib calls into noise.wgsl.
    let plib = include_str!("../../../../../assets/shaders/lib/particle_lib.wgsl");
    let probe = r#"
@group(0) @binding(1) var<storage, read_write> out: array<f32>;
@compute @workgroup_size(1)
fn cs_main() {
    out[0] = u.delta_time;       // first block — must not have moved
    out[1] = u.seed;             // mid-struct anchor
    out[2] = u.splat_sh_degree;  // last pre-A13b field — pins "append, never insert"
    out[3] = u.pan;
    out[4] = u.stereo_width;
    out[5] = u.stereo_corr;
    for (var i = 0u; i < 7u; i = i + 1u) {
        out[6u + i] = band_pan(i);
    }
    // All 16 effect params through the production accessor: 8..15 are the
    // #2984 tail, appended after trail_steps.
    for (var i = 0u; i < 16u; i = i + 1u) {
        out[13u + i] = param(i);
    }
}
"#;
    const N: usize = 29;

    let _guard = gpu_guard();
    let (device, queue) = test_gpu();

    // Distinctive, unequal values so a shifted read cannot coincidentally match.
    let mut u: ParticleUniforms = bytemuck::Zeroable::zeroed();
    u.delta_time = 0.125;
    u.seed = 0.375;
    u.splat_sh_degree = 3.0;
    u.pan = 0.25;
    u.stereo_width = 0.75;
    u.stereo_corr = 0.125;
    u.band_pan = [0.11, 0.22, 0.33, 0.44, 0.55, 0.66, 0.77, 0.0];
    u.effect_params = [1.5, 2.5, 3.5, 4.5, 5.5, 6.5, 7.5, 8.5];
    u.effect_params_hi = [9.25, 10.25, 11.25, 12.25, 13.25, 14.25, 15.25, 16.25];

    let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("probe-uniforms"),
        contents: bytemuck::bytes_of(&u),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let obuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe-out"),
        size: (N * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let rbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe-readback"),
        size: (N * 4) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("uniform-layout-probe"),
        source: wgpu::ShaderSource::Wgsl(format!("{}\n{plib}\n{probe}", probe_libs()).into()),
    });
    // `layout: None` is fine here: the bind group below supplies the *real* Rust-sized buffer,
    // so wgpu checks it against the minimum binding size the WGSL struct implies.
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("uniform-layout-probe"),
        layout: None,
        module: &module,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("uniform-layout-probe"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: ubuf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: obuf.as_entire_binding(),
            },
        ],
    });

    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bg, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    enc.copy_buffer_to_buffer(&obuf, 0, &rbuf, 0, (N * 4) as u64);
    queue.submit([enc.finish()]);

    rbuf.slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    let got: Vec<f32> = bytemuck::cast_slice(&rbuf.slice(..).get_mapped_range()).to_vec();

    let expect = [
        ("delta_time", u.delta_time),
        ("seed", u.seed),
        ("splat_sh_degree", u.splat_sh_degree),
        ("pan", u.pan),
        ("stereo_width", u.stereo_width),
        ("stereo_corr", u.stereo_corr),
    ];
    for (i, (name, want)) in expect.iter().enumerate() {
        assert_eq!(
            got[i],
            *want,
            "{name}: WGSL read {} but Rust wrote {want} — particle_lib.wgsl has drifted from \
             ParticleUniforms ({}-byte struct)",
            got[i],
            std::mem::size_of::<ParticleUniforms>()
        );
    }
    for i in 0..7 {
        assert_eq!(
            got[6 + i],
            u.band_pan[i],
            "band_pan({i}): WGSL read {} but Rust wrote {}",
            got[6 + i],
            u.band_pan[i]
        );
    }
    for i in 0..16 {
        let want = if i < 8 {
            u.effect_params[i]
        } else {
            u.effect_params_hi[i - 8]
        };
        assert_eq!(
            got[13 + i],
            want,
            "param({i}): WGSL read {} but Rust wrote {want} — a particle sim would \
             read the wrong effect param",
            got[13 + i]
        );
    }
}

// Every shipped .pfx render pass, through the production concatenation,
// validated on the CPU against BASELINE WebGPU capabilities. The GPU probes
// cannot catch a capability the dev machine happens to have: #2984 put
// quantizeToF16 (SHADER_FLOAT16_IN_FLOAT32) into a lib prepended to every
// effect, every GPU test passed on the RTX, and only trama's equivalent of
// this test noticed — on a device without it, no effect would compile.
#[test]
fn shipped_pfx_passes_validate_at_baseline_capabilities() {
    use crate::gpu::fullscreen_quad::FULLSCREEN_TRIANGLE_VS;
    let loader = EffectLoader::for_test(&probe_libs());
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shaders");
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for effect in shipped_effects_for_test() {
        for pass in effect.normalized_passes() {
            let src = std::fs::read_to_string(root.join(&pass.shader))
                .unwrap_or_else(|e| panic!("{}: {}: {e}", effect.name, pass.shader));
            let fragment = loader.prepend_library_with_inputs(&src, pass.input_count());
            let full = format!("{FULLSCREEN_TRIANGLE_VS}\n{fragment}");
            if let Err(e) = crate::trama::effect::validate_wgsl(&full) {
                let first = e
                    .to_string()
                    .lines()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(" | ");
                failures.push(format!("{} / {}: {first}", effect.name, pass.shader));
            }
            checked += 1;
        }
    }
    assert!(checked > 50, "only {checked} passes found");
    assert!(
        failures.is_empty(),
        "{} pass(es) need a capability baseline WebGPU does not promise:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// #2984: Etch clears its board on a clock, `fract(t / clear_cycle)`, whose
// rate is 1 / clear_cycle — so moving the clear-cycle fader at a large
// uptime moved the phase by uptime x the change in 1 / clear_cycle, sweeping
// it through the shake window again and again. The defect is a COUNT of
// clears, not a picture drifting, which the generic rates probe cannot see:
// at t=300 it reads the nudge as smaller than motion (a clear switched off).
//
// So this compiles the REAL etch_clearing (extracted from etch_bg.wgsl the
// way etch_clear_cycle_matches pins it) and counts clear onsets during a
// 3 s drag of the fader from 12 s to 24 s at a 300 s clock, fed two ways:
// the old input, t / clear_cycle, and the engine's RateState integral of
// 1 / clear_cycle — exactly what the shader now receives in slot 8.
//
// Run: cargo test -p fosfora-app -- --ignored --nocapture etch_fader_drag
#[test]
#[ignore = "requires a GPU/software adapter"]
fn etch_fader_drag_does_not_fire_spurious_clears() {
    use crate::effect::rates::{RateState, layout};
    use crate::params::{ParamStore, ParamValue};
    use wgpu::util::DeviceExt;

    const BG: &str = include_str!("../../../../../assets/shaders/etch_bg.wgsl");
    let start = BG.find("const ETCH_SHAKE_SECS").expect("ETCH_SHAKE_SECS");
    let body = BG.find("fn etch_clearing").expect("etch_clearing");
    let end = body + BG[body..].find("\n}").expect("close") + 2;
    let clearing_src = &BG[start..end];

    // The drag: 3 s at 60 fps, 12 s -> 24 s, starting at a 300 s clock.
    const FRAMES: usize = 180;
    const DT: f32 = 1.0 / 60.0;
    const T0: f32 = 300.0;
    let cycle_at = |i: usize| 12.0 + 12.0 * i as f32 / (FRAMES - 1) as f32;

    let etch = shipped_effects_for_test()
        .into_iter()
        .find(|e| e.name == "Etch")
        .expect("etch.pfx");
    let slots = layout(&etch.inputs, &etch.rates).expect("rates layout");
    let clear = slots
        .iter()
        .find(|s| s.name == "clear_cycle")
        .expect("clear_cycle rate");
    assert!(clear.period, "clear_cycle must be declared a period");

    // The engine's input: RateState, run from t=0 at 12 s, then the drag.
    let mut store = ParamStore::new();
    store.load_from_defs(&etch.inputs);
    store.set("clear_cycle", ParamValue::Float(12.0));
    let mut rates = RateState::new(slots.clone());
    for _ in 0..(T0 / DT) as usize {
        let mut p = store.pack_to_buffer();
        rates.advance(&mut p, DT);
    }
    let mut integrated = Vec::with_capacity(FRAMES * 2);
    let mut legacy = Vec::with_capacity(FRAMES * 2);
    for i in 0..FRAMES {
        let c = cycle_at(i);
        store.set("clear_cycle", ParamValue::Float(c));
        let mut p = store.pack_to_buffer();
        rates.advance(&mut p, DT);
        integrated.extend([c, p[clear.dst]]);
        legacy.extend([c, (T0 + i as f32 * DT) / c]);
    }

    let _guard = gpu_guard();
    let (device, queue) = test_gpu();
    let shader = format!(
        "{clearing_src}\n\
         @group(0) @binding(0) var<storage, read> inp: array<vec2f>;\n\
         @group(0) @binding(1) var<storage, read_write> out: array<f32>;\n\
         @compute @workgroup_size(64)\n\
         fn cs_main(@builtin(global_invocation_id) gid: vec3u) {{\n\
             let i = gid.x;\n\
             if i >= arrayLength(&inp) {{ return; }}\n\
             out[i] = select(0.0, 1.0, etch_clearing(inp[i].x, inp[i].y));\n\
         }}"
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("etch-clearing-probe"),
        source: wgpu::ShaderSource::Wgsl(shader.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("etch-clearing-probe"),
        layout: None,
        module: &module,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let run = |inputs: &[f32]| -> Vec<f32> {
        let n = inputs.len() / 2;
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(inputs),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bytes = (n * 4) as u64;
        let obuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let stage = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ibuf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: obuf.as_entire_binding(),
                },
            ],
        });
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups((n as u32).div_ceil(64), 1, 1);
        }
        enc.copy_buffer_to_buffer(&obuf, 0, &stage, 0, bytes);
        queue.submit([enc.finish()]);
        stage.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        bytemuck::cast_slice::<u8, f32>(&stage.slice(..).get_mapped_range()).to_vec()
    };
    // Clear ONSETS: frames where clearing starts. One shake is a few
    // consecutive frames and counts once.
    let onsets = |flags: &[f32]| {
        flags
            .windows(2)
            .filter(|w| w[0] < 0.5 && w[1] > 0.5)
            .count()
            + usize::from(flags[0] > 0.5)
    };
    let old = onsets(&run(&legacy));
    let new = onsets(&run(&integrated));
    eprintln!(
        "etch: 3 s fader drag 12 s -> 24 s at a {T0} s clock: clears with t / cycle {old}, \
         with the integral {new}"
    );
    // A 3 s drag through periods of 12-24 s spans at most a quarter of a
    // cycle, so at most one clear is legitimate.
    assert!(
        new <= 1,
        "the integrated clear clock fired {new} clears during a 3 s drag — the \
         fader is sweeping the phase again"
    );
    assert!(
        old > 3,
        "the old clock should fire repeatedly during this drag (it read {old}); if \
         it no longer does, this test has stopped discriminating"
    );
}
