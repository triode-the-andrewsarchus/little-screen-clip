use std::{borrow::Cow, sync::Arc};

struct MarkdownRun<'a> {
    text: Cow<'a, str>,
    style: FontStyle,
    color: Color,
    width: f32,
    ascent: f32,
    descent: f32,
    image: Option<Arc<crate::diagram::Thumbnail>>,
}
impl<'a> MarkdownRun<'a> {
    fn text(
        renderer: &ScreenRenderer,
        text: Cow<'a, str>,
        size: f32,
        style: FontStyle,
        color: Color,
    ) -> Self {
        Self {
            width: renderer.measure_text_width_styled(&text, size, style),
            text,
            style,
            color,
            ascent: size,
            descent: size * 0.2,
            image: None,
        }
    }
}

use super::*;

impl ScreenRenderer {
    /// Draws one inline row with a shared baseline, including native equations.
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit raster bounds, text metrics and palette at the drawing boundary"
    )]
    pub(crate) fn draw_markdown_spans(
        &self,
        pixmap: &mut Pixmap,
        spans: &[crate::markdown::MarkdownSpan],
        x: f32,
        y: f32,
        size: f32,
        default_color: Color,
        clip_right: f32,
        max_height: f32,
        prefix: Option<(&str, Color)>,
    ) -> f32 {
        use crate::markdown::MarkdownSpan;
        let mut runs = Vec::new();
        let mut occupied = 0.0;
        if let Some((text, color)) = prefix {
            let run = MarkdownRun::text(self, text.into(), size, FontStyle::Bold, color);
            occupied += run.width;
            runs.push(run);
        }
        for span in spans {
            if x + occupied >= clip_right {
                break;
            }
            let (text, style, color) = match span {
                MarkdownSpan::Text(text) => (text, FontStyle::Regular, default_color),
                MarkdownSpan::Bold(text) => (
                    text,
                    FontStyle::Bold,
                    self.color_from_hex(&self.palette.colors.text_primary),
                ),
                MarkdownSpan::Italic(text) => (
                    text,
                    FontStyle::Italic,
                    self.color_from_hex_alpha(&self.palette.colors.text_muted, 240),
                ),
                MarkdownSpan::BoldItalic(text) => (
                    text,
                    FontStyle::BoldItalic,
                    self.color_from_hex(&self.palette.colors.text_primary),
                ),
                MarkdownSpan::Code(text) => (
                    text,
                    FontStyle::Mono,
                    self.color_from_hex(&self.palette.undo_border),
                ),
                MarkdownSpan::Link { label, .. } => (
                    label,
                    FontStyle::Regular,
                    self.color_from_hex(&self.palette.url_color),
                ),
                MarkdownSpan::Math {
                    source, display, ..
                } => {
                    let foreground = [
                        default_color.red(),
                        default_color.green(),
                        default_color.blue(),
                    ]
                    .map(|value| (value * 255.0).round() as u8);
                    let format = if *display {
                        little_clip_core::diagram::Format::Math
                    } else {
                        little_clip_core::diagram::Format::MathInline
                    };
                    let result = self
                        .diagram_previews
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .thumbnail_colored(format, source, Some(foreground))
                        .and_then(|preview| {
                            preview
                                .math
                                .map(|layout| (preview, layout))
                                .ok_or_else(|| "Missing equation layout".into())
                        });
                    let run = match result {
                        Ok((preview, layout)) => {
                            let scale = ((clip_right - x - occupied)
                                / (layout.width_em * size).max(1.0))
                            .min(max_height / (layout.height_em * size).max(1.0))
                            .min(1.0);
                            MarkdownRun {
                                text: "".into(),
                                style: FontStyle::Regular,
                                color: default_color,
                                width: layout.width_em * size * scale,
                                ascent: layout.baseline_em * size * scale,
                                descent: (layout.height_em - layout.baseline_em) * size * scale,
                                image: Some(preview),
                            }
                        }
                        Err(error) => MarkdownRun::text(
                            self,
                            format!("Math: {error}").into(),
                            size,
                            FontStyle::Mono,
                            default_color,
                        ),
                    };
                    occupied += run.width;
                    runs.push(run);
                    continue;
                }
            };
            let run = MarkdownRun::text(self, text.as_str().into(), size, style, color);
            occupied += run.width;
            runs.push(run);
        }
        let ascent = runs.iter().map(|run| run.ascent).fold(size, f32::max);
        let descent = runs
            .iter()
            .map(|run| run.descent)
            .fold(size * 0.2, f32::max);
        let mut cx = x;
        // A display equation on its own line is centered at its natural em size.
        if matches!(spans, [MarkdownSpan::Math { display: true, .. }]) {
            cx += ((clip_right - x - occupied) / 2.0).max(0.0);
        }
        for run in runs {
            if let Some(preview) = run.image {
                let image = &preview.image;
                if let Some(pixels) =
                    PixmapRef::from_bytes(&image.premultiplied_rgba, image.width, image.height)
                {
                    let transform = Transform::from_scale(
                        run.width / image.width as f32,
                        (run.ascent + run.descent) / image.height as f32,
                    )
                    .post_translate(cx, y + ascent - run.ascent);
                    pixmap.draw_pixmap(0, 0, pixels, &PixmapPaint::default(), transform, None);
                }
            } else {
                self.draw_text_styled_clipped(
                    pixmap,
                    &run.text,
                    cx,
                    y + ascent - size,
                    size,
                    run.color,
                    run.style,
                    clip_right,
                );
            }
            cx += run.width;
        }
        ascent + descent + 4.0
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Drawing keeps target, geometry, content and palette explicit at the rendering boundary"
    )]
    pub(super) fn draw_markdown_preview(
        &self,
        pixmap: &mut Pixmap,
        content: &str,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        is_hero: bool,
        border_color: Color,
        elision_color: Color,
    ) {
        use crate::markdown::MarkdownLine;
        let body_color = self.color_from_hex(&self.palette.colors.text_muted);
        let content_x = rx + 14.0;
        let max_content_w = (rw - 28.0).max(10.0);
        let clip_right = rx + rw - 10.0;

        let clean_content = content.trim_start_matches(is_zero_width_or_bom);
        let (lines, total_lines) = crate::markdown::parse_markdown_preview(clean_content);
        let max_y = ry + rh - 22.0;
        let mut y = ry + 124.0;

        let mut skipped_until = 0;
        for (idx, line) in lines.iter().enumerate() {
            if idx < skipped_until {
                continue;
            }
            let is_near_bottom = y + 26.0 > max_y;
            let has_more = idx + 1 < total_lines;

            if is_near_bottom && has_more {
                let remaining = total_lines - idx;
                let badge = format!("▼ +{} more lines", remaining);
                self.draw_footer_badge(pixmap, &badge, content_x, y, 16.0, clip_right);
                break;
            }

            if y > max_y {
                break;
            }

            if let MarkdownLine::CodeFence { language } = line
                && let Some(format) = little_clip_core::diagram::Format::from_language(language)
                && let Some(last) =
                    lines
                        .iter()
                        .enumerate()
                        .skip(idx + 1)
                        .find_map(|(end, line)| {
                            matches!(line, MarkdownLine::CodeFence { .. }).then_some(end)
                        })
            {
                let mut source = String::new();
                for line in &lines[idx + 1..last] {
                    if let MarkdownLine::CodeLine { line } = line {
                        source.push_str(line);
                        source.push('\n');
                    }
                }
                y += self.draw_diagram_preview(
                    pixmap,
                    format,
                    &source,
                    content_x,
                    y,
                    max_content_w,
                    (max_y - y - 4.0).max(1.0),
                    body_color,
                    false,
                );
                skipped_until = last + 1;
                continue;
            }

            match line {
                MarkdownLine::FrontMatter(metadata) => {
                    let fields = metadata.display_fields();
                    let heading = match &metadata.value {
                        Ok(_) => format!(
                            "{} metadata · {} fields",
                            metadata.format.name(),
                            fields.len()
                        ),
                        Err(_) => format!("{} metadata · parse error", metadata.format.name()),
                    };
                    self.draw_text_styled_clipped(
                        pixmap,
                        &heading,
                        content_x,
                        y,
                        14.0,
                        border_color,
                        FontStyle::Bold,
                        clip_right,
                    );
                    y += 18.0;
                    for (key, value) in fields.iter().take(2) {
                        if y + 18.0 > max_y {
                            break;
                        }
                        self.draw_text_styled_clipped(
                            pixmap,
                            &format!("{key}: {value}"),
                            content_x,
                            y,
                            13.0,
                            body_color,
                            FontStyle::Regular,
                            clip_right,
                        );
                        y += 18.0;
                    }
                }
                MarkdownLine::Heading {
                    level,
                    text: _,
                    spans,
                } => {
                    let (head_size, head_color_pref, lead_symbol) = match level {
                        1 => (
                            if is_hero { 21.0 } else { 18.0 },
                            self.color_from_hex(&self.palette.harmonies.base),
                            "# ",
                        ),
                        2 => (
                            if is_hero { 19.0 } else { 16.5 },
                            self.color_from_hex(&self.palette.harmonies.triadic_1),
                            "## ",
                        ),
                        _ => (
                            if is_hero { 17.5 } else { 15.0 },
                            self.color_from_hex(&self.palette.harmonies.triadic_2),
                            "### ",
                        ),
                    };
                    let head_color = self.contrast_color(
                        self.color_from_hex(&self.palette.colors.canvas),
                        Some(head_color_pref),
                    );

                    y += self
                        .draw_markdown_spans(
                            pixmap,
                            spans,
                            content_x,
                            y,
                            head_size,
                            head_color,
                            clip_right,
                            max_y - y,
                            Some((lead_symbol, elision_color)),
                        )
                        .max(if is_hero { 26.0 } else { 22.0 });
                }
                MarkdownLine::Bullet { indent, spans } => {
                    let indent_px = (*indent as f32 * 4.0).min(16.0);
                    let size = if is_hero { 18.0 } else { 15.0 };
                    y += self
                        .draw_markdown_spans(
                            pixmap,
                            spans,
                            content_x + indent_px,
                            y,
                            size,
                            body_color,
                            clip_right,
                            max_y - y,
                            Some(("• ", border_color)),
                        )
                        .max(if is_hero { 24.0 } else { 20.0 });
                }
                MarkdownLine::Numbered { number, spans } => {
                    let size = if is_hero { 18.0 } else { 15.0 };
                    y += self
                        .draw_markdown_spans(
                            pixmap,
                            spans,
                            content_x,
                            y,
                            size,
                            body_color,
                            clip_right,
                            max_y - y,
                            Some((&format!("{number} "), border_color)),
                        )
                        .max(if is_hero { 24.0 } else { 20.0 });
                }
                MarkdownLine::Quote { spans } => {
                    let size = if is_hero { 18.0 } else { 15.0 };
                    let quote_fg = self.contrast_color(
                        self.color_from_hex(&self.palette.colors.canvas),
                        Some(self.color_from_hex(&self.palette.colors.text_secondary)),
                    );
                    let height = self
                        .draw_markdown_spans(
                            pixmap,
                            spans,
                            content_x + 10.0,
                            y,
                            size,
                            quote_fg,
                            clip_right,
                            max_y - y,
                            None,
                        )
                        .max(if is_hero { 24.0 } else { 20.0 });
                    if let Some(rect) = Rect::from_xywh(content_x, y, 2.0, height) {
                        let mut paint = Paint::default();
                        paint.set_color(border_color);
                        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
                    }
                    y += height;
                }
                MarkdownLine::Rule => {
                    if let Some(rr) = Rect::from_xywh(content_x, y + 6.0, max_content_w, 1.0) {
                        let mut rp = Paint::default();
                        rp.set_color(self.color_from_hex(&self.palette.colors.surface_panel));
                        pixmap.fill_rect(rr, &rp, Transform::identity(), None);
                    }
                    y += 14.0;
                }
                MarkdownLine::CodeFence { language } => {
                    let label = if language.is_empty() {
                        "```".to_string()
                    } else {
                        format!("``` {}", language)
                    };
                    let fence_fg = self.contrast_color(
                        self.color_from_hex(&self.palette.colors.canvas),
                        Some(self.color_from_hex(&self.palette.colors.text_muted)),
                    );
                    self.draw_text_styled_clipped(
                        pixmap,
                        &label,
                        content_x,
                        y,
                        14.0,
                        fence_fg,
                        FontStyle::Mono,
                        clip_right,
                    );
                    y += 18.0;
                }
                MarkdownLine::CodeLine { line } => {
                    let font_size = if is_hero { 16.0 } else { 13.5 };
                    let code_bg = self.color_from_hex(&self.palette.colors.surface_recessed);
                    let code_fg = self.contrast_color(
                        code_bg,
                        Some(self.color_from_hex(&self.palette.harmonies.warm_attention)),
                    );
                    if let Some(bg) =
                        Rect::from_xywh(content_x, y - 2.0, max_content_w, font_size + 4.0)
                    {
                        let mut p = Paint::default();
                        p.set_color(code_bg);
                        pixmap.fill_rect(bg, &p, Transform::identity(), None);
                    }
                    self.draw_text_styled_clipped(
                        pixmap,
                        line,
                        content_x + 6.0,
                        y,
                        font_size,
                        code_fg,
                        FontStyle::Mono,
                        clip_right,
                    );
                    y += if is_hero { 22.0 } else { 19.0 };
                }
                MarkdownLine::Paragraph { spans } => {
                    let font_size = if is_hero { 18.0 } else { 15.0 };
                    y += self
                        .draw_markdown_spans(
                            pixmap,
                            spans,
                            content_x,
                            y,
                            font_size,
                            body_color,
                            clip_right,
                            max_y - y,
                            None,
                        )
                        .max(if is_hero { 24.0 } else { 20.0 });
                }
                MarkdownLine::IncompleteMath { .. } => {
                    self.draw_text_styled_clipped(
                        pixmap,
                        "Unclosed display equation ($$)",
                        content_x,
                        y,
                        13.0,
                        body_color,
                        FontStyle::Mono,
                        clip_right,
                    );
                    y += 20.0;
                }
                MarkdownLine::Empty => {
                    y += if is_hero { 10.0 } else { 8.0 };
                }
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Drawing keeps target, geometry, content and palette explicit at the rendering boundary"
    )]
    pub(super) fn draw_text_preview(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        ext_hint: Option<&str>,
        rx: f32,
        ry: f32,
        rw: f32,
        rh: f32,
        is_hero: bool,
        elision_color: Color,
        collect_previews: bool,
    ) -> Option<crate::PreparedTextPreview> {
        let inferred = ext_hint
            .is_none()
            .then(|| little_clip_core::diagram::Format::from_snippet(text))
            .flatten();
        if let Some(format) = ext_hint
            .and_then(|extension| {
                little_clip_core::diagram::Format::from_source_extension(extension, text)
            })
            .or(inferred)
        {
            let height = self.draw_diagram_preview(
                pixmap,
                format,
                text,
                rx + 14.0,
                ry + 124.0,
                (rw - 28.0).max(1.0),
                (rh - 150.0).max(1.0),
                self.color_from_hex(&self.palette.colors.text_primary),
                inferred.is_some(),
            );
            if height > 0.0 {
                return None;
            }
        }
        // A guessed diagram which does not render remains readable source. Its
        // syntax hint also keeps table heuristics from consuming partial code.
        let ext_hint = ext_hint.or_else(|| inferred.and_then(|format| format.source_extension()));
        if ext_hint.and_then(little_clip_core::syntax::map_extension_to_language) == Some("md")
            || (ext_hint.is_none() && little_clip_core::syntax::detect_markdown_text(text))
        {
            self.draw_markdown_preview(
                pixmap,
                text,
                rx,
                ry,
                rw,
                rh,
                is_hero,
                self.color_from_hex(&self.palette.harmonies.base),
                elision_color,
            );
            return None;
        }
        let clean_text = text.trim_start_matches(is_zero_width_or_bom);
        let trimmed = clean_text.trim();

        // 1. Unit conversion preview
        if let Some(conv) = units::detect_unit_conversion(trimmed) {
            self.draw_unit_conversion_card(pixmap, &conv, rx, ry, rw, rh, is_hero);
            return None;
        }

        // 2. IP or MAC network preview
        if let Some(net) = network_preview::detect_network_preview(trimmed) {
            self.draw_network_preview_card(pixmap, &net, rx, ry, rw, rh, is_hero);
            return None;
        }

        // 3. Timestamp preview
        if let Some(ts) = timestamps::detect_timestamp(trimmed) {
            self.draw_timestamp_card(pixmap, &ts, rx, ry, rw, rh, is_hero);
            return None;
        }

        // File-backed tables are prepared before rasterization. Inline text
        // may still describe a table unless its extension identifies source code.
        let spreadsheet_opt = if ext_hint
            .and_then(little_clip_core::syntax::map_extension_to_language)
            .is_some()
        {
            // Known text/code files follow the same extension-based route as
            // the workbench. Source delimiters are not evidence of a table.
            None
        } else {
            spreadsheet::detect_csv_text(trimmed)
        };
        if let Some(ref sheet_data) = spreadsheet_opt
            && let Some(first_sheet) = sheet_data.sheets.first()
        {
            self.draw_spreadsheet_card(
                pixmap,
                &sheet_data.title,
                first_sheet,
                rx,
                ry,
                rw,
                rh,
                is_hero,
            );
            return None;
        }

        let p = Path::new(trimmed);
        if self.draw_scene_file_card(pixmap, p, [rx, ry, rw, rh], is_hero) {
            return None;
        }

        self.draw_source_preview(
            pixmap,
            trimmed,
            little_clip_core::syntax::detect_language(trimmed, ext_hint),
            [rx, ry, rw, rh],
            is_hero,
            collect_previews,
        )
    }

    // Keep source painting separate from presentation classification. Both an
    // ordinary snippet and a rejected diagram use this exact fallback path.
    fn draw_source_preview(
        &self,
        pixmap: &mut Pixmap,
        trimmed: &str,
        lang_id: &str,
        [rx, ry, rw, rh]: [f32; 4],
        is_hero: bool,
        collect_previews: bool,
    ) -> Option<crate::PreparedTextPreview> {
        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let text_font_size = if is_hero { 18.0 } else { 15.0 };
        let line_height = if is_hero { 24.0 } else { 20.0 };

        // Classification above uses the normalized text, but the previous renderer
        // accidentally switched back to `text` here. A clipboard/file payload with
        // a leading newline or space therefore produced a visibly indented first
        // glyph in the tiny preview.
        let mut lines = trimmed.lines().peekable();
        let max_y = ry + rh - 22.0;
        let mut line_y = ry + 124.0;

        let syntax = find_syntax(lang_id);

        let theme = THEME_SET
            .themes
            .get("base16-eighties.dark")
            .or_else(|| THEME_SET.themes.values().next())
            .expect("default theme");

        let mut highlighter = HighlightLines::new(syntax, theme);
        // Resolve against the least favorable hero/prior card surface once.
        let preview_background = self.color_from_hex(if self.palette.is_dark {
            &self.palette.colors.surface_raised
        } else {
            &self.palette.colors.surface_panel
        });

        let mut prepared = collect_previews.then(crate::PreparedTextPreview::default);
        while let Some(line) = lines.next() {
            let is_near_bottom = line_y + line_height > max_y;
            if is_near_bottom && lines.peek().is_some() {
                let remaining = 1 + lines.count();
                let badge = format!(
                    "▼ +{} more lines ({})",
                    remaining,
                    little_clip_core::syntax::language_display_name(lang_id)
                );
                if let Some(prepared) = &mut prepared {
                    prepared.more_lines = remaining;
                }
                self.draw_footer_badge(
                    pixmap,
                    &badge,
                    content_x,
                    line_y,
                    text_font_size,
                    clip_right,
                );
                break;
            }

            if line_y > max_y {
                break;
            }

            let ranges = highlighter
                .highlight_line(line, &SYNTAX_SET)
                .unwrap_or_default();

            let mut prepared_line = Vec::new();
            let mut cx = content_x;
            for (style, token_str) in ranges {
                if cx >= clip_right {
                    break;
                }
                let hex = self.palette.map_syntax_color_hex(
                    style.foreground.r,
                    style.foreground.g,
                    style.foreground.b,
                );
                let preferred = self.color_from_hex(hex);
                let fg = self.contrast_color(preview_background, Some(preferred));
                let font_style = if style
                    .font_style
                    .contains(syntect::highlighting::FontStyle::BOLD)
                {
                    FontStyle::Bold
                } else {
                    FontStyle::Mono
                };
                if collect_previews {
                    prepared_line.push(crate::PreparedTextSpan {
                        text: token_str.chars().take(512).collect(),
                        color: [fg.red(), fg.green(), fg.blue(), fg.alpha()]
                            .map(|channel| (channel * 255.0).round() as u8),
                    });
                }
                cx = self.draw_text_styled_clipped(
                    pixmap,
                    token_str,
                    cx,
                    line_y,
                    text_font_size,
                    fg,
                    font_style,
                    clip_right,
                );
            }

            if let Some(prepared) = &mut prepared {
                prepared.lines.push(prepared_line);
            }
            line_y += line_height;
        }
        prepared
    }
}

impl ScreenRenderer {
    #[expect(
        clippy::too_many_arguments,
        reason = "Raster target, source, bounds and palette remain explicit at the drawing boundary"
    )]
    fn draw_diagram_preview(
        &self,
        pixmap: &mut Pixmap,
        format: little_clip_core::diagram::Format,
        source: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        foreground: Color,
        text_fallback: bool,
    ) -> f32 {
        let result = self
            .diagram_previews
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .thumbnail(format, source);
        match result {
            Ok(preview) => {
                let thumbnail = &preview.image;
                let warning_height = if preview.warnings.is_empty() {
                    0.0
                } else {
                    20.0
                };
                let scale = (width / thumbnail.width as f32)
                    .min((height - warning_height).max(1.0) / thumbnail.height as f32);
                let drawn_width = thumbnail.width as f32 * scale;
                let drawn_height = thumbnail.height as f32 * scale;
                if let Some(image) = PixmapRef::from_bytes(
                    &thumbnail.premultiplied_rgba,
                    thumbnail.width,
                    thumbnail.height,
                ) {
                    pixmap.draw_pixmap(
                        0,
                        0,
                        image,
                        &PixmapPaint::default(),
                        Transform::from_scale(scale, scale)
                            .post_translate(x + (width - drawn_width) / 2.0, y),
                        None,
                    );
                }
                if let Some(warning) = preview.warnings.first() {
                    self.draw_text_styled_clipped(
                        pixmap,
                        &format!("Warnings ({}): {warning}", preview.warnings.len()),
                        x,
                        y + drawn_height + 4.0,
                        12.0,
                        foreground,
                        FontStyle::Regular,
                        x + width,
                    );
                }
                drawn_height + warning_height + 10.0
            }
            Err(_) if text_fallback => 0.0,
            Err(error) => {
                self.draw_text_styled_clipped(
                    pixmap,
                    &format!("{}: {error}", format.name()),
                    x,
                    y,
                    13.0,
                    foreground,
                    FontStyle::Regular,
                    x + width,
                );
                20.0
            }
        }
    }
}
