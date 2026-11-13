//! One guarded Markdown hover selection, using the shared cache and studio view.
use super::shelf::preview::{Data, HoverRaster, Source};
use clip_studios::text_preview::{MarkdownExcerptData, MarkdownExcerptView};
use little_clip_core::file_source::{FileSource, ReadCheck};
use std::{
    collections::BTreeSet,
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Default)]
pub(super) struct Hover {
    cache: HoverRaster,
    view: Option<(Arc<MarkdownExcerptData>, MarkdownExcerptView)>,
}
impl Hover {
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.cache.poll(ctx);
    }
    pub fn clear(&mut self) {
        self.cache.clear();
        self.view = None;
    }
    pub fn retain(&mut self, visible: &BTreeSet<uuid::Uuid>) {
        if self.cache.retain(|id| visible.contains(&id)) {
            self.view = None;
        }
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        id: uuid::Uuid,
        source: &FileSource,
        line: usize,
        context: &super::search::PreviewContext,
        palette: &little_clip_core::palette::AppPalette,
    ) -> bool {
        let ready = self.cache.request(
            id,
            line,
            0,
            Source::Markdown {
                source: source.clone(),
                line,
            }
            .checked(source.path(), context),
        );
        let Some(data) = ready.and_then(|ready| ready.data.markdown.clone()) else {
            self.view = None;
            if let Some(ready) = ready {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("Rendered Markdown unavailable: {}", ready.data.detail),
                );
                return false;
            }
            ui.small("Preparing rendered Markdown…");
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(16));
            return true;
        };
        if self
            .view
            .as_ref()
            .is_none_or(|(old, _)| !Arc::ptr_eq(old, &data))
        {
            self.view = Some((data.clone(), MarkdownExcerptView::new(data.clone())));
        }
        ui.label(format!(
            "Markdown · lines {}–{}",
            data.source_lines.start(),
            data.source_lines.end()
        ));
        ui.set_width((ui.ctx().screen_rect().width() - 40.0).clamp(160.0, 760.0));
        let height = (ui.ctx().screen_rect().height() / 3.0).clamp(80.0, 400.0);
        if let Some((_, view)) = &mut self.view {
            view.show(ui, height, palette);
        }
        true
    }
}

pub(super) fn load(source: FileSource, line: usize, cancel: &AtomicBool) -> Data {
    let mut data = Data::new(String::new(), String::new(), crate::icons::Icon::Text);
    match prepare(&source, line, cancel) {
        Ok(excerpt) => data.markdown = Some(excerpt),
        Err(error) => data.detail = error,
    }
    data
}

fn prepare(
    source: &FileSource,
    line: usize,
    cancel: &AtomicBool,
) -> Result<Arc<MarkdownExcerptData>, String> {
    const LIMIT: u64 = 2 * 1024 * 1024;
    let (mut file, _) = ReadCheck::default().open(source.path(), Some(source), cancel)?;
    if file.metadata().map_err(|error| error.to_string())?.len() > LIMIT {
        return Err(
            "Markdown hover source exceeds 2 MiB; showing the selected text excerpt".into(),
        );
    }
    let mut text = String::new();
    file.by_ref()
        .take(LIMIT + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err("Markdown hover cancelled".into());
    }
    let result = MarkdownExcerptData::prepare(&text, line, cancel)?;
    source.check_file(&file)?;
    source.check()?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guarded_hover_loads_the_requested_lines_and_rejects_replaced_content() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("page.md");
        let source = format!(
            "# Start\n\n{}# Destination\n\nRendered words.\n",
            "Filler\n\n".repeat(200)
        );
        std::fs::write(&path, &source).unwrap();
        let guard = FileSource::capture(&path).unwrap();
        let data = load(guard.clone(), 403, &AtomicBool::new(false));
        assert!(*data.markdown.unwrap().source_lines.start() > 390);
        std::fs::write(&path, "# Replaced").unwrap();
        let stale = load(guard, 1, &AtomicBool::new(false));
        assert!(stale.markdown.is_none());
        assert!(stale.detail.contains("changed"), "{}", stale.detail);
    }

    #[test]
    fn cancelled_incomplete_and_oversized_hover_sources_remain_inspectable_failures() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("page.md");
        for text in ["```mermaid\nflowchart LR", &"x".repeat(2 * 1024 * 1024 + 1)] {
            std::fs::write(&path, text).unwrap();
            let data = load(
                FileSource::capture(&path).unwrap(),
                2,
                &AtomicBool::new(false),
            );
            assert!(data.markdown.is_none());
            assert!(!data.detail.is_empty());
        }
        std::fs::write(&path, "# Ready").unwrap();
        let data = load(
            FileSource::capture(&path).unwrap(),
            1,
            &AtomicBool::new(true),
        );
        assert!(data.markdown.is_none());
    }
}
