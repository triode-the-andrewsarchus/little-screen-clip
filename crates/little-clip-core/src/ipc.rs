//! Inter-process communication (IPC) protocol messages for the daemon, CLI, and Workbench.
//!
//! Defines JSON-serializable request/response commands sent across the UNIX domain socket
//! (`/tmp/little-clipd.sock`).

use crate::item::{ClipItem, ClipPayload, PayloadKind};
use crate::slot::{HandSide, SlotId, SlotManager};
use serde::{Deserialize, Serialize};

/// Request command messages sent from clients (`clipctl`, `clip-workbench`) to `little-clipd`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcRequest {
    /// Queries the complete `SlotManager` state.
    GetState,
    /// Queries lightweight slot cards for polling; use `GetSlotItem` for changed item details.
    GetStateSummary,
    /// Queries completed temporary scene orbits for reuse by the mirror.
    GetScenePreviews,
    /// Pushes plain text into the specified hand.
    PushText {
        text: String,
        #[serde(default = "default_left_hand")]
        hand: HandSide,
    },
    /// Pushes raw image bytes and MIME type into the specified hand.
    PushImage {
        bytes: Vec<u8>,
        mime: String,
        #[serde(default = "default_left_hand")]
        hand: HandSide,
    },
    /// Pushes a single filesystem file path into the specified hand.
    PushFile {
        path: std::path::PathBuf,
        #[serde(default = "default_left_hand")]
        hand: HandSide,
    },
    /// Pushes a batch of file paths into the specified hand.
    PushFiles {
        paths: Vec<std::path::PathBuf>,
        #[serde(default = "default_left_hand")]
        hand: HandSide,
    },
    /// Swaps the contents of two slots.
    SwapSlots(SlotId, SlotId),
    /// Triggers an undo operation, restoring the previous Left Hand item.
    Undo,
    /// Clears the contents of a slot.
    Clear(SlotId),
    /// Selects / focuses a slot for UI navigation.
    Select(SlotId),
    /// Executes a registered operation on the active item (and optional secondary/selected slots).
    ExecuteOp {
        op_name: String,
        secondary_slot: Option<SlotId>,
        #[serde(default)]
        selected_slots: Option<Vec<SlotId>>,
        separator: Option<String>,
    },
    /// Executes a multi-step transformation recipe by name.
    ExecuteRecipe { recipe_name: String },
    /// Executes a transformation recipe with an interactive user prompt.
    ExecuteRecipeWithPrompt {
        recipe_name: String,
        prompt_text: Option<String>,
    },
    /// Executes an ad-hoc pipeline defined in YAML.
    ExecuteGraphYaml { yaml: String },
    /// Lists all available transformation recipe names.
    ListRecipes,
    /// Queries the last executed operation or recipe.
    GetLastAction,
    /// Re-runs the last executed action.
    ExecuteLastAction { prompt_text: Option<String> },
    /// Forces a hardware refresh of the USB display.
    ForceRefreshDisplay,
    /// Reloads configuration from disk.
    ReloadConfig,
    /// Locks the hardware display from updating on new clipboard captures.
    Lock,
    /// Unlocks the hardware display to resume automatic updates.
    Unlock,
    /// Sets the display lock status explicitly.
    SetLocked(bool),
    /// Queries current display lock status.
    GetLockStatus,
    /// Dumps the current rolling audio capture buffer to a temporary WAV file.
    GetRollingAudio,
    /// Captures the next OS clipboard change and routes it to the designated hand.
    RouteNextClipboard {
        hand: HandSide,
        timeout_ms: u64,
        restore_os_clipboard: bool,
    },
    /// Retrieves the `ClipItem` stored at a specific `SlotId`.
    GetSlotItem(SlotId),
    /// Queries active automatic clipboard action toggles.
    GetAutoActions,
    /// Enables or disables an automatic clipboard action toggle.
    SetAutoAction { key: String, enabled: bool },
}

/// Represents the most recently performed operation or recipe for quick re-execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LastAction {
    /// A single named operation with optional parameters.
    Op {
        op_name: String,
        selected_slots: Option<Vec<SlotId>>,
        secondary_slot: Option<SlotId>,
        separator: Option<String>,
    },
    /// A named transformation recipe.
    Recipe { recipe_name: String },
}

fn default_left_hand() -> HandSide {
    HandSide::Left
}

/// Response messages returned by `little-clipd` across the IPC socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcResponse {
    /// Current full slot state.
    State(SlotManager),
    /// Lightweight slot cards and selection state suitable for frequent polling.
    StateSummary(SlotStateSummary),
    /// Completed orbits; clients must acquire their own temporary file links.
    ScenePreviews(Vec<ScenePreview>),
    /// Item from a single queried slot.
    SlotItem(Option<ClipItem>),
    /// Map of auto-action keys to enabled status.
    AutoActions(std::collections::HashMap<String, bool>),
    /// Generic confirmation response with message.
    Success { message: String },
    /// Resulting item generated by an operation.
    OpResult { item: ClipItem },
    /// Compact graph result; the daemon applies full output items without serializing them back.
    PipelineExecuted {
        node_previews: std::collections::HashMap<uuid::Uuid, String>,
        output_title: Option<String>,
        duration_ms: u128,
    },
    /// List of registered recipe identifiers.
    RecipeList(Vec<String>),
    /// Last action executed in the session.
    LastAction(Option<LastAction>),
    /// Indicates an operation needs additional user prompt input.
    NeedsPrompt {
        message: String,
        default_text: String,
    },
    /// Current display lock state (`true` if locked).
    LockStatus(bool),
    /// Rolling audio snapshot file path and metadata.
    RollingAudio {
        path: std::path::PathBuf,
        duration_secs: f32,
        sample_rate: u32,
        channels: u16,
    },
    /// Failure response with error description.
    Error { error: String },
}

/// Exact rendering identity for a temporary scene orbit.
#[derive(Debug, Clone, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub struct ScenePreviewKey {
    pub path: std::path::PathBuf,
    pub modified: Option<std::time::SystemTime>,
    pub file_bytes: u64,
    pub size: [u32; 2],
    pub appearance: String,
    pub triangle_budget: usize,
}

/// A completed orbit owned by the daemon until its visible card is retired.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenePreview {
    pub key: ScenePreviewKey,
    pub directory: std::path::PathBuf,
}

/// Change-detection metadata for one occupied slot, excluding clipboard content and labels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlotCard {
    /// Slot represented by this card.
    pub slot: SlotId,
    /// Stable identity of the current item; clients use it to decide when to fetch details.
    pub item_id: uuid::Uuid,
    /// Payload category, without its content or user-supplied display labels.
    pub payload_kind: PayloadKind,
    /// Completion phase of any background-generated preview for this item.
    /// Missing on older daemons, which clients may poll as a compatibility fallback.
    #[serde(default)]
    pub preview_phase: Option<PreviewPhase>,
}

/// Compact preview readiness marker used to refresh long-lived clients.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PreviewPhase {
    NotApplicable,
    Pending,
    Ready,
    Enhanced,
}

/// Compact state projection for long-lived IPC clients.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SlotStateSummary {
    /// Cards for occupied slots. A missing slot is empty.
    pub cards: Vec<SlotCard>,
    /// Currently focused slot.
    pub cursor: SlotId,
    /// Currently selected slots.
    pub selected: Vec<SlotId>,
}

impl SlotStateSummary {
    /// Projects a slot manager without serializing item payloads, media bytes, paths, or labels.
    pub fn from_manager(manager: &SlotManager) -> Self {
        let cards = SlotId::all()
            .into_iter()
            .filter_map(|slot| {
                manager
                    .get(slot)
                    .map(|item| SlotCard::from_item(slot, item))
            })
            .collect();
        Self {
            cards,
            cursor: manager.cursor,
            selected: manager.selected.clone(),
        }
    }

    /// Returns the card currently occupying `slot`, if any.
    pub fn card(&self, slot: SlotId) -> Option<&SlotCard> {
        self.cards.iter().find(|card| card.slot == slot)
    }
}

impl SlotCard {
    fn from_item(slot: SlotId, item: &ClipItem) -> Self {
        let preview_phase = match &item.payload {
            ClipPayload::AudioRef {
                waveform: Some(waveform),
                ..
            } if waveform.spectral_colors.is_some() => PreviewPhase::Enhanced,
            ClipPayload::AudioRef {
                waveform: Some(_), ..
            }
            | ClipPayload::Image {
                thumbnail: Some(_), ..
            }
            | ClipPayload::Animation {
                thumbnail: Some(_), ..
            } => PreviewPhase::Ready,
            ClipPayload::MultiImage { thumbnails, .. } if !thumbnails.is_empty() => {
                PreviewPhase::Ready
            }
            ClipPayload::AudioRef { .. }
            | ClipPayload::Image { .. }
            | ClipPayload::Animation { .. }
            | ClipPayload::MultiImage { .. } => PreviewPhase::Pending,
            _ => PreviewPhase::NotApplicable,
        };
        Self {
            slot,
            item_id: item.id,
            payload_kind: item.kind(),
            preview_phase: Some(preview_phase),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_preview_ipc_preserves_exact_render_identity() {
        let key = ScenePreviewKey {
            path: "/tmp/source with spaces.sog".into(),
            modified: Some(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::new(5, 42)),
            file_bytes: 128,
            size: [480, 320],
            appearance: "#181818".into(),
            triangle_budget: 100,
        };
        let response = IpcResponse::ScenePreviews(vec![ScenePreview {
            key: key.clone(),
            directory: "/tmp/little-clip-orbit-fixture".into(),
        }]);
        let decoded: IpcResponse =
            serde_json::from_str(&serde_json::to_string(&response).unwrap()).unwrap();
        let IpcResponse::ScenePreviews(previews) = decoded else {
            panic!("Expected scene preview descriptors")
        };
        assert_eq!(previews[0].key, key);
    }

    #[test]
    fn test_lock_ipc_serde() {
        let req = IpcRequest::Lock;
        let json = serde_json::to_string(&req).unwrap();
        let de: IpcRequest = serde_json::from_str(&json).unwrap();
        match de {
            IpcRequest::Lock => (),
            _ => panic!("Expected Lock"),
        }

        let req = IpcRequest::SetLocked(true);
        let json = serde_json::to_string(&req).unwrap();
        let de: IpcRequest = serde_json::from_str(&json).unwrap();
        match de {
            IpcRequest::SetLocked(true) => (),
            _ => panic!("Expected SetLocked(true)"),
        }

        let resp = IpcResponse::LockStatus(true);
        let json = serde_json::to_string(&resp).unwrap();
        let de: IpcResponse = serde_json::from_str(&json).unwrap();
        match de {
            IpcResponse::LockStatus(true) => (),
            _ => panic!("Expected LockStatus(true)"),
        }
    }

    #[test]
    fn test_rolling_audio_ipc_serde() {
        let req = IpcRequest::GetRollingAudio;
        let json = serde_json::to_string(&req).unwrap();
        let de: IpcRequest = serde_json::from_str(&json).unwrap();
        match de {
            IpcRequest::GetRollingAudio => (),
            _ => panic!("Expected GetRollingAudio"),
        }

        let resp = IpcResponse::RollingAudio {
            path: std::path::PathBuf::from("/tmp/snapshot.wav"),
            duration_secs: 30.0,
            sample_rate: 44100,
            channels: 2,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let de: IpcResponse = serde_json::from_str(&json).unwrap();
        match de {
            IpcResponse::RollingAudio {
                path,
                duration_secs,
                sample_rate,
                channels,
            } => {
                assert_eq!(path, std::path::PathBuf::from("/tmp/snapshot.wav"));
                assert_eq!(duration_secs, 30.0);
                assert_eq!(sample_rate, 44100);
                assert_eq!(channels, 2);
            }
            _ => panic!("Expected RollingAudio"),
        }
    }

    #[test]
    fn pipeline_execution_response_serde_keeps_previews_without_clip_payloads() {
        let id = uuid::Uuid::new_v4();
        let response = IpcResponse::PipelineExecuted {
            node_previews: [(id, "preview".to_owned())].into_iter().collect(),
            output_title: Some("result".to_owned()),
            duration_ms: 12,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(!json.contains("payload"));
        assert!(matches!(
            serde_json::from_str::<IpcResponse>(&json).unwrap(),
            IpcResponse::PipelineExecuted {
                node_previews,
                output_title: Some(title),
                duration_ms: 12,
            } if node_previews.get(&id).map(String::as_str) == Some("preview") && title == "result"
        ));
    }

    #[test]
    fn state_summary_excludes_item_payloads_and_tracks_replacement() {
        let mut manager = SlotManager::new();
        manager.push_left(ClipItem::from_text("private clipboard text"));
        let first = SlotStateSummary::from_manager(&manager);
        let json = serde_json::to_string(&first).unwrap();

        assert!(!json.contains("private clipboard text"));
        assert_eq!(first.cards.len(), 1);
        assert_eq!(
            first.card(SlotId::LeftHand).unwrap().payload_kind,
            PayloadKind::Text
        );
        assert_eq!(
            first.card(SlotId::LeftHand).unwrap().preview_phase,
            Some(PreviewPhase::NotApplicable)
        );

        // Existing daemons omit the new field; the workbench can still poll them.
        let mut legacy_json: serde_json::Value = serde_json::from_str(&json).unwrap();
        legacy_json["cards"][0]
            .as_object_mut()
            .unwrap()
            .remove("preview_phase");
        let legacy: SlotStateSummary = serde_json::from_value(legacy_json).unwrap();
        assert_eq!(legacy.card(SlotId::LeftHand).unwrap().preview_phase, None);

        manager.push_left(ClipItem::from_text("replacement"));
        let second = SlotStateSummary::from_manager(&manager);
        assert_ne!(
            first.card(SlotId::LeftHand).unwrap().item_id,
            second.card(SlotId::LeftHand).unwrap().item_id
        );
        assert!(second.card(SlotId::LeftPrior1).is_some());
    }

    #[test]
    fn state_summary_tracks_async_audio_preview_completion_without_replacing_item() {
        let mut item = ClipItem::from_text("audio");
        item.payload = ClipPayload::AudioRef {
            path: "/tmp/preview.wav".into(),
            duration_ms: 1000,
            sample_rate: 44100,
            channels: 1,
            format: "WAV".into(),
            waveform: None,
        };
        let item_id = item.id;
        let mut manager = SlotManager::new();
        manager.push_left(item);

        let pending = SlotStateSummary::from_manager(&manager);
        assert_eq!(
            pending.card(SlotId::LeftHand).unwrap().preview_phase,
            Some(PreviewPhase::Pending)
        );

        let waveform = crate::audio::AudioWaveform {
            channels: 1,
            sample_rate: 44100,
            duration_ms: 1000,
            ch1_peaks: vec![0.5],
            ch2_peaks: None,
            spectral_colors: None,
            is_snippet: false,
            snippet_start_ms: 0,
            snippet_duration_ms: 0,
        };
        assert!(manager.update_audio_waveform(item_id, waveform));
        let ready = SlotStateSummary::from_manager(&manager);
        assert_eq!(ready.card(SlotId::LeftHand).unwrap().item_id, item_id);
        assert_eq!(
            ready.card(SlotId::LeftHand).unwrap().preview_phase,
            Some(PreviewPhase::Ready)
        );
        assert_ne!(pending, ready);

        assert!(manager.attach_audio_spectral_colors(item_id, vec![[1, 2, 3]]));
        let enhanced = SlotStateSummary::from_manager(&manager);
        assert_eq!(
            enhanced.card(SlotId::LeftHand).unwrap().preview_phase,
            Some(PreviewPhase::Enhanced)
        );
        assert_ne!(ready, enhanced);
        assert!(
            !serde_json::to_string(&enhanced)
                .unwrap()
                .contains("preview.wav")
        );
    }
}
