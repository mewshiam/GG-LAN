//! GG LAN visual language: colors, small custom widgets, formatters.

use egui::{vec2, Color32, CornerRadius, FontId, Pos2, Response, Sense, Stroke, Ui};

pub const ACCENT: Color32 = Color32::from_rgb(26, 115, 232); // blue 600
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(232, 240, 254); // blue 50
pub const GOOD: Color32 = Color32::from_rgb(30, 142, 62); // green 700
pub const GOOD_SOFT: Color32 = Color32::from_rgb(230, 244, 233);
pub const WARN: Color32 = Color32::from_rgb(197, 138, 0); // amber 700
pub const WARN_SOFT: Color32 = Color32::from_rgb(254, 243, 219);
pub const BAD: Color32 = Color32::from_rgb(217, 48, 37); // red 600
pub const BAD_SOFT: Color32 = Color32::from_rgb(251, 233, 231);
pub const IDLE: Color32 = Color32::from_rgb(128, 134, 139);
pub const IDLE_SOFT: Color32 = Color32::from_rgb(241, 243, 244);
pub const INK: Color32 = Color32::from_rgb(32, 33, 36);
pub const MUTED: Color32 = Color32::from_rgb(95, 99, 104);
pub const CARD: Color32 = Color32::from_rgb(255, 255, 255);
pub const PAGE: Color32 = Color32::from_rgb(242, 244, 247);
pub const HAIRLINE: Color32 = Color32::from_rgb(218, 222, 227);

pub fn card_frame(ui: &mut Ui) -> egui::Frame {
    egui::Frame::group(ui.style())
        .fill(CARD)
        .stroke(Stroke::new(1.0, HAIRLINE))
        .corner_radius(10)
        .inner_margin(egui::Margin::same(12))
}

/// Lighten `c` towards white by factor `w` (0 = keep, 1 = white).
pub fn mix_white(c: Color32, w: f32) -> Color32 {
    let f = |x: u8| -> u8 { (x as f32 * (1.0 - w) + 255.0 * w).round() as u8 };
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// Rounded status pill with a leading dot, e.g. `● Connected`.
pub fn pill(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) -> Response {
    let font = FontId::proportional(11.5);
    let galley = ui.painter().layout_job(egui::text::LayoutJob::simple(
        text.to_owned(),
        font,
        fg,
        f32::INFINITY,
    ));
    let tw = galley.size().x + 24.0;
    let th = 20.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(tw, th), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(th as u8 / 2), bg);
    let cy = rect.center().y;
    p.circle_filled(Pos2::new(rect.left() + 9.0, cy), 3.0, fg);
    p.galley(
        Pos2::new(rect.left() + 17.0, cy - galley.size().y / 2.0),
        galley,
        fg,
    );
    resp
}

/// iOS-style toggle switch bound to a bool.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let size = vec2(36.0, 20.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let track = if *on {
        ACCENT
    } else {
        Color32::from_rgb(189, 193, 198)
    };
    let r = rect.height() / 2.0;
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(r as u8), track);
    let pad = 2.0;
    let knob_r = r - pad;
    let x = rect.left() + pad + knob_r + (rect.width() - 2.0 * (pad + knob_r)) * t;
    p.circle_filled(Pos2::new(x, rect.center().y), knob_r, Color32::WHITE);
    resp
}

/// Bar-style sparkline for a bit-rate series; returns the inner plot rect.
pub fn spark_chart(ui: &mut Ui, series: &[u64], color: Color32, height: f32) -> egui::Rect {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(8), CARD);
    let inner = rect.shrink(8.0);
    if series.is_empty() || inner.width() <= 0.0 || inner.height() <= 0.0 {
        return inner;
    }
    // faint horizontal grid
    for i in 1..4 {
        let y = inner.top() + inner.height() * (i as f32 / 4.0);
        p.line_segment(
            [Pos2::new(inner.left(), y), Pos2::new(inner.right(), y)],
            Stroke::new(1.0, HAIRLINE),
        );
    }
    let max = series.iter().copied().max().unwrap_or(1).clamp(1, u64::MAX) as f32;
    let n = series.len() as f32;
    let bw = (inner.width() / n).clamp(1.5, 10.0);
    let pitch = inner.width() / n;
    let bar_fill = mix_white(color, 0.55);
    for (i, v) in series.iter().enumerate() {
        let h = (*v as f32 / max) * inner.height();
        if h < 0.5 {
            continue;
        }
        let x = inner.right() - ((n - i as f32) * pitch) + (pitch - bw) / 2.0;
        p.rect_filled(
            egui::Rect::from_min_size(Pos2::new(x, inner.bottom() - h), vec2(bw, h)),
            CornerRadius::same(1),
            bar_fill,
        );
    }
    inner
}

// ---------------------------------------------------------------- formatters

pub fn fmt_bps(v: u64) -> String {
    let units = ["b/s", "Kb/s", "Mb/s", "Gb/s"];
    let mut v = v as f64;
    let mut u = 0;
    while v >= 1000.0 && u < units.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{v:.0} {}", units[u])
    } else {
        format!("{v:.1} {}", units[u])
    }
}

pub fn fmt_bytes(v: u64) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = v as f64;
    let mut u = 0;
    while v >= 1024.0 && u < units.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{v:.0} {}", units[u])
    } else {
        format!("{v:.1} {}", units[u])
    }
}
