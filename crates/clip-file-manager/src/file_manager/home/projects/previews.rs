//! Visible task reads have a separate bounded queue; accepted writes survive retirement.
use super::{Record, tasks::Snapshot};
use little_clip_core::{BackgroundJobs, JobError, jobs::task::Task};
use std::{
    collections::BTreeMap,
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};
use uuid::Uuid;
static READS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 2));
const CAPACITY: usize = 24;
const FRESH: Duration = Duration::from_secs(2);
#[derive(Default)]
pub(in crate::file_manager) struct View {
    pub snapshot: Option<Arc<Snapshot>>,
    pub error: Option<String>,
    record: Option<Record>,
    pending: Option<Task<Result<Arc<Snapshot>, String>>>,
    attempted: Option<Instant>,
    seen: u64,
}
#[derive(Clone)]
struct Change {
    snapshot: Arc<Snapshot>,
    index: usize,
    checked: bool,
}
struct Writing {
    change: Change,
    task: Task<Result<Arc<Snapshot>, String>>,
}
#[derive(Default)]
pub(in crate::file_manager) struct Previews {
    entries: BTreeMap<Uuid, View>,
    generation: u64,
    queued: Option<Change>,
    writing: Option<Writing>,
    pub write_error: Option<String>,
}
impl Previews {
    pub fn begin(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn request(&mut self, record: &Record) -> Option<&View> {
        if !self.entries.contains_key(&record.id) && self.entries.len() >= CAPACITY {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, view)| view.seen != self.generation)
                .min_by_key(|(_, view)| view.seen)
                .map(|(id, _)| *id)?;
            self.entries.remove(&oldest);
        }
        let reads = self
            .entries
            .values()
            .filter(|view| view.pending.is_some())
            .count();
        let write = self.writing();
        let view = self.entries.entry(record.id).or_default();
        if view.record.as_ref() != Some(record) {
            *view = View {
                record: Some(record.clone()),
                ..View::default()
            };
        }
        view.seen = self.generation;
        if !write
            && view.pending.is_none()
            && reads < 2
            && view.attempted.is_none_or(|time| time.elapsed() >= FRESH)
        {
            let record = record.clone();
            view.attempted = Some(Instant::now());
            view.pending = Some(
                Task::start(&READS, Duration::from_secs(10), move |_| {
                    Snapshot::load(record).map(Arc::new)
                })
                .with_response_deadline(Duration::from_secs(10)),
            );
        }
        Some(view)
    }
    pub fn end(&mut self) {
        for view in self
            .entries
            .values_mut()
            .filter(|view| view.seen != self.generation)
        {
            view.pending = None;
        }
    }
    pub fn retire(&mut self) {
        for view in self.entries.values_mut() {
            view.pending = None;
        }
    }
    pub fn active(&self) -> bool {
        self.writing() || self.entries.values().any(|view| view.pending.is_some())
    }
    pub fn writing(&self) -> bool {
        self.queued.is_some() || self.writing.is_some()
    }
    pub fn refresh(&mut self, id: Uuid) {
        if !self.writing()
            && let Some(view) = self.entries.get_mut(&id)
        {
            view.pending = None;
            view.attempted = None;
            view.snapshot = None;
            view.error = None;
            self.write_error = None;
        }
    }
    pub fn toggle(&mut self, id: Uuid, index: usize, checked: bool) {
        if !self.writing()
            && self.write_error.is_none()
            && let Some(view) = self.entries.get_mut(&id)
            && view.error.is_none()
            && let Some(snapshot) = &view.snapshot
        {
            view.pending = None;
            self.queued = Some(Change {
                snapshot: snapshot.clone(),
                index,
                checked,
            });
        }
    }
    pub fn tick(&mut self, jobs: &BackgroundJobs) {
        for view in self.entries.values_mut() {
            if let Some(result) = view.pending.as_ref().and_then(Task::poll) {
                view.pending = None;
                match result {
                    Ok(Ok(snapshot)) => {
                        view.snapshot = Some(snapshot);
                        view.error = None;
                    }
                    Err(JobError::QueueFull) => view.attempted = None,
                    Ok(Err(error)) => view.error = Some(error),
                    Err(error) => view.error = Some(error.to_string()),
                }
            }
        }
        self.poll_write();
        if self.writing.is_none()
            && let Some(change) = self.queued.take()
        {
            let request = change.clone();
            let task = Task::start_unbounded(jobs, move |_| {
                request
                    .snapshot
                    .toggle(request.index, request.checked)
                    .map(Arc::new)
            });
            self.writing = Some(Writing { change, task });
        }
    }
    fn poll_write(&mut self) {
        if let Some(result) = self
            .writing
            .as_ref()
            .and_then(|writing| writing.task.poll())
        {
            let writing = self.writing.take().expect("polled task write");
            match result {
                Err(JobError::QueueFull) => self.queued = Some(writing.change),
                Ok(Ok(snapshot)) => {
                    if let Some(view) = self.entries.get_mut(&snapshot.project.id)
                        && view.record.as_ref() == Some(&snapshot.project)
                    {
                        view.snapshot = Some(snapshot);
                        view.error = None;
                        view.attempted = Some(Instant::now());
                    }
                }
                Ok(Err(error)) => {
                    self.write_error = Some(format!("{}: {error}", writing.change.snapshot.title))
                }
                Err(error) => {
                    self.write_error = Some(format!("{}: {error}", writing.change.snapshot.title))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wait(previews: &mut Previews, jobs: &BackgroundJobs, done: impl Fn(&Previews) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !done(previews) {
            previews.tick(jobs);
            assert!(
                Instant::now() < deadline,
                "task preview work did not settle"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn accepted_checkbox_writes_survive_hidden_view_retirement_and_report_stale_failures() {
        let folder = tempfile::tempdir().unwrap();
        let manifest = super::super::source::create(folder.path(), "Work").unwrap();
        let record = Record {
            id: manifest.id,
            title: manifest.title,
            folder: folder.path().into(),
        };
        let jobs = BackgroundJobs::new(1, 2);
        let mut previews = Previews::default();
        previews.begin();
        previews.request(&record);
        previews.end();
        wait(&mut previews, &jobs, |previews| {
            previews.entries[&record.id].snapshot.is_some()
        });
        previews.toggle(record.id, 0, true);
        assert!(previews.writing());
        previews.tick(&jobs);
        previews.retire();
        assert!(previews.writing());
        wait(&mut previews, &jobs, |previews| !previews.writing());
        assert!(previews.write_error.is_none());
        assert!(
            previews.entries[&record.id]
                .snapshot
                .as_ref()
                .unwrap()
                .tasks[0]
                .checked
        );
        assert!(
            std::fs::read_to_string(folder.path().join("TODO.md"))
                .unwrap()
                .contains("[x]")
        );
        let external = b"- [ ] External change\n";
        std::fs::write(folder.path().join("TODO.md"), external).unwrap();
        previews.toggle(record.id, 0, false);
        wait(&mut previews, &jobs, |previews| !previews.writing());
        assert!(previews.write_error.is_some());
        assert_eq!(
            std::fs::read(folder.path().join("TODO.md")).unwrap(),
            external
        );
        previews.refresh(record.id);
        assert!(previews.write_error.is_none());
        previews.request(&record);
        wait(&mut previews, &jobs, |previews| {
            previews.entries[&record.id].pending.is_none()
        });
        assert_eq!(
            previews.entries[&record.id]
                .snapshot
                .as_ref()
                .unwrap()
                .tasks[0]
                .label,
            "External change"
        );
    }
    #[test]
    fn retention_and_pending_reads_remain_bounded_and_hidden_reads_retire() {
        let mut previews = Previews::default();
        previews.begin();
        for _ in 0..100 {
            previews.request(&Record {
                id: Uuid::new_v4(),
                title: "Absent".into(),
                folder: "/missing-project-preview".into(),
            });
        }
        assert_eq!(previews.entries.len(), CAPACITY);
        assert_eq!(
            previews
                .entries
                .values()
                .filter(|view| view.pending.is_some())
                .count(),
            2
        );
        previews.begin();
        previews.end();
        assert!(!previews.active());
        assert_eq!(previews.entries.len(), CAPACITY);
    }
}
