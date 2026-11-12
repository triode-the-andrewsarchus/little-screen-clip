//! Visual node graph canvas editor for custom pipelines and recipes.
//!
//! Provides interactive drag-and-drop node graph design, wire connections, parameter tweaking,
//! recipe loading/saving, and live graph execution.

mod custom_ui;
mod drawing;
mod panels;
mod workspace;

use crate::icons::Icon;
use crate::ui_ext::{BODY_FONT_SIZE, CONTROL_HEIGHT};
use crate::{hex_to_c32, hex_to_c32_alpha};
use drawing::*;
use eframe::egui::{self, Color32, Id, Pos2, Rect, Rounding, Stroke, Vec2};
use little_clip_core::palette::AppPalette;
use little_clip_ops::pipeline::catalog::{instantiate_template, search_templates};
use little_clip_ops::pipeline::recipes::{
    create_script_file_scaffold, get_recipes_dir, load_all_recipes, load_recipes_in_dir,
    make_basic_template_recipe, make_pad_and_caption_recipe, save_recipe, save_recipe_in_dir,
    sync_recipe_script_headers, sync_recipe_script_headers_in_dir,
};
use little_clip_ops::pipeline::types::{
    Connection, NodeGraph, NodeInstance, NodeKind, PortType, Recipe,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Actions emitted by the canvas UI back to the main workbench loop.
pub enum CanvasAction {
    /// Request execution of the current graph by the daemon.
    RunPipeline { yaml: String },
}

/// Wire drag dropped into empty canvas space, awaiting node selection.
#[derive(Debug, Clone)]
pub struct PendingWireConnect {
    pub node_id: Uuid,
    pub pin_name: String,
    pub is_output: bool,
    pub port_type: PortType,
}

/// Interactive state for the visual node pipeline graph editor canvas.
pub struct NodeCanvasState {
    pub script_editors: HashMap<Uuid, crate::script_editor::ScriptEditor>,
    pub graph: NodeGraph,
    pub selected_node: Option<Uuid>,
    pub selected_nodes: HashSet<Uuid>,
    pub box_select_start: Option<Pos2>,
    box_select_additive: bool,
    box_select_base: HashSet<Uuid>,
    pub dragging_from: Option<(Uuid, String, bool)>, // (node_id, pin_name, is_output)
    pub pending_wire_connect: Option<PendingWireConnect>,
    pub node_previews: HashMap<Uuid, String>,
    pub node_telemetry: HashMap<Uuid, little_clip_core::ipc::NodeTelemetry>,
    pub catalog_open: bool,
    pub catalog_search: String,
    pub catalog_category: String,
    pub spawn_node_pos: Option<[f32; 2]>,
    pub save_dialog_open: bool,
    pub save_name: String,
    pub save_is_save_as: bool,
    pub current_recipe_file: Option<PathBuf>,
    pub undo_stack: Vec<NodeGraph>,
    pub redo_stack: Vec<NodeGraph>,
    property_edit_group: Option<Id>,
    pub status_message: Option<(String, bool)>, // (text, is_error)
    pub available_recipes: Vec<Recipe>,
    pub selected_recipe_name: String,
    pub last_run_duration_ms: Option<u128>,
    pub pending_open_studios: Vec<little_clip_core::ipc::StudioOpenTarget>,
    pub pending_external_opens: Vec<little_clip_core::ipc::ExternalOpenTarget>,
}

impl NodeCanvasState {
    /// Creates a canvas state initialized from the available recipes.
    pub fn new() -> Self {
        let recipes = load_all_recipes();
        let default_recipe = recipes
            .iter()
            .find(|r| r.name.contains("Pad and Caption"))
            .cloned()
            .unwrap_or_else(make_pad_and_caption_recipe);

        let sel_name = default_recipe.name.clone();
        let graph = default_recipe.graph;

        let safe_name: String = sel_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = get_recipes_dir().join(format!("{}.yaml", safe_name.to_lowercase()));
        let current_recipe_file = if path.exists() { Some(path) } else { None };

        Self {
            graph,
            script_editors: HashMap::new(),
            selected_node: None,
            selected_nodes: HashSet::new(),
            box_select_start: None,
            box_select_additive: false,
            box_select_base: HashSet::new(),
            dragging_from: None,
            pending_wire_connect: None,
            node_previews: HashMap::new(),
            node_telemetry: HashMap::new(),
            catalog_open: false,
            catalog_search: String::new(),
            catalog_category: "All".to_string(),
            spawn_node_pos: None,
            save_dialog_open: false,
            save_name: String::new(),
            save_is_save_as: false,
            current_recipe_file,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            property_edit_group: None,
            status_message: None,
            available_recipes: recipes,
            selected_recipe_name: sel_name,
            last_run_duration_ms: None,
            pending_open_studios: Vec::new(),
            pending_external_opens: Vec::new(),
        }
    }

    fn save_script_drafts(&mut self) -> Result<(), String> {
        for node in &mut self.graph.nodes {
            if let Some(editor) = self.script_editors.get_mut(&node.id)
                && editor.dirty()
            {
                editor.save(node)?;
            }
        }
        Ok(())
    }

    fn refresh_saved_script_editors(&mut self) {
        for node in &self.graph.nodes {
            if let Some(editor) = self.script_editors.get_mut(&node.id) {
                editor.refresh_after_recipe_save(node);
            }
        }
    }

    fn apply_property_edits(&mut self, edits: Vec<(Uuid, String, String)>, focused: Option<Id>) {
        if edits.is_empty() {
            if focused.is_none() {
                self.property_edit_group = None;
            }
            return;
        }
        if focused.is_none() || self.property_edit_group != focused {
            self.snapshot_undo();
        }
        self.property_edit_group = focused;
        for (node_id, key, value) in edits {
            if let Some(node) = self.graph.nodes.iter_mut().find(|node| node.id == node_id) {
                node.props.insert(key, value);
            }
        }
    }

    pub fn snapshot_undo(&mut self) {
        self.property_edit_group = None;
        if self.undo_stack.len() >= 50 {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(self.graph.clone());
        self.redo_stack.clear();
    }

    /// Returns `true` if `node_id` is part of the current single or multi-node selection.
    pub fn is_node_selected(&self, node_id: Uuid) -> bool {
        self.selected_node == Some(node_id) || self.selected_nodes.contains(&node_id)
    }

    /// Returns the union of `selected_nodes` and `selected_node` for group operations.
    pub fn effective_selected_nodes(&self) -> HashSet<Uuid> {
        let mut selected = self.selected_nodes.clone();
        if let Some(id) = self.selected_node {
            selected.insert(id);
        }
        selected
    }

    /// Selects a single node and clears any previous multi-selection.
    pub fn select_single(&mut self, node_id: Uuid) {
        self.selected_nodes.clear();
        self.selected_nodes.insert(node_id);
        self.selected_node = Some(node_id);
    }

    /// Toggles `node_id` in the current selection and keeps `selected_node` in sync.
    pub fn toggle_node_selection(&mut self, node_id: Uuid) {
        let mut selected = self.effective_selected_nodes();
        if !selected.remove(&node_id) {
            selected.insert(node_id);
        }
        self.set_selected_nodes(selected);
    }

    /// Replaces the current selection with `nodes` and keeps `selected_node` in sync.
    pub fn set_selected_nodes(&mut self, nodes: HashSet<Uuid>) {
        self.selected_nodes = nodes;
        self.selected_node = if self.selected_nodes.len() == 1 {
            self.selected_nodes.iter().next().copied()
        } else {
            None
        };
    }

    /// Clears all node selection state.
    pub fn clear_selection(&mut self) {
        self.selected_nodes.clear();
        self.selected_node = None;
    }

    /// Removes any selected IDs that no longer exist in `self.graph.nodes`.
    fn prune_selection(&mut self) {
        let valid: HashSet<Uuid> = self.graph.nodes.iter().map(|n| n.id).collect();
        self.selected_nodes.retain(|id| valid.contains(id));
        if let Some(id) = self.selected_node
            && !valid.contains(&id)
        {
            self.selected_node = None;
        }
        if self.selected_node.is_none() && self.selected_nodes.len() == 1 {
            self.selected_node = self.selected_nodes.iter().next().copied();
        }
    }

    /// Deletes all currently selected nodes and their incident connections, pushing an undo snapshot.
    /// Returns the number of deleted nodes.
    pub fn delete_selected_nodes(&mut self) -> usize {
        let to_delete = self.effective_selected_nodes();
        if to_delete.is_empty() {
            return 0;
        }
        self.snapshot_undo();
        let count = to_delete.len();
        self.graph.nodes.retain(|n| !to_delete.contains(&n.id));
        self.graph
            .connections
            .retain(|c| !to_delete.contains(&c.from_node) && !to_delete.contains(&c.to_node));
        self.clear_selection();
        self.status_message = Some((
            if count == 1 {
                "Deleted node".to_string()
            } else {
                format!("Deleted {count} nodes")
            },
            false,
        ));
        count
    }

    /// Translates all currently selected nodes in world coordinates by `delta`.
    pub fn translate_selected_nodes(&mut self, delta: Vec2) {
        let selected = self.effective_selected_nodes();
        if selected.is_empty() {
            return;
        }
        for node in &mut self.graph.nodes {
            if selected.contains(&node.id) {
                node.pos[0] += delta.x;
                node.pos[1] += delta.y;
            }
        }
    }

    pub fn undo(&mut self) {
        self.property_edit_group = None;
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.graph.clone());
            self.graph = prev;
            self.prune_selection();
            self.status_message = Some(("↶ Undo".to_string(), false));
        }
    }

    pub fn redo(&mut self) {
        self.property_edit_group = None;
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.graph.clone());
            self.graph = next;
            self.prune_selection();
            self.status_message = Some(("↷ Redo".to_string(), false));
        }
    }

    pub fn reload_recipes(&mut self) {
        self.available_recipes = load_all_recipes();
    }

    fn reload_recipes_in_dir(&mut self, dir: &Path) {
        self.available_recipes = load_recipes_in_dir(dir);
    }

    fn run_pipeline_action(&mut self) -> Option<CanvasAction> {
        if let Err(error) = self.save_script_drafts() {
            self.status_message = Some((error, true));
            return None;
        }
        match serde_yaml::to_string(&self.graph) {
            Ok(yaml) => {
                self.status_message = Some(("Running pipeline…".to_owned(), false));
                Some(CanvasAction::RunPipeline { yaml })
            }
            Err(error) => {
                self.status_message = Some((format!("Error: {}", error), true));
                None
            }
        }
    }

    pub fn apply_pipeline_response(&mut self, response: little_clip_core::ipc::IpcResponse) {
        match response {
            little_clip_core::ipc::IpcResponse::PipelineExecuted {
                node_previews,
                node_telemetry,
                output_title,
                duration_ms,
                open_studios,
                external_opens,
            } => {
                self.node_previews = node_previews;
                self.node_telemetry = node_telemetry;
                self.last_run_duration_ms = Some(duration_ms);
                self.pending_open_studios = open_studios;
                self.pending_external_opens = external_opens;
                let output = output_title.as_deref().unwrap_or("Done");
                self.status_message = Some((
                    format!("Executed in {}ms. Output: {}", duration_ms, output),
                    false,
                ));
            }
            little_clip_core::ipc::IpcResponse::Error { error } => {
                self.status_message = Some((format!("Error: {}", error), true));
            }
            _ => {
                self.status_message =
                    Some(("Error: unexpected pipeline response".to_owned(), true));
            }
        }
    }
}

#[cfg(test)]
mod tests;
