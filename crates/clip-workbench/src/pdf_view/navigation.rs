use super::*;
use crate::image_preview::{ImageToolProcessControl, OcrEngine, OcrMsg, ocr_command, run_png_ocr};
use little_clip_core::pdf::{PdfOutlineEntry, extract_pdf_page_text, read_pdf_outline};
use std::sync::mpsc::{Sender, channel};

#[derive(Clone, Copy, PartialEq, Eq)]
enum NavigationMode {
    Contents,
    Find,
}

enum IndexMessage {
    Outline(Result<Vec<PdfOutlineEntry>, String>),
    Page(usize, String),
    Progress(String),
}

type IndexWorker = (JobTicket, Receiver<Result<Result<(), String>, JobError>>);
type NavigationResults = (String, Option<NavigationMode>, Vec<(usize, String)>);

#[derive(Default)]
pub(super) struct PdfNavigation {
    mode: Option<NavigationMode>,
    query: String,
    focus: bool,
    cursor: usize,
    outline: Vec<PdfOutlineEntry>,
    text: Vec<String>,
    indexed: bool,
    attempted: bool,
    result_cache: Option<NavigationResults>,
    error: Option<String>,
    progress: String,
    engine: OcrEngine,
    worker: Option<IndexWorker>,
    messages: Option<Receiver<IndexMessage>>,
    control: Option<Arc<ImageToolProcessControl>>,
}

impl PdfNavigation {
    fn cancel(&mut self) {
        if let Some(control) = self.control.take() {
            control.cancel();
        }
        if let Some((ticket, _)) = self.worker.take() {
            ticket.cancel();
        }
        self.messages = None;
        self.progress.clear();
    }

    fn start(&mut self, path: PathBuf, count: usize, ocr: bool, ctx: &egui::Context) {
        if self.worker.is_some() || count == 0 {
            return;
        }
        self.attempted = true;
        self.text.resize(count, String::new());
        self.result_cache = None;
        self.error = None;
        self.progress = "Reading PDF…".into();
        let control = Arc::new(ImageToolProcessControl::new());
        let worker_control = control.clone();
        self.control = Some(control);
        let engine = self.engine;
        let (tx, rx) = channel();
        self.messages = Some(rx);
        let repaint = ctx.clone();
        self.worker = Some(BackgroundJobs::global().submit(Duration::MAX, move || {
            let result = index_document(&path, count, ocr, engine, &worker_control, &tx);
            repaint.request_repaint();
            result
        }));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        // Observe completion before draining, so the final published page cannot be lost.
        let result = self.worker.as_ref().map(|(_, rx)| rx.try_recv());
        if let Some(rx) = &self.messages {
            while let Ok(message) = rx.try_recv() {
                match message {
                    IndexMessage::Outline(Ok(outline)) => {
                        self.outline = outline;
                        self.result_cache = None;
                    }
                    IndexMessage::Outline(Err(error)) => {
                        self.error = Some(format!("Contents: {error}"))
                    }
                    IndexMessage::Page(page, text) => {
                        self.text[page - 1] = text;
                        self.result_cache = None;
                    }
                    IndexMessage::Progress(progress) => self.progress = progress,
                }
            }
        }
        match result {
            Some(Ok(result)) => {
                match result {
                    Ok(Ok(())) => self.indexed = true,
                    Ok(Err(error)) => self.error = Some(error),
                    Err(error) => self.error = Some(error.to_string()),
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

    fn open(&mut self, mode: NavigationMode) {
        self.mode = Some(mode);
        self.query.clear();
        self.cursor = 0;
        self.focus = true;
    }

    fn results(&self) -> Vec<(usize, String)> {
        if self.mode == Some(NavigationMode::Contents) {
            let mut entries: Vec<_> = self
                .outline
                .iter()
                .filter_map(|entry| {
                    fuzzy_score(&entry.title, &self.query).map(|score| (score, entry))
                })
                .collect();
            if !self.query.is_empty() {
                entries.sort_by_key(|(score, _)| *score);
            }
            return entries
                .into_iter()
                .map(|(_, entry)| {
                    (
                        entry.page,
                        format!(
                            "{}{} — p. {}",
                            "  ".repeat(entry.depth.min(8)),
                            entry.title,
                            entry.page
                        ),
                    )
                })
                .collect();
        }
        let query = self.query.to_lowercase();
        if query.trim().is_empty() {
            return Vec::new();
        }
        self.text
            .iter()
            .enumerate()
            .filter_map(|(index, text)| {
                let lower = text.to_lowercase();
                if !lower.contains(&query) {
                    return None;
                }
                let snippet = text
                    .lines()
                    .find(|line| line.to_lowercase().contains(&query))
                    .unwrap_or(text)
                    .chars()
                    .take(160)
                    .collect::<String>();
                Some((
                    index + 1,
                    format!(
                        "Page {} · {} matches — {}",
                        index + 1,
                        lower.matches(&query).count(),
                        snippet
                    ),
                ))
            })
            .collect()
    }
    fn render_controls(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        path: &Path,
        page_count: usize,
        edit_in_progress: bool,
    ) {
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text(if self.mode == Some(NavigationMode::Contents) {
                    "Fuzzy search bookmarks…"
                } else {
                    "Search document text…"
                })
                .desired_width(f32::INFINITY),
        );
        if self.focus {
            response.request_focus();
            self.focus = false;
        }
        if response.changed() {
            self.cursor = 0;
        }
        ui.horizontal(|ui| {
            for engine in [OcrEngine::GlmOcr, OcrEngine::Tesseract] {
                ui.selectable_value(&mut self.engine, engine, engine.label());
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.worker.is_none() && !edit_in_progress,
                    egui::Button::new("OCR entire PDF"),
                )
                .clicked()
            {
                self.start(path.to_owned(), page_count, true, ctx);
            }
            if self.worker.is_some() {
                ui.spinner();
                if ui.button("Cancel").clicked() {
                    self.cancel();
                }
            } else if ui.button("Reload text").clicked() {
                self.start(path.to_owned(), page_count, false, ctx);
            }
        });
        self.render_index_status(ui);
    }

    fn render_index_status(&self, ui: &mut egui::Ui) {
        if !self.progress.is_empty() {
            ui.label(&self.progress);
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
        if self.mode == Some(NavigationMode::Contents) && self.outline.is_empty() {
            ui.label("This PDF has no embedded bookmarks. Use Find to search its text; OCR scanned pages first.");
        } else if self.mode == Some(NavigationMode::Find)
            && self.indexed
            && self.text.iter().all(|text| text.trim().is_empty())
        {
            ui.label("No text layer found. Run OCR to make this document searchable.");
        }
    }

    fn render_results(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<usize> {
        let mut destination = None;
        if self
            .result_cache
            .as_ref()
            .is_none_or(|(query, mode, _)| *query != self.query || *mode != self.mode)
        {
            self.result_cache = Some((self.query.clone(), self.mode, self.results()));
        }
        let results = &self
            .result_cache
            .as_ref()
            .expect("results were populated")
            .2;
        if results.is_empty() && !self.query.is_empty() {
            ui.label("No matches");
        }
        if !results.is_empty() {
            let down =
                ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown));
            let up = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp));
            if down {
                self.cursor = (self.cursor + 1) % results.len();
            }
            if up {
                self.cursor = (self.cursor + results.len() - 1) % results.len();
            }
            self.cursor = self.cursor.min(results.len() - 1);
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                destination = Some(results[self.cursor].0);
            }
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for (index, (page, label)) in results.iter().enumerate() {
                        let response = ui.selectable_label(index == self.cursor, label);
                        if (up || down) && index == self.cursor {
                            response.scroll_to_me(None);
                        }
                        if response.clicked() {
                            self.cursor = index;
                            destination = Some(*page);
                        }
                    }
                });
        }

        destination
    }
}

impl Drop for PdfNavigation {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Case-insensitive subsequence matching, preferring contiguous and early matches.
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

fn index_document(
    path: &Path,
    count: usize,
    ocr: bool,
    engine: OcrEngine,
    control: &Arc<ImageToolProcessControl>,
    tx: &Sender<IndexMessage>,
) -> Result<(), String> {
    let _ = tx.send(IndexMessage::Outline(read_pdf_outline(path)));
    for page in 1..=count {
        if control.cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        let _ = tx.send(IndexMessage::Progress(format!(
            "{} page {page} of {count}…",
            if ocr { "OCR" } else { "Reading" }
        )));
        let text = if ocr {
            ocr_page(path, page, engine, control, tx)?
        } else {
            extract_pdf_page_text(path, page)?
        };
        if control.cancel.load(Ordering::SeqCst) {
            return Ok(());
        }
        let _ = tx.send(IndexMessage::Page(page, text));
    }
    Ok(())
}

fn ocr_page(
    path: &Path,
    page: usize,
    engine: OcrEngine,
    control: &Arc<ImageToolProcessControl>,
    progress: &Sender<IndexMessage>,
) -> Result<String, String> {
    let (png, _, _) = render_pdf_page_preview(path, page, 2400)?;
    if control.cancel.load(Ordering::SeqCst) {
        return Ok(String::new());
    }
    let (python, script) = ocr_command();
    let (tx, rx) = channel();
    let progress = progress.clone();
    let output = std::thread::spawn(move || {
        let mut text = String::new();
        for message in rx {
            match message {
                OcrMsg::Success { text: result, .. } => text = result,
                OcrMsg::Progress(status) => {
                    let _ = progress.send(IndexMessage::Progress(status));
                }
                OcrMsg::Error(error) => return Err(format!("Page {page}: {error}")),
            }
        }
        Ok::<_, String>(text)
    });
    // Shared runner drains both pipes and kills its process group on cancellation.
    run_png_ocr(
        png,
        (python, script),
        engine.arg_name().into(),
        None,
        true,
        control.clone(),
        tx,
    );
    output
        .join()
        .map_err(|_| "OCR output reader stopped".to_owned())?
}

impl PdfEditorState {
    pub(super) fn navigation_open(&self) -> bool {
        self.navigation.mode.is_some()
    }

    pub(super) fn reset_navigation(&mut self) {
        self.navigation = PdfNavigation::default();
    }

    /// Returns whether Escape was handled by the navigation panel.
    pub(super) fn navigation_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        self.navigation.poll(ctx);
        let find = ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::F));
        let contents = !ctx.wants_keyboard_input()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::T));
        if find || contents {
            self.navigation.open(if find {
                NavigationMode::Find
            } else {
                NavigationMode::Contents
            });
        }
        if self.navigation.mode.is_some()
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.navigation.mode = None;
            return true;
        }
        false
    }

    pub(super) fn navigation_buttons(&mut self, ui: &mut egui::Ui) {
        if ui.button("Contents [T]").clicked() {
            self.navigation.open(NavigationMode::Contents);
        }
        if ui.button("Find [Ctrl+F]").clicked() {
            self.navigation.open(NavigationMode::Find);
        }
    }

    pub(super) fn render_navigation(&mut self, ctx: &egui::Context) {
        let Some(mode) = self.navigation.mode else {
            return;
        };
        if !self.navigation.attempted
            && self.navigation.worker.is_none()
            && self.navigation.error.is_none()
        {
            self.navigation
                .start(self.path.clone(), self.page_count, false, ctx);
        }
        let mut open = true;
        let mut destination = None;
        egui::Window::new(if mode == NavigationMode::Contents {
            "PDF Contents"
        } else {
            "Find in PDF"
        })
        .id(egui::Id::new("pdf-navigation"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .show(ctx, |ui| {
            self.navigation.render_controls(
                ui,
                ctx,
                &self.path,
                self.page_count,
                self.edit_in_progress,
            );
            destination = self.navigation.render_results(ui, ctx);
            if let Some(page) = self.selected_page.filter(|_| mode == NavigationMode::Find)
                && let Some(text) = self.navigation.text.get(page - 1)
            {
                ui.collapsing("Selected page text", |ui| {
                    if ui.button("Copy page text").clicked() {
                        ui.output_mut(|out| out.copied_text = text.clone());
                    }
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .show(ui, |ui| {
                            ui.label(text);
                        });
                });
            }
        });
        if let Some(page) = destination.filter(|page| *page > 0 && *page <= self.page_count) {
            self.selected_page = Some(page);
            if mode == NavigationMode::Contents {
                self.navigation.mode = None;
            }
        } else if !open {
            self.navigation.mode = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contents_enter_navigates_and_escape_dismisses_find_before_viewer() {
        let ctx = egui::Context::default();
        let palette =
            crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
        let mut state = PdfEditorState::without_jobs(PathBuf::from("fixture.pdf"));
        state.page_count = 3;
        state.navigation.indexed = true;
        state.navigation.attempted = true;
        state.navigation.outline = vec![
            PdfOutlineEntry {
                title: "Introduction".into(),
                page: 1,
                depth: 0,
            },
            PdfOutlineEntry {
                title: "Rendering".into(),
                page: 3,
                depth: 0,
            },
        ];
        let mut close = false;
        {
            let mut frame = |key, modifiers| {
                let _ = ctx.run(
                    egui::RawInput {
                        modifiers,
                        events: vec![egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers,
                        }],
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default()
                            .show(ctx, |ui| state.render_ui(ui, ctx, &palette, &mut close));
                    },
                );
            };
            frame(egui::Key::T, egui::Modifiers::NONE);
            frame(egui::Key::R, egui::Modifiers::NONE);
            frame(egui::Key::Delete, egui::Modifiers::NONE);
            frame(egui::Key::ArrowDown, egui::Modifiers::NONE);
            frame(egui::Key::Enter, egui::Modifiers::NONE);
            frame(egui::Key::F, egui::Modifiers::CTRL);
            frame(egui::Key::Escape, egui::Modifiers::NONE);
        }
        assert_eq!(state.selected_page, Some(3));
        assert!(!state.edit_in_progress);
        assert!(!close);
        assert!(state.navigation.mode.is_none());
        let _ = ctx.run(
            egui::RawInput {
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| state.render_ui(ui, ctx, &palette, &mut close));
            },
        );
        assert!(close);
    }

    #[test]
    fn cancel_keeps_partial_text_without_automatically_restarting_index() {
        let ctx = egui::Context::default();
        let mut state = PdfEditorState::without_jobs(PathBuf::from("fixture.pdf"));
        state.page_count = 1;
        state.navigation.mode = Some(NavigationMode::Find);
        state.navigation.attempted = true;
        state.navigation.text = vec!["recognized so far".into()];
        let control = Arc::new(ImageToolProcessControl::new());
        state.navigation.control = Some(control.clone());
        state.navigation.cancel();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            state.render_navigation(ctx)
        });
        assert!(control.cancel.load(Ordering::SeqCst));
        assert!(state.navigation.worker.is_none());
        assert_eq!(state.navigation.text[0], "recognized so far");
        state.reset_navigation();
        assert!(state.navigation.text.is_empty());
        assert!(!state.navigation.attempted);
    }

    #[test]
    fn completion_drains_last_page_before_releasing_message_receiver() {
        let jobs = BackgroundJobs::new(1, 1);
        let mut navigation = PdfNavigation::default();
        navigation.text = vec![String::new()];
        let (tx, rx) = channel();
        navigation.messages = Some(rx);
        navigation.worker = Some(jobs.submit(Duration::MAX, move || {
            tx.send(IndexMessage::Page(1, "last page".into())).unwrap();
            Ok(())
        }));
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while navigation.worker.is_some() && std::time::Instant::now() < deadline {
            navigation.poll(&egui::Context::default());
            std::thread::yield_now();
        }
        assert!(navigation.indexed);
        assert_eq!(navigation.text, vec!["last page"]);
    }

    #[test]
    fn fuzzy_contents_accepts_subsequence_and_unicode() {
        assert!(fuzzy_score("Chapter Two: Rendering", "chtr rnd").is_some());
        assert!(fuzzy_score("École", "écl").is_some());
        assert!(fuzzy_score("Chapter", "xyz").is_none());
    }
    #[test]
    fn search_keeps_page_boundaries_and_case_insensitive_matches() {
        let mut navigation = PdfNavigation::default();
        navigation.mode = Some(NavigationMode::Find);
        navigation.text = vec!["Hello HELLO".into(), "Other".into(), "hello again".into()];
        navigation.query = "hello".into();
        let results = navigation.results();
        assert_eq!(
            results.iter().map(|row| row.0).collect::<Vec<_>>(),
            vec![1, 3]
        );
        assert!(results[0].1.contains("2 matches"));
    }
}
