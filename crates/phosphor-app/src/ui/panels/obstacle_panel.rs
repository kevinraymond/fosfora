use egui::{RichText, Ui};

use crate::gpu::particle::{ObstacleFit, ObstacleMode};
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;
use crate::ui::widgets::rows::combo_row;

/// Snapshot of obstacle state, collected before UI borrow to avoid borrow conflicts.
#[derive(Clone)]
pub struct ObstacleInfo {
    pub enabled: bool,
    pub mode: ObstacleMode,
    pub fit: ObstacleFit,
    pub threshold: f32,
    pub elasticity: f32,
    /// "image", "webcam", "depth", or "" (none)
    pub source: String,
    pub image_path: Option<String>,
    pub has_particles: bool,
    pub webcam_available: bool,
    pub video_available: bool,
    pub depth_available: bool,
    pub depth_model_downloaded: bool,
    /// Download progress percentage (0-100), or None if not downloading.
    pub depth_downloading: Option<u8>,
    pub depth_download_error: Option<String>,
    pub webcam_devices: Vec<(u32, String)>,
    pub webcam_device_index: u32,
    // Obstacle water accumulation (#1851)
    pub water_enabled: bool,
    pub water_level: f32,
    pub water_source: f32,
    pub water_drain: f32,
    pub water_flux: f32,
    /// Model rotation speed (1 = default; 0 = frozen front view).
    pub model_spin: f32,
    /// Faint model-underlay opacity (0 = hidden).
    pub model_display: f32,
    // Obstacle fluid flow (#1939): Eulerian velocity-grid sim; particles are
    // advected by a real incompressible flow around the obstacle.
    pub fluid_enabled: bool,
    pub fluid_speed: f32,
    pub fluid_coupling: f32,
    pub fluid_vorticity: f32,
    pub fluid_viscosity: f32,
    pub fluid_grid: u32,
}

/// UI commands emitted by the obstacle panel.
#[derive(Clone, Default)]
pub enum ObstacleCommand {
    #[default]
    None,
    SetEnabled(bool),
    SetMode(ObstacleMode),
    SetFit(ObstacleFit),
    SetThreshold(f32),
    SetElasticity(f32),
    LoadImage,
    LoadVideo,
    LoadModel,
    UseWebcam,
    UseDepth,
    DownloadDepthModel,
    Clear,
    SetWaterEnabled(bool),
    SetWaterLevel(f32),
    SetWaterSource(f32),
    SetWaterDrain(f32),
    SetWaterFlux(f32),
    SetModelSpin(f32),
    SetModelDisplay(f32),
    SetFluidEnabled(bool),
    SetFluidSpeed(f32),
    SetFluidCoupling(f32),
    SetFluidVorticity(f32),
    SetFluidViscosity(f32),
    SetFluidGrid(u32),
}

pub fn draw_obstacle_panel(ui: &mut Ui, info: &ObstacleInfo) {
    let tc = theme_colors(ui.ctx());

    if !info.has_particles {
        ui.label(
            RichText::new("No particle system active")
                .size(BODY_SIZE)
                .color(tc.text_secondary),
        );
        return;
    }

    // Enable toggle
    let mut enabled = info.enabled;
    if ui
        .checkbox(&mut enabled, "Enable Obstacle")
        .on_hover_text("Enable particle-obstacle collision")
        .changed()
    {
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("obstacle_cmd"),
                ObstacleCommand::SetEnabled(enabled),
            );
        });
    }

    if !info.enabled {
        return;
    }

    ui.add_space(4.0);

    // Source info + controls
    ui.horizontal(|ui| {
        let source_text = match info.source.as_str() {
            "image" => {
                if let Some(ref path) = info.image_path {
                    let name = std::path::Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "image".to_string());
                    format!("Image: {}", name)
                } else {
                    "Image".to_string()
                }
            }
            "video" => {
                if let Some(ref path) = info.image_path {
                    let name = std::path::Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "video".to_string());
                    format!("Video: {}", name)
                } else {
                    "Video".to_string()
                }
            }
            "webcam" => "Webcam".to_string(),
            "depth" => "Depth (MiDaS)".to_string(),
            "model" => {
                if let Some(ref path) = info.image_path {
                    let name = std::path::Path::new(path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "model".to_string());
                    format!("Model: {}", name)
                } else {
                    "Model".to_string()
                }
            }
            _ => "None".to_string(),
        };
        ui.label(
            RichText::new(source_text)
                .size(BODY_SIZE)
                .color(tc.text_primary),
        );
    });

    // Webcam device selector (when using webcam or depth source with multiple cameras)
    if info.webcam_available
        && (info.source == "webcam" || info.source == "depth")
        && info.webcam_devices.len() > 1
    {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Camera")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );

            let selected_name = info
                .webcam_devices
                .iter()
                .find(|(idx, _)| *idx == info.webcam_device_index)
                .map(|(_, name)| name.as_str())
                .unwrap_or("Camera");

            egui::ComboBox::from_id_salt("obstacle_webcam_device_combo")
                .selected_text(RichText::new(selected_name).size(SMALL_SIZE))
                .width(ui.available_width() - 4.0)
                .show_ui(ui, |ui| {
                    for (idx, name) in &info.webcam_devices {
                        let selected = *idx == info.webcam_device_index;
                        if ui
                            .selectable_label(selected, RichText::new(name).size(SMALL_SIZE))
                            .clicked()
                            && !selected
                        {
                            ui.ctx().data_mut(|d| {
                                d.insert_temp(egui::Id::new("switch_obstacle_webcam_device"), *idx);
                            });
                        }
                    }
                });
        });
        ui.add_space(4.0);
    }

    // Tab-strip source selector
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;

        let tc = &tc;
        let tab_btn = |ui: &mut Ui, label: &str, is_active: bool| -> egui::Response {
            let btn = egui::Button::new(RichText::new(label).size(9.0).color(if is_active {
                egui::Color32::WHITE
            } else {
                tc.text_secondary
            }))
            .fill(if is_active {
                egui::Color32::from_rgba_unmultiplied(0x3b, 0x82, 0xf6, 50)
            } else {
                tc.widget_bg
            })
            .stroke(egui::Stroke::new(
                1.0_f32,
                if is_active {
                    egui::Color32::from_rgba_unmultiplied(0x3b, 0x82, 0xf6, 100)
                } else {
                    tc.card_border
                },
            ))
            .corner_radius(3.0)
            .min_size(egui::vec2(0.0, 22.0));
            ui.add(btn)
        };

        if tab_btn(ui, "Image", info.source == "image")
            .on_hover_text("Load an image as obstacle shape")
            .clicked()
        {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("obstacle_cmd"), ObstacleCommand::LoadImage);
            });
        }
        if tab_btn(ui, "Model", info.source == "model")
            .on_hover_text("Load a 3D model (.glb/.gltf mesh or .ply/.splat cloud) — particles flow over its rotating surface")
            .clicked()
        {
            ui.ctx().data_mut(|d| {
                d.insert_temp(egui::Id::new("obstacle_cmd"), ObstacleCommand::LoadModel);
            });
        }
        if info.video_available {
            if tab_btn(ui, "Video", info.source == "video")
                .on_hover_text("Load a video as animated obstacle")
                .clicked()
            {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("obstacle_cmd"), ObstacleCommand::LoadVideo);
                });
            }
        }
        if info.webcam_available {
            if tab_btn(ui, "Webcam", info.source == "webcam")
                .on_hover_text("Use live webcam feed as obstacle")
                .clicked()
            {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("obstacle_cmd"), ObstacleCommand::UseWebcam);
                });
            }
        }
        if info.depth_available && info.webcam_available {
            if info.depth_model_downloaded {
                if tab_btn(ui, "Depth", info.source == "depth")
                    .on_hover_text("Monocular depth estimation (MiDaS)")
                    .clicked()
                {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("obstacle_cmd"), ObstacleCommand::UseDepth);
                    });
                }
            } else if info.depth_downloading.is_some() {
                let pct = info.depth_downloading.unwrap_or(0);
                ui.add_enabled(
                    false,
                    egui::Button::new(RichText::new(format!("Depth {pct}%")).size(9.0))
                        .min_size(egui::vec2(0.0, 22.0)),
                );
            } else {
                if tab_btn(ui, "Depth", false)
                    .on_hover_text("Requires one-time download (~80 MB)")
                    .clicked()
                {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(egui::Id::new("depth_download_confirm"), true));
                }
            }
        }
        if !info.source.is_empty() {
            if tab_btn(ui, "Clear", false)
                .on_hover_text("Remove obstacle and stop capture")
                .clicked()
            {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::new("obstacle_cmd"), ObstacleCommand::Clear);
                });
            }
        }
    });

    // Show download error if any
    if let Some(ref err) = info.depth_download_error {
        ui.label(
            RichText::new(format!("Download error: {err}"))
                .size(BODY_SIZE - 1.0)
                .color(tc.error),
        );
    }

    ui.add_space(4.0);

    // Model spin (only for the 3D-model source): 0 = frozen front view.
    if info.source == "model" {
        let mut spin = info.model_spin;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                RichText::new("Spin")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            )
            .on_hover_text("Model rotation speed — 0 freezes it to a front view");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(
                    RichText::new(format!("{:.2}", spin))
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                );
                ui.spacing_mut().slider_width = ui.available_width();
                if ui
                    .add(egui::Slider::new(&mut spin, 0.0..=2.0).show_value(false))
                    .changed()
                {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(
                            egui::Id::new("obstacle_cmd"),
                            ObstacleCommand::SetModelSpin(spin),
                        );
                    });
                }
            });
        });

        // Show-model underlay opacity: see the form the water flows over.
        let mut disp = info.model_display;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                RichText::new("Show model")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            )
            .on_hover_text("Faint underlay of the model so you can see what the water flows over");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(
                    RichText::new(format!("{:.2}", disp))
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                );
                ui.spacing_mut().slider_width = ui.available_width();
                if ui
                    .add(egui::Slider::new(&mut disp, 0.0..=1.0).show_value(false))
                    .changed()
                {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(
                            egui::Id::new("obstacle_cmd"),
                            ObstacleCommand::SetModelDisplay(disp),
                        );
                    });
                }
            });
        });
    }

    // Mode dropdown
    let mut mode = info.mode;
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Mode")
                .size(BODY_SIZE)
                .color(tc.text_secondary),
        )
        .on_hover_text("How particles respond when hitting the obstacle");
        egui::ComboBox::from_id_salt("obstacle_mode")
            .selected_text(mode.label())
            .show_ui(ui, |ui| {
                for m in [
                    ObstacleMode::Bounce,
                    ObstacleMode::Stick,
                    ObstacleMode::Flow,
                    ObstacleMode::Contain,
                    ObstacleMode::Drape,
                ] {
                    if ui.selectable_value(&mut mode, m, m.label()).changed() {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(
                                egui::Id::new("obstacle_cmd"),
                                ObstacleCommand::SetMode(mode),
                            );
                        });
                    }
                }
            });
    });

    // Fit dropdown (#1790): how the obstacle map is aspect-fitted to the screen.
    let mut fit = info.fit;
    combo_row(
        ui,
        "obstacle_fit",
        "Fit",
        Some(
            "How the obstacle map is fitted to the screen: Fill crops to cover, Fit letterboxes, Stretch distorts (legacy)",
        ),
        fit.label(),
        |ui| {
            for f in [
                ObstacleFit::Cover,
                ObstacleFit::Contain,
                ObstacleFit::Stretch,
            ] {
                if ui.selectable_value(&mut fit, f, f.label()).changed() {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("obstacle_cmd"), ObstacleCommand::SetFit(fit));
                    });
                }
            }
        },
    );

    // Threshold slider — compact row
    let mut threshold = info.threshold;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(
            RichText::new("Threshold")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        )
        .on_hover_text("Alpha cutoff for collision detection (lower = more sensitive)");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                RichText::new(format!("{:.2}", threshold))
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );
            ui.spacing_mut().slider_width = ui.available_width();
            if ui
                .add(
                    egui::Slider::new(&mut threshold, 0.0..=1.0)
                        .step_by(0.01)
                        .show_value(false),
                )
                .changed()
            {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new("obstacle_cmd"),
                        ObstacleCommand::SetThreshold(threshold),
                    );
                });
            }
        });
    });

    // Elasticity slider — compact row
    let mut elasticity = info.elasticity;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(
            RichText::new("Elasticity")
                .size(SMALL_SIZE)
                .color(tc.text_secondary),
        )
        .on_hover_text("Energy preserved on bounce (0 = absorb, 1 = full)");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(
                RichText::new(format!("{:.2}", elasticity))
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );
            ui.spacing_mut().slider_width = ui.available_width();
            if ui
                .add(
                    egui::Slider::new(&mut elasticity, 0.0..=1.0)
                        .step_by(0.01)
                        .show_value(false),
                )
                .changed()
            {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        egui::Id::new("obstacle_cmd"),
                        ObstacleCommand::SetElasticity(elasticity),
                    );
                });
            }
        });
    });

    // --- Water accumulation (#1851): shallow-water sim over the obstacle ---
    ui.add_space(6.0);
    ui.separator();
    let mut water_on = info.water_enabled;
    if ui
        .checkbox(&mut water_on, "Water accumulation")
        .on_hover_text(
            "Shallow-water sim over the obstacle: particles pool in the recesses and overflow",
        )
        .changed()
    {
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("obstacle_cmd"),
                ObstacleCommand::SetWaterEnabled(water_on),
            );
        });
    }
    if info.water_enabled {
        let tc = &tc;
        let slider = |ui: &mut Ui, label: &str, hover: &str, val: f32, max: f32| -> Option<f32> {
            let mut v = val;
            let mut out = None;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(
                    RichText::new(label)
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                )
                .on_hover_text(hover);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(
                        RichText::new(format!("{:.3}", v))
                            .size(SMALL_SIZE)
                            .color(tc.text_secondary),
                    );
                    ui.spacing_mut().slider_width = ui.available_width();
                    if ui
                        .add(egui::Slider::new(&mut v, 0.0..=max).show_value(false))
                        .changed()
                    {
                        out = Some(v);
                    }
                });
            });
            out
        };
        let emit = |ui: &Ui, cmd: ObstacleCommand| {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("obstacle_cmd"), cmd));
        };
        if let Some(v) = slider(
            ui,
            "Level",
            "How high pooled water lifts the surface",
            info.water_level,
            4.0,
        ) {
            emit(ui, ObstacleCommand::SetWaterLevel(v));
        }
        if let Some(v) = slider(
            ui,
            "Source",
            "Inflow rate — how fast water arrives",
            info.water_source,
            0.05,
        ) {
            emit(ui, ObstacleCommand::SetWaterSource(v));
        }
        if let Some(v) = slider(
            ui,
            "Drain",
            "Evaporation — how fast water leaves",
            info.water_drain,
            0.2,
        ) {
            emit(ui, ObstacleCommand::SetWaterDrain(v));
        }
        if let Some(v) = slider(
            ui,
            "Flow",
            "Flux gain — how fast water levels and flows",
            info.water_flux,
            0.25,
        ) {
            emit(ui, ObstacleCommand::SetWaterFlux(v));
        }
    }

    // --- Fluid flow (#1939): Eulerian velocity-grid sim solving an
    // incompressible flow AROUND the obstacle; particles are advected by it, so
    // water genuinely parts, wakes, and eddies past the silhouette. ---
    ui.add_space(6.0);
    ui.separator();
    let mut fluid_on = info.fluid_enabled;
    if ui
        .checkbox(&mut fluid_on, "Fluid flow")
        .on_hover_text(
            "Real incompressible flow field around the obstacle: bow wave in front, wake and eddies behind. Particle effects (Tide) ride it.",
        )
        .changed()
    {
        ui.ctx().data_mut(|d| {
            d.insert_temp(
                egui::Id::new("obstacle_cmd"),
                ObstacleCommand::SetFluidEnabled(fluid_on),
            );
        });
    }
    if info.fluid_enabled {
        let tc = &tc;
        let slider = |ui: &mut Ui, label: &str, hover: &str, val: f32, max: f32| -> Option<f32> {
            let mut v = val;
            let mut out = None;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(
                    RichText::new(label)
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                )
                .on_hover_text(hover);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.label(
                        RichText::new(format!("{:.3}", v))
                            .size(SMALL_SIZE)
                            .color(tc.text_secondary),
                    );
                    ui.spacing_mut().slider_width = ui.available_width();
                    if ui
                        .add(egui::Slider::new(&mut v, 0.0..=max).show_value(false))
                        .changed()
                    {
                        out = Some(v);
                    }
                });
            });
            out
        };
        let emit = |ui: &Ui, cmd: ObstacleCommand| {
            ui.ctx()
                .data_mut(|d| d.insert_temp(egui::Id::new("obstacle_cmd"), cmd));
        };
        if let Some(v) = slider(
            ui,
            "Speed",
            "How fast the sheet pours — the flow's driver (top-edge inflow)",
            info.fluid_speed,
            2.0,
        ) {
            emit(ui, ObstacleCommand::SetFluidSpeed(v));
        }
        if let Some(v) = slider(
            ui,
            "Follow",
            "How tightly particles lock to the flow field vs their own momentum",
            info.fluid_coupling,
            1.0,
        ) {
            emit(ui, ObstacleCommand::SetFluidCoupling(v));
        }
        if let Some(v) = slider(
            ui,
            "Eddies",
            "Vorticity confinement — crisper swirls and wake turbulence",
            info.fluid_vorticity,
            0.5,
        ) {
            emit(ui, ObstacleCommand::SetFluidVorticity(v));
        }
        if let Some(v) = slider(
            ui,
            "Smooth",
            "Viscosity — higher damps turbulence into a glassier sheet",
            info.fluid_viscosity,
            0.2,
        ) {
            emit(ui, ObstacleCommand::SetFluidViscosity(v));
        }
        // Solver grid resolution — quality vs cost.
        let mut grid = info.fluid_grid;
        combo_row(
            ui,
            "obstacle_fluid_grid",
            "Detail",
            Some("Solver grid resolution: higher = finer flow detail, more GPU cost"),
            match grid {
                0..=160 => "Low (128)",
                161..=384 => "Medium (256)",
                _ => "High (512)",
            },
            |ui| {
                for (g, label) in [
                    (128u32, "Low (128)"),
                    (256, "Medium (256)"),
                    (512, "High (512)"),
                ] {
                    if ui.selectable_value(&mut grid, g, label).changed() {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(
                                egui::Id::new("obstacle_cmd"),
                                ObstacleCommand::SetFluidGrid(g),
                            );
                        });
                    }
                }
            },
        );
    }
}

/// Draw the depth download confirmation modal (must be called at top level, not inside a panel).
pub fn draw_depth_download_modal(ctx: &egui::Context) {
    let show: bool = ctx.data(|d| {
        d.get_temp(egui::Id::new("depth_download_confirm"))
            .unwrap_or(false)
    });
    if !show {
        return;
    }

    let tc = theme_colors(ctx);

    egui::Window::new("Download Depth Model")
        .collapsible(false)
        .resizable(false)
        .fixed_size(egui::Vec2::new(340.0, 0.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label(
                RichText::new("Depth-based obstacle collision uses monocular depth estimation to create a 3D collision map from your webcam.")
                    .size(13.0)
                    .color(tc.text_primary),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new("This requires a one-time download:")
                    .size(13.0)
                    .color(tc.text_secondary),
            );
            ui.add_space(4.0);
            ui.indent("dl_details", |ui| {
                ui.label(RichText::new("ONNX Runtime (~15 MB)").size(12.0).color(tc.text_secondary));
                ui.label(RichText::new("  from github.com/microsoft").size(11.0).color(tc.text_secondary));
                ui.label(RichText::new("MiDaS v2.1 model (~63 MB)").size(12.0).color(tc.text_secondary));
                ui.label(RichText::new("  from huggingface.co").size(11.0).color(tc.text_secondary));
            });
            ui.add_space(4.0);
            ui.label(
                RichText::new("Files are cached locally and only downloaded once.")
                    .size(12.0)
                    .color(tc.text_secondary),
            );
            ui.add_space(12.0);

            let btn_size = egui::Vec2::new(110.0, 32.0);
            ui.horizontal(|ui| {
                let accent = tc.accent;
                let dl_fill = egui::Color32::from_rgba_unmultiplied(
                    accent.r(), accent.g(), accent.b(), 60,
                );
                if ui
                    .add(egui::Button::new(
                        RichText::new("Download").color(accent),
                    ).fill(dl_fill).min_size(btn_size))
                    .clicked()
                {
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("depth_download_confirm"), false);
                        d.insert_temp(egui::Id::new("obstacle_cmd"), ObstacleCommand::DownloadDepthModel);
                    });
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new("Cancel").min_size(btn_size)).clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new("depth_download_confirm"), false);
                        });
                    }
                });
            });
        });
}
