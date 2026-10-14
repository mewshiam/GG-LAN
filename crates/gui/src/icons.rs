//! Outlined vector icons, drawn directly with the egui painter.
//!
//! Deliberately dependency-free: every icon is a small stroke drawing on a
//! normalized unit square, so they stay crisp at any size, follow the active
//! palette color, and add zero binaries or font files.

use egui::{Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

const SW: f32 = 1.7; // base stroke width at 24px

/// Polyline approximation of a circular arc (no native arc shape in epaint).
fn arc(p: &egui::Painter, c: Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    let steps = (((a1 - a0).abs() / 0.25).ceil() as usize).clamp(4, 32);
    let pts: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let a = a0 + (a1 - a0) * (i as f32 / steps as f32);
            Pos2::new(c.x + a.cos() * r, c.y + a.sin() * r)
        })
        .collect();
    p.add(egui::Shape::line(pts, stroke));
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Three connected nodes — the GG LAN mark.
    Lan,
    /// Gear — settings.
    Settings,
    /// Stacked server boxes — hosting / coordinator.
    Server,
    /// Standby symbol — connect / disconnect.
    Power,
    /// Arrow down into a tray — download.
    Download,
    /// Arrow up out of a tray — upload.
    Upload,
    /// Two people — peers.
    Peers,
    /// Terminal prompt — diagnostics / event log.
    Terminal,
    /// Two overlapping sheets — copy.
    Copy,
    /// Checkmark.
    Check,
    /// Cross — close / cancel.
    Close,
    /// Vertical ellipsis — overflow menu.
    Overflow,
    /// Globe — remote network.
    Globe,
}

/// Draw `icon` inside `rect` (the drawing fills the rect) in `color`.
pub fn draw(p: &egui::Painter, icon: Icon, rect: Rect, color: Color32) {
    let size = rect.width().min(rect.height());
    let stroke = Stroke::new((SW * size / 24.0).max(1.2), color);
    // Work in unit space: x,y in 0..24, mapped into the rect.
    let o = rect.left_top();
    let pt = |x: f32, y: f32| Pos2::new(o.x + x * size / 24.0, o.y + y * size / 24.0);

    match icon {
        Icon::Lan => {
            let nodes = [(12.0, 4.5), (4.5, 19.0), (19.5, 19.0)];
            for (a, b) in [(0usize, 1), (0, 2), (1, 2)] {
                p.line_segment(
                    [pt(nodes[a].0, nodes[a].1), pt(nodes[b].0, nodes[b].1)],
                    stroke,
                );
            }
            for (x, y) in nodes {
                p.circle_stroke(pt(x, y), size / 10.0, stroke);
            }
        }
        Icon::Settings => {
            let c = pt(12.0, 12.0);
            p.circle_stroke(c, size * 6.5 / 24.0, stroke);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let d1 = size * 8.6 / 24.0;
                let d2 = size * 10.6 / 24.0;
                p.line_segment(
                    [
                        Pos2::new(c.x + a.cos() * d1, c.y + a.sin() * d1),
                        Pos2::new(c.x + a.cos() * d2, c.y + a.sin() * d2),
                    ],
                    stroke,
                );
            }
        }
        Icon::Server => {
            for y in [4.0, 14.0] {
                let r = egui::Rect::from_min_max(pt(4.0, y), pt(20.0, y + 6.0));
                p.rect_stroke(
                    r,
                    egui::CornerRadius::same(2),
                    stroke,
                    egui::StrokeKind::Middle,
                );
                p.circle_filled(pt(7.0, y + 3.0), size / 48.0, color);
            }
            p.line_segment([pt(12.0, 10.0), pt(12.0, 14.0)], stroke);
        }
        Icon::Power => {
            let c = pt(12.0, 13.0);
            let r = size * 7.5 / 24.0;
            // arc with a gap at the top (standby symbol)
            let gap = 0.55;
            arc(p, c, r, gap, std::f32::consts::TAU - gap, stroke);
            p.line_segment([pt(12.0, 4.5), pt(12.0, 10.5)], stroke);
        }
        Icon::Download => {
            p.line_segment([pt(12.0, 4.0), pt(12.0, 14.5)], stroke);
            p.line_segment([pt(7.5, 10.5), pt(12.0, 14.5)], stroke);
            p.line_segment([pt(16.5, 10.5), pt(12.0, 14.5)], stroke);
            p.line_segment([pt(4.5, 19.0), pt(19.5, 19.0)], stroke);
        }
        Icon::Upload => {
            p.line_segment([pt(12.0, 14.5), pt(12.0, 5.0)], stroke);
            p.line_segment([pt(7.5, 9.0), pt(12.0, 5.0)], stroke);
            p.line_segment([pt(16.5, 9.0), pt(12.0, 5.0)], stroke);
            p.line_segment([pt(4.5, 19.0), pt(19.5, 19.0)], stroke);
        }
        Icon::Peers => {
            // back person
            p.circle_stroke(pt(16.5, 8.5), size * 2.6 / 24.0, stroke);
            arc(
                p,
                pt(16.5, 21.0),
                size * 5.6 / 24.0,
                std::f32::consts::PI,
                std::f32::consts::TAU,
                stroke,
            );
            // front person
            p.circle_stroke(pt(8.5, 8.5), size * 2.9 / 24.0, stroke);
            arc(
                p,
                pt(8.5, 21.0),
                size * 6.4 / 24.0,
                std::f32::consts::PI,
                std::f32::consts::TAU,
                stroke,
            );
        }
        Icon::Terminal => {
            let r = egui::Rect::from_min_max(pt(3.5, 4.5), pt(20.5, 19.5));
            p.rect_stroke(
                r,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
            p.line_segment([pt(7.0, 10.0), pt(9.5, 12.3)], stroke);
            p.line_segment([pt(9.5, 12.3), pt(7.0, 14.6)], stroke);
            p.line_segment([pt(11.5, 15.0), pt(16.0, 15.0)], stroke);
        }
        Icon::Copy => {
            let back = egui::Rect::from_min_max(pt(8.5, 3.5), pt(20.5, 15.5));
            let front = egui::Rect::from_min_max(pt(3.5, 8.5), pt(15.5, 20.5));
            p.rect_stroke(
                back,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
            p.rect_stroke(
                front,
                egui::CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        Icon::Check => {
            p.line_segment([pt(5.0, 12.5), pt(9.5, 17.0)], stroke);
            p.line_segment([pt(9.5, 17.0), pt(19.0, 7.0)], stroke);
        }
        Icon::Close => {
            p.line_segment([pt(6.0, 6.0), pt(18.0, 18.0)], stroke);
            p.line_segment([pt(18.0, 6.0), pt(6.0, 18.0)], stroke);
        }
        Icon::Overflow => {
            for x in [6.0, 12.0, 18.0] {
                p.circle_filled(pt(x, 12.0), size / 36.0, color);
            }
        }
        Icon::Globe => {
            p.circle_stroke(pt(12.0, 12.0), size * 8.0 / 24.0, stroke);
            p.add(egui::Shape::ellipse_stroke(
                pt(12.0, 12.0),
                Vec2::new(size * 7.8 / 24.0, size * 8.0 / 24.0),
                stroke,
            ));
            p.line_segment([pt(4.0, 12.0), pt(20.0, 12.0)], stroke);
        }
    }
}

/// Allocate a square icon slot and draw the icon. Hover-sensed only; wrap
/// with an interactable parent for buttons.
pub fn show(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    draw(ui.painter(), icon, rect, color);
    resp
}
