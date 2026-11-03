//! ShardStroke's look: Painter's layout and structure, a modern surface. The theme is monochrome (pure neutral
//! grays, three elevation steps: window < card < control) and the only colour is the highlight, which the user
//! picks in Settings (`set_accent`). Segoe UI where it exists.

use std::sync::atomic::{AtomicU32, Ordering};

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke};

const fn g(v: u8) -> Color32 {
    Color32::from_rgb(v, v, v)
}

// ---- surfaces (neutral)
/// Window background: the 3D viewport sits on it and shows in the gaps between the cards.
pub const BG_VIEWPORT: Color32 = g(14);
/// Solid card body (popups, floating windows, wells inside cards).
pub const PANEL: Color32 = g(27);
/// Tab bars and title strips, a step below the card body.
pub const PANEL_ALT: Color32 = g(21);
pub const BORDER: Color32 = g(46);
/// Sunken areas: lists, thumbnails, text fields.
pub const WELL: Color32 = g(17);

// ---- controls (neutral)
pub const WIDGET: Color32 = g(40);
pub const WIDGET_HOVER: Color32 = g(53);
pub const WIDGET_PRESSED: Color32 = g(32);
pub const SLIDER_TRACK: Color32 = g(52);
pub const SLIDER_TRACK_OFF: Color32 = g(36);
pub const SLIDER_FILL_OFF: Color32 = g(72);

// ---- text (neutral)
pub const TEXT: Color32 = g(226);
/// Secondary labels, units, hints.
pub const TEXT_DIM: Color32 = g(150);
pub const TEXT_DISABLED: Color32 = g(92);
/// Numeric values of sliders.
pub const VALUE: Color32 = g(240);

// ---- status colours (the only other colours)
pub const ERROR: Color32 = Color32::from_rgb(244, 91, 86);
pub const WARNING: Color32 = Color32::from_rgb(255, 189, 46);

// ---- shape
/// Radius of dock windows and floating panels.
pub const RADIUS_CARD: u8 = 10;
/// Radius of buttons, fields, rows.
pub const RADIUS: u8 = 6;
/// Gap between the cards.
pub const GAP: f32 = 8.0;

// ---- the highlight colour (editable in Settings)

/// Highlight presets offered in Settings; the first one is the default (Painter's green).
pub const ACCENT_PRESETS: [(&str, [u8; 3]); 7] = [
    ("Green", [124, 204, 74]),
    ("Blue", [77, 140, 255]),
    ("Teal", [48, 190, 170]),
    ("Orange", [255, 150, 50]),
    ("Pink", [240, 100, 160]),
    ("Purple", [160, 120, 255]),
    ("White", [235, 235, 235]),
];

static ACCENT_RGB: AtomicU32 = AtomicU32::new(0x7C_CC_4A);

pub fn accent_rgb() -> [u8; 3] {
    let v = ACCENT_RGB.load(Ordering::Relaxed);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

pub fn set_accent(rgb: [u8; 3]) {
    ACCENT_RGB.store(((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32, Ordering::Relaxed);
}

/// The highlight: active tool, selected row, slider fill, focus.
pub fn accent() -> Color32 {
    let [r, g, b] = accent_rgb();
    Color32::from_rgb(r, g, b)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// Brighter highlight for hover / pressed accents.
pub fn accent_hover() -> Color32 {
    mix(accent(), Color32::WHITE, 0.25)
}

/// The highlight at low strength over a card: selected rows and buttons.
pub fn accent_soft() -> Color32 {
    mix(PANEL, accent(), 0.30)
}

/// Text / icon colour that reads on top of the highlight.
pub fn on_accent() -> Color32 {
    let [r, g, b] = accent_rgb();
    let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    if lum > 150.0 {
        Color32::from_gray(18)
    } else {
        Color32::WHITE
    }
}

/// Translucent card body: the 3D viewport shows faintly through the windows laid over it.
pub fn card() -> Color32 {
    Color32::from_rgba_unmultiplied(27, 27, 27, 240)
}

/// Translucent tab bar / title strip of a card.
pub fn card_alt() -> Color32 {
    Color32::from_rgba_unmultiplied(21, 21, 21, 244)
}

/// Menu and status bars over the viewport.
pub fn bar() -> Color32 {
    Color32::from_rgba_unmultiplied(14, 14, 14, 232)
}

/// Semibold face for titles (falls back to the regular face when the system has none).
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

/// The window background as the linear colour of a wgpu clear.
pub fn clear_color() -> wgpu::Color {
    let lin = |c: u8| {
        let c = c as f64 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    wgpu::Color { r: lin(BG_VIEWPORT.r()), g: lin(BG_VIEWPORT.g()), b: lin(BG_VIEWPORT.b()), a: 1.0 }
}

/// Install Segoe UI (regular + semibold) ahead of egui's built-in faces when Windows provides them.
fn install_fonts(ctx: &egui::Context) {
    let dir = std::path::Path::new("C:/Windows/Fonts");
    let load = |name: &str| std::fs::read(dir.join(name)).ok();
    let (Some(regular), semibold_bytes) = (load("segoeui.ttf"), load("seguisb.ttf")) else { return };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("segoe".into(), std::sync::Arc::new(egui::FontData::from_owned(regular)));
    let builtin: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut prop = vec!["segoe".to_string()];
    prop.extend(builtin.iter().cloned());
    fonts.families.insert(FontFamily::Proportional, prop);
    let mut semi = Vec::new();
    if let Some(b) = semibold_bytes {
        fonts.font_data.insert("segoe-semibold".into(), std::sync::Arc::new(egui::FontData::from_owned(b)));
        semi.push("segoe-semibold".to_string());
    }
    semi.push("segoe".to_string());
    semi.extend(builtin);
    fonts.families.insert(FontFamily::Name("semibold".into()), semi);
    ctx.set_fonts(fonts);
}

/// Fonts, visuals and spacing (once per egui context).
pub fn apply(ctx: &egui::Context) {
    install_fonts(ctx);
    apply_visuals(ctx);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 5.0);
        s.spacing.button_padding = egui::vec2(9.0, 4.0);
        s.spacing.interact_size.y = 24.0;
        s.spacing.menu_margin = egui::Margin::same(6);
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(12.5));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(12.5));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(11.0));
        s.text_styles.insert(egui::TextStyle::Heading, semibold(15.0));
    });
}

/// Colours of egui's own widgets; call again after the highlight changed.
pub fn apply_visuals(ctx: &egui::Context) {
    let hl = accent();
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = WELL;
    v.faint_bg_color = PANEL_ALT;
    v.override_text_color = None;
    v.hyperlink_color = hl;
    v.warn_fg_color = WARNING;
    v.error_fg_color = ERROR;
    let [r, g, b] = accent_rgb();
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(r, g, b, 110);
    v.selection.stroke = Stroke::new(1.0, hl);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(RADIUS_CARD);
    v.menu_corner_radius = CornerRadius::same(8);
    v.popup_shadow = egui::epaint::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(140) };
    v.window_shadow = v.popup_shadow;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    let rad = CornerRadius::same(RADIUS);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.corner_radius = rad;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.bg_fill = WIDGET;
    v.widgets.inactive.weak_bg_fill = WIDGET;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.corner_radius = rad;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.bg_fill = WIDGET_HOVER;
    v.widgets.hovered.weak_bg_fill = WIDGET_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, g_(78));
    v.widgets.hovered.corner_radius = rad;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.active.bg_fill = WIDGET_PRESSED;
    v.widgets.active.weak_bg_fill = WIDGET_PRESSED;
    v.widgets.active.bg_stroke = Stroke::new(1.0, hl);
    v.widgets.active.corner_radius = rad;
    v.widgets.active.fg_stroke = Stroke::new(1.0, accent_hover());
    v.widgets.open.bg_fill = WIDGET_PRESSED;
    v.widgets.open.weak_bg_fill = WIDGET_PRESSED;
    v.widgets.open.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.open.corner_radius = rad;
    v.widgets.open.fg_stroke = Stroke::new(1.0, accent_hover());
    ctx.set_visuals(v);
    // scroll bars take their own room beside the content (the floating ones covered the values and reset buttons
    // at the right edge of the panels): a thin solid bar with a small gap
    ctx.style_mut(|s| {
        let mut sc = egui::style::ScrollStyle::solid();
        sc.bar_width = 6.0;
        sc.bar_inner_margin = 4.0;
        sc.bar_outer_margin = 0.0;
        s.spacing.scroll = sc;
    });
}

fn g_(v: u8) -> Color32 {
    g(v)
}
