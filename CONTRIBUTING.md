# Contributing to Phosphor

Thanks for your interest in contributing! Phosphor is a live VJ engine built with Rust, wgpu, and WGSL shaders. Whether you're adding effects, fixing bugs, or improving docs, contributions are welcome.

## Building

**Prerequisites:**
- Rust 1.85+ (install via [rustup](https://rustup.rs))
- Audio input device (built-in mic works)
- Vulkan-capable GPU

**Commands:**
```bash
cargo run --release              # release build (recommended)
cargo run                        # debug build (slower shaders)
RUST_LOG=phosphor_app=debug cargo run  # verbose logging
```

## Adding an Effect

The fastest way to contribute is writing a new visual effect. Three steps:

1. **Create a shader** in `assets/shaders/your_effect.wgsl`. Start from the template in [TECHNICAL.md](TECHNICAL.md#shader-authoring-guide) — you get time, resolution, 20 audio features, up to 16 params, and a WGSL library (noise, palette, SDF, tonemap) auto-prepended.

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

See the [Shader Authoring Guide](TECHNICAL.md#shader-authoring-guide) for uniforms, multi-pass, particles, feedback, and common pitfalls.

## Reporting Bugs

Please include:
- OS and GPU (e.g., "Linux, NVIDIA RTX 4090")
- Steps to reproduce
- Expected vs. actual behavior
- Log output if relevant (`RUST_LOG=phosphor_app=debug cargo run`)

## Code Style

- Follow existing patterns in the codebase
- Keep `cargo clippy` clean
- No new `unsafe` without justification
- Prefer dedicated wgpu abstractions over raw API calls

## Pull Requests

- One feature or fix per PR
- Test on at least one platform before submitting
- Include a brief description of what changed and why
- New effects should include both the `.wgsl` shader and `.pfx` definition

## License

By contributing, you agree that your contributions will be licensed under the same dual MIT/Apache-2.0 license as the project.
