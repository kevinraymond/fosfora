use egui::{Color32, RichText, Ui, Vec2};

use crate::audio::AudioIndicator;
use crate::gpu::ShaderUniforms;
use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;
use crate::ui::widgets::{Mark, paint_mark};

/// The audio light when it isn't reconnecting: a shape per state, because
/// these three were one dot in three hues — identical in the hue-free themes.
fn audio_mark(audio: AudioIndicator) -> (Mark, &'static str) {
    match audio {
        AudioIndicator::Live => (Mark::Active, "live"),
        AudioIndicator::Quiet => (Mark::Warn, "the input has stopped sending audio"),
        AudioIndicator::Failed | AudioIndicator::Reconnecting { .. } => {
            (Mark::Fault, "no audio: the input could not be opened")
        }
    }
}

/// One status light: its mark, its name, and its state in words on hover.
/// Added right to left, so the mark ends up to the right of the name.
fn indicator(ui: &mut Ui, mark: Mark, name: &str, state: &str) {
    let tc = theme_colors(ui.ctx());
    let hover = format!("{name}: {state}");
    paint_mark(ui, mark).on_hover_text(&hover);
    ui.label(RichText::new(name).size(MONO_SIZE).color(tc.text_secondary))
        .on_hover_text(&hover);
    ui.add_space(6.0);
}

fn label(ui: &mut Ui, text: &str) {
    let tc = theme_colors(ui.ctx());
    ui.label(RichText::new(text).size(MONO_SIZE).color(tc.text_secondary));
}

/// Fixed-width value using monospace-style right-aligned layout.
fn fixed_value(ui: &mut Ui, text: &str, width: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(width, ui.spacing().interact_size.y),
        egui::Sense::hover(),
    );
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(SMALL_SIZE),
        color,
    );
    // Right-align the text within the fixed rect
    let text_pos = egui::pos2(
        rect.right() - galley.size().x,
        rect.center().y - galley.size().y * 0.5,
    );
    ui.painter().galley(text_pos, galley, color);
}

/// Duration to show status errors before auto-clearing.
const ERROR_DISPLAY_SECS: f64 = 6.0;

// UI status bar function requires multiple bool flags for indicator states
#[allow(clippy::fn_params_excessive_bools)]
pub fn draw_status_bar(
    ui: &mut Ui,
    shader_error: &Option<String>,
    uniforms: &ShaderUniforms,
    particle_count: Option<u32>,
    midi_enabled: bool,
    midi_recently_active: bool,
    osc_enabled: bool,
    osc_recently_active: bool,
    web_enabled: bool,
    web_client_count: usize,
    ndi_running: bool,
    v4l2_running: bool,
    spout_running: bool,
    syphon_running: bool,
    scene_active: bool,
    scene_cue: Option<(usize, usize)>,
    status_error: &Option<(String, std::time::Instant)>,
    preset_loading: Option<&str>,
    audio: AudioIndicator,
) {
    let tc = theme_colors(ui.ctx());

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;

        // Transient status error (higher priority, auto-clears)
        if let Some((msg, when)) = status_error {
            let elapsed = when.elapsed().as_secs_f64();
            if elapsed < ERROR_DISPLAY_SECS {
                ui.add_space(4.0);
                ui.colored_label(tc.error, RichText::new(msg).size(SMALL_SIZE));
            }
        }
        // Shader errors (dismissable)
        else if let Some(err) = shader_error {
            ui.add_space(4.0);
            ui.colored_label(
                tc.error,
                RichText::new(format!("ERR: {err}")).size(SMALL_SIZE),
            );
            if ui.small_button("×").clicked() {
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("dismiss_shader_error"), true));
            }
        }
        // Preset loading indicator
        else if let Some(name) = preset_loading {
            ui.add_space(4.0);
            // Pulsing dots animation
            let t = ui.input(|i| i.time);
            let dots = match ((t * 3.0) as usize) % 4 {
                0 => "",
                1 => ".",
                2 => "..",
                _ => "...",
            };
            ui.colored_label(
                tc.accent,
                RichText::new(format!("Loading {name}{dots}")).size(SMALL_SIZE),
            );
            ui.ctx().request_repaint();
        }
        // Keyboard hints when idle
        else {
            ui.add_space(4.0);
            ui.label(
                RichText::new("B bindings · C chains · D hide interface · F fullscreen")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );
        }

        // Push right-side items
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;

            // FPS (rightmost) — EMA-smoothed to avoid jitter
            let fps_raw = if uniforms.delta_time > 0.0 {
                1.0 / uniforms.delta_time
            } else {
                0.0
            };
            let fps_id = ui.id().with("smoothed_fps");
            let prev: f32 = ui.ctx().data_mut(|d| d.get_temp(fps_id).unwrap_or(fps_raw));
            let alpha = 0.02_f32;
            let smoothed = prev + alpha * (fps_raw - prev);
            ui.ctx().data_mut(|d| d.insert_temp(fps_id, smoothed));
            let fps = smoothed as u32;
            fixed_value(ui, &format!("{fps}"), 24.0, tc.text_secondary);
            label(ui, "FPS");

            ui.add_space(6.0);

            let running = |on: bool| {
                if on {
                    (Mark::Active, "sending")
                } else {
                    (Mark::Off, "off")
                }
            };

            // NDI — always show (no separate enabled flag; running=on)
            let (m, st) = running(ndi_running);
            indicator(ui, m, "NDI", st);

            // Virtual camera — only where the build can have one
            #[cfg(all(target_os = "linux", feature = "v4l2"))]
            {
                let (m, st) = running(v4l2_running);
                indicator(ui, m, "V4L", st);
            }
            #[cfg(not(all(target_os = "linux", feature = "v4l2")))]
            let _ = v4l2_running;

            // Spout — only where the build can have one
            #[cfg(all(target_os = "windows", feature = "spout"))]
            {
                let (m, st) = running(spout_running);
                indicator(ui, m, "SPT", st);
            }
            #[cfg(not(all(target_os = "windows", feature = "spout")))]
            let _ = spout_running;

            // Syphon — only where the build can have one
            #[cfg(all(target_os = "macos", feature = "syphon"))]
            {
                let (m, st) = running(syphon_running);
                indicator(ui, m, "SYP", st);
            }
            #[cfg(not(all(target_os = "macos", feature = "syphon")))]
            let _ = syphon_running;

            // Web, OSC, MIDI: on and receiving, on and quiet, or off.
            let link = |active: bool, enabled: bool, active_words: &'static str| {
                if active {
                    (Mark::Active, active_words)
                } else if enabled {
                    (Mark::Idle, "on, nothing arriving")
                } else {
                    (Mark::Off, "off")
                }
            };
            let web_words = if web_client_count == 1 {
                "1 client connected".to_string()
            } else {
                format!("{web_client_count} clients connected")
            };
            let (m, st) = link(web_client_count > 0, web_enabled, "");
            indicator(
                ui,
                m,
                "WEB",
                if m == Mark::Active { &web_words } else { st },
            );
            let (m, st) = link(osc_recently_active, osc_enabled, "receiving");
            indicator(ui, m, "OSC", st);
            let (m, st) = link(midi_recently_active, midi_enabled, "receiving");
            indicator(ui, m, "MIDI", st);

            // Audio capture health (A9 #1460) — the only light here that can
            // report a fault, so a reconnect also shows its attempt count.
            match audio {
                AudioIndicator::Reconnecting { attempt } => {
                    // Pulse between warning and idle so a reconnect in progress
                    // reads as activity, not a dead light.
                    let t = ui.input(|i| i.time);
                    let on = ((t * 3.0) as u64).is_multiple_of(2);
                    let max = crate::audio::reconnect::MAX_ATTEMPTS;
                    let words = format!("reconnecting, attempt {attempt} of {max}");
                    let mark = if on { Mark::Warn } else { Mark::Idle };
                    paint_mark(ui, mark).on_hover_text(format!("AUD: {words}"));
                    fixed_value(ui, &format!("{attempt}/{max}"), 22.0, tc.warning);
                    label(ui, "AUD");
                    ui.add_space(6.0);
                    ui.ctx().request_repaint();
                }
                steady => {
                    let (mark, words) = audio_mark(steady);
                    indicator(ui, mark, "AUD", words);
                }
            }

            // Particles
            if let Some(count) = particle_count {
                ui.label(
                    RichText::new(format!("{count}"))
                        .size(SMALL_SIZE)
                        .color(tc.text_primary),
                );
                label(ui, "PTL");
                ui.add_space(6.0);
            }

            // Scene
            if scene_active {
                if let Some((current, total)) = scene_cue {
                    fixed_value(
                        ui,
                        &format!("{}/{}", current + 1, total),
                        28.0,
                        tc.text_primary,
                    );
                }
                indicator(ui, Mark::Active, "SCN", "a scene is playing");
            }

            // BPM + beat dot
            let bpm = uniforms.bpm * 300.0;
            if bpm > 1.0 {
                let beat_on = uniforms.beat > 0.5;
                let bpm_color = if beat_on {
                    tc.beat_color
                } else {
                    tc.text_primary
                };
                // Fixed 3-char width for BPM value (prevents jitter on 2→3 digit changes)
                fixed_value(ui, &format!("{:.0}", bpm), 24.0, bpm_color);
                // The beat is a filled dot that empties between beats.
                let mark = if beat_on { Mark::Active } else { Mark::Idle };
                paint_mark(ui, mark);
                label(ui, "BPM");
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_audio_state_has_its_own_shape() {
        let marks = [
            audio_mark(AudioIndicator::Live).0,
            audio_mark(AudioIndicator::Quiet).0,
            audio_mark(AudioIndicator::Failed).0,
        ];
        assert_ne!(marks[0], marks[1]);
        assert_ne!(marks[1], marks[2]);
        assert_ne!(marks[0], marks[2]);
    }
}
