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
    ExecuteGraphYaml {
        yaml: String,
    },
    ListRecipes,
    ForceRefreshDisplay,
    ReloadConfig,
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
    Error { error: String },
}

