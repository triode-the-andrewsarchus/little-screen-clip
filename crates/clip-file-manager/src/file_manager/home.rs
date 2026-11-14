//! Deliberately pinned path references, separate from clipboard and folder views.
use super::local_state;
use little_clip_core::{BackgroundJobs, JobError, jobs::task::Task};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Pin {
    pub id: Uuid,
    #[serde(with = "little_clip_core::path_serde")]
    pub path: PathBuf,
    pub folder: bool,
}
impl Pin {
    pub fn new(path: PathBuf, folder: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            path,
            folder,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    pins: Vec<Pin>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            pins: Vec::new(),
        }
    }
}
impl State {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("Unsupported Home version; file left unchanged".into());
        }
        if self.pins.len() > 512 {
            return Err("Home exceeds 512 pins".into());
        }
        let mut paths = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for pin in &self.pins {
            let bytes = pin.path.as_os_str().as_bytes();
            if !pin.path.is_absolute()
                || bytes.len() > 4096
                || bytes.contains(&0)
                || pin
                    .path
                    .components()
                    .any(|c| matches!(c, Component::ParentDir))
                || pin.id.is_nil()
                || !ids.insert(pin.id)
                || !paths.insert(&pin.path)
            {
                return Err("Invalid or duplicate Home pin".into());
            }
        }
        Ok(())
    }
    fn apply(&mut self, change: Change) {
        match change {
            Change::Add(pins) => {
                for pin in pins {
                    if !self.pins.iter().any(|p| p.path == pin.path) {
                        self.pins.push(pin);
                    }
                }
            }
            Change::Remove(id) => self.pins.retain(|p| p.id != id),
        }
    }
}
#[derive(Clone)]
enum Change {
    Add(Vec<Pin>),
    Remove(Uuid),
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
pub(super) struct Home {
    pub open: bool,
    pub opening: Option<PathBuf>,
    path: Option<PathBuf>,
    state: State,
    loaded: bool,
    reload: bool,
    queued: Option<Change>,
    pending: Option<Pending>,
    pub error: Option<String>,
}
impl Home {
    pub fn for_user(home: &Path) -> Self {
        let root = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/state"));
        Self::new(root.join("little-screen-clip/home.json"))
    }
    fn new(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            reload: true,
            ..Self::default()
        }
    }
    pub fn pins(&self) -> &[Pin] {
        &self.state.pins
    }
    pub fn busy(&self) -> bool {
        self.reload || self.pending.is_some() || self.queued.is_some()
    }
    pub fn writing(&self) -> bool {
        self.queued.is_some()
            || self
                .pending
                .as_ref()
                .is_some_and(|p| matches!(p.action, Action::Change(_)))
    }
    pub fn ready(&self) -> bool {
        self.path.is_some() && self.loaded && !self.busy() && self.error.is_none()
    }
    pub fn reload(&mut self) {
        if !self.writing() {
            self.reload = self.path.is_some();
        }
    }
    pub fn add(&mut self, pins: Vec<Pin>) {
        if self.ready() {
            self.queued = Some(Change::Add(pins));
        }
    }
    pub fn remove(&mut self, id: Uuid) {
        if self.ready() {
            self.queued = Some(Change::Remove(id));
        }
    }
    pub fn tick(&mut self, jobs: &BackgroundJobs) {
        if let Some(result) = self.pending.as_ref().and_then(|p| p.task.poll()) {
            let pending = self.pending.take().expect("polled Home task");
            match result {
                Err(JobError::QueueFull) => match pending.action {
                    Action::Load => self.reload = true,
                    Action::Change(change) => self.queued = Some(change),
                },
                Ok(Ok(state)) => {
                    self.state = state;
                    self.loaded = true;
                    self.error = None;
                }
                result => {
                    self.error = Some(match result {
                        Ok(Err(error)) => error,
                        Err(error) => error.to_string(),
                        _ => unreachable!(),
                    })
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
                let task = match action.clone() {
                    Action::Load => Task::start(jobs, Duration::from_secs(10), move |_| {
                        let (state, _) = local_state::read::<State>(&path)?;
                        state.validate()?;
                        Ok(state)
                    })
                    .with_response_deadline(Duration::from_secs(10)),
                    Action::Change(change) => {
                        Task::start_unbounded(jobs, move |_| save(&path, change))
                    }
                };
                self.pending = Some(Pending { action, task });
            }
        }
    }
}
fn save(path: &Path, change: Change) -> Result<State, String> {
    local_state::update(path, ".home.lock", State::validate, |state| {
        state.apply(change)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn updates_merge_and_duplicates_keep_identity_without_touching_targets() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("state/home.json");
        let a = Pin::new(root.path().join("missing-folder"), true);
        let b = Pin::new(root.path().join("missing-file"), false);
        save(&path, Change::Add(vec![a.clone()])).unwrap();
        let state = save(
            &path,
            Change::Add(vec![Pin::new(a.path.clone(), false), b.clone()]),
        )
        .unwrap();
        assert_eq!(state.pins.len(), 2);
        assert_eq!(state.pins[0].id, a.id);
        assert!(state.pins[0].folder);
        let state = save(&path, Change::Remove(a.id)).unwrap();
        assert_eq!(state.pins[0].id, b.id);
        assert!(!a.path.exists() && !b.path.exists());
    }
    #[test]
    fn invalid_newer_and_oversized_state_cannot_be_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("home.json");
        for bytes in [
            b"bad json".to_vec(),
            br#"{"version":2,"pins":[]}"#.to_vec(),
            vec![b' '; 1024 * 1024 + 1],
        ] {
            std::fs::write(&path, &bytes).unwrap();
            assert!(save(&path, Change::Add(vec![Pin::new("/example".into(), true)])).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        std::fs::remove_file(&path).unwrap();
        for p in [PathBuf::from("relative"), PathBuf::from("/a/../b")] {
            assert!(save(&path, Change::Add(vec![Pin::new(p, false)])).is_err());
            assert!(!path.exists());
        }
    }
    #[test]
    fn unix_path_bytes_round_trip_and_locked_updates_are_explicit_failures() {
        use std::os::unix::ffi::OsStringExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("home.json");
        let target = root
            .path()
            .join(std::ffi::OsString::from_vec(b"sample-\xff".to_vec()));
        let pin = Pin::new(target.clone(), false);
        save(&path, Change::Add(vec![pin.clone()])).unwrap();
        let directory =
            little_clip_core::kernel_interface::create_content_directory(root.path()).unwrap();
        let lock = local_state::lock(&directory, ".home.lock").unwrap();
        assert!(save(&path, Change::Remove(pin.id)).is_err());
        drop(lock);
        let (state, _) = local_state::read::<State>(&path).unwrap();
        assert_eq!(state.pins[0].path, target);
    }
}
