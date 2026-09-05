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
    Error { error: String },
}


