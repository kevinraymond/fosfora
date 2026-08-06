use crate::audio::features::AudioFeatures;

/// Named preset indices for the force matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbiosisPreset {
    Ecosystem = 0,
    Crystals = 1,
    Hunters = 2,
    Membrane = 3,
    Chaos = 4,
    Symmetric = 5,
}

impl SymbiosisPreset {
    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Ecosystem,
            1 => Self::Crystals,
            2 => Self::Hunters,
            3 => Self::Membrane,
            4 => Self::Chaos,
            5 => Self::Symmetric,
            _ => Self::Ecosystem,
        }
    }

    pub fn count() -> usize {
        6
    }

    /// Return the preset force matrix (8x8, row-major, 8-wide stride).
    pub fn matrix(&self, num_species: u32) -> [f32; 64] {
        let n = num_species.min(8) as usize;
        let mut m = [0.0f32; 64];
        match self {
            // Ecosystem: mixed attractions/repulsions creating clusters that orbit each other
            Self::Ecosystem => {
                for i in 0..n {
                    for j in 0..n {
                        let diff = (j as i32 - i as i32).rem_euclid(n as i32) as f32;
                        let t = diff / n as f32;
                        // Same species: mild attraction. +1 neighbor: strong attraction.
                        // +2: repulsion. Others: weak varied.
                        m[i * 8 + j] = if i == j {
                            0.3
                        } else if t < 0.25 {
                            0.7
                        } else if t < 0.5 {
                            -0.5
                        } else if t < 0.75 {
                            0.2
                        } else {
                            -0.3
                        };
                    }
                }
            }
            // Crystals: strong same-species attraction, inter-species repulsion → lattice formation
            Self::Crystals => {
                for i in 0..n {
                    for j in 0..n {
                        m[i * 8 + j] = if i == j { 0.8 } else { -0.4 };
                    }
                }
            }
            // Hunters: asymmetric predator-prey chain (A chases B, B chases C, ...)
            Self::Hunters => {
                for i in 0..n {
                    for j in 0..n {
                        m[i * 8 + j] = if i == j {
                            0.1
                        } else if j == (i + 1) % n {
                            0.9 // chase next species
                        } else if j == (i + n - 1) % n {
                            -0.8 // flee from previous
                        } else {
                            0.0
                        };
                    }
                }
            }
            // Membrane: species form concentric shells
            Self::Membrane => {
                for i in 0..n {
                    for j in 0..n {
                        let dist = ((j as i32 - i as i32).abs())
                            .min(n as i32 - (j as i32 - i as i32).abs());
                        m[i * 8 + j] = if dist == 0 {
                            0.5
                        } else if dist == 1 {
                            0.3
                        } else {
                            -0.2 - 0.1 * dist as f32
                        };
                    }
                }
            }
            // Chaos: all entries near extremes
            Self::Chaos => {
                for i in 0..n {
                    for j in 0..n {
                        // Deterministic pseudo-random from indices
                        let h = ((i * 7 + j * 13 + 5) % 17) as f32 / 17.0;
                        m[i * 8 + j] = h * 2.0 - 1.0;
                    }
                }
            }
            // Symmetric: force(A,B) == force(B,A) — produces crystals/clusters
            Self::Symmetric => {
                for i in 0..n {
                    for j in i..n {
                        let h = ((i * 11 + j * 3 + 7) % 19) as f32 / 19.0;
                        let v = h * 1.6 - 0.8;
                        m[i * 8 + j] = v;
                        m[j * 8 + i] = v;
                    }
                }
            }
        }
        m
    }
}

/// CPU-side state for the symbiosis force matrix.
pub struct SymbiosisState {
    pub num_species: u32,
    current_matrix: [f32; 64],
    target_matrix: [f32; 64],
    interpolation_progress: f32,
    interpolation_duration: f32,
    current_preset_index: usize,
    /// Accumulated flux perturbation seed
    flux_phase: f32,
}

impl SymbiosisState {
    pub fn new(num_species: u32) -> Self {
        let preset = SymbiosisPreset::Ecosystem;
        let matrix = preset.matrix(num_species);
        Self {
            num_species,
            current_matrix: matrix,
            target_matrix: matrix,
            interpolation_progress: 1.0,
            interpolation_duration: 0.5,
            current_preset_index: 0,
            flux_phase: 0.0,
        }
    }

    /// Set a new target preset, beginning interpolation.
    pub fn set_preset(&mut self, index: usize) {
        if index == self.current_preset_index && self.interpolation_progress >= 1.0 {
            return;
        }
        self.current_preset_index = index;
        let preset = SymbiosisPreset::from_index(index);
        self.target_matrix = preset.matrix(self.num_species);
        self.interpolation_progress = 0.0;
    }

    /// Update species count and regenerate target matrix.
    pub fn set_num_species(&mut self, n: u32) {
        let n = n.clamp(2, 8);
        if n != self.num_species {
            self.num_species = n;
            let preset = SymbiosisPreset::from_index(self.current_preset_index);
            self.target_matrix = preset.matrix(self.num_species);
            self.interpolation_progress = 0.0;
        }
    }

    /// Randomize `count` random matrix entries (onset trigger).
    pub fn shuffle_entries(&mut self, count: usize, seed: f32) {
        let n = self.num_species as usize;
        for k in 0..count {
            let s = seed + k as f32 * 7.13;
            let h = (s * 43_758.547).sin().fract().abs();
            let i = (h * n as f32) as usize % n;
            let j = ((h * 31.7 + 0.3).fract() * n as f32) as usize % n;
            let val = (h * 97.3 + 0.7).fract() * 2.0 - 1.0;
            self.target_matrix[i * 8 + j] = val;
        }
        // Quick blend to the shuffled entries
        self.interpolation_progress = 0.5;
    }

    /// Generate a fully random matrix.
    #[allow(dead_code)]
    pub fn randomize_matrix(&mut self, seed: f32) {
        let n = self.num_species as usize;
        for i in 0..n {
            for j in 0..n {
                let s = seed + (i * 8 + j) as f32 * 3.17;
                self.target_matrix[i * 8 + j] = (s * 43_758.547).sin().fract() * 2.0 - 1.0;
            }
        }
        self.interpolation_progress = 0.0;
    }

    /// Per-frame update: interpolate matrix, apply flux perturbation.
    pub fn update(&mut self, dt: f32, audio: &AudioFeatures) {
        // Interpolate toward target
        if self.interpolation_progress < 1.0 {
            self.interpolation_progress += dt / self.interpolation_duration;
            if self.interpolation_progress >= 1.0 {
                self.interpolation_progress = 1.0;
                self.current_matrix = self.target_matrix;
            }
        }

        // Onset shuffle: randomize a few entries on strong onsets
        if audio.onset > 0.5 {
            let count = (audio.onset * 4.0) as usize;
            self.shuffle_entries(count, audio.onset * 100.0 + self.flux_phase);
        }

        // Flux perturbation: subtle continuous drift
        self.flux_phase += dt * 0.5;
    }

    /// Return the active blended + perturbed matrix.
    pub fn active_matrix(&self) -> [f32; 64] {
        if self.interpolation_progress >= 1.0 {
            return self.current_matrix;
        }
        let t = self.interpolation_progress;
        let mut result = [0.0f32; 64];
        for i in 0..64 {
            result[i] = self.current_matrix[i] * (1.0 - t) + self.target_matrix[i] * t;
        }
        result
    }
}
