//! Reusable Material-3-style components.
//!
//! Everything the screens draw goes through these helpers (plus
//! [`crate::theme`] tokens and [`crate::icons`]) so that hover/focus/pressed/
//! disabled states, corner radii, spacing and color usage stay consistent
//! app-wide.

use crate::icons::{self, Icon};
use crate::theme::{self, Palette, StatusColor, StatusKind, R_MD, R_SM, SP1, SP2, SP3};
use egui::{
    vec2, Align2, Color32, CornerRadius, FontId, Id, Pos2, Response, Sense, Stroke, Ui, Vec2,
};

// ---------------------------------------------------------------- helpers

/// Shift `c` towards white (`w > 0`) or black (`w < 0`).
fn shade(c: Color32, w: f32) -> Color32 {
    let f = |x: u8| -> u8 {
        let t = if w > 0.0 { 255.0 } else { 0.0 };
        (x as f32 * (1.0 - w.abs()) + t * w.abs()).round() as u8
    };
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

fn transparent() -> Color32 {
    Color32::from_rgba_unmultiplied(0, 0, 0, 0)
}

fn fade(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a * 255.0) as u8)
}

// ---------------------------------------------------------------- buttons

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtnStyle {
    /// Filled with the primary color — the one main action.
    Filled,
    /// Tonal (secondary container) — prominent secondary action.
    Tonal,
    /// Outlined — secondary action.
    Outlined,
    /// Quiet text button — tertiary / cancel actions.
    Text,
    /// Filled with the error color — destructive actions.
    Destructive,
}

/// Material-style button with an optional leading icon and full
/// default/hover/pressed/focused/disabled states. Height 36, corner R_MD.
pub fn button(
    ui: &mut Ui,
    pal: &Palette,
    style: BtnStyle,
    icon: Option<Icon>,
    text: &str,
    enabled: bool,
) -> Response {
    button_sized(
        ui,
        pal,
        style,
        icon,
        text,
        enabled,
        vec2(76.0, 36.0),
        16.0,
        12.5,
        R_MD,
    )
}

/// Compact variant for in-table actions (Ping, copy…).
pub fn button_small(
    ui: &mut Ui,
    pal: &Palette,
    style: BtnStyle,
    icon: Option<Icon>,
    text: &str,
    enabled: bool,
) -> Response {
    button_sized(
        ui,
        pal,
        style,
        icon,
        text,
        enabled,
        vec2(40.0, 26.0),
        10.0,
        11.5,
        R_SM,
    )
}

#[allow(clippy::too_many_arguments)]
fn button_sized(
    ui: &mut Ui,
    pal: &Palette,
    style: BtnStyle,
    icon: Option<Icon>,
    text: &str,
    enabled: bool,
    min_size: Vec2,
    pad: f32,
    font_size: f32,
    radius: u8,
) -> Response {
    let font = FontId::proportional(font_size);
    let galley = ui.painter().layout_job(egui::text::LayoutJob::simple(
        text.to_string(),
        font,
        Color32::WHITE,
        f32::INFINITY,
    ));
    let icon_w = if icon.is_some() { font_size + 6.0 } else { 0.0 };
    let w = (galley.size().x + icon_w + pad * 2.0).max(min_size.x);
    let h = min_size.y;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover() // disabled buttons never register clicks
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), sense);
    let p = ui.painter();

    let (base, on) = match style {
        BtnStyle::Filled => (pal.primary, pal.on_primary),
        BtnStyle::Tonal => (pal.secondary_container, pal.on_secondary_container),
        BtnStyle::Outlined => (transparent(), pal.on_surface),
        BtnStyle::Text => (transparent(), pal.primary),
        BtnStyle::Destructive => (pal.error.fg, pal.on_error),
    };

    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();
    let active = enabled && (hovered || pressed);

    // background
    let dir = if pal.dark { -1.0 } else { 1.0 };
    let bg = if pressed {
        shade(base, -0.10 * dir)
    } else if hovered {
        shade(base, 0.08 * dir)
    } else {
        base
    };
    let bg = match style {
        BtnStyle::Outlined | BtnStyle::Text if !active => transparent(),
        BtnStyle::Outlined | BtnStyle::Text if active => pal.surface_container,
        _ => bg,
    };
    let alpha = if enabled { 1.0 } else { 0.45 };
    if bg != transparent() {
        p.rect_filled(rect, CornerRadius::same(radius), fade(bg, alpha));
    }
    if matches!(style, BtnStyle::Outlined) {
        p.rect_stroke(
            rect.shrink(0.5),
            CornerRadius::same(radius),
            Stroke::new(1.2, fade(pal.outline, alpha)),
            egui::StrokeKind::Middle,
        );
    }
    // keyboard focus ring (visible focus, outside the fill)
    if enabled && resp.has_focus() {
        p.rect_stroke(
            rect.expand(2.0),
            CornerRadius::same(radius + 2),
            Stroke::new(2.0, pal.primary),
            egui::StrokeKind::Middle,
        );
    }

    // contents: icon + label, centered as a group
    let fg = fade(on, alpha);
    let mut x = rect.center().x - (galley.size().x + icon_w) / 2.0;
    if let Some(ic) = icon {
        let s = font_size + 4.5;
        icons::draw(
            p,
            ic,
            egui::Rect::from_center_size(Pos2::new(x + s / 2.0 - 1.0, rect.center().y), vec2(s, s)),
            fg,
        );
        x += icon_w;
    }
    p.galley_with_override_text_color(
        Pos2::new(x, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
    resp
}

/// Filled destructive button — removal and other irreversible actions.
pub fn destructive(ui: &mut Ui, pal: &Palette, text: &str) -> Response {
    button_sized(
        ui,
        pal,
        BtnStyle::Destructive,
        None,
        text,
        true,
        vec2(96.0, 36.0),
        16.0,
        12.5,
        R_MD,
    )
}

/// Fixed-size button (for equal-width side-by-side actions).
pub fn button_exact(
    ui: &mut Ui,
    pal: &Palette,
    style: BtnStyle,
    icon: Option<Icon>,
    text: &str,
    size: Vec2,
) -> Response {
    button_sized(ui, pal, style, icon, text, true, size, 10.0, 12.5, R_MD)
}

/// Icon-only button with hover feedback. Always pair with a tooltip at the
/// call site.
pub fn icon_button(ui: &mut Ui, pal: &Palette, icon: Icon, size: f32, enabled: bool) -> Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), sense);
    let fill = if !enabled {
        transparent()
    } else if resp.is_pointer_button_down_on() {
        pal.surface_highest
    } else if resp.hovered() {
        pal.surface_container
    } else {
        transparent()
    };
    if fill != transparent() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(R_SM), fill);
    }
    let color = if enabled {
        pal.on_surface_variant
    } else {
        fade(pal.outline, 0.5)
    };
    let inner = rect.shrink(rect.width() * 0.22);
    icons::draw(ui.painter(), icon, inner, color);
    resp
}

/// Copy-to-clipboard icon button; returns `true` on the click that copied.
pub fn copy_button(ui: &mut Ui, pal: &Palette, value: &str) -> bool {
    let r = icon_button(ui, pal, Icon::Copy, 26.0, true);
    if r.clicked() {
        ui.ctx().copy_text(value.to_string());
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------- indicators

/// Leading-dot status pill, e.g. `● Connected`. Color never carries the
/// meaning alone — always pair with the visible label.
pub fn status_pill(ui: &mut Ui, pal: &Palette, kind: StatusKind, label: &str) -> Response {
    let sc = pal.status(kind);
    pill(ui, &sc, label, true)
}

/// Small tonal tag without a dot — path badges, host markers.
pub fn tag(ui: &mut Ui, sc: &StatusColor, label: &str) -> Response {
    pill(ui, sc, label, false)
}

/// Neutral tag using secondary container tones.
pub fn neutral_tag(ui: &mut Ui, pal: &Palette, label: &str) -> Response {
    let sc = StatusColor {
        fg: pal.on_secondary_container,
        bg: pal.secondary_container,
    };
    pill(ui, &sc, label, false)
}

fn pill(ui: &mut Ui, sc: &StatusColor, text: &str, dot: bool) -> Response {
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout_job(egui::text::LayoutJob::simple(
        text.to_string(),
        font,
        sc.fg,
        f32::INFINITY,
    ));
    let tw = galley.size().x + 20.0 + if dot { 7.0 } else { 0.0 };
    let th = 21.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(tw, th), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same((th / 2.0) as u8), sc.bg);
    let cy = rect.center().y;
    let mut text_left = rect.left() + 10.0;
    if dot {
        p.circle_filled(Pos2::new(rect.left() + 12.0, cy), 3.2, sc.fg);
        text_left = rect.left() + 19.0;
    }
    p.galley(
        Pos2::new(text_left, cy - galley.size().y / 2.0),
        galley,
        sc.fg,
    );
    resp
}

/// Colored status dot on its own (sidebar rows). `pulse` marks
/// connecting/waiting states with a gentle radial beat.
pub fn status_dot(ui: &mut Ui, color: Color32, pulse: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    if pulse {
        let t = ui.input(|i| i.time) as f32;
        let phase = (t * 1.6).fract();
        let k = 1.0 - phase;
        p.circle_filled(c, 4.6 + k * 4.5, fade(color, 0.35 * (1.0 - phase)));
    }
    p.circle_filled(c, 4.6, color);
    resp
}

// ---------------------------------------------------------------- switch

/// M3-style switch bound to a bool. Track on = primary; the knob grows and
/// gains a check when on. Only the knob animates, and only while toggling.
pub fn switch(ui: &mut Ui, pal: &Palette, on: &mut bool) -> Response {
    let size = vec2(36.0, 20.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let p = ui.painter();
    let r = rect.height() / 2.0;

    // track
    if *on {
        p.rect_filled(rect, CornerRadius::same(r as u8), pal.primary);
    } else {
        p.rect_filled(rect, CornerRadius::same(r as u8), pal.surface_highest);
        p.rect_stroke(
            rect,
            CornerRadius::same(r as u8),
            Stroke::new(1.5, pal.outline),
            egui::StrokeKind::Middle,
        );
    }

    // knob: 12px off -> 15px on, carries a check when on
    let knob_r = 6.0 + 1.5 * t;
    let c = Pos2::new(
        rect.left() + 2.0 + 6.0 + (rect.width() - 4.0 - 12.0) * t,
        rect.center().y,
    );
    let knob = if *on { pal.on_primary } else { pal.outline };
    p.circle_filled(c, knob_r, knob);
    if t > 0.5 {
        icons::draw(
            p,
            Icon::Check,
            egui::Rect::from_center_size(c, vec2(knob_r, knob_r)),
            fade(pal.primary, (t - 0.5) * 2.0),
        );
    }
    resp
}

// ---------------------------------------------------------------- containers

/// Standard card frame: tonal surface, generous radius, no border.
pub fn card(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.surface_low)
        .corner_radius(crate::theme::R_LG)
        .inner_margin(egui::Margin::same(16))
}

/// One hairline separator across the available width.
pub fn hairline(ui: &mut Ui, pal: &Palette) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().hline(
        rect.left()..=rect.right(),
        rect.center().y,
        Stroke::new(1.0, pal.outline_variant),
    );
}

// ---------------------------------------------------------------- inputs
/// Labeled single-line text field with hint and inline error state.
pub fn field(
    ui: &mut Ui,
    pal: &Palette,
    label: &str,
    value: &mut String,
    hint: &str,
    error: Option<&str>,
) -> Response {
    ui.label(theme::label(label).color(pal.on_surface_variant));
    let resp = ui.scope(|ui| {
        let v = ui.visuals_mut();
        v.widgets.inactive.bg_fill = pal.field_bg;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, pal.outline_variant);
        v.widgets.inactive.corner_radius = CornerRadius::same(R_MD);
        v.widgets.hovered.bg_fill = pal.field_bg;
        v.widgets.hovered.corner_radius = CornerRadius::same(R_MD);
        v.widgets.active.bg_fill = pal.field_bg;
        v.widgets.active.corner_radius = CornerRadius::same(R_MD);
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(hint)
                .desired_width(f32::INFINITY)
                .min_size(vec2(0.0, 38.0)),
        )
    });
    let resp = resp.inner;
    if let Some(err) = error {
        ui.painter().rect_stroke(
            resp.rect.shrink(0.5),
            CornerRadius::same(R_MD),
            Stroke::new(1.6, pal.error.fg),
            egui::StrokeKind::Middle,
        );
        let galley = ui.painter().layout_job(egui::text::LayoutJob::simple(
            err.to_string(),
            FontId::proportional(11.5),
            pal.error.fg,
            f32::INFINITY,
        ));
        let (er, _) = ui.allocate_exact_size(
            vec2(galley.size().x + 2.0, galley.size().y + 3.0),
            Sense::hover(),
        );
        ui.painter().galley(er.left_top(), galley, pal.error.fg);
    } else {
        ui.add_space(1.0);
    }
    resp
}

/// M3 segmented control (single-select). Options are (value, label) pairs.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    pal: &Palette,
    id: &str,
    options: &[(&T, &str)],
    selected: &mut T,
) {
    let h = 38.0;
    let n = options.len().max(1);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(R_MD), pal.field_bg);
    let seg_w = rect.width() / n as f32;
    for (i, pair) in options.iter().enumerate() {
        let (val, label): (&T, &str) = *pair;
        let seg = egui::Rect::from_min_max(
            Pos2::new(rect.left() + i as f32 * seg_w, rect.top()),
            Pos2::new(rect.left() + (i + 1) as f32 * seg_w, rect.bottom()),
        );
        let resp = ui.interact(seg, Id::new(id).with(i), Sense::click());
        let is_sel = *val == *selected;
        if is_sel {
            p.rect_filled(
                seg.shrink(2.0),
                CornerRadius::same(R_SM),
                pal.secondary_container,
            );
        } else if resp.hovered() {
            p.rect_filled(
                seg.shrink(2.0),
                CornerRadius::same(R_SM),
                pal.surface_container,
            );
        }
        let fg = if is_sel {
            pal.on_secondary_container
        } else {
            pal.on_surface_variant
        };
        let galley = p.layout_job(egui::text::LayoutJob::simple(
            (*label).to_string(),
            FontId::proportional(12.5),
            fg,
            f32::INFINITY,
        ));
        let tw = galley.size().x + if is_sel { 18.0 } else { 0.0 };
        let mut x = seg.center().x - tw / 2.0;
        if is_sel {
            icons::draw(
                p,
                Icon::Check,
                egui::Rect::from_center_size(Pos2::new(x + 7.0, seg.center().y), vec2(13.0, 13.0)),
                pal.on_secondary_container,
            );
            x += 18.0;
        }
        p.galley(
            Pos2::new(x, seg.center().y - galley.size().y / 2.0),
            galley,
            fg,
        );
        if resp.clicked() {
            *selected = *val;
        }
    }
}

// ---------------------------------------------------------------- feedback

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SnackKind {
    Ok,
    Error,
}

#[derive(Clone)]
pub struct Snack {
    pub text: String,
    pub kind: SnackKind,
    pub at: std::time::Instant,
}

/// Stacked snackbars, bottom-center, auto-dismissing with a gentle fade.
/// Inverse-surface tones per M3, so they read on top of any surface.
pub fn snackbars(ctx: &egui::Context, pal: &Palette, snacks: &mut Vec<Snack>) {
    let now = std::time::Instant::now();
    snacks.retain(|s| now.duration_since(s.at).as_secs_f32() < 4.5);
    if snacks.is_empty() {
        return;
    }
    let shown: Vec<Snack> = snacks.iter().rev().take(3).rev().cloned().collect();
    egui::Area::new(egui::Id::new("gg_snacks"))
        .anchor(Align2::CENTER_BOTTOM, [0.0, -SP3])
        .order(egui::Order::Foreground)
        .interactable(false)
        .show(ctx, |ui| {
            for s in shown {
                let age = now.duration_since(s.at).as_secs_f32();
                let alpha = if age > 3.5 {
                    ((4.5 - age) / 1.0).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let (bg, fg) = if pal.dark {
                    (
                        Color32::from_rgb(0xE6, 0xE0, 0xE9),
                        Color32::from_rgb(0x31, 0x2E, 0x36),
                    )
                } else {
                    (
                        Color32::from_rgb(0x32, 0x2F, 0x35),
                        Color32::from_rgb(0xF5, 0xEF, 0xF7),
                    )
                };
                let bg = fade(bg, alpha);
                let fg = fade(fg, alpha);
                let (icon, ic) = match s.kind {
                    SnackKind::Ok => (
                        Icon::Check,
                        if pal.dark {
                            theme::DARK.success.fg
                        } else {
                            theme::LIGHT.success.fg
                        },
                    ),
                    SnackKind::Error => (
                        Icon::Close,
                        if pal.dark {
                            theme::DARK.error.fg
                        } else {
                            theme::LIGHT.error.fg
                        },
                    ),
                };
                egui::Frame::new()
                    .fill(bg)
                    .corner_radius(R_MD)
                    .inner_margin(egui::Margin::symmetric(14, 9))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            icons::draw(
                                ui.painter(),
                                icon,
                                egui::Rect::from_center_size(
                                    ui.next_widget_position() + vec2(8.0, 9.0),
                                    vec2(14.0, 14.0),
                                ),
                                ic,
                            );
                            ui.add_space(20.0);
                            ui.label(theme::label(s.text.clone()).color(fg));
                        });
                    });
                ui.add_space(SP1);
            }
        });
}

// ---------------------------------------------------------------- empty state

/// Centered icon + title + body for empty / placeholder content.
pub fn empty_state(ui: &mut Ui, pal: &Palette, icon: Icon, title: &str, body: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(SP2);
        let (rect, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(R_SM), pal.secondary_container);
        icons::draw(p, icon, rect.shrink(14.0), pal.on_secondary_container);
        ui.add_space(SP2);
        ui.label(theme::section(title).color(pal.on_surface));
        ui.label(theme::label(body).color(pal.on_surface_variant));
        ui.add_space(SP1);
    });
}
