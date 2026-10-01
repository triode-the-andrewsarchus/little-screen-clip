//! Visual node graph canvas editor for custom pipelines and recipes.
//!
//! Provides interactive drag-and-drop node graph design, wire connections, parameter tweaking,
//! recipe loading/saving, and live graph execution.

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
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Actions emitted by the canvas UI back to the main workbench loop.
pub enum CanvasAction {
    /// Request execution of the current graph by the daemon.
    RunPipeline { yaml: String },
}

/// Interactive state for the visual node pipeline graph editor canvas.
pub struct NodeCanvasState {
    pub script_editors: HashMap<Uuid, crate::script_editor::ScriptEditor>,
    pub graph: NodeGraph,
    pub selected_node: Option<Uuid>,
    pub dragging_from: Option<(Uuid, String, bool)>, // (node_id, pin_name, is_output)
    pub node_previews: HashMap<Uuid, String>,
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
    pub status_message: Option<(String, bool)>, // (text, is_error)
    pub available_recipes: Vec<Recipe>,
    pub selected_recipe_name: String,
    pub last_run_duration_ms: Option<u128>,
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
            dragging_from: None,
            node_previews: HashMap::new(),
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
            status_message: None,
            available_recipes: recipes,
            selected_recipe_name: sel_name,
            last_run_duration_ms: None,
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

    pub fn snapshot_undo(&mut self) {
        if self.undo_stack.len() >= 50 {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(self.graph.clone());
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.graph.clone());
            self.graph = prev;
            self.status_message = Some(("↶ Undo".to_string(), false));
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.graph.clone());
            self.graph = next;
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
                output_title,
                duration_ms,
            } => {
                self.node_previews = node_previews;
                self.last_run_duration_ms = Some(duration_ms);
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
