use super::*;
use crate::ui_tabs::{Tab, TabBar};
use little_clip_core::pdf::{PdfOutlineEntry, extract_pdf_page_text, read_pdf_outline};
use std::sync::mpsc::channel;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum LeftTab {
    #[default]
    Pages,
    Contents,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RightTab {
    Find,
    Ocr,
}

enum IndexMessage {
    Outline(Result<Vec<PdfOutlineEntry>, String>),
    Page(usize, String),
}
type IndexWorker = (JobTicket, Receiver<Result<Result<(), String>, JobError>>);

#[derive(Default)]
pub(super) struct PdfNavigation {
    pub left: LeftTab,
    pub right: Option<RightTab>,
    pub query: String,
    contents_query: String,
    focus_find: bool,
    focus_contents: bool,
    find_cursor: usize,
    contents_cursor: usize,
    outline: Vec<PdfOutlineEntry>,
    text: Vec<String>,
    attempted: bool,
    error: Option<String>,
    worker: Option<IndexWorker>,
    messages: Option<Receiver<IndexMessage>>,
    cancel: Option<Arc<AtomicBool>>,
    result_cache: Option<(String, Vec<(usize, String)>)>,
}

impl PdfNavigation {
    fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::SeqCst);
        }
        if let Some((ticket, _)) = self.worker.take() {
            ticket.cancel();
        }
        self.messages = None;
    }

    fn start(
        &mut self,
        path: PathBuf,
        count: usize,
        ctx: &egui::Context,
        check: crate::file_source::ReadCheck,
    ) {
        if self.worker.is_some() || count == 0 {
            return;
        }
        self.attempted = true;
        self.text.resize(count, String::new());
        self.result_cache = None;
        self.error = None;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        self.cancel = Some(cancel);
        let (tx, rx) = channel();
        self.messages = Some(rx);
        let repaint = ctx.clone();
        self.worker = Some(BackgroundJobs::global().submit(Duration::MAX, move || {
            let result = (|| {
                let _ = tx.send(IndexMessage::Outline(
                    check.read(&worker_cancel, || read_pdf_outline(&path)),
                ));
                for page in 1..=count {
                    if worker_cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let text = check.read(&worker_cancel, || extract_pdf_page_text(&path, page))?;
                    if worker_cancel.load(Ordering::SeqCst) {
                        break;
                    }
                    let _ = tx.send(IndexMessage::Page(page, text));
                }
                Ok(())
            })();
            repaint.request_repaint();
            result
        }));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let result = self.worker.as_ref().map(|(_, rx)| rx.try_recv());
        if let Some(rx) = &self.messages {
            while let Ok(message) = rx.try_recv() {
                match message {
                    IndexMessage::Outline(Ok(outline)) => self.outline = outline,
                    IndexMessage::Outline(Err(error)) => {
                        self.error = Some(format!("Contents: {error}"))
                    }
                    IndexMessage::Page(page, text) => {
                        self.text[page - 1] = text;
                        self.result_cache = None;
                    }
                }
            }
        }
        match result {
            Some(Ok(result)) => {
                if let Err(error) = result
                    .map_err(|error| error.to_string())
                    .and_then(|result| result)
                {
                    self.error = Some(error);
                }
                self.cancel();
            }
            Some(Err(TryRecvError::Disconnected)) => {
                self.error = Some("PDF index worker stopped".into());
                self.cancel();
            }
            _ => {}
        }
        if self.worker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    pub fn left_tabs(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let tabs = [
            Tab::new(LeftTab::Pages, "Pages").icon(Icon::Pdf),
            Tab::new(LeftTab::Contents, "ToC").icon(Icon::Book),
        ];
        if let Some(tab) = TabBar::new("pdf-left-tabs", self.left, &tabs, palette)
            .equal_widths()
            .show(ui)
        {
            self.left = tab;
            self.focus_contents = tab == LeftTab::Contents;
        }
    }

    fn contents(&self) -> Vec<(usize, String)> {
        let mut entries = self
            .outline
            .iter()
            .filter_map(|entry| {
                fuzzy_score(&entry.title, &self.contents_query).map(|score| (score, entry))
            })
            .collect::<Vec<_>>();
        if !self.contents_query.is_empty() {
            entries.sort_by_key(|(score, _)| *score);
        }
        entries
            .into_iter()
            .map(|(_, entry)| {
                (
                    entry.page,
                    format!(
                        "{}{} · {}",
                        "  ".repeat(entry.depth.min(8)),
                        entry.title,
                        entry.page
                    ),
                )
            })
            .collect()
    }

    pub fn render_contents(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<usize> {
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.contents_query)
                .hint_text("Fuzzy search ToC…")
                .desired_width(f32::INFINITY),
        );
        if self.focus_contents {
            response.request_focus();
            self.focus_contents = false;
        }
        if response.changed() {
            self.contents_cursor = 0;
        }
        let results = self.contents();
        if results.is_empty() {
            ui.label(if self.worker.is_some() {
                "Reading contents…"
            } else {
                "No matching bookmarks"
            });
        }
        render_result_list(
            ui,
            ctx,
            &results,
            &mut self.contents_cursor,
            response.has_focus() || response.lost_focus(),
        )
    }

    fn find_results(&self) -> Vec<(usize, String)> {
        let query = self.query.to_lowercase();
        if query.trim().is_empty() {
            return Vec::new();
        }
        self.text
            .iter()
            .enumerate()
            .filter_map(|(index, text)| {
                let count = text.to_lowercase().matches(&query).count();
                if count == 0 {
                    return None;
                }
                let snippet = text
                    .lines()
                    .find(|line| line.to_lowercase().contains(&query))
                    .unwrap_or(text)
                    .chars()
                    .take(120)
                    .collect::<String>();
                Some((
                    index + 1,
                    format!(
                        "Page {} · {} match{}\n{}",
                        index + 1,
                        count,
                        if count == 1 { "" } else { "es" },
                        snippet
                    ),
                ))
            })
            .collect()
    }

    fn render_find(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<usize> {
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text("Find in PDF…")
                .desired_width(f32::INFINITY),
        );
        if self.focus_find {
            response.request_focus();
            self.focus_find = false;
        }
        if response.changed() {
            self.find_cursor = 0;
        }
        if self
            .result_cache
            .as_ref()
            .is_none_or(|(query, _)| *query != self.query)
        {
            self.result_cache = Some((self.query.clone(), self.find_results()));
        }
        let results = &self
            .result_cache
            .as_ref()
            .expect("find results populated")
            .1;
        if results.is_empty() && !self.query.is_empty() {
            ui.label("No matches");
        }
        render_result_list(
            ui,
            ctx,
            results,
            &mut self.find_cursor,
            response.has_focus() || response.lost_focus(),
        )
    }
}

impl Drop for PdfNavigation {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn render_result_list(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    results: &[(usize, String)],
    cursor: &mut usize,
    focused: bool,
) -> Option<usize> {
    if results.is_empty() {
        return None;
    }
    let down = focused
        && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
    let up = focused
        && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
    *cursor = (*cursor).min(results.len() - 1);
    if down {
        *cursor = (*cursor + 1) % results.len();
    }
    if up {
        *cursor = (*cursor + results.len() - 1) % results.len();
    }
    let mut destination = (focused
        && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter)))
    .then_some(results[*cursor].0);
    egui::ScrollArea::vertical()
        .id_source("pdf-result-list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (index, (page, label)) in results.iter().enumerate() {
                let response = ui.selectable_label(index == *cursor, label);
                if (up || down) && index == *cursor {
                    response.scroll_to_me(None);
                }
                if response.clicked() {
                    *cursor = index;
                    destination = Some(*page);
                }
            }
        });
    destination
}

fn fuzzy_score(title: &str, query: &str) -> Option<usize> {
    let title = title.to_lowercase();
    let mut remaining = title.chars().enumerate();
    let mut score = 0;
    for wanted in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let (position, _) = remaining.find(|(_, actual)| *actual == wanted)?;
        score += position;
    }
    Some(score)
}

impl PdfEditorState {
    pub(super) fn reset_navigation(&mut self) {
        self.navigation.cancel();
        self.navigation.text.clear();
        self.navigation.outline.clear();
        self.navigation.error = None;
        self.navigation.attempted = false;
        self.navigation.result_cache = None;
    }

    pub(super) fn navigation_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        self.navigation.poll(ctx);
        if !self.navigation.attempted && self.info_job.is_none() {
            self.navigation.start(
                self.path.clone(),
                self.page_count,
                ctx,
                self.read_check.clone(),
            );
        }
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::F)) {
            self.navigation.right = Some(RightTab::Find);
            self.navigation.focus_find = true;
        }
        if !ctx.wants_keyboard_input()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::T))
        {
            self.navigation.left = LeftTab::Contents;
            self.navigation.focus_contents = true;
        }
        if self.navigation.right.is_some()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.navigation.right = None;
            return true;
        }
        false
    }

    pub(super) fn navigation_buttons(&mut self, ui: &mut egui::Ui) {
        if ui.button("Find [Ctrl+F]").clicked() {
            self.navigation.right = Some(RightTab::Find);
            self.navigation.focus_find = true;
        }
        if ui.button("OCR").clicked() {
            self.navigation.right = Some(RightTab::Ocr);
        }
    }

    pub(super) fn render_right_bar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let Some(tab) = self.navigation.right else {
            return;
        };
        let tabs = [
            Tab::new(RightTab::Find, "Find").icon(Icon::Search),
            Tab::new(RightTab::Ocr, "OCR").icon(Icon::Text),
        ];
        if let Some(tab) = TabBar::new("pdf-right-tabs", tab, &tabs, palette)
            .equal_widths()
            .show(ui)
        {
            self.navigation.right = Some(tab);
            self.navigation.focus_find = tab == RightTab::Find;
        }
        ui.add_space(crate::ui_ext::SPACE_SM);
        if ui.small_button("Hide panel").clicked() {
            self.navigation.right = None;
        }
        match self.navigation.right {
            Some(RightTab::Find) => {
                if self.navigation.worker.is_some() {
                    ui.spinner();
                    ui.label("Reading document text…");
                }
                if let Some(error) = &self.navigation.error {
                    ui.label(error);
                }
                if let Some(page) = self.navigation.render_find(ui, ctx) {
                    self.selected_page = Some(page);
                }
            }
            Some(RightTab::Ocr) => self.render_pdf_ocr_controls(ui),
            None => {}
        }
    }

    fn render_pdf_ocr_controls(&mut self, ui: &mut egui::Ui) {
        ui.label("Tesseract · searchable PDF");
        ui.label(
            "Adds invisible text beneath the page content. Native text and images are preserved.",
        );
        ui.label("Language (installed Tesseract code)");
        ui.add(
            egui::TextEdit::singleline(&mut self.document_tools.ocr_language)
                .hint_text("eng")
                .desired_width(f32::INFINITY),
        );
        if self.document_tools.ocr_busy() {
            ui.spinner();
            if ui.button("Cancel OCR").clicked() {
                self.document_tools.cancel_ocr();
            }
        } else if ui
            .add_enabled(
                !self.edit_in_progress
                    && self.info_job.is_none()
                    && self.document_tools.layout.is_some()
                    && !self.document_tools.ocr_complete,
                egui::Button::new(if self.document_tools.ocr_complete {
                    "Text layer already complete"
                } else {
                    "Add text layer to PDF"
                }),
            )
            .clicked()
        {
            self.start_pdf_ocr();
        }
        ui.label(&self.document_tools.ocr_status);
        ui.label("The searchable copy is saved beside the source and opens here automatically.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fuzzy_contents_accepts_subsequence_and_unicode() {
        assert!(fuzzy_score("Chapter Two: Rendering", "chtr rnd").is_some());
        assert!(fuzzy_score("École", "écl").is_some());
        assert!(fuzzy_score("Chapter", "xyz").is_none());
    }
    #[test]
    fn search_keeps_page_boundaries_and_case_insensitive_matches() {
        let mut navigation = PdfNavigation::default();
        navigation.query = "hello".into();
        navigation.text = vec!["Hello HELLO".into(), "Other".into(), "hello again".into()];
        let results = navigation.find_results();
        assert_eq!(
            results.iter().map(|row| row.0).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert!(results[0].1.contains("2 matches"));
    }
    #[test]
    fn document_invalidation_keeps_tabs_queries_and_discards_old_index() {
        let mut state = PdfEditorState::without_jobs(PathBuf::from("fixture.pdf"));
        state.navigation.left = LeftTab::Contents;
        state.navigation.right = Some(RightTab::Ocr);
        state.navigation.query = "keep query".into();
        state.navigation.text = vec!["old text".into()];
        state.reset_navigation();
        assert!(state.navigation.left == LeftTab::Contents);
        assert!(state.navigation.right == Some(RightTab::Ocr));
        assert_eq!(state.navigation.query, "keep query");
        assert!(state.navigation.text.is_empty());
        assert!(!state.navigation.attempted);
    }
}
