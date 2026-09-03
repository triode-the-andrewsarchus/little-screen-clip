use eframe::egui;
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use little_clip_core::{AppConfig, ClipItem, SlotId, SlotManager};
use little_clip_ops::pipeline::recipes::load_all_recipes;
use little_clip_ops::pipeline::types::Recipe;
use little_clip_ops::{get_applicable_ops, OpDescriptor};
use little_clip_render::{ScreenRenderer, SCREEN_HEIGHT, SCREEN_WIDTH};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

mod canvas;
use canvas::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkbenchView {
    Palette,
    NodeMachine,
}

#[derive(Clone)]
enum PaletteAction {
    Op(OpDescriptor),
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
}

impl WorkbenchApp {
    fn new(state: SlotManager) -> Self {
        let mut config = AppConfig::load();
        // Desktop monitor mirror is always upright (no 180 flip)
        config.display.rotate_180 = false;
        let renderer = ScreenRenderer::new(config.clone())
            .unwrap_or_else(|_| ScreenRenderer::fallback(config));

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

        Self {
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
            let req = match act {
                PaletteAction::Op(op) => IpcRequest::ExecuteOp {
                    op_name: op.name.to_string(),
                    secondary_slot: None,
                    selected_slots: Some(self.selected_slots.clone()),
                    separator: None,
                },
                PaletteAction::Recipe(r) => IpcRequest::ExecuteRecipe {
                    recipe_name: r.name.clone(),
                },
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

impl eframe::App for WorkbenchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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

        if self.state.has_active_animation() {
            self.dirty_mirror = true;
            ctx.request_repaint_after(Duration::from_millis(80));
        } else {
            ctx.request_repaint_after(Duration::from_millis(120));
        }

        // Global keybindings
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // F2 or Tab toggles view between Palette and Node Machine
        if ctx.input(|i| i.key_pressed(egui::Key::F2)) {
            self.view = match self.view {
                WorkbenchView::Palette => WorkbenchView::NodeMachine,
                WorkbenchView::NodeMachine => WorkbenchView::Palette,
            };
        }

        // Quick slot shortcuts when in Palette view and search is empty
        if self.view == WorkbenchView::Palette {
            if self.search_query.is_empty() {
                if ctx.input(|i| i.key_pressed(egui::Key::Tab)) {
                    self.view = WorkbenchView::NodeMachine;
                } else if ctx.input(|i| i.key_pressed(egui::Key::Space)) {
                    self.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
                    self.dirty_mirror = true;
                    self.selected_action_index = 0;
                } else if ctx.input(|i| i.key_pressed(egui::Key::Num1)) {
                    self.selected_slots = vec![SlotId::LeftHand, SlotId::LeftPrior1];
                    self.dirty_mirror = true;
                    self.selected_action_index = 0;
                } else if ctx.input(|i| i.key_pressed(egui::Key::Num2)) {
                    self.selected_slots = vec![SlotId::LeftHand, SlotId::LeftPrior2];
                    self.dirty_mirror = true;
                    self.selected_action_index = 0;
                } else if ctx.input(|i| i.key_pressed(egui::Key::Num3)) {
                    self.selected_slots = vec![SlotId::LeftHand, SlotId::LeftPrior1, SlotId::LeftPrior2];
                    self.dirty_mirror = true;
                    self.selected_action_index = 0;
                } else if ctx.input(|i| i.key_pressed(egui::Key::Num0)) {
                    self.selected_slots = vec![SlotId::LeftHand];
                    self.dirty_mirror = true;
                    self.selected_action_index = 0;
                }
            }

            // Intercept first-character shortcuts from search input
            if self.search_query.starts_with(' ') {
                self.selected_slots = vec![SlotId::LeftHand, SlotId::RightHand];
                self.search_query = self.search_query.trim_start().to_string();
                self.dirty_mirror = true;
                self.selected_action_index = 0;
            } else if self.search_query == "1" {
                self.selected_slots = vec![SlotId::LeftHand, SlotId::LeftPrior1];
                self.search_query.clear();
                self.dirty_mirror = true;
                self.selected_action_index = 0;
            } else if self.search_query == "2" {
                self.selected_slots = vec![SlotId::LeftHand, SlotId::LeftPrior2];
                self.search_query.clear();
                self.dirty_mirror = true;
                self.selected_action_index = 0;
            } else if self.search_query == "3" {
                self.selected_slots = vec![SlotId::LeftHand, SlotId::LeftPrior1, SlotId::LeftPrior2];
                self.search_query.clear();
                self.dirty_mirror = true;
                self.selected_action_index = 0;
            } else if self.search_query == "0" || self.search_query == "`" {
                self.selected_slots = vec![SlotId::LeftHand];
                self.search_query.clear();
                self.dirty_mirror = true;
                self.selected_action_index = 0;
            }

            let filtered = self.filtered_actions();
            if !filtered.is_empty() && self.selected_action_index >= filtered.len() {
                self.selected_action_index = 0;
            }

            // Arrow navigation
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) && !filtered.is_empty() {
                self.selected_action_index = (self.selected_action_index + 1) % filtered.len();
            }
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) && !filtered.is_empty() {
                if self.selected_action_index == 0 {
                    self.selected_action_index = filtered.len() - 1;
                } else {
                    self.selected_action_index -= 1;
                }
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.execute_current_action(ctx);
            }
        }

        // Top Panel: Title Row + View Tabs + Live Ambient Mirror
        egui::TopBottomPanel::top("top_mirror_panel")
            .frame(
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(15, 17, 23))
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading(
                        egui::RichText::new("LITTLE SCREEN CLIP")
                            .color(egui::Color32::from_rgb(99, 102, 241))
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("— Ambient Dual-Hand Live Mirror")
                            .color(egui::Color32::from_rgb(148, 163, 184)),
                    );

                    // View Mode Switcher Tabs on the Right
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let is_node = self.view == WorkbenchView::NodeMachine;
                        let node_tab = ui.selectable_label(
                            is_node,
                            egui::RichText::new("★ Big Node Machine [F2]")
                                .strong()
                                .color(if is_node {
                                    egui::Color32::WHITE
                                } else {
                                    egui::Color32::from_rgb(168, 85, 247)
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
                                    egui::Color32::from_rgb(99, 102, 241)
                                }),
                        );
                        if pal_tab.clicked() {
                            self.view = WorkbenchView::Palette;
                        }
                    });
                });

                ui.add_space(8.0);

                // Re-render mirror if dirty
                if self.dirty_mirror || self.mirror_texture.is_none() {
                    self.update_mirror_texture(ctx);
                }

                // Draw Full-Width Pixel-Perfect Mirror Texture
                if let Some(ref texture) = self.mirror_texture {
                    let avail_w = ui.available_width();
                    let aspect = SCREEN_HEIGHT as f32 / SCREEN_WIDTH as f32; // 462 / 1920 ≈ 0.2406
                    let target_h = avail_w * aspect;

                    let response = ui.add(
                        egui::Image::new(texture)
                            .fit_to_exact_size(egui::vec2(avail_w, target_h))
                            .sense(egui::Sense::click()),
                    );

                    // Allow clicking directly on the mirror slots to toggle selection
                    if response.clicked() {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let rect = response.rect;
                            let rel_x = (pos.x - rect.min.x) / rect.width() * SCREEN_WIDTH as f32;
                            let clicked_slot = if rel_x < 240.0 {
                                SlotId::Undo
                            } else if rel_x < 600.0 {
                                SlotId::LeftHand
                            } else if rel_x < 840.0 {
                                SlotId::LeftPrior1
                            } else if rel_x < 1080.0 {
                                SlotId::LeftPrior2
                            } else if rel_x < 1320.0 {
                                SlotId::RightPrior2
                            } else if rel_x < 1560.0 {
                                SlotId::RightPrior1
                            } else {
                                SlotId::RightHand
                            };

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
                    }
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.visuals_mut().override_text_color = Some(egui::Color32::from_rgb(241, 245, 249));

            match self.view {
                WorkbenchView::Palette => {
                    let filtered = self.filtered_actions();

                    // Selected slots banner & shortcuts
                    let slot_labels: Vec<&str> = self.selected_slots.iter().map(|s| s.label()).collect();
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
                    });

                    ui.add_space(6.0);

                    // Search input (autofocused)
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Action > ")
                                .color(egui::Color32::from_rgb(99, 102, 241))
                                .strong(),
                        );
                        let response = ui.add_sized(
                            [ui.available_width() - 20.0, 32.0],
                            egui::TextEdit::singleline(&mut self.search_query)
                                .hint_text("Type action or /recipe to execute..."),
                        );
                        response.request_focus();
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
            }
        });
    }
}

fn send_ipc_request(req: &IpcRequest) -> Result<IpcResponse, Box<dyn std::error::Error>> {
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

fn main() -> Result<(), eframe::Error> {
    let state = match send_ipc_request(&IpcRequest::GetState) {
        Ok(IpcResponse::State(s)) => s,
        _ => SlotManager::default(),
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1240.0, 740.0])
            .with_resizable(true)
            .with_decorations(true)
            .with_app_id("little-clip-workbench")
            .with_window_type(egui::viewport::X11WindowType::Dialog)
            .with_title("Little Screen Clip — Crafting Workbench & Node Machine"),
        ..Default::default()
    };

    eframe::run_native(
        "clip-workbench",
        options,
        Box::new(|_cc| Box::new(WorkbenchApp::new(state))),
    )
}
