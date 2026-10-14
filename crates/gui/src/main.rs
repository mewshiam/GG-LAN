//! GG LAN — native desktop client for the GG LAN virtual gaming network.
//!
//! Manages one `gg-lan-agent` process per joined network (and can host a
//! `gg-lan-server` coordinator for locally-hosted networks), presenting a
//! Material-3-inspired UI: networks sidebar, live dashboard, traffic charts,
//! peers and diagnostics.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod components;
mod http;
mod icons;
mod model;
mod theme;
mod view;

fn main() {
    // Renderer selection: wgpu by default; set GG_LAN_RENDERER=glow for the
    // OpenGL fallback (useful on GPU-less machines, VMs and headless sessions).
    let renderer = match std::env::var("GG_LAN_RENDERER").as_deref() {
        Ok("glow") => eframe::Renderer::Glow,
        _ => eframe::Renderer::default(),
    };
    let opts = eframe::NativeOptions {
        renderer,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([760.0, 540.0]),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "GG LAN",
        opts,
        Box::new(|cc| {
            // Palette + visuals are applied every frame by the app itself
            // (see view::GgApp::render), including the persisted theme
            // preference and OS dark-mode detection.
            let _ = cc;
            Ok(Box::new(view::GgApp::new()))
        }),
    ) {
        eprintln!("GG LAN failed to start: {e}");
        std::process::exit(1);
    }
}
