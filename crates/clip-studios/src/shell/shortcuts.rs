use egui::{self, Key};

/// Undo and redo actions detected by the shared studio shortcut dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommonShortcut {
    Undo,
    Redo,
}

pub struct ShortcutDispatcher;

impl ShortcutDispatcher {
    /// Detects undo and redo when keyboard focus is outside text fields.
    pub fn check_common(ctx: &egui::Context) -> Option<CommonShortcut> {
        if ctx.wants_keyboard_input() {
            return None;
        }

        ctx.input(|input| {
            let modifiers = input.modifiers;
            if !(modifiers.command || modifiers.ctrl || modifiers.mac_cmd) {
                return None;
            }

            if input.key_pressed(Key::Z) {
                return Some(if modifiers.shift {
                    CommonShortcut::Redo
                } else {
                    CommonShortcut::Undo
                });
            }
            input.key_pressed(Key::Y).then_some(CommonShortcut::Redo)
        })
    }
}
