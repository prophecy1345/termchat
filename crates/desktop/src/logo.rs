//! The termchat mark (docs/logo/termchat-mark-transparent.svg), drawn as
//! strokes so it stays sharp at any size and needs no image library.

use eframe::egui::{self, Color32, Pos2, Stroke, epaint::PathShape, pos2, vec2};

const MAUVE: Color32 = Color32::from_rgb(0xcb, 0xa6, 0xf7);
const TEAL: Color32 = Color32::from_rgb(0x94, 0xe2, 0xd5);

/// The window icon: the store icon at 64 px, as raw RGBA.
pub fn icon() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!("../assets/termchat-64.rgba").to_vec(),
        width: 64,
        height: 64,
    }
}

/// Draws the mark in a `size` square.
pub fn mark(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    // The SVG's own 1024 units, to this square.
    let at = |x: f32, y: f32| rect.min + vec2(x, y) * (size / 1024.0);
    let line = |colour| Stroke::new(64.0 * size / 1024.0, colour);
    let painter = ui.painter();

    // The shackle: down the left side, over the top, down the right.
    let mut shackle = vec![at(392.0, 354.0)];
    shackle.extend((0..=16).map(|i| {
        let t = std::f32::consts::PI * (1.0 + i as f32 / 16.0);
        at(512.0 + 120.0 * t.cos(), 290.0 + 120.0 * t.sin())
    }));
    shackle.push(at(632.0, 354.0));
    painter.add(PathShape::line(shackle, line(MAUVE)));

    // The speech bubble, its corners as quadratic curves.
    let mut bubble = vec![at(330.0, 354.0), at(694.0, 354.0)];
    quad(&mut bubble, at(792.0, 354.0), at(792.0, 452.0));
    bubble.push(at(792.0, 666.0));
    quad(&mut bubble, at(792.0, 764.0), at(694.0, 764.0));
    bubble.extend([at(440.0, 764.0), at(304.0, 850.0), at(304.0, 760.0)]);
    quad(&mut bubble, at(232.0, 748.0), at(232.0, 666.0));
    bubble.push(at(232.0, 452.0));
    quad(&mut bubble, at(232.0, 354.0), at(330.0, 354.0));
    painter.add(PathShape::closed_line(bubble, line(MAUVE)));

    // The prompt: ">" and "_".
    let prompt = vec![at(370.0, 500.0), at(450.0, 570.0), at(370.0, 640.0)];
    painter.add(PathShape::line(prompt, line(MAUVE)));
    painter.line_segment([at(538.0, 640.0), at(650.0, 640.0)], line(TEAL));
    // Round caps, as in the SVG.
    for (p, colour) in [(at(538.0, 640.0), TEAL), (at(650.0, 640.0), TEAL)] {
        painter.circle_filled(p, 32.0 * size / 1024.0, colour);
    }
}

/// Appends a quadratic curve from the last point through `c` to `end`.
fn quad(points: &mut Vec<Pos2>, c: Pos2, end: Pos2) {
    let start = points.last().copied().unwrap_or(c);
    points.extend((1..=8).map(|i| {
        let t = i as f32 / 8.0;
        let u = 1.0 - t;
        pos2(
            u * u * start.x + 2.0 * u * t * c.x + t * t * end.x,
            u * u * start.y + 2.0 * u * t * c.y + t * t * end.y,
        )
    }));
}
