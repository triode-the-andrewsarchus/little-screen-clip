//! Baseline-aware mixed prose/equation rows, using the shared preview owner.
use super::{PreviewKey, Previews};
use crate::text_preview::*;
use little_clip_render::markdown::{MarkdownLine, MarkdownSpan};

pub(super) fn show_line(
    ui: &mut egui::Ui,
    line: &MarkdownLine,
    palette: &AppPalette,
    block: usize,
    previews: &mut Previews,
) -> bool {
    let (spans, size, color, prefix) = match line {
        MarkdownLine::Heading { level, spans, .. } => (
            spans,
            match level {
                1 => HEADING_FONT_SIZE,
                2 => SUBHEADING_FONT_SIZE,
                _ => BODY_FONT_SIZE,
            },
            hex_to_c32(&palette.colors.text_primary),
            String::new(),
        ),
        MarkdownLine::Bullet { spans, .. } => (
            spans,
            BODY_FONT_SIZE,
            hex_to_c32(&palette.colors.text_primary),
            "• ".into(),
        ),
        MarkdownLine::Numbered { number, spans } => (
            spans,
            BODY_FONT_SIZE,
            hex_to_c32(&palette.colors.text_primary),
            format!("{number} "),
        ),
        MarkdownLine::Quote { spans } => (
            spans,
            BODY_FONT_SIZE,
            hex_to_c32(&palette.colors.text_secondary),
            String::new(),
        ),
        MarkdownLine::Paragraph { spans } => (
            spans,
            BODY_FONT_SIZE,
            hex_to_c32(&palette.colors.text_primary),
            String::new(),
        ),
        _ => return false,
    };
    if !spans
        .iter()
        .any(|span| matches!(span, MarkdownSpan::Math { .. }))
    {
        return false;
    }
    if matches!(line, MarkdownLine::Heading { .. }) {
        ui.add_space(crate::ui_ext::SPACE_SM);
    }
    match line {
        MarkdownLine::Quote { .. } => {
            super::quote_frame(palette).show(ui, |ui| {
                show_spans(ui, spans, size, color, &prefix, block, previews)
            });
        }
        MarkdownLine::Bullet { indent, .. } if *indent > 0 => {
            let padding = (*indent as f32 * 16.0).min((ui.available_width() - 32.0).max(0.0));
            ui.horizontal_top(|ui| {
                ui.add_space(padding);
                ui.vertical(|ui| show_spans(ui, spans, size, color, &prefix, block, previews));
            });
        }
        _ => show_spans(ui, spans, size, color, &prefix, block, previews),
    }
    if matches!(line, MarkdownLine::Heading { .. }) {
        ui.add_space(crate::ui_ext::SPACE_XS);
    }
    true
}

enum Content {
    Text(Arc<egui::Galley>),
    Equation {
        key: PreviewKey,
        source: String,
        display: bool,
        fallback: Option<Arc<egui::Galley>>,
    },
}
struct Atom {
    content: Content,
    width: f32,
    ascent: f32,
    descent: f32,
    url: Option<String>,
    display: bool,
}
impl Atom {
    fn text(
        ui: &egui::Ui,
        text: egui::RichText,
        width: f32,
        size: f32,
        url: Option<String>,
    ) -> Self {
        let galley = egui::WidgetText::from(text).into_galley(
            ui,
            Some(true),
            width,
            FontId::proportional(size),
        );
        let ascent = galley
            .rows
            .first()
            .and_then(|row| row.glyphs.first())
            .map_or(size, |glyph| glyph.pos.y);
        Self {
            width: galley.size().x,
            ascent,
            descent: (galley.size().y - ascent).max(0.0),
            content: Content::Text(galley),
            url,
            display: false,
        }
    }
}

struct LineContext {
    width: f32,
    size: f32,
    color: Color32,
    block: usize,
}

// A formula is one indivisible item. Text wraps with egui's own font shaping;
// each resulting row shares its largest ascent and descent.
fn show_spans(
    ui: &mut egui::Ui,
    spans: &[MarkdownSpan],
    size: f32,
    color: Color32,
    prefix: &str,
    block: usize,
    previews: &mut Previews,
) {
    let line = LineContext {
        width: ui.available_width().max(1.0),
        size,
        color,
        block,
    };
    let mut atoms = Vec::new();
    if !prefix.is_empty() {
        atoms.push(Atom::text(
            ui,
            egui::RichText::new(prefix).size(size).color(color).strong(),
            line.width,
            size,
            None,
        ));
    }
    for (span_index, span) in spans.iter().enumerate() {
        atoms.extend(atoms_for_span(ui, span, span_index, &line, previews));
    }
    for (row_index, row) in wrap_atoms(atoms, line.width).into_iter().enumerate() {
        let ascent = row.iter().map(|atom| atom.ascent).fold(size, f32::max);
        let descent = row
            .iter()
            .map(|atom| atom.descent)
            .fold(size * 0.2, f32::max);
        let occupied: f32 = row.iter().map(|atom| atom.width).sum();
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(line.width, ascent + descent + 2.0),
            egui::Sense::hover(),
        );
        let mut x = rect.left();
        if row.len() == 1 && row[0].display {
            x += ((line.width - occupied) / 2.0).max(0.0);
        }
        for (atom_index, atom) in row.into_iter().enumerate() {
            let atom_rect = egui::Rect::from_min_size(
                egui::pos2(x, rect.top() + ascent - atom.ascent),
                egui::vec2(atom.width, atom.ascent + atom.descent),
            );
            let id = ui.id().with(("math-row", block, row_index, atom_index));
            let response = paint_atom(ui, &atom, atom_rect, id, &line, previews);
            if let Some(url) = atom.url {
                if response.clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(&url));
                }
                response
                    .on_hover_text(url)
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
            }
            x += atom.width;
        }
    }
}

fn atoms_for_span(
    ui: &egui::Ui,
    span: &MarkdownSpan,
    span_index: usize,
    line: &LineContext,
    previews: &mut Previews,
) -> Vec<Atom> {
    let MarkdownSpan::Math {
        source,
        display,
        url,
    } = span
    else {
        return prose_atoms(ui, span, line);
    };
    let key = PreviewKey::Span {
        block: line.block,
        span: span_index,
    };
    let preview = previews.entry(key).or_default();
    let mut atom = if let Some(layout) = preview
        .math_layout()
        .filter(|_| preview.math_error().is_none())
    {
        let scale = (line.width / (layout.width_em * line.size).max(1.0))
            .min(520.0 / (layout.height_em * line.size).max(1.0))
            .min(1.0);
        Atom {
            content: Content::Equation {
                key,
                source: source.clone(),
                display: *display,
                fallback: None,
            },
            width: layout.width_em * line.size * scale,
            ascent: layout.baseline_em * line.size * scale,
            descent: (layout.height_em - layout.baseline_em) * line.size * scale,
            url: url.clone(),
            display: *display,
        }
    } else {
        // Loading/error text is bounded independently of the owned source. Do not
        // shape a whole large equation synchronously just to show its placeholder.
        let text: String = source.chars().take(160).collect();
        let (text, color) = preview
            .math_error()
            .map_or((text.clone(), line.color), |error| {
                (
                    format!("{text} — {}", error.chars().take(512).collect::<String>()),
                    ui.visuals().error_fg_color,
                )
            });
        Atom::text(
            ui,
            egui::RichText::new(text)
                .monospace()
                .size(line.size)
                .color(color),
            line.width,
            line.size,
            url.clone(),
        )
    };
    if let Content::Text(galley) = &atom.content {
        atom.content = Content::Equation {
            key,
            source: source.clone(),
            display: *display,
            fallback: Some(galley.clone()),
        };
        atom.display = *display;
    }
    vec![atom]
}

fn prose_atoms(ui: &egui::Ui, span: &MarkdownSpan, line: &LineContext) -> Vec<Atom> {
    let (text, bold, italic, code, url) = match span {
        MarkdownSpan::Text(text) => (text, false, false, false, None),
        MarkdownSpan::Bold(text) => (text, true, false, false, None),
        MarkdownSpan::Italic(text) => (text, false, true, false, None),
        MarkdownSpan::BoldItalic(text) => (text, true, true, false, None),
        MarkdownSpan::Code(text) => (text, false, false, true, None),
        MarkdownSpan::Link { label, url } => (label, false, false, false, Some(url.clone())),
        MarkdownSpan::Math { .. } => return Vec::new(),
    };
    text.split_inclusive(char::is_whitespace)
        .map(|word| {
            let mut rich = egui::RichText::new(word).size(line.size).color(line.color);
            if bold {
                rich = rich.strong();
            }
            if italic {
                rich = rich.italics();
            }
            if code {
                rich = rich
                    .monospace()
                    .background_color(ui.visuals().code_bg_color);
            }
            if url.is_some() {
                rich = rich.underline();
            }
            Atom::text(ui, rich, line.width, line.size, url.clone())
        })
        .collect()
}

fn paint_atom(
    ui: &mut egui::Ui,
    atom: &Atom,
    rect: egui::Rect,
    id: egui::Id,
    line: &LineContext,
    previews: &mut Previews,
) -> egui::Response {
    match &atom.content {
        Content::Text(galley) => {
            ui.painter().galley(rect.min, galley.clone(), line.color);
            ui.interact(rect, id, egui::Sense::click())
        }
        Content::Equation {
            key,
            source,
            display,
            fallback,
        } => {
            let preview = previews.entry(*key).or_default();
            // Geometry is known before admission, including wrapped paragraphs:
            // only the actually visible equation may start/poll its worker.
            if ui.is_rect_visible(rect) {
                preview.prepare_math(ui, source, *display, line.color);
            } else {
                preview.suspend();
            }
            if let Some(galley) = fallback {
                ui.painter().galley(rect.min, galley.clone(), line.color);
                ui.interact(rect, id, egui::Sense::hover())
                    .on_hover_text(source.chars().take(2048).collect::<String>())
            } else {
                preview.paint_math(ui, rect, source, id)
            }
        }
    }
}

fn wrap_atoms(atoms: Vec<Atom>, width: f32) -> Vec<Vec<Atom>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut occupied = 0.0;
    for atom in atoms {
        if !row.is_empty() && (atom.display || occupied + atom.width > width) {
            rows.push(std::mem::take(&mut row));
            occupied = 0.0;
        }
        occupied += atom.width;
        let display = atom.display;
        row.push(atom);
        if display {
            rows.push(std::mem::take(&mut row));
            occupied = 0.0;
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_equation_document(
        content: &str,
        layout: little_clip_render::diagram::MathLayout,
        visuals: egui::Visuals,
    ) -> (egui::Rect, egui::Rect, f32) {
        let ctx = egui::Context::default();
        ctx.set_visuals(visuals);
        let (preview, texture_id) = crate::diagram_preview::Preview::equation_fixture(&ctx, layout);
        let mut previews = Previews::from([(PreviewKey::Span { block: 0, span: 1 }, preview)]);
        let blocks = super::super::parse_blocks(content);
        let palette = AppPalette::default();
        let mut available = egui::Rect::NOTHING;
        let mut following_top = 0.0;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(500.0, 300.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    available = ui.available_rect_before_wrap();
                    super::super::render_blocks(ui, &blocks, &palette, Some(&mut previews));
                    following_top = ui.label("Following paragraph").rect.top();
                });
            },
        );
        // Inspect requested layout bounds before tessellation adds its
        // antialiasing fringe around the image rectangle.
        let bounds = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Rect(rect) if rect.fill_texture_id == texture_id => {
                    Some(rect.rect)
                }
                _ => None,
            })
            .expect("Markdown math should paint its texture");
        (bounds, available, following_top)
    }

    #[test]
    fn document_math_is_painted_at_em_size_and_next_paragraph_clears_its_depth() {
        let layout = little_clip_render::diagram::MathLayout {
            width_em: 4.0,
            height_em: 3.0,
            baseline_em: 2.0,
        };
        for visuals in [egui::Visuals::dark(), egui::Visuals::light()] {
            let (bounds, _, following_top) =
                render_equation_document("Before $x$ after", layout, visuals);
            assert!((bounds.width() - BODY_FONT_SIZE * layout.width_em).abs() < 0.1);
            assert!((bounds.height() - BODY_FONT_SIZE * layout.height_em).abs() < 0.1);
            assert!(
                following_top >= bounds.bottom(),
                "next paragraph must clear the fraction depth"
            );
        }
    }

    #[test]
    fn display_equation_wraps_into_a_separate_centered_row() {
        let layout = little_clip_render::diagram::MathLayout {
            width_em: 3.0,
            height_em: 2.0,
            baseline_em: 1.5,
        };
        let (bounds, available, following_top) =
            render_equation_document("Before $$x$$ after", layout, egui::Visuals::dark());
        assert!((bounds.center().x - available.center().x).abs() < 0.1);
        assert!(bounds.top() > available.top() + BODY_FONT_SIZE);
        assert!(following_top > bounds.bottom() + BODY_FONT_SIZE);
    }
}
