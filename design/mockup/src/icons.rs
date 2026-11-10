//! Tiny vector icon set drawn with the egui painter (no icon font / image assets).

use eframe::egui;
use eframe::egui::{pos2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Brush,
    Eraser,
    Cube,
    PolyFill,
    Smudge,
    Clone,
    Picker,
    Select,
    Bake,
    Camera,
    Eye,
    EyeOff,
    Folder,
    ChevronRight,
    ChevronDown,
    ChevronUp,
    Reset,
    Menu,
    Plus,
    Trash,
    Wand,
    Mask,
    Bucket,
    Sphere,
    Close,
    Maximize,
    Export,
    Display,
    Shader,
    Environment,
    History,
    Assets,
    Pressure,
    Lazy,
    Symmetry,
    Pause,
    Alpha,
    Stencil,
    Material,
    Dropper,
    Anchor,
    /// Post-processing (tone mapping / exposure) switch of the viewport.
    PostFx,
    /// Keep a transient panel open (pushpin).
    Pin,
    /// Exchange two values (two opposite arrows).
    Swap,
}

/// Draw `icon` fitted into `r` (square-ish) with colour `c`.
pub fn draw(p: &Painter, r: Rect, icon: Icon, c: Color32) {
    let r = Rect::from_center_size(r.center(), egui::vec2(r.width().min(r.height()), r.width().min(r.height())));
    let u = |x: f32, y: f32| pos2(r.min.x + x * r.width(), r.min.y + y * r.height());
    let s = Stroke::new(1.5, c);
    let line = |a: Pos2, b: Pos2| {
        p.line_segment([a, b], s);
    };
    let poly = |pts: Vec<Pos2>| {
        p.add(Shape::closed_line(pts, s));
    };
    let rad = r.width();
    match icon {
        Icon::Brush => {
            line(u(0.82, 0.12), u(0.36, 0.62));
            p.circle_filled(u(0.27, 0.74), rad * 0.15, c);
            line(u(0.30, 0.52), u(0.50, 0.72));
        }
        Icon::PostFx => {
            // a large and a small sparkle
            let star = |cx: f32, cy: f32, k: f32| {
                line(u(cx, cy - k), u(cx, cy + k));
                line(u(cx - k, cy), u(cx + k, cy));
                line(u(cx - k * 0.45, cy - k * 0.45), u(cx + k * 0.45, cy + k * 0.45));
                line(u(cx - k * 0.45, cy + k * 0.45), u(cx + k * 0.45, cy - k * 0.45));
            };
            star(0.42, 0.58, 0.36);
            star(0.80, 0.22, 0.16);
        }
        Icon::Eraser => {
            poly(vec![u(0.15, 0.62), u(0.55, 0.18), u(0.88, 0.45), u(0.48, 0.88)]);
            line(u(0.33, 0.45), u(0.68, 0.70));
        }
        Icon::Cube => {
            poly(vec![u(0.5, 0.08), u(0.88, 0.28), u(0.88, 0.72), u(0.5, 0.92), u(0.12, 0.72), u(0.12, 0.28)]);
            line(u(0.5, 0.5), u(0.5, 0.92));
            line(u(0.5, 0.5), u(0.12, 0.28));
            line(u(0.5, 0.5), u(0.88, 0.28));
        }
        Icon::PolyFill => {
            poly(vec![u(0.2, 0.75), u(0.15, 0.3), u(0.55, 0.12), u(0.88, 0.45), u(0.7, 0.85)]);
            p.circle_filled(u(0.5, 0.52), rad * 0.1, c);
        }
        Icon::Smudge => {
            p.circle_stroke(u(0.35, 0.35), rad * 0.2, s);
            line(u(0.35, 0.55), u(0.55, 0.88));
            line(u(0.5, 0.45), u(0.88, 0.7));
        }
        Icon::Clone => {
            p.circle_stroke(u(0.5, 0.28), rad * 0.18, s);
            line(u(0.5, 0.46), u(0.5, 0.62));
            line(u(0.2, 0.62), u(0.8, 0.62));
            line(u(0.15, 0.82), u(0.85, 0.82));
        }
        Icon::Picker | Icon::Dropper => {
            line(u(0.85, 0.15), u(0.30, 0.70));
            line(u(0.72, 0.28), u(0.85, 0.15));
            line(u(0.30, 0.70), u(0.15, 0.88));
            p.circle_stroke(u(0.78, 0.22), rad * 0.1, s);
        }
        Icon::Select => {
            p.rect_stroke(
                Rect::from_min_max(u(0.15, 0.15), u(0.85, 0.85)),
                2.0,
                Stroke::new(1.0, c),
                egui::StrokeKind::Inside,
            );
            line(u(0.4, 0.4), u(0.8, 0.7));
        }
        Icon::Bake => {
            p.circle_stroke(u(0.5, 0.5), rad * 0.36, s);
            line(u(0.5, 0.5), u(0.5, 0.2));
            line(u(0.5, 0.5), u(0.7, 0.6));
        }
        Icon::Camera => {
            p.rect_stroke(
                Rect::from_min_max(u(0.12, 0.3), u(0.88, 0.8)),
                2.0,
                Stroke::new(1.0, c),
                egui::StrokeKind::Inside,
            );
            p.circle_stroke(u(0.5, 0.55), rad * 0.16, s);
        }
        Icon::Eye | Icon::EyeOff => {
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    u(0.08 + 0.84 * t, 0.5 - 0.28 * (std::f32::consts::PI * t).sin())
                })
                .chain((0..=12).rev().map(|i| {
                    let t = i as f32 / 12.0;
                    u(0.08 + 0.84 * t, 0.5 + 0.28 * (std::f32::consts::PI * t).sin())
                }))
                .collect();
            p.add(Shape::closed_line(pts, s));
            p.circle_filled(u(0.5, 0.5), rad * 0.12, c);
            if icon == Icon::EyeOff {
                line(u(0.15, 0.85), u(0.85, 0.15));
            }
        }
        Icon::Folder => {
            poly(vec![u(0.1, 0.25), u(0.4, 0.25), u(0.48, 0.35), u(0.9, 0.35), u(0.9, 0.8), u(0.1, 0.8)]);
        }
        Icon::ChevronRight => {
            line(u(0.38, 0.25), u(0.65, 0.5));
            line(u(0.65, 0.5), u(0.38, 0.75));
        }
        Icon::ChevronDown => {
            line(u(0.25, 0.38), u(0.5, 0.65));
            line(u(0.5, 0.65), u(0.75, 0.38));
        }
        Icon::ChevronUp => {
            line(u(0.25, 0.62), u(0.5, 0.35));
            line(u(0.5, 0.35), u(0.75, 0.62));
        }
        Icon::Reset => {
            let pts: Vec<Pos2> = (0..=18)
                .map(|i| {
                    let a = (40.0 + 290.0 * i as f32 / 18.0).to_radians();
                    u(0.5 + 0.34 * a.cos(), 0.5 - 0.34 * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, s));
            line(u(0.84, 0.28), u(0.84, 0.46));
            line(u(0.84, 0.28), u(0.66, 0.30));
        }
        Icon::Menu => {
            line(u(0.15, 0.3), u(0.85, 0.3));
            line(u(0.15, 0.5), u(0.85, 0.5));
            line(u(0.15, 0.7), u(0.55, 0.7));
            line(u(0.65, 0.66), u(0.75, 0.76));
            line(u(0.75, 0.76), u(0.85, 0.66));
        }
        Icon::Plus => {
            line(u(0.5, 0.2), u(0.5, 0.8));
            line(u(0.2, 0.5), u(0.8, 0.5));
        }
        Icon::Trash => {
            line(u(0.2, 0.28), u(0.8, 0.28));
            line(u(0.4, 0.28), u(0.4, 0.15));
            line(u(0.4, 0.15), u(0.6, 0.15));
            line(u(0.6, 0.15), u(0.6, 0.28));
            poly(vec![u(0.27, 0.28), u(0.73, 0.28), u(0.68, 0.88), u(0.32, 0.88)]);
        }
        Icon::Wand => {
            line(u(0.15, 0.85), u(0.6, 0.4));
            line(u(0.7, 0.15), u(0.7, 0.4));
            line(u(0.58, 0.28), u(0.82, 0.28));
            line(u(0.8, 0.6), u(0.8, 0.8));
            line(u(0.7, 0.7), u(0.9, 0.7));
        }
        Icon::Mask => {
            p.rect_stroke(
                Rect::from_min_max(u(0.12, 0.12), u(0.88, 0.88)),
                2.0,
                Stroke::new(1.0, c),
                egui::StrokeKind::Inside,
            );
            p.circle_filled(u(0.5, 0.5), rad * 0.22, c);
        }
        Icon::Bucket => {
            poly(vec![u(0.2, 0.4), u(0.5, 0.15), u(0.8, 0.45), u(0.5, 0.8)]);
            line(u(0.5, 0.15), u(0.5, 0.05));
            p.circle_filled(u(0.82, 0.78), rad * 0.08, c);
        }
        Icon::Sphere => {
            p.circle_stroke(u(0.5, 0.5), rad * 0.38, s);
            p.circle_filled(u(0.38, 0.38), rad * 0.07, c);
        }
        Icon::Close => {
            line(u(0.25, 0.25), u(0.75, 0.75));
            line(u(0.75, 0.25), u(0.25, 0.75));
        }
        Icon::Anchor => {
            p.circle_stroke(u(0.5, 0.2), rad * 0.1, s);
            line(u(0.5, 0.3), u(0.5, 0.86));
            line(u(0.3, 0.42), u(0.7, 0.42));
            line(u(0.2, 0.62), u(0.32, 0.78));
            line(u(0.32, 0.78), u(0.5, 0.86));
            line(u(0.8, 0.62), u(0.68, 0.78));
            line(u(0.68, 0.78), u(0.5, 0.86));
        }
        Icon::Maximize => {
            p.rect_stroke(
                Rect::from_min_max(u(0.22, 0.22), u(0.78, 0.78)),
                1.0,
                Stroke::new(1.0, c),
                egui::StrokeKind::Inside,
            );
        }
        Icon::Export => {
            line(u(0.5, 0.15), u(0.5, 0.6));
            line(u(0.5, 0.15), u(0.35, 0.3));
            line(u(0.5, 0.15), u(0.65, 0.3));
            poly(vec![u(0.18, 0.5), u(0.18, 0.88), u(0.82, 0.88), u(0.82, 0.5)]);
        }
        Icon::Display => {
            p.rect_stroke(
                Rect::from_min_max(u(0.12, 0.2), u(0.88, 0.72)),
                2.0,
                Stroke::new(1.0, c),
                egui::StrokeKind::Inside,
            );
            line(u(0.4, 0.88), u(0.6, 0.88));
        }
        Icon::Shader => {
            poly(vec![u(0.5, 0.1), u(0.9, 0.5), u(0.5, 0.9), u(0.1, 0.5)]);
        }
        Icon::Environment => {
            p.circle_stroke(u(0.5, 0.5), rad * 0.36, s);
            line(u(0.14, 0.5), u(0.86, 0.5));
            line(u(0.5, 0.14), u(0.5, 0.86));
        }
        Icon::History => {
            p.circle_stroke(u(0.5, 0.5), rad * 0.36, s);
            line(u(0.5, 0.5), u(0.5, 0.28));
            line(u(0.5, 0.5), u(0.66, 0.58));
        }
        Icon::Assets => {
            p.rect_stroke(
                Rect::from_min_max(u(0.15, 0.15), u(0.85, 0.85)),
                2.0,
                Stroke::new(1.0, c),
                egui::StrokeKind::Inside,
            );
            line(u(0.15, 0.5), u(0.85, 0.5));
            line(u(0.5, 0.15), u(0.5, 0.85));
        }
        Icon::Pressure => {
            let pts: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let t = i as f32 / 10.0;
                    u(0.15 + 0.7 * t, 0.8 - 0.55 * t * t)
                })
                .collect();
            p.add(Shape::line(pts, s));
        }
        Icon::Lazy => {
            p.circle_filled(u(0.5, 0.5), rad * 0.14, c);
            p.circle_stroke(u(0.5, 0.5), rad * 0.34, Stroke::new(1.0, c));
        }
        Icon::Symmetry => {
            line(u(0.5, 0.12), u(0.5, 0.88));
            poly(vec![u(0.4, 0.3), u(0.4, 0.7), u(0.15, 0.5)]);
            poly(vec![u(0.6, 0.3), u(0.6, 0.7), u(0.85, 0.5)]);
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_min_max(u(0.28, 0.22), u(0.42, 0.78)), 1.0, c);
            p.rect_filled(Rect::from_min_max(u(0.58, 0.22), u(0.72, 0.78)), 1.0, c);
        }
        Icon::Alpha => {
            p.circle_stroke(u(0.5, 0.5), rad * 0.34, s);
            p.circle_filled(u(0.5, 0.5), rad * 0.16, c);
        }
        Icon::Stencil => {
            p.rect_stroke(
                Rect::from_min_max(u(0.15, 0.15), u(0.85, 0.85)),
                1.0,
                Stroke::new(1.0, c),
                egui::StrokeKind::Inside,
            );
            p.circle_filled(u(0.5, 0.5), rad * 0.18, c);
        }
        Icon::Material => {
            p.circle_stroke(u(0.5, 0.5), rad * 0.38, s);
            p.circle_filled(u(0.62, 0.62), rad * 0.14, c);
        }
        Icon::Pin => {
            poly(vec![u(0.38, 0.12), u(0.62, 0.12), u(0.58, 0.40), u(0.74, 0.56), u(0.26, 0.56), u(0.42, 0.40)]);
            line(u(0.5, 0.56), u(0.5, 0.90));
        }
        Icon::Swap => {
            line(u(0.15, 0.35), u(0.85, 0.35));
            line(u(0.85, 0.35), u(0.68, 0.18));
            line(u(0.85, 0.65), u(0.15, 0.65));
            line(u(0.15, 0.65), u(0.32, 0.82));
        }
    }
}
