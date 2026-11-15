//! Deliberately pinned path references, separate from clipboard and folder views.
mod board;
mod session;
use super::local_state;
pub(in crate::file_manager) use board::{Board, Card, Content};
pub(in crate::file_manager) use session::Session;
const STATE_LIMIT: usize = 2 * 1024 * 1024;
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
    #[serde(default)]
    board: Board,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: 2,
            pins: Vec::new(),
            board: Board::default(),
        }
    }
}
impl State {
    fn validate(&self) -> Result<(), String> {
        if ![1, 2].contains(&self.version) {
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
        self.board.validate(&self.pins)
    }
    fn apply(&mut self, change: Change) -> Result<(), String> {
        if self.version == 1 {
            for pin in &self.pins {
                self.board.add_shortcut(pin.id);
            }
            self.version = 2;
        }
        let previous = self.board.clone();
        match change {
            Change::Add(pins) => {
                for pin in pins {
                    if !self.pins.iter().any(|p| p.path == pin.path) {
                        self.board.add_shortcut(pin.id);
                        self.pins.push(pin);
                    }
                }
            }
            Change::Remove(id) => {
                self.pins.retain(|p| p.id != id);
                self.board.remove_pin(id);
            }
            Change::Board {
                expected,
                mut board,
                ..
            } => {
                if expected != self.board.revision {
                    return Err(
                        "Home was rearranged in another window; reload before replacing its layout"
                            .into(),
                    );
                }
                board.retain_artwork_bindings()?;
                self.board = board;
            }
        }
        if previous != self.board {
            self.board.revision = previous.revision + 1;
        }
        Ok(())
    }
}
#[derive(Clone)]
enum Change {
    Add(Vec<Pin>),
    Remove(Uuid),
    Board {
        expected: u64,
        board: Board,
        generation: Option<u64>,
    },
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
    pub session: Session,
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
    pub fn canvas_parts(&mut self) -> (&[Pin], &mut Session) {
        (&self.state.pins, &mut self.session)
    }
    pub fn pins(&self) -> &[Pin] {
        &self.state.pins
    }
    pub fn busy(&self) -> bool {
        self.reload
            || self.pending.is_some()
            || self.queued.is_some()
            || self.session.canvas.busy()
            || (self.session.dirty() && self.error.is_none())
    }
    pub fn writing(&self) -> bool {
        self.session.dirty()
            || self.session.canvas.busy()
            || self.queued.is_some()
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
    pub fn editing_allowed(&self) -> bool {
        self.loaded
            && (self.error.is_none() || self.session.dirty())
            && !self.reload
            && self
                .queued
                .as_ref()
                .is_none_or(|change| matches!(change, Change::Board { .. }))
            && self.pending.as_ref().is_none_or(|pending| {
                matches!(pending.action, Action::Change(Change::Board { .. }))
            })
    }
    pub fn poll_canvas(&mut self, ctx: &egui::Context) {
        self.session.poll(ctx);
    }
    pub fn prepare_close(&mut self) -> bool {
        if self.editing_allowed() {
            self.session.commit_labels();
        }
        if !self.session.allow_leave() {
            self.open = true;
            return false;
        }
        true
    }
    pub fn retry_board(&mut self) {
        if self.session.dirty() {
            self.error = None;
        } else {
            self.reload();
        }
    }
    pub fn discard_board(&mut self) {
        if self.pending.is_none() && self.queued.is_none() {
            self.session.discard(&self.state.board);
            self.error = None;
            self.reload();
        }
    }
    fn accept(&mut self, state: State, action: Action) {
        match action {
            Action::Change(Change::Board {
                generation: Some(generation),
                ..
            }) => self.session.acknowledge(&state.board, generation),
            Action::Change(Change::Add(_) | Change::Remove(_)) => {
                self.session.references_changed(&state.board, &state.pins)
            }
            _ => self.session.install(&state.board),
        }
        if state.version == 1 {
            let mut board = state.board.clone();
            for pin in &state.pins {
                board.add_shortcut(pin.id);
            }
            self.queued = Some(Change::Board {
                expected: state.board.revision,
                board,
                generation: None,
            });
        }
        self.state = state;
        self.loaded = true;
        self.error = None;
    }
    fn queue_draft(&mut self) {
        if self.pending.is_none()
            && self.queued.is_none()
            && self.loaded
            && !self.reload
            && self.error.is_none()
            && self.session.due()
        {
            self.queued = Some(Change::Board {
                expected: self.session.board.revision,
                board: self.session.board.clone(),
                generation: Some(self.session.generation()),
            });
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
                Ok(Ok(state)) => self.accept(state, pending.action),
                result => {
                    self.error = Some(match result {
                        Ok(Err(error)) => error,
                        Err(error) => error.to_string(),
                        _ => unreachable!(),
                    })
                }
            }
        }
        self.queue_draft();
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
                        let (state, _) = local_state::read::<State>(&path, STATE_LIMIT)?;
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
    local_state::update(path, ".home.lock", STATE_LIMIT, State::validate, |state| {
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
            br#"{"version":999,"pins":[]}"#.to_vec(),
            vec![b' '; STATE_LIMIT + 1],
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
        let (state, _) = local_state::read::<State>(&path, STATE_LIMIT).unwrap();
        assert_eq!(state.pins[0].path, target);
    }
}

#[cfg(test)]
mod board_persistence_tests {
    use super::*;
    #[test]
    fn old_pins_migrate_into_layout_without_replacing_identity_or_target_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("home.json");
        let pin = Pin::new(root.path().join("outside-folder"), true);
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({"version":1,"pins":[pin]})).unwrap(),
        )
        .unwrap();
        let state = save(&path, Change::Add(vec![pin.clone()])).unwrap();
        assert_eq!(state.version, 2);
        assert_eq!(state.pins[0].id, pin.id);
        assert_eq!(state.pins[0].path, pin.path);
        assert_eq!(state.board.cards.len(), 1);
        state.validate().unwrap();
        assert!(!pin.path.exists());
    }
    #[test]
    fn migration_has_room_for_layout_without_stranding_a_valid_near_limit_legacy_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("home.json");
        let pins: Vec<_> = (0..512)
            .map(|index| Pin::new(format!("/{}-{index}", "p".repeat(1800)).into(), false))
            .collect();
        let bytes = serde_json::to_vec(&serde_json::json!({"version":1,"pins":pins})).unwrap();
        assert!((900_000..1024 * 1024).contains(&bytes.len()));
        std::fs::write(&path, bytes).unwrap();
        let state = save(&path, Change::Add(vec![pins[0].clone()])).unwrap();
        assert_eq!(state.board.cards.len(), 512);
        assert_eq!(state.pins.last().unwrap().id, pins.last().unwrap().id);
        assert!(std::fs::metadata(&path).unwrap().len() <= STATE_LIMIT as u64);
    }
    #[test]
    fn stale_layout_write_is_rejected_without_losing_the_other_windows_arrangement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("home.json");
        let initial = save(&path, Change::Add(vec![Pin::new("/target".into(), false)])).unwrap();
        let mut first = initial.board.clone();
        first.title = "Window one".into();
        save(
            &path,
            Change::Board {
                expected: initial.board.revision,
                board: first,
                generation: None,
            },
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut second = initial.board.clone();
        second.title = "Stale window".into();
        assert!(
            save(
                &path,
                Change::Board {
                    expected: initial.board.revision,
                    board: second,
                    generation: None
                }
            )
            .unwrap_err()
            .contains("another window")
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
}

#[cfg(test)]
mod session_delivery_tests {
    use super::*;
    fn await_state(home: &mut Home, jobs: &BackgroundJobs, predicate: impl Fn(&Home) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            home.tick(jobs);
            if predicate(home) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Home delivery did not settle: {:?}",
                home.error
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    #[test]
    fn draft_changes_defer_close_until_the_accepted_state_is_published() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("home.json");
        let jobs = BackgroundJobs::new(1, 2);
        let mut home = Home::new(path.clone());
        await_state(&mut home, &jobs, Home::ready);
        home.session.arrange = true;
        home.session.title_draft = "My desk".into();
        assert!(home.prepare_close());
        assert!(
            home.writing(),
            "debounce cannot release the write/close owner"
        );
        await_state(&mut home, &jobs, |home| !home.writing());
        let (state, _) = local_state::read::<State>(&path, STATE_LIMIT).unwrap();
        assert_eq!(state.board.title, "My desk");
        assert_eq!(home.session.board.revision, state.board.revision);
        assert!(home.ready());
    }
    #[test]
    fn rejected_conflicting_save_retains_the_draft_until_explicit_discard_and_reload() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("home.json");
        let jobs = BackgroundJobs::new(1, 2);
        let mut home = Home::new(path.clone());
        await_state(&mut home, &jobs, Home::ready);
        home.session
            .edit(|board| board.title = "Local unsaved".into());
        let mut other = home.state.board.clone();
        other.title = "Other window".into();
        save(
            &path,
            Change::Board {
                expected: other.revision,
                board: other,
                generation: None,
            },
        )
        .unwrap();
        await_state(&mut home, &jobs, |home| home.error.is_some());
        assert_eq!(home.session.board.title, "Local unsaved");
        assert!(home.writing());
        assert!(
            home.editing_allowed(),
            "the draft remains editable for recovery"
        );
        assert!(home.error.as_ref().unwrap().contains("another window"));
        home.discard_board();
        await_state(&mut home, &jobs, Home::ready);
        assert_eq!(home.session.board.title, "Other window");
        assert!(!home.writing());
    }
}
