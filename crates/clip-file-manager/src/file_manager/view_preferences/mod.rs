//! Local folder presentation choices; no content-directory sidecars.
mod store;
#[cfg(test)]
mod tests;
mod ui;

use super::BrowserView;

use little_clip_core::{BackgroundJobs, JobError, jobs::task::Task};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preference {
    pub view: BrowserView,
    pub descendants: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    #[serde(with = "little_clip_core::path_serde")]
    path: PathBuf,
    preference: Preference,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    folders: Vec<Record>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            folders: Vec::new(),
        }
    }
}
impl State {
    fn exact(&self, path: &Path) -> Option<Preference> {
        self.folders
            .binary_search_by(|record| record.path.as_path().cmp(path))
            .ok()
            .map(|index| self.folders[index].preference)
    }
    fn effective(&self, path: &Path) -> Option<(&Path, Preference)> {
        path.ancestors().enumerate().find_map(|(depth, ancestor)| {
            let index = self
                .folders
                .binary_search_by(|record| record.path.as_path().cmp(ancestor))
                .ok()?;
            let record = &self.folders[index];
            (depth == 0 || record.preference.descendants)
                .then_some((record.path.as_path(), record.preference))
        })
    }
    fn update(&mut self, path: &Path, preference: Option<Preference>) {
        self.folders.retain(|r| r.path != path);
        if let Some(preference) = preference {
            self.folders.push(Record {
                path: path.into(),
                preference,
            });
        }
        self.folders.sort_by(|a, b| a.path.cmp(&b.path));
    }
}
#[derive(Clone)]
struct Change {
    path: PathBuf,
    preference: Option<Preference>,
    apply_epoch: Option<u64>,
}
#[derive(Clone)]
enum Action {
    Load,
    Change(Change),
}
struct Pending {
    action: Action,
    task: Task<Result<State, String>>,
}
#[derive(Default)]
pub(super) struct Preferences {
    path: Option<PathBuf>,
    state: State,
    loaded: bool,
    reload: bool,
    queued: Option<Change>,
    pending: Option<Pending>,
    error: Option<String>,
    pub report: Option<String>,
}
#[derive(Default)]
pub(super) struct Update {
    pub changed: bool,
    pub applied: Option<(PathBuf, u64)>,
}
impl Preferences {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            reload: true,
            ..Default::default()
        }
    }
    pub fn for_user(home: &Path) -> Self {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/state"));
        Self::new(state.join("little-screen-clip/folder-views.json"))
    }
    pub fn reload(&mut self) {
        self.reload = self.path.is_some();
    }
    pub fn active(&self) -> bool {
        self.pending.is_some() || self.queued.is_some() || self.reload
    }
    pub fn writing(&self) -> bool {
        self.queued.is_some()
            || self
                .pending
                .as_ref()
                .is_some_and(|p| matches!(p.action, Action::Change(_)))
    }
    fn ready(&self) -> bool {
        self.path.is_some() && self.loaded && !self.active()
    }
    pub fn effective(&self, path: &Path) -> Option<BrowserView> {
        self.state
            .effective(path)
            .map(|(_, preference)| preference.view)
    }
    fn change(&mut self, path: PathBuf, preference: Option<Preference>, apply_epoch: Option<u64>) {
        if self.ready() {
            self.queued = Some(Change {
                path,
                preference,
                apply_epoch,
            });
        }
    }
    pub fn tick(&mut self, jobs: &BackgroundJobs) -> Update {
        let mut update = Update::default();
        if let Some(result) = self.pending.as_ref().and_then(|p| p.task.poll()) {
            let pending = self.pending.take().expect("polled preference task");
            match result {
                Err(JobError::QueueFull) => match pending.action {
                    Action::Load => self.reload = true,
                    Action::Change(change) => self.queued = Some(change),
                },
                Ok(Ok(state)) => {
                    self.state = state;
                    self.loaded = true;
                    self.error = None;
                    update.changed = true;
                    if let Action::Change(change) = pending.action {
                        update.applied = change.apply_epoch.map(|epoch| (change.path, epoch));
                    }
                }
                result => {
                    let error = match result {
                        Ok(Err(error)) => error,
                        Err(error) => error.to_string(),
                        _ => unreachable!(),
                    };
                    self.report = Some(format!("Folder preferences: {error}"));
                    self.error = Some(error);
                }
            }
        }
        if self.pending.is_none()
            && let Some(path) = self.path.clone()
        {
            let action = self
                .queued
                .take()
                .map(Action::Change)
                .or_else(|| std::mem::take(&mut self.reload).then_some(Action::Load));
            if let Some(action) = action {
                self.start(jobs, path, action);
            }
        }
        update
    }
    fn start(&mut self, jobs: &BackgroundJobs, path: PathBuf, action: Action) {
        let task = match action.clone() {
            Action::Load => Task::start(jobs, Duration::from_secs(10), move |_| {
                store::read(&path).map(|(state, _)| state)
            })
            .with_response_deadline(Duration::from_secs(10)),
            // Accepted writes retain an unbounded delivery owner through publication.
            Action::Change(change) => {
                Task::start_unbounded(jobs, move |_| store::save(&path, &change))
            }
        };
        self.pending = Some(Pending { action, task });
    }
}
