//! ADR-044: a received image, opened on a click. The engine's Rust decoder
//! runs on a worker thread, never the interface's; the pixels go straight
//! into a texture and the decoded buffer is wiped. Nothing is written to
//! disk or the clipboard.

use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, RichText, Sense, TextureOptions, Vec2};
use termchat_engine::picture::{Picture, decode};

use crate::chat::{Chat, Pal};
use crate::dialog::Dialog;

pub struct Viewer {
    /// The message, in the open chat or group.
    m: usize,
    name: String,
    coming: Option<Receiver<Result<Picture, String>>>,
    texture: Option<egui::TextureHandle>,
    error: String,
    zoom: f32,
    pan: Vec2,
}

impl Viewer {
    /// Starts decoding `bytes` of message `m` off the interface thread.
    pub fn open(m: usize, name: String, bytes: Vec<u8>) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(decode(&bytes));
        });
        Self {
            m,
            name,
            coming: Some(rx),
            texture: None,
            error: String::new(),
            zoom: 1.0,
            pan: Vec2::ZERO,
        }
    }

    fn take(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.coming else { return };
        let got = match rx.try_recv() {
            Ok(got) => got,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("this image cannot be shown".into()),
        };
        self.coming = None;
        match got {
            Ok(p) => {
                let size = [p.width as usize, p.height as usize];
                let image = ColorImage::from_rgba_unmultiplied(size, &p.rgba);
                self.texture = Some(ctx.load_texture("viewer", image, TextureOptions::LINEAR));
                // `p` is dropped here, and its buffer wiped.
            }
            Err(e) => self.error = e,
        }
    }
}

impl Chat {
    /// The open image, over everything. Esc or Close shuts it.
    pub fn viewer(&mut self, ctx: &egui::Context, p: &Pal) {
        let Some(mut v) = self.viewer.take() else {
            return;
        };
        v.take(ctx);
        if v.coming.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let screen = ctx.content_rect();
        let mut keep = true;
        let r = egui::Modal::new(egui::Id::unique("viewer")).show(ctx, |ui| {
            let size = screen.size() * 0.9;
            ui.set_min_size(size);
            ui.set_max_size(size);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&v.name).monospace());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    keep &= !ui.button("Close").clicked();
                    if ui.button("Save…").clicked() {
                        self.dialog = Some(Dialog::save_file(v.m));
                        keep = false;
                    }
                });
            });
            let (area, response) = ui.allocate_exact_size(ui.available_size(), Sense::drag());
            ui.painter().rect_filled(area, 0.0, Color32::BLACK);
            match &v.texture {
                Some(t) => {
                    if response.hovered() {
                        let (scroll, pinch) =
                            ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
                        v.zoom = (v.zoom * pinch * (1.0 + scroll / 400.0)).clamp(1.0, 8.0);
                    }
                    v.pan += response.drag_delta();
                    if v.zoom == 1.0 {
                        v.pan = Vec2::ZERO;
                    }
                    let tex = t.size_vec2();
                    let fit = (area.width() / tex.x).min(area.height() / tex.y).min(1.0);
                    let shown = tex * fit * v.zoom;
                    let rect = Rect::from_center_size(area.center() + v.pan, shown);
                    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                    ui.painter()
                        .with_clip_rect(area)
                        .image(t.id(), rect, uv, Color32::WHITE);
                }
                None => {
                    let text = if v.error.is_empty() {
                        "…"
                    } else {
                        v.error.as_str()
                    };
                    let color = if v.error.is_empty() { p.dim } else { p.warn };
                    ui.painter().text(
                        area.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::monospace(14.0),
                        color,
                    );
                }
            }
        });
        if keep && !r.should_close() {
            self.viewer = Some(v);
        }
    }
}
