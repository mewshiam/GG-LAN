//! GG LAN native client UI — Material 3 inspired composition layer.
//!
//! This file orchestrates screens (header, sidebar, dashboard, dialogs);
//! the visual language lives in [`crate::theme`], the widgets in
//! [`crate::components`] and [`crate::icons`]. All networking/model logic is
//! untouched from the original client.

use crate::components::{self, BtnStyle, Snack, SnackKind};
use crate::icons::{self, Icon};
use crate::model::*;
use crate::theme::{
    self, fmt_bps, fmt_bytes, Palette, StatusKind, R_MD, R_XL, SP1, SP2, SP3, SP4, SP6,
};
use egui::{vec2, Align2, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
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
    join_server_error: Option<String>,
    join_code_error: Option<String>,

    pub create_open: bool,
    pub create_name: String,
    pub create_mode: &'static str,
    pub create_now: bool,

    pub settings_open: bool,
    settings_backup: Option<GuiConfig>,

    /// Set by dialog bodies to request closing (the `open` bool is borrowed
    /// by the window while the body runs).
    dlg_close: bool,
    remove_target: Option<usize>,

    pub snacks: Vec<Snack>,
    /// Cached "share this" LAN address per coordinator port.
    lan_hints: HashMap<u16, String>,
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
        let selected = (!nets.is_empty()).then_some(0);
        let mut app = Self {
            nets,
            cfg,
            cfg_path,
            cfg_dir,
            identity,
            bus: EventBus::default(),
            pings: HashMap::new(),
            selected,
            join_open: false,
            join_name: String::new(),
            join_server: String::new(),
            join_code: String::new(),
            join_now: true,
            join_server_error: None,
            join_code_error: None,
            create_open: false,
            create_name: String::new(),
            create_mode: "mesh",
            create_now: true,
            settings_open: false,
            settings_backup: None,
            dlg_close: false,
            remove_target: None,
            snacks: Vec::new(),
            lan_hints: HashMap::new(),
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
                    self.snack(format!("Coordinator failed: {e:#}"), SnackKind::Error);
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
            Err(e) => self.snack(format!("Agent failed: {e:#}"), SnackKind::Error),
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
        self.selected = if self.nets.is_empty() {
            None
        } else {
            Some(i.min(self.nets.len() - 1))
        };
        self.remove_target = None;
        self.save();
    }

    pub fn shutdown(&mut self) {
        for i in 0..self.nets.len() {
            self.stop_procs(i, true);
        }
    }

    fn snack(&mut self, text: impl Into<String>, kind: SnackKind) {
        self.snacks.push(Snack {
            text: text.into(),
            kind,
            at: Instant::now(),
        });
        if self.snacks.len() > 6 {
            self.snacks.remove(0);
        }
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
                self.snack(
                    format!("Agent for \u{201c}{name}\u{201d} stopped — see the event log"),
                    SnackKind::Error,
                );
            }
        }
    }

    // ------------------------------------------------------------ layout

    fn render(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.pump_events();
        ctx.request_repaint_after(Duration::from_millis(400));

        let pal = theme::ThemePrefs::parse(&self.cfg.theme).palette(&ctx);
        theme::apply_visuals(&ctx, pal);

        // Escape closes any open dialog, like a native modal.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.join_open = false;
            self.create_open = false;
            if self.settings_open {
                // dismissed without saving -> revert any in-dialog edits
                if let Some(b) = self.settings_backup.take() {
                    self.cfg = b;
                }
            }
            self.settings_open = false;
            self.remove_target = None;
            self.join_server_error = None;
            self.join_code_error = None;
        }

        egui::Panel::top("gg_header")
            .exact_size(52.0)
            .frame(egui::Frame::new().fill(pal.surface))
            .show(ui, |ui| self.header(ui, pal));

        egui::Panel::left("gg_nets")
            .exact_size(292.0)
            .frame(
                egui::Frame::new()
                    .fill(pal.surface_low)
                    .inner_margin(egui::Margin::same(12)),
            )
            .show(ui, |ui| self.sidebar(ui, pal));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(pal.surface)
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ui, |ui| self.central(ui, pal));

        self.dialogs(&ctx, pal);
        components::snackbars(&ctx, pal, &mut self.snacks);
    }

    fn header(&mut self, ui: &mut Ui, pal: &Palette) {
        ui.horizontal_centered(|ui| {
            ui.add_space(SP2);
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(9), pal.primary);
            icons::draw(p, Icon::Lan, rect.shrink(7.0), pal.on_primary);
            ui.add_space(SP2);
            ui.label(
                RichText::new("GG LAN")
                    .size(15.0)
                    .strong()
                    .color(pal.on_surface),
            );
            ui.label(
                theme::caption(format!("v{}", env!("CARGO_PKG_VERSION")))
                    .color(pal.on_surface_variant),
            );

            // understated connection summary
            let total = self.nets.len();
            if total > 0 {
                let connected = self
                    .nets
                    .iter_mut()
                    .map(|n| n.state())
                    .filter(|s| matches!(s, NetState::Connected))
                    .count();
                ui.add_space(SP4);
                let kind = if connected == total {
                    StatusKind::Connected
                } else if connected > 0 {
                    StatusKind::Waiting
                } else {
                    StatusKind::Offline
                };
                let sc = pal.status(kind);
                components::status_dot(ui, sc.fg, false);
                ui.label(
                    theme::caption(format!("{connected} of {total} connected"))
                        .color(pal.on_surface_variant),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(SP2);
                let sb = components::icon_button(ui, pal, Icon::Settings, 32.0, true);
                if sb.clicked() {
                    self.settings_backup = Some(self.cfg.clone());
                    self.settings_open = true;
                }
                sb.on_hover_text("Settings — nickname, theme, binaries");

                if !self.nets.is_empty() {
                    let any_on = self.nets.iter().any(|n| n.agent.is_some());
                    let (label, on) = if any_on {
                        ("Disconnect all", false)
                    } else {
                        ("Connect all", true)
                    };
                    if components::button(ui, pal, BtnStyle::Text, Some(Icon::Power), label, true)
                        .clicked()
                    {
                        if on {
                            for i in 0..self.nets.len() {
                                self.connect(i);
                            }
                        } else {
                            for i in 0..self.nets.len() {
                                self.disconnect(i);
                            }
                        }
                    }
                }
            });
        });
        let rect = ui.max_rect();
        ui.painter().hline(
            rect.left()..=rect.right(),
            rect.bottom() - 0.5,
            Stroke::new(1.0, pal.outline_variant),
        );
    }

    // ------------------------------------------------------------ sidebar

    fn sidebar(&mut self, ui: &mut Ui, pal: &Palette) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Networks")
                    .size(14.0)
                    .strong()
                    .color(pal.on_surface),
            );
            if !self.nets.is_empty() {
                ui.label(
                    theme::caption(format!("{}", self.nets.len())).color(pal.on_surface_variant),
                );
            }
        });
        ui.add_space(SP2);

        let mut open_join = false;
        let mut open_host = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = SP1;
            let w = ((ui.available_width() - SP1) / 2.0).floor();
            if components::button_exact(
                ui,
                pal,
                BtnStyle::Outlined,
                Some(Icon::Globe),
                "Join",
                vec2(w, 36.0),
            )
            .clicked()
            {
                open_join = true;
            }
            if components::button_exact(
                ui,
                pal,
                BtnStyle::Filled,
                Some(Icon::Server),
                "Host",
                vec2(w, 36.0),
            )
            .clicked()
            {
                open_host = true;
            }
        });
        if open_join {
            self.join_open = true;
            self.join_server_error = None;
            self.join_code_error = None;
        }
        if open_host {
            self.create_open = true;
        }
        ui.add_space(SP3);

        let mut toggle: Option<(usize, bool)> = None;
        let mut select: Option<usize> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.nets.is_empty() {
                    ui.add_space(SP4);
                    components::empty_state(
                        ui,
                        pal,
                        Icon::Globe,
                        "No networks yet",
                        "Join a friend's network with a code,\nor host your own LAN party.",
                    );
                }
                for i in 0..self.nets.len() {
                    self.net_row(ui, pal, i, &mut toggle, &mut select);
                }
            });

        if let Some(i) = select {
            self.selected = Some(i);
        }
        if let Some((i, on)) = toggle {
            // Toggling a network also shows its dashboard.
            self.selected = Some(i);
            if on {
                self.connect(i);
            } else {
                self.disconnect(i);
            }
        }
    }

    fn net_row(
        &mut self,
        ui: &mut Ui,
        pal: &Palette,
        i: usize,
        toggle: &mut Option<(usize, bool)>,
        select: &mut Option<usize>,
    ) {
        let selected = self.selected == Some(i);
        let st = self.nets[i].state();
        let name = self.nets[i].cfg.name.clone();
        let local = self.nets[i].cfg.local;
        let mode = self.nets[i].cfg.mode.clone();
        let peers_n = self.nets[i]
            .status
            .as_ref()
            .and_then(|s| s.get("peers"))
            .and_then(|p| p.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let alive = matches!(
            st,
            NetState::Starting | NetState::Waiting | NetState::Connected
        );
        let mut on = alive;

        let id = egui::Id::new("netrow").with(i);
        // hover from the previous frame's response (stable across frames)
        let hovered = ui
            .ctx()
            .read_response(id)
            .map(|r| r.hovered())
            .unwrap_or(false);
        let bg = if selected {
            if hovered {
                pal.tonal_hover(pal.secondary_container)
            } else {
                pal.secondary_container
            }
        } else if hovered {
            pal.surface_container
        } else {
            Color32::TRANSPARENT
        };
        let fg = if selected {
            pal.on_secondary_container
        } else {
            pal.on_surface
        };
        let meta_fg = if selected {
            pal.over_surface(pal.on_secondary_container, 0.25)
        } else {
            pal.on_surface_variant
        };

        let mut meta = if mode == "hub" { "hub" } else { "mesh" }.to_string();
        meta.push_str(&format!(
            " \u{00b7} {peers_n} peer{}",
            if peers_n == 1 { "" } else { "s" }
        ));
        if local {
            meta.push_str(" \u{00b7} host");
        }
        let pulse = matches!(st, NetState::Starting | NetState::Waiting);

        // The row-level click target must not overlap the switch: egui's
        // hit-test gives a click to the topmost (last registered) widget, and
        // the row interact is registered after the frame contents — an
        // overlapping rect would swallow switch clicks.
        let mut sw_rect = Rect::ZERO;
        let out = egui::Frame::new()
            .fill(bg)
            .corner_radius(CornerRadius::same(R_MD))
            .inner_margin(egui::Margin::symmetric(10, 7))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.set_min_height(40.0);
                    // switch reserved on the right, content fills the left
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let sw = components::switch(ui, pal, &mut on);
                        sw_rect = sw.rect;
                        if sw.changed() {
                            *toggle = Some((i, on));
                        }
                        sw.on_hover_text(if alive { "Disconnect" } else { "Connect" });

                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            components::status_dot(ui, pal.status(kind_of(st)).fg, pulse);
                            ui.add_space(4.0);
                            let avail = ui.available_width();
                            ui.vertical(|ui| {
                                ui.set_max_width((avail - 6.0).max(64.0));
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(name.as_str()).size(13.5).strong().color(fg),
                                    )
                                    .truncate(),
                                );
                                ui.label(RichText::new(meta.as_str()).size(11.0).color(meta_fg));
                            });
                        });
                    });
                });
            });
        let mut row_rect = out.response.rect;
        if sw_rect.max.x > row_rect.min.x && sw_rect.max.x <= row_rect.max.x {
            row_rect.max.x = sw_rect.min.x;
        }
        let resp = ui.interact(row_rect, id, Sense::click());
        if resp.clicked() {
            *select = Some(i);
        }
        ui.add_space(SP1);
    }

    // ------------------------------------------------------------ central

    fn central(&mut self, ui: &mut Ui, pal: &Palette) {
        let Some(i) = self.selected.filter(|i| *i < self.nets.len()) else {
            self.welcome(ui, pal);
            return;
        };
        let name = self.nets[i].cfg.name.clone();
        let server = self.nets[i].cfg.server.clone();
        let local = self.nets[i].cfg.local;
        let st = self.nets[i].state();
        let status = self.nets[i].status.clone();
        let log: Vec<String> = self.nets[i].log.iter().cloned().collect();
        let status_port = self.nets[i].agent.as_ref().map(|a| a.status_port);
        let agent_up = self.nets[i].agent.is_some();
        let kind = kind_of(st);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.dash_heading(ui, pal, i, &name, &server, local, kind, st, agent_up);
                ui.add_space(SP3);

                self.metrics_card(ui, pal, kind, st, &status);
                ui.add_space(SP3);

                if local {
                    self.share_card(ui, pal, i);
                    ui.add_space(SP3);
                }

                self.traffic_card(ui, pal, &status);
                ui.add_space(SP3);

                self.peers_card(ui, pal, i, &status, status_port);
                ui.add_space(SP3);

                self.log_section(ui, pal, &log);
                ui.add_space(SP1);
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn dash_heading(
        &mut self,
        ui: &mut Ui,
        pal: &Palette,
        i: usize,
        name: &str,
        server: &str,
        local: bool,
        kind: StatusKind,
        st: NetState,
        agent_up: bool,
    ) {
        let mut do_connect = false;
        let mut do_disconnect = false;
        let mut copy_addr = false;
        let mut copy_code = false;
        let mut ask_remove = false;

        ui.horizontal(|ui| {
            // right-aligned actions first (reserved from the right edge),
            // then the title group fills the remaining width, left-aligned.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // overflow / management actions
                let ob = components::icon_button(ui, pal, Icon::Overflow, 34.0, true)
                    .on_hover_text("More actions");
                egui::Popup::from_toggle_button_response(&ob)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .frame(
                        egui::Frame::new()
                            .fill(pal.surface_high)
                            .corner_radius(R_MD)
                            .inner_margin(egui::Margin::same(6)),
                    )
                    .show(|ui| {
                        if ui.button("Copy address").clicked() {
                            copy_addr = true;
                        }
                        if ui.button("Copy join code").clicked() {
                            copy_code = true;
                        }
                        ui.separator();
                        if ui
                            .button(RichText::new("Remove network").color(pal.error.fg))
                            .clicked()
                        {
                            ask_remove = true;
                        }
                    });

                if agent_up {
                    if components::button(
                        ui,
                        pal,
                        BtnStyle::Tonal,
                        Some(Icon::Power),
                        "Disconnect",
                        true,
                    )
                    .clicked()
                    {
                        do_disconnect = true;
                    }
                } else if components::button(
                    ui,
                    pal,
                    BtnStyle::Filled,
                    Some(Icon::Power),
                    "Connect",
                    true,
                )
                .clicked()
                {
                    do_connect = true;
                }

                // left side: title + status + host tag (truncates to fit)
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(theme::page_title(name.to_string()).color(pal.on_surface))
                            .truncate(),
                    );
                    ui.add_space(SP2);
                    components::status_pill(ui, pal, kind, st.label());
                    if local {
                        components::neutral_tag(ui, pal, "Host");
                    }
                });
            });
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(theme::caption("coordinator").color(pal.on_surface_variant));
            let addr = if server.is_empty() {
                "starts with the network"
            } else {
                server
            };
            ui.label(theme::mono_small(addr).color(pal.on_surface_variant));
        });

        if copy_addr && !server.is_empty() {
            ui.ctx().copy_text(server.to_string());
            self.snack("Address copied to clipboard", SnackKind::Ok);
        }
        if copy_code {
            let code = self.nets[i].cfg.code.clone();
            ui.ctx().copy_text(code);
            self.snack("Join code copied to clipboard", SnackKind::Ok);
        }
        if ask_remove {
            self.remove_target = Some(i);
        }
        if do_connect {
            self.connect(i);
        }
        if do_disconnect {
            self.disconnect(i);
        }
    }

    fn metrics_card(
        &mut self,
        ui: &mut Ui,
        pal: &Palette,
        kind: StatusKind,
        st: NetState,
        status: &Option<serde_json::Value>,
    ) {
        let (my_ip, peers, rx_now, tx_now, rx_total, tx_total) = stat_snapshot(status);
        let has = status.is_some();
        let mut copied_addr = false;

        components::card(pal).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(SP6, SP3);

                // status
                ui.vertical(|ui| {
                    ui.label(theme::caption("Status").color(pal.on_surface_variant));
                    components::status_pill(ui, pal, kind, st.label());
                });

                // virtual address
                ui.vertical(|ui| {
                    ui.label(theme::caption("Your address").color(pal.on_surface_variant));
                    ui.horizontal(|ui| {
                        ui.label(
                            theme::mono(if has { my_ip.clone() } else { "—".into() })
                                .strong()
                                .color(pal.on_surface),
                        );
                        if has
                            && !my_ip.is_empty()
                            && my_ip != "—"
                            && components::copy_button(ui, pal, &my_ip)
                        {
                            copied_addr = true;
                        }
                    });
                });

                // peers
                ui.vertical(|ui| {
                    ui.label(theme::caption("Peers").color(pal.on_surface_variant));
                    ui.label(
                        theme::mono(if has {
                            peers.len().to_string()
                        } else {
                            "—".into()
                        })
                        .strong()
                        .color(if peers.is_empty() {
                            pal.on_surface_variant
                        } else {
                            pal.on_surface
                        }),
                    );
                });

                // down
                ui.vertical(|ui| {
                    ui.label(theme::caption("Down").color(pal.on_surface_variant));
                    ui.horizontal(|ui| {
                        icons::show(ui, Icon::Download, 15.0, pal.series_down);
                        ui.label(
                            theme::mono(if has { fmt_bps(rx_now) } else { "—".into() })
                                .strong()
                                .color(pal.series_down),
                        );
                    });
                });

                // up
                ui.vertical(|ui| {
                    ui.label(theme::caption("Up").color(pal.on_surface_variant));
                    ui.horizontal(|ui| {
                        icons::show(ui, Icon::Upload, 15.0, pal.series_up);
                        ui.label(
                            theme::mono(if has { fmt_bps(tx_now) } else { "—".into() })
                                .strong()
                                .color(pal.series_up),
                        );
                    });
                });
            });
            ui.add_space(SP1);
            ui.label(
                theme::caption(if has {
                    format!(
                        "Session traffic · {} down · {} up",
                        fmt_bytes(rx_total),
                        fmt_bytes(tx_total)
                    )
                } else {
                    "Session traffic · no data yet".into()
                })
                .color(pal.on_surface_variant),
            );
        });

        if copied_addr {
            self.snack("Address copied to clipboard", SnackKind::Ok);
        }
    }

    fn share_card(&mut self, ui: &mut Ui, pal: &Palette, i: usize) {
        let server = self.nets[i].cfg.server.clone();
        let code = self.nets[i].cfg.code.clone();
        let port = server
            .rsplit(':')
            .next()
            .and_then(|p| p.parse::<u16>().ok());
        let lan = port.and_then(|p| self.lan_hint(p));
        let addr = lan.clone().unwrap_or_else(|| server.clone());
        let mut copied_a = false;
        let mut copied_c = false;

        components::card(pal).show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::show(ui, Icon::Server, 16.0, pal.primary);
                ui.label(theme::section("Invite friends").color(pal.on_surface));
            });
            ui.add_space(2.0);
            ui.label(
                theme::label(
                    "Share this address and join code — friends join from their GG LAN client.",
                )
                .color(pal.on_surface_variant),
            );
            ui.add_space(SP2);
            if addr.is_empty() {
                ui.label(
                    theme::caption("The address appears once the network starts.")
                        .color(pal.on_surface_variant),
                );
            } else {
                share_row(ui, pal, "Address", &addr, &mut copied_a);
                share_row(ui, pal, "Join code", &code, &mut copied_c);
            }
        });

        if copied_a {
            self.snack("Address copied to clipboard", SnackKind::Ok);
        }
        if copied_c {
            self.snack("Join code copied to clipboard", SnackKind::Ok);
        }
    }

    fn traffic_card(&mut self, ui: &mut Ui, pal: &Palette, status: &Option<serde_json::Value>) {
        let (rx_series, tx_series) = series_snapshot(status);
        let has = status.is_some();
        let rx_now = *rx_series.last().unwrap_or(&0);
        let tx_now = *tx_series.last().unwrap_or(&0);

        components::card(pal).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(theme::section("Traffic").color(pal.on_surface));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(theme::caption("last 60 s").color(pal.on_surface_variant));
                    legend_chip(ui, pal, "Up", fmt_bps(tx_now), pal.series_up, has);
                    legend_chip(ui, pal, "Down", fmt_bps(rx_now), pal.series_down, has);
                });
            });
            ui.add_space(SP2);
            traffic_chart(ui, pal, &rx_series, &tx_series);
        });
    }

    fn peers_card(
        &mut self,
        ui: &mut Ui,
        pal: &Palette,
        i: usize,
        status: &Option<serde_json::Value>,
        status_port: Option<u16>,
    ) {
        let peers = stat_snapshot(status).1;
        let mut ping: Option<String> = None;
        let mut copied_ip: Option<String> = None;
        let show_traffic = ui.available_width() >= 620.0;

        components::card(pal).show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::show(ui, Icon::Peers, 16.0, pal.on_surface_variant);
                ui.label(theme::section("Peers").color(pal.on_surface));
                ui.label(
                    theme::caption(format!("{} online", peers.len())).color(pal.on_surface_variant),
                );
            });
            ui.add_space(SP2);

            if peers.is_empty() {
                components::empty_state(
                    ui,
                    pal,
                    Icon::Peers,
                    "No peers connected",
                    "Share the invite so friends can join this LAN.",
                );
                return;
            }

            let widths = PeerWidths::for_width(ui.available_width(), show_traffic);
            peer_header(ui, pal, &widths);
            for p in &peers {
                let ping_res = self.pings.get(&(i, p.ip.clone())).cloned();
                peer_row(
                    ui,
                    pal,
                    p,
                    ping_res,
                    status_port,
                    &widths,
                    &mut ping,
                    &mut copied_ip,
                );
            }
        });

        if let Some(ip) = ping {
            self.pings.insert((i, ip.clone()), "…".into());
            if let Some(port) = status_port {
                fire_ping(i, port, ip, self.bus.tx.clone());
            }
        }
        if let Some(ip) = copied_ip {
            ui.ctx().copy_text(ip);
            self.snack("Peer address copied", SnackKind::Ok);
        }
    }

    fn log_section(&mut self, ui: &mut Ui, pal: &Palette, log: &[String]) {
        ui.horizontal(|ui| {
            icons::show(ui, Icon::Terminal, 15.0, pal.on_surface_variant);
            egui::CollapsingHeader::new(
                RichText::new("Event log")
                    .size(14.0)
                    .strong()
                    .color(pal.on_surface),
            )
            .default_open(false)
            .show(ui, |ui| {
                ui.add_space(SP1);
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for l in log.iter().rev() {
                            ui.label(theme::mono_small(l.clone()).color(pal.on_surface_variant));
                        }
                    });
            });
        });
    }

    fn welcome(&mut self, ui: &mut Ui, pal: &Palette) {
        ui.vertical_centered(|ui| {
            ui.add_space(64.0);
            let (rect, _) = ui.allocate_exact_size(vec2(76.0, 76.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(rect, CornerRadius::same(22), pal.primary_container);
            icons::draw(p, Icon::Lan, rect.shrink(20.0), pal.on_primary_container);
            ui.add_space(SP4);
            ui.label(theme::page_title("GG LAN").color(pal.on_surface));
            ui.add_space(SP1);
            ui.label(
                theme::body("A virtual LAN for your games — anywhere.")
                    .color(pal.on_surface_variant),
            );
            ui.label(
                theme::caption(
                    "Encrypted peer-to-peer overlay with NAT traversal, for LAN parties over the internet.",
                )
                .color(pal.on_surface_variant),
            );
            ui.add_space(SP4);
            ui.horizontal(|ui| {
                if components::button(ui, pal, BtnStyle::Filled, Some(Icon::Server), "Host a network", true)
                    .clicked()
                {
                    self.create_open = true;
                }
                ui.add_space(SP2);
                if components::button(ui, pal, BtnStyle::Outlined, Some(Icon::Globe), "Join a network", true)
                    .clicked()
                {
                    self.join_open = true;
                }
            });
            ui.add_space(SP6);
            ui.label(
                theme::caption(format!(
                    "Nickname \u{201c}{}\u{201d} · GG LAN v{}",
                    self.cfg.nickname,
                    env!("CARGO_PKG_VERSION")
                ))
                .color(pal.outline),
            );
        });
    }

    // ------------------------------------------------------------ dialogs

    fn dialogs(&mut self, ctx: &egui::Context, pal: &Palette) {
        // ---- join
        let mut join_open = self.join_open;
        if join_open {
            let closed = modal_frame(ctx, pal, "Join a network", 450.0, |ui, pal| {
                self.join_body(ui, pal)
            });
            if closed {
                self.dlg_close = true;
            }
        }
        if self.dlg_close {
            self.dlg_close = false;
            join_open = false;
        }
        self.join_open = join_open;

        // ---- create / host
        let mut create_open = self.create_open;
        if create_open {
            let closed = modal_frame(ctx, pal, "Host a network", 460.0, |ui, pal| {
                self.host_body(ui, pal)
            });
            if closed {
                self.dlg_close = true;
            }
        }
        if self.dlg_close {
            self.dlg_close = false;
            create_open = false;
        }
        self.create_open = create_open;

        // ---- settings
        let mut settings_open = self.settings_open;
        if settings_open {
            let closed = modal_frame(ctx, pal, "Settings", 540.0, |ui, pal| {
                self.settings_body(ui, pal)
            });
            if closed {
                self.dlg_close = true;
            }
        }
        if self.dlg_close {
            self.dlg_close = false;
            settings_open = false;
            // dismissed without saving -> revert any in-dialog edits
            if let Some(b) = self.settings_backup.take() {
                self.cfg = b;
            }
        }
        self.settings_open = settings_open;

        // ---- remove confirmation
        if let Some(i) = self.remove_target {
            let name = self
                .nets
                .get(i)
                .map(|n| n.cfg.name.clone())
                .unwrap_or_default();
            let mut do_remove = false;
            let closed = modal_frame(ctx, pal, "Remove network?", 440.0, |ui, pal| {
                ui.label(
                    theme::label(format!(
                        "This disconnects and forgets \u{201c}{name}\u{201d}. You will need its address and join code to join it again."
                    ))
                    .color(pal.on_surface_variant),
                );
                ui.add_space(SP4);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if components::destructive(ui, pal, "Remove network").clicked() {
                        do_remove = true;
                    }
                    if components::button(ui, pal, BtnStyle::Text, None, "Cancel", true).clicked() {
                        self.dlg_close = true;
                    }
                });
            });
            if closed {
                self.dlg_close = true;
            }
            if self.dlg_close {
                self.dlg_close = false;
                self.remove_target = None;
            }
            if do_remove {
                self.remove_net(i);
                self.snack("Network removed", SnackKind::Ok);
            }
        }

        // Enter submits the join form while it is open.
        if self.join_open {
            let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
            if enter {
                self.join_submit();
            }
        }
    }

    fn join_body(&mut self, ui: &mut Ui, pal: &Palette) {
        ui.label(
            theme::label("Ask the host for their address and join code.")
                .color(pal.on_surface_variant),
        );
        ui.add_space(SP3);

        // clear field errors as the user types
        if !self.join_server.trim().is_empty() {
            self.join_server_error = None;
        }
        if !self.join_code.trim().is_empty() {
            self.join_code_error = None;
        }

        components::field(
            ui,
            pal,
            "Network name",
            &mut self.join_name,
            "e.g. Friday Night Games",
            None,
        );
        ui.add_space(SP2);
        components::field(
            ui,
            pal,
            "Server address",
            &mut self.join_server,
            "host or ip:port — default port :7100",
            self.join_server_error.as_deref(),
        );
        ui.add_space(SP2);
        components::field(
            ui,
            pal,
            "Join code",
            &mut self.join_code,
            "the code the host shared with you",
            self.join_code_error.as_deref(),
        );
        ui.add_space(SP3);
        ui.add(egui::Checkbox::new(
            &mut self.join_now,
            RichText::new("Connect immediately").color(pal.on_surface_variant),
        ));
        ui.add_space(SP4);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if components::button(ui, pal, BtnStyle::Filled, None, "Join network", true).clicked() {
                self.join_submit();
            }
            if components::button(ui, pal, BtnStyle::Text, None, "Cancel", true).clicked() {
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
        // inline validation — keep the dialog open until the form is valid
        self.join_server_error = if server.is_empty() {
            Some("Enter the host's address, e.g. 203.0.113.7:7100".into())
        } else {
            None
        };
        self.join_code_error = if code.is_empty() {
            Some("Enter the join code the host shared with you".into())
        } else {
            None
        };
        if self.join_server_error.is_some() || self.join_code_error.is_some() {
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
        let joined_name = self.nets[idx].cfg.name.clone();
        self.join_name.clear();
        self.join_server.clear();
        self.join_code.clear();
        if self.join_now {
            self.connect(idx);
            self.snack(
                format!("Joining \u{201c}{joined_name}\u{201d}…"),
                SnackKind::Ok,
            );
        } else {
            self.snack(
                format!("Network \u{201c}{joined_name}\u{201d} added"),
                SnackKind::Ok,
            );
        }
    }

    fn host_body(&mut self, ui: &mut Ui, pal: &Palette) {
        ui.label(
            theme::label("Run a coordinator on this machine and invite your friends.")
                .color(pal.on_surface_variant),
        );
        ui.add_space(SP3);
        components::field(
            ui,
            pal,
            "Network name",
            &mut self.create_name,
            "e.g. CS2 LAN Party",
            None,
        );
        ui.add_space(SP3);
        ui.label(theme::label("Mode").color(pal.on_surface_variant));
        let mode_ptr: &'static str = self.create_mode;
        let mut mode = mode_ptr;
        components::segmented(
            ui,
            pal,
            "host_mode",
            &[(&"mesh", "Mesh"), (&"hub", "Hub")],
            &mut mode,
        );
        self.create_mode = mode;
        ui.add_space(SP2);
        ui.label(
            theme::caption(if mode == "hub" {
                "All traffic is relayed through this machine — works when direct connections are blocked. Traffic stays end-to-end encrypted."
            } else {
                "Peers connect directly to each other when possible, with encrypted relay fallback. Best performance for games."
            })
            .color(pal.on_surface_variant),
        );
        ui.add_space(SP3);
        ui.add(egui::Checkbox::new(
            &mut self.create_now,
            RichText::new("Start hosting now").color(pal.on_surface_variant),
        ));
        ui.add_space(SP4);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if components::button(
                ui,
                pal,
                BtnStyle::Filled,
                Some(Icon::Server),
                "Create network",
                true,
            )
            .clicked()
            {
                self.host_submit();
            }
            if components::button(ui, pal, BtnStyle::Text, None, "Cancel", true).clicked() {
                self.dlg_close = true;
            }
        });
    }

    fn host_submit(&mut self) {
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
            match port.and_then(|p| self.lan_hint(p)) {
                Some(addr) => self.snack(
                    format!(
                        "Hosting \u{201c}{name}\u{201d} — friends join {addr} with code {code}"
                    ),
                    SnackKind::Ok,
                ),
                None => self.snack(
                    format!("Hosting \u{201c}{name}\u{201d} — code {code}"),
                    SnackKind::Ok,
                ),
            }
        } else {
            self.snack(
                format!("Network \u{201c}{name}\u{201d} created (not started)"),
                SnackKind::Ok,
            );
        }
    }

    fn settings_body(&mut self, ui: &mut Ui, pal: &Palette) {
        // ---- appearance
        ui.label(theme::label("Appearance").strong().color(pal.on_surface));
        ui.add_space(SP1);
        let mut pref = theme::ThemePrefs::parse(&self.cfg.theme);
        components::segmented(
            ui,
            pal,
            "theme_pref",
            &[
                (&theme::ThemePrefs::System, "System"),
                (&theme::ThemePrefs::Light, "Light"),
                (&theme::ThemePrefs::Dark, "Dark"),
            ],
            &mut pref,
        );
        self.cfg.theme = pref.as_str().into();
        ui.add_space(2.0);
        ui.label(
            theme::caption("System follows your operating system's dark mode.")
                .color(pal.on_surface_variant),
        );
        ui.add_space(SP3);
        components::hairline(ui, pal);
        ui.add_space(SP3);

        // ---- identity
        ui.label(theme::label("Identity").strong().color(pal.on_surface));
        ui.add_space(SP1);
        components::field(
            ui,
            pal,
            "Nickname",
            &mut self.cfg.nickname,
            "shown to other players",
            None,
        );
        ui.add_space(SP3);
        components::hairline(ui, pal);
        ui.add_space(SP3);

        // ---- runtime
        ui.label(theme::label("Runtime").strong().color(pal.on_surface));
        ui.add_space(SP1);
        components::field(
            ui,
            pal,
            "Agent binary",
            &mut self.cfg.agent_path,
            "blank = auto-detect (next to GG LAN)",
            None,
        );
        ui.add_space(SP2);
        components::field(
            ui,
            pal,
            "Coordinator binary",
            &mut self.cfg.server_path,
            "blank = auto-detect (next to GG LAN)",
            None,
        );
        ui.add_space(SP3);
        components::hairline(ui, pal);
        ui.add_space(SP3);

        // ---- configuration & diagnostics (read-only)
        ui.label(theme::label("Configuration").strong().color(pal.on_surface));
        ui.add_space(SP1);
        let mut copied = false;
        path_row(ui, pal, "Config file", &self.cfg_path, &mut copied);
        path_row(ui, pal, "Identity file", &self.identity, &mut copied);
        ui.add_space(SP1);
        ui.label(
            theme::caption(format!(
                "GG LAN v{} · your node identity never leaves this machine",
                env!("CARGO_PKG_VERSION")
            ))
            .color(pal.on_surface_variant),
        );
        ui.add_space(SP4);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if components::button(ui, pal, BtnStyle::Filled, None, "Save", true).clicked() {
                self.save();
                self.dlg_close = true;
            }
            if components::button(ui, pal, BtnStyle::Text, None, "Cancel", true).clicked() {
                if let Some(b) = self.settings_backup.take() {
                    self.cfg = b;
                }
                self.dlg_close = true;
            }
        });
        if copied {
            self.snack("Path copied to clipboard", SnackKind::Ok);
        }
    }

    // ------------------------------------------------------------ helpers

    /// Compute (and cache) this machine's LAN address for a coordinator port.
    fn lan_hint(&mut self, port: u16) -> Option<String> {
        if let Some(h) = self.lan_hints.get(&port) {
            return Some(h.clone());
        }
        let hint = lan_address(port)?;
        self.lan_hints.insert(port, hint.clone());
        Some(hint)
    }
}

impl Drop for GgApp {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------- free helpers

fn modal_frame(
    ctx: &egui::Context,
    pal: &Palette,
    title: &str,
    width: f32,
    body: impl FnOnce(&mut Ui, &Palette),
) -> bool {
    let mut close = false;
    egui::Window::new(title)
        .title_bar(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::new()
                .fill(pal.dialog)
                .corner_radius(R_XL)
                .inner_margin(egui::Margin::same(24)),
        )
        .show(ctx, |ui| {
            ui.set_min_width(width);
            ui.horizontal(|ui| {
                ui.label(theme::section(title).color(pal.on_surface));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let cb = components::icon_button(ui, pal, Icon::Close, 28.0, true);
                    if cb.clicked() {
                        close = true;
                    }
                    cb.on_hover_text("Close");
                });
            });
            ui.add_space(SP3);
            body(ui, pal);
        });
    close
}

fn kind_of(st: NetState) -> StatusKind {
    match st {
        NetState::Connected => StatusKind::Connected,
        NetState::Starting => StatusKind::Connecting,
        NetState::Waiting => StatusKind::Waiting,
        NetState::Off => StatusKind::Offline,
        NetState::Dead => StatusKind::Error,
    }
}

fn share_row(ui: &mut Ui, pal: &Palette, label: &str, value: &str, copied: &mut bool) {
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(theme::label(label).color(pal.on_surface_variant)).truncate());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if components::copy_button(ui, pal, value) {
                *copied = true;
            }
            ui.add_space(SP1);
            ui.label(theme::mono(value.to_string()).color(pal.on_surface));
        });
    });
}

fn path_row(ui: &mut Ui, pal: &Palette, label: &str, path: &std::path::Path, copied: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(theme::caption(label).color(pal.on_surface_variant));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if components::copy_button(ui, pal, &path.display().to_string()) {
                *copied = true;
            }
            ui.add_space(SP1);
            ui.add(
                egui::Label::new(
                    theme::mono_small(path.display().to_string()).color(pal.on_surface_variant),
                )
                .truncate(),
            );
        });
    });
}

fn legend_chip(ui: &mut Ui, pal: &Palette, label: &str, value: String, color: Color32, has: bool) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(9.0, 9.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 3.5, color);
        ui.label(
            theme::caption(format!("{label} {}", if has { value } else { "—".into() }))
                .color(pal.on_surface_variant),
        );
    });
}

struct PeerWidths {
    name: f32,
    ip: f32,
    path: f32,
    rtt: f32,
    traffic: f32,
    ping: f32,
    show_traffic: bool,
}

impl PeerWidths {
    /// Column layout adapts to the available width: the secondary traffic
    /// column drops first, then columns compact — essential peer info
    /// (name, address, path, rtt, ping) always stays reachable.
    fn for_width(avail: f32, show_traffic: bool) -> Self {
        let wide = avail >= 520.0;
        let ip = if wide { 104.0 } else { 88.0 };
        let path = 66.0;
        let rtt = 52.0;
        let traffic = if show_traffic { 88.0 } else { 0.0 };
        let ping = if wide { 168.0 } else { 112.0 };
        let used = ip + path + rtt + traffic + ping + 5.0 * SP3;
        let name = (avail - used).max(72.0);
        Self {
            name,
            ip,
            path,
            rtt,
            traffic,
            ping,
            show_traffic,
        }
    }
}

fn peer_header(ui: &mut Ui, pal: &Palette, w: &PeerWidths) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = SP3;
        let c = pal.on_surface_variant;
        ui.add_sized(
            [w.name, 16.0],
            egui::Label::new(theme::caption("Name").color(c)),
        );
        ui.add_sized(
            [w.ip, 16.0],
            egui::Label::new(theme::caption("Address").color(c)),
        );
        ui.add_sized(
            [w.path, 16.0],
            egui::Label::new(theme::caption("Path").color(c)),
        );
        ui.add_sized(
            [w.rtt, 16.0],
            egui::Label::new(theme::caption("RTT").color(c)).halign(egui::Align::Max),
        );
        if w.show_traffic {
            ui.add_sized(
                [w.traffic, 16.0],
                egui::Label::new(theme::caption("Traffic").color(c)).halign(egui::Align::Max),
            );
        }
        ui.add_sized(
            [w.ping, 16.0],
            egui::Label::new(theme::caption("").color(c)),
        );
    });
    ui.add_space(SP1);
    components::hairline(ui, pal);
}

#[allow(clippy::too_many_arguments)]
fn peer_row(
    ui: &mut Ui,
    pal: &Palette,
    p: &PeerView,
    ping_res: Option<String>,
    status_port: Option<u16>,
    w: &PeerWidths,
    ping: &mut Option<String>,
    copied_ip: &mut Option<String>,
) {
    let row_h = 36.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = SP3;
        // name
        ui.add_sized(
            [w.name, row_h],
            egui::Label::new(
                RichText::new(p.name.clone())
                    .size(13.0)
                    .strong()
                    .color(pal.on_surface),
            )
            .truncate(),
        );
        // address (click to copy)
        let ip_label =
            egui::Label::new(theme::mono_small(p.ip.clone()).color(pal.on_surface_variant))
                .sense(Sense::click());
        let ipr = ui.add_sized([w.ip, row_h], ip_label);
        if ipr.clicked() {
            *copied_ip = Some(p.ip.clone());
        }
        ipr.on_hover_cursor(egui::CursorIcon::Copy)
            .on_hover_text("Click to copy");
        // path
        let (pc, tag_label) = match p.path.as_str() {
            "direct" => (pal.success, "direct"),
            "relay" => (pal.warn, "relay"),
            _ => (pal.off, p.path.as_str()),
        };
        components::tag(ui, &pc, tag_label);
        // rtt
        ui.add_sized(
            [w.rtt, row_h],
            egui::Label::new(
                theme::mono_small(
                    p.rtt
                        .map(|r| format!("{r} ms"))
                        .unwrap_or_else(|| "—".into()),
                )
                .color(if p.rtt.is_some() {
                    pal.on_surface
                } else {
                    pal.outline
                }),
            )
            .halign(egui::Align::Max),
        );
        // traffic
        if w.show_traffic {
            ui.add_sized(
                [w.traffic, row_h],
                egui::Label::new(
                    theme::mono_small(fmt_bytes(p.bytes_in + p.bytes_out))
                        .color(pal.on_surface_variant),
                )
                .halign(egui::Align::Max),
            );
        }
        // ping
        ui.add_sized([w.ping, row_h], |ui: &mut Ui| {
            ui.horizontal(|ui| {
                match &ping_res {
                    Some(res) if res == "…" => {
                        ui.add(egui::Spinner::new().size(13.0));
                    }
                    Some(res) => {
                        let color = if res.starts_with("failed") || res.starts_with("error") {
                            pal.error.fg
                        } else if res.ends_with(" ms") {
                            pal.success.fg
                        } else {
                            pal.on_surface_variant
                        };
                        ui.add(
                            egui::Label::new(theme::mono_small(res.clone()).color(color))
                                .truncate(),
                        );
                    }
                    None => {}
                }
                if status_port.is_some() {
                    if components::button_small(ui, pal, BtnStyle::Outlined, None, "Ping", true)
                        .clicked()
                    {
                        *ping = Some(p.ip.clone());
                    }
                } else {
                    ui.label(theme::caption("offline").color(pal.outline));
                }
            });
            ui.response()
        });
    });
    components::hairline(ui, pal);
}

// ---------------------------------------------------------------- traffic chart

fn traffic_chart(ui: &mut Ui, pal: &Palette, rx: &[u64], tx: &[u64]) {
    let h = 138.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter();
    let plot = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top()),
        Pos2::new(rect.right() - 56.0, rect.bottom() - 15.0),
    );
    if plot.width() < 60.0 || plot.height() < 40.0 {
        return;
    }

    let empty = rx.is_empty() && tx.is_empty();
    let max = rx
        .iter()
        .chain(tx.iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32
        * 1.25;

    // subtle horizontal grid + right-side axis labels
    for i in 0..=3 {
        let y = plot.bottom() - plot.height() * i as f32 / 3.0;
        p.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(1.0, pal.outline_variant),
        );
        let label = if i == 0 {
            "0".to_string()
        } else {
            fmt_bps((max * i as f32 / 3.0) as u64)
        };
        let g = p.layout_job(egui::text::LayoutJob::simple(
            label,
            egui::FontId::monospace(10.0),
            pal.on_surface_variant,
            f32::INFINITY,
        ));
        p.galley(
            Pos2::new(rect.right() - g.size().x, y - g.size().y / 2.0),
            g,
            pal.on_surface_variant,
        );
    }
    // x labels
    for (x, t) in [(plot.left(), "-60 s"), (plot.right(), "now")] {
        let g = p.layout_job(egui::text::LayoutJob::simple(
            t.to_string(),
            egui::FontId::monospace(10.0),
            pal.on_surface_variant,
            f32::INFINITY,
        ));
        p.galley(
            Pos2::new(
                (x - g.size().x / 2.0).clamp(plot.left(), plot.right() - g.size().x),
                plot.bottom() + 4.0,
            ),
            g,
            pal.on_surface_variant,
        );
    }

    if empty {
        let g = p.layout_job(egui::text::LayoutJob::simple(
            "Waiting for traffic data…".into(),
            egui::FontId::proportional(12.5),
            pal.on_surface_variant,
            f32::INFINITY,
        ));
        p.galley(
            Pos2::new(
                plot.center().x - g.size().x / 2.0,
                plot.center().y - g.size().y / 2.0,
            ),
            g,
            pal.on_surface_variant,
        );
        return;
    }

    for (series, color) in [(rx, pal.series_down), (tx, pal.series_up)] {
        let n = series.len();
        if n == 0 {
            continue;
        }
        let step = plot.width() / 59.0;
        let pts: Vec<Pos2> = series
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let x = plot.right() - (n - 1 - i) as f32 * step;
                let y = plot.bottom() - (*v as f32 / max) * plot.height();
                Pos2::new(x, y)
            })
            .collect();
        // area fill (convex enough: series + baseline)
        let mut area = pts.clone();
        area.push(Pos2::new(plot.right(), plot.bottom()));
        area.push(Pos2::new(pts[0].x, plot.bottom()));
        p.add(egui::Shape::Path(egui::epaint::PathShape {
            points: area,
            closed: true,
            fill: pal.area_fill(color),
            stroke: Stroke::NONE.into(),
        }));
        p.add(egui::Shape::line(pts, Stroke::new(1.8, color)));
    }
}

// ---------------------------------------------------------------- snapshots

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
