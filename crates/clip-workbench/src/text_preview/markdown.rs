use super::*;

impl TextPreviewState {
    /// Renders styled markdown view.
    pub(super) fn render_markdown_view(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let parsed = little_clip_render::markdown::parse_markdown(&self.content);

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(0.0, 6.0);

                let mut in_code_block = false;
                let mut code_block_buf = String::new();
                let mut code_block_lang = String::new();

                for line in parsed {
                    render_markdown_element(
                        ui,
                        line,
                        &mut in_code_block,
                        &mut code_block_buf,
                        &mut code_block_lang,
                        palette,
                    );
                }

                flush_code_block(
                    ui,
                    &mut in_code_block,
                    &mut code_block_buf,
                    &mut code_block_lang,
                    palette,
                );
            });
    }
}

pub(super) fn flush_code_block(
    ui: &mut egui::Ui,
    in_code: &mut bool,
    buf: &mut String,
    lang: &mut String,
    palette: &AppPalette,
) {
    if *in_code {
        render_styled_code_block(ui, buf, lang, palette);
        buf.clear();
        lang.clear();
        *in_code = false;
    }
}

pub(super) fn render_markdown_element(
    ui: &mut egui::Ui,
    line: little_clip_render::markdown::MarkdownLine,
    in_code_block: &mut bool,
    code_block_buf: &mut String,
    code_block_lang: &mut String,
    palette: &AppPalette,
) {
    match line {
        little_clip_render::markdown::MarkdownLine::CodeFence { language } => {
            if *in_code_block {
                flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            } else {
                *in_code_block = true;
                *code_block_lang = language;
            }
        }
        little_clip_render::markdown::MarkdownLine::CodeLine { line } => {
            if *in_code_block {
                code_block_buf.push_str(&line);
                code_block_buf.push('\n');
            }
        }
        little_clip_render::markdown::MarkdownLine::Heading {
            level,
            text: _,
            spans,
        } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
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
                    render_markdown_span(ui, &span, size, color, palette);
                }
            });
            ui.add_space(crate::ui_ext::SPACE_XS);
        }
        little_clip_render::markdown::MarkdownLine::Bullet { indent, spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(indent as f32 * 16.0);
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
                        &span,
                        BODY_FONT_SIZE,
                        hex_to_c32(&palette.colors.text_primary),
                        palette,
                    );
                }
            });
        }
        little_clip_render::markdown::MarkdownLine::Numbered { number, spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
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
                        &span,
                        BODY_FONT_SIZE,
                        hex_to_c32(&palette.colors.text_primary),
                        palette,
                    );
                }
            });
        }
        little_clip_render::markdown::MarkdownLine::Quote { spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            egui::Frame::none()
                .fill(hex_to_c32(&palette.colors.surface_recessed))
                .stroke(Stroke::new(3.0_f32, hex_to_c32(&palette.harmonies.base)))
                .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                .rounding(Rounding::same(crate::ui_ext::BADGE_CORNER_RADIUS))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for span in spans {
                            render_markdown_span(
                                ui,
                                &span,
                                BODY_FONT_SIZE,
                                hex_to_c32(&palette.colors.text_secondary),
                                palette,
                            );
                        }
                    });
                });
        }
        little_clip_render::markdown::MarkdownLine::Rule => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
            ui.separator();
            ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
        }
        little_clip_render::markdown::MarkdownLine::Paragraph { spans } => {
            flush_code_block(ui, in_code_block, code_block_buf, code_block_lang, palette);
            ui.horizontal_wrapped(|ui| {
                for span in spans {
                    render_markdown_span(
                        ui,
                        &span,
                        BODY_FONT_SIZE,
                        hex_to_c32(&palette.colors.text_primary),
                        palette,
                    );
                }
            });
        }
        little_clip_render::markdown::MarkdownLine::Empty => {
            if *in_code_block {
                code_block_buf.push('\n');
            } else {
                ui.add_space(crate::ui_ext::ITEM_SPACING_Y);
            }
        }
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

            let mut code_clone = code.to_string();
            let text_edit = egui::TextEdit::multiline(&mut code_clone)
                .font(FontId::monospace(crate::ui_ext::MONO_FONT_SIZE))
                .layouter(&mut layouter)
                .desired_width(f32::INFINITY)
                .lock_focus(false)
                .interactive(true);

            ui.add(text_edit);
        });
}
