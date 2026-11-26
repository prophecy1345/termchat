//! ADR-048 display and composer controls for chat messages.

use std::ops::Range;
use std::sync::Arc;

use eframe::egui::{self, FontId, Stroke, TextBuffer, TextFormat};
use egui::text::{CCursor, CCursorRange, CharIndex, LayoutJob};
use termchat_engine::icons::Icon;
use termchat_engine::markdown::{Block, Span, parse};

use crate::{chat::Pal, sprites};

const BOLD_FONT: &str = "Hack Bold";

fn bold_family() -> egui::FontFamily {
    egui::FontFamily::Name(BOLD_FONT.into())
}

pub fn add_bold_font(fonts: &mut egui::FontDefinitions) {
    // Hack v3.003, matching egui's bundled monospace face. See LICENSE.Hack.txt.
    let data = egui::FontData::from_static(include_bytes!("../assets/Hack-Bold.ttf"));
    fonts.font_data.insert(BOLD_FONT.into(), Arc::new(data));
    let mut family = fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .clone();
    family.insert(0, BOLD_FONT.into());
    fonts.families.insert(bold_family(), family);
}

pub fn toolbar(ui: &mut egui::Ui, p: &Pal) -> Option<Icon> {
    ui.horizontal_wrapped(|ui| {
        let mut picked = None;
        for (icon, hint) in [
            (Icon::Bold, "Bold"),
            (Icon::Italic, "Italic"),
            (Icon::Strike, "Strikethrough"),
            (Icon::Code, "Inline code"),
            (Icon::CodeBlock, "Code block"),
            (Icon::Link, "Link"),
            (Icon::Quote, "Quote"),
            (Icon::List, "List"),
        ] {
            if sprites::button(ui, p, icon, hint).clicked() {
                picked = Some(icon);
            }
        }
        picked
    })
    .inner
}

/// Keep the cursor (which egui counts in characters) inside new markup.
pub fn insert(text: &mut String, selection: Option<CCursorRange>, icon: Icon) -> CCursor {
    let range = selection
        .unwrap_or_else(|| CCursorRange::one(CCursor::end_of_str(text)))
        .as_sorted_char_range();
    let start = text.byte_index_from_char_index(range.start).0;
    let end = text.byte_index_from_char_index(range.end).0;
    let selected = &text[start..end];
    let (before, after) = match icon {
        Icon::Bold => ("**", "**"),
        Icon::Italic => ("*", "*"),
        Icon::Strike => ("~~", "~~"),
        Icon::Code => ("`", "`"),
        Icon::Link if start == end => ("[", "]()"),
        Icon::Link => {
            text.insert_str(end, "]()");
            text.insert(start, '[');
            return CCursor::new(text[..end + 3].chars().count());
        }
        Icon::CodeBlock => (
            if start == 0 || text[..start].ends_with('\n') {
                "```\n"
            } else {
                "\n```\n"
            },
            if end == text.len() || text[end..].starts_with('\n') {
                "\n```"
            } else {
                "\n```\n"
            },
        ),
        Icon::Quote | Icon::List => {
            let prefix = if icon == Icon::Quote { "> " } else { "- " };
            let line = text[..start].rfind('\n').map_or(0, |n| n + 1);
            let mut replacement = prefix.to_owned();
            let mut lines = text[line..end].split_inclusive('\n').peekable();
            while let Some(chunk) = lines.next() {
                replacement.push_str(chunk);
                if lines.peek().is_some() {
                    replacement.push_str(prefix);
                }
            }
            let cursor = line + replacement.len();
            text.replace_range(line..end, &replacement);
            return CCursor::new(text[..cursor].chars().count());
        }
        _ => return CCursor::new(text[..end].chars().count()),
    };
    let cursor = text[..end].chars().count() + before.chars().count();
    // The end marker goes after the selection, or below its last line in a fence.
    let after = if icon == Icon::CodeBlock && selected.ends_with('\n') {
        &after[1..]
    } else {
        after
    };
    text.insert_str(end, after);
    text.insert_str(start, before);
    CCursor::new(cursor)
}

pub struct Link {
    address: String,
    rects: Vec<egui::Rect>,
}

/// Paint the engine's display blocks and return the link hit areas.
pub fn show(ui: &mut egui::Ui, p: &Pal, text: &str, quote: bool) -> Vec<Link> {
    let text_style = egui::TextStyle::Monospace.resolve(ui.style());
    let normal = TextFormat::simple(text_style.clone(), if quote { p.dim } else { p.text });
    let small = egui::TextStyle::Small.resolve(ui.style());
    let Some(blocks) = parse(text) else {
        let text = termchat_engine::engine::clean_text(text);
        let text = if quote {
            text.lines().next().unwrap_or_default()
        } else {
            &text
        };
        ui.label(egui::RichText::new(text).monospace().color(normal.color));
        return Vec::new();
    };
    let mut links = Vec::new();
    for (i, block) in blocks.into_iter().enumerate() {
        if quote && i != 0 {
            break;
        }
        let (prefix, spans) = match block {
            Block::Line { spans } => (String::new(), spans),
            Block::Quote { spans } => ("│ ".into(), spans),
            Block::Bullet { spans } => ("• ".into(), spans),
            Block::Numbered { number, spans } => (format!("{number}. "), spans),
            Block::Code { text } => {
                let text = if quote {
                    text.lines().next().unwrap_or_default()
                } else {
                    &text
                };
                egui::ScrollArea::horizontal()
                    .id_salt(("message code", i))
                    .auto_shrink([true, true])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .fill(p.surface)
                            .inner_margin(4)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(egui::RichText::new(text).monospace())
                                        .extend()
                                        .selectable(false),
                                );
                            });
                    });
                continue;
            }
        };
        let mut job = LayoutJob::default();
        let mut ranges = Vec::new();
        let mut chars = prefix.chars().count();
        if !prefix.is_empty() {
            job.append(&prefix, 0.0, TextFormat::simple(text_style.clone(), p.dim));
        }
        for span in spans {
            let mut style = normal.clone();
            let text = match span {
                Span::Plain { text } => text,
                Span::Bold { text } => {
                    style.font_id = FontId::new(text_style.size, bold_family());
                    text
                }
                Span::Italic { text } => {
                    style.italics = true;
                    text
                }
                Span::Strike { text } => {
                    style.strikethrough = Stroke::new(1.0, style.color);
                    text
                }
                Span::Code { text } => {
                    style.background = p.surface;
                    text
                }
                Span::Link { label, address } => {
                    style.color = p.accent;
                    style.underline = Stroke::new(1.0, p.accent);
                    job.append(&label, 0.0, style);
                    let shown = format!(" <{address}>");
                    job.append(&shown, 0.0, TextFormat::simple(small.clone(), p.dim));
                    let end = chars + label.chars().count() + shown.chars().count();
                    ranges.push((
                        chars..end,
                        Link {
                            address,
                            rects: Vec::new(),
                        },
                    ));
                    chars = end;
                    continue;
                }
            };
            chars += text.chars().count();
            job.append(&text, 0.0, style);
        }
        if job.text.is_empty() {
            job.append(" ", 0.0, normal.clone());
        }
        job.wrap.max_width = ui.available_width();
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let response = ui.add(egui::Label::new(galley.clone()).selectable(false));
        link_rects(&galley, response.rect.min, &mut ranges);
        links.extend(ranges.into_iter().map(|(_, link)| link));
    }
    links
}

fn link_rects(galley: &egui::Galley, origin: egui::Pos2, links: &mut [(Range<usize>, Link)]) {
    let (mut start, mut next) = (0, 0);
    for row in galley.rows.iter() {
        let end = start + row.glyphs.len();
        while let Some((range, link)) = links.get_mut(next) {
            if range.start >= end {
                break;
            }
            let row_rect = row.rect().translate(origin.to_vec2());
            let x = |at: usize| row_rect.min.x + row.x_offset(CharIndex(at - start));
            let r = egui::Rect::from_min_max(
                egui::pos2(x(range.start.max(start)), row_rect.min.y),
                egui::pos2(x(range.end.min(end)), row_rect.max.y),
            );
            if r.is_positive() {
                link.rects.push(r);
            }
            if range.end > end {
                break;
            }
            next += 1;
        }
        start = end + usize::from(row.ends_with_newline);
    }
}

/// Register after the bubble so an address click is not a bubble double-click.
pub fn copy_link(ui: &mut egui::Ui, id: egui::Id, links: Vec<Link>) -> bool {
    for (i, link) in links.into_iter().enumerate() {
        // An edit between mouse-down and mouse-up must not retarget the click.
        let link_id = id.with((i, &link.address));
        for (n, rect) in link.rects.into_iter().enumerate() {
            let sense = if n == 0 {
                egui::Sense::click()
            } else {
                egui::Sense::click() - egui::Sense::FOCUSABLE
            };
            let response = ui
                .interact(rect, link_id.with(n), sense)
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Copy address");
            if n == 0 {
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::Button,
                        ui.is_enabled(),
                        format!("Copy address {}", link.address),
                    )
                });
            }
            if response.clicked() {
                ui.ctx().copy_text(link.address);
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_wraps_unicode_and_positions_the_caret() {
        let mut text = "до 🦔 после".to_string();
        let selected = Some(CCursorRange::two(CCursor::new(3), CCursor::new(4)));
        let cursor = insert(&mut text, selected, Icon::Bold);
        assert_eq!(text, "до **🦔** после");
        assert_eq!(cursor.index, CharIndex(6));

        let mut text = "сайт".to_string();
        let selected = Some(CCursorRange::select_all_str(&text));
        let cursor = insert(&mut text, selected, Icon::Link);
        assert_eq!(text, "[сайт]()");
        assert_eq!(cursor.index, CharIndex(7));

        let mut text = "a\nb\nc".to_string();
        let selected = Some(CCursorRange::select_all_str(&text));
        insert(&mut text, selected, Icon::List);
        assert_eq!(text, "- a\n- b\n- c");

        let mut text = "a\nb".to_string();
        let cursor = insert(
            &mut text,
            Some(CCursorRange::one(CCursor::new(1))),
            Icon::CodeBlock,
        );
        assert_eq!(text, "a\n```\n\n```\nb");
        assert_eq!(cursor.index, CharIndex(6));
    }

    #[test]
    fn wrapped_link_copies_the_entire_address_over_the_chat_bubble() {
        let ctx = egui::Context::default();
        Pal::fonts(&ctx);
        let pal = Pal::named("termchat");
        let address =
            "https://example.org/a-long-path-that-wraps-on-a-narrow-bubble?query=значение";
        let text = format!("🦔 **жирный** [ссылка]({address})");
        let edited = text.replace("example.org", "example.net");
        let source = std::cell::Cell::new(text.as_str());
        let point = std::cell::Cell::new(None);
        let draw = |ui: &mut egui::Ui| {
            ui.set_max_width(132.0);
            let bubble = egui::Frame::new().show(ui, |ui| show(ui, &pal, source.get(), false));
            let bubble_click = bubble.response.interact(egui::Sense::click()).clicked();
            let links = bubble.inner;
            assert_eq!(links.len(), 1);
            assert!(links[0].rects.len() > 1);
            point.set(links[0].rects.last().map(|r| r.center()));
            let id = ui.scope_id().with("test links");
            assert!(!bubble_click);
            copy_link(ui, id, links);
        };
        let run = |events| {
            let mut out = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                draw,
            );
            out.textures_delta.clear();
            out
        };
        let _ = run(Vec::new());
        let Some(at) = point.get() else {
            panic!("link had no hit area")
        };
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = run(vec![egui::Event::PointerMoved(at), button(true)]);
        let out = run(vec![button(false)]);
        assert!(matches!(out.platform_output.commands.as_slice(),
            [egui::OutputCommand::CopyText(copied)] if copied == address));

        let _ = run(vec![button(true)]);
        source.set(&edited);
        let _ = run(Vec::new());
        let out = run(vec![button(false)]);
        assert!(
            out.platform_output.commands.is_empty(),
            "the address changed during the click"
        );
    }

    #[test]
    fn code_scrolls_inside_a_narrow_message() {
        let ctx = egui::Context::default();
        let pal = Pal::named("termchat");
        let line = "unwrapped_code".repeat(20);
        let text = format!("```\n{line}\n```");
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.set_max_width(132.0);
            let bubble = egui::Frame::new().show(ui, |ui| show(ui, &pal, &text, false));
            assert!(bubble.inner.is_empty());
            assert!(bubble.response.rect.width() <= 132.0);
        });
        out.textures_delta.clear();
        assert!(out.shapes.iter().any(|shape| {
            matches!(&shape.shape, egui::Shape::Text(t)
                if t.galley.text() == line && t.galley.rows.len() == 1)
        }));
    }
}
