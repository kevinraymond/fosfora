// Default fallback shader — minimal dark gradient (self-contained, no lib prepend)

struct PhosphorUniforms {
    time: f32,
    delta_time: f32,
    resolution: vec2f,

    sub_bass: f32,
    bass: f32,
    low_mid: f32,
    mid: f32,
    upper_mid: f32,
    presence: f32,
    brilliance: f32,
    rms: f32,

    kick: f32,
    centroid: f32,
    flux: f32,
    flatness: f32,
    rolloff: f32,
    bandwidth: f32,
    zcr: f32,
    onset: f32,
    beat: f32,
    beat_phase: f32,
    bpm: f32,
    beat_strength: f32,

    params: array<vec4f, 4>,
    feedback_decay: f32,
    frame_index: f32,

    dominant_chroma: f32,
    scroll_phase: f32,
    mfcc: array<vec4f, 4>,
    chroma: array<vec4f, 3>,

    // Reserved audio features (batched ABI bump #1505) — 0.0 until each detector lands.
    loudness_m: f32,
    loudness_s: f32,
    loudness_trend: f32,
    key_class: f32,
    key_is_minor: f32,
    key_confidence: f32,
    downbeat: f32,
    bar_phase: f32,
    beat_in_bar: f32,
    pan: f32,
    stereo_width: f32,
    stereo_corr: f32,
    section_novelty: f32,
    buildup: f32,
    drop: f32,

    // Reserved audio features (batched ABI bump #1629, "v3") — 0.0 until each detector lands.
    percussive_energy: f32,
    harmonic_energy: f32,
    harmonic_ratio: f32,
    pitch: f32,
    pitch_confidence: f32,
    contrast_0: f32,
    contrast_1: f32,
    contrast_2: f32,
    contrast_3: f32,
    contrast_4: f32,
    contrast_5: f32,
    contrast_mean: f32,
    timbre_flux: f32,
}

@group(0) @binding(0) var<uniform> u: PhosphorUniforms;
@group(0) @binding(1) var prev_frame: texture_2d<f32>;
@group(0) @binding(2) var prev_sampler: sampler;
// A17 audio textures (#1505) — 1x1 placeholders until the A17 DSP uploads real data.
@group(0) @binding(3) var audio_waveform: texture_2d<f32>;
@group(0) @binding(4) var audio_spectrum: texture_2d<f32>;
@group(0) @binding(5) var audio_spectrogram: texture_2d<f32>;
@group(0) @binding(6) var audio_sampler: sampler;

@fragment
fn fs_main(@builtin(position) frag_coord: vec4f) -> @location(0) vec4f {
    let uv = frag_coord.xy / u.resolution;
    let col = vec3f(uv.x * 0.05, 0.02, uv.y * 0.05 + 0.02);
    return vec4f(col, 1.0);
}
