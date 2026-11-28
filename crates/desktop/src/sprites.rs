//! Pixel sprites: the shared icons (engine `icons`), fine and flat, and the
//! reactions, glowing a little like an arcade screen.

use eframe::egui::{self, Color32, Stroke};
use termchat_engine::contact_icons::{self, ContactIcon};
use termchat_engine::icons::{self, Icon};

use crate::chat::Pal;

/// Lit pixels at `origin`, `px` points each, over a soft glow if `glow`.
pub(crate) fn paint(
    painter: &egui::Painter,
    origin: egui::Pos2,
    px: f32,
    pixels: impl Iterator<Item = (u8, u8)> + Clone,
    colour: Color32,
    glow: bool,
) {
    // Whole screen pixels, so a sprite stays sharp at any scale.
    let ppp = painter.ctx().pixels_per_point();
    let px = (px * ppp).round().max(1.0) / ppp;
    let origin = (origin.to_vec2() * ppp).round().to_pos2() / ppp;
    let cell = |x: u8, y: u8, grow: f32| {
        let at = origin + egui::vec2(f32::from(x) * px, f32::from(y) * px);
        egui::Rect::from_min_size(at, egui::vec2(px, px)).expand(grow)
    };
    for (x, y) in pixels.clone().filter(|_| glow) {
        painter.rect_filled(cell(x, y, px * 0.6), 0, colour.gamma_multiply(0.18));
    }
    // A mesh, not rects: egui feathers a rect's edges, which all but erases
    // a one-pixel line.
    let mut mesh = egui::Mesh::default();
    for (x, y) in pixels {
        mesh.add_colored_rect(cell(x, y, 0.0), colour);
    }
    painter.add(mesh);
}

/// A contact icon (engine `contact_icons`) filling the square `rect`.
pub(crate) fn contact_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    icon: ContactIcon,
    colour: Color32,
) {
    let scale = rect.width() / contact_icons::GRID;
    let at = |(x, y): (f32, f32)| rect.min + egui::vec2(x, y) * scale;
    let width = contact_icons::STROKE * scale;
    let d = contact_icons::drawing(icon);
    for line in &d.lines {
        let points: Vec<_> = line.iter().copied().map(at).collect();
        // Round caps: egui ends a line square.
        for end in [points.first(), points.last()].into_iter().flatten() {
            painter.circle_filled(*end, width / 2.0, colour);
        }
        painter.add(egui::Shape::line(points, Stroke::new(width, colour)));
    }
    for &dot in &d.dots {
        painter.circle_filled(at(dot), contact_icons::DOT * scale, colour);
    }
}

/// An icon in the flow, `px` points per pixel.
pub(crate) fn icon(ui: &mut egui::Ui, icon: Icon, px: f32, colour: Color32) -> egui::Response {
    let side = f32::from(icons::SIDE) * px;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    let painter = ui.painter_at(rect.expand(px));
    paint(&painter, rect.min, px, icons::pixels(icon), colour, false);
    response
}

/// An icon that is a button: lit and framed under the pointer, named for
/// screen readers and in its tooltip.
pub(crate) fn button(ui: &mut egui::Ui, p: &Pal, icon: Icon, hint: &str) -> egui::Response {
    let px = 1.0;
    let pad = 6.0;
    let side = f32::from(icons::SIDE) * px + 2.0 * pad;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, hint));
    let hot = response.hovered() || response.has_focus();
    let colour = if hot { p.accent } else { p.text };
    let painter = ui.painter_at(rect.expand(2.0));
    if hot {
        painter.rect_stroke(rect, 2, Stroke::new(1.0, colour), egui::StrokeKind::Inside);
    }
    let origin = rect.min + egui::vec2(pad, pad);
    paint(&painter, origin, px, icons::pixels(icon), colour, false);
    response.on_hover_text(hint)
}

/// An icon, then what `after` draws: in reading order even where the
/// layout runs right to left, as in a bubble of one's own.
pub(crate) fn lead<R>(
    ui: &mut egui::Ui,
    icon: Icon,
    px: f32,
    colour: Color32,
    after: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if ui.layout().main_dir() == egui::Direction::RightToLeft {
            let r = after(ui);
            self::icon(ui, icon, px, colour);
            r
        } else {
            self::icon(ui, icon, px, colour);
            after(ui)
        }
    })
    .inner
}

/// An icon before a line of text, both in `colour`.
pub(crate) fn labelled(ui: &mut egui::Ui, icon: Icon, text: egui::RichText, colour: Color32) {
    lead(ui, icon, 1.0, colour, |ui| ui.label(text.color(colour)));
}

/// The fold mark of a collapsing header, as our chevron sprite.
pub(crate) fn fold_icon(ui: &mut egui::Ui, openness: f32, response: &egui::Response) {
    let icon = if openness > 0.5 {
        Icon::Unfolded
    } else {
        Icon::Folded
    };
    let colour = ui.style().interact(response).fg_stroke.color;
    let px = 1.0;
    let side = f32::from(icons::SIDE) * px;
    let origin = response.rect.center() - egui::vec2(side, side) / 2.0;
    paint(ui.painter(), origin, px, icons::pixels(icon), colour, false);
}
