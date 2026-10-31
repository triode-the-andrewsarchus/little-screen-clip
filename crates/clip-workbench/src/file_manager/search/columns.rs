//! Window-owned column definitions/results and one cancellable file request.
use super::{
    answer::{Answer, State},
    policy,
    query::{Operator, Query},
    reader::SourceRoot,
    service::{AnswerJob, AnswerResult},
};
use crate::file_manager::filesystem::{Directory, Entry};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

type CellKey = (PathBuf, u64);
#[derive(Clone, PartialEq, Eq, Hash)]
struct FolderKey {
    path: PathBuf,
    object: (u64, u64),
    mount: Option<u64>,
}
#[derive(Default)]
pub(in crate::file_manager) struct Columns {
    folders: HashMap<FolderKey, Folder>,
    active: Option<FolderKey>,
    pending: Option<Pending>,
    serial: u64,
    pub priority: Vec<usize>,
    suspended: bool,
    pipeline: Option<String>,
}
#[derive(Default)]
pub(in crate::file_manager) struct Folder {
    pub(super) columns: Vec<Column>,
    pub sort: Option<(u64, bool)>,
    pub focused: u64,
    pub paused: bool,
    signature: u64,
    visible: Vec<bool>,
    tie_order: HashMap<PathBuf, (u64, usize)>,
}
pub(super) struct Column {
    pub id: u64,
    pub title: String,
    pub query: Query,
    signature: String,
    pub cells: HashMap<PathBuf, (u64, Answer)>,
    pub finished: usize,
    pub total: usize,
    todo: VecDeque<usize>,
    plans: super::answer::extraction::SharedPlans,
}
struct Pending {
    id: u64,
    folder: FolderKey,
    column: u64,
    key: CellKey,
    index: usize,
    cancel: Arc<AtomicBool>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Column {
    pub fn answer(&self, entry: &Entry) -> Option<&Answer> {
        self.cells
            .get(&entry.path)
            .filter(|(revision, _)| *revision == entry.revision)
            .map(|(_, answer)| answer)
    }
    pub fn skips(&self, entry: &Entry) -> Option<&'static str> {
        if !self.query.metadata_matches(entry) {
            return Some("Excluded by this column's selectors");
        }
        if policy::secret_file(&entry.path) || policy::excluded(&entry.path).is_some() {
            return Some("Excluded by the content policy");
        }
        if !super::answer::supports(entry) {
            return Some("Unsupported by the current document/image answer path");
        }
        None
    }
    fn eligible(&self, entry: &Entry) -> bool {
        self.skips(entry).is_none() && self.answer(entry).is_none()
    }
}
impl Columns {
    pub fn observe_pipeline(&mut self, pipeline: Option<String>) -> bool {
        let Some(pipeline) = pipeline else {
            return false;
        };
        if self.pipeline.as_ref() == Some(&pipeline) {
            return false;
        }
        let changed = self.pipeline.replace(pipeline).is_some();
        if changed {
            self.invalidate();
        }
        changed
    }
    pub fn invalidate(&mut self) {
        self.cancel_pending();
        for folder in self.folders.values_mut() {
            for column in &mut folder.columns {
                column.cells.clear();
                if let Ok(mut plans) = column.plans.lock() {
                    plans.clear();
                }
                column.finished = 0;
                column.todo = folder
                    .visible
                    .iter()
                    .enumerate()
                    .filter_map(|(index, visible)| visible.then_some(index))
                    .collect();
            }
        }
    }
    pub fn current(&self) -> Option<&Folder> {
        self.folders.get(self.active.as_ref()?)
    }
    pub fn current_mut(&mut self) -> Option<&mut Folder> {
        self.folders.get_mut(self.active.as_ref()?)
    }
    pub fn count(&self) -> usize {
        self.current().map_or(0, |folder| folder.columns.len())
    }
    pub fn set_suspended(&mut self, suspended: bool) {
        if suspended {
            self.cancel_pending();
        }
        self.suspended = suspended;
    }
    pub fn cancel_pending(&mut self) {
        if let Some(pending) = self.pending.take()
            && let Some(column) = self.folders.get_mut(&pending.folder).and_then(|folder| {
                folder
                    .columns
                    .iter_mut()
                    .find(|column| column.id == pending.column)
            })
        {
            column.todo.push_front(pending.index);
        }
    }
    pub fn leave(&mut self) {
        self.cancel_pending();
        self.active = None;
        self.priority.clear();
    }
    pub fn reconcile(&mut self, directory: &Directory, visible: &[usize]) {
        let key = directory.object.map(|object| FolderKey {
            path: directory.path.clone(),
            object,
            mount: directory.mount,
        });
        let changed = key != self.active;
        if changed {
            self.cancel_pending();
            self.priority.clear();
            self.active = key;
        }
        let Some(folder) = self.current_mut() else {
            return;
        };
        let mut membership = vec![false; directory.entries.len()];
        for &index in visible {
            if let Some(value) = membership.get_mut(index) {
                *value = true;
            }
        }
        if changed || folder.signature != directory.signature || membership != folder.visible {
            let keys: HashMap<_, _> = directory
                .entries
                .iter()
                .map(|entry| (entry.path.clone(), entry.revision))
                .collect();
            for column in &mut folder.columns {
                column
                    .cells
                    .retain(|path, (revision, _)| keys.get(path) == Some(revision));
                column.todo = visible.iter().copied().collect();
                column.total = visible
                    .iter()
                    .filter(|&&index| column.skips(&directory.entries[index]).is_none())
                    .count();
                column.finished = visible
                    .iter()
                    .filter(|&&index| {
                        column.skips(&directory.entries[index]).is_none()
                            && column.answer(&directory.entries[index]).is_some()
                    })
                    .count();
            }
            folder.signature = directory.signature;
            folder.visible = membership;
        }
        if self.pending.as_ref().is_some_and(|pending| {
            !directory
                .entries
                .iter()
                .any(|entry| (entry.path.clone(), entry.revision) == pending.key)
        }) {
            self.cancel_pending();
        }
    }
    pub fn submit(
        &mut self,
        directory: &Directory,
        visible: &[usize],
        query: Query,
    ) -> Result<(), String> {
        let Operator::Answer(question) = &query.operator else {
            return Err("Expected a question command".into());
        };
        if let Some(reason) = policy::excluded(&directory.path) {
            return Err(reason.into());
        }
        let object = directory
            .object
            .ok_or("Open a verified ordinary folder before adding an answer column")?;
        let key = FolderKey {
            path: directory.path.clone(),
            object,
            mount: directory.mount,
        };
        self.folders.entry(key.clone()).or_default();
        self.reconcile(directory, visible);
        let signature = format!("{query:?}");
        let folder = self.folders.get_mut(&key).unwrap();
        if let Some(column) = folder
            .columns
            .iter()
            .find(|column| column.signature == signature)
        {
            folder.focused = column.id;
            return Ok(());
        }
        if folder.columns.len() >= 4 {
            return Err("Each folder can have at most four temporary answer columns".into());
        }
        self.serial = self.serial.wrapping_add(1);
        folder.focused = self.serial;
        let mut column = Column {
            id: self.serial,
            title: question.clone(),
            query,
            signature,
            cells: HashMap::new(),
            todo: visible.iter().copied().collect(),
            finished: 0,
            total: 0,
            plans: Default::default(),
        };
        column.total = visible
            .iter()
            .filter(|&&index| column.skips(&directory.entries[index]).is_none())
            .count();
        folder.columns.push(column);
        folder.paused = false;
        Ok(())
    }
    pub(super) fn next(&mut self, directory: &Directory) -> Option<AnswerJob> {
        if self.pending.is_some() || self.suspended {
            return None;
        }
        let key = self.active.clone()?;
        let folder = self.folders.get_mut(&key)?;
        if folder.paused {
            return None;
        }
        let (column, index) = next_cell(folder, directory, &self.priority)?;
        let entry = directory.entries[index].clone();
        let Operator::Answer(question) = &column.query.operator else {
            return None;
        };
        self.serial = self.serial.wrapping_add(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let job = AnswerJob {
            id: self.serial,
            root: SourceRoot {
                path: key.path.clone(),
                device: key.object.0,
                inode: key.object.1,
                mount: directory.mount,
                session: None,
            },
            entry: entry.clone(),
            question: question.clone(),
            kind: column.query.answer_type,
            cancel: cancel.clone(),
            plans: column.plans.clone(),
        };
        self.pending = Some(Pending {
            id: job.id,
            folder: key,
            column: column.id,
            key: (entry.path, entry.revision),
            index,
            cancel,
        });
        Some(job)
    }
    pub(super) fn accept(&mut self, result: AnswerResult) -> bool {
        if self
            .pending
            .as_ref()
            .is_none_or(|pending| pending.id != result.id)
        {
            return false;
        }
        if self
            .pending
            .as_ref()
            .unwrap()
            .cancel
            .load(Ordering::Relaxed)
        {
            self.cancel_pending();
            return false;
        }
        let pending = self.pending.take().unwrap();
        if self.active.as_ref() != Some(&pending.folder) {
            return false;
        }
        let Some(folder) = self.folders.get_mut(&pending.folder) else {
            return false;
        };
        let Some(column) = folder
            .columns
            .iter_mut()
            .find(|column| column.id == pending.column)
        else {
            return false;
        };
        if matches!(result.answer.state, State::Failed)
            && result.answer.reason.contains("not installed")
        {
            folder.paused = true;
        }
        column
            .cells
            .insert(pending.key.0.clone(), (pending.key.1, result.answer));
        column.finished += 1;
        true
    }
    pub fn sort(&mut self, directory: &Directory, visible: &mut [usize]) {
        let Some(folder) = self.current_mut() else {
            return;
        };
        if let Some((id, descending)) = folder.sort
            && let Some(column) = folder.columns.iter().find(|column| column.id == id)
        {
            let rank = |entry: &Entry| {
                folder
                    .tie_order
                    .get(&entry.path)
                    .filter(|(revision, _)| *revision == entry.revision)
                    .map_or(usize::MAX, |(_, rank)| *rank)
            };
            visible.sort_by(|&a, &b| {
                Answer::compare(
                    column.answer(&directory.entries[a]),
                    column.answer(&directory.entries[b]),
                    descending,
                )
                .then_with(|| rank(&directory.entries[a]).cmp(&rank(&directory.entries[b])))
            });
        }
        folder.tie_order = visible
            .iter()
            .enumerate()
            .map(|(rank, &index)| {
                let entry = &directory.entries[index];
                (entry.path.clone(), (entry.revision, rank))
            })
            .collect();
    }
    pub fn remove(&mut self, id: u64) {
        self.cancel_pending();
        if let Some(folder) = self.current_mut() {
            folder.columns.retain(|column| column.id != id);
            if folder.sort.is_some_and(|sort| sort.0 == id) {
                folder.sort = None;
            }
        }
    }
    pub fn retry(&mut self, id: u64, refresh: bool) {
        self.cancel_pending();
        if let Some(folder) = self.current_mut() {
            folder.paused = false;
            if let Some(column) = folder.columns.iter_mut().find(|column| column.id == id) {
                if let Ok(mut plans) = column.plans.lock() {
                    plans.clear();
                }
                column.cells.retain(|_, (_, answer)| {
                    !refresh
                        && !matches!(
                            answer.state,
                            State::Failed | State::Unavailable | State::Unknown
                        )
                });
                column.todo = folder
                    .visible
                    .iter()
                    .enumerate()
                    .filter_map(|(index, visible)| visible.then_some(index))
                    .collect();
                folder.signature = 0;
            }
        }
    }
}
fn next_cell<'a>(
    folder: &'a mut Folder,
    directory: &Directory,
    priority: &[usize],
) -> Option<(&'a mut Column, usize)> {
    let priority = folder
        .columns
        .iter()
        .enumerate()
        .find_map(|(column, value)| {
            priority
                .iter()
                .copied()
                .find(|&index| {
                    directory
                        .entries
                        .get(index)
                        .is_some_and(|entry| value.eligible(entry))
                })
                .map(|index| (column, index))
        });
    if let Some((column, index)) = priority {
        return Some((&mut folder.columns[column], index));
    }
    for column in &mut folder.columns {
        while let Some(index) = column.todo.pop_front() {
            if directory
                .entries
                .get(index)
                .is_some_and(|entry| column.eligible(entry))
            {
                return Some((column, index));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::filesystem;
    fn read(path: &std::path::Path) -> Directory {
        filesystem::read_directory(path, &AtomicBool::new(false)).unwrap()
    }
    fn fixture() -> (tempfile::TempDir, Directory) {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), "A short story.").unwrap();
        std::fs::write(root.path().join("b.txt"), "A second story.").unwrap();
        std::fs::write(root.path().join("picture.png"), "fixture").unwrap();
        let directory = read(root.path());
        (root, directory)
    }
    fn query() -> Query {
        Query::parse(":q:\"Does the text mention a story?\";type:boolean").unwrap()
    }
    #[test]
    fn still_images_enter_the_answer_queue_without_admitting_audio() {
        let (root, _) = fixture();
        std::fs::write(root.path().join("sound.wav"), "fixture").unwrap();
        let directory = read(root.path());
        let visible: Vec<_> = (0..directory.entries.len()).collect();
        let mut columns = Columns::default();
        columns.submit(&directory, &visible, query()).unwrap();
        let mut names = Vec::new();
        while let Some(job) = columns.next(&directory) {
            names.push(job.entry.name.clone());
            columns.accept(AnswerResult {
                id: job.id,
                answer: Answer::status(State::Unknown, "Fixture"),
            });
        }
        assert!(names.iter().any(|name| name == "picture.png"));
        assert!(!names.iter().any(|name| name == "sound.wav"));
        assert_eq!(names.len(), 3);
    }
    #[test]
    fn a_mount_change_retires_the_column_context_even_with_identical_file_metadata() {
        let (_root, directory) = fixture();
        let visible: Vec<_> = (0..directory.entries.len()).collect();
        let mut columns = Columns::default();
        columns.submit(&directory, &visible, query()).unwrap();
        let job = columns.next(&directory).unwrap();
        let mut changed = directory.clone();
        changed.mount = Some(directory.mount.unwrap().wrapping_add(1));
        columns.reconcile(&changed, &visible);
        assert!(job.cancel.load(Ordering::Relaxed));
        assert_eq!(columns.count(), 0);
        assert!(!columns.accept(AnswerResult {
            id: job.id,
            answer: Answer::status(State::NotFound, "Old mount")
        }));
    }
    #[test]
    fn columns_retain_completed_results_on_return_and_retry_interrupted_jobs() {
        let (_root, directory) = fixture();
        let visible: Vec<_> = (0..directory.entries.len()).collect();
        let mut columns = Columns::default();
        columns.submit(&directory, &visible, query()).unwrap();
        let job = columns.next(&directory).unwrap();
        let id = job.id;
        assert!(columns.accept(AnswerResult {
            id,
            answer: Answer::status(State::NotApplicable, "Test result")
        }));
        let second = columns.next(&directory).unwrap();
        columns.leave();
        assert!(second.cancel.load(Ordering::Relaxed));
        assert!(!columns.accept(AnswerResult {
            id: second.id,
            answer: Answer::status(State::NotFound, "Late")
        }));
        columns.reconcile(&directory, &visible);
        let folder = columns.current().unwrap();
        assert_eq!(folder.columns.len(), 1);
        assert_eq!(folder.columns[0].finished, 1);
        assert!(folder.columns[0].answer(&job.entry).is_some());
        // Service controls may interrupt a job without leaving this folder.
        let interrupted = columns.next(&directory).unwrap();
        assert_eq!(interrupted.entry.path, second.entry.path);
        interrupted.cancel.store(true, Ordering::Relaxed);
        assert!(!columns.accept(AnswerResult {
            id: interrupted.id,
            answer: Answer::status(State::Failed, "Search model request cancelled"),
        }));
        let column = &columns.current().unwrap().columns[0];
        assert_eq!(column.finished, 1);
        assert!(column.answer(&job.entry).is_some());
        assert!(column.answer(&interrupted.entry).is_none());
        let retried = columns.next(&directory).unwrap();
        assert_eq!(retried.entry.path, interrupted.entry.path);
        assert!(!retried.cancel.load(Ordering::Relaxed));
        assert_ne!(retried.id, interrupted.id);
        assert!(!columns.accept(AnswerResult {
            id: interrupted.id,
            answer: Answer::status(State::NotFound, "Obsolete completion"),
        }));
        assert!(columns.pending.as_ref().is_some_and(|p| p.id == retried.id));
    }
    #[test]
    fn retry_preserves_completed_answers_while_refresh_retires_all_cells_and_old_work() {
        let (_root, directory) = fixture();
        let visible: Vec<_> = (0..directory.entries.len()).collect();
        for state in [
            State::Value(super::super::answer::Value::Boolean(false), "false".into()),
            State::NotApplicable,
            State::NotFound,
            State::Ambiguous,
            State::Unknown,
            State::Failed,
            State::Unavailable,
        ] {
            let preserved = !matches!(state, State::Unknown | State::Failed | State::Unavailable);
            let mut columns = Columns::default();
            columns.submit(&directory, &visible, query()).unwrap();
            let first = columns.next(&directory).unwrap();
            assert!(columns.accept(AnswerResult {
                id: first.id,
                answer: Answer::status(state, "Original answer"),
            }));
            let old = columns.next(&directory).unwrap();
            let column = columns.current().unwrap().columns[0].id;
            columns.retry(column, false);
            columns.reconcile(&directory, &visible);
            assert!(old.cancel.load(Ordering::Relaxed));
            assert_eq!(
                columns.current().unwrap().columns[0]
                    .answer(&first.entry)
                    .is_some(),
                preserved
            );
            let replacement = columns.next(&directory).unwrap();
            assert!(!columns.accept(AnswerResult {
                id: old.id,
                answer: Answer::status(State::NotFound, "Obsolete answer"),
            }));
            assert!(
                columns
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.id == replacement.id)
            );
            columns.retry(column, true);
            columns.reconcile(&directory, &visible);
            assert!(replacement.cancel.load(Ordering::Relaxed));
            let refreshed = &columns.current().unwrap().columns[0];
            assert!(refreshed.cells.is_empty());
            assert_eq!(refreshed.finished, 0);
            assert_eq!(refreshed.total, 3);
            assert!(columns.next(&directory).is_some());
        }
    }
    #[test]
    fn replacement_invalidates_pending_and_completed_cells_and_folder_identity() {
        let (root, directory) = fixture();
        let visible: Vec<_> = (0..directory.entries.len()).collect();
        let mut columns = Columns::default();
        columns.submit(&directory, &visible, query()).unwrap();
        let pending = columns.next(&directory).unwrap();
        std::fs::write(root.path().join("replacement"), "New content").unwrap();
        std::fs::rename(root.path().join("replacement"), &pending.entry.path).unwrap();
        let changed = read(root.path());
        columns.reconcile(&changed, &visible);
        assert!(pending.cancel.load(Ordering::Relaxed));
        assert!(!columns.accept(AnswerResult {
            id: pending.id,
            answer: Answer::status(State::NotFound, "Obsolete")
        }));
        let mut replaced = changed.clone();
        replaced.object = Some((0, 1));
        columns.reconcile(&replaced, &visible);
        assert_eq!(
            columns.count(),
            0,
            "A different directory cannot inherit session answers"
        );
    }
    #[test]
    fn removed_columns_and_changed_pipelines_reject_old_results_after_requeue() {
        for remove_column in [false, true] {
            let (_root, directory) = fixture();
            let visible: Vec<_> = (0..directory.entries.len()).collect();
            let mut columns = Columns::default();
            columns.submit(&directory, &visible, query()).unwrap();
            columns.observe_pipeline(Some("version-one".into()));
            let old = columns.next(&directory).unwrap();
            if remove_column {
                let id = columns.current().unwrap().columns[0].id;
                columns.remove(id);
                columns.submit(&directory, &visible, query()).unwrap();
                assert_ne!(columns.current().unwrap().columns[0].id, id);
            } else {
                assert!(columns.observe_pipeline(Some("version-two".into())));
            }
            assert!(old.cancel.load(Ordering::Relaxed));
            let current = columns.next(&directory).unwrap();
            assert_ne!(current.id, old.id);
            assert!(!columns.accept(AnswerResult {
                id: old.id,
                answer: Answer::status(State::NotFound, "Obsolete completion"),
            }));
            assert!(
                columns
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.id == current.id)
            );
            assert_eq!(columns.current().unwrap().columns[0].finished, 0);
            assert!(columns.accept(AnswerResult {
                id: current.id,
                answer: Answer::status(State::NotFound, "Current completion"),
            }));
            assert_eq!(columns.current().unwrap().columns[0].finished, 1);
            assert_eq!(
                columns.current().unwrap().columns[0]
                    .answer(&current.entry)
                    .unwrap()
                    .reason,
                "Current completion"
            );
        }
    }
    #[test]
    fn repeated_questions_focus_existing_column_and_visible_rows_precede_rest() {
        let (_root, directory) = fixture();
        let visible: Vec<_> = (0..directory.entries.len()).collect();
        let mut columns = Columns::default();
        columns.submit(&directory, &visible, query()).unwrap();
        columns.submit(&directory, &visible, query()).unwrap();
        assert_eq!(columns.count(), 1);
        let index = directory
            .entries
            .iter()
            .position(|entry| entry.name == "b.txt")
            .unwrap();
        columns.priority = vec![index];
        let pending = columns.next(&directory).unwrap();
        assert_eq!(pending.entry.name, "b.txt");
        columns.set_suspended(true);
        assert!(pending.cancel.load(Ordering::Relaxed));
        assert!(columns.next(&directory).is_none());
        columns.set_suspended(false);
        assert_eq!(columns.next(&directory).unwrap().entry.name, "b.txt");
        for number in 0..3 {
            columns
                .submit(
                    &directory,
                    &visible,
                    Query::parse(&format!(":q:\"Question {number}\";type:text")).unwrap(),
                )
                .unwrap();
        }
        let original_id = columns.current().unwrap().columns[0].id;
        assert_ne!(columns.current().unwrap().focused, original_id);
        columns.submit(&directory, &visible, query()).unwrap();
        assert_eq!(columns.current().unwrap().focused, original_id);
        assert_eq!(columns.count(), 4);
        assert!(
            columns
                .submit(
                    &directory,
                    &visible,
                    Query::parse(":q:\"Fifth question\"").unwrap()
                )
                .is_err()
        );
    }
    #[test]
    fn equal_answers_keep_prior_display_order_and_pipeline_changes_retire_cached_values() {
        let (_root, directory) = fixture();
        let mut visible: Vec<_> = (0..directory.entries.len()).collect();
        let mut columns = Columns::default();
        columns.submit(&directory, &visible, query()).unwrap();
        assert!(!columns.observe_pipeline(Some("version-one".into())));
        let id = columns.current().unwrap().columns[0].id;
        columns.current_mut().unwrap().sort = Some((id, false));
        let b = directory
            .entries
            .iter()
            .position(|entry| entry.name == "b.txt")
            .unwrap();
        columns.priority = vec![b];
        for _ in 0..2 {
            let job = columns.next(&directory).unwrap();
            columns.accept(AnswerResult {
                id: job.id,
                answer: Answer::status(
                    State::Value(super::super::answer::Value::Boolean(false), "false".into()),
                    "Fixture",
                ),
            });
            visible = (0..directory.entries.len()).collect();
            columns.sort(&directory, &mut visible);
        }
        assert_eq!(directory.entries[visible[0]].name, "b.txt");
        assert!(columns.observe_pipeline(Some("version-two".into())));
        assert_eq!(columns.current().unwrap().columns[0].finished, 0);
        assert!(columns.current().unwrap().columns[0].cells.is_empty());
        assert!(columns.next(&directory).is_some());
    }
}
