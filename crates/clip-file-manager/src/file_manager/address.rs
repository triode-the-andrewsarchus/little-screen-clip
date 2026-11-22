use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use egui::{Event, EventFilter, Key, Modifiers, TextEdit, Ui, text_edit::TextEditOutput, vec2};

use super::ui::{address_id, focus_text, list_id};
use super::{
    BackgroundJobs, FileManager, JobError, display_path, parse_address, preview_job::Task,
};

const LISTING_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

#[derive(Default)]
pub(super) struct Completion {
    folder: Option<PathBuf>,
    pending: Option<Task<Vec<String>>>,
    names: Vec<String>,
    selected: usize,
    suffix_space: i8,
}

impl Completion {
    fn update(&mut self, ctx: &egui::Context, folder: PathBuf) {
        if self.folder.as_ref() != Some(&folder) {
            self.names.clear();
            self.selected = 0;
            self.folder = Some(folder.clone());
            self.pending = Some(
                Task::start(BackgroundJobs::global(), LISTING_TIMEOUT, move |cancel| {
                    directory_names(&folder, cancel)
                })
                .with_response_deadline(LISTING_TIMEOUT),
            );
        }
        match self.pending.as_ref().and_then(Task::poll) {
            Some(result) => {
                self.pending = None;
                match result {
                    Ok(names) => self.names = names,
                    Err(JobError::QueueFull) => {
                        self.folder = None;
                        ctx.request_repaint_after(POLL_INTERVAL);
                    }
                    Err(_) => {}
                }
            }
            None if self.pending.is_some() => ctx.request_repaint_after(POLL_INTERVAL),
            None => {}
        }
    }

    fn matches(&self, prefix: &str, show_hidden: bool) -> &[String] {
        let first_hidden = self.names.partition_point(|name| !name.starts_with('.'));
        let names = match prefix.as_bytes().first() {
            None if show_hidden => return &self.names,
            Some(b'.') => &self.names[first_hidden..],
            _ => &self.names[..first_hidden],
        };
        let first = names.partition_point(|name| name.as_str() < prefix);
        let count = names[first..].partition_point(|name| name.starts_with(prefix));
        &names[first..first + count]
    }

    fn keys(&mut self, ctx: &egui::Context, prefix: &str, show_hidden: bool) {
        let names = self.matches(prefix, show_hidden);
        if names.is_empty() {
            return;
        }
        let mut selected = self.selected.min(names.len() - 1);
        let mut previous_focus = false;
        ctx.input_mut(|input| {
            let mut can_complete = true;
            input.events.retain_mut(|event| {
                if !can_complete {
                    return true;
                }
                match address_key(event) {
                    Some(Key::ArrowUp) => selected = (selected + names.len() - 1) % names.len(),
                    Some(Key::ArrowDown) => selected = (selected + 1) % names.len(),
                    Some(Key::Tab | Key::Slash | Key::ArrowRight) => {
                        let suffix = &names[selected][prefix.len()..];
                        *event = Event::Text(format!("{suffix}/"));
                        can_complete = false;
                        return true;
                    }
                    _ => {
                        previous_focus = matches!(
                            event,
                            Event::Key {
                                key: Key::Tab,
                                pressed: true,
                                modifiers: Modifiers::SHIFT,
                                ..
                            }
                        );
                        // A coalesced edit or cursor move may change the target of
                        // a later completion. Let TextEdit handle those events.
                        can_complete = !matches!(
                            event,
                            Event::Key { pressed: true, key, .. } if *key != Key::Slash
                        ) && !matches!(
                            event,
                            Event::Text(_)
                                | Event::Paste(_)
                                | Event::Cut
                                | Event::Ime(_)
                                | Event::PointerButton { pressed: true, .. }
                                | Event::MiddleClickPaste { .. }
                        );
                        return true;
                    }
                }
                false
            });
        });
        self.selected = selected;
        if previous_focus {
            ctx.memory_mut(|memory| memory.move_focus(egui::FocusDirection::Previous));
        }
    }
}

// Native slash input comes as a text event, including on layouts that use Shift.
fn address_key(event: &Event) -> Option<Key> {
    match event {
        Event::Text(text) if text == "/" => Some(Key::Slash),
        Event::Key {
            key: key @ (Key::ArrowUp | Key::ArrowDown | Key::ArrowRight | Key::Tab),
            pressed: true,
            modifiers: Modifiers::NONE,
            ..
        } => Some(*key),
        _ => None,
    }
}

fn directory_names(folder: &Path, cancel: &AtomicBool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut names: Vec<_> = entries
        .take_while(|_| !cancel.load(Ordering::Relaxed))
        .flatten()
        .filter(|entry| {
            entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() || (kind.is_symlink() && entry.path().is_dir()))
        })
        .map(|entry| display_path(Path::new(&entry.file_name())))
        .collect();
    names.sort_unstable_by(|a, b| {
        a.starts_with('.')
            .cmp(&b.starts_with('.'))
            .then_with(|| a.cmp(b))
    });
    names
}

impl FileManager {
    pub(super) fn address_ui(&mut self, ui: &mut Ui, width: f32) {
        self.complete_address_on_input(ui);
        let id = address_id();
        let suffix_space = self.address_completion.suffix_space;
        let mut edit = TextEdit::singleline(&mut self.address)
            .id(id)
            .hint_text("Directory path [Ctrl+L]")
            .desired_width(width);
        if suffix_space > 0 {
            let mut margin = egui::Margin::from(ui.spacing().button_padding);
            margin.right = margin.right.saturating_add(suffix_space);
            edit = edit.margin(margin);
        }
        let mut output = edit.show(ui);
        output.response.response.clone().on_hover_text(
            r"Enter a directory path. Relative paths and ~/ are supported. Use ↑/↓ to choose a folder and Tab, /, or → to complete it. Use \\ for a literal backslash and \xHH for an escaped byte.",
        );
        if std::mem::take(&mut self.focus_address) {
            focus_text(ui.ctx(), &mut output, self.address.chars().count());
        }
        if output.response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter)) {
            self.submit_address();
            ui.memory_mut(|memory| memory.request_focus(list_id()));
        } else if output.response.lost_focus() {
            self.address = self.location_label();
        }
        self.address_editing = ui.memory(|memory| memory.has_focus(id));
        self.suggest_address(ui, &output);
    }

    fn complete_address_on_input(&mut self, ui: &Ui) {
        let id = address_id();
        if ui.is_enabled()
            && !self.focus_address
            && ui.memory(|memory| memory.has_focus(id))
            && TextEdit::load_state(ui.ctx(), id).is_some_and(|state| at_end(&state, &self.address))
            && let Some((folder, start)) = self.address_parent()
            && self.address_completion.folder.as_ref() == Some(&folder)
        {
            self.address_completion
                .keys(ui.ctx(), &self.address[start..], self.show_hidden);
        }
    }

    fn suggest_address(&mut self, ui: &Ui, output: &TextEditOutput) {
        let Some((folder, start)) = self
            .address_parent()
            .filter(|_| self.address_editing && ui.is_enabled())
        else {
            self.address_completion = Completion::default();
            return;
        };
        let completion = &mut self.address_completion;
        completion.update(ui.ctx(), folder);
        if output.response.changed() {
            completion.selected = 0;
        }
        let prefix = &self.address[start..];
        let names = completion.matches(prefix, self.show_hidden);
        let name = names.get(completion.selected.min(names.len().saturating_sub(1)));
        let suffix_space = if let Some(name) = name
            && at_end(&output.state, &self.address)
        {
            let suffix = format!("{}/", &name[prefix.len()..]);
            // Focus traversal reads this filter at the start of the next frame.
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    output.response.id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                );
            });
            paint_suffix(ui, output, suffix)
        } else {
            0
        };
        if completion.suffix_space != suffix_space {
            completion.suffix_space = suffix_space;
            ui.ctx().request_repaint();
        }
    }

    fn address_parent(&self) -> Option<(PathBuf, usize)> {
        let start = self.address.rfind('/').map_or(0, |slash| slash + 1);
        let (parent, prefix) = self.address.split_at(start);
        // Leave the literal shortcuts available for typing /, ./, ../ or ~/.
        if self.address.is_empty() || self.address == "~" || matches!(prefix, "." | "..") {
            return None;
        }
        let folder = parse_address(
            if parent.is_empty() { "." } else { parent },
            self.path(),
            &self.home,
        )
        .ok()?;
        Some((self.archives.resolve(&folder), start))
    }
}

fn at_end(state: &egui::text_edit::TextEditState, draft: &str) -> bool {
    state
        .cursor
        .char_range()
        .is_some_and(|range| range.is_empty() && range.primary.index.0 == draft.chars().count())
}

fn paint_suffix(ui: &Ui, output: &TextEditOutput, suffix: String) -> i8 {
    let Some(section) = output.galley.job.sections.first() else {
        return 0;
    };
    let end = output.galley.pos_from_cursor(output.galley.end());
    // TextEdit's inner clip can shrink to the typed text even in a wide field.
    // Extend it to the field's right padding independently of the scroll margin.
    let mut clip = output.text_clip_rect;
    let padding = egui::Margin::from(ui.spacing().button_padding);
    clip.max.x = output.response.rect.right() - f32::from(padding.right);
    let painter = ui.painter_at(clip);
    let galley = painter.layout_no_wrap(
        suffix,
        section.format.font_id.clone(),
        ui.visuals().weak_text_color(),
    );
    let caret_gap = ui.visuals().text_cursor.stroke.width / 2.0 + 1.0 / ui.ctx().pixels_per_point();
    let gap = caret_gap - galley.mesh_bounds.left().min(0.0);
    let position = output.galley_pos + vec2(end.left() - output.galley.rect.left() + gap, 0.0);
    let width = gap + galley.size().x.max(galley.mesh_bounds.right());
    painter.galley(position, galley, ui.visuals().weak_text_color());
    // Reserve room for the hint when TextEdit would scroll the caret flush
    // with the right edge. Keep most of the field available to the typed path.
    if output.galley.size().x + width > clip.width() {
        width.ceil().min(clip.width() / 3.0) as i8
    } else {
        0
    }
}
