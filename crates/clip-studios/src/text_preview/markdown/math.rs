//! Baseline-aware mixed prose, image, and equation rows.
use super::{DocMedia, LinkTarget, PreviewKey, images::ImageStatus};
use crate::text_preview::*;
use little_clip_render::markdown::{ImagePresentation, MarkdownSpan};

pub(super) fn needs_inline_flow(spans: &[MarkdownSpan]) -> bool {
    spans.iter().any(|span| match span {
        MarkdownSpan::Math { .. }
        | MarkdownSpan::Image { .. }
        | MarkdownSpan::StyledImage { .. } => true,
        MarkdownSpan::Spoiler { spans } => needs_inline_flow(spans),
        _ => false,
    })
}

enum Content {
    Text(Arc<egui::Galley>),
    Equation {
        key: PreviewKey,
        source: String,
        display: bool,
        fallback: Option<Arc<egui::Galley>>,
    },
    Image {
        texture: egui::TextureHandle,
        rounding: egui::Rounding,
        alt: String,
    },
    HardBreak,
}

struct Atom {
    content: Content,
    width: f32,
    ascent: f32,
    descent: f32,
    link: Option<LinkTarget>,
    display: bool,
}

impl Atom {
    fn from_galley(galley: Arc<egui::Galley>, size: f32, link: Option<LinkTarget>) -> Self {
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
            link,
            display: false,
        }
    }

    fn formatted(
        ui: &egui::Ui,
        text: &str,
        format: egui::TextFormat,
        width: f32,
        size: f32,
        link: Option<LinkTarget>,
    ) -> Self {
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = width;
        job.append(text, 0.0, format);
        let galley = ui.fonts(|fonts| fonts.layout_job(job));
        Self::from_galley(galley, size, link)
    }

    fn text(
        ui: &egui::Ui,
        text: egui::RichText,
        width: f32,
        size: f32,
        link: Option<LinkTarget>,
    ) -> Self {
        let galley = egui::WidgetText::from(text).into_galley(
            ui,
            Some(true),
            width,
            FontId::proportional(size),
        );
        Self::from_galley(galley, size, link)
    }

    fn hard_break(size: f32) -> Self {
        Self {
            content: Content::HardBreak,
            width: 0.0,
            ascent: size,
            descent: size * 0.2,
            link: None,
            display: false,
        }
    }
}

fn row_metrics(row: &[Atom], base_size: f32) -> (f32, f32, f32) {
    let mut ascent = base_size;
    let mut descent = base_size * 0.2;
    let mut occupied = 0.0_f32;
    for atom in row {
        ascent = ascent.max(atom.ascent);
        descent = descent.max(atom.descent);
        occupied += atom.width;
    }
    (ascent, descent, occupied)
}

struct LineContext<'a> {
    width: f32,
    size: f32,
    prose_color: Color32,
    bold_color: Color32,
    palette: &'a AppPalette,
    block: u64,
}

#[derive(Clone, Copy)]
pub(super) struct SpanRenderStyle<'a> {
    pub size: f32,
    pub prose_color: Color32,
    pub bold_color: Color32,
    pub palette: &'a AppPalette,
    pub block: u64,
}

pub(super) fn measure_spans_size(
    ui: &egui::Ui,
    spans: &[MarkdownSpan],
    width: f32,
    style: SpanRenderStyle<'_>,
    media: &mut DocMedia<'_>,
) -> Vec2 {
    let line = LineContext {
        width: width.max(1.0),
        size: style.size,
        prose_color: style.prose_color,
        bold_color: style.bold_color,
        palette: style.palette,
        block: style.block,
    };
    let rows = wrap_atoms(collect_atoms(ui, spans, &line, media), line.width);
    let mut max_w: f32 = 0.0;
    let mut total_h: f32 = 0.0;
    for row in rows {
        let (ascent, descent, occupied) = row_metrics(&row, style.size);
        max_w = max_w.max(occupied);
        total_h += ascent + descent + 2.0;
    }
    Vec2::new(max_w.min(line.width), total_h.max(style.size))
}

// A formula or inline image is one indivisible item. Text wraps with egui's
// font shaping; each resulting row shares its largest ascent and descent.
pub(super) fn show_spans(
    ui: &mut egui::Ui,
    spans: &[MarkdownSpan],
    style: SpanRenderStyle<'_>,
    media: &mut DocMedia<'_>,
    align: egui::Align,
) {
    let line = LineContext {
        width: ui.available_width().max(1.0),
        size: style.size,
        prose_color: style.prose_color,
        bold_color: style.bold_color,
        palette: style.palette,
        block: style.block,
    };
    let atoms = collect_atoms(ui, spans, &line, media);
    for (row_index, row) in wrap_atoms(atoms, line.width).into_iter().enumerate() {
        let (ascent, descent, occupied) = row_metrics(&row, style.size);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(line.width, ascent + descent + 2.0),
            egui::Sense::hover(),
        );
        let mut x = rect.left();
        if (row.len() == 1 && row[0].display) || align == egui::Align::Center {
            x += ((line.width - occupied) / 2.0).max(0.0);
        } else if align == egui::Align::RIGHT {
            x += (line.width - occupied).max(0.0);
        }
        for (atom_index, atom) in row.into_iter().enumerate() {
            if matches!(atom.content, Content::HardBreak) {
                continue;
            }
            let atom_rect = egui::Rect::from_min_size(
                egui::pos2(x, rect.top() + ascent - atom.ascent),
                egui::vec2(atom.width, atom.ascent + atom.descent),
            );
            let id = ui
                .id()
                .with(("math-row", style.block, row_index, atom_index));
            let response = paint_atom(ui, &atom, atom_rect, id, &line, media);
            if let Some(link) = atom.link {
                if link.is_internal() {
                    let y = match &atom.content {
                        Content::Text(galley) => {
                            let offset = (line.size * 0.15).clamp(1.5, 3.0);
                            (atom_rect.top() + atom.ascent + offset)
                                .min(atom_rect.bottom() - 0.5)
                                .min(atom_rect.top() + galley.size().y - 0.5)
                        }
                        _ => (atom_rect.bottom() - 1.0).max(atom_rect.top()),
                    };
                    let stroke_w = if response.hovered() {
                        1.5_f32
                    } else {
                        1.25_f32
                    };
                    super::paint_dashed_h_line(
                        ui.painter(),
                        atom_rect.left(),
                        atom_rect.right(),
                        y,
                        Stroke::new(stroke_w, link.dash_color),
                        4.0,
                        2.5,
                    );
                }
                if response.clicked() {
                    super::activate_link_target(ui, &link, media.navigate_to.as_deref_mut());
                }
                response
                    .on_hover_text(link.hover_label())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
            }
            x += atom.width;
        }
    }
}

fn collect_atoms(
    ui: &egui::Ui,
    spans: &[MarkdownSpan],
    line: &LineContext<'_>,
    media: &mut DocMedia<'_>,
) -> Vec<Atom> {
    let mut atoms = Vec::new();
    for (span_index, span) in spans.iter().enumerate() {
        atoms.extend(atoms_for_span(ui, span, span_index, line, media));
    }
    atoms
}

fn atoms_for_span(
    ui: &egui::Ui,
    span: &MarkdownSpan,
    span_index: usize,
    line: &LineContext<'_>,
    media: &mut DocMedia<'_>,
) -> Vec<Atom> {
    match span {
        MarkdownSpan::Math {
            source,
            display,
            url,
        } => {
            let link_fg = super::link_color(line.palette);
            let link = url.as_deref().map(|raw| media.resolve_target(raw, link_fg));
            let Some(previews) = media.previews.as_deref_mut() else {
                let text: String = source.chars().take(160).collect();
                let mut rich = egui::RichText::new(text).monospace().size(line.size).color(
                    if link.is_some() {
                        link_fg
                    } else {
                        line.prose_color
                    },
                );
                if link.as_ref().is_some_and(|l| !l.is_internal()) {
                    rich = rich.underline();
                }
                return vec![Atom::text(ui, rich, line.width, line.size, link)];
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
                    link,
                    display: *display,
                }
            } else {
                let text: String = source.chars().take(160).collect();
                let (text, color) =
                    preview
                        .math_error()
                        .map_or((text.clone(), line.prose_color), |error| {
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
                    link,
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
        MarkdownSpan::Image { alt, url } => image_atoms(
            ui,
            alt,
            url,
            None,
            ImagePresentation::default(),
            line,
            media,
        ),
        MarkdownSpan::StyledImage {
            alt,
            url,
            href,
            style,
        } => image_atoms(ui, alt, url, href.as_deref(), *style, line, media),
        _ => prose_atoms(ui, span, span_index, line, media),
    }
}

fn image_atoms(
    ui: &egui::Ui,
    alt: &str,
    url: &str,
    href: Option<&str>,
    style: ImagePresentation,
    line: &LineContext<'_>,
    media: &mut DocMedia<'_>,
) -> Vec<Atom> {
    let dash_color = super::link_color(line.palette);
    let link_target = href
        .map(|h| media.resolve_target(h, dash_color))
        .or_else(|| {
            is_web_url(url).then(|| LinkTarget {
                raw_url: url.to_owned(),
                internal_path: None,
                dash_color,
            })
        });
    if let Some(images) = media.images.as_deref_mut() {
        match images.resolve(ui, url) {
            ImageStatus::Ready {
                texture,
                pixel_size,
            } => {
                let size = super::compute_image_display_size(
                    pixel_size,
                    style,
                    line.width,
                    (line.width * 0.7).clamp(24.0, 260.0),
                );
                let rounding = super::image_rounding(style, size);
                return vec![Atom {
                    content: Content::Image {
                        texture,
                        rounding,
                        alt: alt.to_owned(),
                    },
                    width: size.x,
                    ascent: size.y,
                    descent: 2.0,
                    link: link_target,
                    display: false,
                }];
            }
            ImageStatus::Loading => {
                let label = if alt.is_empty() {
                    "[loading image]".to_owned()
                } else {
                    format!("[loading: {alt}]")
                };
                return vec![Atom::text(
                    ui,
                    egui::RichText::new(label)
                        .italics()
                        .size(line.size)
                        .color(hex_to_c32(&line.palette.colors.text_muted)),
                    line.width,
                    line.size,
                    link_target,
                )];
            }
            ImageStatus::Unavailable => {}
        }
    }

    let fallback_label = if alt.is_empty() {
        "[image]".to_owned()
    } else {
        format!("[image: {alt}]")
    };
    let mut rich = egui::RichText::new(fallback_label)
        .italics()
        .size(line.size);
    if link_target.is_some() {
        rich = rich.color(dash_color);
        if link_target.as_ref().is_some_and(|l| !l.is_internal()) {
            rich = rich.underline();
        }
    } else {
        rich = rich.color(hex_to_c32(&line.palette.colors.text_muted));
    }
    vec![Atom::text(ui, rich, line.width, line.size, link_target)]
}

fn is_web_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn plain_span_text(span: &MarkdownSpan) -> &str {
    match span {
        MarkdownSpan::Text(t)
        | MarkdownSpan::Bold(t)
        | MarkdownSpan::Italic(t)
        | MarkdownSpan::BoldItalic(t)
        | MarkdownSpan::Strikethrough(t)
        | MarkdownSpan::Code(t) => t,
        MarkdownSpan::HighlightedCode { code, .. } => code,
        MarkdownSpan::Styled { text, .. } => text,
        MarkdownSpan::Link { label, .. } => label,
        MarkdownSpan::Math { source, .. } => source,
        MarkdownSpan::Image { .. }
        | MarkdownSpan::StyledImage { .. }
        | MarkdownSpan::Spoiler { .. }
        | MarkdownSpan::HardBreak => "",
    }
}

/// Splits `text` into word/character-wrapped `Atom`s, preserving single-glyph fallback.
fn push_wrapped_text_atoms(
    ui: &egui::Ui,
    text: &str,
    format: &egui::TextFormat,
    max_width: f32,
    size: f32,
    link: Option<&LinkTarget>,
    atoms: &mut Vec<Atom>,
) {
    for word in text.split_inclusive(char::is_whitespace) {
        let atom = Atom::formatted(ui, word, format.clone(), max_width, size, link.cloned());
        if atom.width <= max_width || word.chars().count() <= 1 {
            atoms.push(atom);
            continue;
        }
        // Break oversized whitespace-free tokens (CJK runs, long URLs) at char boundaries.
        let mut chunk_start = 0usize;
        for (idx, ch) in word.char_indices() {
            let next_end = idx + ch.len_utf8();
            let measured = Atom::formatted(
                ui,
                &word[chunk_start..next_end],
                format.clone(),
                max_width,
                size,
                link.cloned(),
            );
            if measured.width > max_width && idx > chunk_start {
                atoms.push(Atom::formatted(
                    ui,
                    &word[chunk_start..idx],
                    format.clone(),
                    max_width,
                    size,
                    link.cloned(),
                ));
                chunk_start = idx;
            }
        }
        if chunk_start < word.len() {
            atoms.push(Atom::formatted(
                ui,
                &word[chunk_start..],
                format.clone(),
                max_width,
                size,
                link.cloned(),
            ));
        }
    }
}

fn highlighted_code_atoms(
    ui: &egui::Ui,
    language: &str,
    code: &str,
    line: &LineContext<'_>,
) -> Vec<Atom> {
    let code_bg = hex_to_c32(&line.palette.colors.surface_panel);
    let fallback_fg = super::inline_code_fg(line.palette);
    let mono = FontId::monospace((line.size * 0.9).max(13.0));
    let mut atoms = Vec::new();
    for (token, fg) in
        super::inline_highlighted_runs(code, language, code_bg, fallback_fg, line.palette)
    {
        let fmt = egui::TextFormat {
            font_id: mono.clone(),
            color: fg,
            background: code_bg,
            ..Default::default()
        };
        push_wrapped_text_atoms(ui, &token, &fmt, line.width, line.size, None, &mut atoms);
    }
    atoms
}

fn spoiler_atoms(
    ui: &egui::Ui,
    inner: &[MarkdownSpan],
    span_index: usize,
    line: &LineContext<'_>,
) -> Vec<Atom> {
    let open_id = egui::Id::new(("md_spoiler_open", line.block, span_index));
    let hover_id = egui::Id::new(("md_spoiler_hover", line.block, span_index));
    let revealed = ui.ctx().data(|d| {
        d.get_temp::<bool>(open_id).unwrap_or(false)
            || d.get_temp::<bool>(hover_id).unwrap_or(false)
    });
    let mask = super::spoiler_mask_color(line.palette);
    let label_text: String = inner.iter().map(plain_span_text).collect();
    let fmt = egui::TextFormat {
        font_id: FontId::proportional(line.size),
        color: if revealed { line.prose_color } else { mask },
        background: if revealed {
            hex_to_c32(&line.palette.colors.surface_panel)
        } else {
            mask
        },
        ..Default::default()
    };
    let mut atoms = Vec::new();
    push_wrapped_text_atoms(
        ui,
        &label_text,
        &fmt,
        line.width,
        line.size,
        None,
        &mut atoms,
    );
    atoms
}

fn prose_atoms(
    ui: &egui::Ui,
    span: &MarkdownSpan,
    span_index: usize,
    line: &LineContext<'_>,
    media: &mut DocMedia<'_>,
) -> Vec<Atom> {
    let (text, url, format) = match span {
        MarkdownSpan::HardBreak => return vec![Atom::hard_break(line.size)],
        MarkdownSpan::HighlightedCode { language, code } => {
            return highlighted_code_atoms(ui, language, code, line);
        }
        MarkdownSpan::Spoiler { spans: inner } => {
            return spoiler_atoms(ui, inner, span_index, line);
        }
        MarkdownSpan::Text(text) => (
            text.as_str(),
            None,
            egui::TextFormat {
                font_id: FontId::proportional(line.size),
                color: line.prose_color,
                ..Default::default()
            },
        ),
        MarkdownSpan::Bold(text) => (
            text.as_str(),
            None,
            egui::TextFormat {
                font_id: FontId::proportional(line.size),
                color: line.bold_color,
                ..Default::default()
            },
        ),
        MarkdownSpan::Italic(text) => (
            text.as_str(),
            None,
            egui::TextFormat {
                font_id: FontId::proportional(line.size),
                color: line.prose_color,
                italics: true,
                ..Default::default()
            },
        ),
        MarkdownSpan::BoldItalic(text) => (
            text.as_str(),
            None,
            egui::TextFormat {
                font_id: FontId::proportional(line.size),
                color: line.bold_color,
                italics: true,
                ..Default::default()
            },
        ),
        MarkdownSpan::Strikethrough(text) => {
            let muted = hex_to_c32(&line.palette.colors.text_muted);
            (
                text.as_str(),
                None,
                egui::TextFormat {
                    font_id: FontId::proportional(line.size),
                    color: muted,
                    strikethrough: Stroke::new(1.0_f32, muted),
                    ..Default::default()
                },
            )
        }
        MarkdownSpan::Code(text) => (
            text.as_str(),
            None,
            egui::TextFormat {
                font_id: FontId::monospace((line.size * 0.9).max(13.0)),
                color: super::inline_code_fg(line.palette),
                background: hex_to_c32(&line.palette.colors.surface_panel),
                ..Default::default()
            },
        ),
        MarkdownSpan::Link { label, url } => {
            let link_fg = super::link_color(line.palette);
            let target = media.resolve_target(url, link_fg);
            let underline = if target.is_internal() {
                Stroke::NONE
            } else {
                Stroke::new(1.0_f32, link_fg)
            };
            (
                label.as_str(),
                Some(target),
                egui::TextFormat {
                    font_id: FontId::proportional(line.size),
                    color: link_fg,
                    underline,
                    ..Default::default()
                },
            )
        }
        MarkdownSpan::Styled { text, url, css } => {
            let mut fmt = super::build_inline_css_text_format(
                *css,
                url.is_some(),
                line.size,
                line.prose_color,
                line.bold_color,
                hex_to_c32(&line.palette.colors.canvas),
                line.palette,
            );
            let link = url.as_deref().map(|raw| {
                let target = media.resolve_target(raw, fmt.color);
                if target.is_internal() {
                    fmt.underline = Stroke::NONE;
                }
                target
            });
            (text.as_str(), link, fmt)
        }
        MarkdownSpan::Image { .. }
        | MarkdownSpan::StyledImage { .. }
        | MarkdownSpan::Math { .. } => return Vec::new(),
    };

    let mut atoms = Vec::new();
    push_wrapped_text_atoms(
        ui,
        text,
        &format,
        line.width,
        line.size,
        url.as_ref(),
        &mut atoms,
    );
    atoms
}

fn paint_atom(
    ui: &mut egui::Ui,
    atom: &Atom,
    rect: egui::Rect,
    id: egui::Id,
    line: &LineContext<'_>,
    media: &mut DocMedia<'_>,
) -> egui::Response {
    let sense = if atom.link.is_some() {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    match &atom.content {
        Content::Text(galley) => {
            ui.painter()
                .galley(rect.min, galley.clone(), line.prose_color);
            ui.interact(rect, id, sense)
        }
        Content::Equation {
            key,
            source,
            display,
            fallback,
        } => {
            if let Some(previews) = media.previews.as_deref_mut() {
                let preview = previews.entry(*key).or_default();
                if ui.is_rect_visible(rect) {
                    preview.prepare_math(ui, source, *display, line.prose_color);
                } else {
                    preview.suspend();
                }
                if let Some(galley) = fallback {
                    ui.painter()
                        .galley(rect.min, galley.clone(), line.prose_color);
                    let r = ui.interact(rect, id, sense);
                    if atom.link.is_none() {
                        r.clone()
                            .on_hover_text(source.chars().take(2048).collect::<String>());
                    }
                    r
                } else if atom.link.is_some() {
                    preview.paint_math_linked(ui, rect, id)
                } else {
                    preview.paint_math(ui, rect, source, id)
                }
            } else if let Some(galley) = fallback {
                ui.painter()
                    .galley(rect.min, galley.clone(), line.prose_color);
                ui.interact(rect, id, sense)
            } else {
                ui.interact(rect, id, sense)
            }
        }
        Content::Image {
            texture,
            rounding,
            alt,
        } => {
            egui::Image::new(texture)
                .rounding(*rounding)
                .paint_at(ui, rect);
            let response = ui.interact(rect, id, sense);
            if !alt.is_empty() && atom.link.is_none() {
                response.clone().on_hover_text(alt);
            }
            response
        }
        Content::HardBreak => ui.interact(rect, id, egui::Sense::hover()),
    }
}

fn wrap_atoms(atoms: Vec<Atom>, width: f32) -> Vec<Vec<Atom>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut occupied = 0.0;
    for atom in atoms {
        if matches!(atom.content, Content::HardBreak) {
            rows.push(std::mem::take(&mut row));
            occupied = 0.0;
            continue;
        }
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
    use super::super::Previews;
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
