//! GG LAN — native desktop client for the GG LAN virtual gaming network.
//!
//! Manages one `gg-lan-agent` process per joined network (and can host a
//! `gg-lan-server` coordinator for locally-hosted networks), presenting a
//! Radmin VPN-style UI: networks list, peers, live traffic and diagnostics.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod http;
mod model;
mod theme;
mod view;

fn main() {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 720.0])
            .with_min_inner_size([940.0, 620.0]),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "GG LAN",
        opts,
        Box::new(|cc| {
            let mut v = egui::Visuals::light();
            v.panel_fill = theme::PAGE;
            v.window_fill = theme::CARD;
            v.extreme_bg_color = theme::PAGE;
            cc.egui_ctx.set_visuals(v);
            Ok(Box::new(view::GgApp::new()))
        }),
    ) {
        eprintln!("GG LAN failed to start: {e}");
        std::process::exit(1);
    }
}
