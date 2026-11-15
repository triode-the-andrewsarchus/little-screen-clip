//! Project registry and accepted writes have a lifecycle independent of Home layout.
mod previews;
pub(in crate::file_manager) use previews::View;
pub(in crate::file_manager) use tasks::Snapshot;
mod registry;
mod source;
mod tasks;
use little_clip_core::{BackgroundJobs, JobError, jobs::task::Task};
pub(in crate::file_manager) use registry::{Conflict, Record};
use registry::{Outcome, State};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

#[derive(Clone)]
enum Action {
    Load,
    Register {
        folder: PathBuf,
        title: Option<String>,
        replace: Option<PathBuf>,
    },
    Remove(Uuid),
}
impl Action {
    fn run(self, path: &Path) -> Result<Outcome, String> {
        match self {
            Self::Load => registry::load(path).map(Outcome::State),
            Self::Register {
                folder,
                title,
                replace,
            } => registry::register(path, &folder, title.as_deref(), replace.as_deref()),
            Self::Remove(id) => registry::remove(path, id).map(Outcome::State),
        }
    }
}
struct Pending {
    action: Action,
    task: Task<Result<Outcome, String>>,
}
#[derive(Default)]
pub(in crate::file_manager) struct Projects {
    path: Option<PathBuf>,
    state: State,
    loaded: bool,
    queued: Option<Action>,
    pending: Option<Pending>,
    pub error: Option<String>,
    pub conflict: Option<Conflict>,
    pub form: Option<(PathBuf, String)>,
    pub previews: previews::Previews,
}
impl Projects {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            queued: Some(Action::Load),
            ..Self::default()
        }
    }
    pub fn records(&self) -> &[Record] {
        &self.state.projects
    }
    pub fn preview_parts(&mut self) -> (&[Record], &mut previews::Previews) {
        (&self.state.projects, &mut self.previews)
    }
    pub fn busy(&self) -> bool {
        self.queued.is_some() || self.pending.is_some() || self.previews.active()
    }
    pub fn writing(&self) -> bool {
        self.previews.writing()
            || self
                .queued
                .as_ref()
                .is_some_and(|action| !matches!(action, Action::Load))
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| !matches!(pending.action, Action::Load))
    }
    pub fn ready(&self) -> bool {
        self.loaded
            && self.queued.is_none()
            && self.pending.is_none()
            && !self.previews.writing()
            && self.error.is_none()
            && self.path.is_some()
    }
    pub fn refresh(&mut self) {
        if self.path.is_some() && !self.busy() {
            self.queued = Some(Action::Load);
            self.error = None;
        }
    }
    pub fn register(&mut self, folder: PathBuf, title: Option<String>) {
        if self.ready() {
            self.queued = Some(Action::Register {
                folder,
                title,
                replace: None,
            });
            self.form = None;
            self.conflict = None;
        }
    }
    pub fn relocate(&mut self) {
        if self.ready()
            && let Some(conflict) = self.conflict.take()
        {
            self.queued = Some(Action::Register {
                folder: conflict.project.folder,
                title: None,
                replace: Some(conflict.previous),
            });
        }
    }
    pub fn remove(&mut self, id: Uuid) {
        if self.ready() {
            self.queued = Some(Action::Remove(id));
        }
    }
    pub fn tick(&mut self, jobs: &BackgroundJobs) {
        self.previews.tick(jobs);
        if let Some(result) = self
            .pending
            .as_ref()
            .and_then(|pending| pending.task.poll())
        {
            let pending = self.pending.take().expect("polled project task");
            match result {
                Err(JobError::QueueFull) => self.queued = Some(pending.action),
                Ok(Ok(Outcome::State(state))) => {
                    self.state = state;
                    self.loaded = true;
                    self.error = None;
                }
                Ok(Ok(Outcome::Conflict(conflict))) => self.conflict = Some(conflict),
                Ok(Err(error)) => self.error = Some(error),
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        if self.pending.is_none()
            && let (Some(path), Some(action)) = (self.path.clone(), self.queued.take())
        {
            let request = action.clone();
            let task = if matches!(action, Action::Load) {
                Task::start(jobs, Duration::from_secs(10), move |_| request.run(&path))
                    .with_response_deadline(Duration::from_secs(10))
            } else {
                Task::start_unbounded(jobs, move |_| request.run(&path))
            };
            self.pending = Some(Pending { action, task });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settle(projects: &mut Projects, jobs: &BackgroundJobs) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while projects.busy() {
            projects.tick(jobs);
            assert!(
                std::time::Instant::now() < deadline,
                "project work did not finish"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(projects.error.is_none(), "{:?}", projects.error);
    }
    #[test]
    fn accepted_creation_and_unregister_retain_close_ownership_without_deleting_content() {
        let local = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let jobs = BackgroundJobs::new(1, 2);
        let path = local.path().join("projects.json");
        let mut projects = Projects::new(path.clone());
        assert!(!projects.writing());
        settle(&mut projects, &jobs);
        projects.register(folder.path().into(), Some("Work".into()));
        assert!(
            projects.writing(),
            "queue admission retains the mutation owner"
        );
        projects.tick(&jobs);
        assert!(
            projects.writing(),
            "accepted work retains the mutation owner"
        );
        settle(&mut projects, &jobs);
        assert!(projects.ready());
        let id = projects.records()[0].id;
        let mut reopened = Projects::new(path);
        settle(&mut reopened, &jobs);
        assert_eq!(reopened.records()[0].id, id);
        projects.remove(id);
        assert!(projects.writing());
        settle(&mut projects, &jobs);
        assert!(projects.records().is_empty());
        assert!(folder.path().join("TODO.md").is_file());
        assert!(folder.path().join(source::MANIFEST).is_file());
    }
}
