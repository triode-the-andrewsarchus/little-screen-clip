mod math;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum PreviewKey {
    Block(usize),
    Span { block: usize, span: usize },
}
pub(super) type Previews = std::collections::HashMap<PreviewKey, crate::diagram_preview::Preview>;

use super::*;
use little_clip_render::markdown::MarkdownLine;

pub(super) enum MarkdownBlock {
    Line(MarkdownLine),
    Code { language: String, text: String },
}

pub(super) fn parse_blocks(content: &str) -> Vec<MarkdownBlock> {
    prepare_blocks(little_clip_render::markdown::parse_markdown(content))
}

pub(super) fn prepare_blocks(lines: impl IntoIterator<Item = MarkdownLine>) -> Vec<MarkdownBlock> {
    let mut blocks = Vec::new();
    let mut code: Option<(String, String)> = None;
    for line in lines {
        match (line, &mut code) {
            (MarkdownLine::CodeLine { line }, Some((_, text))) => {
                text.push_str(&line);
                text.push('\n');
            }
            (MarkdownLine::Empty, Some((_, text))) => text.push('\n'),
            (MarkdownLine::CodeFence { language }, None) => code = Some((language, String::new())),
            (line, _) => {
                if let Some((language, text)) = code.take() {
                    blocks.push(MarkdownBlock::Code { language, text });
                }
                if !matches!(
                    line,
                    MarkdownLine::CodeFence { .. } | MarkdownLine::CodeLine { .. }
                ) {
                    blocks.push(MarkdownBlock::Line(line));
                }
            }
        }
    }
    if let Some((language, text)) = code {
        blocks.push(MarkdownBlock::Code { language, text });
    }
    blocks
}

impl TextPreviewState {
    /// Retires read-only diagram jobs when the document presentation is hidden.
    pub fn cancel_diagram_previews(&mut self) {
        self.diagram_previews.clear();
    }

    /// Renders styled markdown view.
    pub(super) fn render_markdown_view(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let blocks = self.markdown.get_or_init(|| parse_blocks(&self.content));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                render_blocks(ui, blocks, palette, Some(&mut self.diagram_previews))
            });
    }
}

pub fn render_lines(
    ui: &mut egui::Ui,
    parsed: impl IntoIterator<Item = MarkdownLine>,
    palette: &AppPalette,
) {
    render_blocks(ui, &prepare_blocks(parsed), palette, None);
}

fn render_blocks(
    ui: &mut egui::Ui,
    blocks: &[MarkdownBlock],
    palette: &AppPalette,
    mut previews: Option<&mut Previews>,
) {
    ui.spacing_mut().item_spacing = Vec2::new(0.0, 6.0);
    for (index, block) in blocks.iter().enumerate() {
        match block {
            MarkdownBlock::Line(line) => {
                if previews
                    .as_deref_mut()
                    .is_none_or(|previews| !math::show_line(ui, line, palette, index, previews))
                {
                    render_markdown_element(ui, line, palette);
                }
            }
            MarkdownBlock::Code { language, text } => {
                if let Some(format) = little_clip_core::diagram::Format::from_language(language)
                    && let Some(previews) = previews.as_deref_mut()
                {
                    previews
                        .entry(PreviewKey::Block(index))
                        .or_default()
                        .show(ui, format, text);
                    egui::CollapsingHeader::new("Diagram source")
                        .id_source(index)
                        .show(ui, |ui| {
                            render_styled_code_block(ui, text, language, palette);
                        });
                } else {
                    render_styled_code_block(ui, text, language, palette);
                }
            }
        }
    }
}

fn quote_frame(palette: &AppPalette) -> egui::Frame {
    egui::Frame::none()
        .fill(hex_to_c32(&palette.colors.surface_recessed))
        .stroke(Stroke::new(3.0_f32, hex_to_c32(&palette.harmonies.base)))
        .inner_margin(egui::Margin::symmetric(10.0, 4.0))
        .rounding(Rounding::same(crate::ui_ext::BADGE_CORNER_RADIUS))
}

fn render_markdown_element(ui: &mut egui::Ui, line: &MarkdownLine, palette: &AppPalette) {
    match line {
        MarkdownLine::FrontMatter(metadata) => {
            egui::CollapsingHeader::new(format!("{} metadata", metadata.format.name()))
                .default_open(true)
                .show(ui, |ui| {
                    if let Err(error) = &metadata.value {
                        ui.colored_label(ui.visuals().error_fg_color, error);
                    } else {
                        egui::Grid::new(ui.next_auto_id())
                            .striped(true)
                            .show(ui, |ui| {
                                for (key, value) in metadata.display_fields() {
                                    ui.label(egui::RichText::new(key).strong());
                                    ui.label(value);
                                    ui.end_row();
                                }
                            });
                    }
                });
        }
        MarkdownLine::Heading {
            level,
            text: _,
            spans,
        } => {
            ui.add_space(crate::ui_ext::SPACE_SM);
            ui.horizontal_wrapped(|ui| {
                let (size, color) = match level {
                    1 => (HEADING_FONT_SIZE, hex_to_c32(&palette.colors.text_primary)),
                    2 => (
                        SUBHEADING_FONT_SIZE,
                        hex_to_c32(&palette.colors.text_primary),
                    ),
                    _ => (BODY_FONT_SIZE, hex_to_c32(&palette.colors.text_secondary)),
                };
                for span in spans {
                    render_markdown_span(ui, span, size, color, palette);
                }
            });
            ui.add_space(crate::ui_ext::SPACE_XS);
        }
        MarkdownLine::Bullet { indent, spans } => {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(*indent as f32 * 16.0);
                ui.label(egui::RichText::new("•").size(BODY_FONT_SIZE).color(
                    crate::ui_ext::contrast_text_c32_on(
                        ui.visuals().panel_fill,
                        ui.visuals().panel_fill,
                        Some(hex_to_c32(&palette.harmonies.base)),
                        palette,
                    ),
                ));
                ui.add_space(crate::ui_ext::SPACE_XS);
                for span in spans {
                    render_markdown_span(
                        ui,
                        span,
                        BODY_FONT_SIZE,
                        hex_to_c32(&palette.colors.text_primary),
                        palette,
                    );
                }
            });
        }
        MarkdownLine::Numbered { number, spans } => {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(format!("{}.", number))
                        .size(BODY_FONT_SIZE)
                        .strong()
                        .color(crate::ui_ext::contrast_text_c32_on(
                            ui.visuals().panel_fill,
                            ui.visuals().panel_fill,
                            Some(hex_to_c32(&palette.harmonies.base)),
                            palette,
                        )),
                );
                ui.add_space(crate::ui_ext::SPACE_XS);
                for span in spans {
                    render_markdown_span(
                        ui,
                        span,
                        BODY_FONT_SIZE,
                        hex_to_c32(&palette.colors.text_primary),
                        palette,
                    );
                }
            });
        }
        MarkdownLine::Quote { spans } => {
            quote_frame(palette).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for span in spans {
                        render_markdown_span(
                            ui,
                            span,
                            BODY_FONT_SIZE,
                            hex_to_c32(&palette.colors.text_secondary),
                            palette,
                        );
                    }
                });
            });
        }
        MarkdownLine::Rule => {
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
            ui.separator();
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        }
        MarkdownLine::Paragraph { spans } => {
            ui.horizontal_wrapped(|ui| {
                for span in spans {
                    render_markdown_span(
                        ui,
                        span,
                        BODY_FONT_SIZE,
                        hex_to_c32(&palette.colors.text_primary),
                        palette,
                    );
                }
            });
        }
        MarkdownLine::IncompleteMath { source } => {
            ui.colored_label(
                ui.visuals().error_fg_color,
                "Unclosed display equation ($$)",
            );
            ui.monospace(source.chars().take(2048).collect::<String>());
        }
        MarkdownLine::Empty => {
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        }
        MarkdownLine::CodeFence { .. } | MarkdownLine::CodeLine { .. } => {}
    }
}

/// Renders a single markdown inline span into egui UI.
pub(super) fn render_markdown_span(
    ui: &mut egui::Ui,
    span: &little_clip_render::markdown::MarkdownSpan,
    size: f32,
    color: Color32,
    palette: &AppPalette,
) {
    match span {
        little_clip_render::markdown::MarkdownSpan::Math { source, .. } => {
            ui.monospace(source);
        }
        little_clip_render::markdown::MarkdownSpan::Text(s) => {
            ui.label(egui::RichText::new(s).size(size).color(color));
        }
        little_clip_render::markdown::MarkdownSpan::Bold(s) => {
            ui.label(egui::RichText::new(s).size(size).strong().color(color));
        }
        little_clip_render::markdown::MarkdownSpan::Italic(s) => {
            ui.label(egui::RichText::new(s).size(size).italics().color(color));
        }
        little_clip_render::markdown::MarkdownSpan::BoldItalic(s) => {
            ui.label(
                egui::RichText::new(s)
                    .size(size)
                    .strong()
                    .italics()
                    .color(color),
            );
        }
        little_clip_render::markdown::MarkdownSpan::Code(s) => {
            egui::Frame::none()
                .fill(hex_to_c32(&palette.colors.surface_panel))
                .inner_margin(egui::Margin::symmetric(4.0, 1.0))
                .rounding(Rounding::same(crate::ui_ext::BADGE_CORNER_RADIUS))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(s)
                            .font(FontId::monospace(size.max(MONO_FONT_SIZE)))
                            .color(crate::ui_ext::contrast_text_c32_on(
                                hex_to_c32(&palette.colors.surface_panel),
                                hex_to_c32(&palette.colors.surface_panel),
                                Some(hex_to_c32(&palette.harmonies.triadic_2)),
                                palette,
                            )),
                    );
                });
        }
        little_clip_render::markdown::MarkdownSpan::Link { label, url } => {
            ui.hyperlink_to(
                egui::RichText::new(label)
                    .size(size)
                    .color(crate::ui_ext::contrast_text_c32_on(
                        hex_to_c32(&palette.colors.surface_panel),
                        hex_to_c32(&palette.colors.surface_panel),
                        Some(hex_to_c32(&palette.harmonies.base)),
                        palette,
                    ))
                    .underline(),
                url,
            );
        }
    }
}

/// Renders a styled code block for markdown view.
pub(super) fn render_styled_code_block(
    ui: &mut egui::Ui,
    code: &str,
    lang: &str,
    palette: &AppPalette,
) {
    let syntax = find_syntax(lang);

    egui::Frame::none()
        .fill(hex_to_c32(&palette.colors.surface_recessed))
        .stroke(Stroke::new(
            1.0_f32,
            hex_to_c32(&palette.colors.surface_panel),
        ))
        .inner_margin(egui::Margin::same(10.0))
        .rounding(Rounding::same(crate::ui_ext::CONTROL_CORNER_RADIUS))
        .show(ui, |ui| {
            if !lang.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(lang.to_uppercase())
                            .size(crate::ui_ext::BODY_FONT_SIZE)
                            .color(hex_to_c32(&palette.colors.text_muted)),
                    );
                });
                ui.add_space(crate::ui_ext::SPACE_XS);
            }

            let mut layouter = |ui: &egui::Ui, text: &str, wrap_width: f32| {
                let mut layout_job = build_highlighted_layout_job(text, syntax, palette, &[]);
                layout_job.wrap.max_width = wrap_width;
                ui.fonts(|f| f.layout_job(layout_job))
            };

            let mut code = code;
            let text_edit = egui::TextEdit::multiline(&mut code)
                .font(FontId::monospace(crate::ui_ext::MONO_FONT_SIZE))
                .layouter(&mut layouter)
                .desired_width(f32::INFINITY)
                .lock_focus(false)
                .interactive(true);

            ui.add(text_edit);
        });
}
