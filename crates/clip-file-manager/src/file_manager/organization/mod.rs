//! Shared local organization owner, using the existing guarded state worker.
mod model;
use super::local_state;
use little_clip_core::{BackgroundJobs, JobError, jobs::task::Task};
pub(in crate::file_manager) use model::{Action, Color, Definitions, Observation, Snapshot};
use model::{Change, DefinitionEdit, LIMIT, State};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[derive(Clone)]
enum Request {
    Load,
    Change(Arc<Change>),
}
struct Pending {
    request: Request,
    task: Task<Result<Snapshot, String>>,
}
#[derive(Default)]
pub(super) struct Organization {
    path: Option<PathBuf>,
    pub snapshot: Arc<Snapshot>,
    loaded: bool,
    reload: bool,
    queued: Option<Arc<Change>>,
    pending: Option<Pending>,
    pub draft: Option<Definitions>,
    pub error: Option<String>,
}
impl Organization {
    pub fn for_user(home: &Path) -> Self {
        let root = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".local/state"));
        Self::new(root.join("little-screen-clip/organization.json"))
    }
    fn new(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            reload: true,
            ..Self::default()
        }
    }
    pub fn active(&self) -> bool {
        self.reload || self.queued.is_some() || self.pending.is_some()
    }
    pub fn writing(&self) -> bool {
        self.queued.is_some()
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| matches!(pending.request, Request::Change(_)))
    }
    pub fn ready(&self) -> bool {
        self.loaded && self.path.is_some() && !self.active()
    }
    pub fn reload(&mut self) {
        if !self.active() {
            self.reload = self.path.is_some();
            self.error = None;
            self.draft = None;
        }
    }
    pub fn refresh_if_clean(&mut self) {
        if self
            .draft
            .as_ref()
            .is_none_or(|draft| draft == self.snapshot.definitions())
        {
            self.reload();
        }
    }
    pub fn change(&mut self, action: Action) {
        if self.ready() {
            let definitions = self
                .draft
                .as_ref()
                .filter(|draft| *draft != self.snapshot.definitions())
                .cloned();
            if let Some(draft) = &definitions
                && let Err(error) = draft.validate()
            {
                self.error = Some(error);
                return;
            }
            self.queued = Some(Arc::new(Change {
                definitions: definitions.map(|next| DefinitionEdit {
                    base: self.snapshot.definitions().clone(),
                    next,
                }),
                action,
            }));
            self.error = None;
        }
    }
    pub fn prepare_close(&mut self) -> bool {
        if self
            .draft
            .as_ref()
            .is_some_and(|draft| draft != self.snapshot.definitions())
        {
            self.change(Action::Definitions);
            return self.error.is_none() && self.writing();
        }
        true
    }
    pub fn tick(&mut self, jobs: &BackgroundJobs) {
        if let Some(result) = self
            .pending
            .as_ref()
            .and_then(|pending| pending.task.poll())
        {
            let pending = self.pending.take().expect("polled organization request");
            match result {
                Err(JobError::QueueFull) => match pending.request {
                    Request::Load => self.reload = true,
                    Request::Change(change) => self.queued = Some(change),
                },
                Ok(Ok(snapshot)) => {
                    self.snapshot = Arc::new(snapshot);
                    self.loaded = true;
                    self.draft = None;
                    self.error = None;
                }
                Ok(Err(error)) => self.error = Some(error),
                Err(error) => self.error = Some(error.to_string()),
            }
        }
        if self.pending.is_none()
            && let Some(path) = self.path.clone()
        {
            let request = self
                .queued
                .take()
                .map(Request::Change)
                .or_else(|| std::mem::take(&mut self.reload).then_some(Request::Load));
            if let Some(request) = request {
                let task = match request.clone() {
                    Request::Load => Task::start(jobs, Duration::from_secs(10), move |_| {
                        let (state, _) = local_state::read::<State>(&path, LIMIT)?;
                        state.validate()?;
                        Ok(Snapshot::new(state))
                    })
                    .with_response_deadline(Duration::from_secs(10)),
                    Request::Change(change) => Task::start_unbounded(jobs, move |_| {
                        local_state::update(
                            &path,
                            ".organization.lock",
                            LIMIT,
                            State::validate,
                            |state| state.apply((*change).clone()),
                        )
                        .map(Snapshot::new)
                    }),
                };
                self.pending = Some(Pending { request, task });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settle(owner: &mut Organization, jobs: &BackgroundJobs) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while owner.active() {
            owner.tick(jobs);
            assert!(
                std::time::Instant::now() < deadline,
                "organization delivery stalled"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn accepted_marks_merge_across_windows_and_definition_close_retains_ownership() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("organization.json");
        let jobs = BackgroundJobs::new(1, 4);
        let mut first = Organization::new(path.clone());
        let mut second = Organization::new(path.clone());
        settle(&mut first, &jobs);
        settle(&mut second, &jobs);
        first.change(Action::Enable(true));
        settle(&mut first, &jobs);
        let item = root.path().join("file.txt");
        std::fs::write(&item, b"content").unwrap();
        let target = Observation {
            path: item.clone(),
            identity: little_clip_core::file_inventory::identity(
                &std::fs::metadata(&item).unwrap(),
            ),
        };
        let mut other_definitions = Definitions::default();
        other_definitions.meanings[2] = "Reviewed".into();
        second.draft = Some(other_definitions);
        second.change(Action::Assign(vec![target], Some(Color::Green)));
        assert!(second.writing());
        settle(&mut second, &jobs);
        assert!(second.error.is_none(), "{:?}", second.error);
        first.draft = Some(Definitions {
            name: "Review session".into(),
            ..Definitions::default()
        });
        assert!(first.prepare_close());
        assert!(first.writing());
        settle(&mut first, &jobs);
        assert_eq!(
            first.snapshot.count(),
            1,
            "definition edits merge rather than replacing a stale snapshot"
        );
        let second_item = root.path().join("second.txt");
        std::fs::write(&second_item, b"second").unwrap();
        let target = Observation {
            path: second_item.clone(),
            identity: little_clip_core::file_inventory::identity(
                &std::fs::metadata(&second_item).unwrap(),
            ),
        };
        second.change(Action::Assign(vec![target], Some(Color::Red)));
        settle(&mut second, &jobs);
        assert_eq!(
            second.snapshot.count(),
            2,
            "a stale window adds an assignment without dropping existing marks"
        );
        let mut reopened = Organization::new(path.clone());
        settle(&mut reopened, &jobs);
        assert_eq!(reopened.snapshot.count(), 2);
        assert_eq!(reopened.snapshot.definitions().name, "Review session");
        assert_eq!(
            reopened.snapshot.definitions().meanings[2],
            "Reviewed",
            "an unchanged local field preserves the other window's edit"
        );
        assert_eq!(std::fs::read(item).unwrap(), b"content");
        let mut other = Organization::new(path.clone());
        settle(&mut other, &jobs);
        reopened.draft = Some(Definitions {
            name: "One".into(),
            ..reopened.snapshot.definitions().clone()
        });
        other.draft = Some(Definitions {
            name: "Two".into(),
            ..other.snapshot.definitions().clone()
        });
        reopened.change(Action::Definitions);
        settle(&mut reopened, &jobs);
        let before_conflict = std::fs::read(&path).unwrap();
        other.change(Action::Definitions);
        settle(&mut other, &jobs);
        assert!(other.error.is_some());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before_conflict,
            "a same-field conflict cannot replace the other window's label"
        );
        let mut future: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        future["version"] = serde_json::json!(99);
        for bytes in [b"broken".to_vec(), serde_json::to_vec(&future).unwrap()] {
            std::fs::write(&path, &bytes).unwrap();
            reopened.change(Action::Clear);
            settle(&mut reopened, &jobs);
            assert!(reopened.error.is_some());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
}
