//! The quiet 1980s synthwave the site promises (site/src/styles/site.css): a
//! violet glow from the top, a neon grid floor, scanlines, gradient buttons.
//! Colours come from the theme, so every theme gets its own version.

use eframe::egui::{self, Color32, Mesh, Rect, Stroke, pos2, vec2};

use crate::chat::Pal;

/// `a` moved towards `b` by `t` (0 to 1).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// A gradient over `rect`: `a` to `b`, top to bottom or left to right.
fn gradient(painter: &egui::Painter, rect: Rect, a: Color32, b: Color32, across: bool) {
    let (tl, tr, bl, br) = match across {
        true => (a, b, a, b),
        false => (a, a, b, b),
    };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), tl);
    mesh.colored_vertex(rect.right_top(), tr);
    mesh.colored_vertex(rect.left_bottom(), bl);
    mesh.colored_vertex(rect.right_bottom(), br);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    painter.add(mesh);
}

/// The background: the theme's base with a violet glow from the top and,
/// with `floor`, the neon grid at the bottom.
pub fn backdrop(ui: &egui::Ui, p: &Pal, rect: Rect, floor: bool) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0, p.bg);
    let top = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.min.y + rect.height() * 0.6));
    gradient(&painter, top, mix(p.bg, p.mauve, 0.16), p.bg, false);
    if !floor {
        return;
    }
    // A floor in perspective: rows closer together towards the horizon,
    // columns meeting at its middle; both fade into the distance.
    // Low enough to stay under centred content in a small window.
    let depth = (rect.height() * 0.24).min(200.0);
    let horizon = rect.max.y - depth;
    for i in 1..=12 {
        let t = (i as f32 / 12.0).powi(2);
        let y = horizon + depth * t;
        let line = Stroke::new(1.0, p.mauve.gamma_multiply(0.45 * t));
        painter.line_segment([pos2(rect.min.x, y), pos2(rect.max.x, y)], line);
    }
    let middle = rect.center().x;
    for i in -16..=16 {
        let x = i as f32;
        let line = Stroke::new(1.0, p.pink.gamma_multiply(0.22));
        let far = pos2(middle + x * 14.0, horizon);
        let near = pos2(middle + x * 110.0, rect.max.y);
        painter.line_segment([far, near], line);
    }
}

/// Faint scanlines over the whole window, like the site's.
pub fn scanlines(ctx: &egui::Context) {
    let rect = ctx.content_rect();
    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("scanlines"));
    let painter = ctx.layer_painter(layer);
    let line = Stroke::new(1.0, Color32::from_black_alpha(22));
    let mut y = rect.min.y + 1.0;
    while y < rect.max.y {
        painter.line_segment([pos2(rect.min.x, y), pos2(rect.max.x, y)], line);
        y += 3.0;
    }
}

/// The main action: a mauve-to-pink gradient with dark text and a faint glow.
pub fn button(ui: &mut egui::Ui, p: &Pal, text: &str, size: egui::Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), text));
    let lift = match (response.is_pointer_button_down_on(), response.hovered()) {
        (true, _) => -0.1,
        (false, true) => 0.15,
        (false, false) => 0.0,
    };
    let shade = |c: Color32| match lift > 0.0 {
        true => mix(c, Color32::WHITE, lift),
        false => mix(c, Color32::BLACK, -lift),
    };
    let painter = ui.painter();
    painter.rect_stroke(
        rect.expand(2.0),
        4,
        Stroke::new(1.0, p.mauve.gamma_multiply(0.35)),
        egui::StrokeKind::Outside,
    );
    gradient(painter, rect, shade(p.mauve), shade(p.pink), true);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(15.0),
        p.bg,
    );
    if response.has_focus() {
        painter.rect_stroke(
            rect.expand(4.0),
            4,
            Stroke::new(1.0, p.text),
            egui::StrokeKind::Outside,
        );
    }
    response
}

/// `text` as a QR code in a `size` square, black on white so phones read it
/// (the Android app scans invites this way).
pub fn qr(ui: &mut egui::Ui, text: &str, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4, Color32::WHITE);
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else {
        return;
    };
    let n = code.width();
    // Four modules of quiet zone on each side, as the standard asks.
    let module = size / (n + 8) as f32;
    for (i, colour) in code.to_colors().iter().enumerate() {
        if *colour == qrcode::Color::Dark {
            let x = rect.min.x + module * (4 + i % n) as f32;
            let y = rect.min.y + module * (4 + i / n) as f32;
            let cell = Rect::from_min_size(pos2(x, y), vec2(module, module));
            painter.rect_filled(cell.expand(0.3), 0, Color32::BLACK);
        }
    }
}
