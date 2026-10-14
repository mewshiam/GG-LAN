//! GG LAN design system — Material 3 inspired semantic tokens.
//!
//! Every color the UI draws comes from a [`Palette`], of which there are two
//! static instances ([`LIGHT`] and [`DARK`]). Components never hardcode
//! colors; they receive the active palette, so light/dark theming is a matter
//! of swapping one reference.
//!
//! The palette follows Material 3 tonal roles:
//! - `primary` / `on_primary` / `primary_container` — brand green, main actions
//! - `secondary_container` / `on_secondary_container` — selected nav items,
//!   tonal (non-filled) buttons
//! - `surface*` — elevation ladder (M3 tonal elevation: darker = higher in
//!   light theme, lighter = higher in dark theme)
//! - `on_surface` / `on_surface_variant` / `outline` / `outline_variant` — text
//!   and divider roles
//! - status roles (`success_*`, `info_*`, `warn_*`, `error_*`, `off_*`) —
//!   connection state communication, deliberately distinct from the brand hue

use egui::{Color32, Context, Visuals};

// ---------------------------------------------------------------- palette

#[derive(Clone, Copy)]
pub struct StatusColor {
    /// Foreground (dot / text).
    pub fg: Color32,
    /// Container (background) tone.
    pub bg: Color32,
}

/// A named connection state used by status widgets.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Connected,
    Connecting,
    Waiting,
    Offline,
    Error,
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,

    // --- brand / actions
    pub primary: Color32,
    pub on_primary: Color32,
    pub primary_container: Color32,
    pub on_primary_container: Color32,
    pub secondary_container: Color32,
    pub on_secondary_container: Color32,

    // --- tertiary (info / upload series)
    pub tertiary: Color32,

    // --- surfaces
    pub surface: Color32,
    pub surface_low: Color32,
    pub surface_container: Color32,
    pub surface_high: Color32,
    pub surface_highest: Color32,
    /// Field / input background.
    pub field_bg: Color32,
    /// Dialog surface (container high).
    pub dialog: Color32,

    // --- text & lines
    pub on_surface: Color32,
    pub on_surface_variant: Color32,
    pub outline: Color32,
    pub outline_variant: Color32,

    // --- status roles
    pub success: StatusColor,
    pub info: StatusColor,
    pub warn: StatusColor,
    pub error: StatusColor,
    pub off: StatusColor,
    /// Text on a filled `error.fg` button.
    pub on_error: Color32,

    // --- series (chart)
    pub series_down: Color32,
    pub series_up: Color32,
}

pub const LIGHT: Palette = Palette {
    dark: false,

    primary: Color32::from_rgb(0x38, 0x6A, 0x20),
    on_primary: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    primary_container: Color32::from_rgb(0xB7, 0xF3, 0x97),
    on_primary_container: Color32::from_rgb(0x10, 0x20, 0x00),

    secondary_container: Color32::from_rgb(0xD9, 0xE7, 0xCB),
    on_secondary_container: Color32::from_rgb(0x14, 0x1E, 0x0B),

    tertiary: Color32::from_rgb(0x2E, 0x6F, 0xA3),

    surface: Color32::from_rgb(0xFA, 0xFA, 0xF5),
    surface_low: Color32::from_rgb(0xF4, 0xF4, 0xEE),
    surface_container: Color32::from_rgb(0xEE, 0xEE, 0xE7),
    surface_high: Color32::from_rgb(0xE8, 0xE8, 0xE1),
    surface_highest: Color32::from_rgb(0xE2, 0xE2, 0xDB),
    field_bg: Color32::from_rgb(0xEC, 0xEC, 0xE4),
    dialog: Color32::from_rgb(0xE8, 0xE8, 0xE1),

    on_surface: Color32::from_rgb(0x1B, 0x1C, 0x18),
    on_surface_variant: Color32::from_rgb(0x45, 0x48, 0x3E),
    outline: Color32::from_rgb(0x74, 0x79, 0x6D),
    outline_variant: Color32::from_rgb(0xC4, 0xC8, 0xBA),

    success: StatusColor {
        fg: Color32::from_rgb(0x00, 0x6E, 0x57),
        bg: Color32::from_rgb(0xC6, 0xEF, 0xDF),
    },
    info: StatusColor {
        fg: Color32::from_rgb(0x28, 0x5E, 0x8A),
        bg: Color32::from_rgb(0xD1, 0xE3, 0xF8),
    },
    warn: StatusColor {
        fg: Color32::from_rgb(0x82, 0x59, 0x00),
        bg: Color32::from_rgb(0xFF, 0xE0, 0xB8),
    },
    error: StatusColor {
        fg: Color32::from_rgb(0xB3, 0x26, 0x1E),
        bg: Color32::from_rgb(0xF9, 0xDE, 0xDC),
    },
    off: StatusColor {
        fg: Color32::from_rgb(0x6B, 0x6F, 0x61),
        bg: Color32::from_rgb(0xE4, 0xE5, 0xDC),
    },
    on_error: Color32::from_rgb(0xFF, 0xFF, 0xFF),

    series_down: Color32::from_rgb(0x38, 0x6A, 0x20),
    series_up: Color32::from_rgb(0x2E, 0x6F, 0xA3),
};

pub const DARK: Palette = Palette {
    dark: true,

    primary: Color32::from_rgb(0x9F, 0xD7, 0x7C),
    on_primary: Color32::from_rgb(0x0F, 0x39, 0x00),
    primary_container: Color32::from_rgb(0x29, 0x52, 0x00),
    on_primary_container: Color32::from_rgb(0xBB, 0xF0, 0x99),

    secondary_container: Color32::from_rgb(0x3F, 0x4A, 0x38),
    on_secondary_container: Color32::from_rgb(0xD5, 0xE7, 0xC8),

    tertiary: Color32::from_rgb(0xA5, 0xC8, 0xF2),

    surface: Color32::from_rgb(0x12, 0x14, 0x10),
    surface_low: Color32::from_rgb(0x1A, 0x1C, 0x17),
    surface_container: Color32::from_rgb(0x1E, 0x20, 0x1B),
    surface_high: Color32::from_rgb(0x28, 0x2B, 0x25),
    surface_highest: Color32::from_rgb(0x33, 0x36, 0x30),
    field_bg: Color32::from_rgb(0x33, 0x36, 0x30),
    dialog: Color32::from_rgb(0x28, 0x2B, 0x25),

    on_surface: Color32::from_rgb(0xE3, 0xE3, 0xDC),
    on_surface_variant: Color32::from_rgb(0xC5, 0xC9, 0xBB),
    outline: Color32::from_rgb(0x8F, 0x93, 0x84),
    outline_variant: Color32::from_rgb(0x4A, 0x4E, 0x44),

    success: StatusColor {
        fg: Color32::from_rgb(0x63, 0xDB, 0xAD),
        bg: Color32::from_rgb(0x00, 0x51, 0x3E),
    },
    info: StatusColor {
        fg: Color32::from_rgb(0xA5, 0xC8, 0xF2),
        bg: Color32::from_rgb(0x28, 0x46, 0x69),
    },
    warn: StatusColor {
        fg: Color32::from_rgb(0xF2, 0xC7, 0x77),
        bg: Color32::from_rgb(0x5C, 0x44, 0x0B),
    },
    error: StatusColor {
        fg: Color32::from_rgb(0xF2, 0xB8, 0xB5),
        bg: Color32::from_rgb(0x8C, 0x1D, 0x18),
    },
    off: StatusColor {
        fg: Color32::from_rgb(0xA6, 0xAA, 0x9D),
        bg: Color32::from_rgb(0x3F, 0x42, 0x39),
    },
    on_error: Color32::from_rgb(0x60, 0x14, 0x10),

    series_down: Color32::from_rgb(0x9F, 0xD7, 0x7C),
    series_up: Color32::from_rgb(0x8F, 0xB8, 0xE8),
};

impl Palette {
    pub fn status(&self, kind: StatusKind) -> StatusColor {
        match kind {
            StatusKind::Connected => self.success,
            StatusKind::Connecting => self.info,
            StatusKind::Waiting => self.warn,
            StatusKind::Offline => self.off,
            StatusKind::Error => self.error,
        }
    }

    /// Blend `c` towards this palette's page tone (used for hover overlays).
    pub fn over_surface(&self, c: Color32, w: f32) -> Color32 {
        let base = self.surface_low;
        let f = |a: u8, b: u8| -> u8 { (a as f32 * (1.0 - w) + b as f32 * w).round() as u8 };
        Color32::from_rgb(f(c.r(), base.r()), f(c.g(), base.g()), f(c.b(), base.b()))
    }

    /// Slight tonal shift for hovering a tonal container: darker in light
    /// theme, lighter in dark theme (M3 tonal elevation direction).
    pub fn tonal_hover(&self, c: Color32) -> Color32 {
        let target = if self.dark {
            self.surface_highest
        } else {
            Color32::BLACK
        };
        let w = if self.dark { 0.10 } else { 0.06 };
        let f = |a: u8, b: u8| -> u8 { (a as f32 * (1.0 - w) + b as f32 * w).round() as u8 };
        Color32::from_rgb(
            f(c.r(), target.r()),
            f(c.g(), target.g()),
            f(c.b(), target.b()),
        )
    }

    /// A translucent overlay of `c` (used for chart area fills).
    pub fn area_fill(&self, c: Color32) -> Color32 {
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 36)
    }
}

// ---------------------------------------------------------------- preference

/// User theme preference, persisted in `gui.json`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ThemePrefs {
    System,
    Light,
    Dark,
}

impl ThemePrefs {
    pub fn as_str(&self) -> &'static str {
        match self {
            ThemePrefs::System => "system",
            ThemePrefs::Light => "light",
            ThemePrefs::Dark => "dark",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "light" => ThemePrefs::Light,
            "dark" => ThemePrefs::Dark,
            _ => ThemePrefs::System,
        }
    }

    /// Resolve the effective palette, consulting the OS theme when `System`.
    pub fn palette(&self, ctx: &Context) -> &'static Palette {
        let dark = match self {
            ThemePrefs::Light => false,
            ThemePrefs::Dark => true,
            ThemePrefs::System => ctx
                .system_theme()
                .map(|t| t == egui::Theme::Dark)
                .unwrap_or(false),
        };
        if dark {
            &DARK
        } else {
            &LIGHT
        }
    }
}

// ---------------------------------------------------------------- visuals

/// Push egui's built-in widget/selection visuals towards the palette so the
/// stock widgets (TextEdit, ComboBox, checkboxes, scrollbars, the text
/// cursor, window shadows) harmonize with the custom components.
pub fn apply_visuals(ctx: &Context, pal: &Palette) {
    let mut v = if pal.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = pal.surface;
    v.window_fill = pal.dialog;
    v.extreme_bg_color = pal.field_bg;
    v.faint_bg_color = pal.surface_container;
    // Hints/placeholders must stay readable on the field tone in both themes
    // (the stock weak gray is too faint on the dark palette).
    v.weak_text_color = Some(pal.on_surface_variant);
    v.hyperlink_color = pal.tertiary;
    v.selection.bg_fill = pal.primary;
    v.selection.stroke.color = pal.on_primary;
    v.text_cursor.stroke.color = pal.on_surface;
    v.warn_fg_color = pal.warn.fg;
    v.error_fg_color = pal.error.fg;

    let w = &mut v.widgets;
    w.inactive.bg_fill = pal.field_bg;
    w.inactive.bg_stroke = egui::Stroke::new(1.0, pal.outline_variant);
    w.inactive.fg_stroke.color = pal.on_surface;
    w.hovered.bg_fill = pal.surface_highest;
    w.hovered.fg_stroke.color = pal.on_surface;
    w.active.bg_fill = pal.surface_highest;
    w.active.fg_stroke.color = pal.on_surface;
    w.open.bg_fill = pal.surface_highest;
    w.open.fg_stroke.color = pal.on_surface;

    ctx.set_visuals(v);
}

// ---------------------------------------------------------------- typography
//
// One type scale, expressed as helper functions so every screen speaks the
// same language. Ubuntu (egui's bundled proportional face) for UI text,
// the bundled monospace face for numbers, addresses and diagnostics.

pub fn page_title(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(24.0).strong()
}

pub fn section(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(16.0).strong()
}

pub fn body(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(14.0)
}

pub fn label(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(12.5)
}

pub fn caption(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(11.0)
}

/// Monospace for IPs, endpoints, join codes.
pub fn mono(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(13.0).monospace()
}

/// Small monospace (event log, chart axis).
pub fn mono_small(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(11.0).monospace()
}

// ---------------------------------------------------------------- metrics

/// 4px base grid; prefer these over ad-hoc numbers.
pub const SP1: f32 = 4.0;
pub const SP2: f32 = 8.0;
pub const SP3: f32 = 12.0;
pub const SP4: f32 = 16.0;
pub const SP6: f32 = 24.0;

/// Corner radii (M3 shape scale, desktop-tuned).
pub const R_SM: u8 = 8; // chips, small controls
pub const R_MD: u8 = 10; // buttons, inputs
pub const R_LG: u8 = 14; // cards
pub const R_XL: u8 = 22; // dialogs

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
