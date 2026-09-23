//! Keeps the open workbench's slot state in sync with daemon updates.

use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use little_clip_core::ipc::{IpcRequest, IpcResponse, SlotStateSummary};
use little_clip_core::{ClipItem, ClipPayload, SlotId, SlotManager};

use crate::send_ipc_request;

fn preview_may_progress(item: &ClipItem) -> bool {
    match &item.payload {
        ClipPayload::AudioRef { waveform, .. } => waveform.as_ref().is_none_or(|waveform| {
            !waveform.ch1_peaks.is_empty() && waveform.spectral_colors.is_none()
        }),
        ClipPayload::Image { thumbnail, .. } => thumbnail.is_none(),
        ClipPayload::Animation { thumbnail, .. } => thumbnail.is_none(),
        ClipPayload::MultiImage {
            preview_indices,
            thumbnails,
            ..
        } => thumbnails.len() < preview_indices.len(),
        _ => false,
    }
}

fn needs_hydration(
    slot: SlotId,
    summary: &SlotStateSummary,
    previous: &SlotStateSummary,
    state: &SlotManager,
) -> bool {
    summary.card(slot) != previous.card(slot)
        || summary
            .card(slot)
            .is_some_and(|card| card.preview_phase.is_none())
            && state.get(slot).is_some_and(preview_may_progress)
}

fn hydrate_changed_slots(
    summary: &SlotStateSummary,
    previous: &SlotStateSummary,
    state: &mut SlotManager,
) -> Result<bool, ()> {
    let mut changed = false;
    for slot in SlotId::all() {
        if !needs_hydration(slot, summary, previous, state) {
            continue;
        }
        let Ok(IpcResponse::SlotItem(item)) = send_ipc_request(&IpcRequest::GetSlotItem(slot))
        else {
            return Err(());
        };
        let current = state.get_mut(slot).expect("all SlotId values are valid");
        if *current != item {
            *current = item;
            changed = true;
        }
    }
    Ok(changed)
}

pub(crate) fn spawn_state_poller(initial_state: SlotManager) -> Receiver<SlotManager> {
    let (state_tx, state_rx) = channel();
    std::thread::spawn(move || {
        let mut state = initial_state;
        let mut previous_summary = SlotStateSummary::from_manager(&state);
        let mut poll_interval_ms = 250u64;
        let mut next_legacy_preview_probe = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(poll_interval_ms));
            let Ok(IpcResponse::StateSummary(summary)) =
                send_ipc_request(&IpcRequest::GetStateSummary)
            else {
                poll_interval_ms = (poll_interval_ms + 100).min(1000);
                continue;
            };
            let summary_changed = summary != previous_summary;
            if !summary_changed {
                poll_interval_ms = (poll_interval_ms + 50).min(1000);
                if !summary
                    .cards
                    .iter()
                    .any(|card| card.preview_phase.is_none())
                    || Instant::now() < next_legacy_preview_probe
                {
                    continue;
                }
                next_legacy_preview_probe = Instant::now() + Duration::from_secs(2);
            } else {
                poll_interval_ms = 250;
            }

            let Ok(hydrated_changed) =
                hydrate_changed_slots(&summary, &previous_summary, &mut state)
            else {
                continue;
            };
            state.cursor = summary.cursor;
            state.selected = summary.selected.clone();
            previous_summary = summary;
            if (summary_changed || hydrated_changed) && state_tx.send(state.clone()).is_err() {
                break;
            }
        }
    });
    state_rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_daemon_card_rehydrates_pending_preview_without_an_identity_change() {
        let mut item = ClipItem::from_text("gallery");
        item.payload = ClipPayload::MultiImage {
            paths: vec![],
            total_count: 1,
            preview_indices: vec![0],
            thumbnails: vec![],
        };
        let mut manager = SlotManager::new();
        manager.push_left(item);
        let mut legacy = SlotStateSummary::from_manager(&manager);
        legacy.cards[0].preview_phase = None;
        assert!(needs_hydration(
            SlotId::LeftHand,
            &legacy,
            &legacy,
            &manager
        ));
    }
}
