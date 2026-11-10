//! ShardStroke's look (the UI kit of design/shardstroke_ui.figcraft, as built in design/mockup): neutral greys,
//! square shapes, 2 px window borders, the Silkscreen pixel font for all text, and one colour, the highlight,
//! which the user picks in Settings (`set_accent`). Animations follow the level set in Settings.

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke};

const fn g(v: u8) -> Color32 {
    Color32::from_rgb(v, v, v)
}

// ---- surfaces (neutral)
/// Window background: the 3D viewport sits on it and shows in the gaps between the cards.
pub const BG_VIEWPORT: Color32 = g(14);
/// Window body (docked panels, popups, floating windows).
pub const PANEL: Color32 = g(21);
/// Title strips and headers, a step above the body.
pub const PANEL_ALT: Color32 = g(27);
pub const BORDER: Color32 = g(46);
/// Borders of controls (buttons, fields, segmented).
pub const BORDER_STRONG: Color32 = g(58);
/// Sunken areas: lists, thumbnails, text fields.
pub const WELL: Color32 = g(14);
/// Selected row.
pub const SELECTED: Color32 = g(42);

// ---- controls (neutral)
pub const WIDGET: Color32 = g(40);
/// Row hover; buttons invert to light instead (see `apply_visuals`).
pub const WIDGET_HOVER: Color32 = g(38);
pub const WIDGET_PRESSED: Color32 = g(32);
/// Empty part of a block bar.
pub const SLIDER_TRACK: Color32 = g(30);
pub const SLIDER_TRACK_OFF: Color32 = g(24);
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
/// Radius of dock windows and floating panels: square.
pub const RADIUS_CARD: u8 = 0;
/// Radius of buttons, fields, rows: square.
pub const RADIUS: u8 = 0;
/// Gap between the cards.
pub const GAP: f32 = 4.0;
/// Border of windows (panels, popups, menus, dialogs); controls keep 1 px.
pub const WINDOW_STROKE: f32 = 2.0;

// ---- the highlight colour (editable in Settings)

/// Highlight presets offered in Settings; the first one is the default (the kit's purple).
pub const ACCENT_PRESETS: [(&str, [u8; 3]); 5] = [
    ("Purple", [108, 35, 220]),
    ("Green", [124, 204, 74]),
    ("Blue", [77, 140, 255]),
    ("Teal", [48, 190, 170]),
    ("Orange", [255, 150, 50]),
];

static ACCENT_RGB: AtomicU32 = AtomicU32::new(0x6C_23_DC);

// ---- animation level (Settings > Appearance)

/// 0 off, 1 basic (fades, glows, sliding highlights), 2 advanced (also the heavier effects).
static ANIMATION: AtomicU8 = AtomicU8::new(2);

pub fn set_animation(level: u8) {
    ANIMATION.store(level.min(2), Ordering::Relaxed);
}

pub fn animation() -> u8 {
    ANIMATION.load(Ordering::Relaxed)
}

/// Seconds for an animation of `t` seconds at the current level (0 when animations are off).
pub fn anim(t: f32) -> f32 {
    if animation() == 0 { 0.0 } else { t }
}

/// The heavier effects (rolling text, ASCII rain / dissolve, moving stripes) run only at the advanced level.
pub fn heavy() -> bool {
    animation() == 2
}

/// Soft glow around `r` (a blurred shadow with no offset), the kit's mark of active / primary items.
pub fn glow(p: &egui::Painter, r: egui::Rect, c: Color32, alpha: f32, blur: u8) {
    if alpha > 0.004 {
        let col = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (alpha.clamp(0.0, 1.0) * 255.0) as u8);
        // egui centres the blur on the edge: doubling it and spreading by half matches the design file
        p.add(egui::epaint::Shadow { offset: [0, 0], blur: blur.saturating_mul(2), spread: blur / 2, color: col }.as_shape(r, 0.0));
    }
}

pub fn accent_rgb() -> [u8; 3] {
    let v = ACCENT_RGB.load(Ordering::Relaxed);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

pub fn set_accent(rgb: [u8; 3]) {
    ACCENT_RGB.store(((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32, Ordering::Relaxed);
}

/// Outline around the picked layer / mask thumbnail (Settings > Appearance).
static SELECTION_RGB: AtomicU32 = AtomicU32::new(0xE2_E2_E2);

pub fn set_selection_outline(rgb: [u8; 3]) {
    SELECTION_RGB.store(((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32, Ordering::Relaxed);
}

pub fn selection_outline() -> Color32 {
    let v = SELECTION_RGB.load(Ordering::Relaxed);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
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

/// Silkscreen (bundled, SIL Open Font License, see assets/fonts) for all text; Segoe UI and egui's own faces
/// only fill in glyphs the pixel font lacks.
fn install_fonts(ctx: &egui::Context) {
    use std::sync::Arc;
    let mut fonts = egui::FontDefinitions::default();
    let font = |b: &'static [u8]| Arc::new(egui::FontData::from_static(b));
    fonts.font_data.insert("silk".into(), font(include_bytes!("../assets/fonts/Silkscreen-Regular.ttf")));
    let mut fallback = Vec::new();
    if let Ok(b) = std::fs::read("C:/Windows/Fonts/segoeui.ttf") {
        fonts.font_data.insert("segoe".into(), Arc::new(egui::FontData::from_owned(b)));
        fallback.push("segoe".to_string());
    }
    for fam in [FontFamily::Proportional, FontFamily::Monospace] {
        let builtin = fonts.families.get(&fam).cloned().unwrap_or_default();
        let mut list = vec!["silk".to_string()];
        list.extend(fallback.iter().cloned());
        list.extend(builtin);
        fonts.families.insert(fam, list);
    }
    // titles use the regular face too: the pixel font's bold cut reads badly (user decision)
    let semi = fonts.families[&FontFamily::Proportional].clone();
    // the app's name on the start screen is the one text not in the pixel font (JetBrains Mono, OFL)
    fonts.font_data.insert("jetbrains".into(), font(include_bytes!("../assets/fonts/JetBrainsMono.ttf")));
    let mut title = vec!["jetbrains".to_string()];
    title.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts.families.insert(FontFamily::Name("title".into()), title);
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
        // the pixel font reads at smaller sizes than a regular face
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(10.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(10.0));
        s.text_styles.insert(egui::TextStyle::Small, FontId::proportional(8.0));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::monospace(10.0));
        s.text_styles.insert(egui::TextStyle::Heading, semibold(12.0));
    });
    apply_animation(ctx);
}

/// egui's own animations (collapsing headers, scroll, tooltips) follow the level too.
pub fn apply_animation(ctx: &egui::Context) {
    ctx.style_mut(|s| s.animation_time = anim(1.0 / 8.0));
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
    // text on a selected item (combo lists, selectable rows) reads in white over the highlight
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    v.window_stroke = Stroke::new(WINDOW_STROKE, BORDER);
    v.window_corner_radius = CornerRadius::same(RADIUS_CARD);
    v.menu_corner_radius = CornerRadius::same(RADIUS);
    v.window_shadow = egui::epaint::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(140) };
    // popups, menus and tooltips: a hard pixel drop shadow, like the kit's blocks
    v.popup_shadow = egui::epaint::Shadow { offset: [3, 3], blur: 0, spread: 0, color: Color32::from_black_alpha(200) };
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };
    let rad = CornerRadius::same(RADIUS);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.weak_bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.corner_radius = rad;
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.bg_fill = WIDGET;
    v.widgets.inactive.weak_bg_fill = WIDGET;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
    v.widgets.inactive.corner_radius = rad;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    // hover inverts: light block, dark text
    v.widgets.hovered.bg_fill = TEXT;
    v.widgets.hovered.weak_bg_fill = TEXT;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.corner_radius = rad;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, BG_VIEWPORT);
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
        // tooltips: quick, narrow enough to read as a note, roomy padding
        s.interaction.tooltip_delay = 0.35;
        s.spacing.tooltip_width = 280.0;
        s.spacing.menu_margin = egui::Margin::same(8);
    });
}

