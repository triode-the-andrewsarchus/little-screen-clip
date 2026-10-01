use super::*;

/// Precomputes compact syntax spans once, avoiding text layout and per-frame parsing.
pub(super) fn build_minimap_line(
    line: &str,
    highlighter: &mut HighlightLines<'static>,
) -> MinimapLine {
    let mut column = 0;
    let mut segments = Vec::new();
    for (style, token) in highlighter
        .highlight_line(line, &SYNTAX_SET)
        .unwrap_or_default()
    {
        let token_width = token.chars().count();
        let leading_space = token.chars().take_while(|ch| ch.is_whitespace()).count();
        let visible_start = column + leading_space;
        let visible_end = column + token.trim_end().chars().count();
        if visible_start < visible_end && visible_start < MINIMAP_COLUMNS {
            segments.push(MinimapSegment {
                start_column: visible_start,
                end_column: visible_end.min(MINIMAP_COLUMNS),
                color: style.foreground,
            });
        }
        column += token_width;
    }
    MinimapLine { segments }
}

pub(super) fn build_neutral_minimap_line(line: &str) -> MinimapLine {
    let mut first = None;
    let mut end = 0;
    for (column, character) in line.chars().take(MINIMAP_COLUMNS).enumerate() {
        if !character.is_whitespace() {
            first.get_or_insert(column);
            end = column + 1;
        }
    }
    MinimapLine {
        segments: first
            .map(|start_column| {
                vec![MinimapSegment {
                    start_column,
                    end_column: end,
                    color: MINIMAP_NEUTRAL_COLOR,
                }]
            })
            .unwrap_or_default(),
    }
}

pub(super) fn minimap_source_shape(
    source: &str,
    mut is_cancelled: impl FnMut() -> bool,
) -> Option<(usize, bool)> {
    let mut line_count = 1;
    let mut line_start = 0;
    let mut neutral_fallback = false;
    let mut bytes_until_cancel_check = 0;
    for (index, byte) in source.as_bytes().iter().enumerate() {
        if bytes_until_cancel_check == 0 && is_cancelled() {
            return None;
        }
        bytes_until_cancel_check += 1;
        if bytes_until_cancel_check == MINIMAP_CANCELLATION_CHECK_BYTES {
            bytes_until_cancel_check = 0;
        }
        if *byte == b'\n' {
            line_count += 1;
            neutral_fallback |= index - line_start > MINIMAP_MAX_SYNTAX_LINE_BYTES;
            line_start = index + 1;
        }
    }
    neutral_fallback |= source.len() - line_start > MINIMAP_MAX_SYNTAX_LINE_BYTES;
    (!is_cancelled()).then_some((line_count, neutral_fallback))
}

pub(super) fn build_minimap_lines(
    text: &std::sync::Weak<String>,
    syntax: &'static SyntaxReference,
    cancelled: &AtomicBool,
) -> MinimapResult {
    let Some(source) = text.upgrade() else {
        return MinimapResult::Cancelled;
    };
    let Some((line_count, neutral_fallback)) =
        minimap_source_shape(&source, || cancelled.load(Ordering::Acquire))
    else {
        return MinimapResult::Cancelled;
    };
    drop(source);
    if cancelled.load(Ordering::Acquire) {
        return MinimapResult::Cancelled;
    }
    let stride = line_count.div_ceil(MINIMAP_RETAINED_LINES).max(1);
    let mut highlighter = HighlightLines::new(syntax, &THEME_SET.themes["base16-eighties.dark"]);
    let mut lines = Vec::with_capacity(line_count.div_ceil(stride));
    let mut line_buffer = String::new();
    let mut cursor = 0;
    for source_line in 0..line_count {
        if cancelled.load(Ordering::Acquire) {
            return MinimapResult::Cancelled;
        }
        let Some(source) = text.upgrade() else {
            return MinimapResult::Cancelled;
        };
        let mut line_end = cursor;
        while line_end < source.len() {
            if cancelled.load(Ordering::Acquire) {
                return MinimapResult::Cancelled;
            }
            let chunk_end = (line_end + MINIMAP_CANCELLATION_CHECK_BYTES).min(source.len());
            if let Some(offset) = source.as_bytes()[line_end..chunk_end]
                .iter()
                .position(|byte| *byte == b'\n')
            {
                line_end += offset;
                break;
            }
            line_end = chunk_end;
        }
        let next_cursor = (line_end + usize::from(line_end < source.len())).min(source.len());
        if neutral_fallback {
            // Skipping syntax for only the oversized line would leave the
            // parser state unknown for every line after it.
            if cancelled.load(Ordering::Acquire) {
                return MinimapResult::Cancelled;
            }
            if source_line % stride == 0 {
                lines.push((
                    source_line,
                    build_neutral_minimap_line(&source[cursor..line_end]),
                ));
            }
            cursor = next_cursor;
            drop(source);
            continue;
        }
        line_buffer.clear();
        let mut copy_cursor = cursor;
        while copy_cursor < line_end {
            if cancelled.load(Ordering::Acquire) {
                return MinimapResult::Cancelled;
            }
            let mut copy_end = (copy_cursor + MINIMAP_CANCELLATION_CHECK_BYTES).min(line_end);
            while !source.is_char_boundary(copy_end) {
                copy_end -= 1;
            }
            if copy_end == copy_cursor {
                copy_end = (copy_cursor + 4).min(line_end);
                while !source.is_char_boundary(copy_end) {
                    copy_end += 1;
                }
            }
            line_buffer.push_str(&source[copy_cursor..copy_end]);
            copy_cursor = copy_end;
        }
        if next_cursor > line_end {
            // The syntax set uses newline-aware grammars. Python comments and
            // other line scopes need the source newline to terminate.
            line_buffer.push('\n');
        }
        cursor = next_cursor;
        // Syntect needs a complete logical line to preserve grammar state. Do
        // not keep the full document Arc alive while that line is highlighted.
        drop(source);
        if cancelled.load(Ordering::Acquire) {
            return MinimapResult::Cancelled;
        }
        let minimap_line = build_minimap_line(&line_buffer, &mut highlighter);
        if cancelled.load(Ordering::Acquire) {
            return MinimapResult::Cancelled;
        }
        if source_line % stride == 0 {
            lines.push((source_line, minimap_line));
        }
        if line_buffer.capacity() > MINIMAP_SCRATCH_REUSE_LIMIT {
            line_buffer = String::new();
        }
    }
    if cancelled.load(Ordering::Acquire) {
        MinimapResult::Cancelled
    } else {
        MinimapResult::Lines(lines)
    }
}

/// Draws a VS Code-style overview from syntax-colored rectangles, not glyphs.
pub(super) fn render_code_minimap(
    ui: &mut egui::Ui,
    lines: &[(usize, MinimapLine)],
    line_first_fragment: &[usize],
    rows: &CodeRowHeights,
    height: f32,
    scroll_fraction: f32,
    view_fraction: f32,
    palette: &AppPalette,
) -> Option<f32> {
    let (map_rect, response) = ui.allocate_exact_size(
        Vec2::new(MINIMAP_WIDTH, height),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter();
    painter.rect_filled(
        map_rect,
        Rounding::same(3.0),
        hex_to_c32(&palette.colors.surface_panel),
    );

    let content_rect = map_rect.shrink2(Vec2::new(4.0, 3.0));
    let line_count = line_first_fragment.len().max(1);
    // Do not emit thousands of tiny shapes for large sources. Each sampled row
    // still represents its source position, retaining the document's color rhythm.
    let stride =
        ((lines.len() as f32 / content_rect.height().ceil().max(1.0)).ceil() as usize).max(1);
    for (sample_index, (line_index, line)) in lines.iter().enumerate().step_by(stride) {
        let display_row = line_first_fragment.get(*line_index).copied().unwrap_or(0);
        let row_top = content_rect.top()
            + content_rect.height() * rows.offset(display_row) / rows.total_height().max(1.0);
        let next_line = lines
            .get(sample_index + stride)
            .map(|(line, _)| *line)
            .unwrap_or(line_count);
        let next_display_row = line_first_fragment
            .get(next_line)
            .copied()
            .unwrap_or(rows.heights.len());
        let row_bottom = (content_rect.top()
            + content_rect.height() * rows.offset(next_display_row) / rows.total_height().max(1.0))
        .max(row_top + 1.0)
        .min(content_rect.bottom());
        for segment in &line.segments {
            let x_start = content_rect.left()
                + content_rect.width() * segment.start_column as f32 / MINIMAP_COLUMNS as f32;
            let x_end = content_rect.left()
                + content_rect.width() * segment.end_column as f32 / MINIMAP_COLUMNS as f32;
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x_start, row_top),
                    egui::pos2(x_end, row_bottom),
                ),
                Rounding::ZERO,
                map_syntect_color_to_theme(segment.color, palette),
            );
        }
    }

    let view_height = (content_rect.height() * view_fraction)
        .clamp(12.0_f32.min(content_rect.height()), content_rect.height());
    let view_top = content_rect.top()
        + (content_rect.height() - view_height) * scroll_fraction.clamp(0.0, 1.0);
    let view_rect = egui::Rect::from_min_size(
        egui::pos2(content_rect.left(), view_top),
        Vec2::new(content_rect.width(), view_height),
    );
    painter.rect_filled(
        view_rect,
        Rounding::ZERO,
        hex_to_c32_alpha(&palette.harmonies.base, 45),
    );
    painter.rect_stroke(
        view_rect,
        Rounding::ZERO,
        Stroke::new(1.0_f32, hex_to_c32(&palette.harmonies.base)),
    );

    response
        .interact_pointer_pos()
        .filter(|_| response.clicked() || response.dragged())
        .map(|pointer| {
            ((pointer.y - content_rect.top()) / content_rect.height().max(1.0)).clamp(0.0, 1.0)
        })
}
impl MinimapCache {
    pub(super) fn start(&mut self, text: Arc<String>, syntax_name: String, jobs: &BackgroundJobs) {
        if self.started {
            return;
        }
        self.started = true;
        let weak_text = Arc::downgrade(&text);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (ticket, receiver) = jobs.submit(std::time::Duration::MAX, move || {
            build_minimap_lines(&weak_text, find_syntax(&syntax_name), &worker_cancelled)
        });
        self.job = Some((ticket, receiver));
        self.cancelled = Some(cancelled);
    }

    pub(super) fn poll(&mut self) -> bool {
        let Some((_, receiver)) = &self.job else {
            return false;
        };
        match receiver.try_recv() {
            Ok(Ok(MinimapResult::Lines(lines))) => {
                self.lines = lines;
                self.job = None;
                self.cancelled = None;
                self.ready = true;
                true
            }
            Ok(Err(JobError::QueueFull)) => {
                self.job = None;
                self.cancelled = None;
                self.started = false;
                false
            }
            Ok(Ok(MinimapResult::Cancelled))
            | Ok(Err(_))
            | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.job = None;
                self.cancelled = None;
                self.failed = true;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
        }
    }

    pub(super) fn cancel(&self) {
        if let Some(cancelled) = &self.cancelled {
            cancelled.store(true, Ordering::Release);
        }
        if let Some((ticket, _)) = &self.job {
            ticket.cancel();
        }
    }
}

impl Drop for MinimapCache {
    fn drop(&mut self) {
        // Queued jobs hold only a Weak source reference. Running work checks
        // cancellation between lines and owns only its current logical-line
        // scratch buffer while syntect processes that line.
        self.cancel();
    }
}
