use crate::item::ClipItem;
use crate::slot::{HandSide, SlotId, SlotManager};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcRequest {
    GetState,
    PushText {
        text: String,
        #[serde(default = "default_left_hand")]
        hand: HandSide,
    },
    PushImage {
        bytes: Vec<u8>,
        mime: String,
        #[serde(default = "default_left_hand")]
        hand: HandSide,
    },
    PushFile {
        path: std::path::PathBuf,
        #[serde(default = "default_left_hand")]
        hand: HandSide,
    },
    PushFiles {
        paths: Vec<std::path::PathBuf>,
        #[serde(default = "default_left_hand")]
        hand: HandSide,
    },
    SwapSlots(SlotId, SlotId),
    Undo,
    Clear(SlotId),
    Select(SlotId),
    ExecuteOp {
        op_name: String,
        secondary_slot: Option<SlotId>,
        #[serde(default)]
        selected_slots: Option<Vec<SlotId>>,
        separator: Option<String>,
    },
    ExecuteRecipe {
        recipe_name: String,
    },
    ExecuteRecipeWithPrompt {
        recipe_name: String,
        prompt_text: Option<String>,
    },
    ExecuteGraphYaml {
        yaml: String,
    },
    ListRecipes,
    GetLastAction,
    ExecuteLastAction {
        prompt_text: Option<String>,
    },
    ForceRefreshDisplay,
    ReloadConfig,
    Lock,
    Unlock,
    SetLocked(bool),
    GetLockStatus,
    GetRollingAudio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LastAction {
    Op {
        op_name: String,
        selected_slots: Option<Vec<SlotId>>,
        secondary_slot: Option<SlotId>,
        separator: Option<String>,
    },
    Recipe {
        recipe_name: String,
    },
}

fn default_left_hand() -> HandSide {
    HandSide::Left
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IpcResponse {
    State(SlotManager),
    Success { message: String },
    OpResult { item: ClipItem },
    RecipeList(Vec<String>),
    LastAction(Option<LastAction>),
    NeedsPrompt { message: String, default_text: String },
    LockStatus(bool),
    RollingAudio {
        path: std::path::PathBuf,
        duration_secs: f32,
        sample_rate: u32,
        channels: u16,
    },
    Error { error: String },
}

#[cfg(test)]
mod tests {
    use super::*;

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
}


