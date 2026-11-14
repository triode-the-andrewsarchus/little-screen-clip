//! Small revision-checked summaries for grouped List columns; no waveform or cover decoding.
use super::{
    filesystem::{Directory, Entry},
    preview_job::Task,
    search::PreviewContext,
    sorting::{self, Category},
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::OnceLock,
    time::Duration,
};

const CAPACITY: usize = 8192;
type Key = (PathBuf, u64);
type ResultSummary = Result<Summary, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Field {
    Artist,
    Album,
    Genre,
    Duration,
}
impl Field {
    pub const AUDIO: [Self; 4] = [Self::Artist, Self::Album, Self::Genre, Self::Duration];
    pub fn label(self) -> &'static str {
        match self {
            Self::Artist => "Artist",
            Self::Album => "Album",
            Self::Genre => "Genre",
            Self::Duration => "Duration",
        }
    }
}
#[derive(Clone, Default)]
struct Summary {
    artist: String,
    album: String,
    genre: String,
    duration: Option<u64>,
}
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Value {
    Text(String),
    Number(u64),
}
impl Summary {
    fn value(&self, field: Field) -> Option<Value> {
        let text = match field {
            Field::Artist => &self.artist,
            Field::Album => &self.album,
            Field::Genre => &self.genre,
            Field::Duration => return self.duration.map(Value::Number),
        };
        (!text.is_empty()).then(|| Value::Text(text.to_lowercase()))
    }
    fn cell(&self, field: Field) -> String {
        match field {
            Field::Artist => self.artist.clone(),
            Field::Album => self.album.clone(),
            Field::Genre => self.genre.clone(),
            Field::Duration => self.duration.map_or_else(String::new, |ms| {
                format!("{}:{:02}", ms / 60_000, ms / 1000 % 60)
            }),
        }
    }
}
#[derive(Default)]
pub(super) struct Metadata {
    path: PathBuf,
    context: PreviewContext,
    values: HashMap<Key, ResultSummary>,
    pending: Vec<(Entry, Task<ResultSummary>)>,
    sorts: HashMap<Category, (Field, bool)>,
    total: usize,
    finished: usize,
    candidates: Vec<usize>,
    cursor: usize,
    order_dirty: bool,
    last_order: Option<std::time::Instant>,
}
fn eligible(entry: &Entry) -> bool {
    entry.regular_file
        && entry.system.is_none()
        && !entry.symlink
        && entry.identity.is_some()
        && little_clip_core::audio::is_audio_path(&entry.path)
}
fn key(entry: &Entry) -> Key {
    (entry.path.clone(), entry.revision)
}
fn jobs() -> &'static little_clip_core::BackgroundJobs {
    static JOBS: OnceLock<little_clip_core::BackgroundJobs> = OnceLock::new();
    JOBS.get_or_init(|| little_clip_core::BackgroundJobs::new(2, 4))
}
fn load(entry: &Entry, cancel: &std::sync::atomic::AtomicBool) -> ResultSummary {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("Metadata read cancelled".into());
    }
    super::filesystem::check_revision(&entry.path, entry.revision)?;
    let source = crate::file_source::FileSource::capture(&entry.path)?;
    let metadata = super::music::preview::metadata(&entry.path, cancel);
    source.check()?;
    super::filesystem::check_revision(&entry.path, entry.revision)?;
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("Metadata read cancelled".into());
    }
    Ok(Summary {
        artist: metadata.artist.trim().into(),
        album: metadata.album.trim().into(),
        genre: metadata.genre.trim().into(),
        duration: (metadata.duration.is_finite() && metadata.duration > 0.0)
            .then(|| (f64::from(metadata.duration) * 1000.0) as u64),
    })
}
impl Metadata {
    pub fn retire(&mut self) {
        self.pending.clear();
        self.cursor = 0;
    }
    pub fn configure(&mut self, path: &std::path::Path, context: &PreviewContext) -> bool {
        let changed = self.context.replace(context) || self.path != path;
        if changed {
            self.retire();
            self.values.clear();
            self.cursor = 0;
            self.finished = 0;
            self.order_dirty = false;
            self.last_order = None;
            self.path = path.into();
        }
        changed
    }
    pub fn reconcile(&mut self, directory: &Directory, visible: &[usize]) {
        let mut candidates = Vec::new();
        self.total = 0;
        for &index in visible {
            if eligible(&directory.entries[index]) {
                self.total += 1;
                if candidates.len() < CAPACITY {
                    candidates.push(index);
                }
            }
        }
        let live: HashSet<_> = candidates
            .iter()
            .map(|&index| key(&directory.entries[index]))
            .collect();
        let previous = self.values.len() + self.pending.len();
        self.values.retain(|key, _| live.contains(key));
        self.pending.retain(|(entry, _)| live.contains(&key(entry)));
        if self.candidates != candidates || self.values.len() + self.pending.len() != previous {
            self.cursor = 0;
        }
        self.candidates = candidates;
        self.finished = self.values.len();
    }
    pub fn step(&mut self, ctx: &egui::Context, directory: &Directory) -> bool {
        self.order_dirty |= self.poll();
        self.finished = self.values.len();
        while self.pending.len() < 2 && self.cursor < self.candidates.len() {
            let entry = &directory.entries[self.candidates[self.cursor]];
            self.cursor += 1;
            if self.values.contains_key(&key(entry))
                || self
                    .pending
                    .iter()
                    .any(|(pending, _)| key(pending) == key(entry))
            {
                continue;
            }
            let source = entry.clone();
            let context = self.context.clone();
            self.pending.push((
                entry.clone(),
                Task::start(jobs(), Duration::from_secs(15), move |cancel| {
                    context.read(&source.path, cancel, || load(&source, cancel))
                }),
            ));
        }
        // Tags can finish in small batches. Paint them immediately, but coalesce
        // sorting so large listings do not rebuild on every completed header.
        let reorder = !self.sorts.is_empty()
            && self.order_dirty
            && self
                .last_order
                .is_none_or(|at| at.elapsed() >= Duration::from_millis(200));
        if reorder {
            self.order_dirty = false;
            self.last_order = Some(std::time::Instant::now());
        }
        if !self.pending.is_empty() || (self.order_dirty && !self.sorts.is_empty()) {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        reorder
    }
    fn poll(&mut self) -> bool {
        let mut changed = false;
        for index in (0..self.pending.len()).rev() {
            if let Some(result) = self.pending[index].1.poll() {
                let (entry, _) = self.pending.swap_remove(index);
                if matches!(result, Err(little_clip_core::JobError::QueueFull)) {
                    self.cursor = 0;
                    continue;
                }
                self.values.insert(
                    key(&entry),
                    result
                        .map_err(|error| format!("{error:?}"))
                        .and_then(|value| value),
                );
                changed = true;
            }
        }
        changed
    }
    pub fn status(&self) -> String {
        format!(
            "Metadata {}/{}{}",
            self.finished,
            self.total,
            if self.values.len() >= CAPACITY && self.finished < self.total {
                " · summary limit reached; remaining files unanalyzed"
            } else {
                ""
            }
        )
    }
    pub fn cell(&self, entry: &Entry, field: Field) -> String {
        if !eligible(entry) {
            return "—".into();
        }
        match self.values.get(&key(entry)) {
            Some(Ok(summary)) => {
                let text = summary.cell(field);
                if text.is_empty() { "—".into() } else { text }
            }
            Some(Err(_)) => "Unavailable".into(),
            None => if self.values.len() >= CAPACITY {
                "Not analyzed"
            } else {
                "Pending"
            }
            .into(),
        }
    }
    pub fn error(&self, entry: &Entry) -> Option<&str> {
        self.values
            .get(&key(entry))?
            .as_ref()
            .err()
            .map(String::as_str)
    }
    pub fn sort(&self, family: Category) -> Option<(Field, bool)> {
        self.sorts.get(&family).copied()
    }
    pub fn toggle_sort(&mut self, family: Category, field: Field) {
        let descending = self
            .sort(family)
            .is_some_and(|sort| sort.0 == field && !sort.1);
        self.sorts.insert(family, (field, descending));
        self.order_dirty = false;
        self.last_order = Some(std::time::Instant::now());
    }
    pub fn order(&self, directory: &Directory, visible: &mut [usize]) {
        for group in visible.chunk_by_mut(|a, b| {
            sorting::family(&directory.entries[*a]) == sorting::family(&directory.entries[*b])
        }) {
            let Some(&(field, descending)) = self
                .sorts
                .get(&sorting::family(&directory.entries[group[0]]))
            else {
                continue;
            };
            let mut keys = group
                .iter()
                .map(|&index| {
                    let value = self
                        .values
                        .get(&key(&directory.entries[index]))
                        .and_then(|summary| summary.as_ref().ok())
                        .and_then(|summary| summary.value(field));
                    (index, value)
                })
                .collect::<Vec<_>>();
            keys.sort_by(|a, b| sorting::available(a.1.as_ref(), b.1.as_ref(), descending));
            for (slot, (index, _)) in group.iter_mut().zip(keys) {
                *slot = index;
            }
        }
    }
}

impl super::FileManager {
    pub(super) fn tick_metadata(&mut self, ctx: &egui::Context) {
        if !self
            .list_sort()
            .is_some_and(|sort| sort.column == sorting::Column::Type)
            || self.search.has_offline_results()
            || self.pending.is_some()
        {
            self.metadata.retire();
            return;
        }
        let Some(directory) = &self.directory else {
            return;
        };
        let context = self.search_preview_context();
        let reset = self.metadata.configure(&directory.path, &context);
        if self.metadata.step(ctx, directory) | reset {
            let anchor = self.list_anchor();
            self.rebuild_visible();
            self.restore_list_anchor(anchor);
        }
    }
}

impl Metadata {
    pub fn paint_header(
        &self,
        ui: &mut egui::Ui,
        columns: super::ui::ListColumns,
        row: egui::Rect,
        family: Category,
        colors: crate::ui_appearance::WorkbenchColors,
    ) -> Option<Field> {
        let mut clicked = None;
        for (index, field) in Field::AUDIO.into_iter().enumerate() {
            let Some(cell) = columns.metadata_cell(row, index) else {
                break;
            };
            let active = self.sort(family).filter(|sort| sort.0 == field);
            let label = match active {
                Some((_, true)) => format!("{} ↓", field.label()),
                Some(_) => format!("{} ↑", field.label()),
                None => field.label().into(),
            };
            let text = egui::RichText::new(label).color(if active.is_some() {
                colors.accent
            } else {
                colors.text_secondary
            });
            if ui
                .put(cell, egui::Button::new(text).frame(false))
                .on_hover_text(format!(
                    "Sort this family by {}; missing values stay last. {}",
                    field.label(),
                    self.status()
                ))
                .clicked()
            {
                clicked = Some(field);
            }
        }
        clicked
    }
    pub fn paint_cells(
        &self,
        ui: &egui::Ui,
        columns: super::ui::ListColumns,
        row: egui::Rect,
        entry: &Entry,
        colors: crate::ui_appearance::WorkbenchColors,
    ) {
        for (index, field) in Field::AUDIO.into_iter().enumerate() {
            let Some(cell) = columns.metadata_cell(row, index) else {
                break;
            };
            let text = self.cell(entry, field);
            super::ui::paint_text(
                ui,
                cell,
                5.0,
                cell.width() - 10.0,
                &text,
                colors.text_secondary,
            );
            ui.interact(
                cell,
                ui.make_persistent_id(("metadata_cell", &entry.path, field)),
                egui::Sense::hover(),
            )
            .on_hover_text(self.error(entry).unwrap_or(&text));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn fixture() -> (tempfile::TempDir, Directory) {
        let folder = tempfile::tempdir().unwrap();
        for name in [
            "alpha.wav",
            "blank.wav",
            "failed.wav",
            "omega.mp3",
            "tie.wav",
            "photo.png",
        ] {
            std::fs::write(folder.path().join(name), "fixture").unwrap();
        }
        let directory =
            super::super::filesystem::read_directory(folder.path(), &AtomicBool::new(false))
                .unwrap();
        (folder, directory)
    }
    fn summaries(directory: &Directory) -> Metadata {
        let mut state = Metadata::default();
        for entry in &directory.entries {
            let artist = match entry.name.as_str() {
                "alpha.wav" | "tie.wav" => "Álpha",
                "omega.mp3" => "Zulu",
                "failed.wav" => {
                    state.values.insert(key(entry), Err("bad header".into()));
                    continue;
                }
                _ => "",
            };
            state.values.insert(
                key(entry),
                Ok(Summary {
                    artist: artist.into(),
                    ..Default::default()
                }),
            );
        }
        state
    }
    #[test]
    fn family_metadata_sorts_stably_and_keeps_missing_failures_and_other_families_last() {
        let (_folder, directory) = fixture();
        let mut state = summaries(&directory);
        for field in Field::AUDIO {
            state.toggle_sort(Category::Audio, field);
            for descending in [false, true] {
                if descending {
                    state.toggle_sort(Category::Audio, field);
                }
                let mut visible = (0..directory.entries.len()).collect::<Vec<_>>();
                sorting::Sort {
                    column: sorting::Column::Type,
                    descending: false,
                }
                .apply(&directory, &mut visible);
                state.order(&directory, &mut visible);
                let names = visible
                    .iter()
                    .map(|&i| directory.entries[i].name.as_str())
                    .collect::<Vec<_>>();
                assert_eq!(names.last(), Some(&"photo.png"));
                if field == Field::Artist {
                    assert_eq!(&names[3..5], ["blank.wav", "failed.wav"]);
                    assert!(
                        names.iter().position(|name| *name == "alpha.wav")
                            < names.iter().position(|name| *name == "tie.wav")
                    );
                    assert_eq!(names[0], if descending { "alpha.wav" } else { "omega.mp3" });
                }
            }
        }
    }
    #[test]
    fn numeric_duration_and_casefolded_tags_are_typed_and_missing_is_not_zero() {
        let summary = Summary {
            artist: "Artist".into(),
            duration: Some(123_456),
            ..Default::default()
        };
        assert!(summary.value(Field::Artist) == Some(Value::Text("artist".into())));
        assert!(summary.value(Field::Duration) == Some(Value::Number(123_456)));
        assert!(summary.value(Field::Genre).is_none());
        assert_eq!(summary.cell(Field::Duration), "2:03");
    }
    #[test]
    fn changed_files_and_search_contexts_retire_old_summaries() {
        let (_folder, mut directory) = fixture();
        let mut state = summaries(&directory);
        state.configure(&directory.path, &PreviewContext::default());
        assert!(state.values.is_empty());
        state = summaries(&directory);
        let stale = directory.entries[0].clone();
        directory.entries[0].revision = stale.revision.wrapping_add(1);
        state.reconcile(
            &directory,
            &(0..directory.entries.len()).collect::<Vec<_>>(),
        );
        assert!(!state.values.contains_key(&key(&stale)));
        state.configure(&directory.path, &PreviewContext::empty_results());
        assert!(state.values.is_empty());
        let error = load(&stale, &AtomicBool::new(false)).err();
        // The original revision remains current on disk, but a replacement
        // observation must be rejected before reading any payload.
        assert!(error.is_none());
        assert!(load(&directory.entries[0], &AtomicBool::new(false)).is_err());
    }
    #[test]
    fn metadata_reorder_preserves_the_scroll_anchor_and_selected_file() {
        let (_folder, directory) = fixture();
        let mut manager = super::super::FileManager::new(directory.path.clone());
        manager.pending = None;
        manager.commit(directory.clone(), super::super::Navigation::New);
        manager.sort_by(sorting::Column::Type);
        manager.metadata = summaries(&directory);
        let selected = directory.path.join("omega.mp3");
        manager.select(selected.clone());
        manager.location_mut().unwrap().scroll = manager.list_offsets[1] + 13.0;
        let anchor = manager.list_anchor();
        manager.metadata.toggle_sort(Category::Audio, Field::Artist);
        manager.rebuild_visible();
        manager.restore_list_anchor(anchor.clone());
        assert_eq!(manager.list_anchor(), anchor);
        assert_eq!(
            manager.location().unwrap().selected.as_ref(),
            Some(&selected)
        );
    }
    #[test]
    fn summary_budget_is_explicit_and_retirement_keeps_completed_values_retryable() {
        let (_folder, mut directory) = fixture();
        let original = directory
            .entries
            .iter()
            .find(|entry| entry.name == "alpha.wav")
            .unwrap()
            .clone();
        directory.entries = (0..CAPACITY + 3)
            .map(|index| {
                let mut entry = original.clone();
                entry.path = directory.path.join(format!("{index}.wav"));
                entry.name = format!("{index}.wav");
                entry
            })
            .collect();
        let visible = (0..directory.entries.len()).collect::<Vec<_>>();
        let mut state = Metadata::default();
        for entry in directory.entries.iter().take(CAPACITY) {
            state.values.insert(key(entry), Ok(Summary::default()));
        }
        state.reconcile(&directory, &visible);
        assert_eq!(state.candidates.len(), CAPACITY);
        assert_eq!(state.total, CAPACITY + 3);
        assert!(state.status().contains("remaining files unanalyzed"));
        assert_eq!(
            state.cell(&directory.entries[CAPACITY], Field::Artist),
            "Not analyzed"
        );
        state.cursor = 2;
        state.retire();
        assert_eq!(state.cursor, 0);
        assert_eq!(state.values.len(), CAPACITY);
        state.reconcile(&directory, &[CAPACITY]);
        assert!(state.values.is_empty());
        assert_eq!(state.total, 1);
        assert_eq!(state.candidates, [CAPACITY]);
    }

    #[test]
    fn native_tag_summary_reads_genre_and_duration_without_preview_artifacts() {
        use little_clip_core::process::CommandRequest;
        let (_source_directory, source) =
            crate::audio_preview::test_support::generate_fixture_wav("tags", 0.3);
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("tagged.wav");
        let output = CommandRequest::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(source.as_os_str())
            .args([
                "-metadata",
                "artist=Artist",
                "-metadata",
                "album=Album",
                "-metadata",
                "genre=Jazz",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(path.as_os_str())
            .timeout(Duration::from_secs(5))
            .run()
            .unwrap();
        assert!(output.status.success(), "{}", output.stderr_lossy());
        let directory =
            super::super::filesystem::read_directory(folder.path(), &AtomicBool::new(false))
                .unwrap();
        let summary = load(&directory.entries[0], &AtomicBool::new(false)).unwrap();
        assert_eq!(summary.artist, "Artist");
        assert_eq!(summary.album, "Album");
        assert_eq!(summary.genre, "Jazz");
        assert!(summary.duration.is_some_and(|ms| (250..=350).contains(&ms)));
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }
}
