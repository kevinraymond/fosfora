use std::path::PathBuf;

use egui::{
    Color32, CornerRadius, Frame, Id, Key, Margin, Modifiers, Order, Rect, RichText, Stroke,
    StrokeKind, Vec2,
};
use egui_code_editor::{ColorTheme, Syntax, Token, TokenType};

use crate::ui::theme::ThemeMode;
use crate::ui::theme::colors::theme_colors;

/// State for the shader editor overlay.
pub struct ShaderEditorState {
    pub open: bool,
    pub minimized: bool,
    pub editor_opacity: f32,
    pub file_path: Option<PathBuf>,
    pub file_name: String,
    pub effect_name: String,
    pub code: String,
    pub disk_content: String,
    pub compile_error: Option<String>,
    pub new_effect_prompt: bool,
    pub new_effect_name: String,
    pub copy_builtin_mode: bool,
}

impl Default for ShaderEditorState {
    fn default() -> Self {
        Self {
            open: false,
            minimized: false,
            editor_opacity: 0.85,
            file_path: None,
            file_name: String::new(),
            effect_name: String::new(),
            code: String::new(),
            disk_content: String::new(),
            compile_error: None,
            new_effect_prompt: false,
            new_effect_name: String::new(),
            copy_builtin_mode: false,
        }
    }
}

impl ShaderEditorState {
    pub fn is_dirty(&self) -> bool {
        self.code != self.disk_content
    }

    pub fn open_file(&mut self, effect_name: &str, path: PathBuf, content: String) {
        self.open = true;
        self.minimized = false;
        self.file_name = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        self.effect_name = effect_name.to_string();
        self.file_path = Some(path);
        self.code = content.clone();
        self.disk_content = content;
        self.compile_error = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.file_path = None;
        self.code.clear();
        self.disk_content.clear();
        self.compile_error = None;
    }
}

// --- Vector icon buttons (no font dependency) ---

fn icon_button(
    ui: &mut egui::Ui,
    _id: &str,
    color: Color32,
    paint: impl FnOnce(&egui::Painter, Rect, Color32),
) -> egui::Response {
    let size = Vec2::splat(16.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let c = if response.hovered() { Color32::WHITE } else { color };
    paint(ui.painter(), rect, c);
    response
}

fn close_icon(ui: &mut egui::Ui, id: &str, color: Color32) -> egui::Response {
    icon_button(ui, id, color, |painter, rect, c| {
        let center = rect.center();
        let s = 3.5;
        let stroke = Stroke::new(1.5, c);
        painter.line_segment(
            [
                egui::pos2(center.x - s, center.y - s),
                egui::pos2(center.x + s, center.y + s),
            ],
            stroke,
        );
        painter.line_segment(
            [
                egui::pos2(center.x + s, center.y - s),
                egui::pos2(center.x - s, center.y + s),
            ],
            stroke,
        );
    })
}

fn minimize_icon(ui: &mut egui::Ui, id: &str, color: Color32) -> egui::Response {
    icon_button(ui, id, color, |painter, rect, c| {
        let center = rect.center();
        let s = 4.0;
        let stroke = Stroke::new(1.5, c);
        let y = center.y + 3.0;
        painter.line_segment(
            [egui::pos2(center.x - s, y), egui::pos2(center.x + s, y)],
            stroke,
        );
    })
}

fn restore_icon(ui: &mut egui::Ui, id: &str, color: Color32) -> egui::Response {
    icon_button(ui, id, color, |painter, rect, c| {
        let center = rect.center();
        let s = 4.0;
        let stroke = Stroke::new(1.5, c);
        let r = Rect::from_center_size(center, Vec2::splat(s * 2.0));
        painter.rect_stroke(r, 1.0, stroke, StrokeKind::Outside);
    })
}

/// Build a WGSL syntax definition for the code editor.
fn wgsl_syntax() -> Syntax {
    Syntax::new("wgsl")
        .with_case_sensitive(true)
        .with_comment("//")
        .with_comment_multiline(["/*", "*/"])
        .with_keywords([
            "fn", "var", "let", "const", "struct", "return", "if", "else", "for", "while", "loop",
            "break", "continue", "discard", "switch", "case", "default", "override", "enable",
            "true", "false",
        ])
        .with_types([
            "f32", "f16", "i32", "u32", "bool", "vec2f", "vec3f", "vec4f", "vec2i", "vec3i",
            "vec4i", "vec2u", "vec3u", "vec4u", "mat2x2f", "mat3x3f", "mat4x4f", "array",
            "texture_2d", "texture_storage_2d", "sampler", "ptr",
        ])
        .with_special([
            "@fragment",
            "@vertex",
            "@compute",
            "@group",
            "@binding",
            "@location",
            "@builtin",
            "@workgroup_size",
            "uniform",
            "storage",
            "read",
            "read_write",
        ])
}

/// Get the color theme for syntax highlighting (no bg hacking needed).
fn editor_color_theme(theme: ThemeMode) -> ColorTheme {
    match theme {
        ThemeMode::Light => ColorTheme::GITHUB_LIGHT,
        _ => ColorTheme::AYU_DARK,
    }
}

/// Draw the shader editor as an overlay with semi-transparent code area.
/// Returns true if the editor is open.
pub fn draw_shader_editor(ctx: &egui::Context, state: &mut ShaderEditorState, theme: ThemeMode) -> bool {
    use egui::TextBuffer;

    if !state.open {
        return false;
    }

    let tc = theme_colors(ctx);
    let screen = ctx.input(|i| i.screen_rect());
    let color_theme = editor_color_theme(theme);
    let fontsize = 13.0f32;

    // Header bar height + toolbar + separators
    let header_height = 60.0;
    let error_height = if state.compile_error.is_some() { 28.0 } else { 0.0 };

    // Minimized shows ~5 lines, normal uses 80% of screen height
    let code_height = if state.minimized {
        5.0 * 17.0
    } else {
        (screen.height() * 0.80 - header_height - error_height).max(200.0)
    };

    let panel_h = header_height + code_height + error_height;
    let panel_w = (screen.width() * 0.85).max(500.0).min(screen.width() - 20.0);

    // Centered when normal, anchored to bottom when minimized
    let panel_pos = egui::pos2(
        (screen.width() - panel_w) * 0.5,
        if state.minimized {
            screen.height() - panel_h - 30.0 // 30px from bottom for status bar
        } else {
            (screen.height() - panel_h) * 0.5
        },
    );

    // Semi-transparent code background from theme bg + opacity
    let bg_base = color_theme.bg();
    let code_bg = Color32::from_rgba_unmultiplied(
        bg_base.r(),
        bg_base.g(),
        bg_base.b(),
        (state.editor_opacity * 255.0) as u8,
    );

    let editor_id = Id::new("shader_editor_overlay");
    egui::Area::new(editor_id)
        .fixed_pos(panel_pos)
        .order(Order::Foreground)
        .show(ctx, |ui| {
            ui.set_min_width(panel_w);
            ui.set_max_width(panel_w);

            // --- Opaque header frame ---
            Frame {
                fill: tc.panel,
                inner_margin: Margin::same(0),
                stroke: Stroke::new(1.0, tc.card_border),
                corner_radius: CornerRadius {
                    nw: 8,
                    ne: 8,
                    sw: 0,
                    se: 0,
                },
                ..Default::default()
            }
            .show(ui, |ui| {
                // Title bar
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(&state.effect_name)
                            .size(14.0)
                            .color(tc.text_primary)
                            .strong(),
                    );
                    ui.label(
                        RichText::new(&state.file_name)
                            .size(12.0)
                            .color(tc.text_secondary),
                    );
                    if state.is_dirty() {
                        ui.label(RichText::new("*").size(14.0).color(tc.warning));
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);

                        if close_icon(ui, "editor_close", tc.text_secondary)
                            .on_hover_text("Close (Esc)")
                            .clicked()
                        {
                            state.close();
                        }

                        ui.add_space(4.0);

                        if state.minimized {
                            if restore_icon(ui, "editor_restore", tc.text_secondary)
                                .on_hover_text("Expand")
                                .clicked()
                            {
                                state.minimized = false;
                            }
                        } else {
                            if minimize_icon(ui, "editor_min", tc.text_secondary)
                                .on_hover_text("Minimize")
                                .clicked()
                            {
                                state.minimized = true;
                            }
                        }
                    });
                });

                ui.add_space(2.0);
                ui.separator();

                // Toolbar: Save / Revert / Opacity
                ui.horizontal(|ui| {
                    ui.add_space(10.0);

                    let save_enabled = state.is_dirty();
                    let save_btn = ui.add_enabled(
                        save_enabled,
                        egui::Button::new(
                            RichText::new("Save")
                                .size(12.0)
                                .color(if save_enabled { tc.text_primary } else { tc.text_secondary }),
                        )
                        .fill(tc.card_bg)
                        .stroke(Stroke::new(1.0, tc.card_border))
                        .corner_radius(CornerRadius::same(3)),
                    );
                    if save_btn.clicked() {
                        ctx.data_mut(|d| {
                            d.insert_temp(Id::new("shader_editor_save"), true);
                        });
                    }
                    save_btn.on_hover_text("Ctrl+S");

                    if ui
                        .add_enabled(
                            state.is_dirty(),
                            egui::Button::new(
                                RichText::new("Revert").size(12.0).color(tc.text_secondary),
                            )
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE),
                        )
                        .on_hover_text("Discard changes")
                        .clicked()
                    {
                        state.code = state.disk_content.clone();
                    }

                    // Right-aligned opacity slider
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        ui.spacing_mut().slider_width = 80.0;
                        let slider = egui::Slider::new(&mut state.editor_opacity, 0.25..=1.0)
                            .show_value(false)
                            .text("BG");
                        ui.add(slider)
                            .on_hover_text(format!("Background opacity: {:.0}%", state.editor_opacity * 100.0));
                    });
                });
                ui.add_space(2.0);
            });

            // --- Semi-transparent code area ---
            // Outer Frame provides the semi-transparent fill; TextEdit bg set to transparent
            // so the shader effect shows through at the desired opacity.
            Frame {
                fill: code_bg,
                inner_margin: Margin::same(0),
                stroke: Stroke::new(1.0, tc.card_border),
                corner_radius: CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: 8,
                    se: 8,
                },
                ..Default::default()
            }
            .show(ui, |ui| {
                // Override visuals so TextEdit bg is transparent (outer Frame provides color)
                let style = ui.style_mut();
                style.visuals.extreme_bg_color = Color32::TRANSPARENT;
                style.visuals.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
                style.visuals.selection.stroke.color = color_theme.cursor();
                style.visuals.selection.bg_fill = color_theme.selection();
                style.override_font_id = Some(egui::FontId::monospace(fontsize));
                style.visuals.text_cursor.stroke.width = fontsize * 0.1;

                let syntax = wgsl_syntax();
                let num_color = color_theme.type_color(TokenType::Comment(true));

                // State-dependent ids force egui to recreate widgets on minimize/expand toggle
                let mode_salt = if state.minimized { "min" } else { "full" };
                egui::ScrollArea::vertical()
                    .id_salt(format!("shader_scroll_{mode_salt}"))
                    .min_scrolled_height(code_height)
                    .max_height(code_height)
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            // Line numbers
                            let text = &state.code;
                            let line_count = if text.ends_with('\n') || text.is_empty() {
                                text.lines().count() + 1
                            } else {
                                text.lines().count()
                            }
                            .max(5);
                            let max_digits = line_count.to_string().len();
                            let mut nums = (1..=line_count)
                                .map(|i| {
                                    let label = i.to_string();
                                    format!(
                                        "{}{label}",
                                        " ".repeat(max_digits.saturating_sub(label.len()))
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join("\n");

                            #[allow(clippy::cast_precision_loss)]
                            let num_width = max_digits as f32 * fontsize * 0.5;

                            let mut num_layouter =
                                |ui: &egui::Ui,
                                 buf: &dyn TextBuffer,
                                 _wrap: f32| {
                                    let job = egui::text::LayoutJob::single_section(
                                        buf.as_str().to_string(),
                                        egui::text::TextFormat::simple(
                                            egui::FontId::monospace(fontsize),
                                            num_color,
                                        ),
                                    );
                                    ui.fonts_mut(|f| f.layout_job(job))
                                };

                            ui.add(
                                egui::TextEdit::multiline(&mut nums)
                                    .id_source(format!("shader_numlines_{mode_salt}"))
                                    .interactive(false)
                                    .frame(false)
                                    .desired_rows(5)
                                    .desired_width(num_width)
                                    .layouter(&mut num_layouter),
                            );

                            // Code editor with syntax highlighting
                            egui::ScrollArea::horizontal()
                                .id_salt(format!("shader_hscroll_{mode_salt}"))
                                .show(ui, |ui| {
                                    let mut code_layouter =
                                        |ui: &egui::Ui,
                                         buf: &dyn TextBuffer,
                                         _wrap: f32| {
                                            let mut token = Token::default();
                                            let tokens = token.tokens(&syntax, buf.as_str());
                                            let mut job = egui::text::LayoutJob::default();
                                            for t in tokens {
                                                if !t.buffer().is_empty() {
                                                    job.append(
                                                        t.buffer(),
                                                        0.0,
                                                        egui_code_editor::format_token(
                                                            &color_theme,
                                                            fontsize,
                                                            t.ty(),
                                                        ),
                                                    );
                                                }
                                            }
                                            ui.fonts_mut(|f| f.layout_job(job))
                                        };

                                    egui::TextEdit::multiline(&mut state.code)
                                        .id_source(format!("shader_code_{mode_salt}"))
                                        .lock_focus(true)
                                        .desired_rows(60)
                                        .frame(true)
                                        .desired_width(f32::INFINITY)
                                        .layouter(&mut code_layouter)
                                        .show(ui);
                                });
                        });
                    });

                // Compile error bar
                if let Some(ref error) = state.compile_error {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(truncate_error(error, 200))
                                .size(11.0)
                                .color(tc.error),
                        );
                    });
                }
            });
        });

    // Handle Ctrl+S
    let ctrl_s = ctx.input(|i| {
        i.key_pressed(Key::S) && i.modifiers.matches_exact(Modifiers::COMMAND)
    });
    if ctrl_s && state.is_dirty() {
        ctx.data_mut(|d| {
            d.insert_temp(Id::new("shader_editor_save"), true);
        });
    }

    // Handle Esc to close
    let esc = ctx.input(|i| i.key_pressed(Key::Escape));
    if esc {
        state.close();
    }

    true
}

/// Draw the "New Effect" name prompt as a small centered window.
pub fn draw_new_effect_prompt(ctx: &egui::Context, state: &mut ShaderEditorState) {
    if !state.new_effect_prompt {
        return;
    }

    let tc = theme_colors(ctx);

    let title = if state.copy_builtin_mode { "Copy Effect" } else { "New Effect" };
    let label = if state.copy_builtin_mode { "New effect name:" } else { "Effect name:" };
    let btn_label = if state.copy_builtin_mode { "Copy" } else { "Create" };

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .fixed_size(Vec2::new(300.0, 0.0))
        .show(ctx, |ui| {
            ui.label(
                RichText::new(label)
                    .size(13.0)
                    .color(tc.text_primary),
            );
            ui.add_space(4.0);
            let response = ui.text_edit_singleline(&mut state.new_effect_name);

            // Auto-focus
            if response.gained_focus() || state.new_effect_name.is_empty() {
                response.request_focus();
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let name_valid = !state.new_effect_name.trim().is_empty();
                let enter_pressed = ui.input(|i| i.key_pressed(Key::Enter));

                if ui
                    .add_enabled(
                        name_valid,
                        egui::Button::new(btn_label),
                    )
                    .clicked()
                    || (enter_pressed && name_valid)
                {
                    let name = state.new_effect_name.trim().to_string();
                    let signal = if state.copy_builtin_mode {
                        "create_copy_effect"
                    } else {
                        "create_new_effect"
                    };
                    ctx.data_mut(|d| {
                        d.insert_temp(Id::new(signal), name);
                    });
                    state.new_effect_prompt = false;
                    state.new_effect_name.clear();
                    state.copy_builtin_mode = false;
                }

                if ui.button("Cancel").clicked()
                    || ui.input(|i| i.key_pressed(Key::Escape))
                {
                    state.new_effect_prompt = false;
                    state.new_effect_name.clear();
                    state.copy_builtin_mode = false;
                }
            });
        });
}

fn truncate_error(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let mut end = max_len;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &s[..end]
    }
}
