use eframe::egui::{self, Key};

/// High-level shortcut actions detected by the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommonShortcut {
    Save,
    Copy,
    Undo,
    Redo,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    ZoomActual,
    Close,
}

pub struct ShortcutDispatcher;

impl ShortcutDispatcher {
    /// Handles the hierarchical Escape cancellation stack:
    /// 1. If `can_cancel_task` is true, calls `cancel_task()`.
    /// 2. Else if `can_reset_tool` is true, calls `reset_tool()`.
    /// 3. Else, sets `*close = true`.
    pub fn handle_escape(
        ui: &egui::Ui,
        can_cancel_task: bool,
        cancel_task: impl FnOnce(),
        can_reset_tool: bool,
        reset_tool: impl FnOnce(),
        close: &mut bool,
    ) -> bool {
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            if can_cancel_task {
                cancel_task();
            } else if can_reset_tool {
                reset_tool();
            } else {
                *close = true;
            }
            true
        } else {
            false
        }
    }

    /// Evaluates common global studio shortcuts when keyboard focus is not inside a text field.
    pub fn check_common(ctx: &egui::Context) -> Option<CommonShortcut> {
        if ctx.wants_keyboard_input() {
            return None;
        }

        let cmd = ctx.input(|i| i.modifiers.command || i.modifiers.ctrl || i.modifiers.mac_cmd);
        let shift = ctx.input(|i| i.modifiers.shift);

        if cmd {
            if ctx.input(|i| i.key_pressed(Key::S)) {
                return Some(CommonShortcut::Save);
            }
            if ctx.input(|i| i.key_pressed(Key::C)) {
                return Some(CommonShortcut::Copy);
            }
            if ctx.input(|i| i.key_pressed(Key::Z)) {
                if shift {
                    return Some(CommonShortcut::Redo);
                } else {
                    return Some(CommonShortcut::Undo);
                }
            }
            if ctx.input(|i| i.key_pressed(Key::Y)) {
                return Some(CommonShortcut::Redo);
            }
            if ctx.input(|i| i.key_pressed(Key::Num0)) {
                return Some(CommonShortcut::ZoomFit);
            }
        } else {
            if ctx.input(|i| i.key_pressed(Key::Num0)) {
                return Some(CommonShortcut::ZoomFit);
            }
            if ctx.input(|i| i.key_pressed(Key::Num1)) {
                return Some(CommonShortcut::ZoomActual);
            }
            if ctx.input(|i| i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals)) {
                return Some(CommonShortcut::ZoomIn);
            }
            if ctx.input(|i| i.key_pressed(Key::Minus)) {
                return Some(CommonShortcut::ZoomOut);
            }
        }

        None
    }
}
