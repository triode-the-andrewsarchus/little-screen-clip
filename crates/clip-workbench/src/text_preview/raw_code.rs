use super::*;

impl TextPreviewState {
    pub(super) fn source_fragment(&self, row: usize) -> (CodeFragment, &str) {
        let fragment = self.code_fragments[row];
        (
            fragment,
            &self.content[fragment.start_byte..fragment.end_byte],
        )
    }

    /// Measure a small batch per frame. Long logical lines are split into
    /// bounded UTF-8 fragments before they reach egui's text shaper.
    pub(super) fn measure_code_rows(
        &mut self,
        ui: &egui::Ui,
        width: f32,
        minimum_height: f32,
        font: &FontId,
    ) {
        let started = std::time::Instant::now();
        for _ in 0..64 {
            let row = self.code_rows.next_measurement;
            if row >= self.code_fragments.len() {
                break;
            }
            let (_, source) = self.source_fragment(row);
            let galley = ui.fonts(|fonts| {
                fonts.layout(source.to_owned(), font.clone(), Color32::WHITE, width)
            });
            self.code_rows
                .set_height(row, (galley.size().y + 4.0).max(minimum_height));
            self.code_rows.next_measurement += 1;
            if started.elapsed() >= std::time::Duration::from_millis(2) {
                break;
            }
        }
        if self.code_rows.next_measurement < self.code_fragments.len() {
            ui.ctx().request_repaint();
        }
    }

    /// Renders raw / code view with syntax highlighting, line numbers gutter, and search match highlighting.
    pub(super) fn render_raw_code_view(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        if !ui.ctx().wants_keyboard_input()
            && ui.input(|input| {
                input
                    .events
                    .iter()
                    .any(|event| matches!(event, egui::Event::Copy))
                    || (input.key_pressed(egui::Key::C)
                        && (input.modifiers.command || input.modifiers.ctrl))
            })
        {
            let text = self
                .code_selection
                .filter(|(anchor, focus)| anchor != focus)
                .map(|(anchor, focus)| &self.content[anchor.min(focus)..anchor.max(focus)])
                .unwrap_or(&self.content);
            ui.ctx().copy_text(text.to_owned());
            // The platform output serves egui's clipboard integration. The
            // workbench also writes through its desktop clipboard adapter.
            #[cfg(not(test))]
            if let Err(error) = little_clip_core::copy_text_to_system_clipboard(text) {
                tracing::debug!(%error, "failed to copy raw text selection to system clipboard");
            }
        }
        let syntax = find_syntax(self.language);
        let code_font = FontId::monospace(crate::ui_ext::MONO_FONT_SIZE);
        let (line_height, digit_width) = ui.fonts(|fonts| {
            (
                fonts.row_height(&code_font) + 4.0,
                fonts.glyph_width(&code_font, '0'),
            )
        });

        let show_gutter = self.show_line_numbers;
        let mut scroll_area = egui::ScrollArea::new([!self.wrap_lines, true])
            .id_source("text_code_virtual_scroll")
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded);

        // Unified editor surface frame
        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.surface_recessed))
            .stroke(Stroke::new(
                1.0_f32,
                hex_to_c32(&palette.colors.surface_panel),
            ))
            .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                let total_rows = self.code_fragments.len().max(1);
                let digits = self.line_first_fragment.len().to_string().len().max(2);
                // A horizontal layout shrink-wraps children by default.  That left the
                // virtual ScrollArea with a one-row-high clip rect on some egui backends.
                // Reserve the viewport explicitly before asking it to virtualize rows.
                let surface_size = ui.available_size();
                // The scroll area reports its actual content height below. Use that
                // measurement from the previous frame to avoid showing a minimap for
                // files that fit, while still allowing wrapping to affect overflow.
                let show_minimap = self.minimap_overflows;
                let minimap_width = if show_minimap { MINIMAP_WIDTH } else { 0.0 };
                let editor_width = (surface_size.x - minimap_width - 6.0).max(64.0);
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        Vec2::new(editor_width, surface_size.y.max(1.0)),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let gutter_width = if show_gutter {
                                digit_width * (digits + 1) as f32 + 12.0
                            } else {
                                0.0
                            };
                            let wrap_width = if self.wrap_lines {
                                (ui.available_width()
                                    - gutter_width
                                    - ui.spacing().scroll.allocated_width())
                                .max(digit_width)
                            } else {
                                f32::INFINITY
                            };
                            // egui rounds scroll offsets to physical pixels. A
                            // rounded boundary must still anchor the following
                            // source row when preceding estimates are updated.
                            let rounding_tolerance = 0.5 / ui.ctx().pixels_per_point();
                            let anchor_row = self
                                .code_rows
                                .row_at(self.code_scroll_offset + rounding_tolerance);
                            let anchor_inset = (self.code_scroll_offset
                                - self.code_rows.offset(anchor_row))
                            .max(0.0);
                            self.code_rows.reset(
                                total_rows,
                                wrap_width,
                                line_height,
                                ui.ctx().pixels_per_point(),
                            );
                            self.measure_code_rows(ui, wrap_width, line_height, &code_font);
                            let anchored_offset = self.code_rows.offset(anchor_row) + anchor_inset;
                            let requested_offset =
                                self.scroll_to_offset.take().unwrap_or_else(|| {
                                    self.scroll_to_line
                                        .take()
                                        .map(|line| self.code_rows.offset(line.saturating_sub(1)))
                                        .unwrap_or(anchored_offset)
                                });
                            scroll_area = scroll_area.vertical_scroll_offset(requested_offset);
                            let output = scroll_area.show_viewport(ui, |ui, viewport| {
                                let origin = ui.max_rect().min;
                                let mut row =
                                    self.code_rows.row_at(viewport.top() + rounding_tolerance);
                                // Keep row identities stable while the visible range changes.
                                ui.skip_ahead_auto_ids(row);
                                let pointer = ui.ctx().pointer_interact_pos();
                                let pressed = ui.input(|input| input.pointer.primary_pressed());
                                let down = ui.input(|input| input.pointer.primary_down());
                                let released = ui.input(|input| input.pointer.any_released());
                                let mut y = self.code_rows.offset(row);
                                while row < total_rows && y <= viewport.bottom() {
                                    let fragment = self.code_fragments[row];
                                    let source_line = self
                                        .line_first_fragment
                                        .partition_point(|first_fragment| *first_fragment <= row)
                                        .saturating_sub(1);
                                    let highlight_result = self.matches.as_mut().map(|spool| {
                                        line_highlight_ranges_from_spool(
                                            spool,
                                            self.active_match_idx,
                                            fragment.start_byte,
                                            fragment.end_byte,
                                        )
                                    });
                                    let (matches, search_read_error) = match highlight_result {
                                        Some(Ok(matches)) => (matches, None),
                                        Some(Err(error)) => (
                                            Vec::new(),
                                            Some(format!("could not read search results: {error}")),
                                        ),
                                        None => (Vec::new(), None),
                                    };
                                    let mut job = build_highlighted_layout_job(
                                        &self.content[fragment.start_byte..fragment.end_byte],
                                        syntax,
                                        palette,
                                        &matches,
                                    );
                                    if let Some(error) = search_read_error {
                                        self.fail_search(error);
                                    }
                                    job.wrap.max_width = wrap_width;
                                    let galley = ui.fonts(|fonts| fonts.layout_job(job));
                                    let line =
                                        &self.content[fragment.start_byte..fragment.end_byte];
                                    let height = (galley.size().y + 4.0).max(line_height);
                                    self.code_rows.set_height(row, height);
                                    let text_rect = egui::Rect::from_min_size(
                                        origin + egui::vec2(gutter_width, y + 2.0),
                                        galley.size(),
                                    );
                                    if show_gutter && self.line_first_fragment[source_line] == row {
                                        ui.painter().text(
                                            origin + egui::vec2(gutter_width - 10.0, y + 2.0),
                                            egui::Align2::RIGHT_TOP,
                                            format!("{}", source_line + 1),
                                            code_font.clone(),
                                            hex_to_c32(&palette.colors.text_muted),
                                        );
                                    }
                                    let response = ui
                                        .push_id(row, |ui| {
                                            ui.put(
                                                text_rect,
                                                egui::Label::new(galley.clone())
                                                    .selectable(false)
                                                    .sense(egui::Sense::click_and_drag()),
                                            )
                                        })
                                        .inner;
                                    if let Some(pointer) = pointer.filter(|point| {
                                        text_rect.contains(*point)
                                            && ui.clip_rect().contains(*point)
                                    }) {
                                        if (pressed && response.contains_pointer())
                                            || (self.code_selection_dragging && (down || released))
                                        {
                                            let cursor =
                                                galley.cursor_from_pos(pointer - text_rect.min);
                                            let local_byte = line
                                                .char_indices()
                                                .nth(cursor.ccursor.index)
                                                .map_or(line.len(), |(byte, _)| byte);
                                            let source_byte = fragment.start_byte + local_byte;
                                            if pressed {
                                                self.code_selection =
                                                    Some((source_byte, source_byte));
                                                self.code_selection_dragging = true;
                                            } else if let Some((_, focus)) =
                                                &mut self.code_selection
                                            {
                                                if *focus != source_byte {
                                                    *focus = source_byte;
                                                    ui.ctx().request_repaint();
                                                }
                                            }
                                        }
                                    }
                                    if let Some((anchor, focus)) = self.code_selection {
                                        paint_code_selection(
                                            ui,
                                            &galley,
                                            text_rect.min,
                                            line,
                                            fragment,
                                            anchor.min(focus),
                                            anchor.max(focus),
                                            hex_to_c32_alpha(&palette.harmonies.base, 90),
                                        );
                                    }
                                    y += height;
                                    row += 1;
                                }
                                if released {
                                    self.code_selection_dragging = false;
                                }
                                // `put` advances the layout cursor to the visible
                                // rows. Reserve the document from its origin,
                                // rather than adding its height after that cursor.
                                ui.expand_to_include_rect(egui::Rect::from_min_size(
                                    origin,
                                    egui::vec2(0.0, self.code_rows.total_height()),
                                ));
                            });
                            self.code_scroll_offset = output.state.offset.y;
                            self.minimap_fraction = (output.state.offset.y
                                / (output.content_size.y - output.inner_rect.height()).max(1.0))
                            .clamp(0.0, 1.0);
                            self.minimap_view_fraction = (output.inner_rect.height()
                                / output.content_size.y.max(1.0))
                            .clamp(0.0, 1.0);
                            self.minimap_content_height = output.content_size.y;
                            self.minimap_viewport_height = output.inner_rect.height();
                            self.minimap_overflows =
                                output.content_size.y > output.inner_rect.height();
                            if self.minimap_overflows {
                                self.minimap_lines.start(
                                    Arc::clone(&self.content),
                                    self.language.to_owned(),
                                    BackgroundJobs::global(),
                                );
                                if self.minimap_lines.poll() {
                                    ui.ctx().request_repaint();
                                } else if !self.minimap_lines.ready && !self.minimap_lines.failed {
                                    ui.ctx().request_repaint_after(
                                        std::time::Duration::from_millis(40),
                                    );
                                }
                            }
                        },
                    );
                    if show_minimap {
                        if let Some(fraction) = render_code_minimap(
                            ui,
                            &self.minimap_lines.lines,
                            &self.line_first_fragment,
                            &self.code_rows,
                            surface_size.y.max(1.0),
                            self.minimap_fraction,
                            self.minimap_view_fraction,
                            palette,
                        ) {
                            let max_offset = (self.minimap_content_height
                                - self.minimap_viewport_height)
                                .max(0.0);
                            self.scroll_to_offset = Some(
                                (fraction * self.minimap_content_height
                                    - self.minimap_viewport_height * 0.5)
                                    .clamp(0.0, max_offset),
                            );
                        }
                    }
                });
            });
    }
}

/// Projects document search ranges into one source line.
#[cfg(test)]
pub(super) fn line_highlight_ranges(
    matches: &[SearchMatch],
    active_index: usize,
    line_start: usize,
    line_end: usize,
) -> Vec<(usize, usize, bool)> {
    let first = matches.partition_point(|matched| matched.end_byte <= line_start);
    matches[first..]
        .iter()
        .enumerate()
        .take_while(|(_, matched)| matched.start_byte < line_end)
        .map(|(offset, matched)| {
            (
                matched.start_byte.max(line_start) - line_start,
                matched.end_byte.min(line_end) - line_start,
                first + offset == active_index,
            )
        })
        .collect()
}

pub(super) fn line_highlight_ranges_from_spool(
    spool: &mut SearchMatchSpool,
    active_index: u64,
    line_start: usize,
    line_end: usize,
) -> io::Result<Vec<(usize, usize, bool)>> {
    spool
        .ranges_for_fragment(line_start as u64, line_end as u64)?
        .map(|matched| {
            let (index, matched) = matched?;
            let start = usize::try_from(matched.start_byte).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "match start exceeds platform size",
                )
            })?;
            let end = usize::try_from(matched.end_byte).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "match end exceeds platform size",
                )
            })?;
            Ok((
                start.max(line_start) - line_start,
                end.min(line_end) - line_start,
                index == active_index,
            ))
        })
        .collect()
}

pub(super) fn paint_code_selection(
    ui: &egui::Ui,
    galley: &egui::Galley,
    galley_pos: egui::Pos2,
    line: &str,
    fragment: CodeFragment,
    selection_start: usize,
    selection_end: usize,
    color: Color32,
) {
    let start = selection_start.max(fragment.start_byte);
    let end = selection_end.min(fragment.end_byte);
    if start >= end {
        return;
    }
    let start_char = line[..start - fragment.start_byte].chars().count();
    let end_char = line[..end - fragment.start_byte].chars().count();
    let mut row_start = 0;
    for row in &galley.rows {
        let row_end = row_start + row.char_count_including_newline();
        let from = start_char.max(row_start).min(row_end) - row_start;
        let to = end_char.max(row_start).min(row_end) - row_start;
        if from < to {
            let rect = egui::Rect::from_min_max(
                galley_pos + egui::vec2(row.x_offset(from), row.min_y()),
                galley_pos + egui::vec2(row.x_offset(to), row.max_y()),
            );
            ui.painter().rect_filled(rect, egui::Rounding::ZERO, color);
        }
        row_start = row_end;
    }
}

/// Appends only the intersecting part of a syntax token with a search background.
pub(super) fn append_highlighted_token(
    job: &mut egui::text::LayoutJob,
    token: &str,
    token_start: usize,
    foreground: Color32,
    palette: &AppPalette,
    matches: &[(usize, usize, bool)],
) {
    let token_end = token_start + token.len();
    let mut boundaries = vec![token_start, token_end];
    for &(start, end, _) in matches {
        if start < token_end && end > token_start {
            boundaries.extend([start.max(token_start), end.min(token_end)]);
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    for range in boundaries.windows(2) {
        let matched = matches
            .iter()
            .find(|&&(start, end, _)| start <= range[0] && end >= range[1]);
        let background = match matched {
            Some((_, _, true)) => hex_to_c32(&palette.harmonies.complementary),
            Some(_) => hex_to_c32_alpha(&palette.diff_mod, 100),
            None => Color32::TRANSPARENT,
        };
        let color = if matched.is_some() {
            crate::ui_ext::contrast_text_c32(background, Some(foreground), palette)
        } else {
            foreground
        };
        job.append(
            &token[range[0] - token_start..range[1] - token_start],
            0.0,
            egui::TextFormat {
                font_id: FontId::monospace(MONO_FONT_SIZE),
                color,
                background,
                ..Default::default()
            },
        );
    }
}

/// Builds syntax highlighting with exact, precomputed search ranges.
pub fn build_highlighted_layout_job(
    text: &str,
    syntax: &SyntaxReference,
    palette: &AppPalette,
    match_ranges: &[(usize, usize, bool)],
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let syn_theme = &THEME_SET.themes["base16-eighties.dark"];
    let mut highlighter = HighlightLines::new(syntax, syn_theme);
    let mut byte_offset = 0;
    for line in text.split_inclusive('\n') {
        for (style, token) in highlighter
            .highlight_line(line, &SYNTAX_SET)
            .unwrap_or_default()
        {
            append_highlighted_token(
                &mut job,
                token,
                byte_offset,
                map_syntect_color_to_theme(style.foreground, palette),
                palette,
                match_ranges,
            );
            byte_offset += token.len();
        }
    }
    job
}
