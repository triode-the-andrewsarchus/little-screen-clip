//! Shared local organization owner, using the existing guarded state worker.
mod model;
mod sets;
use super::local_state;
use little_clip_core::{BackgroundJobs, JobError, jobs::task::Task};
pub(in crate::file_manager) use model::{Action, Color, Definitions, Observation, Snapshot};
use model::{Change, DefinitionEdit, LIMIT, State};
pub(in crate::file_manager) use sets::{Filter, Group, SetChange};
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
    pub draft_base: Option<Definitions>,
    pub filter: Filter,
    pub set_editor: Option<(Option<Group>, Group)>,
    pub error: Option<String>,
    pub relocations: super::reference_updates::Deferred,
}
impl Organization {
    pub fn for_user(home: &Path) -> Self {
        Self::new(local_state::directory_for_user(home).join("organization.json"))
    }
    fn new(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            reload: true,
            ..Self::default()
        }
    }
    pub fn active(&self) -> bool {
        self.relocations.active() || self.reload || self.queued.is_some() || self.pending.is_some()
    }
    pub fn writing(&self) -> bool {
        self.relocations.writing()
            || self.queued.is_some()
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| matches!(pending.request, Request::Change(_)))
    }
    pub fn ready(&self) -> bool {
        self.loaded && self.path.is_some() && !self.active() && !self.relocations.failed()
    }
    pub fn follow_rename(&mut self, source: &Path, destination: &Path) {
        if self.path.is_some() {
            self.relocations.push(source, destination);
        }
    }
    pub fn reload(&mut self) {
        if !self.active() {
            self.reload = self.path.is_some();
            self.error = None;
            self.draft = None;
            self.draft_base = None;
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
                    base: self
                        .draft_base
                        .clone()
                        .unwrap_or_else(|| self.snapshot.definitions().clone()),
                    next,
                }),
                action,
            }));
            self.error = None;
        }
    }
    pub fn prepare_close(&mut self) -> bool {
        if self.set_editor.is_some() {
            self.error = Some("Save or discard the sorting-set draft before closing".into());
            return false;
        }
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
    fn accept_snapshot(&mut self, snapshot: Snapshot, request: &Request) {
        if matches!(*request, Request::Change(ref change) if matches!(change.action, Action::Sets(SetChange::Create(_) | SetChange::Edit { .. })))
        {
            self.set_editor = None;
        }
        let foreign = matches!(*request,Request::Change(ref change) if matches!(change.action,Action::Relocate(_)));
        let dirty = self.draft.as_ref().is_some_and(|draft| {
            draft
                != self
                    .draft_base
                    .as_ref()
                    .unwrap_or_else(|| self.snapshot.definitions())
        });
        if foreign && dirty && self.draft_base.is_none() {
            self.draft_base = Some(self.snapshot.definitions().clone());
        }
        self.snapshot = Arc::new(snapshot);
        if let Filter::Group(id) = self.filter
            && self.snapshot.sets.group(id).is_none()
        {
            self.filter = Filter::All;
        }
        self.loaded = true;
        if !foreign || !dirty {
            self.draft = None;
            self.draft_base = None;
        }
        self.error = None;
    }
    fn reject_request(&mut self, request: Request, error: String) {
        if let Request::Change(change) = request
            && let Action::Relocate(relocations) = &change.action
        {
            self.relocations.reject(relocations.clone(), error);
            self.error = None;
        } else {
            self.error = Some(error);
        }
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
                Ok(Ok(snapshot)) => self.accept_snapshot(snapshot, &pending.request),
                result => {
                    let error = match result {
                        Ok(Err(error)) => error,
                        Err(error) => error.to_string(),
                        _ => unreachable!(),
                    };
                    self.reject_request(pending.request, error);
                }
            }
        }
        if self.pending.is_none()
            && let Some(path) = self.path.clone()
        {
            let request = self
                .queued
                .take()
                .map(Request::Change)
                .or_else(|| {
                    self.relocations.next().map(|change| {
                        Request::Change(Arc::new(Change {
                            definitions: None,
                            action: Action::Relocate(change),
                        }))
                    })
                })
                .or_else(|| std::mem::take(&mut self.reload).then_some(Request::Load));
            if let Some(request) = request {
                let task = match request.clone() {
                    Request::Load => Task::start(jobs, Duration::from_secs(10), move |_| {
                        let (state, _) = local_state::read::<State>(&path, LIMIT)?;
                        state.validate()?;
                        Ok(Snapshot::new(state))
                    })
                    .with_response_deadline(Duration::from_secs(10)),
                    Request::Change(change) => {
                        Task::start_unbounded(jobs, move |_| save(&path, &change))
                    }
                };
                self.pending = Some(Pending { request, task });
            }
        }
    }
}

fn save(path: &Path, change: &Change) -> Result<Snapshot, String> {
    local_state::update_when(
        path,
        ".organization.lock",
        LIMIT,
        State::validate,
        |state| {
            let changed = match &change.action {
                Action::Relocate(changes) => state.relocation_changes(changes),
                _ => true,
            };
            if changed {
                state.apply(change.clone())?;
            }
            Ok(changed)
        },
    )
    .map(Snapshot::new)
}

// Publication can change both classification badges and which files are visible.
impl super::FileManager {
    pub(super) fn tick_organization(&mut self) {
        let previous = self.organization.snapshot.clone();
        self.organization.tick(&local_state::IO);
        if !Arc::ptr_eq(&previous, &self.organization.snapshot) {
            self.rebuild_visible();
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
    fn renamed_marks_sets_and_recovery_follow_the_file_without_saving_definition_drafts() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("organization.json");
        let old = root.path().join("old.txt");
        let new = root.path().join("new.txt");
        std::fs::write(&old, b"reviewed").unwrap();
        let jobs = BackgroundJobs::new(1, 4);
        let mut owner = Organization::new(path.clone());
        settle(&mut owner, &jobs);
        let entry = super::super::filesystem::read_directory(
            root.path(),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == old)
        .unwrap();
        owner.change(Action::Enable(true));
        settle(&mut owner, &jobs);
        owner.change(Action::Assign(
            vec![Observation::for_entry(&entry).unwrap()],
            Some(Color::Green),
        ));
        settle(&mut owner, &jobs);
        let group = owner.snapshot.sets.groups[0].id;
        owner.change(Action::Sets(SetChange::Assign {
            paths: vec![old.clone()],
            group: Some(group),
        }));
        settle(&mut owner, &jobs);
        owner.change(Action::Clear);
        settle(&mut owner, &jobs);
        owner.draft_base = Some(owner.snapshot.definitions().clone());
        let mut draft = owner.snapshot.definitions().clone();
        draft.meanings[0] = "My red".into();
        owner.draft = Some(draft);
        let mut other = Organization::new(path.clone());
        settle(&mut other, &jobs);
        let mut remote = other.snapshot.definitions().clone();
        remote.name = "Remote name".into();
        other.draft = Some(remote);
        other.change(Action::Definitions);
        settle(&mut other, &jobs);
        std::fs::rename(&old, &new).unwrap();
        owner.follow_rename(&old, &new);
        assert!(owner.writing());
        settle(&mut owner, &jobs);
        assert!(!owner.relocations.failed());
        assert_eq!(owner.draft.as_ref().unwrap().meanings[0], "My red");
        assert_eq!(owner.snapshot.definitions().name, "Remote name");
        assert!(owner.snapshot.sets.matches(&new, Filter::Group(group)));
        assert!(!owner.snapshot.sets.matches(&old, Filter::Group(group)));
        owner.change(Action::Restore(owner.snapshot.recovery_revision()));
        settle(&mut owner, &jobs);
        assert_eq!(
            owner.snapshot.definitions().name,
            "Remote name",
            "unchanged draft fields must preserve remote labels"
        );
        assert_eq!(owner.snapshot.definitions().meanings[0], "My red");
        let entry = super::super::filesystem::read_directory(
            root.path(),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap()
        .entries
        .into_iter()
        .find(|e| e.path == new)
        .unwrap();
        assert_eq!(owner.snapshot.color(&entry), Some(Color::Green));
        let mut reopened = Organization::new(path);
        settle(&mut reopened, &jobs);
        assert_eq!(reopened.snapshot.color(&entry), Some(Color::Green));
        assert_eq!(std::fs::read(new).unwrap(), b"reviewed");
    }
    #[test]
    fn set_filters_keep_folder_routes_and_reconcile_hidden_selection() {
        let root = tempfile::tempdir().unwrap();
        for name in ["keep.txt", "other.txt"] {
            std::fs::write(root.path().join(name), name).unwrap();
        }
        std::fs::create_dir(root.path().join("folder")).unwrap();
        let mut browser = super::super::FileManager::new(root.path().into());
        let jobs = BackgroundJobs::new(2, 16);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while browser.location().is_none() {
            browser.tick(&jobs);
            assert!(
                std::time::Instant::now() < deadline,
                "initial directory did not load"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        browser.select(root.path().join("other.txt"));
        let mut state = State::default();
        let group = uuid::Uuid::from_u128(1);
        state
            .apply(Change {
                definitions: None,
                action: Action::Sets(SetChange::Assign {
                    paths: vec![root.path().join("keep.txt")],
                    group: Some(group),
                }),
            })
            .unwrap();
        browser.organization.snapshot = Arc::new(Snapshot::new(state));
        browser.organization.filter = Filter::Group(group);
        browser.rebuild_visible();
        let names: Vec<_> = browser
            .displayed_entries()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(names, vec!["folder", "keep.txt"]);
        assert!(
            !browser
                .location()
                .unwrap()
                .selection
                .contains(&root.path().join("other.txt"))
        );
        browser.organization.filter = Filter::Unassigned;
        browser.rebuild_visible();
        let names: Vec<_> = browser
            .displayed_entries()
            .iter()
            .map(|e| e.name.clone())
            .collect();
        assert_eq!(names, vec!["folder", "other.txt"]);
    }
    #[test]
    fn sorting_sets_merge_assignments_preserve_unsubmitted_drafts_and_reopen() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("organization.json");
        let jobs = BackgroundJobs::new(1, 4);
        let mut first = Organization::new(path.clone());
        let mut second = Organization::new(path.clone());
        settle(&mut first, &jobs);
        settle(&mut second, &jobs);
        let a = first.snapshot.sets.groups[0].id;
        let b = first.snapshot.sets.groups[1].id;
        first.set_editor = Some((None, Group::new("Draft", None)));
        first.change(Action::Sets(SetChange::Assign {
            paths: vec!["/first".into()],
            group: Some(a),
        }));
        settle(&mut first, &jobs);
        assert!(
            first.set_editor.is_some(),
            "assigning files must not discard an unapplied group draft"
        );
        assert!(!first.prepare_close());
        first.set_editor = None;
        second.change(Action::Sets(SetChange::Assign {
            paths: vec!["/second".into()],
            group: Some(b),
        }));
        settle(&mut second, &jobs);
        let mut reopened = Organization::new(path);
        settle(&mut reopened, &jobs);
        assert!(
            reopened
                .snapshot
                .sets
                .matches(Path::new("/first"), Filter::Group(a))
        );
        assert!(
            reopened
                .snapshot
                .sets
                .matches(Path::new("/second"), Filter::Group(b))
        );
        let child = Group::new("Keep", Some(a));
        let id = child.id;
        reopened.set_editor = Some((None, child.clone()));
        reopened.change(Action::Sets(SetChange::Create(child)));
        settle(&mut reopened, &jobs);
        assert!(reopened.set_editor.is_none());
        assert_eq!(reopened.snapshot.sets.label(id), "A / Keep");
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
