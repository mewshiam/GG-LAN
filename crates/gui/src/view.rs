//! GG LAN native client UI (eframe/egui, Radmin VPN-style layout).

use crate::model::*;
use crate::theme::*;
use egui::{Align2, Color32, CornerRadius, RichText, Sense, Stroke, Ui, Vec2};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct GgApp {
    pub cfg: GuiConfig,
    pub cfg_path: PathBuf,
    pub cfg_dir: PathBuf,
    pub identity: PathBuf,
    pub nets: Vec<NetEntry>,
    pub selected: Option<usize>,
    pub bus: EventBus,
    pub pings: HashMap<(usize, String), String>,

    pub join_open: bool,
    pub join_name: String,
    pub join_server: String,
    pub join_code: String,
    pub join_now: bool,

    pub create_open: bool,
    pub create_name: String,
    pub create_mode: String,
    pub create_now: bool,

    pub settings_open: bool,
    /// Set by dialog bodies to request closing (the `open` bool is borrowed
    /// by the window while the body runs).
    dlg_close: bool,
    pub toast: Option<(String, Instant, bool)>,
    pub confirm_remove: Option<(usize, Instant)>,
}

impl eframe::App for GgApp {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.render(ui, frame);
    }
}

impl GgApp {
    pub fn new() -> Self {
        let (cfg, cfg_path) = load_config();
        let cfg_dir = config_dir();
        let _ = std::fs::create_dir_all(&cfg_dir);
        let identity = ensure_identity(&cfg_dir);
        let nets: Vec<NetEntry> = cfg.networks.iter().cloned().map(NetEntry::new).collect();
        let mut app = Self {
            nets,
            cfg,
            cfg_path,
            cfg_dir: cfg_dir.clone(),
            identity,
            bus: EventBus::default(),
            pings: HashMap::new(),
            selected: None,
            join_open: false,
            join_name: String::new(),
            join_server: String::new(),
            join_code: String::new(),
            join_now: true,
            create_open: false,
            create_name: String::new(),
            create_mode: "mesh".into(),
            create_now: true,
            settings_open: false,
            dlg_close: false,
            toast: None,
            confirm_remove: None,
        };
        let auto: Vec<usize> = app
            .nets
            .iter()
            .enumerate()
            .filter(|(_, n)| n.cfg.enabled)
            .map(|(i, _)| i)
            .collect();
        for i in auto {
            app.connect(i);
        }
        app
    }

    // ------------------------------------------------------------ lifecycle

    fn save(&self) {
        save_config(&self.cfg, &self.cfg_path);
    }

    /// Start (or restart) the agent for network `i`, spinning up the local
    /// coordinator first when this is a hosted network.
    fn connect(&mut self, i: usize) {
        self.stop_procs(i, false);
        let Some(net) = self.nets.get(i) else { return };
        if net.cfg.local && net.server.is_none() {
            match start_local_server(&self.cfg, i, &net.cfg, &self.cfg_dir, self.bus.tx.clone()) {
                Ok((sp, addr)) => {
                    self.nets[i].server = Some(sp);
                    self.nets[i].cfg.server = addr;
                    self.cfg.networks[i].server = self.nets[i].cfg.server.clone();
                }
                Err(e) => {
                    self.toast(format!("Coordinator failed: {e:#}"), false);
                    return;
                }
            }
        }
        let net = self.nets[i].cfg.clone();
        match start_agent(&self.cfg, i, &net, &self.identity, self.bus.tx.clone()) {
            Ok(a) => {
                self.nets[i].agent = Some(a);
                self.nets[i].reset_crash_notice();
                self.nets[i].cfg.enabled = true;
                self.cfg.networks[i].enabled = true;
                self.save();
            }
            Err(e) => self.toast(format!("Agent failed: {e:#}"), false),
        }
    }

    /// Stop agent (and optionally the local coordinator) for network `i`.
    fn stop_procs(&mut self, i: usize, drop_server: bool) {
        if let Some(n) = self.nets.get_mut(i) {
            if let Some(mut a) = n.agent.take() {
                a.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                let _ = a.child.kill();
                let _ = a.child.wait();
            }
            if drop_server {
                if let Some(mut s) = n.server.take() {
                    let _ = s.child.kill();
                    let _ = s.child.wait();
                }
            }
            n.status = None;
            n.last_ok = None;
        }
    }

    fn disconnect(&mut self, i: usize) {
        self.stop_procs(i, true);
        if let Some(n) = self.nets.get_mut(i) {
            n.cfg.enabled = false;
        }
        if let Some(c) = self.cfg.networks.get_mut(i) {
            c.enabled = false;
        }
        self.save();
    }

    fn remove_net(&mut self, i: usize) {
        self.stop_procs(i, true);
        if i < self.nets.len() {
            self.nets.remove(i);
            self.cfg.networks.remove(i);
        }
        self.pings.clear();
        self.selected = None;
        self.confirm_remove = None;
        self.save();
    }

    pub fn shutdown(&mut self) {
        for i in 0..self.nets.len() {
            self.stop_procs(i, true);
        }
    }

    fn toast(&mut self, text: impl Into<String>, ok: bool) {
        self.toast = Some((text.into(), Instant::now(), ok));
    }

    // ------------------------------------------------------------ events

    fn pump_events(&mut self) {
        while let Ok(ev) = self.bus.rx.try_recv() {
            match ev {
                Event::Status(i, v) => {
                    if let Some(n) = self.nets.get_mut(i) {
                        n.status = Some(v);
                        n.last_ok = Some(Instant::now());
                    }
                }
                Event::Log(i, l) => {
                    if let Some(n) = self.nets.get_mut(i) {
                        n.push_log(l);
                    }
                }
                Event::Ping(i, ip, text) => {
                    self.pings.insert((i, ip), text);
                }
            }
        }
        for i in 0..self.nets.len() {
            if self.nets[i].take_crash() {
                let name = self.nets[i].cfg.name.clone();
                self.toast(format!("Agent for '{name}' stopped — see event log"), false);
            }
        }
    }

    // ------------------------------------------------------------ layout

    fn render(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump_events();
        ctx.request_repaint_after(Duration::from_millis(400));

        egui::Panel::top("gg_header")
            .frame(header_frame())
            .show(ui, |ui| self.header(ui));
        egui::Panel::left("gg_nets")
            .exact_size(312.0)
            .frame(panel_frame())
            .show(ui, |ui| self.sidebar(ui));
        egui::CentralPanel::default()
            .frame(panel_frame())
            .show(ui, |ui| self.central(ui));

        self.dialogs(&ctx);
        self.draw_toast(&ctx);
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(r, CornerRadius::same(8), ACCENT);
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                "GG",
                egui::FontId::proportional(13.5),
                Color32::WHITE,
            );
            ui.add_space(8.0);
            ui.label(RichText::new("GG LAN").size(16.0).strong().color(INK));
            ui.label(
                RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                    .size(11.0)
                    .color(MUTED),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(RichText::new("Settings").size(12.5))
                    .on_hover_text("Nickname, binaries, config location")
                    .clicked()
                {
                    self.settings_open = true;
                }
                let any_on = self.nets.iter().any(|n| n.agent.is_some());
                let (label, enable) = if any_on {
                    ("Disconnect all", false)
                } else {
                    ("Connect all", true)
                };
                let b = egui::Button::new(RichText::new(label).size(12.5).color(Color32::WHITE));
                let b = if enable {
                    b.fill(ACCENT)
                } else {
                    b.fill(Color32::from_rgb(189, 193, 198))
                };
                if ui.add(b).clicked() {
                    if enable {
                        for i in 0..self.nets.len() {
                            self.connect(i);
                        }
                    } else {
                        for i in 0..self.nets.len() {
                            self.disconnect(i);
                        }
                    }
                }
            });
        });
    }

    // ------------------------------------------------------------ sidebar

    fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("NETWORKS").size(11.5).strong().color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(RichText::new("+ Host").size(12.0))
                    .on_hover_text("Create and host a new network on this machine")
                    .clicked()
                {
                    self.create_open = true;
                }
                if ui
                    .button(RichText::new("+ Join").size(12.0))
                    .on_hover_text("Join an existing network with a join code")
                    .clicked()
                {
                    self.join_open = true;
                }
            });
        });
        ui.add_space(4.0);

        let mut toggle: Option<(usize, bool)> = None;
        let mut select: Option<usize> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.nets.is_empty() {
                    ui.add_space(16.0);
                    ui.label(
                        RichText::new(
                            "No networks yet.\nJoin a friend's network with a code,\nor host your own LAN party.",
                        )
                        .size(12.5)
                        .color(MUTED),
                    );
                }
                for i in 0..self.nets.len() {
                    self.net_card(ui, i, &mut toggle, &mut select);
                }
            });

        if let Some(i) = select {
            self.selected = Some(i);
        }
        if let Some((i, on)) = toggle {
            if on {
                self.connect(i);
            } else {
                self.disconnect(i);
            }
        }
    }

    fn net_card(
        &mut self,
        ui: &mut Ui,
        i: usize,
        toggle: &mut Option<(usize, bool)>,
        select: &mut Option<usize>,
    ) {
        let selected = self.selected == Some(i);
        let name = self.nets[i].cfg.name.clone();
        let server = self.nets[i].cfg.server.clone();
        let local = self.nets[i].cfg.local;
        let mode = self.nets[i].cfg.mode.clone();
        let st = self.nets[i].state();
        let peers_n = self.nets[i]
            .status
            .as_ref()
            .and_then(|s| s.get("peers"))
            .and_then(|p| p.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let mut on = self.nets[i].cfg.enabled;

        let (fg, bg) = state_colors(st);
        let card = egui::Frame::group(ui.style())
            .fill(if selected { ACCENT_SOFT } else { CARD })
            .stroke(Stroke::new(
                if selected { 1.6 } else { 1.0 },
                if selected { ACCENT } else { HAIRLINE },
            ))
            .corner_radius(10)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    pill(ui, st.label(), fg, bg);
                    if local {
                        pill(ui, "HOST", ACCENT, ACCENT_SOFT);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let sw = switch(ui, &mut on);
                        if sw.changed() {
                            *toggle = Some((i, on));
                        }
                        sw.on_hover_text("Connect / disconnect");
                    });
                });
                ui.add_space(2.0);
                ui.label(RichText::new(name).size(14.0).strong().color(INK));
                ui.label(
                    RichText::new(format!(
                        "{server} · {peers_n} peer{}",
                        if peers_n == 1 { "" } else { "s" }
                    ))
                    .size(11.5)
                    .color(MUTED),
                );
                ui.label(
                    RichText::new(if mode == "hub" {
                        "hub mode — traffic relayed"
                    } else {
                        "mesh mode — direct P2P"
                    })
                    .size(10.5)
                    .color(MUTED),
                );
            });
        let rect = card.response.rect;
        let resp = ui.interact(rect, egui::Id::new("netcard").with(i), Sense::click());
        if resp.clicked() {
            *select = Some(i);
        }
        ui.add_space(6.0);
    }

    // ------------------------------------------------------------ central

    fn central(&mut self, ui: &mut Ui) {
        let Some(i) = self.selected.filter(|i| *i < self.nets.len()) else {
            self.hero(ui);
            return;
        };
        let name = self.nets[i].cfg.name.clone();
        let server = self.nets[i].cfg.server.clone();
        let local = self.nets[i].cfg.local;
        let st = self.nets[i].state();
        let status = self.nets[i].status.clone();
        let log: Vec<String> = self.nets[i].log.iter().cloned().collect();
        let status_port = self.nets[i].agent.as_ref().map(|a| a.status_port);

        ui.add_space(12.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // ---- title row
                let mut do_remove = false;
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(name).size(20.0).strong().color(INK));
                    let (fg, bg) = state_colors(st);
                    pill(ui, st.label(), fg, bg);
                    if local {
                        pill(ui, "HOSTED HERE", ACCENT, ACCENT_SOFT);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // remove with confirm
                        if let Some((ci, at)) = self.confirm_remove {
                            if ci == i && at.elapsed() < Duration::from_secs(3) {
                                if ui
                                    .button(RichText::new("Really remove?").size(12.0).color(BAD))
                                    .clicked()
                                {
                                    do_remove = true;
                                }
                                if ui.button(RichText::new("Keep").size(12.0)).clicked() {
                                    self.confirm_remove = None;
                                }
                            } else {
                                self.confirm_remove = None;
                            }
                        }
                        if self.confirm_remove.is_none()
                            && ui
                                .button(RichText::new("Remove").size(12.0).color(MUTED))
                                .on_hover_text("Forget this network")
                                .clicked()
                        {
                            self.confirm_remove = Some((i, Instant::now()));
                        }
                        // connect / disconnect
                        if self.nets[i].agent.is_some() {
                            if ui.button(RichText::new("Disconnect").size(12.0)).clicked() {
                                self.disconnect(i);
                            }
                        } else if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Connect").size(12.0).color(Color32::WHITE),
                                )
                                .fill(ACCENT),
                            )
                            .clicked()
                        {
                            self.connect(i);
                        }
                    });
                });
                if do_remove {
                    self.remove_net(i);
                    return;
                }
                ui.label(
                    RichText::new(format!("coordinator {server}"))
                        .size(11.5)
                        .color(MUTED),
                );
                ui.add_space(10.0);

                // ---- stat cards
                let (my_ip, peers, rx_now, tx_now, rx_total, tx_total) = stat_snapshot(&status);
                ui.horizontal(|ui| {
                    stat_card(ui, "STATUS", st.label().into(), state_colors(st).0);
                    stat_card(ui, "MY ADDRESS", my_ip.clone(), INK);
                    stat_card(
                        ui,
                        "PEERS",
                        peers.len().to_string(),
                        if peers.is_empty() { MUTED } else { INK },
                    );
                    stat_card(ui, "DOWNLOAD", fmt_bps(rx_now), GOOD);
                    stat_card(ui, "UPLOAD", fmt_bps(tx_now), WARN);
                    stat_card(ui, "TOTAL TRAFFIC", fmt_bytes(rx_total + tx_total), INK);
                });
                ui.add_space(10.0);

                // ---- traffic charts
                let (rx_series, tx_series) = series_snapshot(&status);
                card_frame(ui).show(ui, |ui| {
                    ui.label(
                        RichText::new("TRAFFIC — LAST 60 SECONDS")
                            .size(11.0)
                            .strong()
                            .color(MUTED),
                    );
                    ui.columns(2, |cols| {
                        let inner = spark_chart(&mut cols[0], &rx_series, GOOD, 110.0);
                        cols[0].painter().text(
                            inner.right_top(),
                            Align2::RIGHT_TOP,
                            format!("down {rx_now}"),
                            egui::FontId::proportional(11.0),
                            GOOD,
                        );
                        let inner = spark_chart(&mut cols[1], &tx_series, WARN, 110.0);
                        cols[1].painter().text(
                            inner.right_top(),
                            Align2::RIGHT_TOP,
                            format!("up {tx_now}"),
                            egui::FontId::proportional(11.0),
                            WARN,
                        );
                    });
                });
                ui.add_space(10.0);

                // ---- peers table
                card_frame(ui).show(ui, |ui| {
                    ui.label(RichText::new("PEERS").size(11.0).strong().color(MUTED));
                    ui.add_space(4.0);
                    if peers.is_empty() {
                        ui.label(
                            RichText::new("No other peers connected to this network yet.")
                                .size(12.0)
                                .color(MUTED),
                        );
                    } else {
                        egui::Grid::new("peers")
                            .num_columns(6)
                            .striped(true)
                            .min_row_height(30.0)
                            .spacing([18.0, 2.0])
                            .show(ui, |ui| {
                                for h in ["NAME", "IP", "PATH", "RTT", "TRAFFIC", ""] {
                                    ui.label(RichText::new(h).size(10.5).strong().color(MUTED));
                                }
                                ui.end_row();
                                for p in &peers {
                                    ui.label(RichText::new(p.name.clone()).size(12.5).color(INK));
                                    ui.label(
                                        RichText::new(p.ip.clone())
                                            .size(12.5)
                                            .color(MUTED)
                                            .monospace(),
                                    );
                                    let (fg, bg) = path_colors(&p.path);
                                    pill(ui, &p.path, fg, bg);
                                    ui.label(
                                        RichText::new(
                                            p.rtt
                                                .map(|r| format!("{r} ms"))
                                                .unwrap_or_else(|| "—".into()),
                                        )
                                        .size(12.5)
                                        .color(INK),
                                    );
                                    ui.label(
                                        RichText::new(fmt_bytes(p.bytes_in + p.bytes_out))
                                            .size(12.5)
                                            .color(MUTED),
                                    );
                                    ui.horizontal(|ui| {
                                        if let Some(port) = status_port {
                                            if ui.button(RichText::new("Ping").size(11.0)).clicked()
                                            {
                                                self.pings.insert((i, p.ip.clone()), "…".into());
                                                fire_ping(
                                                    i,
                                                    port,
                                                    p.ip.clone(),
                                                    self.bus.tx.clone(),
                                                );
                                            }
                                            if let Some(res) = self.pings.get(&(i, p.ip.clone())) {
                                                ui.label(
                                                    RichText::new(res.clone())
                                                        .size(11.0)
                                                        .color(MUTED),
                                                );
                                            }
                                        } else {
                                            ui.label(
                                                RichText::new("offline").size(11.0).color(MUTED),
                                            );
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                    }
                });
                ui.add_space(10.0);

                // ---- event log
                egui::CollapsingHeader::new(
                    RichText::new("EVENT LOG").size(11.0).strong().color(MUTED),
                )
                .default_open(false)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .stick_to_bottom(true)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for l in log.iter().rev() {
                                ui.label(
                                    RichText::new(l.clone()).size(11.0).monospace().color(MUTED),
                                );
                            }
                        });
                });
                ui.add_space(8.0);
            });
    }

    fn hero(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            let (r, _) = ui.allocate_exact_size(Vec2::splat(64.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(r, CornerRadius::same(16), ACCENT);
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                "GG",
                egui::FontId::proportional(28.0),
                Color32::WHITE,
            );
            ui.add_space(16.0);
            ui.label(RichText::new("GG LAN").size(28.0).strong().color(INK));
            ui.add_space(6.0);
            ui.label(
                RichText::new("A virtual LAN for your games — anywhere.")
                    .size(14.0)
                    .color(MUTED),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new("Encrypted peer-to-peer overlay network with NAT traversal,")
                    .size(12.0)
                    .color(MUTED),
            );
            ui.label(
                RichText::new("designed for LAN parties over the internet.")
                    .size(12.0)
                    .color(MUTED),
            );
            ui.add_space(22.0);
            ui.horizontal(|ui| {
                let b = egui::Button::new(
                    RichText::new("Join a network")
                        .size(13.0)
                        .color(Color32::WHITE),
                )
                .fill(ACCENT);
                if ui.add(b).clicked() {
                    self.join_open = true;
                }
                ui.add_space(6.0);
                let b = egui::Button::new(RichText::new("Host a network").size(13.0));
                if ui.add(b).clicked() {
                    self.create_open = true;
                }
            });
            ui.add_space(30.0);
            ui.label(
                RichText::new(format!(
                    "nickname \u{201c}{}\u{201d} · config {}",
                    self.cfg.nickname,
                    self.cfg_dir.display()
                ))
                .size(10.5)
                .color(MUTED),
            );
        });
    }

    // ------------------------------------------------------------ dialogs

    fn dialogs(&mut self, ctx: &egui::Context) {
        // ---- join
        let mut join_open = self.join_open;
        egui::Window::new("Join a network")
            .open(&mut join_open)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                self.join_body(ui);
            });
        if self.dlg_close {
            join_open = false;
            self.dlg_close = false;
        }
        self.join_open = join_open;

        // ---- create / host
        let mut create_open = self.create_open;
        egui::Window::new("Host a network")
            .open(&mut create_open)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                self.create_body(ui);
            });
        if self.dlg_close {
            create_open = false;
            self.dlg_close = false;
        }
        self.create_open = create_open;

        // ---- settings
        let mut settings_open = self.settings_open;
        egui::Window::new("Settings")
            .open(&mut settings_open)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                self.settings_body(ui);
            });
        if self.dlg_close {
            settings_open = false;
            self.dlg_close = false;
        }
        self.settings_open = settings_open;
    }

    fn join_body(&mut self, ui: &mut Ui) {
        ui.set_min_width(380.0);
        ui.label(
            RichText::new("Ask the host for their address and join code.")
                .size(12.0)
                .color(MUTED),
        );
        ui.add_space(8.0);
        field(
            ui,
            "Network name",
            &mut self.join_name,
            "e.g. Friday Night Games",
        );
        field(
            ui,
            "Server address",
            &mut self.join_server,
            "host or ip:port (default :7100)",
        );
        field(
            ui,
            "Join code",
            &mut self.join_code,
            "the code the host shared with you",
        );
        ui.add_space(4.0);
        ui.checkbox(&mut self.join_now, "Connect immediately");
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("Join network")
                            .size(12.5)
                            .color(Color32::WHITE),
                    )
                    .fill(ACCENT),
                )
                .clicked()
            {
                self.join_submit();
            }
            if ui.button(RichText::new("Cancel").size(12.5)).clicked() {
                self.dlg_close = true;
            }
        });
    }

    fn join_submit(&mut self) {
        let mut name = self.join_name.trim().to_string();
        let mut server = self.join_server.trim().to_string();
        let code = self.join_code.trim().to_string();
        if name.is_empty() {
            name = "network".into();
        }
        if !server.is_empty() && !server.contains(':') {
            server = format!("{server}:7100");
        }
        if server.is_empty() || code.is_empty() {
            self.toast("Server address and join code are required", false);
            return;
        }
        let cfg = NetCfg {
            name,
            server,
            code,
            mode: "mesh".into(),
            local: false,
            enabled: false,
        };
        self.cfg.networks.push(cfg.clone());
        self.nets.push(NetEntry::new(cfg));
        let idx = self.nets.len() - 1;
        self.selected = Some(idx);
        self.save();
        self.dlg_close = true;
        self.join_name.clear();
        self.join_server.clear();
        self.join_code.clear();
        if self.join_now {
            self.connect(idx);
        }
        self.toast("Network added", true);
    }

    fn create_body(&mut self, ui: &mut Ui) {
        ui.set_min_width(420.0);
        ui.label(
            RichText::new("Host a coordinator on this machine and share the code with friends.")
                .size(12.0)
                .color(MUTED),
        );
        ui.add_space(8.0);
        field(
            ui,
            "Network name",
            &mut self.create_name,
            "e.g. CS2 Lan Party",
        );
        ui.label(RichText::new("Mode").size(11.5).color(MUTED));
        egui::ComboBox::from_id_salt("create_mode")
            .selected_text(if self.create_mode == "hub" {
                "Hub — all traffic relayed through the host"
            } else {
                "Mesh — direct peer-to-peer when possible"
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.create_mode,
                    "mesh".into(),
                    "Mesh — direct peer-to-peer when possible",
                );
                ui.selectable_value(
                    &mut self.create_mode,
                    "hub".into(),
                    "Hub — all traffic relayed through the host",
                );
            });
        ui.add_space(4.0);
        ui.checkbox(&mut self.create_now, "Start hosting now");
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("Create network")
                            .size(12.5)
                            .color(Color32::WHITE),
                    )
                    .fill(ACCENT),
                )
                .clicked()
            {
                self.create_submit();
            }
            if ui.button(RichText::new("Cancel").size(12.5)).clicked() {
                self.dlg_close = true;
            }
        });
    }

    fn create_submit(&mut self) {
        let mut name = self.create_name.trim().to_string();
        if name.is_empty() {
            name = "my lan".into();
        }
        let code = new_join_code();
        let cfg = NetCfg {
            name: name.clone(),
            server: String::new(), // filled when the local server binds
            code,
            mode: if self.create_mode == "hub" {
                "hub".into()
            } else {
                "mesh".into()
            },
            local: true,
            enabled: false,
        };
        self.cfg.networks.push(cfg.clone());
        self.nets.push(NetEntry::new(cfg));
        let idx = self.nets.len() - 1;
        self.selected = Some(idx);
        self.save();
        self.dlg_close = true;
        self.create_name.clear();
        if self.create_now {
            self.connect(idx);
            let port = self.nets[idx]
                .cfg
                .server
                .rsplit(':')
                .next()
                .and_then(|p| p.parse::<u16>().ok());
            let code = self.nets[idx].cfg.code.clone();
            if let Some(port) = port {
                if let Some(addr) = lan_address(port) {
                    self.toast(
                        format!(
                            "Hosting \u{201c}{name}\u{201d} — friends join {addr} with code {code}"
                        ),
                        true,
                    );
                } else {
                    self.toast(
                        format!("Hosting \u{201c}{name}\u{201d} — code {code}"),
                        true,
                    );
                }
            } else {
                self.toast(format!("Hosting \u{201c}{name}\u{201d}"), true);
            }
        } else {
            self.toast(
                format!("Network \u{201c}{name}\u{201d} created (not started)"),
                true,
            );
        }
    }

    fn settings_body(&mut self, ui: &mut Ui) {
        ui.set_min_width(420.0);
        ui.label(
            RichText::new("How this machine appears to peers.")
                .size(12.0)
                .color(MUTED),
        );
        field(
            ui,
            "Nickname",
            &mut self.cfg.nickname,
            "shown to other players",
        );
        ui.add_space(6.0);
        field(
            ui,
            "Agent binary",
            &mut self.cfg.agent_path,
            "blank = auto-detect (next to GG LAN)",
        );
        field(
            ui,
            "Coordinator binary",
            &mut self.cfg.server_path,
            "blank = auto-detect (next to GG LAN)",
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!("config: {}", self.cfg_path.display()))
                .size(10.5)
                .color(MUTED),
        );
        ui.label(
            RichText::new(format!("identity: {}", self.identity.display()))
                .size(10.5)
                .color(MUTED),
        );
        ui.add_space(10.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new("Save").size(12.5).color(Color32::WHITE))
                        .fill(ACCENT),
                )
                .clicked()
            {
                self.save();
                self.dlg_close = true;
            }
            if ui.button(RichText::new("Cancel").size(12.5)).clicked() {
                self.dlg_close = true;
            }
        });
    }

    // ------------------------------------------------------------ toast

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((text, at, ok)) = self.toast.clone() else {
            return;
        };
        let age = at.elapsed().as_secs_f32();
        if age > 4.0 {
            self.toast = None;
            return;
        }
        let fade = if age > 3.0 { 1.0 - (age - 3.0) } else { 1.0 };
        let alpha = (fade * 235.0) as u8;
        let color = if ok { GOOD } else { BAD };
        let bg = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
        egui::Area::new(egui::Id::new("gg_toast"))
            .anchor(Align2::CENTER_BOTTOM, [0.0, -28.0])
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::group(ui.style())
                    .fill(bg)
                    .stroke(Stroke::NONE)
                    .corner_radius(8)
                    .inner_margin(egui::Margin::symmetric(14, 8))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(text)
                                .size(12.5)
                                .strong()
                                .color(Color32::WHITE),
                        );
                    });
            });
    }
}

impl Drop for GgApp {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------- helpers

fn field(ui: &mut Ui, label: &str, value: &mut String, hint: &str) {
    ui.label(RichText::new(label).size(11.5).color(MUTED));
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(f32::INFINITY),
    );
    ui.add_space(4.0);
}

fn state_colors(st: NetState) -> (Color32, Color32) {
    match st {
        NetState::Connected => (GOOD, GOOD_SOFT),
        NetState::Waiting | NetState::Starting => (WARN, WARN_SOFT),
        NetState::Dead => (BAD, BAD_SOFT),
        NetState::Off => (IDLE, IDLE_SOFT),
    }
}

fn path_colors(path: &str) -> (Color32, Color32) {
    match path {
        "direct" => (GOOD, GOOD_SOFT),
        "relay" => (WARN, WARN_SOFT),
        _ => (IDLE, IDLE_SOFT),
    }
}

fn header_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, HAIRLINE))
}

fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PAGE)
        .inner_margin(egui::Margin::same(16))
        .stroke(Stroke::NONE)
}

struct PeerView {
    name: String,
    ip: String,
    path: String,
    rtt: Option<u64>,
    bytes_in: u64,
    bytes_out: u64,
}

fn stat_snapshot(
    status: &Option<serde_json::Value>,
) -> (String, Vec<PeerView>, u64, u64, u64, u64) {
    let s = match status {
        Some(s) => s,
        None => return (String::new(), Vec::new(), 0, 0, 0, 0),
    };
    let my_ip = s
        .pointer("/my/ip")
        .and_then(|v| v.as_str())
        .unwrap_or("—")
        .to_string();
    let peers: Vec<PeerView> = s
        .get("peers")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .map(|p| PeerView {
                    name: p.get("name").and_then(|v| v.as_str()).unwrap_or("?").into(),
                    ip: p.get("ip").and_then(|v| v.as_str()).unwrap_or("?").into(),
                    path: p.get("path").and_then(|v| v.as_str()).unwrap_or("?").into(),
                    rtt: p.get("rtt_ms").and_then(|v| v.as_u64()),
                    bytes_in: p.get("bytes_in").and_then(|v| v.as_u64()).unwrap_or(0),
                    bytes_out: p.get("bytes_out").and_then(|v| v.as_u64()).unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();
    let g = |k: &str| s.pointer(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let last = |k: &str| {
        s.pointer(k)
            .and_then(|v| v.as_array())
            .and_then(|a| a.last())
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    };
    (
        my_ip,
        peers,
        last("/series/rx_bps"),
        last("/series/tx_bps"),
        g("/counters/rx_bytes"),
        g("/counters/tx_bytes"),
    )
}

fn series_snapshot(status: &Option<serde_json::Value>) -> (Vec<u64>, Vec<u64>) {
    let mut rx = Vec::new();
    let mut tx = Vec::new();
    if let Some(s) = status {
        for (out, key) in [(&mut rx, "/series/rx_bps"), (&mut tx, "/series/tx_bps")] {
            if let Some(arr) = s.pointer(key).and_then(|v| v.as_array()) {
                for v in arr {
                    out.push(v.as_u64().unwrap_or(0));
                }
            }
        }
    }
    (rx, tx)
}

fn stat_card(ui: &mut Ui, label: &str, value: String, color: Color32) {
    card_frame(ui).show(ui, |ui| {
        ui.set_min_width(108.0);
        ui.label(RichText::new(label).size(9.5).strong().color(MUTED));
        ui.label(RichText::new(value).size(15.0).strong().color(color));
    });
}
