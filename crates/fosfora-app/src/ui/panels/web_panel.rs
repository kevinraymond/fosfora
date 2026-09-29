use egui::{Color32, RichText, Ui};

use crate::ui::theme::colors::theme_colors;
use crate::ui::theme::tokens::*;
use crate::web::WebSystem;

const WEB_BLUE: Color32 = Color32::from_rgb(0x50, 0x90, 0xE0);

pub fn draw_web_panel(ui: &mut Ui, web: &mut WebSystem) {
    let tc = theme_colors(ui.ctx());

    // Enable checkbox
    let mut enabled = web.config.enabled;
    if ui
        .checkbox(&mut enabled, RichText::new("Enable Web").size(SMALL_SIZE))
        .changed()
    {
        web.set_enabled(enabled);
    }

    // Port config
    ui.horizontal(|ui| {
        ui.label(RichText::new("Port").size(SMALL_SIZE));
        let mut port = web.config.port;
        let resp = ui.add(
            egui::DragValue::new(&mut port)
                .range(1024..=65535)
                .speed(1.0),
        );
        if resp.changed() {
            web.config.port = port;
            web.config.save();
            web.restart_server();
        }
    });
    let mut lan = web.config.lan;
    if ui
        .checkbox(
            &mut lan,
            RichText::new("Allow other devices").size(SMALL_SIZE),
        )
        .on_hover_text("Off: only this computer can connect; on: a phone or tablet can too")
        .changed()
    {
        web.set_lan(lan);
    }

    // URL display
    if web.is_running() {
        ui.separator();

        // Show all local addresses
        let port = web.config.port;

        // Show localhost
        let url = format!("http://localhost:{port}");
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("URL")
                    .size(SMALL_SIZE)
                    .color(tc.text_secondary),
            );
            if ui
                .link(RichText::new(&url).size(SMALL_SIZE).color(WEB_BLUE))
                .clicked()
            {
                ui.ctx().copy_text(url.clone());
            }
        });

        // Other devices: the LAN link carries the access key they need.
        if web.config.lan {
            let key = web.config.access_key.clone();
            if let Some(ip) = get_lan_ip() {
                let lan_url = format!("http://{ip}:{port}/?key={key}");
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("LAN")
                            .size(SMALL_SIZE)
                            .color(tc.text_secondary),
                    );
                    if ui
                        .link(RichText::new(&lan_url).size(SMALL_SIZE).color(WEB_BLUE))
                        .on_hover_text("Click to copy. Includes the access key.")
                        .clicked()
                    {
                        ui.ctx().copy_text(lan_url.clone());
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Key")
                        .size(SMALL_SIZE)
                        .color(tc.text_secondary),
                );
                if ui
                    .link(RichText::new(&key).size(SMALL_SIZE).monospace())
                    .on_hover_text("Click to copy. Bridges take it as --key or FOSFORA_KEY.")
                    .clicked()
                {
                    ui.ctx().copy_text(key.clone());
                }
                if ui
                    .small_button("New")
                    .on_hover_text("Disconnects every device using the current key")
                    .clicked()
                {
                    web.replace_access_key();
                }
            });
        }
    }
}

/// Try to find a LAN IP address (non-loopback IPv4).
fn get_lan_ip() -> Option<String> {
    // Simple approach: bind a UDP socket to an external address and check local_addr
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    let ip = addr.ip();
    if ip.is_loopback() {
        None
    } else {
        Some(ip.to_string())
    }
}
