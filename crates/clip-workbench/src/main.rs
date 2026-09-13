//! Little Screen Clip desktop GUI workbench (`clip-workbench`).
//!
//! An egui-based control center providing multi-slot visual inspection, operation command palette,
//! visual node graph builder, audio/image/video/3D/PDF/MIDI inspection windows, and live screen mirror.

use eframe::egui;
use little_clip_core::color::parse_hex_color_rgba;
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use little_clip_core::theme::{find_themes_dir, load_themes_from_dir, AppPalette, DerivedHarmonies, Theme};
use little_clip_core::{AppConfig, ClipItem, SlotId, SlotManager};
use little_clip_ops::pipeline::recipes::{find_recipe_by_name, load_all_recipes};
use little_clip_ops::pipeline::types::{NodeKind, Recipe};
use little_clip_ops::{get_applicable_ops, OpDescriptor};
use little_clip_render::{ScreenRenderer, SCREEN_HEIGHT, SCREEN_WIDTH};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

mod ansi;
mod audio_preview;
mod binary_diff_view;
mod canvas;
mod diff_view;
mod image_preview;
mod rolling_audio_strip;
mod spreadsheet_view;
mod pdf_view;
mod midi_view;
mod stl_view;
mod text_preview;
use audio_preview::AudioPreviewState;
use binary_diff_view::BinaryDiffViewState;
use canvas::*;
use diff_view::DiffViewerState;
use image_preview::ImagePreviewState;
use rolling_audio_strip::RollingAudioStripState;
use spreadsheet_view::SpreadsheetViewerState;
use pdf_view::PdfEditorState;
use midi_view::MidiViewerState;
use stl_view::StlViewerState;
use text_preview::TextPreviewState;
use little_clip_ops::binary_diff::resolve_binary_diff_pair;
use little_clip_ops::diff::{resolve_diff_pair, resolve_diff_target, DiffDisplayMode};

/// Filter category for theme selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeFilter {
    All,
    Dark,
    Light,
}

/// Primary navigation view tabs in the desktop workbench.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkbenchView {
    /// Command palette for searching and executing transform operations and recipes.
    Palette,
    /// Interactive visual node pipeline recipe editor.
    NodeMachine,
    /// Automated action trigger rules configuration.
    AutoActions,
    /// Hundred Rabbits color themes selector and live config editor.
    Themes,
}

fn hex_to_c32(hex: &str) -> egui::Color32 {
    let (r, g, b, a) = parse_hex_color_rgba(hex);
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

fn apply_theme_to_egui(ctx: &egui::Context, palette: &AppPalette) {
    let mut visuals = if palette.is_dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };

    visuals.panel_fill = hex_to_c32(&palette.background);
    visuals.window_fill = hex_to_c32(&palette.prior_card_bg);
    visuals.extreme_bg_color = hex_to_c32(&palette.hero_card_bg);

    visuals.widgets.noninteractive.bg_fill = hex_to_c32(&palette.prior_card_bg);
    visuals.widgets.noninteractive.fg_stroke.color = hex_to_c32(&palette.title_color);
    visuals.widgets.noninteractive.bg_stroke.color = hex_to_c32(&palette.b_low);

    visuals.widgets.inactive.bg_fill = hex_to_c32(&palette.undo_card_bg);
    visuals.widgets.inactive.fg_stroke.color = hex_to_c32(&palette.subtitle_color);
    visuals.widgets.inactive.bg_stroke.color = hex_to_c32(&palette.b_low);

    visuals.widgets.hovered.bg_fill = hex_to_c32(&palette.prior_card_bg);
    visuals.widgets.hovered.fg_stroke.color = hex_to_c32(&palette.title_color);
    visuals.widgets.hovered.bg_stroke.color = hex_to_c32(&palette.left_hand_border);

    visuals.widgets.active.bg_fill = hex_to_c32(&palette.hero_card_bg);
    visuals.widgets.active.fg_stroke.color = egui::Color32::WHITE;
    visuals.widgets.active.bg_stroke.color = hex_to_c32(&palette.cursor_border);

    visuals.selection.bg_fill = hex_to_c32(&palette.left_hand_border);
    visuals.selection.stroke.color = egui::Color32::WHITE;

    visuals.hyperlink_color = hex_to_c32(&palette.url_color);

    ctx.set_visuals(visuals);
}


/// Action item displayed in the command palette search list.
#[derive(Clone)]
enum PaletteAction {
    /// Built-in Rust transformation operation.
    Op(OpDescriptor),
    /// Configured visual pipeline recipe.
    Recipe(Recipe),
}

impl PaletteAction {
    fn label(&self) -> &str {
        match self {
            PaletteAction::Op(op) => op.label,
            PaletteAction::Recipe(r) => &r.name,
        }
    }

    fn category(&self) -> &str {
        match self {
            PaletteAction::Op(op) => op.category,
            PaletteAction::Recipe(_) => "Recipe",
        }
    }

    fn description(&self) -> &str {
        match self {
            PaletteAction::Op(op) => op.description,
            PaletteAction::Recipe(r) => &r.description,
        }
    }
}

struct InAppPromptState {
    recipe_name: String,
    message: String,
    input_text: String,
    first_frame: bool,
}

struct WorkbenchApp {
    state: SlotManager,
    state_rx: Receiver<SlotManager>,
    view: WorkbenchView,
    canvas_state: NodeCanvasState,
    recipes: Vec<Recipe>,
    search_query: String,
    selected_action_index: usize,
    selected_slots: Vec<SlotId>,
    status_msg: Option<String>,
    renderer: ScreenRenderer,
    mirror_texture: Option<egui::TextureHandle>,
    dirty_mirror: bool,
    in_app_prompt: Option<InAppPromptState>,
    diff_viewer: Option<DiffViewerState>,
    binary_diff_viewer: Option<BinaryDiffViewState>,
    audio_preview: Option<AudioPreviewState>,
    image_preview: Option<ImagePreviewState>,
    spreadsheet_viewer: Option<SpreadsheetViewerState>,
    pdf_viewer: Option<PdfEditorState>,
    midi_viewer: Option<MidiViewerState>,
    stl_viewer: Option<StlViewerState>,
    text_preview: Option<TextPreviewState>,
    rolling_audio_strip: Option<RollingAudioStripState>,
    dragging_slot: Option<SlotId>,
    config: AppConfig,
    available_themes: Vec<Theme>,
    theme_search: String,
    theme_filter: ThemeFilter,
    theme_applied: bool,
}

impl WorkbenchApp {
    fn new(
        state: SlotManager,
        start_with_diff: bool,
        start_with_binary_diff: bool,
        initial_audio: Option<PathBuf>,
        initial_image: Option<PathBuf>,
    ) -> Self {
        let mut config = AppConfig::load();
        // Desktop monitor mirror is always upright (no 180 flip)
        config.display.rotate_180 = false;
        let renderer = ScreenRenderer::new(config.clone())
            .unwrap_or_else(|_| ScreenRenderer::fallback(config.clone()));

        let (state_tx, state_rx) = channel::<SlotManager>();

        // Background poller thread for live updates from daemon
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(250));
                if let Ok(IpcResponse::State(s)) = send_ipc_request(&IpcRequest::GetState) {
                    if state_tx.send(s).is_err() {
                        break; // UI closed
                    }
                }
            }
        });

        let canvas_state = NodeCanvasState::new();
        let recipes = load_all_recipes();

        let audio_preview = initial_audio.map(|p| {
            let title = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Audio file".to_string());
            AudioPreviewState::new(p, title).with_editor(config.audio.editor.clone())
        });

        let image_preview = initial_image.and_then(|p| {
            ImagePreviewState::from_path(p, &config).ok()
        });

        let rolling_audio_strip = Some(RollingAudioStripState::new());

        let mut available_themes = load_themes_from_dir(find_themes_dir());
        if available_themes.is_empty() {
            available_themes.push(Theme::apollo());
        }

        let mut app = Self {
            state,
            state_rx,
            view: WorkbenchView::Palette,
            canvas_state,
            recipes,
            search_query: String::new(),
            selected_action_index: 0,
            selected_slots: vec![SlotId::LeftHand],
            status_msg: None,
            renderer,
            mirror_texture: None,
            dirty_mirror: true,
            in_app_prompt: None,
            diff_viewer: None,
            binary_diff_viewer: None,
            audio_preview,
            image_preview,
            spreadsheet_viewer: None,
            pdf_viewer: None,
            midi_viewer: None,
            stl_viewer: None,
            text_preview: None,
            rolling_audio_strip,
            dragging_slot: None,
            config,
            available_themes,
            theme_search: String::new(),
            theme_filter: ThemeFilter::All,
            theme_applied: false,
        };

        if start_with_binary_diff {
            app.open_binary_diff_viewer();
        } else if start_with_diff {
            app.open_diff_viewer();
        }

        app
    }

    fn open_diff_viewer(&mut self) {
        let (lhs_target, rhs_target) = if self.selected_slots.len() >= 2 {
            let items: Vec<&ClipItem> = self.selected_slots.iter().filter_map(|s| self.state.get(*s)).collect();
            let img_items: Vec<&ClipItem> = items.iter().filter(|i| resolve_diff_target(i).map(|t| t.is_image()).unwrap_or(false)).copied().collect();
            if img_items.len() >= 2 {
                (resolve_diff_target(img_items[0]), resolve_diff_target(img_items[1]))
            } else if let Some((t1, t2)) = items.iter().find_map(|i| resolve_diff_pair(i).filter(|(a, b)| a.is_image() && b.is_image())) {
                (Some(t1), Some(t2))
            } else if items.len() >= 2 {
                (resolve_diff_target(items[0]), resolve_diff_target(items[1]))
            } else if items.len() == 1 {
                if let Some((t1, t2)) = resolve_diff_pair(items[0]) {
                    (Some(t1), Some(t2))
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            }
        } else if self.selected_slots.len() == 1 {
            let selected_item = self.state.get(self.selected_slots[0]);
            if let Some(item) = selected_item {
                if let Some((t1, t2)) = resolve_diff_pair(item) {
                    (Some(t1), Some(t2))
                } else {
                    let other = if self.selected_slots[0] == SlotId::LeftHand {
                        self.state.right_hand()
                            .or_else(|| self.state.get(SlotId::LeftPrior1))
                            .or_else(|| self.state.get(SlotId::Undo))
                    } else {
                        self.state.left_hand()
                    };
                    (resolve_diff_target(item), other.and_then(resolve_diff_target))
                }
            } else {
                (None, None)
            }
        } else {
            let left = self.state.left_hand();
            let right = self.state.right_hand();
            if let (Some(l), Some(r)) = (left, right) {
                if let Some((t1, t2)) = resolve_diff_pair(l).filter(|(a, b)| a.is_image() && b.is_image()) {
                    if !resolve_diff_target(r).map(|t| t.is_image()).unwrap_or(false) {
                        (Some(t1), Some(t2))
                    } else {
                        (resolve_diff_target(l), resolve_diff_target(r))
                    }
                } else {
                    (resolve_diff_target(l), resolve_diff_target(r))
                }
            } else if let Some(l) = left {
                if let Some((t1, t2)) = resolve_diff_pair(l) {
                    (Some(t1), Some(t2))
                } else {
                    let prior = self
                        .state
                        .get(SlotId::LeftPrior1)
                        .or_else(|| self.state.get(SlotId::Undo))
                        .or_else(|| self.state.get(SlotId::LeftPrior2));
                    (resolve_diff_target(l), prior.and_then(resolve_diff_target))
                }
            } else {
                (None, None)
            }
        };

        match (lhs_target, rhs_target) {
            (Some(lhs), Some(rhs)) => {
                if lhs.is_image() && !rhs.is_image() {
                    self.status_msg = Some("Comparison target is not an image (both targets must be images or text)".to_string());
                } else if !lhs.is_image() && rhs.is_image() {
                    self.status_msg = Some("Left target is not an image (both targets must be images or text)".to_string());
                } else {
                    self.diff_viewer = Some(DiffViewerState::new(lhs, rhs, DiffDisplayMode::SideBySide));
                }
            }
            (Some(_), None) => {
                self.status_msg = Some("Diff requires two items. Copy a second item to Right Hand or select 2 slots.".to_string());
            }
            (None, _) => {
                self.status_msg = Some("No item selected or slot is empty. Push or copy content first.".to_string());
            }
        }
    }

    fn open_binary_diff_viewer(&mut self) {
        let selected_items = self.get_selected_items();
        if let Some(((l_lbl, l_bytes), (r_lbl, r_bytes))) =
            resolve_binary_diff_pair(&selected_items, &self.state)
        {
            self.binary_diff_viewer =
                Some(BinaryDiffViewState::new(l_lbl, l_bytes, r_lbl, r_bytes));
        } else {
            self.status_msg = Some(
                "Binary diff requires 2 slots or 1 slot with 2 files/paths.".to_string(),
            );
        }
    }


    fn get_selected_items(&self) -> Vec<&ClipItem> {
        self.selected_slots
            .iter()
            .filter_map(|slot| self.state.get(*slot))
            .collect()
    }

    fn filtered_actions(&self) -> Vec<PaletteAction> {
        let q = self.search_query.to_lowercase();
        let mut actions = Vec::new();

        // 1. Built-in ops
        let items = self.get_selected_items();
        let ops = get_applicable_ops(&items);
        for op in ops {
            if q.is_empty()
                || op.name.to_lowercase().contains(&q)
                || op.label.to_lowercase().contains(&q)
                || op.description.to_lowercase().contains(&q)
                || op.category.to_lowercase().contains(&q)
            {
                actions.push(PaletteAction::Op(op));
            }
        }

        // 2. Discovered Recipes
        for r in &self.recipes {
            if !r.is_applicable(&items) {
                continue;
            }
            if q.is_empty()
                || r.name.to_lowercase().contains(&q)
                || r.description.to_lowercase().contains(&q)
                || r.tags.iter().any(|t| t.to_lowercase().contains(&q))
                || q.starts_with('/')
            {
                actions.push(PaletteAction::Recipe(r.clone()));
            }
        }

        actions
    }

    fn execute_current_action(&mut self, ctx: &egui::Context) {
        let actions = self.filtered_actions();
        if let Some(act) = actions.get(self.selected_action_index) {
            match act {
                PaletteAction::Op(op) => {
                    if op.name == "diff" {
                        self.open_diff_viewer();
                        return;
                    }
                    if op.name == "binary-diff" {
                        self.open_binary_diff_viewer();
                        return;
                    }

                    let req = IpcRequest::ExecuteOp {
                        op_name: op.name.to_string(),
                        secondary_slot: None,
                        selected_slots: Some(self.selected_slots.clone()),
                        separator: None,
                    };
                    if let Ok(resp) = send_ipc_request(&req) {
                        match resp {
                            IpcResponse::OpResult { .. } => {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            IpcResponse::Error { error } => {
                                self.status_msg = Some(format!("Error: {}", error));
                            }
                            _ => {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                    }
                }
                PaletteAction::Recipe(r) => {
                    if let Some((msg, def)) = recipe_needs_prompt(r, &self.state) {
                        self.in_app_prompt = Some(InAppPromptState {
                            recipe_name: r.name.clone(),
                            message: msg,
                            input_text: def,
                            first_frame: true,
                        });
                    } else {
                        let req = IpcRequest::ExecuteRecipe {
                            recipe_name: r.name.clone(),
                        };
                        if let Ok(resp) = send_ipc_request(&req) {
                            match resp {
                                IpcResponse::OpResult { .. } | IpcResponse::Success { .. } => {
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                                IpcResponse::Error { error } => {
                                    self.status_msg = Some(format!("Error: {}", error));
                                }
                                _ => {
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                            }
                        }
                    }
                }
            }
        }
    }


    fn update_mirror_texture(&mut self, ctx: &egui::Context) {
        let mut render_state = self.state.clone();
        render_state.selected = self.selected_slots.clone();

        if let Ok(pixmap) = self.renderer.render_pixmap(&render_state) {
            let pixels: Vec<egui::Color32> = pixmap
                .data()
                .chunks_exact(4)
                .map(|p| egui::Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3]))
                .collect();

            let color_image = egui::ColorImage {
                size: [SCREEN_WIDTH as usize, SCREEN_HEIGHT as usize],
                pixels,
            };

            self.mirror_texture = Some(ctx.load_texture(
                "ambient-mirror",
                color_image,
                egui::TextureOptions::LINEAR,
            ));
            self.dirty_mirror = false;
        }
    }
}

fn recipe_needs_prompt(recipe: &Recipe, state: &SlotManager) -> Option<(String, String)> {
    for node in &recipe.graph.nodes {
        if let NodeKind::PromptOrHandText = node.kind {
            let hand_prop = node.props.get("hand").map(|s| s.as_str()).unwrap_or("right");
            let item = if hand_prop.eq_ignore_ascii_case("left") {
                state.left_hand()
            } else {
                state.right_hand()
            };
            let has_text = item.map_or(false, |it| match &it.payload {
                little_clip_core::ClipPayload::Text(t) | little_clip_core::ClipPayload::Markdown(t) => !t.trim().is_empty(),
                little_clip_core::ClipPayload::Url(_) => true,
                _ => false,
            });
            if !has_text {
                let msg = node.props.get("prompt_message").cloned().unwrap_or_else(|| format!("Enter text for {}:", recipe.name));
                let def = node.props.get("default_text").cloned().unwrap_or_default();
                return Some((msg, def));
            }
        }
    }
    None
}

impl eframe::App for WorkbenchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Apply theme to egui on first frame or theme changes
        if !self.theme_applied {
            apply_theme_to_egui(ctx, &self.renderer.palette);
            self.theme_applied = true;
        }

        // Live state update from background daemon poller
        let mut state_changed = false;
        while let Ok(new_state) = self.state_rx.try_recv() {
            if new_state != self.state {
                self.state = new_state;
                state_changed = true;
            }
        }
        if state_changed {
            self.dirty_mirror = true;
            ctx.request_repaint();
        }

        if self.render_modal_viewers(ctx) {
            return;
        }

        if self.state.has_active_animation() {
            self.dirty_mirror = true;
            ctx.request_repaint_after(Duration::from_millis(80));
        } else {
            ctx.request_repaint_after(Duration::from_millis(120));
        }

        self.handle_global_shortcuts(ctx);

        // Top Header Panel: Mirror of Trofeo Screen + Tabs
        let header_bg = hex_to_c32(&self.renderer.palette.background);
        let brand_color = hex_to_c32(&self.renderer.palette.left_hand_border);
        let sub_color = hex_to_c32(&self.renderer.palette.subtitle_color);

        egui::TopBottomPanel::top("header_panel")
            .frame(
                egui::Frame::none()
                    .fill(header_bg)
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(
                        egui::RichText::new("LITTLE SCREEN CLIP")
                            .color(brand_color)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("— Ambient Dual-Hand Live Mirror")
                            .color(sub_color),
                    );

                    // View Mode Switcher Tabs on the Right
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let is_node = self.view == WorkbenchView::NodeMachine;
                        let node_tab = ui.selectable_label(
                            is_node,
                            egui::RichText::new("★ Big Node Machine [F1]")
                                .strong()
                                .color(if is_node {
                                    egui::Color32::WHITE
                                } else {
                                    hex_to_c32(&self.renderer.palette.right_prior1_border)
                                }),
                        );
                        if node_tab.clicked() {
                            self.view = WorkbenchView::NodeMachine;
                        }

                        let is_pal = self.view == WorkbenchView::Palette;
                        let pal_tab = ui.selectable_label(
                            is_pal,
                            egui::RichText::new("📋 Quick Palette [F2]")
                                .strong()
                                .color(if is_pal {
                                    egui::Color32::WHITE
                                } else {
                                    brand_color
                                }),
                        );
                        if pal_tab.clicked() {
                            self.view = WorkbenchView::Palette;
                        }

                        // Auto-Actions Settings Tab Button
                        let is_auto = self.view == WorkbenchView::AutoActions;
                        let auto_btn = ui.selectable_label(
                            is_auto,
                            egui::RichText::new("⚙ Auto-Actions [F3]")
                                .strong()
                                .color(if is_auto {
                                    egui::Color32::WHITE
                                } else {
                                    hex_to_c32(&self.renderer.palette.url_color)
                                }),
                        );
                        if auto_btn.clicked() {
                            self.view = if is_auto {
                                WorkbenchView::Palette
                            } else {
                                WorkbenchView::AutoActions
                            };
                        }

                        // Themes Settings Tab Button
                        let is_themes = self.view == WorkbenchView::Themes;
                        let themes_btn = ui.selectable_label(
                            is_themes,
                            egui::RichText::new("🎨 Themes [F4]")
                                .strong()
                                .color(if is_themes {
                                    egui::Color32::WHITE
                                } else {
                                    hex_to_c32(&self.renderer.palette.right_hand_border)
                                }),
                        );
                        if themes_btn.clicked() {
                            self.view = if is_themes {
                                WorkbenchView::Palette
                            } else {
                                WorkbenchView::Themes
                            };
                        }
                    });
                });


                ui.add_space(8.0);

                // Re-render mirror if dirty
                if self.dirty_mirror || self.mirror_texture.is_none() {
                    self.update_mirror_texture(ctx);
                }

                let left_priors = self.config.slots.left_priors;
                let get_slot_from_rel_x = |rel_x: f32| -> SlotId {
                    if rel_x < 240.0 {
                        SlotId::Undo
                    } else if rel_x < 600.0 {
                        SlotId::LeftHand
                    } else if rel_x < 840.0 {
                        SlotId::LeftPrior1
                    } else if rel_x < 1080.0 {
                        SlotId::LeftPrior2
                    } else if rel_x < 1320.0 {
                        if left_priors >= 3 {
                            SlotId::LeftPrior3
                        } else {
                            SlotId::RightPrior2
                        }
                    } else if rel_x < 1560.0 {
                        SlotId::RightPrior1
                    } else {
                        SlotId::RightHand
                    }
                };

                // Draw Full-Width Pixel-Perfect Mirror Texture
                if let Some(ref texture) = self.mirror_texture {
                    let avail_w = ui.available_width();
                    let aspect = SCREEN_HEIGHT as f32 / SCREEN_WIDTH as f32; // 462 / 1920 ≈ 0.2406
                    let target_h = avail_w * aspect;

                    let response = ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(egui::vec2(avail_w, target_h))
                            .sense(egui::Sense::click_and_drag()),
                    );

                    // Drag-and-drop slot swapping
                    if response.drag_started() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let rect = response.rect;
                            let rel_x = (pos.x - rect.min.x) / rect.width() * SCREEN_WIDTH as f32;
                            let source = get_slot_from_rel_x(rel_x);
                            if source != SlotId::Undo {
                                self.dragging_slot = Some(source);
                            }
                        }
                    }

                    if response.dragged() {
                        if let Some(source) = self.dragging_slot {
                            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                            egui::show_tooltip_at_pointer(ctx, egui::Id::new("slot_drag_tip"), |ui| {
                                ui.label(format!("Dragging {} (drop on another slot to swap)", source.label()));
                            });
                        }
                    }

                    if response.drag_stopped() {
                        if let Some(source) = self.dragging_slot.take() {
                            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                                let rect = response.rect;
                                if rect.contains(pos) {
                                    let rel_x = (pos.x - rect.min.x) / rect.width() * SCREEN_WIDTH as f32;
                                    let target = get_slot_from_rel_x(rel_x);
                                    if target != source && target != SlotId::Undo {
                                        let _ = send_ipc_request(&IpcRequest::SwapSlots(source, target));
                                        self.state.swap_slots(source, target);
                                        self.dirty_mirror = true;
                                        self.status_msg = Some(format!("Swapped {} and {}", source.label(), target.label()));
                                    }
                                }
                            }
                        }
                    }

                    // Allow clicking directly on the mirror slots to toggle selection
                    if response.clicked() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let rect = response.rect;
                            let rel_x = (pos.x - rect.min.x) / rect.width() * SCREEN_WIDTH as f32;
                            let clicked_slot = get_slot_from_rel_x(rel_x);

                            if clicked_slot != SlotId::Undo {
                                if let Some(p) = self.selected_slots.iter().position(|&s| s == clicked_slot) {
                                    if self.selected_slots.len() > 1 {
                                        self.selected_slots.remove(p);
                                    }
                                } else {
                                    self.selected_slots.push(clicked_slot);
                                }
                                self.dirty_mirror = true;
                                self.selected_action_index = 0;
                            }
                        }
                    } else if response.secondary_clicked() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let rect = response.rect;
                            let rel_x = (pos.x - rect.min.x) / rect.width() * SCREEN_WIDTH as f32;
                            let clicked_slot = get_slot_from_rel_x(rel_x);

                            let item = self.state.get(clicked_slot).cloned();
                            if let Some(item) = item {
                                self.open_slot_preview_modal(&item);
                            }
                        }
                    }
                }
            });

        let mut open_full_editor: Option<(PathBuf, Option<(f32, f32)>)> = None;

        if let Some(strip) = &mut self.rolling_audio_strip {
            egui::TopBottomPanel::bottom("rolling_audio_strip_panel")
                .frame(
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(18, 22, 32))
                        .inner_margin(egui::Margin::symmetric(14.0, 6.0))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 55, 75))),
                )
                .show(ctx, |ui| {
                    strip.render_ui(ui, ctx, &mut open_full_editor);
                });
        }

        if let Some((path, sel)) = open_full_editor {
            let mut editor = AudioPreviewState::new(path, "Rolling Audio Snapshot".to_string()).with_editor(self.config.audio.editor.clone());
            if let Some(region) = sel {
                editor.selection = Some(region);
            }
            self.audio_preview = Some(editor);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(hex_to_c32(&self.renderer.palette.title_color));

            match self.view {
                WorkbenchView::Palette => {
                    let filtered = self.filtered_actions();

                    // Selected slots banner & shortcuts
                    let slot_labels: Vec<&str> = self.selected_slots.iter().map(|s| s.label()).collect();
                    let first_audio_item = self.selected_slots.iter().find_map(|s| {
                        self.state.get(*s).and_then(|it| it.audio_path().map(|p| (p, it.title.clone())))
                    });
                    let first_image_item = self.selected_slots.iter().find_map(|s| {
                        self.state.get(*s).filter(|it| it.is_image())
                    });

                    let mut open_diff_requested = false;

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!("Selected ({}): {}", self.selected_slots.len(), slot_labels.join(" + ")))
                                .color(egui::Color32::from_rgb(168, 85, 247))
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new(" • [Space] Hands  |  [1] L+L1  |  [2] L+L2  |  [3] 3 Left  |  [0] L only")
                                .color(egui::Color32::from_rgb(148, 163, 184))
                                .small(),
                        );

                        if let Some((path, title)) = first_audio_item {
                            ui.add_space(8.0);
                            let audio_btn = egui::Button::new(
                                egui::RichText::new("🎵 Preview & Slice Audio")
                                    .small()
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(16, 185, 129))
                            .rounding(4.0);
                            if ui.add(audio_btn).clicked() {
                                self.audio_preview = Some(AudioPreviewState::new(path, title).with_editor(self.config.audio.editor.clone()));
                            }
                        }

                        if let Some(img_item) = first_image_item {
                            ui.add_space(8.0);
                            let img_btn = egui::Button::new(
                                egui::RichText::new("🖼 Preview & Edit Image")
                                    .small()
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(168, 85, 247))
                            .rounding(4.0);
                            if ui.add(img_btn).clicked() {
                                if let Ok(state) = ImagePreviewState::from_item(img_item, &self.config) {
                                    self.image_preview = Some(state);
                                }
                            }
                        }

                        let has_two_images = if self.selected_slots.len() >= 2 {
                            let img_count = self
                                .selected_slots
                                .iter()
                                .filter_map(|s| self.state.get(*s))
                                .filter(|it| resolve_diff_target(it).map(|t| t.is_image()).unwrap_or(false))
                                .count();
                            img_count >= 2
                                || self.selected_slots.iter().any(|s| {
                                    self.state
                                        .get(*s)
                                        .and_then(|it| resolve_diff_pair(it))
                                        .map(|(t1, t2)| t1.is_image() && t2.is_image())
                                        .unwrap_or(false)
                                })
                        } else if self.selected_slots.len() == 1 {
                            self.state
                                .get(self.selected_slots[0])
                                .and_then(|it| resolve_diff_pair(it))
                                .map(|(t1, t2)| t1.is_image() && t2.is_image())
                                .unwrap_or(false)
                        } else {
                            false
                        };
                        if has_two_images {
                            ui.add_space(8.0);
                            let diff_btn = egui::Button::new(
                                egui::RichText::new("🔍 Compare Images")
                                    .small()
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(234, 88, 12))
                            .rounding(4.0);
                            if ui.add(diff_btn).on_hover_text("Open interactive visual diff comparing the 2 images").clicked() {
                                open_diff_requested = true;
                            }
                        }

                        let first_pdf_item = self.selected_slots.iter().find_map(|s| {
                            self.state.get(*s).and_then(|it| it.file_path()).filter(|p| little_clip_core::pdf::is_pdf_path(p))
                        });
                        let first_spreadsheet_item = self.selected_slots.iter().find_map(|s| {
                            let it = self.state.get(*s)?;
                            if let Some(p) = it.file_path().filter(|p| little_clip_ops::is_spreadsheet_file(p)) {
                                little_clip_ops::load_spreadsheet(&p).ok().map(|data| (data, Some(p)))
                            } else if let Some(txt) = it.to_clipboard_text() {
                                little_clip_ops::detect_csv_text(txt.trim()).map(|data| (data, None))
                            } else {
                                None
                            }
                        });
                        let first_midi_item = self.selected_slots.iter().find_map(|s| {
                            let it = self.state.get(*s)?;
                            if let Some(p) = it.file_path().filter(|p| {
                                p.extension().and_then(|e| e.to_str()).map(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi")).unwrap_or(false)
                            }) {
                                Some(p)
                            } else if let Some(txt) = it.to_clipboard_text() {
                                let p = std::path::Path::new(txt.trim());
                                if p.is_file() && p.extension().and_then(|e| e.to_str()).map(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi")).unwrap_or(false) {
                                    Some(p.to_path_buf())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        });
                        let first_stl_item = self.selected_slots.iter().find_map(|s| {
                            let it = self.state.get(*s)?;
                            if let Some(p) = it.file_path().filter(|p| little_clip_ops::is_stl_file(p)) {
                                Some(p)
                            } else if let Some(txt) = it.to_clipboard_text() {
                                let p = std::path::Path::new(txt.trim());
                                if p.is_file() && little_clip_ops::is_stl_file(p) {
                                    Some(p.to_path_buf())
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        });

                        if let Some(pdf_path) = first_pdf_item {
                            ui.add_space(8.0);
                            let pdf_btn = egui::Button::new(
                                egui::RichText::new("📑 Edit PDF")
                                    .small()
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(220, 38, 38))
                            .rounding(4.0);
                            if ui.add(pdf_btn).on_hover_text("Open PDF Page Editor (Delete / Rotate pages)").clicked() {
                                self.pdf_viewer = Some(PdfEditorState::new(pdf_path));
                            }
                        }

                        if let Some((data, path)) = first_spreadsheet_item {
                            ui.add_space(8.0);
                            let sheet_btn = egui::Button::new(
                                egui::RichText::new("📊 View Spreadsheet")
                                    .small()
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(13, 148, 136))
                            .rounding(4.0);
                            if ui.add(sheet_btn).on_hover_text("Open full multi-sheet X/Y scrollable table").clicked() {
                                self.spreadsheet_viewer = Some(SpreadsheetViewerState::new(data, path));
                            }
                        }

                        if let Some(midi_path) = first_midi_item {
                            ui.add_space(8.0);
                            let midi_btn = egui::Button::new(
                                egui::RichText::new("🎹 MIDI Piano Roll")
                                    .small()
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(37, 99, 235)) // Blue
                            .rounding(4.0);
                            if ui.add(midi_btn).on_hover_text("Open interactive MIDI piano roll visualizer & synth audio").clicked() {
                                if let Ok(st) = MidiViewerState::from_file(midi_path) {
                                    self.midi_viewer = Some(st);
                                }
                            }
                        }

                        if let Some(stl_path) = first_stl_item {
                            ui.add_space(8.0);
                            let stl_btn = egui::Button::new(
                                egui::RichText::new("🧊 3D STL Model")
                                    .small()
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(217, 119, 6)) // Amber
                            .rounding(4.0);
                            if ui.add(stl_btn).on_hover_text("Open interactive 3D orbit viewer (pan, zoom, rotate)").clicked() {
                                if let Ok(st) = StlViewerState::from_file(stl_path) {
                                    self.stl_viewer = Some(st);
                                }
                            }
                        }

                        // Smart text previews: Unit conversions, Network (IP/MAC), and Timestamps
                        let first_text_item = self.selected_slots.iter().find_map(|s| {
                            self.state.get(*s).and_then(|it| it.to_clipboard_text())
                        });
                        if let Some(ref text) = first_text_item {
                            let trimmed = text.trim();
                            if let Some(conv) = little_clip_ops::detect_unit_conversion(trimmed) {
                                ui.add_space(8.0);
                                let label = format!("⚖ {}", conv.summary_line());
                                let btn = egui::Button::new(
                                    egui::RichText::new(label)
                                        .small()
                                        .strong()
                                        .color(egui::Color32::from_rgb(167, 243, 208)),
                                )
                                .fill(egui::Color32::from_rgb(6, 78, 59))
                                .rounding(4.0);
                                if ui.add(btn).on_hover_text("Click to copy converted unit").clicked() {
                                    ui.output_mut(|o| o.copied_text = conv.primary_conversion.clone());
                                    self.status_msg = Some(format!("Copied: {}", conv.primary_conversion));
                                }
                            } else if let Some(net) = little_clip_ops::detect_network_preview(trimmed) {
                                ui.add_space(8.0);
                                let label = match &net {
                                    little_clip_ops::NetworkPreview::Ip { ip, rfc_class, .. } => format!("🌐 {} ({})", ip, rfc_class),
                                    little_clip_ops::NetworkPreview::Mac { formatted, vendor, .. } => format!("🔌 {} ({})", formatted, vendor),
                                };
                                let btn = egui::Button::new(
                                    egui::RichText::new(label)
                                        .small()
                                        .strong()
                                        .color(egui::Color32::from_rgb(186, 230, 253)),
                                )
                                .fill(egui::Color32::from_rgb(12, 74, 110))
                                .rounding(4.0);
                                if ui.add(btn).on_hover_text("Network address info (Click to copy summary)").clicked() {
                                    let lines = net.summary_lines().join("\n");
                                    ui.output_mut(|o| o.copied_text = lines.clone());
                                    self.status_msg = Some("Copied network info".to_string());
                                }
                            } else if let Some(ts) = little_clip_ops::detect_timestamp(trimmed) {
                                ui.add_space(8.0);
                                let label = format!("⏱ {} ({})", ts.human_readable, ts.relative);
                                let btn = egui::Button::new(
                                    egui::RichText::new(label)
                                        .small()
                                        .strong()
                                        .color(egui::Color32::from_rgb(253, 230, 138)),
                                )
                                .fill(egui::Color32::from_rgb(120, 53, 15))
                                .rounding(4.0);
                                if ui.add(btn).on_hover_text(format!("UTC: {}\nEpoch: {}\nClick to copy ISO 8601", ts.utc_iso, ts.epoch_sec)).clicked() {
                                    ui.output_mut(|o| o.copied_text = ts.utc_iso.clone());
                                    self.status_msg = Some(format!("Copied ISO 8601: {}", ts.utc_iso));
                                }
                            }
                        }

                        if let Some((raw_txt, file_p, is_md)) = Self::check_inspector_text_preview(&self.selected_slots, &self.state) {
                            ui.add_space(8.0);
                            let label = if is_md {
                                "📖 Preview Markdown"
                            } else {
                                "📄 View Text / Code"
                            };
                            let txt_btn = egui::Button::new(
                                egui::RichText::new(label)
                                    .small()
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(14, 116, 144))
                            .rounding(4.0);
                            if ui.add(txt_btn).on_hover_text("Open big text preview (syntax highlighting, line wrapping, search, and editor export)").clicked() {
                                if let Some(p) = file_p {
                                    if let Some(st) = TextPreviewState::from_file(p, &self.config) {
                                        self.text_preview = Some(st);
                                    }
                                } else if let Some(txt) = raw_txt {
                                    self.text_preview = Some(TextPreviewState::from_text(txt, None, &self.config));
                                }
                            }
                        }

                    });

                    if open_diff_requested {
                        self.open_diff_viewer();
                    }

                    ui.add_space(6.0);

                    // Search input (autofocused)
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Action > ")
                                .color(egui::Color32::from_rgb(99, 102, 241))
                                .strong(),
                        );
                        let audio_region_active = self.is_audio_region_active();
                        let mut text_edit = egui::TextEdit::singleline(&mut self.search_query)
                            .hint_text("Type action or /recipe to execute...");
                        if audio_region_active {
                            text_edit = text_edit.interactive(false);
                        }
                        let response = ui.add_sized(
                            [ui.available_width() - 20.0, 32.0],
                            text_edit,
                        );
                        if !audio_region_active {
                            response.request_focus();
                        } else {
                            if response.has_focus() {
                                ctx.memory_mut(|m| m.surrender_focus(response.id));
                            }
                            if response.clicked() || (response.hovered() && ctx.input(|i| i.pointer.primary_clicked())) {
                                if let Some(strip) = &mut self.rolling_audio_strip {
                                    strip.selected_region = None;
                                    strip.stop();
                                }
                                response.request_focus();
                            }
                        }
                    });

                    ui.add_space(8.0);

                    // Filtered Action List filling available vertical height
                    let avail_h = (ui.available_height() - 40.0).max(120.0);
                    egui::ScrollArea::vertical().max_height(avail_h).show(ui, |ui| {
                        if filtered.is_empty() {
                            ui.add_space(10.0);
                            ui.label(
                                egui::RichText::new("No matching actions or recipes found.")
                                    .color(egui::Color32::from_rgb(148, 163, 184))
                                    .italics(),
                            );
                        } else {
                            for (idx, act) in filtered.iter().enumerate() {
                                let is_selected = idx == self.selected_action_index;
                                let bg = if is_selected {
                                    egui::Color32::from_rgb(45, 55, 80)
                                } else {
                                    egui::Color32::from_rgb(24, 28, 38)
                                };

                                let frame = egui::Frame::none()
                                    .fill(bg)
                                    .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                                    .rounding(4.0);

                                frame.show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let label_color = if is_selected {
                                            egui::Color32::from_rgb(255, 255, 255)
                                        } else {
                                            egui::Color32::from_rgb(226, 232, 240)
                                        };

                                        ui.label(egui::RichText::new(act.label()).color(label_color).strong());
                                        let cat_color = if act.category() == "Recipe" {
                                            egui::Color32::from_rgb(168, 85, 247)
                                        } else {
                                            egui::Color32::from_rgb(148, 163, 184)
                                        };
                                        ui.label(
                                            egui::RichText::new(format!("[{}]", act.category()))
                                                .color(cat_color)
                                                .small()
                                                .strong(),
                                        );
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(
                                                egui::RichText::new(act.description())
                                                    .color(egui::Color32::from_rgb(148, 163, 184))
                                                    .small(),
                                            );
                                        });
                                    });
                                });
                                ui.add_space(2.0);
                            }
                        }
                    });

                    if let Some(msg) = &self.status_msg {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(msg).color(egui::Color32::from_rgb(239, 68, 68)));
                    }

                    // Footer hints
                    ui.add_space(8.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("[Enter] Execute & Paste  |  [F2 / Tab] Big Node Machine  |  [Esc] Dismiss")
                                .color(egui::Color32::from_rgb(100, 116, 139))
                                .small(),
                        );
                    });
                }
                WorkbenchView::NodeMachine => {
                    let action = self.canvas_state.ui(ui, &self.state);
                    if let Some(CanvasAction::RunPipeline) = action {
                        if let Ok(yaml) = serde_yaml::to_string(&self.canvas_state.graph) {
                            let _ = send_ipc_request(&IpcRequest::ExecuteGraphYaml { yaml });
                            self.dirty_mirror = true;
                            ctx.request_repaint();
                        }
                    }

                    ui.add_space(6.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("[Right/Mid Drag] Pan Canvas  |  [Scroll] Zoom  |  [F2] Quick Palette  |  [Esc] Dismiss")
                                .color(egui::Color32::from_rgb(100, 116, 139))
                                .small(),
                        );
                    });
                }
                WorkbenchView::AutoActions => {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.heading(
                            egui::RichText::new("⚙ Matched Auto-Actions")
                                .color(egui::Color32::from_rgb(56, 189, 248))
                                .strong(),
                        );
                        ui.label(
                            egui::RichText::new("— Actions trigger automatically on clipboard match")
                                .color(egui::Color32::from_rgb(148, 163, 184)),
                        );
                    });
                    ui.add_space(8.0);

                    egui::ScrollArea::vertical().show(ui, |ui| {
                        // Card 1: URL Tracking Cleaner
                        egui::Frame::none()
                            .fill(egui::Color32::from_rgb(24, 28, 38))
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(56, 189, 248)))
                            .rounding(egui::Rounding::same(8.0))
                            .inner_margin(egui::Margin::symmetric(16.0, 14.0))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let mut trim = self.config.auto_actions.trim_link_tracking;
                                    let cb = ui.checkbox(
                                        &mut trim,
                                        egui::RichText::new("Clean Tracking from Links")
                                            .strong()
                                            .size(15.0)
                                            .color(egui::Color32::WHITE),
                                    );
                                    if cb.changed() {
                                        self.config.auto_actions.trim_link_tracking = trim;
                                        let _ = send_ipc_request(&IpcRequest::SetAutoAction {
                                            key: "trim_link_tracking".to_string(),
                                            enabled: trim,
                                        });
                                    }

                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        let (status_text, status_color) = if trim {
                                            ("ENABLED", egui::Color32::from_rgb(34, 197, 94))
                                        } else {
                                            ("DISABLED", egui::Color32::from_rgb(148, 163, 184))
                                        };
                                        egui::Frame::none()
                                            .fill(if trim { egui::Color32::from_rgb(20, 83, 45) } else { egui::Color32::from_rgb(51, 65, 85) })
                                            .rounding(egui::Rounding::same(4.0))
                                            .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                                            .show(ui, |ui| {
                                                ui.label(egui::RichText::new(status_text).size(11.0).strong().color(status_color));
                                            });
                                    });
                                });

                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(
                                        "Automatically strips tracking queries (utm_source, utm_medium, fbclid, gclid, twclid, si, etc.) when any HTTP or HTTPS URL is copied to the clipboard.",
                                    )
                                    .color(egui::Color32::from_rgb(203, 213, 225)),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(
                                        "↳ Behavior: Cleaned link is placed directly into the active hand; original untracked URL is preserved in Undo [Z] for instant bypass.",
                                    )
                                    .small()
                                    .color(egui::Color32::from_rgb(148, 163, 184)),
                                );
                            });

                        ui.add_space(14.0);
                    });

                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("[Esc / F2] Back to Quick Palette  |  [F1] Big Node Machine  |  [F4] Themes")
                                .color(hex_to_c32(&self.renderer.palette.subtitle_color))
                                .small(),
                        );
                    });
                }
                WorkbenchView::Themes => {
                    self.render_themes_view(ui, ctx);
                }
            }
        });
    }
}

impl WorkbenchApp {
    pub fn is_audio_region_active(&self) -> bool {
        self.rolling_audio_strip
            .as_ref()
            .map_or(false, |s| s.selected_region.is_some())
    }

    fn render_modal_viewers(&mut self, ctx: &egui::Context) -> bool {
        self.render_prompt_modal(ctx)
            || self.render_diff_modals(ctx)
            || self.render_audio_image_modals(ctx)
            || self.render_doc_3d_modals(ctx)
    }

    fn render_prompt_modal(&mut self, ctx: &egui::Context) -> bool {
        if let Some(prompt) = &mut self.in_app_prompt {
            let mut close_prompt = false;
            let mut submit_text: Option<String> = None;

            egui::Window::new(format!("✦ Input Required — {}", prompt.recipe_name))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .fixed_size([500.0, 180.0])
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(egui::Color32::from_rgb(20, 24, 34))
                        .stroke(egui::Stroke::new(1.5, egui::Color32::from_rgb(99, 102, 241)))
                        .rounding(8.0)
                        .inner_margin(egui::Margin::symmetric(22.0, 18.0)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("✦")
                                .color(egui::Color32::from_rgb(168, 85, 247))
                                .strong()
                                .size(18.0),
                        );
                        ui.heading(
                            egui::RichText::new(&prompt.recipe_name)
                                .color(egui::Color32::from_rgb(241, 245, 249))
                                .strong(),
                        );
                    });

                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(&prompt.message)
                            .color(egui::Color32::from_rgb(148, 163, 184))
                            .size(13.0),
                    );

                    ui.add_space(10.0);
                    let edit_resp = ui.add(
                        egui::TextEdit::singleline(&mut prompt.input_text)
                            .desired_width(ui.available_width())
                            .hint_text("Type input text here..."),
                    );

                    if prompt.first_frame {
                        edit_resp.request_focus();
                        prompt.first_frame = false;
                    }

                    let enter = edit_resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter));

                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let submit_btn = ui.add(
                            egui::Button::new(
                                egui::RichText::new("Execute & Run [Enter]")
                                    .color(egui::Color32::WHITE)
                                    .strong(),
                            )
                            .fill(egui::Color32::from_rgb(79, 70, 229)),
                        );

                        let cancel_btn = ui.add(
                            egui::Button::new(
                                egui::RichText::new("Cancel [Esc]")
                                    .color(egui::Color32::from_rgb(148, 163, 184)),
                            )
                            .fill(egui::Color32::from_rgb(30, 41, 59)),
                        );

                        if submit_btn.clicked() || enter {
                            submit_text = Some(prompt.input_text.clone());
                        }

                        if cancel_btn.clicked() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close_prompt = true;
                        }
                    });
                });

            if let Some(text) = submit_text {
                let req = IpcRequest::ExecuteRecipeWithPrompt {
                    recipe_name: prompt.recipe_name.clone(),
                    prompt_text: Some(text),
                };
                let _ = send_ipc_request(&req);
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if close_prompt {
                self.in_app_prompt = None;
            }
            return true;
        }
        false
    }

    fn render_diff_modals(&mut self, ctx: &egui::Context) -> bool {
        if let Some(diff_state) = &mut self.diff_viewer {
            let mut close_diff = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    diff_state.render_ui(ui, ctx, &mut close_diff);
                });

            if close_diff {
                self.diff_viewer = None;
            }
            return true;
        }

        if let Some(binary_diff_state) = &mut self.binary_diff_viewer {
            let mut close_binary_diff = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    binary_diff_state.render_ui(ui, ctx, &mut close_binary_diff);
                });

            if close_binary_diff {
                self.binary_diff_viewer = None;
            }
            return true;
        }

        false
    }

    fn render_audio_image_modals(&mut self, ctx: &egui::Context) -> bool {
        if let Some(audio_state) = &mut self.audio_preview {
            let mut close_audio = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    audio_state.render_ui(ui, ctx, &mut close_audio);
                });

            if close_audio {
                audio_state.stop();
                self.audio_preview = None;
            }
            return true;
        }

        if let Some(image_state) = &mut self.image_preview {
            let mut close_image = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    image_state.render_ui(ui, ctx, &mut close_image);
                });

            if close_image {
                self.image_preview = None;
            }
            return true;
        }

        false
    }

    fn render_doc_3d_modals(&mut self, ctx: &egui::Context) -> bool {
        if let Some(sheet_state) = &mut self.spreadsheet_viewer {
            let mut close_sheet = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    sheet_state.render_ui(ui, ctx, &mut close_sheet);
                });

            if close_sheet {
                self.spreadsheet_viewer = None;
            }
            return true;
        }

        if let Some(pdf_state) = &mut self.pdf_viewer {
            let mut close_pdf = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    pdf_state.render_ui(ui, ctx, &mut close_pdf);
                });

            if close_pdf {
                if pdf_state.dirty {
                    self.dirty_mirror = true;
                }
                self.pdf_viewer = None;
            }
            return true;
        }

        if let Some(midi_state) = &mut self.midi_viewer {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                midi_state.stop();
                self.midi_viewer = None;
                return true;
            }
            let mut close_midi = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    midi_state.render_ui(ui, &mut close_midi);
                });

            if close_midi {
                midi_state.stop();
                self.midi_viewer = None;
            }
            return true;
        }

        if let Some(stl_state) = &mut self.stl_viewer {
            let mut close_stl = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    stl_state.render_ui(ui, &mut close_stl);
                });

            if close_stl {
                self.stl_viewer = None;
            }
            return true;
        }

        if let Some(text_state) = &mut self.text_preview {
            let mut close_text = false;
            egui::CentralPanel::default()
                .frame(egui::Frame::none())
                .show(ctx, |ui| {
                    text_state.render_ui(ui, ctx, &mut close_text);
                });

            if close_text {
                self.text_preview = None;
            }
            return true;
        }

        false
    }

    fn open_slot_file_preview(&mut self, fp: &std::path::Path, is_image: bool) {
        if fp.extension().and_then(|s| s.to_str()).map(|e| e.eq_ignore_ascii_case("mid") || e.eq_ignore_ascii_case("midi")).unwrap_or(false) {
            if let Ok(st) = MidiViewerState::from_file(fp.to_path_buf()) {
                self.midi_viewer = Some(st);
            }
        } else if little_clip_ops::is_stl_file(fp) {
            if let Ok(st) = StlViewerState::from_file(fp.to_path_buf()) {
                self.stl_viewer = Some(st);
            }
        } else if little_clip_core::pdf::is_pdf_path(fp) {
            self.pdf_viewer = Some(PdfEditorState::new(fp.to_path_buf()));
        } else if little_clip_ops::is_spreadsheet_file(fp) {
            if let Ok(data) = little_clip_ops::load_spreadsheet(fp) {
                self.spreadsheet_viewer = Some(SpreadsheetViewerState::new(data, Some(fp.to_path_buf())));
            }
        } else if is_image || little_clip_ops::is_image_path(fp) {
            if let Ok(editor) = ImagePreviewState::from_path(fp.to_path_buf(), &self.config) {
                self.image_preview = Some(editor);
            }
        } else if little_clip_ops::is_text_file(fp) {
            if let Some(st) = TextPreviewState::from_file(fp, &self.config) {
                self.text_preview = Some(st);
            }
        }
    }

    fn open_slot_text_preview(&mut self, text: &str, title: &str) {
        let p = std::path::Path::new(text.trim());
        if p.is_file() {
            self.open_slot_file_preview(p, false);
            return;
        }
        if let Some(data) = little_clip_ops::detect_csv_text(text.trim()) {
            self.spreadsheet_viewer = Some(SpreadsheetViewerState::new(data, None));
        } else {
            self.text_preview = Some(TextPreviewState::from_text(text.to_string(), Some(title.to_string()), &self.config));
        }
    }

    fn open_slot_preview_modal(&mut self, item: &ClipItem) {
        if let Some(audio_path) = item.audio_path() {
            self.audio_preview = Some(AudioPreviewState::new(audio_path, item.title.clone()).with_editor(self.config.audio.editor.clone()));
            return;
        }
        if let Some(fp) = item.file_path() {
            self.open_slot_file_preview(&fp, item.is_image());
        } else if item.is_image() {
            if let Ok(editor) = ImagePreviewState::from_item(item, &self.config) {
                self.image_preview = Some(editor);
            }
        } else if let Some(text) = item.to_clipboard_text() {
            self.open_slot_text_preview(&text, &item.title);
        }
    }

    fn check_inspector_text_preview(
        selected_slots: &[SlotId],
        state: &SlotManager,
    ) -> Option<(Option<String>, Option<PathBuf>, bool)> {
        let (raw_txt, file_p): (Option<String>, Option<PathBuf>) = selected_slots.iter().find_map(|s| {
            let it = state.get(*s)?;
            if let Some(p) = it.file_path().filter(|p| little_clip_ops::is_text_file(p)) {
                Some((None, Some(p)))
            } else if let Some(txt) = it.to_clipboard_text() {
                let p = std::path::Path::new(txt.trim());
                if p.is_file() && little_clip_ops::is_text_file(p) {
                    Some((None, Some(p.to_path_buf())))
                } else {
                    Some((Some(txt), None))
                }
            } else {
                None
            }
        })?;

        let is_md = if let Some(p) = &file_p {
            p.extension().map(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown")).unwrap_or(false)
        } else if let Some(txt) = &raw_txt {
            text_preview::detect_markdown_text(txt)
        } else {
            false
        };

        Some((raw_txt, file_p, is_md))
    }

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        let audio_active = self.is_audio_region_active();
        self.handle_escape_shortcut(ctx, audio_active);
        self.handle_audio_strip_shortcuts(ctx, audio_active);
        self.handle_view_switching_shortcuts(ctx);
        self.handle_palette_shortcuts(ctx, audio_active);
    }

    fn handle_escape_shortcut(&mut self, ctx: &egui::Context, audio_region_active: bool) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if audio_region_active {
                if let Some(strip) = &mut self.rolling_audio_strip {
                    strip.selected_region = None;
                    strip.stop();
                }
            } else if self.view == WorkbenchView::AutoActions || self.view == WorkbenchView::Themes {
                self.view = WorkbenchView::Palette;
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    fn handle_audio_strip_shortcuts(&mut self, ctx: &egui::Context, audio_region_active: bool) {
        self.handle_audio_strip_playback(ctx, audio_region_active);
        self.handle_audio_strip_copy(ctx, audio_region_active);
        self.handle_audio_strip_refresh(ctx);
    }

    fn handle_audio_strip_playback(&mut self, ctx: &egui::Context, audio_region_active: bool) {
        if !audio_region_active {
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
            if let Some(strip) = &mut self.rolling_audio_strip {
                strip.toggle_playback();
            }
        }
    }

    fn handle_audio_strip_copy(&mut self, ctx: &egui::Context, audio_region_active: bool) {
        if !audio_region_active {
            return;
        }
        let copy_pressed = ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::C));
        if !copy_pressed {
            return;
        }
        if let Some(strip) = &mut self.rolling_audio_strip {
            if strip.selected_region.is_some() {
                let now = ctx.input(|i| i.time);
                if strip.copy_selection(now) {
                    if let Ok(IpcResponse::State(s)) = send_ipc_request(&IpcRequest::GetState) {
                        self.state = s;
                    }
                    self.dirty_mirror = true;
                }
            }
        }
    }

    fn handle_audio_strip_refresh(&mut self, ctx: &egui::Context) {
        let refresh_pressed = ctx.input(|i| i.key_pressed(egui::Key::F5) || (i.modifiers.command && i.key_pressed(egui::Key::R)));
        if !refresh_pressed {
            return;
        }
        if let Some(strip) = &mut self.rolling_audio_strip {
            strip.load_snapshot();
            let now = ctx.input(|i| i.time);
            let peak = strip.max_amplitude();
            let toast_msg = if peak < 0.001 {
                "⟳ Snapshot refreshed: buffer is currently silence".to_string()
            } else {
                format!("⟳ Snapshot refreshed ({:.1}s, peak: {:.0}%)", strip.duration_sec, peak * 100.0)
            };
            strip.status_toast = Some((toast_msg, now));
        }
    }

    fn handle_view_switching_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.view = WorkbenchView::NodeMachine;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
            self.view = match self.view {
                WorkbenchView::Palette => WorkbenchView::NodeMachine,
                WorkbenchView::NodeMachine | WorkbenchView::AutoActions | WorkbenchView::Themes => WorkbenchView::Palette,
            };
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
            self.view = match self.view {
                WorkbenchView::AutoActions => WorkbenchView::Palette,
                _ => WorkbenchView::AutoActions,
            };
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F4)) {
            self.view = match self.view {
                WorkbenchView::Themes => WorkbenchView::Palette,
                _ => WorkbenchView::Themes,
            };
        }
    }

    fn handle_palette_shortcuts(&mut self, ctx: &egui::Context, audio_region_active: bool) {
        if self.view != WorkbenchView::Palette {
            return;
        }

        if self.search_query.is_empty() {
            self.handle_empty_search_shortcuts(ctx, audio_region_active);
        } else {
            self.handle_search_prefix_shortcuts(audio_region_active);
        }

        self.handle_palette_nav_and_execution(ctx);
    }

    fn handle_empty_search_shortcuts(&mut self, ctx: &egui::Context, audio_region_active: bool) {
        if ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
            self.view = WorkbenchView::NodeMachine;
        } else if ctx.input(|i| i.key_pressed(egui::Key::Space)) && !audio_region_active {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::RightHand]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num1)) {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior1]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num2)) {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior2]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num3)) {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior1, SlotId::LeftPrior2]);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Num0)) {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand]);
        } else if (ctx.input(|i| i.key_pressed(egui::Key::S) || i.key_pressed(egui::Key::X))) && self.selected_slots.len() == 2 {
            self.swap_selected_slots();
        }
    }

    fn apply_quick_slot_selection(&mut self, slots: Vec<SlotId>) {
        self.selected_slots = slots;
        self.dirty_mirror = true;
        self.selected_action_index = 0;
    }

    fn swap_selected_slots(&mut self) {
        let a = self.selected_slots[0];
        let b = self.selected_slots[1];
        let _ = send_ipc_request(&IpcRequest::SwapSlots(a, b));
        self.state.swap_slots(a, b);
        self.dirty_mirror = true;
        self.status_msg = Some(format!("Swapped {} and {}", a.label(), b.label()));
    }

    fn handle_search_prefix_shortcuts(&mut self, audio_region_active: bool) {
        if self.search_query.starts_with(' ') {
            if !audio_region_active {
                self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::RightHand]);
            }
            self.search_query = self.search_query.trim_start().to_string();
        } else if self.search_query == "1" {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior1]);
            self.search_query.clear();
        } else if self.search_query == "2" {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior2]);
            self.search_query.clear();
        } else if self.search_query == "3" {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand, SlotId::LeftPrior1, SlotId::LeftPrior2]);
            self.search_query.clear();
        } else if self.search_query == "0" || self.search_query == "`" {
            self.apply_quick_slot_selection(vec![SlotId::LeftHand]);
            self.search_query.clear();
        } else if (self.search_query.trim() == "swap" || ((self.search_query == "s" || self.search_query == "x") && self.selected_slots.len() == 2)) && self.selected_slots.len() == 2 {
            self.swap_selected_slots();
            self.search_query.clear();
        }
    }

    fn handle_palette_nav_and_execution(&mut self, ctx: &egui::Context) {
        let filtered = self.filtered_actions();
        if !filtered.is_empty() && self.selected_action_index >= filtered.len() {
            self.selected_action_index = 0;
        }

        if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !filtered.is_empty() {
            self.selected_action_index = (self.selected_action_index + 1) % filtered.len();
        }
        if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) && !filtered.is_empty() {
            self.selected_action_index = if self.selected_action_index == 0 {
                filtered.len() - 1
            } else {
                self.selected_action_index - 1
            };
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            self.execute_current_action(ctx);
        }
    }

    fn render_themes_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = self.renderer.palette.clone();
        let active_theme_name = self.config.colors.theme.clone();

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading(
                egui::RichText::new("🎨 Hundred Rabbits Themes")
                    .color(hex_to_c32(&palette.left_hand_border))
                    .strong(),
            );
            ui.label(
                egui::RichText::new(format!("— Active: {}", active_theme_name))
                    .color(hex_to_c32(&palette.title_color))
                    .strong(),
            );

            // Active theme tag
            egui::Frame::none()
                .fill(hex_to_c32(&palette.b_low))
                .stroke(egui::Stroke::new(1.0, hex_to_c32(&palette.left_hand_border)))
                .rounding(egui::Rounding::same(4.0))
                .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(if palette.is_dark { "🌙 Dark" } else { "☀️ Light" })
                            .small()
                            .color(hex_to_c32(&palette.subtitle_color)),
                    );
                });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let reset_btn = egui::Button::new(
                    egui::RichText::new("↺ Reset Overrides")
                        .small()
                        .color(hex_to_c32(&palette.title_color)),
                )
                .fill(hex_to_c32(&palette.undo_card_bg))
                .stroke(egui::Stroke::new(1.0, hex_to_c32(&palette.undo_border)))
                .rounding(4.0);

                if ui
                    .add(reset_btn)
                    .on_hover_text("Clear all manual color overrides in config.toml to restore pure theme colors")
                    .clicked()
                {
                    self.config.colors.clear_overrides();
                    let _ = little_clip_core::config::AppConfig::clear_overrides_in_file();
                    self.renderer.set_config(self.config.clone());
                    apply_theme_to_egui(ctx, &self.renderer.palette);
                    self.dirty_mirror = true;
                    let _ = send_ipc_request(&IpcRequest::ReloadConfig);
                    self.status_msg = Some("Color overrides cleared — pure theme restored".to_string());
                }
            });
        });

        ui.add_space(8.0);

        // Filter and Search Bar
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.theme_search)
                    .hint_text("🔍 Search themes (e.g. apollo, peppermint, nord, paper)...")
                    .desired_width(320.0),
            );

            if !self.theme_search.is_empty() && ui.button("✕").clicked() {
                self.theme_search.clear();
            }

            ui.add_space(12.0);

            // Filter chips
            let all_sel = self.theme_filter == ThemeFilter::All;
            if ui.selectable_label(all_sel, format!("All ({})", self.available_themes.len())).clicked() {
                self.theme_filter = ThemeFilter::All;
            }

            let dark_count = self.available_themes.iter().filter(|t| t.is_dark()).count();
            let dark_sel = self.theme_filter == ThemeFilter::Dark;
            if ui.selectable_label(dark_sel, format!("🌙 Dark ({})", dark_count)).clicked() {
                self.theme_filter = ThemeFilter::Dark;
            }

            let light_count = self.available_themes.len().saturating_sub(dark_count);
            let light_sel = self.theme_filter == ThemeFilter::Light;
            if ui.selectable_label(light_sel, format!("☀️ Light ({})", light_count)).clicked() {
                self.theme_filter = ThemeFilter::Light;
            }
        });

        ui.add_space(10.0);

        let query = self.theme_search.trim().to_lowercase();
        let filtered: Vec<&Theme> = self
            .available_themes
            .iter()
            .filter(|t| {
                if !query.is_empty() && !t.name.to_lowercase().contains(&query) {
                    return false;
                }
                match self.theme_filter {
                    ThemeFilter::All => true,
                    ThemeFilter::Dark => t.is_dark(),
                    ThemeFilter::Light => !t.is_dark(),
                }
            })
            .collect();

        if filtered.is_empty() {
            ui.label(
                egui::RichText::new("No themes match your search criteria.")
                    .color(hex_to_c32(&palette.subtitle_color))
                    .italics(),
            );
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            let row_w = ui.available_width() - 8.0;

            for theme in filtered {
                let is_active = theme.name == active_theme_name;
                let card_id = ui.make_persistent_id(format!("theme_bar_{}", theme.name));

                let card_bg = hex_to_c32(&theme.background);
                let border_color = if is_active {
                    hex_to_c32(&palette.cursor_border)
                } else {
                    hex_to_c32(&theme.b_low)
                };

                let card_stroke = egui::Stroke::new(if is_active { 2.0 } else { 1.0 }, border_color);

                let frame_resp = egui::Frame::none()
                    .fill(card_bg)
                    .stroke(card_stroke)
                    .rounding(egui::Rounding::same(6.0))
                    .inner_margin(egui::Margin::symmetric(14.0, 9.0))
                    .show(ui, |ui| {
                        ui.set_width(row_w);

                        ui.horizontal(|ui| {
                            // Left side: Theme name and Light/Dark tag
                            let title_color = hex_to_c32(&theme.f_high);
                            ui.label(
                                egui::RichText::new(&theme.name)
                                    .strong()
                                    .size(15.0)
                                    .color(title_color),
                            );

                            ui.label(
                                egui::RichText::new(if theme.is_dark() { "🌙 Dark" } else { "☀️ Light" })
                                    .size(11.0)
                                    .color(hex_to_c32(&theme.f_low)),
                            );

                            // Right side: Active badge, harmonies, swatches
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if is_active {
                                    egui::Frame::none()
                                        .fill(hex_to_c32(&palette.b_low))
                                        .stroke(egui::Stroke::new(1.0, hex_to_c32(&palette.cursor_border)))
                                        .rounding(egui::Rounding::same(4.0))
                                        .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                                        .show(ui, |ui| {
                                            ui.label(
                                                egui::RichText::new("✓ ACTIVE")
                                                    .size(11.0)
                                                    .strong()
                                                    .color(hex_to_c32(&palette.cursor_border)),
                                            );
                                        });
                                }

                                ui.add_space(8.0);

                                // 5 Harmony dots
                                let harmonies = DerivedHarmonies::from_base_hex(&theme.f_med, theme.is_dark());
                                let harm_swatches = [
                                    ("Cur", &harmonies.tetradic_2, "Cursor (Tetradic Accent)"),
                                    ("Tri2", &harmonies.triadic_2, "Prior Slot (Triadic 2)"),
                                    ("Tri1", &harmonies.triadic_1, "Prior Slot (Triadic 1)"),
                                    ("RH", &harmonies.complementary, "Right Hand (Complementary)"),
                                    ("LH", &harmonies.base, "Left Hand (Base Accent)"),
                                ];

                                for (_id, hex, label) in harm_swatches {
                                    let c = hex_to_c32(hex);
                                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                                    ui.painter().circle_filled(rect.center(), 4.5, c);
                                    ui.painter().circle_stroke(rect.center(), 4.5, egui::Stroke::new(0.5, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 60)));
                                    resp.on_hover_ui(|ui| {
                                        ui.label(egui::RichText::new(format!("{}: {}", label, hex)).small());
                                    });
                                }

                                ui.add_space(6.0);
                                ui.label(egui::RichText::new("•").size(10.0).color(hex_to_c32(&theme.f_low)));
                                ui.add_space(6.0);

                                // 9 Theme swatches (displayed right-to-left: b_inv down to background)
                                let swatches = [
                                    ("b_inv", &theme.b_inv, "Background Inverted (Badges)"),
                                    ("b_low", &theme.b_low, "Background Low (Undo / Recessed)"),
                                    ("b_med", &theme.b_med, "Background Medium (History Cards)"),
                                    ("b_high", &theme.b_high, "Background High (Hero Cards)"),
                                    ("f_inv", &theme.f_inv, "Foreground Inverted (Overlays)"),
                                    ("f_low", &theme.f_low, "Foreground Low (Muted Subtitles)"),
                                    ("f_med", &theme.f_med, "Foreground Medium (Primary Accent)"),
                                    ("f_high", &theme.f_high, "Foreground High (Headlines)"),
                                    ("background", &theme.background, "Canvas Background"),
                                ];

                                for (_id, hex, label) in swatches {
                                    let c = hex_to_c32(hex);
                                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(15.0, 13.0), egui::Sense::hover());
                                    ui.painter().rect(rect, 2.0, c, egui::Stroke::new(0.5, egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40)));
                                    resp.on_hover_ui(|ui| {
                                        ui.label(egui::RichText::new(format!("{}: {}", label, hex)).small().strong());
                                    });
                                }
                            });
                        });
                    });

                // Make the entire frame clickable
                let mut card_resp = ui.interact(frame_resp.response.rect, card_id, egui::Sense::click());
                if !is_active {
                    card_resp = card_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    if card_resp.hovered() {
                        ui.painter().rect_stroke(
                            frame_resp.response.rect,
                            6.0,
                            egui::Stroke::new(1.5, hex_to_c32(&theme.f_med)),
                        );
                    }
                }

                if card_resp.clicked() && !is_active {
                    self.config.colors.theme = theme.name.clone();
                    let _ = little_clip_core::config::AppConfig::save_theme_to_file(&theme.name);
                    self.renderer.set_config(self.config.clone());
                    apply_theme_to_egui(ctx, &self.renderer.palette);
                    self.dirty_mirror = true;
                    let _ = send_ipc_request(&IpcRequest::ReloadConfig);
                    self.status_msg = Some(format!("Theme '{}' activated", theme.name));
                }

                ui.add_space(5.0);
            }
        });
    }
}


pub(crate) fn send_ipc_request(req: &IpcRequest) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    let sock_path = get_socket_path();
    let mut stream = UnixStream::connect(&sock_path)?;

    let json = serde_json::to_string(req)?;
    stream.write_all(json.as_bytes())?;
    stream.write_all(b"\n")?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let resp: IpcResponse = serde_json::from_str(&line)?;
    Ok(resp)
}

fn get_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("little-screen-clip.sock")
    } else {
        PathBuf::from("/tmp/little-screen-clip.sock")
    }
}

struct PromptApp {
    title: String,
    message: String,
    input_text: String,
    recipe_name: Option<String>,
    first_frame: bool,
}

impl PromptApp {
    pub fn new(title: String, message: String, default_text: String, recipe_name: Option<String>) -> Self {
        Self {
            title,
            message,
            input_text: default_text,
            recipe_name,
            first_frame: true,
        }
    }
}

impl eframe::App for PromptApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(15, 17, 23))
                    .inner_margin(egui::Margin::symmetric(24.0, 20.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("✦")
                            .color(egui::Color32::from_rgb(168, 85, 247))
                            .strong()
                            .size(18.0),
                    );
                    ui.heading(
                        egui::RichText::new(&self.title)
                            .color(egui::Color32::from_rgb(241, 245, 249))
                            .strong(),
                    );
                });

                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(&self.message)
                        .color(egui::Color32::from_rgb(148, 163, 184))
                        .size(13.0),
                );

                ui.add_space(12.0);
                let edit_resp = ui.add(
                    egui::TextEdit::singleline(&mut self.input_text)
                        .font(egui::TextStyle::Body)
                        .desired_width(ui.available_width())
                        .hint_text("Enter input text..."),
                );

                if self.first_frame {
                    edit_resp.request_focus();
                    self.first_frame = false;
                }

                let enter = edit_resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter));

                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let submit_btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new("Execute & Run [Enter]")
                                .color(egui::Color32::WHITE)
                                .strong(),
                        )
                        .fill(egui::Color32::from_rgb(79, 70, 229))
                        .min_size(egui::vec2(140.0, 32.0)),
                    );

                    let cancel_btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new("Cancel [Esc]")
                                .color(egui::Color32::from_rgb(148, 163, 184)),
                        )
                        .fill(egui::Color32::from_rgb(30, 41, 59))
                        .min_size(egui::vec2(80.0, 32.0)),
                    );

                    if submit_btn.clicked() || enter {
                        let text = self.input_text.clone();
                        if let Some(ref r_name) = self.recipe_name {
                            let _ = send_ipc_request(&IpcRequest::ExecuteRecipeWithPrompt {
                                recipe_name: r_name.clone(),
                                prompt_text: Some(text),
                            });
                        } else {
                            let _ = send_ipc_request(&IpcRequest::ExecuteLastAction {
                                prompt_text: Some(text),
                            });
                        }
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }

                    if cancel_btn.clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
    }
}

fn run_prompt_modal(
    title: String,
    message: String,
    default_text: String,
    recipe_name: Option<String>,
) -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([480.0, 180.0])
            .with_resizable(false)
            .with_decorations(true)
            .with_app_id("little-clip-prompt")
            .with_window_type(egui::viewport::X11WindowType::Dialog)
            .with_title(format!("Little Screen Clip — {}", title)),
        ..Default::default()
    };

    eframe::run_native(
        "clip-prompt",
        options,
        Box::new(move |_cc| Box::new(PromptApp::new(title, message, default_text, recipe_name))),
    )
}

fn main() -> Result<(), eframe::Error> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("Little Screen Clip — Crafting Workbench");
        println!("\nUsage: clip-workbench [OPTIONS]");
        println!("\nOptions:");
        println!("  -d, --diff           Open diff comparison viewer directly");
        println!("  -b, --binary         Open interactive binary diff viewer directly");
        println!("  -a, --audio <PATH>   Open audio waveform preview & slicer directly");
        println!("  -i, --image <PATH>   Open image preview & editor directly");
        println!("  -l, --last           Re-execute last action (with modal prompt if needed)");
        println!("  -h, --help           Print help");
        return Ok(());
    }

    let is_last = args.iter().any(|a| a == "--last" || a == "-l");
    let is_diff = args.iter().any(|a| a == "--diff" || a == "-d");
    let is_binary_diff = args.iter().any(|a| a == "--binary-diff" || a == "--binary" || a == "-b");
    let initial_audio = args
        .iter()
        .position(|a| a == "--audio" || a == "-a")
        .and_then(|idx| args.get(idx + 1))
        .map(PathBuf::from);
    let initial_image = args
        .iter()
        .position(|a| a == "--image" || a == "-i")
        .and_then(|idx| args.get(idx + 1))
        .map(PathBuf::from);

    if is_last {
        let state = match send_ipc_request(&IpcRequest::GetState) {
            Ok(IpcResponse::State(s)) => s,
            _ => SlotManager::default(),
        };

        if let Ok(IpcResponse::LastAction(Some(action))) = send_ipc_request(&IpcRequest::GetLastAction) {
            match action {
                little_clip_core::ipc::LastAction::Recipe { recipe_name } => {
                    if let Some(recipe) = find_recipe_by_name(&recipe_name) {
                        if let Some((msg, def)) = recipe_needs_prompt(&recipe, &state) {
                            return run_prompt_modal(
                                recipe.name.clone(),
                                msg,
                                def,
                                Some(recipe.name),
                            );
                        }
                    }
                    // No prompt needed, execute immediately
                    let _ = send_ipc_request(&IpcRequest::ExecuteLastAction { prompt_text: None });
                    return Ok(());
                }
                little_clip_core::ipc::LastAction::Op { .. } => {
                    let _ = send_ipc_request(&IpcRequest::ExecuteLastAction { prompt_text: None });
                    return Ok(());
                }
            }
        } else {
            eprintln!("No last action available to run.");
            return Ok(());
        }
    }

    let state = match send_ipc_request(&IpcRequest::GetState) {
        Ok(IpcResponse::State(s)) => s,
        _ => SlotManager::default(),
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 680.0])
            .with_resizable(false)
            .with_decorations(true)
            .with_app_id("little-clip-workbench")
            .with_window_type(egui::viewport::X11WindowType::Dialog)
            .with_title("Little Screen Clip — Crafting Workbench"),
        ..Default::default()
    };

    eframe::run_native(
        "clip-workbench",
        options,
        Box::new(move |_cc| Box::new(WorkbenchApp::new(state, is_diff, is_binary_diff, initial_audio, initial_image))),
    )
}

