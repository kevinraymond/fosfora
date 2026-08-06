#![allow(dead_code)]
use egui::Color32;

// Dark theme colors (WCAG 2.2 AA verified)
pub const DARK_CANVAS: Color32 = Color32::from_rgb(0x12, 0x12, 0x12);
pub const DARK_PANEL: Color32 = Color32::from_rgba_premultiplied(0x22, 0x22, 0x22, 0xE6);
pub const DARK_TEXT_PRIMARY: Color32 = Color32::from_rgb(0xE8, 0xE8, 0xE8); // 12.8:1
pub const DARK_TEXT_SECONDARY: Color32 = Color32::from_rgb(0xA0, 0xA0, 0xA0); // 6.2:1
pub const DARK_ACCENT: Color32 = Color32::from_rgb(0x44, 0x88, 0xFF); // VJ accent blue
pub const DARK_ERROR: Color32 = Color32::from_rgb(0xE0, 0x60, 0x60);
pub const DARK_WARNING: Color32 = Color32::from_rgb(0xD4, 0xA0, 0x40);
pub const DARK_SUCCESS: Color32 = Color32::from_rgb(0x50, 0xC0, 0x70);
pub const DARK_WIDGET_BG: Color32 = Color32::from_rgb(0x2A, 0x2A, 0x2A);
pub const DARK_WIDGET_BG_HOVER: Color32 = Color32::from_rgb(0x35, 0x35, 0x35);
pub const DARK_WIDGET_BG_ACTIVE: Color32 = Color32::from_rgb(0x40, 0x40, 0x40);
pub const DARK_SEPARATOR: Color32 = Color32::from_rgb(0x3A, 0x3A, 0x3A);

// Card/section colors
pub const CARD_BG: Color32 = Color32::from_rgb(0x24, 0x24, 0x24);
pub const CARD_BORDER: Color32 = Color32::from_rgb(0x33, 0x33, 0x33);
pub const BEAT_COLOR: Color32 = Color32::from_rgb(0xFF, 0x55, 0x77);
pub const METER_BG: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A);

// Light theme colors (WCAG 2.2 AA verified)
pub const LIGHT_CANVAS: Color32 = Color32::from_rgb(0xF5, 0xF5, 0xF5);
pub const LIGHT_PANEL: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
pub const LIGHT_TEXT_PRIMARY: Color32 = Color32::from_rgb(0x1A, 0x1A, 0x1A); // 17.4:1
pub const LIGHT_TEXT_SECONDARY: Color32 = Color32::from_rgb(0x5A, 0x5A, 0x5A);
pub const LIGHT_ACCENT: Color32 = Color32::from_rgb(0x09, 0x69, 0xA8); // 6.1:1
pub const LIGHT_ERROR: Color32 = Color32::from_rgb(0xC0, 0x30, 0x30);
pub const LIGHT_WARNING: Color32 = Color32::from_rgb(0xA0, 0x70, 0x10);
pub const LIGHT_SUCCESS: Color32 = Color32::from_rgb(0x20, 0x80, 0x40);
pub const LIGHT_WIDGET_BG: Color32 = Color32::from_rgb(0xE8, 0xE8, 0xE8);
pub const LIGHT_WIDGET_BG_HOVER: Color32 = Color32::from_rgb(0xDD, 0xDD, 0xDD);
pub const LIGHT_WIDGET_BG_ACTIVE: Color32 = Color32::from_rgb(0xD0, 0xD0, 0xD0);
pub const LIGHT_SEPARATOR: Color32 = Color32::from_rgb(0xD5, 0xD5, 0xD5);

// Layout constants
pub const PANEL_ROUNDING: u8 = 6;
pub const WIDGET_ROUNDING: u8 = 4;
pub const CARD_ROUNDING: u8 = 8;
pub const CARD_PADDING: f32 = 8.0;
pub const CARD_MARGIN: f32 = 4.0;
pub const METER_HEIGHT: f32 = 48.0;
pub const METER_GAP: f32 = 3.0;
pub const SPACING: f32 = 6.0;
pub const SPACING_Y: f32 = 4.0;
pub const MIN_INTERACT_HEIGHT: f32 = 22.0;
pub const MIN_INTERACT_WIDTH: f32 = 40.0;
pub const FOCUS_RING_WIDTH: f32 = 2.0;

// Typography
pub const BODY_SIZE: f32 = 12.0;
pub const HEADING_SIZE: f32 = 11.0;
pub const MONO_SIZE: f32 = 10.0;
pub const SMALL_SIZE: f32 = 10.0;
