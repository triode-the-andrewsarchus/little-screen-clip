//! Transient literal-name navigation over the loaded browser entries.
//!
//! Keep query editing separate from local match ranking so later search stages
//! can extend matching without changing browser focus or selection handling.
use egui::{Event, Key};

use super::FileManager;

const IDLE_SECONDS: f64 = 2.0;

#[derive(Default)]
pub(super) struct TypeNavigation {
    query: String,
    updated_at: f64,
    matched: bool,
}

impl TypeNavigation {
    pub(super) fn clear(&mut self) {
        self.query.clear();
        self.matched = false;
    }

    fn active(&self, now: f64) -> bool {
        !self.query.is_empty() && now < self.updated_at + IDLE_SECONDS
    }

    pub(super) fn hint(&self, now: f64) -> Option<String> {
        self.active(now).then(|| {
            format!(
                "Find: {}{}",
                self.query,
                if self.matched { "" } else { " — No match" }
            )
        })
    }

    fn consume(&mut self, events: &mut Vec<Event>, now: f64, is_similarity: bool) -> (bool, bool) {
        let mut changed = false;
        let mut finish = false;
        events.retain(|event| {
            match event {
                Event::Text(text) if !text.is_empty() && !text.chars().any(char::is_control) => {
                    // A bare Space retains its media-playback meaning.
                    if self.query.is_empty() && text.chars().all(char::is_whitespace) {
                        return true;
                    }
                    // In similarity view, -, +, and = override the find bar.
                    if is_similarity
                        && self.query.is_empty()
                        && (text == "-" || text == "+" || text == "=")
                    {
                        return true;
                    }
                    self.query.push_str(text);
                }
                Event::Key {
                    key: Key::Backspace,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none() && !self.query.is_empty() => {
                    self.query.pop();
                }
                Event::Key {
                    key: Key::Escape,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none() && !self.query.is_empty() => {
                    self.clear();
                }
                Event::Key {
                    key:
                        Key::ArrowUp
                        | Key::ArrowDown
                        | Key::ArrowLeft
                        | Key::ArrowRight
                        | Key::Enter
                        | Key::Tab
                        | Key::Delete
                        | Key::F2,
                    pressed: true,
                    ..
                } => {
                    // Apply preceding text before Enter/arrows use the selection.
                    finish = true;
                    return true;
                }
                _ => return true,
            }
            self.updated_at = now;
            changed = true;
            false
        });
        (changed, finish)
    }
}

/// Literal matching only: exact, prefix, then substring. Ties keep display order.
fn name_rank(name: &str, query: &str) -> Option<u8> {
    let name = name.to_lowercase();
    if name == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if name.contains(query) {
        Some(2)
    } else {
        None
    }
}

impl FileManager {
    pub(super) fn type_navigation_keys(&mut self, ctx: &egui::Context) {
        let (now, modifiers, pointer_pressed) =
            ctx.input(|input| (input.time, input.modifiers, input.pointer.any_pressed()));
        if !self.type_navigation.active(now) || pointer_pressed {
            self.type_navigation.clear();
        }
        let browser_focused = !super::ui::popup_open(ctx)
            && ctx.memory(|memory| {
                memory.focused().is_none() || memory.has_focus(super::ui::list_id())
            });
        // Shift changes literal text; other modifiers belong to shortcuts.
        let shortcut_modifiers = egui::Modifiers {
            shift: false,
            ..modifiers
        };
        if !browser_focused
            || self.focus_address
            || self.pending.is_some()
            || !shortcut_modifiers.is_none()
        {
            self.type_navigation.clear();
            return;
        }
        let is_similarity = self.view == super::BrowserView::Photos
            && self.photo_mode == super::gallery::similarity::PhotoMode::Similarity;
        let (changed, finish) = ctx.input_mut(|input| {
            self.type_navigation
                .consume(&mut input.events, now, is_similarity)
        });
        if changed {
            self.match_typed_name();
            if !self.type_navigation.query.is_empty() {
                ctx.memory_mut(|memory| memory.request_focus(super::ui::list_id()));
            }
            ctx.request_repaint();
        }
        if finish {
            self.type_navigation.clear();
        }
        if self.type_navigation.active(now) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                (self.type_navigation.updated_at + IDLE_SECONDS - now).max(0.0),
            ));
        }
    }

    fn match_typed_name(&mut self) {
        let query = self.type_navigation.query.to_lowercase();
        if query.is_empty() {
            return;
        }
        let target = self
            .displayed_entries()
            .into_iter()
            .filter_map(|entry| name_rank(&entry.name, &query).map(|rank| (rank, entry)))
            .min_by_key(|(rank, _)| *rank)
            .map(|(_, entry)| entry.path.clone());
        self.type_navigation.matched = target.is_some();
        if let Some(path) = target {
            if self
                .location()
                .and_then(|location| location.selected.as_ref())
                != Some(&path)
                || self.selection_count() != 1
            {
                self.select(path);
            }
            self.reveal_selection = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_names_rank_exact_then_prefix_then_substring() {
        assert_eq!(name_rank("Report", "report"), Some(0));
        assert_eq!(name_rank("REPORT.txt", "report"), Some(1));
        assert_eq!(name_rank("old-report.txt", "report"), Some(2));
        assert_eq!(name_rank("r_e_p_o_r_t", "report"), None);
        assert_eq!(name_rank("Résumé.txt", "résumé"), Some(1));
    }

    #[test]
    fn hint_expires_and_backspace_removes_a_unicode_character() {
        let mut state = TypeNavigation::default();
        let mut text = vec![Event::Text("café".into())];
        assert_eq!(state.consume(&mut text, 10.0, false), (true, false));
        assert!(text.is_empty());
        assert!(state.hint(11.9).is_some());
        assert!(state.hint(12.0).is_none());
        let mut backspace = vec![Event::Key {
            key: Key::Backspace,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }];
        state.consume(&mut backspace, 11.0, false);
        assert_eq!(state.query, "caf");
        assert!(backspace.is_empty());
    }

    #[test]
    fn similarity_view_overrides_plus_minus_equals_when_query_empty() {
        let mut state = TypeNavigation::default();
        let mut text = vec![
            Event::Text("-".into()),
            Event::Text("+".into()),
            Event::Text("=".into()),
        ];
        assert_eq!(state.consume(&mut text, 10.0, true), (false, false));
        assert_eq!(
            text.len(),
            3,
            "Events must not be consumed in similarity view"
        );
        assert!(state.query.is_empty());
    }
}
