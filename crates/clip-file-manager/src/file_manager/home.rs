//! Deliberately pinned path references, separate from clipboard and folder views.
pub(in crate::file_manager) mod artwork;
mod board;
pub(in crate::file_manager) mod drops;
pub(in crate::file_manager) mod folders;
pub(in crate::file_manager) mod ink;
pub(in crate::file_manager) mod projects;
pub(in crate::file_manager) mod recipes;
pub(in crate::file_manager) mod regions;
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
    fn relocate(&mut self, source: &Path, destination: &Path) -> Result<(), String> {
        let mut by_path = std::collections::BTreeMap::new();
        let mut replacements = std::collections::BTreeMap::new();
        for pin in &mut self.pins {
            let moved = super::edits::relocate(&mut pin.path, source, destination);
            if let Some((previous, previous_moved)) = by_path.get(&pin.path).copied() {
                if previous_moved && !moved {
                    replacements.insert(pin.id, previous);
                } else {
                    replacements.insert(previous, pin.id);
                    by_path.insert(pin.path.clone(), (pin.id, moved));
                }
            } else {
                by_path.insert(pin.path.clone(), (pin.id, moved));
            }
        }
        self.pins.retain(|pin| !replacements.contains_key(&pin.id));
        let valid = self.pins.iter().map(|pin| pin.id).collect();
        self.board.retain_pin_refs(&valid, &replacements);
        self.validate()
    }
    fn reference(&mut self, pin: Pin) -> Uuid {
        if let Some(old) = self
            .pins
            .iter()
            .find(|old| old.id == pin.id || old.path == pin.path)
        {
            return old.id;
        }
        let id = pin.id;
        self.pins.push(pin);
        id
    }
    fn install_region(
        &mut self,
        mut region: regions::Region,
        pin: Option<Pin>,
        expected: u64,
        original: Option<regions::Region>,
    ) -> Result<(), String> {
        if expected != self.board.revision {
            return Err(
                "Home changed in another window; reload before saving this click region".into(),
            );
        }
        if self.board.regions.iter().find(|old| old.id == region.id) != original.as_ref() {
            return Err(
                "This click region changed; cancel the draft and reopen Edit action before saving"
                    .into(),
            );
        }
        if let Some(pin) = pin {
            region.action = Some(regions::RegionAction::Open {
                pin: self.reference(pin),
            });
        }
        if let Some(old) = self
            .board
            .regions
            .iter_mut()
            .find(|old| old.id == region.id)
        {
            *old = region;
        } else {
            self.board.regions.push(region);
        }
        Ok(())
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
            Change::Relocate(changes) => {
                for change in changes {
                    self.relocate(&change.source, &change.destination)?;
                }
            }
            Change::Add(pins) => {
                for pin in pins {
                    if !self.pins.iter().any(|p| p.path == pin.path) {
                        self.board.add_shortcut(pin.id);
                        self.pins.push(pin);
                    }
                }
            }
            Change::Import { pins, point, .. } => {
                for (index, pin) in pins.into_iter().enumerate() {
                    let id = self.reference(pin);
                    self.board.cards.push(Card {
                        id: Uuid::new_v4(),
                        rect: [
                            point[0] - 160.0 + index as f64 * 24.0,
                            point[1] - 120.0 + index as f64 * 24.0,
                            320.0,
                            240.0,
                        ],
                        content: Content::Artwork {
                            pin: id,
                            label: String::new(),
                        },
                    });
                }
            }
            Change::Region {
                region,
                pin,
                expected,
                original,
            } => self.install_region(region, pin, expected, original.map(|value| *value))?,
            Change::Folder { pin, point } => {
                let pin = self.reference(pin);
                self.board.cards.push(Card {
                    id: Uuid::new_v4(),
                    rect: [point[0] - 300.0, point[1] - 220.0, 600.0, 440.0],
                    content: Content::Folder {
                        pin,
                        label: String::new(),
                        options: Default::default(),
                    },
                });
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
                self.board = *board;
            }
        }
        if previous != self.board {
            self.board.revision = previous
                .revision
                .checked_add(1)
                .ok_or("Home layout revision exhausted")?;
        }
        Ok(())
    }
}
#[derive(Clone)]
enum Change {
    Add(Vec<Pin>),
    Folder {
        pin: Pin,
        point: [f64; 2],
    },
    Import {
        pins: Vec<Pin>,
        point: [f64; 2],
        token: Option<u64>,
    },
    Region {
        region: regions::Region,
        pin: Option<Pin>,
        expected: u64,
        original: Option<Box<regions::Region>>,
    },
    Remove(Uuid),
    Relocate(Vec<super::reference_updates::Relocation>),
    Board {
        expected: u64,
        board: Box<Board>,
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
    pub projects: projects::Projects,
    pub opening: Option<PathBuf>,
    pub artwork: artwork::Assets,
    pub folders: folders::Previews,
    pub folder_filters: BTreeSet<Uuid>,
    pub drop_rect: Option<egui::Rect>,
    pub region_ui: regions::Interaction,
    pub ink: ink::Ink,
    pub drops: drops::Drops,
    pub recipes: recipes::Recipes,
    pub native_results: Vec<(u64, bool)>,
    path: Option<PathBuf>,
    state: State,
    loaded: bool,
    reload: bool,
    queued: Option<Change>,
    pending: Option<Pending>,
    pub error: Option<String>,
    pub relocations: super::reference_updates::Deferred,
}
impl Home {
    pub fn for_user(home: &Path) -> Self {
        Self::new(local_state::directory_for_user(home).join("home.json"))
    }
    fn new(path: PathBuf) -> Self {
        Self {
            projects: projects::Projects::new(path.with_file_name("projects.json")),
            path: Some(path),
            reload: true,
            ..Self::default()
        }
    }
    pub fn canvas_parts(
        &mut self,
    ) -> (
        &[Pin],
        &mut Session,
        &mut projects::Projects,
        &mut artwork::Assets,
        &mut folders::Previews,
        &mut ink::Ink,
    ) {
        (
            &self.state.pins,
            &mut self.session,
            &mut self.projects,
            &mut self.artwork,
            &mut self.folders,
            &mut self.ink,
        )
    }
    pub fn pins(&self) -> &[Pin] {
        &self.state.pins
    }
    pub fn busy(&self) -> bool {
        self.relocations.active()
            || self.reload
            || self.pending.is_some()
            || self.queued.is_some()
            || self.session.canvas.busy()
            || (self.session.dirty() && self.error.is_none())
    }
    pub fn writing(&self) -> bool {
        self.ink.drawing()
            || self.recipes.writing()
            || self.relocations.writing()
            || self.projects.writing()
            || self.session.dirty()
            || self.session.canvas.busy()
            || self.queued.is_some()
            || self
                .pending
                .as_ref()
                .is_some_and(|p| matches!(p.action, Action::Change(_)))
    }
    pub fn ready(&self) -> bool {
        self.path.is_some()
            && self.loaded
            && !self.busy()
            && self.error.is_none()
            && !self.relocations.failed()
    }
    pub fn reload(&mut self) {
        if !self.writing() {
            self.reload = self.path.is_some();
            self.projects.refresh();
        }
    }
    pub fn follow_rename(&mut self, source: &Path, destination: &Path) {
        if self.path.is_some() {
            self.session.commit_labels();
            self.relocations.push(source, destination);
        }
    }
    pub fn add(&mut self, pins: Vec<Pin>) {
        if self.ready() {
            self.queued = Some(Change::Add(pins));
        }
    }
    pub fn drop_point(&self, point: egui::Pos2) -> Option<[f64; 2]> {
        if !self.open || !self.ready() {
            return None;
        }
        self.session.canvas.world_in(self.drop_rect?, point)
    }
    pub fn import_artwork(
        &mut self,
        paths: Vec<PathBuf>,
        point: [f64; 2],
        token: Option<u64>,
    ) -> bool {
        if !self.open
            || !self.ready()
            || paths.is_empty()
            || paths.len() > 32
            || !point
                .iter()
                .all(|value| value.is_finite() && value.abs() <= 1e8)
            || paths.iter().any(|path| !artwork::supported(path))
        {
            return false;
        }
        self.queued = Some(Change::Import {
            pins: paths
                .into_iter()
                .map(|path| Pin::new(path, false))
                .collect(),
            point,
            token,
        });
        true
    }
    pub fn save_region(
        &mut self,
        region: regions::Region,
        pin: Option<Pin>,
        original: Option<regions::Region>,
    ) -> bool {
        if !self.ready() {
            return false;
        }
        self.queued = Some(Change::Region {
            region,
            pin,
            original: original.map(Box::new),
            expected: self.session.board.revision,
        });
        true
    }
    pub fn add_folder(&mut self, path: PathBuf, point: [f64; 2]) {
        if self.ready() {
            self.queued = Some(Change::Folder {
                pin: Pin::new(path, true),
                point,
            });
        }
    }
    pub fn remove(&mut self, id: Uuid) {
        if self.ready() {
            self.queued = Some(Change::Remove(id));
        }
    }
    pub fn editing_allowed(&self) -> bool {
        self.loaded
            && !self.relocations.failed()
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
        if !self.prepare_drop_close() {
            return false;
        }
        self.finish_ink();
        if self.ink.drawing() {
            self.open = true;
            return false;
        }
        if self.region_ui.draft.is_some() || self.region_ui.tracing {
            self.region_ui.leave_notice = true;
            self.open = true;
            return false;
        }
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
        self.cancel_drop();
        if self.pending.is_none() && self.queued.is_none() {
            self.session.discard(&self.state.board);
            self.error = None;
            self.reload();
        }
    }
    fn accept(&mut self, state: State, action: Action) {
        if let Action::Change(Change::Import {
            token: Some(token), ..
        }) = &action
        {
            self.native_results.push((*token, true));
        }
        if let Action::Change(Change::Region { region, .. }) = &action {
            self.region_ui.saved(region.id);
        }
        match action {
            Action::Change(Change::Board {
                generation: Some(generation),
                ..
            }) => self.session.acknowledge(&state.board, generation),
            Action::Change(Change::Relocate(change)) => {
                let replacements = self
                    .state
                    .pins
                    .iter()
                    .filter_map(|old| {
                        if state.pins.iter().any(|pin| pin.id == old.id) {
                            return None;
                        }
                        let mut target = old.path.clone();
                        for relocation in &change {
                            super::edits::relocate(
                                &mut target,
                                &relocation.source,
                                &relocation.destination,
                            );
                        }
                        state
                            .pins
                            .iter()
                            .find(|pin| pin.path == target)
                            .map(|pin| (old.id, pin.id))
                    })
                    .collect();
                self.region_ui.retarget(&state.pins, &replacements);
                self.session
                    .references_relocated(&state.board, &state.pins, &replacements);
            }
            Action::Change(
                Change::Add(_)
                | Change::Remove(_)
                | Change::Import { .. }
                | Change::Region { .. }
                | Change::Folder { .. },
            ) => self.session.references_changed(&state.board, &state.pins),
            _ => self.session.install(&state.board),
        }
        if state.version == 1 {
            let mut board = state.board.clone();
            for pin in &state.pins {
                board.add_shortcut(pin.id);
            }
            self.queued = Some(Change::Board {
                expected: state.board.revision,
                board: Box::new(board),
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
                board: Box::new(self.session.board.clone()),
                generation: Some(self.session.generation()),
            });
        }
    }
    fn complete_request(
        &mut self,
        action: Action,
        result: Result<Result<State, String>, JobError>,
    ) {
        match result {
            Err(JobError::QueueFull) => match action {
                Action::Load => self.reload = true,
                Action::Change(change) => self.queued = Some(change),
            },
            Ok(Ok(state)) => self.accept(state, action),
            result => {
                let error = match result {
                    Ok(Err(error)) => error,
                    Err(error) => error.to_string(),
                    _ => unreachable!(),
                };
                if let Action::Change(Change::Import {
                    token: Some(token), ..
                }) = &action
                {
                    self.native_results.push((*token, false));
                }
                if let Action::Change(Change::Relocate(changes)) = action {
                    self.relocations.reject(changes, error);
                    self.error = None;
                } else {
                    self.error = Some(error);
                }
            }
        }
    }
    pub fn tick(&mut self, jobs: &BackgroundJobs) {
        self.projects.tick(jobs);
        self.tick_drops();
        self.recipes.tick();
        if !self.open {
            self.projects.previews.retire();
            self.artwork.retire();
            self.ink.retire_reads();
            self.folders.retire();
            self.recipes.retire_reads();
        }
        if let Some(result) = self.pending.as_ref().and_then(|p| p.task.poll()) {
            let pending = self.pending.take().expect("polled Home task");
            self.complete_request(pending.action, result);
        }
        self.queue_draft();
        if self.pending.is_none()
            && let Some(path) = self.path.clone()
        {
            let action = self
                .queued
                .take()
                .map(Action::Change)
                .or_else(|| {
                    self.session
                        .settled_for_relocation()
                        .then(|| self.relocations.next())
                        .flatten()
                        .map(|change| Action::Change(Change::Relocate(change)))
                })
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
    local_state::update_when(path, ".home.lock", STATE_LIMIT, State::validate, |state| {
        let changed = match &change {
            Change::Relocate(changes) => state.pins.iter().any(|pin| {
                changes
                    .iter()
                    .any(|change| pin.path.starts_with(&change.source))
            }),
            _ => true,
        };
        if changed {
            state.apply(change)?;
        }
        Ok(changed)
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn live_folder_panes_share_a_source_but_keep_filters_and_layout_independent() {
        let mut state = super::State::default();
        let pin = super::Pin::new("/tmp/downloads".into(), true);
        for point in [[0., 0.], [800., 0.]] {
            state
                .apply(super::Change::Folder {
                    pin: pin.clone(),
                    point,
                })
                .unwrap();
        }
        assert_eq!(state.pins.len(), 1);
        assert_eq!(state.board.cards.len(), 2);
        if let super::Content::Folder { options, .. } = &mut state.board.cards[0].content {
            options.extensions = "wav".into();
            options.recent_minutes = 60;
            options.presentation = super::folders::Presentation::Icons;
        } else {
            panic!()
        }
        let super::Content::Folder { options, .. } = &state.board.cards[1].content else {
            panic!()
        };
        assert_eq!(*options, super::folders::Options::default());
        let restored: super::State =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(state.board, restored.board);
        state
            .relocate(
                std::path::Path::new("/tmp/downloads"),
                std::path::Path::new("/tmp/inbox"),
            )
            .unwrap();
        assert_eq!(state.pins[0].path, std::path::Path::new("/tmp/inbox"));
        assert!(
            state.board.cards.iter().all(
                |card| matches!(card.content,super::Content::Folder {pin: id,..} if id==pin.id)
            )
        );
        state.apply(super::Change::Remove(pin.id)).unwrap();
        assert!(state.board.cards.is_empty());
    }
    #[test]
    fn click_region_uses_stable_pin_through_rename_merge_unpin_and_layout_history() {
        let pin = super::Pin::new("/tmp/notes".into(), true);
        let mut state = super::State::default();
        state.apply(super::Change::Add(vec![pin.clone()])).unwrap();
        let region = super::regions::Region {
            id: uuid::Uuid::new_v4(),
            label: "Read notes".into(),
            points: vec![[0., 0.], [80., 0.], [40., 60.]],
            action: None,
            enabled: true,
        };
        let revision = state.board.revision;
        state
            .apply(super::Change::Region {
                region: region.clone(),
                pin: Some(super::Pin::new(pin.path.clone(), true)),
                expected: revision,
                original: None,
            })
            .unwrap();
        assert_eq!(state.pins.len(), 1);
        assert_eq!(state.board.regions[0].pin(), Some(pin.id));
        assert_eq!(
            state.board.cards.len(),
            1,
            "an action target must not add another shortcut card"
        );
        let mut session = super::Session::default();
        session.install(&state.board);
        session.edit(|board| board.regions[0].enabled = false);
        session.undo();
        assert!(session.board.regions[0].enabled);
        state
            .relocate(
                std::path::Path::new("/tmp/notes"),
                std::path::Path::new("/tmp/new-notes"),
            )
            .unwrap();
        assert_eq!(state.board.regions[0].pin(), Some(pin.id));
        assert_eq!(state.pins[0].path, std::path::Path::new("/tmp/new-notes"));
        let saved = serde_json::to_vec(&state).unwrap();
        let restored: super::State = serde_json::from_slice(&saved).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.board.regions, state.board.regions);
        assert!(
            state
                .apply(super::Change::Region {
                    region,
                    pin: Some(pin.clone()),
                    expected: revision,
                    original: None,
                })
                .is_err()
        );
        state.apply(super::Change::Remove(pin.id)).unwrap();
        assert_eq!(state.board.regions.len(), 1);
        assert!(!state.board.regions[0].enabled);
        assert!(state.board.regions[0].pin().is_none());
    }
    #[test]
    fn imported_artwork_keeps_source_references_and_independent_placement_through_moves() {
        let mut state = super::State::default();
        state
            .apply(super::Change::Import {
                pins: vec![
                    super::Pin::new("/art/icon.svg".into(), false),
                    super::Pin::new("/art/icon.svg".into(), false),
                ],
                point: [140.0, -80.0],
                token: None,
            })
            .unwrap();
        assert_eq!(state.pins.len(), 1);
        assert_eq!(state.board.cards.len(), 2);
        let id = state.pins[0].id;
        assert_eq!(state.board.cards[0].rect, [-20.0, -200.0, 320.0, 240.0]);
        assert_eq!(state.board.cards[1].rect, [4.0, -176.0, 320.0, 240.0]);
        state
            .relocate(
                std::path::Path::new("/art"),
                std::path::Path::new("/renamed"),
            )
            .unwrap();
        assert_eq!(
            state.pins[0].path,
            std::path::Path::new("/renamed/icon.svg")
        );
        assert!(
            state.board.cards.iter().all(
                |card| matches!(card.content, super::Content::Artwork { pin, .. } if pin == id)
            )
        );
        state.apply(super::Change::Remove(id)).unwrap();
        assert!(state.board.cards.is_empty());
        state.validate().unwrap();
    }
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
                board: Box::new(first),
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
                    board: Box::new(second),
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
    #[test]
    fn relocation_preserves_pending_layout_and_coalesces_pins_without_losing_cards_or_undo() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.txt");
        let destination = root.path().join("destination.txt");
        std::fs::write(&source, b"original").unwrap();
        let path = root.path().join("home.json");
        let source_pin = Pin::new(source.clone(), false);
        let source_id = source_pin.id;
        save(
            &path,
            Change::Add(vec![source_pin, Pin::new(destination.clone(), false)]),
        )
        .unwrap();
        let jobs = BackgroundJobs::new(1, 4);
        let mut home = Home::new(path.clone());
        await_state(&mut home, &jobs, |home| home.ready());
        home.session.arrange = true;
        home.session.title_draft = "Current work".into();
        std::fs::rename(&source, &destination).unwrap();
        home.follow_rename(&source, &destination);
        assert!(home.writing());
        await_state(&mut home, &jobs, |home| !home.busy() && !home.writing());
        assert_eq!(home.pins().len(), 1);
        assert_eq!(home.pins()[0].id, source_id);
        assert_eq!(home.pins()[0].path, destination);
        assert_eq!(home.session.board.title, "Current work");
        assert_eq!(home.session.board.cards.len(), 2);
        assert!(
            home.session
                .board
                .cards
                .iter()
                .all(|card| matches!(card.content,Content::Shortcut {pin,..} if pin==source_id))
        );
        home.session.undo();
        assert_eq!(home.session.board.title, "Home");
        assert!(
            home.session
                .board
                .cards
                .iter()
                .all(|card| matches!(card.content,Content::Shortcut {pin,..} if pin==source_id))
        );
        assert_eq!(std::fs::read(&home.pins()[0].path).unwrap(), b"original");
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
        assert!(!source.exists());
        let (state, _) = local_state::read::<State>(&path, STATE_LIMIT).unwrap();
        state.validate().unwrap();
        assert_eq!(state.pins[0].path, destination);
    }
    #[test]
    fn failed_reference_write_blocks_close_and_retries_only_metadata() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.txt");
        let destination = root.path().join("destination.txt");
        std::fs::write(&source, b"original").unwrap();
        let path = root.path().join("home.json");
        save(&path, Change::Add(vec![Pin::new(source.clone(), false)])).unwrap();
        let jobs = BackgroundJobs::new(1, 4);
        let mut home = Home::new(path);
        await_state(&mut home, &jobs, |home| home.ready());
        let directory =
            little_clip_core::kernel_interface::pin_content_directory(root.path()).unwrap();
        let lock = local_state::lock(&directory, ".home.lock").unwrap();
        std::fs::rename(&source, &destination).unwrap();
        home.follow_rename(&source, &destination);
        await_state(&mut home, &jobs, |home| home.relocations.failed());
        let mut browser = super::super::FileManager {
            home_surface: home,
            ..Default::default()
        };
        assert!(!browser.prepare_presentation_close());
        assert!(browser.presentation_writing());
        drop(lock);
        browser.home_surface.relocations.retry();
        await_state(&mut browser.home_surface, &jobs, |home| {
            !home.busy() && !home.writing()
        });
        assert!(browser.prepare_presentation_close());
        assert_eq!(browser.home_surface.pins()[0].path, destination);
        assert_eq!(std::fs::read(&destination).unwrap(), b"original");
        assert!(!source.exists());
    }
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
    fn dropped_folders_wait_for_intent_and_preview_placement_uses_drop_coordinates() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("Folder");
        std::fs::create_dir(&folder).unwrap();
        let jobs = BackgroundJobs::new(1, 4);
        let mut home = Home::new(root.path().join("home.json"));
        home.open = true;
        await_state(&mut home, &jobs, Home::ready);
        assert!(home.receive_drop(vec![folder.clone()], [900., 600.], 71, Default::default()));
        assert!(!home.prepare_close());
        await_state(&mut home, &jobs, |home| !home.drops.items.is_empty());
        assert!(home.session.board.cards.is_empty());
        assert!(!folder.join(".fmproject").exists());
        let super::drops::Item::Folder(path, manifest) = home.drops.items.pop().unwrap() else {
            panic!()
        };
        assert!(!manifest);
        home.add_folder(path, home.drops.point);
        assert!(!home.prepare_close());
        await_state(&mut home, &jobs, |home| {
            home.ready() && !home.session.board.cards.is_empty()
        });
        assert_eq!(home.session.board.cards[0].rect, [600., 380., 600., 440.]);
        home.tick(&jobs);
        assert!(home.native_results.contains(&(71, true)));
        assert!(!folder.join(".fmproject").exists());
    }
    #[test]
    fn unsaved_region_requires_explicit_discard_before_leaving() {
        let mut home = Home::default();
        home.region_ui.draft = Some(super::regions::Draft::from_region(
            super::regions::Region {
                id: uuid::Uuid::new_v4(),
                label: "Draft".into(),
                points: vec![[0., 0.], [100., 0.], [0., 100.]],
                enabled: true,
                action: None,
            },
            &[],
            None,
        ));
        assert!(!home.prepare_close());
        assert!(home.open && home.region_ui.leave_notice);
        home.region_ui.cancel();
        assert!(home.prepare_close());
    }
    #[test]
    fn region_edit_cannot_overwrite_changed_content_after_reloading_revision() {
        let mut state = super::State::default();
        let original = super::regions::Region {
            id: uuid::Uuid::new_v4(),
            label: "Original".into(),
            points: vec![[0., 0.], [100., 0.], [0., 100.]],
            enabled: true,
            action: None,
        };
        state.board.regions.push(original.clone());
        state.board.regions[0].label = "Changed elsewhere".into();
        let expected = state.board.revision;
        assert!(
            state
                .install_region(original.clone(), None, expected, Some(original))
                .is_err()
        );
        assert_eq!(state.board.regions[0].label, "Changed elsewhere");
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
                board: Box::new(other),
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
