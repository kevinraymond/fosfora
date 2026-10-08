pub mod appearance_panel;
pub mod audio_mappings_panel;
pub mod audio_panel;
pub mod binding_helpers;
pub mod binding_matrix;
pub mod bindings_panel;
pub mod catalog_panel;
pub mod cue_strip;
pub mod helix_panel;
pub mod lattice_panel;
pub mod layer_panel;
#[cfg(feature = "link")]
pub mod link_panel;
pub mod media_panel;
#[cfg(feature = "ndi")]
pub mod ndi_panel;
pub mod obstacle_panel;
pub mod osc_panel;
pub mod output_window_panel;
pub mod param_panel;
pub mod particle_panel;
pub mod postfx_panel;
pub mod preset_panel;
pub mod recording_panel;
pub mod scene_panel;
pub mod shader_editor;
#[cfg(all(target_os = "windows", feature = "spout"))]
pub mod spout_panel;
pub mod stack_panel;
pub mod status_bar;
#[cfg(all(target_os = "macos", feature = "syphon"))]
pub mod syphon_panel;
pub mod timeline_bar;
#[cfg(all(target_os = "linux", feature = "v4l2"))]
pub mod v4l2_panel;
pub mod volumetric_panel;
pub mod webcam_panel;
