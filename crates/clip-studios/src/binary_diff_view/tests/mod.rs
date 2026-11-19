use super::*;

mod handoff;
mod presentation;
mod state;

pub(super) fn trigger_global_shortcut(state: &mut BinaryDiffViewState, key: egui::Key) {
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
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
    ctx.end_pass().drop_without_applying_deltas();
    assert!(!close_requested);
}
