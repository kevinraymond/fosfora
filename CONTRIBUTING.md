# Contributing to Fosfora

Thanks for your interest in contributing! Fosfora is a live VJ engine built with Rust, wgpu, and WGSL shaders. Whether you're adding effects, fixing bugs, or improving docs, contributions are welcome.

## Building

**Prerequisites:**
- Rust 1.97+ (pinned via `rust-toolchain.toml`; install via [rustup](https://rustup.rs))
- Audio input device (built-in mic works)
- Vulkan-capable GPU

**Setup:**
```bash
git config core.hooksPath .githooks   # enable pre-commit checks (fmt + clippy)
```

**Commands:**
```bash
cargo run --release              # release build (recommended)
cargo run                        # debug build (slower shaders)
RUST_LOG=fosfora=debug cargo run  # verbose logging
```

## Adding an Effect

The fastest way to contribute is writing a new visual effect. Three steps:

1. **Create a shader** in `assets/shaders/your_effect.wgsl`. Start from the template in [docs/TECHNICAL.md](docs/TECHNICAL.md#shader-authoring-guide) — you get time, resolution, 74 audio features, up to 16 params, and a WGSL library (noise, palette, SDF, tonemap) auto-prepended.

2. **Create a definition** in `assets/effects/your_effect.pfx` (JSON):
   ```json
   {
       "name": "Your Effect",
       "author": "You",
       "description": "What it looks like",
       "shader": "your_effect.wgsl",
       "inputs": [
           { "type": "Float", "name": "speed", "default": 0.5, "min": 0.0, "max": 1.0 }
       ]
   }
   ```

3. **Run** — the effect appears in the browser automatically. Edit the shader while running; it hot-reloads on save.

See the [Shader Authoring Guide](docs/TECHNICAL.md#shader-authoring-guide) for uniforms, multi-pass, particles, feedback, and common pitfalls.

## Adding a trama Node

A trama node is one self-contained `.wgsl` file in `assets/trama/effects/` — the manifest
lives in a `/*! trama { … } */` comment at the top of the shader, so there is no second file
to keep in step. Scaffold one:

```sh
cargo xtask new-effect my_effect            # a 1-input effect
cargo xtask new-effect my_mix --inputs 2    # takes two pictures
cargo xtask new-effect my_source --source   # generates one, takes no input
```

The app watches that directory and treats a path it has not seen before as a new effect, so
the node appears in a running app's palette without a restart. If the file stops compiling,
the last good version keeps rendering and the compiler's text shows in the node inspector
under a `· ERROR` title — output never blanks because you saved a typo.

Two rules the scaffold already follows:

- **Color is premultiplied** (see [docs/alpha.md](docs/alpha.md)): RGB is scaled by coverage,
  and RGB > A is legal and means additive light. Scaling a whole `vec4f` preserves that; a
  non-linear tone curve does not, so divide coverage back out first and multiply it in again
  the way `levels.wgsl` does.
- **Never write `u.time * some_param`.** Multiplying a parameter by absolute time multiplies
  every *change* in that parameter by the app's uptime, so a modulated speed strobes instead
  of speeding up. List the parameter under `"rates"` in the manifest and its slot arrives as
  the engine-integrated phase. A unit test refuses the pattern in shipped nodes.

## Reporting Bugs

Please include:
- OS and GPU (e.g., "Linux, NVIDIA RTX 4090")
- Steps to reproduce
- Expected vs. actual behavior
- Log output if relevant (`RUST_LOG=fosfora=debug cargo run`)

## Code Style

- Follow existing patterns in the codebase
- Keep `cargo clippy` clean
- No new `unsafe` without justification
- Prefer dedicated wgpu abstractions over raw API calls

The pre-commit hook and the CI `lint` job run clippy on the **host** target only, so
`#[cfg(target_os = "…")]` code for other platforms is not linted locally. CI's `build`
matrix runs clippy per native target to cover it. If you touch Windows/macOS-gated code,
lint it yourself by cross-compiling clippy (targets install via `rustup target add`):

```sh
cargo clippy --target x86_64-pc-windows-gnu -- -D warnings   # Windows-gated code
cargo clippy --target aarch64-apple-darwin  -- -D warnings   # macOS-gated code
```

## Pull Requests

- One feature or fix per PR
- Test on at least one platform before submitting
- Include a brief description of what changed and why
- New effects should include both the `.wgsl` shader and `.pfx` definition

## License

By contributing, you agree that your contributions will be licensed under the same dual MIT/Apache-2.0 license as the project.
