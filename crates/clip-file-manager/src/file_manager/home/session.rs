//! Home draft/history stays independent from accepted disk-write snapshots.
use super::Pin;
use super::board::{Binding, Board, Card, Content};
use clip_studios::drawing_canvas::Canvas;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};
use uuid::Uuid;

const HISTORY: usize = 24;
#[derive(Default)]
pub(in crate::file_manager) struct Session {
    pub board: Board,
    pub canvas: Canvas,
    pub arrange: bool,
    pub leave_notice: bool,
    pub selected: Option<Uuid>,
    pub label_draft: String,
    pub title_draft: String,
    pub spawn: [f64; 2],
    generation: u64,
    saved: u64,
    loaded: bool,
    undo: Vec<Board>,
    redo: Vec<Board>,
    drag_before: Option<Board>,
    last_edit: Option<Instant>,
}
impl Session {
    pub fn dirty(&self) -> bool {
        self.generation != self.saved
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn due(&self) -> bool {
        self.dirty()
            && self.drag_before.is_none()
            && self
                .last_edit
                .is_some_and(|time| time.elapsed() >= Duration::from_millis(300))
    }
    pub fn install(&mut self, board: &Board) {
        if self.dirty() {
            return;
        }
        if !self.loaded || self.board != *board {
            self.board = board.clone();
            self.undo.clear();
            self.redo.clear();
            self.selected = None;
            self.title_draft = board.title.clone();
            if let Some(view) = board.view {
                self.canvas.restore_view(view);
            } else {
                self.canvas.fit();
            }
        }
        self.loaded = true;
    }
    pub fn references_changed(&mut self, board: &Board, pins: &[Pin]) {
        let valid: BTreeSet<_> = pins.iter().map(|pin| pin.id).collect();
        let prune = |snapshot: &mut Board| snapshot.retain_pin_refs(&valid, &Default::default());
        for snapshot in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            prune(snapshot);
            snapshot.revision = board.revision;
        }
        let mut previous = self.board.clone();
        prune(&mut previous);
        previous.revision = board.revision;
        let added = board
            .cards
            .iter()
            .any(|card| !previous.cards.iter().any(|old| old.id == card.id));
        if previous != *board {
            self.remember(previous);
        } else {
            self.redo.clear();
        }
        self.board = board.clone();
        self.undo.dedup();
        while self.undo.last() == Some(&self.board) {
            self.undo.pop();
        }
        self.title_draft = board.title.clone();
        if !board
            .cards
            .iter()
            .any(|card| Some(card.id) == self.selected)
        {
            self.selected = None;
        }
        if added {
            self.canvas.fit();
        }
    }
    pub fn settled_for_relocation(&self) -> bool {
        !self.dirty()
            && !self.canvas.busy()
            && !self.canvas.has_unapplied_text()
            && self.drag_before.is_none()
    }
    pub fn references_relocated(
        &mut self,
        board: &Board,
        pins: &[Pin],
        replacements: &std::collections::BTreeMap<Uuid, Uuid>,
    ) {
        let valid = pins.iter().map(|pin| pin.id).collect();
        for snapshot in std::iter::once(&mut self.board)
            .chain(self.undo.iter_mut())
            .chain(self.redo.iter_mut())
            .chain(self.drag_before.iter_mut())
        {
            snapshot.retain_pin_refs(&valid, replacements);
            snapshot.revision = board.revision;
        }
        self.install(board);
    }
    fn remember(&mut self, previous: Board) {
        self.undo.push(previous);
        if self.undo.len() > HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }
    pub fn acknowledge(&mut self, board: &Board, generation: u64) {
        self.saved = generation;
        if self.generation == generation {
            self.board = board.clone();
        } else {
            self.board.revision = board.revision;
        }
    }
    pub fn discard(&mut self, board: &Board) {
        self.saved = self.generation;
        self.drag_before = None;
        self.canvas = Canvas::default();
        self.loaded = false;
        self.install(board);
    }
    pub fn edit(&mut self, change: impl FnOnce(&mut Board)) {
        let previous = self.board.clone();
        change(&mut self.board);
        if previous != self.board {
            self.remember(previous);
            self.touch();
        }
    }
    fn touch(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.last_edit = Some(Instant::now());
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(source) = self.canvas.poll(ctx, &self.board.drawing) {
            self.edit(|board| board.drawing = source);
        }
        if !self.canvas.selected().is_empty() {
            self.selected = None;
        }
    }
    pub fn retain_view(&mut self) {
        if let Some(view) = self.canvas.view()
            && self.board.view != Some(view)
        {
            self.board.view = Some(view);
            self.touch();
        }
    }
    pub fn commit_labels(&mut self) {
        if !self.arrange {
            return;
        }
        let title = self.title_draft.trim().to_owned();
        if !title.is_empty() && title.len() <= 256 {
            self.edit(|board| board.title = title);
        }
        self.rename_selected();
    }
    pub fn allow_leave(&mut self) -> bool {
        if self.canvas.has_unapplied_text() {
            self.leave_notice = true;
            self.arrange = true;
            return false;
        }
        self.leave_notice = false;
        true
    }
    pub fn select(&mut self, id: Uuid) {
        if self.selected != Some(id) {
            self.selected = Some(id);
            self.label_draft = self
                .board
                .cards
                .iter()
                .find(|card| card.id == id)
                .map(|card| match &card.content {
                    Content::Shortcut { label, .. }
                    | Content::Artwork { label, .. }
                    | Content::Folder { label, .. } => label.clone(),
                    Content::Section { title, .. }
                    | Content::Projects { title }
                    | Content::Project { title, .. } => title.clone(),
                })
                .unwrap_or_default();
        }
    }
    pub fn section(&mut self) {
        let id = Uuid::new_v4();
        let [x, y] = self.spawn;
        self.edit(|board| {
            board.cards.insert(
                0,
                Card {
                    id,
                    rect: [x - 260.0, y - 40.0, 520.0, 360.0],
                    content: Content::Section {
                        title: "A new corner".into(),
                        color: [69, 132, 134],
                    },
                },
            )
        });
        self.select(id);
    }
    pub fn rename_selected(&mut self) {
        let title = self.label_draft.trim().to_owned();
        if title.len() > 256 {
            return;
        }
        let selected = self.selected;
        self.edit(|board| {
            if let Some(card) = board
                .cards
                .iter_mut()
                .find(|card| Some(card.id) == selected)
            {
                match &mut card.content {
                    Content::Shortcut { label, .. }
                    | Content::Artwork { label, .. }
                    | Content::Folder { label, .. } => *label = title,
                    Content::Section { title: label, .. }
                    | Content::Projects { title: label }
                    | Content::Project { title: label, .. }
                        if !title.is_empty() =>
                    {
                        *label = title
                    }
                    _ => {}
                }
            }
        });
    }
    pub fn remove_selected(&mut self) {
        if let Some(id) = self.selected.take() {
            self.edit(|board| board.cards.retain(|card| card.id != id));
        }
    }
    pub fn link(&mut self, pin: Uuid) {
        let selected = self.canvas.selected().to_vec();
        if selected.is_empty() {
            return;
        }
        self.edit(|board| {
            board
                .bindings
                .retain(|binding| !selected.contains(&binding.element));
            board
                .bindings
                .extend(selected.into_iter().map(|element| Binding { element, pin }));
        });
    }
    pub fn begin_drag(&mut self, id: Uuid) {
        self.select(id);
        if self.drag_before.is_none() {
            self.drag_before = Some(self.board.clone());
        }
    }
    pub fn drag(&mut self, id: Uuid, delta: [f64; 2], resize: bool) {
        if let Some(card) = self.board.cards.iter_mut().find(|card| card.id == id) {
            if resize {
                if matches!(card.content, Content::Artwork { .. }) {
                    let [width, height] = [card.rect[2], card.rect[3]];
                    let [x, y] = [delta[0] / width, delta[1] / height];
                    let factor = (1.0 + if x.abs() > y.abs() { x } else { y }).clamp(
                        (8.0 / width).max(8.0 / height),
                        (8000.0 / width).min(8000.0 / height),
                    );
                    card.rect[2] = width * factor;
                    card.rect[3] = height * factor;
                } else {
                    card.rect[2] = (card.rect[2] + delta[0]).clamp(80.0, 8000.0);
                    card.rect[3] = (card.rect[3] + delta[1]).clamp(50.0, 8000.0);
                }
            } else {
                card.rect[0] = (card.rect[0] + delta[0]).clamp(-1e8, 1e8);
                card.rect[1] = (card.rect[1] + delta[1]).clamp(-1e8, 1e8);
            }
            self.touch();
        }
    }
    pub fn end_drag(&mut self) {
        if let Some(previous) = self.drag_before.take()
            && previous != self.board
        {
            self.remember(previous);
        }
    }
    pub fn cancel_drag(&mut self) -> bool {
        let Some(previous) = self.drag_before.take() else {
            return false;
        };
        self.board = previous;
        self.touch();
        true
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo(&mut self) {
        if let Some(mut previous) = self.undo.pop() {
            previous.revision = self.board.revision;
            previous.view = self.board.view;
            self.redo.push(std::mem::replace(&mut self.board, previous));
            self.selected = None;
            self.title_draft = self.board.title.clone();
            self.touch();
        }
    }
    pub fn redo(&mut self) {
        if let Some(mut next) = self.redo.pop() {
            next.revision = self.board.revision;
            next.view = self.board.view;
            self.undo.push(std::mem::replace(&mut self.board, next));
            self.selected = None;
            self.title_draft = self.board.title.clone();
            self.touch();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn artwork_resize_scales_both_axes_and_undo_keeps_its_reference() {
        let mut session = super::Session::default();
        let id = uuid::Uuid::new_v4();
        let pin = uuid::Uuid::new_v4();
        session.board.cards.push(super::Card {
            id,
            rect: [10.0, 20.0, 320.0, 240.0],
            content: super::Content::Artwork {
                pin,
                label: String::new(),
            },
        });
        session.begin_drag(id);
        session.drag(id, [40.0, 0.0], true);
        session.end_drag();
        assert_eq!(session.board.cards[0].rect, [10.0, 20.0, 360.0, 270.0]);
        session.undo();
        assert_eq!(session.board.cards[0].rect, [10.0, 20.0, 320.0, 240.0]);
        assert!(
            matches!(session.board.cards[0].content, super::Content::Artwork {pin: actual,..} if actual == pin)
        );
    }
    #[test]
    fn closing_commits_unfocused_title_and_card_label_drafts() {
        let mut session = super::Session::default();
        let pin = uuid::Uuid::new_v4();
        let mut board = super::Board::default();
        board.add_shortcut(pin);
        session.install(&board);
        session.arrange = true;
        session.select(board.cards[0].id);
        session.title_draft = " My desk ".into();
        session.label_draft = " Current work ".into();
        session.commit_labels();
        assert_eq!(session.board.title, "My desk");
        assert!(
            matches!(&session.board.cards[0].content, super::Content::Shortcut { label, .. } if label == "Current work")
        );
        assert!(session.dirty());
        assert!(session.allow_leave());
    }
    #[test]
    fn persisted_camera_restores_and_layout_undo_keeps_the_new_view() {
        use clip_studios::drawing_canvas::ViewState;
        let view = ViewState {
            center: [-42.0, 809.0],
            scale: 1.5,
        };
        let board = super::Board {
            view: Some(view),
            ..Default::default()
        };
        let mut session = super::Session::default();
        session.install(&board);
        assert_eq!(session.canvas.view(), Some(view));
        session.edit(|board| board.title = "My desk".into());
        let changed = ViewState {
            center: [1000.0, -280.0],
            scale: 0.75,
        };
        assert!(session.canvas.restore_view(changed));
        session.retain_view();
        assert!(session.dirty());
        session.undo();
        assert_eq!(session.board.title, "Home");
        assert_eq!(session.board.view, Some(changed));
        assert_eq!(session.canvas.view(), Some(changed));
        session.redo();
        assert_eq!(session.board.title, "My desk");
        assert_eq!(session.board.view, Some(changed));
        let bytes = serde_json::to_vec(&session.board).unwrap();
        let saved = serde_json::from_slice(&bytes).unwrap();
        let mut reopened = super::Session::default();
        reopened.install(&saved);
        assert_eq!(reopened.canvas.view(), Some(changed));
    }
    use super::*;
    #[test]
    fn late_save_acknowledgement_keeps_newer_edits_and_advances_their_base_revision() {
        let mut session = Session::default();
        session.install(&Board::default());
        session.edit(|board| board.title = "First edit".into());
        let generation = session.generation();
        let mut accepted = session.board.clone();
        accepted.revision = 1;
        session.edit(|board| board.title = "Newer draft".into());
        session.acknowledge(&accepted, generation);
        assert_eq!(session.board.title, "Newer draft");
        assert_eq!(session.board.revision, 1);
        assert!(session.dirty());
        session.acknowledge(&session.board.clone(), session.generation());
        assert!(!session.dirty());
    }
    #[test]
    fn dragging_is_one_undo_step_and_never_saves_a_half_finished_gesture() {
        let mut board = Board::default();
        board.add_shortcut(Uuid::new_v4());
        let id = board.cards[0].id;
        let mut session = Session::default();
        session.install(&board);
        session.begin_drag(id);
        for _ in 0..8 {
            session.drag(id, [12.0, -3.0], false);
        }
        assert!(!session.due());
        session.end_drag();
        assert_eq!(session.undo.len(), 1);
        assert_eq!(
            session.board.cards[0].rect[0],
            board.cards[0].rect[0] + 96.0
        );
        session.board.revision = 9;
        session.undo();
        assert_eq!(session.board.cards[0].rect, board.cards[0].rect);
        assert_eq!(session.board.revision, 9);
        session.redo();
        assert_eq!(
            session.board.cards[0].rect[0],
            board.cards[0].rect[0] + 96.0
        );
        assert_eq!(session.board.revision, 9);
    }
    #[test]
    fn resizing_is_bounded_and_cancelling_a_drag_restores_the_whole_card() {
        let mut board = Board::default();
        board.add_shortcut(Uuid::new_v4());
        let id = board.cards[0].id;
        let mut session = Session::default();
        session.install(&board);
        session.begin_drag(id);
        session.drag(id, [1e6, -1e6], true);
        assert_eq!(&session.board.cards[0].rect[2..], &[8000.0, 50.0]);
        assert!(session.cancel_drag());
        assert_eq!(session.board.cards, board.cards);
        assert!(!session.can_undo());
    }
    #[test]
    fn discard_restores_the_accepted_board_and_reloading_cannot_erase_a_dirty_draft() {
        let mut session = Session::default();
        session.install(&Board::default());
        session.edit(|board| board.title = "Unsaved".into());
        let incoming = Board {
            title: "Other window".into(),
            revision: 3,
            ..Board::default()
        };
        session.install(&incoming);
        assert_eq!(session.board.title, "Unsaved");
        session.discard(&incoming);
        assert_eq!(session.board.title, "Other window");
        assert!(!session.dirty());
        assert!(!session.can_undo());
    }
}

#[cfg(test)]
mod reference_history_tests {
    use super::*;
    #[test]
    fn adding_shortcuts_preserves_history_and_unpinning_cannot_restore_orphaned_targets() {
        let pin = Pin::new("/reference".into(), false);
        let mut session = Session::default();
        session.install(&Board::default());
        session.edit(|board| board.title = "My title".into());
        session.acknowledge(&session.board.clone(), session.generation());
        let mut incoming = session.board.clone();
        incoming.add_shortcut(pin.id);
        incoming.revision = 1;
        session.references_changed(&incoming, std::slice::from_ref(&pin));
        session.undo();
        assert!(session.board.cards.is_empty());
        assert_eq!(session.board.title, "My title");
        session.undo();
        assert_eq!(session.board.title, "Home");
        session.redo();
        session.redo();
        assert_eq!(session.board.cards.len(), 1);
        let mut removed = session.board.clone();
        removed.remove_pin(pin.id);
        removed.revision = 2;
        session.references_changed(&removed, &[]);
        while session.can_undo() {
            session.undo();
            session.board.validate(&[]).unwrap();
            assert!(session.board.cards.is_empty());
        }
    }
}
