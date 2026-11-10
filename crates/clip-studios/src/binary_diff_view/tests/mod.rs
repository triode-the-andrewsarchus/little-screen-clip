use super::*;

mod handoff;
mod presentation;
mod state;

pub(super) fn trigger_global_shortcut(state: &mut BinaryDiffViewState, key: egui::Key) {
    let ctx = egui::Context::default();
    ctx.begin_frame(egui::RawInput {
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    let mut close_requested = false;
    state.handle_global_shortcuts(&ctx, &mut close_requested);
    let _ = ctx.end_frame();
    assert!(!close_requested);
}
