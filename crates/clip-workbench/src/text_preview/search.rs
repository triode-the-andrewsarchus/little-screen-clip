use super::*;

impl TextPreviewState {
    /// Recalculates search matches based on query, regex, and case sensitivity.
    pub fn update_search_matches(&mut self) {
        if let Some(job) = self.search_job.take() {
            job.cancel();
        }
        self.search_revision = self.search_revision.wrapping_add(1);
        self.matches = None;
        self.match_count = 0;
        self.regex_error = None;
        self.search_error = None;
        self.search_retry_at = None;
        if self.find_query.is_empty() {
            self.search_pending = false;
            self.search_debounce_until = None;
            self.active_match_idx = 0;
            return;
        }
        self.search_pending = true;
        self.search_debounce_until = Some(Instant::now() + SEARCH_DEBOUNCE);
    }

    pub(super) fn service_search_jobs(
        &mut self,
        ctx: &egui::Context,
        jobs: &BackgroundJobs,
        now: Instant,
    ) {
        self.poll_search_job(now);
        if !self.search_pending {
            return;
        }
        if self.search_job.is_some() {
            ctx.request_repaint_after(SEARCH_RETRY_DELAY);
            return;
        }
        if let Some(until) = self
            .search_debounce_until
            .take()
            .filter(|until| *until > now)
        {
            ctx.request_repaint_after(until.saturating_duration_since(now));
            self.search_debounce_until = Some(until);
            return;
        }
        if let Some(until) = self.search_retry_at.take().filter(|until| *until > now) {
            ctx.request_repaint_after(until.saturating_duration_since(now));
            self.search_retry_at = Some(until);
            return;
        }

        let revision = self.search_revision;
        let content = Arc::clone(&self.content);
        let query = self.find_query.clone();
        let is_regex = self.is_regex;
        let case_sensitive = self.case_sensitive;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let (ticket, receiver) = jobs.submit(Duration::MAX, move || {
            build_search_matches(content, query, is_regex, case_sensitive, &worker_cancelled)
        });
        self.search_job = Some(SearchMatchJob {
            revision,
            ticket,
            cancelled,
            receiver,
        });
        ctx.request_repaint_after(SEARCH_RETRY_DELAY);
    }

    pub(super) fn poll_search_job(&mut self, now: Instant) {
        let result = self.search_job.as_ref().map(|job| job.receiver.try_recv());
        let result = match result {
            Some(Ok(result)) => result,
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => return,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                let job = self.search_job.take().expect("polled search job");
                if job.revision == self.search_revision {
                    self.search_pending = false;
                    self.fail_search("search worker stopped before returning results".into());
                }
                return;
            }
        };
        let job = self.search_job.take().expect("completed search job");
        if job.revision != self.search_revision {
            return;
        }
        match result {
            Ok(Ok(SearchBuildOutput::Matches(spool))) => {
                self.match_count = spool.len();
                self.matches = Some(spool);
                self.active_match_idx = if self.active_match_idx >= self.match_count {
                    0
                } else {
                    self.active_match_idx
                };
                self.search_pending = false;
                self.search_retry_at = None;
            }
            Ok(Ok(SearchBuildOutput::InvalidRegex(error))) => {
                self.active_match_idx = 0;
                self.regex_error = Some(error);
                self.search_pending = false;
            }
            Ok(Ok(SearchBuildOutput::Cancelled)) | Err(JobError::QueueFull) => {
                self.search_pending = true;
                self.search_retry_at = Some(now + SEARCH_RETRY_DELAY);
            }
            Ok(Err(error)) => {
                self.fail_search(error);
                self.search_pending = false;
            }
            Err(JobError::Cancelled) => {
                self.search_pending = false;
            }
            Err(error) => {
                self.fail_search(error.to_string());
                self.search_pending = false;
            }
        }
    }

    pub(super) fn search_match_at(&mut self, index: u64) -> Option<SearchMatch> {
        if index >= self.match_count {
            return None;
        }
        let range = match self.matches.as_mut()?.get(index) {
            Ok(range) => range,
            Err(error) => {
                self.fail_search(format!("could not read search results: {error}"));
                return None;
            }
        };
        let (start_byte, end_byte) = match (
            usize::try_from(range.start_byte),
            usize::try_from(range.end_byte),
        ) {
            (Ok(start), Ok(end)) => (start, end),
            _ => {
                self.fail_search("search result exceeds platform address size".into());
                return None;
            }
        };
        // Each source line's first display fragment retains its exact start
        // byte, so use that index instead of keeping a duplicate line-start Vec.
        let line = self.line_first_fragment.partition_point(|first_fragment| {
            self.code_fragments[*first_fragment].start_byte <= start_byte
        });
        Some(SearchMatch {
            start_byte,
            end_byte,
            line,
        })
    }

    pub(super) fn fail_search(&mut self, error: String) {
        self.matches = None;
        self.match_count = 0;
        self.active_match_idx = 0;
        self.search_error = Some(error);
    }

    #[cfg(test)]
    pub fn match_count(&self) -> u64 {
        self.match_count
    }

    #[cfg(test)]
    pub fn search_match(&mut self, index: u64) -> Option<SearchMatch> {
        self.search_match_at(index)
    }

    /// Advances to the next search match.
    pub fn next_match(&mut self) {
        if self.match_count > 0 {
            self.active_match_idx = (self.active_match_idx + 1) % self.match_count;
            if let Some(matched) = self.search_match_at(self.active_match_idx) {
                self.scroll_to_line = Some(self.display_row_for_byte(matched.start_byte) + 1);
            }
        }
    }

    /// Moves to the previous search match.
    pub fn prev_match(&mut self) {
        if self.match_count > 0 {
            if self.active_match_idx == 0 {
                self.active_match_idx = self.match_count - 1;
            } else {
                self.active_match_idx -= 1;
            }
            if let Some(matched) = self.search_match_at(self.active_match_idx) {
                self.scroll_to_line = Some(self.display_row_for_byte(matched.start_byte) + 1);
            }
        }
    }

    pub(super) fn render_search_status_label(&self, ui: &mut egui::Ui, palette: &AppPalette) {
        let surface = hex_to_c32(&palette.colors.surface_recessed);
        if self.find_query.is_empty() {
            return;
        }
        if let Some(ref err) = self.regex_error {
            ui.horizontal_wrapped(|ui| {
                let error_color = crate::ui_ext::contrast_text_c32_on(
                    surface,
                    surface,
                    Some(hex_to_c32(&palette.diff_del)),
                    palette,
                );
                Icon::Alert.ui(ui, 13.0, error_color);
                ui.label(
                    egui::RichText::new(format!("Regex error: {}", err))
                        .size(crate::ui_ext::BODY_FONT_SIZE)
                        .color(error_color),
                );
            });
        } else if let Some(ref err) = self.search_error {
            ui.label(
                egui::RichText::new(format!("Search failed: {err}"))
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(hex_to_c32(&palette.diff_del)),
            );
        } else if self.search_pending {
            ui.label(
                egui::RichText::new("Searching…")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(hex_to_c32(&palette.colors.text_muted)),
            );
        } else if self.match_count == 0 {
            ui.label(
                egui::RichText::new("No matches")
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .color(hex_to_c32(&palette.colors.text_muted)),
            );
        } else {
            let count_text = format!("{}/{} matches", self.active_match_idx + 1, self.match_count);
            ui.label(
                egui::RichText::new(count_text)
                    .size(crate::ui_ext::BODY_FONT_SIZE)
                    .strong()
                    .color(crate::ui_ext::contrast_text_c32_on(
                        surface,
                        surface,
                        Some(hex_to_c32(&palette.harmonies.complementary)),
                        palette,
                    )),
            );
        }
    }

    pub(super) fn render_search_options(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        let case_btn = crate::mode_tab_button("Aa", self.case_sensitive, palette);
        if ui
            .add(case_btn)
            .on_hover_text("Match Case (Case Sensitive)")
            .clicked()
        {
            self.case_sensitive = !self.case_sensitive;
            self.update_search_matches();
        }

        let regex_btn = crate::mode_tab_button(".*", self.is_regex, palette);
        if ui
            .add(regex_btn)
            .on_hover_text("Use Regular Expression")
            .clicked()
        {
            self.is_regex = !self.is_regex;
            self.update_search_matches();
        }
    }

    pub(super) fn render_search_toolbar(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        ui.add_space(crate::ui_ext::SPACE_SM);
        egui::Frame::none()
            .fill(hex_to_c32(&palette.colors.surface_recessed))
            .stroke(Stroke::new(
                1.0_f32,
                hex_to_c32(&palette.colors.surface_panel),
            ))
            .rounding(Rounding::same(crate::ui_ext::PANEL_CORNER_RADIUS))
            .inner_margin(egui::Margin::symmetric(
                crate::ui_ext::BUTTON_PADDING_X,
                crate::ui_ext::BUTTON_PADDING_Y,
            ))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    Icon::Search.ui(ui, 15.0, hex_to_c32(&palette.colors.text_secondary));

                    let search_edit = egui::TextEdit::singleline(&mut self.find_query)
                        .hint_text("Find in text (Enter for next, Shift+Enter for prev)...");

                    let search_width = ui.available_width().min(280.0).max(120.0);
                    let response =
                        ui.add_sized([search_width, crate::ui_ext::CONTROL_HEIGHT], search_edit);
                    if self.focus_find_input {
                        response.request_focus();
                        self.focus_find_input = false;
                    }

                    if response.changed() {
                        self.update_search_matches();
                    }

                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if ui.input(|i| i.modifiers.shift) {
                            self.prev_match();
                        } else {
                            self.next_match();
                        }
                    }

                    self.render_search_status_label(ui, palette);

                    let prev_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::ArrowUp),
                        "Prev",
                        hex_to_c32(&palette.colors.surface_panel),
                        palette,
                    );
                    if ui.add(prev_btn).clicked() {
                        self.prev_match();
                    }

                    let next_btn = crate::ui_ext::secondary_button_icon(
                        Some(Icon::ArrowDown),
                        "Next",
                        hex_to_c32(&palette.colors.surface_panel),
                        palette,
                    );
                    if ui.add(next_btn).clicked() {
                        self.next_match();
                    }

                    ui.add_space(crate::ui_ext::SPACE_SM);
                    self.render_search_options(ui, palette);
                });
            });
    }
}

pub(super) const SEARCH_MATCH_RECORD_BYTES: usize = 16;
pub(super) const SEARCH_MATCH_MEMORY_LIMIT_BYTES: usize = 64 * 1024;
pub(super) const SEARCH_DEBOUNCE: Duration = Duration::from_millis(80);
pub(super) const SEARCH_RETRY_DELAY: Duration = Duration::from_millis(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SearchMatchRange {
    pub(super) start_byte: u64,
    pub(super) end_byte: u64,
}

/// Stores fixed-size match ranges in memory until the bounded threshold, then spills to cache.
pub(super) struct SearchMatchSpool {
    pub(super) file: tempfile::SpooledTempFile,
    pub(super) record_count: u64,
    pub(super) cursor_at_end: bool,
    pub(super) last_range: Option<SearchMatchRange>,
}

pub(super) struct SearchMatchJob {
    pub(super) revision: u64,
    pub(super) ticket: JobTicket,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) receiver: Receiver<Result<Result<SearchBuildOutput, String>, JobError>>,
}

impl Drop for SearchMatchJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub(super) enum SearchBuildOutput {
    Matches(SearchMatchSpool),
    InvalidRegex(String),
    Cancelled,
}

impl SearchMatchJob {
    pub(super) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.ticket.cancel();
    }
}

pub(super) fn build_search_matches(
    content: Arc<String>,
    query: String,
    is_regex: bool,
    case_sensitive: bool,
    cancelled: &AtomicBool,
) -> Result<SearchBuildOutput, String> {
    if cancelled.load(Ordering::Acquire) {
        return Ok(SearchBuildOutput::Cancelled);
    }
    let pattern = if is_regex {
        query
    } else {
        regex::escape(&query)
    };
    let regex = match RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
    {
        Ok(regex) => regex,
        Err(error) => return Ok(SearchBuildOutput::InvalidRegex(error.to_string())),
    };
    let mut spool = SearchMatchSpool::new().map_err(|error| error.to_string())?;
    for (index, matched) in regex.find_iter(content.as_str()).enumerate() {
        if index % 256 == 0 && cancelled.load(Ordering::Acquire) {
            return Ok(SearchBuildOutput::Cancelled);
        }
        spool
            .append(matched.start() as u64, matched.end() as u64)
            .map_err(|error| error.to_string())?;
    }
    if cancelled.load(Ordering::Acquire) {
        return Ok(SearchBuildOutput::Cancelled);
    }
    Ok(SearchBuildOutput::Matches(spool))
}

impl SearchMatchSpool {
    pub(super) fn new() -> io::Result<Self> {
        let directory =
            little_clip_core::staging::get_staging_dir().map_err(|error| match error {
                little_clip_core::staging::StagingError::Io(error) => error,
            })?;
        Self::new_in(&directory)
    }

    pub(super) fn new_in(directory: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        Ok(Self {
            file: tempfile::spooled_tempfile_in(SEARCH_MATCH_MEMORY_LIMIT_BYTES, directory),
            record_count: 0,
            cursor_at_end: true,
            last_range: None,
        })
    }

    pub(super) fn len(&self) -> u64 {
        self.record_count
    }

    pub(super) fn append(&mut self, start_byte: u64, end_byte: u64) -> io::Result<()> {
        if start_byte > end_byte
            || self
                .last_range
                .is_some_and(|last| start_byte < last.start_byte || end_byte < last.end_byte)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "match ranges must be ordered and non-reversed",
            ));
        }
        let record_count = self
            .record_count
            .checked_add(1)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "match count overflow"))?;
        let byte_len = record_count
            .checked_mul(SEARCH_MATCH_RECORD_BYTES as u64)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "match spool size overflow")
            })?;
        let record = encode_search_match_range(start_byte, end_byte);

        if byte_len > SEARCH_MATCH_MEMORY_LIMIT_BYTES as u64 && !self.file.is_rolled() {
            self.file.roll()?;
        }
        if !self.cursor_at_end {
            let offset = self.record_count * SEARCH_MATCH_RECORD_BYTES as u64;
            self.file.seek(SeekFrom::Start(offset))?;
        }
        self.cursor_at_end = false;
        self.file.write_all(&record)?;
        self.cursor_at_end = true;
        self.record_count = record_count;
        self.last_range = Some(SearchMatchRange {
            start_byte,
            end_byte,
        });
        Ok(())
    }

    pub(super) fn get(&mut self, index: u64) -> io::Result<SearchMatchRange> {
        if index >= self.record_count {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "match index is out of bounds",
            ));
        }
        let offset = index * SEARCH_MATCH_RECORD_BYTES as u64;
        let mut record = [0; SEARCH_MATCH_RECORD_BYTES];
        self.cursor_at_end = false;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.read_exact(&mut record)?;
        self.cursor_at_end = index + 1 == self.record_count;
        Ok(decode_search_match_range(record))
    }

    /// Finds matches intersecting one half-open source fragment using constant iterator storage.
    pub(super) fn ranges_for_fragment(
        &mut self,
        fragment_start: u64,
        fragment_end: u64,
    ) -> io::Result<SearchMatchRangeIter<'_>> {
        let mut low = 0;
        let mut high = self.record_count;
        if fragment_start >= fragment_end {
            low = high;
        } else {
            while low < high {
                let middle = low + (high - low) / 2;
                if self.get(middle)?.end_byte <= fragment_start {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
        }
        Ok(SearchMatchRangeIter {
            spool: self,
            next_index: low,
            fragment_start,
            fragment_end,
            finished: false,
            needs_seek: true,
        })
    }

    #[cfg(test)]
    pub(super) fn is_rolled(&self) -> bool {
        self.file.is_rolled()
    }
}

/// Streams intersecting ranges without allocating a fragment-sized match vector.
pub(super) struct SearchMatchRangeIter<'a> {
    pub(super) spool: &'a mut SearchMatchSpool,
    pub(super) next_index: u64,
    pub(super) fragment_start: u64,
    pub(super) fragment_end: u64,
    pub(super) finished: bool,
    pub(super) needs_seek: bool,
}

impl Iterator for SearchMatchRangeIter<'_> {
    type Item = io::Result<(u64, SearchMatchRange)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        while self.next_index < self.spool.len() {
            let index = self.next_index;
            self.next_index += 1;
            if self.needs_seek {
                let offset = index * SEARCH_MATCH_RECORD_BYTES as u64;
                self.spool.cursor_at_end = false;
                if let Err(error) = self.spool.file.seek(SeekFrom::Start(offset)) {
                    self.finished = true;
                    return Some(Err(error));
                }
                self.needs_seek = false;
            }
            let mut record = [0; SEARCH_MATCH_RECORD_BYTES];
            self.spool.cursor_at_end = false;
            let range = match self.spool.file.read_exact(&mut record) {
                Ok(()) => {
                    let range = decode_search_match_range(record);
                    self.spool.cursor_at_end = index + 1 == self.spool.len();
                    range
                }
                Err(error) => {
                    self.finished = true;
                    return Some(Err(error));
                }
            };
            if range.start_byte >= self.fragment_end {
                self.finished = true;
                return None;
            }
            // Existing projection hides matches on either edge but keeps interior zero-width hits.
            if range.end_byte > self.fragment_start && range.start_byte < self.fragment_end {
                return Some(Ok((index, range)));
            }
        }
        self.finished = true;
        None
    }
}

pub(super) fn encode_search_match_range(
    start_byte: u64,
    end_byte: u64,
) -> [u8; SEARCH_MATCH_RECORD_BYTES] {
    let mut record = [0; SEARCH_MATCH_RECORD_BYTES];
    record[..8].copy_from_slice(&start_byte.to_le_bytes());
    record[8..].copy_from_slice(&end_byte.to_le_bytes());
    record
}

pub(super) fn decode_search_match_range(
    record: [u8; SEARCH_MATCH_RECORD_BYTES],
) -> SearchMatchRange {
    let mut start = [0; 8];
    let mut end = [0; 8];
    start.copy_from_slice(&record[..8]);
    end.copy_from_slice(&record[8..]);
    SearchMatchRange {
        start_byte: u64::from_le_bytes(start),
        end_byte: u64::from_le_bytes(end),
    }
}
