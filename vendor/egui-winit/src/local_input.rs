/// Context data key for applications that handle non-text clipboard payloads.
/// Set its temporary boolean value only while a custom clipboard surface has focus.
/// When true, clipboard shortcuts arrive as ordinary key events, without a text
/// clipboard read. Text editors should leave this false.
pub const CLIPBOARD_SHORTCUTS_AS_KEYS: &str = "egui_winit_clipboard_shortcuts_as_keys";

/// Optional `Vec<egui::KeyboardShortcut>` context data for shortcuts that move
/// focus from a custom clipboard surface into a text editor. Matching a press
/// restores text clipboard routing for subsequent events in the same batch.
pub const CLIPBOARD_TEXT_FOCUS_SHORTCUTS: &str = "egui_winit_clipboard_text_focus_shortcuts";

pub(crate) fn clipboard_shortcuts_as_keys(
    ctx: &egui::Context,
    modifiers: egui::Modifiers,
    key: egui::Key,
    pressed: bool,
) -> bool {
    ctx.data_mut(|data| {
        let id = egui::Id::unique(CLIPBOARD_SHORTCUTS_AS_KEYS);
        let forward = data.get_temp::<bool>(id).unwrap_or(false);
        let text_focus = data
            .get_temp::<Vec<egui::KeyboardShortcut>>(egui::Id::unique(
                CLIPBOARD_TEXT_FOCUS_SHORTCUTS,
            ))
            .is_some_and(|shortcuts| {
                shortcuts.iter().any(|shortcut| {
                    shortcut.logical_key == key && modifiers.matches_logically(shortcut.modifiers)
                })
            });
        if forward && pressed && text_focus {
            data.insert_temp(id, false);
            false
        } else {
            forward
        }
    })
}

/// One layout-aware printable key press before Shift/Caps Lock are applied.
///
/// This preserves configurable punctuation and layout-aware Shift bindings.
/// Consumers must still yield to focused text editors.
#[derive(Clone, Debug)]
pub struct LogicalKeyPress {
    pub character: char,
    pub modifiers: egui::Modifiers,
    pub egui_key: Option<egui::Key>,
}

const LOGICAL_KEY_PRESSES: &str = "egui_winit_logical_key_presses";
const LOGICAL_KEY_CONSUMER: &str = "egui_winit_logical_key_consumer";

/// Drains recent logical presses and registers interest for the next frame.
///
/// Input collection expires when this is not called for a frame. Events from a
/// hidden view are never replayed on return. The queue holds at most 128 presses.
pub fn take_logical_key_presses(ctx: &egui::Context) -> Vec<LogicalKeyPress> {
    let frame = ctx.cumulative_frame_nr();
    ctx.data_mut(|data| {
        data.insert_temp(egui::Id::unique(LOGICAL_KEY_CONSUMER), frame);
        data.remove_temp::<Vec<(u64, LogicalKeyPress)>>(egui::Id::unique(LOGICAL_KEY_PRESSES))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(observed, event)| (observed.saturating_add(1) >= frame).then_some(event))
            .collect()
    })
}

/// Keeps optional character shortcuts separate from egui's fixed-key translation.
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
pub(crate) fn record_logical_key_press(
    ctx: &egui::Context,
    modifiers: egui::Modifiers,
    event: &winit::event::KeyEvent,
    logical_key: Option<egui::Key>,
) {
    if event.state != winit::event::ElementState::Pressed {
        return;
    }
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    if let winit::keyboard::Key::Character(text) = event.key_without_modifiers() {
        if let Some(character) = text.chars().next().filter(|_| text.chars().count() == 1) {
            let frame = ctx.cumulative_frame_nr();
            ctx.data_mut(|data| {
                let recent = data.get_temp::<u64>(egui::Id::unique(LOGICAL_KEY_CONSUMER));
                if recent.is_some_and(|observed| observed.saturating_add(1) >= frame) {
                    let presses = data.get_temp_mut_or_default::<Vec<(u64, LogicalKeyPress)>>(
                        egui::Id::unique(LOGICAL_KEY_PRESSES),
                    );
                    presses.retain(|(observed, _)| observed.saturating_add(1) >= frame);
                    if presses.len() < 128 {
                        presses.push((
                            frame,
                            LogicalKeyPress {
                                character,
                                modifiers,
                                egui_key: logical_key,
                            },
                        ));
                    }
                }
            });
        }
    }
}
