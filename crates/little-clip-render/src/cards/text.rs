use super::*;

impl ScreenRenderer {
    /// TrueType text width measurement with styling.

    pub(super) fn draw_markdown_spans(
        &self,
        pixmap: &mut Pixmap,
        spans: &[crate::markdown::MarkdownSpan],
        mut cx: f32,
        y: f32,
        size: f32,
        default_color: Color,
        clip_right: f32,
    ) -> f32 {
        use crate::markdown::MarkdownSpan;
        let url_color = self.color_from_hex(&self.palette.url_color);
        let title_color = self.color_from_hex(&self.palette.colors.text_primary);
        let italic_color = self.color_from_hex_alpha(&self.palette.colors.text_muted, 240);
        let code_color = self.color_from_hex(&self.palette.undo_border);

        for span in spans {
            if cx >= clip_right {
                break;
            }
            match span {
                MarkdownSpan::Text(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        s,
                        cx,
                        y,
                        size,
                        default_color,
                        FontStyle::Regular,
                        clip_right,
                    );
                }
                MarkdownSpan::Bold(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        s,
                        cx,
                        y,
                        size,
                        title_color,
                        FontStyle::Bold,
                        clip_right,
                    );
                }
                MarkdownSpan::Italic(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        s,
                        cx,
                        y,
                        size,
                        italic_color,
                        FontStyle::Italic,
                        clip_right,
                    );
                }
                MarkdownSpan::BoldItalic(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        s,
                        cx,
                        y,
                        size,
                        title_color,
                        FontStyle::BoldItalic,
                        clip_right,
                    );
                }
                MarkdownSpan::Code(s) => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        s,
                        cx,
                        y,
                        size,
                        code_color,
                        FontStyle::Mono,
                        clip_right,
                    );
                }
                MarkdownSpan::Link { label, .. } => {
                    cx = self.draw_text_styled_clipped(
                        pixmap,
                        label,
                        cx,
                        y,
                        size,
                        url_color,
                        FontStyle::Regular,
                        clip_right,
                    );
                }
            }
        }
        cx
    }

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

        for (idx, line) in lines.iter().enumerate() {
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

            match line {
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

                    let cx = self.draw_text_styled_clipped(
                        pixmap,
                        lead_symbol,
                        content_x,
                        y,
                        head_size,
                        elision_color,
                        FontStyle::Bold,
                        clip_right,
                    );
                    self.draw_markdown_spans(
                        pixmap, spans, cx, y, head_size, head_color, clip_right,
                    );
                    y += if is_hero { 26.0 } else { 22.0 };
                }
                MarkdownLine::Bullet { indent, spans } => {
                    let indent_px = (*indent as f32 * 4.0).min(16.0);
                    let bullet_x = content_x + indent_px;
                    let cx = self.draw_text_styled_clipped(
                        pixmap,
                        "• ",
                        bullet_x,
                        y,
                        if is_hero { 18.0 } else { 15.0 },
                        border_color,
                        FontStyle::Bold,
                        clip_right,
                    );
                    let font_size = if is_hero { 18.0 } else { 15.0 };
                    self.draw_markdown_spans(
                        pixmap, spans, cx, y, font_size, body_color, clip_right,
                    );
                    y += if is_hero { 24.0 } else { 20.0 };
                }
                MarkdownLine::Numbered { number, spans } => {
                    let font_size = if is_hero { 18.0 } else { 15.0 };
                    let cx = self.draw_text_styled_clipped(
                        pixmap,
                        &format!("{} ", number),
                        content_x,
                        y,
                        font_size,
                        border_color,
                        FontStyle::Bold,
                        clip_right,
                    );
                    self.draw_markdown_spans(
                        pixmap, spans, cx, y, font_size, body_color, clip_right,
                    );
                    y += if is_hero { 24.0 } else { 20.0 };
                }
                MarkdownLine::Quote { spans } => {
                    if let Some(qr) =
                        Rect::from_xywh(content_x, y, 2.5, if is_hero { 18.0 } else { 15.0 })
                    {
                        let mut qp = Paint::default();
                        qp.set_color(self.color_from_hex(&self.palette.harmonies.base));
                        pixmap.fill_rect(qr, &qp, Transform::identity(), None);
                    }
                    let font_size = if is_hero { 17.5 } else { 14.5 };
                    let quote_fg = self.contrast_color(
                        self.color_from_hex(&self.palette.colors.canvas),
                        Some(self.color_from_hex(&self.palette.colors.text_secondary)),
                    );
                    self.draw_markdown_spans(
                        pixmap,
                        spans,
                        content_x + 10.0,
                        y,
                        font_size,
                        quote_fg,
                        clip_right,
                    );
                    y += if is_hero { 24.0 } else { 20.0 };
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
                    self.draw_markdown_spans(
                        pixmap, spans, content_x, y, font_size, body_color, clip_right,
                    );
                    y += if is_hero { 24.0 } else { 20.0 };
                }
                MarkdownLine::Empty => {
                    y += if is_hero { 10.0 } else { 8.0 };
                }
            }
        }
    }

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
        _elision_color: Color,
    ) {
        let clean_text = text.trim_start_matches(is_zero_width_or_bom);
        let trimmed = clean_text.trim();

        // 1. Unit conversion preview
        if let Some(conv) = units::detect_unit_conversion(trimmed) {
            self.draw_unit_conversion_card(pixmap, &conv, rx, ry, rw, rh, is_hero);
            return;
        }

        // 2. IP or MAC network preview
        if let Some(net) = network_preview::detect_network_preview(trimmed) {
            self.draw_network_preview_card(pixmap, &net, rx, ry, rw, rh, is_hero);
            return;
        }

        // 3. Timestamp preview
        if let Some(ts) = timestamps::detect_timestamp(trimmed) {
            self.draw_timestamp_card(pixmap, &ts, rx, ry, rw, rh, is_hero);
            return;
        }

        // 4. Spreadsheet preview (CSV text or spreadsheet file)
        let p = Path::new(trimmed);
        let spreadsheet_opt = if p.is_file() && spreadsheet::is_spreadsheet_file(p) {
            spreadsheet::load_spreadsheet(p).ok()
        } else if ext_hint
            .and_then(little_clip_core::syntax::map_extension_to_language)
            .is_some()
        {
            // Known text/code files follow the same extension-based route as
            // the workbench. Source delimiters are not evidence of a table.
            None
        } else {
            spreadsheet::detect_csv_text(trimmed)
        };
        if let Some(ref sheet_data) = spreadsheet_opt {
            if let Some(first_sheet) = sheet_data.sheets.first() {
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
                return;
            }
        }

        // 5. MIDI File Preview
        if p.is_file()
            && (p
                .extension()
                .and_then(|s| s.to_str())
                .map(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi"))
                .unwrap_or(false))
        {
            if let Ok(midi_data) = midi::MidiData::parse_file(p) {
                self.draw_midi_card(pixmap, &midi_data, rx, ry, rw, rh, is_hero);
                return;
            }
        }

        if self.draw_scene_file_card(pixmap, p, [rx, ry, rw, rh], is_hero) {
            return;
        }

        let content_x = rx + 14.0;
        let clip_right = rx + rw - 10.0;
        let text_font_size = if is_hero { 18.0 } else { 15.0 };
        let line_height = if is_hero { 24.0 } else { 20.0 };

        // Classification above uses the normalized text, but the previous renderer
        // accidentally switched back to `text` here. A clipboard/file payload with
        // a leading newline or space therefore produced a visibly indented first
        // glyph in the tiny preview.
        let lines: Vec<&str> = trimmed.lines().collect();
        let total_lines = lines.len();
        let max_y = ry + rh - 22.0;
        let mut line_y = ry + 124.0;

        let lang_id = little_clip_core::syntax::detect_language(trimmed, ext_hint);
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

        for (idx, line) in lines.iter().enumerate() {
            let is_near_bottom = line_y + line_height > max_y;
            let has_more = idx + 1 < total_lines;

            if is_near_bottom && has_more {
                let remaining = total_lines - idx;
                let badge = format!(
                    "▼ +{} more lines ({})",
                    remaining,
                    little_clip_core::syntax::language_display_name(lang_id)
                );
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

            line_y += line_height;
        }
    }
}
