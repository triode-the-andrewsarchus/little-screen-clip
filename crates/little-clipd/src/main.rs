//! Little Screen Clip background daemon (`little-clipd`).
//!
//! Manages active clipboard items, background audio recording, physical display updates,
//! Wayland clipboard monitoring (`wl-paste`), and IPC communication with `clipctl` and `clip-workbench`.

use little_clip_core::{
    ClipItem, HandSide, SlotId, SlotManager, copy_bytes_to_system_clipboard,
    copy_text_to_system_clipboard, file_uri_list,
    ipc::{IpcRequest, IpcResponse},
    rolling_audio::RollingAudioBuffer,
};
use little_clip_driver::TrofeoDriver;
use little_clip_render::ScreenRenderer;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, watch};
use tracing::{error, info, warn};

mod clipboard_route;
mod display_worker;
mod lock_monitor;
mod pipeline;
mod rolling_capture;

const MAX_IPC_REQUEST_BYTES: usize = 1024 * 1024;

async fn read_bounded_ipc_line<R>(
    reader: &mut R,
    request: &mut Vec<u8>,
) -> std::io::Result<Option<usize>>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    request.clear();
    loop {
        let (take, found_newline) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                return Ok((!request.is_empty()).then_some(request.len()));
            }
            let take = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |position| position + 1);
            if request.len() + take > MAX_IPC_REQUEST_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "IPC request exceeds byte limit",
                ));
            }
            request.extend_from_slice(&available[..take]);
            (take, take < available.len() || available[take - 1] == b'\n')
        };
        reader.consume(take);
        if found_newline {
            return Ok(Some(request.len()));
        }
    }
}

/// Shared runtime state across daemon IPC handlers, background workers, and display pipelines.
pub struct DaemonContext {
    /// Active application configuration.
    pub config: RwLock<little_clip_core::AppConfig>,
    /// Multi-slot clipboard inventory state.
    pub manager: RwLock<SlotManager>,
    /// Frame rasterizer for secondary screen hardware.
    pub renderer: RwLock<ScreenRenderer>,
    /// Frame sender channel connected to the USB display worker.
    pub display_tx: watch::Sender<Option<Arc<[u8]>>>,
    /// Circular buffer of recently self-copied text strings to ignore in clipboard watcher.
    pub self_copied_text: RwLock<Vec<String>>,
    /// Hash of recently self-copied image to avoid echo loops.
    pub self_copied_image_hash: RwLock<Option<u64>>,
    /// Most recently executed IPC action for repeat / replay commands.
    pub last_action: RwLock<Option<little_clip_core::ipc::LastAction>>,
    /// Flag indicating whether the screen is blanked due to session lock or sleep.
    pub is_locked: AtomicBool,
    /// Continuous PCM audio circular buffer.
    pub rolling_audio: Arc<std::sync::RwLock<RollingAudioBuffer>>,
    /// Audio capture settings shared with the rolling capture worker.
    pub rolling_capture_config: Arc<std::sync::RwLock<little_clip_core::config::AudioConfig>>,
    /// Temporary routing override for the next incoming system clipboard copy.
    route_next_clipboard: RwLock<clipboard_route::ClipboardRoute>,
}

impl DaemonContext {
    /// Records text copied to OS clipboard by this daemon to prevent echo loop re-ingestion.
    pub async fn add_self_copied_text(&self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let mut guard = self.self_copied_text.write().await;
        if guard.len() >= 20 {
            guard.remove(0);
        }
        guard.push(text);
    }

    /// Checks if the given text matches a recently self-copied string.
    pub async fn is_self_copied_text(&self, text: &str) -> bool {
        let guard = self.self_copied_text.read().await;
        let t_norm = text.trim().replace("\r\n", "\n");
        for st in guard.iter() {
            if st == text || st.trim() == text.trim() || st.trim().replace("\r\n", "\n") == t_norm {
                return true;
            }
        }
        false
    }

    /// Returns `true` if the display is currently blanked due to system lock or sleep.
    pub async fn is_locked(&self) -> bool {
        self.is_locked.load(Ordering::SeqCst)
    }

    /// Updates the system locked state and triggers screen blanking or refresh.
    pub async fn set_locked(&self, locked: bool) {
        let old = self.is_locked.swap(locked, Ordering::SeqCst);
        if old == locked {
            return;
        }

        if locked {
            info!("Lock state engaged -> Blanking hardware display");
            self.render_and_send_blank().await;
        } else {
            info!("Lock state cleared -> Restoring display contents");
            self.refresh_display().await;
        }
    }

    /// Renders a blank screen frame and dispatches it to the physical display.
    pub async fn render_and_send_blank(&self) {
        let frame = {
            let renderer = self.renderer.read().await;
            match renderer.render_blank_frame() {
                Ok(f) => f,
                Err(e) => {
                    error!("Render blank error: {}", e);
                    return;
                }
            }
        };
        let _ = self.display_tx.send(Some(frame.into()));
    }

    /// Rerenders the current slot inventory layout and sends the new frame to the display.
    pub async fn refresh_display(&self) {
        if self.is_locked.load(Ordering::SeqCst) {
            self.render_and_send_blank().await;
            return;
        }

        let frame = {
            let mgr = self.manager.read().await;
            let renderer = self.renderer.read().await;
            match renderer.render_frame(&mgr) {
                Ok(f) => f,
                Err(e) => {
                    error!("Render error: {}", e);
                    return;
                }
            }
        };

        // Non-blocking send to display worker
        let _ = self.display_tx.send(Some(frame.into()));
    }

    /// Consumes and returns any active routing override (hand destination and clipboard restore flag).
    pub async fn take_route_override(&self) -> (little_clip_core::HandSide, bool) {
        self.route_next_clipboard
            .write()
            .await
            .take(tokio::time::Instant::now())
    }

    /// Restores the previously active OS clipboard item if a route-with-restore operation was performed.
    pub fn maybe_restore_os_clipboard_after_route(
        self: &Arc<Self>,
        restore_os: bool,
        target_hand: little_clip_core::HandSide,
        prior_left: Option<ClipItem>,
    ) {
        if restore_os && target_hand == little_clip_core::HandSide::Right {
            if let Some(left_item) = prior_left {
                let restore_ctx = Arc::clone(self);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(80)).await;
                    copy_to_os_clipboard(&left_item, &restore_ctx).await;
                });
            }
        }
    }
}

fn get_last_action_path() -> PathBuf {
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(config_home)
            .join("little-screen-clip")
            .join("last_action.json")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".config")
            .join("little-screen-clip")
            .join("last_action.json")
    } else {
        PathBuf::from("/tmp/little-clip-last-action.json")
    }
}

async fn save_last_action(action: &little_clip_core::ipc::LastAction, ctx: &DaemonContext) {
    let mut lock = ctx.last_action.write().await;
    *lock = Some(action.clone());
    let path = get_last_action_path();
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            warn!(
                "Failed to create directory for last action state at {}: {err}",
                parent.display()
            );
            return;
        }
    }
    match serde_json::to_string_pretty(action) {
        Ok(json) => {
            if let Err(err) = std::fs::write(&path, json) {
                warn!("Failed to persist last action to {}: {err}", path.display());
            }
        }
        Err(err) => warn!("Failed to serialize last action state: {err}"),
    }
}

fn load_last_action_from_disk() -> Option<little_clip_core::ipc::LastAction> {
    let path = get_last_action_path();
    if let Ok(data) = std::fs::read_to_string(path) {
        serde_json::from_str(&data).ok()
    } else {
        None
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("Starting Little Screen Clip Daemon (little-clipd)...");

    let config = little_clip_core::config::AppConfig::load();
    let renderer = ScreenRenderer::new(config.clone()).unwrap_or_else(|e| {
        warn!("Font loading issue: {}. Continuing with fallback...", e);
        ScreenRenderer::fallback(config.clone())
    });

    let (display_tx, display_rx) = watch::channel(None);

    // Hardware enumeration, writes, recovery and release share one dedicated thread.
    // Frame publication never waits for USB; only the newest frame is retained.
    let _display_worker =
        display_worker::spawn(display_rx, TrofeoDriver::open, |driver, frame| {
            driver.check_alive()?;
            driver.send_frame(frame)
        })?;

    let rolling_audio = Arc::new(std::sync::RwLock::new(RollingAudioBuffer::new(
        config.audio.rolling_capture_duration_sec,
        48000,
        2,
    )));
    let rolling_capture_config = Arc::new(std::sync::RwLock::new(config.audio.clone()));

    let ctx = Arc::new(DaemonContext {
        config: RwLock::new(config.clone()),
        manager: RwLock::new(SlotManager::default()),
        renderer: RwLock::new(renderer),
        display_tx,
        self_copied_text: RwLock::new(Vec::new()),
        self_copied_image_hash: RwLock::new(None),
        last_action: RwLock::new(load_last_action_from_disk()),
        is_locked: AtomicBool::new(false),
        rolling_audio: Arc::clone(&rolling_audio),
        rolling_capture_config: Arc::clone(&rolling_capture_config),
        route_next_clipboard: RwLock::new(clipboard_route::ClipboardRoute::default()),
    });

    // Start background rolling audio capture thread
    let capture_audio = Arc::clone(&rolling_audio);
    rolling_capture::start_rolling_capture_thread(capture_audio, rolling_capture_config);

    // Push initial blank state to screen
    ctx.refresh_display().await;

    // Spawn background lock and sleep monitor
    let lock_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        lock_monitor::run_lock_monitor(lock_ctx).await;
    });

    let clipboard_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        run_clipboard_poller(clipboard_ctx).await;
    });

    // Start animation ticker loop (paced to protect embedded MCU from hardware watchdog/overrun)
    let anim_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        loop {
            if anim_ctx.is_locked.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
            let (interval_ms, has_anim) = {
                let r = anim_ctx.renderer.read().await;
                let mgr = anim_ctx.manager.read().await;
                (
                    r.config.display.animation_interval_ms.max(50),
                    mgr.has_active_animation(),
                )
            };
            if has_anim {
                anim_ctx.refresh_display().await;
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            } else {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    });

    // Bind UNIX socket
    let sock_path = get_socket_path();
    let _ = std::fs::remove_file(&sock_path);
    if let Some(parent) = sock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let listener = UnixListener::bind(&sock_path)?;
    info!("IPC server listening on {}", sock_path.display());

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let client_ctx = Arc::clone(&ctx);
                tokio::spawn(async move {
                    handle_client(stream, client_ctx).await;
                });
            }
            Err(e) => {
                error!("Socket accept error: {}", e);
            }
        }
    }
}

fn evaluate_op(
    mgr: &SlotManager,
    op_name: &str,
    selected_slots: Option<&[SlotId]>,
    secondary_slot: Option<SlotId>,
    separator: Option<&str>,
) -> Result<ClipItem, little_clip_ops::OpError> {
    if let Some(slots) = selected_slots {
        let items: Vec<&ClipItem> = slots.iter().filter_map(|s| mgr.get(*s)).collect();
        if items.is_empty() {
            Err(little_clip_ops::OpError::ExecutionFailed(
                "Selected slots are empty".to_string(),
            ))
        } else {
            little_clip_ops::execute_multi_op(op_name, &items, separator)
        }
    } else {
        let left_item = mgr.left_hand().cloned();
        if let Some(left) = left_item {
            if let Some(sec_slot) = secondary_slot {
                if let Some(sec_item) = mgr.get(sec_slot) {
                    little_clip_ops::execute_binary_op(op_name, &left, sec_item, separator)
                } else {
                    Err(little_clip_ops::OpError::ExecutionFailed(format!(
                        "Slot {} is empty",
                        sec_slot.label()
                    )))
                }
            } else if let Some(right) = mgr.right_hand() {
                if op_name == "binary-diff"
                    && little_clip_ops::binary_diff::resolve_two_files_from_single_item(&left)
                        .is_some()
                {
                    little_clip_ops::execute_unary_op(op_name, &left)
                } else {
                    // Try binary with RightHand as natural default operand, or fallback to unary
                    little_clip_ops::execute_binary_op(op_name, &left, right, separator)
                        .or_else(|_| little_clip_ops::execute_unary_op(op_name, &left))
                }
            } else {
                little_clip_ops::execute_unary_op(op_name, &left)
            }
        } else {
            Err(little_clip_ops::OpError::ExecutionFailed(
                "Left Hand is empty".to_string(),
            ))
        }
    }
}

async fn apply_op_result_and_respond(new_item: ClipItem, ctx: &Arc<DaemonContext>) -> IpcResponse {
    schedule_async_previews(&new_item, Arc::clone(ctx));
    let text_opt = extract_clipboard_text(&new_item);
    if let Some(ref t) = text_opt {
        ctx.add_self_copied_text(t.clone()).await;
    }
    copy_to_os_clipboard(&new_item, ctx).await;
    {
        let mut mgr = ctx.manager.write().await;
        mgr.apply_op_result(new_item.clone());
    }

    ctx.refresh_display().await;
    IpcResponse::OpResult { item: new_item }
}

async fn handle_execute_op(
    op_name: String,
    selected_slots: Option<Vec<SlotId>>,
    secondary_slot: Option<SlotId>,
    separator: Option<String>,
    record_action: bool,
    ctx: &Arc<DaemonContext>,
) -> IpcResponse {
    let result = {
        let mgr = ctx.manager.read().await;
        evaluate_op(
            &mgr,
            &op_name,
            selected_slots.as_deref(),
            secondary_slot,
            separator.as_deref(),
        )
    };

    match result {
        Ok(new_item) => {
            if record_action {
                save_last_action(
                    &little_clip_core::ipc::LastAction::Op {
                        op_name,
                        selected_slots,
                        secondary_slot,
                        separator,
                    },
                    ctx,
                )
                .await;
            }
            apply_op_result_and_respond(new_item, ctx).await
        }
        Err(e) => IpcResponse::Error {
            error: e.to_string(),
        },
    }
}

async fn handle_execute_recipe(
    recipe_name: String,
    prompt_text: Option<String>,
    record_action: bool,
    ctx: &Arc<DaemonContext>,
) -> IpcResponse {
    let mgr_snapshot = { ctx.manager.read().await.clone() };
    let requested_name = recipe_name.clone();
    let result = pipeline::execute(move || {
        let recipe = little_clip_ops::find_recipe_by_name(&requested_name)
            .ok_or_else(|| format!("Recipe not found: {requested_name}"))?;
        little_clip_ops::PipelineExecutor::new(&recipe.graph, &mgr_snapshot)
            .with_single_prompt(prompt_text)
            .execute()
            .map_err(|error| format!("Recipe execution failed: {error}"))
    })
    .await;
    match result {
        Ok(pipe_res) => {
            if record_action {
                save_last_action(
                    &little_clip_core::ipc::LastAction::Recipe { recipe_name },
                    ctx,
                )
                .await;
            }
            apply_pipeline_result(pipe_res, ctx).await
        }
        Err(error) => IpcResponse::Error { error },
    }
}

fn prepare_push_text(text: String, trim_tracking: bool) -> (ClipItem, Option<ClipItem>) {
    if trim_tracking {
        if let Some(cleaned) = little_clip_ops::clean_url_tracking(&text) {
            (
                ClipItem::from_text(cleaned),
                Some(ClipItem::from_text(text)),
            )
        } else {
            (ClipItem::from_text(text), None)
        }
    } else {
        (ClipItem::from_text(text), None)
    }
}

fn apply_prepared_push_text(
    manager: &mut SlotManager,
    item: ClipItem,
    original: Option<ClipItem>,
    hand: HandSide,
) {
    if let Some(original) = original {
        manager.undo = Some(original);
    }
    manager.push_to_hand(item, hand);
}

// Reference collections must stay references, even for one image or media file.
fn build_file_item(paths: Vec<PathBuf>, references_only: bool) -> ClipItem {
    if !references_only && paths.len() == 1 {
        ClipItem::from_single_file(&paths[0])
    } else {
        ClipItem::from_files(paths)
    }
}

async fn handle_client(stream: UnixStream, ctx: Arc<DaemonContext>) {
    info!("Client connected to IPC socket");
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut request_bytes = Vec::new();

    loop {
        match read_bounded_ipc_line(&mut reader, &mut request_bytes).await {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::InvalidData {
                    let response = IpcResponse::Error {
                        error: format!("IPC request exceeds {} byte limit", MAX_IPC_REQUEST_BYTES),
                    };
                    if let Ok(json) = serde_json::to_string(&response) {
                        let _ = writer.write_all(format!("{json}\n").as_bytes()).await;
                    }
                }
                warn!("Failed to read IPC request: {e}");
                break;
            }
        }
        info!("IPC request received ({} bytes)", request_bytes.len());
        let req: Result<IpcRequest, _> = serde_json::from_slice(&request_bytes);
        let references_only = matches!(&req, Ok(IpcRequest::PushFileReferences { .. }));
        let resp = match req {
            Ok(IpcRequest::GetState) => {
                info!("Processing GetState...");
                let mgr = ctx.manager.read().await;
                info!("Got manager read lock for GetState");
                IpcResponse::State(mgr.clone())
            }
            Ok(IpcRequest::GetStateSummary) => {
                let mgr = ctx.manager.read().await;
                IpcResponse::StateSummary(little_clip_core::ipc::SlotStateSummary::from_manager(
                    &mgr,
                ))
            }
            Ok(IpcRequest::GetScenePreviews) => {
                IpcResponse::ScenePreviews(ctx.renderer.read().await.scene_previews())
            }
            Ok(IpcRequest::PushText { text, hand }) => {
                let trim_tracking = ctx.config.read().await.auto_actions.trim_link_tracking;
                let (item, original) = prepare_push_text(text, trim_tracking);
                schedule_async_previews(&item, Arc::clone(&ctx));
                {
                    let mut mgr = ctx.manager.write().await;
                    apply_prepared_push_text(&mut mgr, item.clone(), original, hand);
                }
                publish_pushed_clipboard(&item, hand, &ctx).await;
                ctx.refresh_display().await;
                IpcResponse::Success {
                    message: format!("Pushed to {:?}", hand),
                }
            }
            Ok(IpcRequest::PushImage { bytes, mime, hand }) => {
                let item_opt =
                    tokio::task::spawn_blocking(move || ClipItem::from_image_bytes(bytes, mime))
                        .await
                        .unwrap_or(None);

                if let Some(item) = item_opt {
                    schedule_async_previews(&item, Arc::clone(&ctx));
                    {
                        let mut mgr = ctx.manager.write().await;
                        mgr.push_to_hand(item.clone(), hand);
                    }
                    publish_pushed_clipboard(&item, hand, &ctx).await;
                    ctx.refresh_display().await;
                    IpcResponse::Success {
                        message: format!("Pushed image to {:?}", hand),
                    }
                } else {
                    IpcResponse::Error {
                        error: "Failed to decode image bytes".to_string(),
                    }
                }
            }
            Ok(IpcRequest::PushFile { path, hand }) => {
                let path_clone = path.clone();
                let item =
                    tokio::task::spawn_blocking(move || ClipItem::from_single_file(&path_clone))
                        .await
                        .unwrap_or_else(|_| ClipItem::from_single_file(&path));

                schedule_async_previews(&item, Arc::clone(&ctx));
                {
                    let mut mgr = ctx.manager.write().await;
                    mgr.push_to_hand(item.clone(), hand);
                }
                publish_pushed_clipboard(&item, hand, &ctx).await;
                ctx.refresh_display().await;
                IpcResponse::Success {
                    message: format!("Pushed file {:?} to {:?}", path, hand),
                }
            }
            Ok(IpcRequest::PushFiles { paths, hand })
            | Ok(IpcRequest::PushFileReferences { paths, hand }) => {
                if paths.is_empty() {
                    IpcResponse::Error {
                        error: "No files provided".to_string(),
                    }
                } else {
                    let paths_clone = paths.clone();
                    let item = tokio::task::spawn_blocking(move || {
                        build_file_item(paths_clone, references_only)
                    })
                    .await
                    .unwrap_or_else(|_| build_file_item(paths.clone(), references_only));

                    schedule_async_previews(&item, Arc::clone(&ctx));
                    {
                        let mut mgr = ctx.manager.write().await;
                        mgr.push_to_hand(item.clone(), hand);
                    }
                    publish_pushed_clipboard(&item, hand, &ctx).await;
                    ctx.refresh_display().await;
                    IpcResponse::Success {
                        message: format!("Pushed {} file(s) to {:?}", paths.len(), hand),
                    }
                }
            }
            Ok(IpcRequest::SwapSlots(a, b)) => {
                {
                    let mut mgr = ctx.manager.write().await;
                    mgr.swap_slots(a, b);
                }
                ctx.refresh_display().await;
                IpcResponse::Success {
                    message: format!("Swapped {} and {}", a.label(), b.label()),
                }
            }
            Ok(IpcRequest::Undo) => {
                let success = {
                    let mut mgr = ctx.manager.write().await;
                    mgr.undo()
                };
                if success {
                    ctx.refresh_display().await;
                    IpcResponse::Success {
                        message: "Undid last Left Hand operation".to_string(),
                    }
                } else {
                    IpcResponse::Error {
                        error: "Nothing to undo (Undo slot is empty)".to_string(),
                    }
                }
            }
            Ok(IpcRequest::Clear(slot)) => {
                {
                    let mut mgr = ctx.manager.write().await;
                    mgr.clear(slot);
                }
                ctx.refresh_display().await;
                IpcResponse::Success {
                    message: format!("Cleared {}", slot.label()),
                }
            }
            Ok(IpcRequest::Select(slot)) => {
                {
                    let mut mgr = ctx.manager.write().await;
                    mgr.toggle_selection(slot);
                    mgr.set_cursor(slot);
                }
                ctx.refresh_display().await;
                IpcResponse::Success {
                    message: format!("Selected {}", slot.label()),
                }
            }
            Ok(IpcRequest::ExecuteOp {
                op_name,
                secondary_slot,
                selected_slots,
                separator,
            }) => {
                handle_execute_op(
                    op_name,
                    selected_slots,
                    secondary_slot,
                    separator,
                    true,
                    &ctx,
                )
                .await
            }
            Ok(IpcRequest::ExecuteRecipe { recipe_name }) => {
                handle_execute_recipe(recipe_name, None, true, &ctx).await
            }
            Ok(IpcRequest::ExecuteRecipeWithPrompt {
                recipe_name,
                prompt_text,
            }) => handle_execute_recipe(recipe_name, prompt_text, true, &ctx).await,
            Ok(IpcRequest::GetLastAction) => {
                let act = ctx.last_action.read().await.clone();
                IpcResponse::LastAction(act)
            }
            Ok(IpcRequest::ExecuteLastAction { prompt_text }) => {
                let action = ctx.last_action.read().await.clone();
                match action {
                    Some(little_clip_core::ipc::LastAction::Recipe { recipe_name }) => {
                        handle_execute_recipe(recipe_name, prompt_text, false, &ctx).await
                    }
                    Some(little_clip_core::ipc::LastAction::Op {
                        op_name,
                        selected_slots,
                        secondary_slot,
                        separator,
                    }) => {
                        handle_execute_op(
                            op_name,
                            selected_slots,
                            secondary_slot,
                            separator,
                            false,
                            &ctx,
                        )
                        .await
                    }
                    None => IpcResponse::Error {
                        error: "No prior action has been executed yet".to_string(),
                    },
                }
            }

            Ok(IpcRequest::ExecuteGraphYaml { yaml }) => {
                match serde_yaml::from_str::<little_clip_ops::NodeGraph>(&yaml) {
                    Ok(graph) => {
                        let mgr_snapshot = { ctx.manager.read().await.clone() };
                        let started = std::time::Instant::now();
                        let execution = pipeline::execute(move || {
                            little_clip_ops::PipelineExecutor::new(&graph, &mgr_snapshot)
                                .execute()
                                .map_err(|error| format!("Graph execution failed: {error}"))
                        })
                        .await;
                        match execution {
                            Ok(pipe_res) => {
                                let duration_ms = started.elapsed().as_millis();
                                let node_previews = pipe_res.node_previews.clone();
                                let output_title = pipe_res
                                    .left_hand_item
                                    .as_ref()
                                    .map(|item| item.title.clone());
                                match apply_pipeline_result(pipe_res, &ctx).await {
                                    IpcResponse::OpResult { .. } | IpcResponse::Success { .. } => {
                                        IpcResponse::PipelineExecuted {
                                            node_previews,
                                            output_title,
                                            duration_ms,
                                        }
                                    }
                                    other => other,
                                }
                            }
                            Err(error) => IpcResponse::Error { error },
                        }
                    }
                    Err(e) => IpcResponse::Error {
                        error: format!("Invalid graph YAML: {}", e),
                    },
                }
            }
            Ok(IpcRequest::ListRecipes) => {
                let recipes = little_clip_ops::load_all_recipes();
                let names = recipes.into_iter().map(|r| r.name).collect();
                IpcResponse::RecipeList(names)
            }
            Ok(IpcRequest::ForceRefreshDisplay) => {
                ctx.refresh_display().await;
                IpcResponse::Success {
                    message: "Display refreshed".to_string(),
                }
            }

            Ok(IpcRequest::ReloadConfig) => {
                let new_config = little_clip_core::config::AppConfig::load();
                let new_audio_config = new_config.audio.clone();
                {
                    let mut config = ctx.config.write().await;
                    *config = new_config.clone();
                }
                match ctx.rolling_capture_config.write() {
                    Ok(mut config) => *config = new_audio_config.clone(),
                    Err(_) => warn!("Rolling audio configuration lock was poisoned during reload"),
                }
                match ctx.rolling_audio.write() {
                    Ok(mut buffer) => {
                        buffer.reconfigure(
                            new_audio_config.rolling_capture_duration_sec,
                            48_000,
                            2,
                        );
                    }
                    Err(_) => warn!("Rolling audio buffer lock was poisoned during reload"),
                }
                {
                    let mut r = ctx.renderer.write().await;
                    r.set_config(new_config);
                }
                ctx.refresh_display().await;
                IpcResponse::Success {
                    message: "Configuration reloaded and applied".to_string(),
                }
            }
            Ok(IpcRequest::Lock) => {
                ctx.set_locked(true).await;
                IpcResponse::Success {
                    message: "Screen locked (blanked)".to_string(),
                }
            }
            Ok(IpcRequest::Unlock) => {
                ctx.set_locked(false).await;
                IpcResponse::Success {
                    message: "Screen unlocked".to_string(),
                }
            }
            Ok(IpcRequest::SetLocked(locked)) => {
                ctx.set_locked(locked).await;
                IpcResponse::Success {
                    message: if locked {
                        "Screen locked (blanked)".to_string()
                    } else {
                        "Screen unlocked".to_string()
                    },
                }
            }
            Ok(IpcRequest::GetLockStatus) => IpcResponse::LockStatus(ctx.is_locked().await),
            Ok(IpcRequest::GetRollingAudio) => {
                let rolling_audio = Arc::clone(&ctx.rolling_audio);
                let res = tokio::task::spawn_blocking(move || {
                    let snapshot_dir = get_rolling_snapshot_dir();
                    std::fs::create_dir_all(&snapshot_dir)?;
                    let snapshot_path =
                        snapshot_dir.join(format!("snapshot-{}.wav", uuid::Uuid::new_v4()));
                    let guard = rolling_audio.read().map_err(|_| {
                        std::io::Error::other("rolling audio buffer lock was poisoned")
                    })?;
                    let duration = guard.write_wav(&snapshot_path)?;
                    Ok::<_, std::io::Error>((
                        duration,
                        guard.sample_rate(),
                        guard.channels(),
                        snapshot_path,
                    ))
                })
                .await;
                match res {
                    Ok(Ok((dur, sr, ch, snapshot_path))) => {
                        info!(
                            "Captured rolling audio snapshot: {:.2}s -> {:?}",
                            dur, snapshot_path
                        );
                        IpcResponse::RollingAudio {
                            path: snapshot_path,
                            duration_secs: dur,
                            sample_rate: sr,
                            channels: ch,
                        }
                    }
                    Ok(Err(e)) => {
                        error!("Failed to write rolling audio snapshot: {}", e);
                        IpcResponse::Error {
                            error: format!("Failed to capture rolling audio: {}", e),
                        }
                    }
                    Err(e) => IpcResponse::Error {
                        error: format!("Rolling audio snapshot worker failed: {e}"),
                    },
                }
            }
            Ok(IpcRequest::RouteNextClipboard {
                request_id,
                hand,
                timeout_ms,
                restore_os_clipboard,
            }) => {
                let expires =
                    tokio::time::Instant::now() + Duration::from_millis(timeout_ms.min(60_000));
                let mut route = ctx.route_next_clipboard.write().await;
                route.install(request_id, hand, expires, restore_os_clipboard);
                match request_id {
                    Some(request_id) => IpcResponse::ClipboardRoute {
                        request_id,
                        status: route.status(request_id, tokio::time::Instant::now()),
                    },
                    None => IpcResponse::Success {
                        message: format!("Next clipboard event routed to {hand:?}"),
                    },
                }
            }
            Ok(IpcRequest::GetClipboardRoute { request_id }) => IpcResponse::ClipboardRoute {
                request_id,
                status: ctx
                    .route_next_clipboard
                    .read()
                    .await
                    .status(request_id, tokio::time::Instant::now()),
            },
            Ok(IpcRequest::CancelClipboardRoute { request_id }) => IpcResponse::ClipboardRoute {
                request_id,
                status: ctx
                    .route_next_clipboard
                    .write()
                    .await
                    .cancel(request_id, tokio::time::Instant::now()),
            },
            Ok(IpcRequest::GetSlotItem(slot)) => {
                let mgr = ctx.manager.read().await;
                let item = mgr.get(slot).cloned();
                IpcResponse::SlotItem(item)
            }
            Ok(IpcRequest::GetAutoActions) => {
                let cfg = ctx.config.read().await;
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "trim_link_tracking".to_string(),
                    cfg.auto_actions.trim_link_tracking,
                );
                IpcResponse::AutoActions(map)
            }
            Ok(IpcRequest::SetAutoAction { key, enabled }) => match key.as_str() {
                "trim_link_tracking" => {
                    let mut cfg = ctx.config.write().await;
                    cfg.auto_actions.trim_link_tracking = enabled;
                    IpcResponse::Success {
                        message: format!("Auto-action '{}' set to {}", key, enabled),
                    }
                }
                _ => IpcResponse::Error {
                    error: format!("Unknown auto-action: {}", key),
                },
            },
            Err(e) => IpcResponse::Error {
                error: format!("Invalid request: {}", e),
            },
        };

        if let Ok(json_str) = serde_json::to_string(&resp) {
            info!("Sending {} bytes of response JSON...", json_str.len());
            let _ = writer.write_all(json_str.as_bytes()).await;
            let _ = writer.write_all(b"\n").await;
            let _ = writer.flush().await;
        }
    }
}

async fn apply_pipeline_result(
    res: little_clip_ops::PipelineExecutionResult,
    ctx: &Arc<DaemonContext>,
) -> IpcResponse {
    let mut last_item = None;

    if let Some(right_item) = res.right_hand_item {
        schedule_async_previews(&right_item, Arc::clone(ctx));
        let mut mgr = ctx.manager.write().await;
        mgr.push_right(right_item.clone());
        last_item = Some(right_item);
    }

    if let Some(left_item) = res.left_hand_item {
        schedule_async_previews(&left_item, Arc::clone(ctx));
        let text_opt = extract_clipboard_text(&left_item);
        if let Some(ref t) = text_opt {
            ctx.add_self_copied_text(t.clone()).await;
        }
        copy_to_os_clipboard(&left_item, ctx).await;
        {
            let mut mgr = ctx.manager.write().await;
            mgr.apply_op_result(left_item.clone());
        }

        last_item = Some(left_item);
    }

    ctx.refresh_display().await;

    if res.should_paste {
        simulate_paste_action();
    }

    if let Some(item) = last_item {
        IpcResponse::OpResult { item }
    } else {
        IpcResponse::Success {
            message: format!(
                "Pipeline executed successfully (saved {} files)",
                res.saved_files.len()
            ),
        }
    }
}

fn simulate_paste_action() {
    let _ = little_clip_core::BackgroundJobs::global().submit(
        std::time::Duration::from_secs(2),
        || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            if little_clip_core::command_available("wtype").is_some() {
                let _ = little_clip_core::CommandRequest::new("wtype")
                    .args(["-M", "ctrl", "-k", "v", "-m", "ctrl"])
                    .discard_output()
                    .run();
            }
        },
    );
}

fn schedule_thumbnail_generation(item: &ClipItem, ctx: Arc<DaemonContext>) {
    if let little_clip_core::ClipPayload::Image {
        ref bytes,
        ref thumbnail,
        ref mime,
        ..
    } = item.payload
    {
        if thumbnail.is_none() {
            let item_id = item.id;
            let bytes = bytes.clone();
            let mime = mime.clone();
            tokio::task::spawn_blocking(move || {
                if let Some(thumb) = little_clip_core::item::ImageThumbnail::generate_with_mime(
                    &bytes,
                    Some(&mime),
                    320,
                    220,
                ) {
                    tokio::spawn(async move {
                        let updated = {
                            let mut mgr = ctx.manager.write().await;
                            mgr.update_thumbnail(item_id, thumb)
                        };
                        if updated {
                            ctx.refresh_display().await;
                        }
                    });
                }
            });
        }
    }
}

fn schedule_audio_waveform_generation(item: &ClipItem, ctx: Arc<DaemonContext>) {
    if let little_clip_core::ClipPayload::AudioRef {
        ref path,
        duration_ms,
        ref waveform,
        ..
    } = item.payload
    {
        if waveform.is_none() {
            let item_id = item.id;
            let path_clone = path.clone();
            let ctx_p1 = Arc::clone(&ctx);
            let ctx_p2 = Arc::clone(&ctx);

            let audio_cfg = match ctx.renderer.try_read() {
                Ok(r) => r.config.audio.clone(),
                Err(_) => little_clip_core::AppConfig::load().audio,
            };

            tokio::task::spawn_blocking(move || {
                // Phase 1: Fast plain waveform generation (200 bars)
                // If duration exceeds audio_cfg.max_full_waveform_sec, extracts a 30s snippet
                let audio_cfg_p2 = audio_cfg.clone();
                if let Some(plain_wf) =
                    little_clip_core::compute_plain_waveform(&path_clone, 200, &audio_cfg)
                {
                    let path_p2 = path_clone.clone();
                    tokio::spawn(async move {
                        let updated = {
                            let mut mgr = ctx_p1.manager.write().await;
                            mgr.update_audio_waveform(item_id, plain_wf)
                        };
                        if updated {
                            ctx_p1.refresh_display().await;
                        }

                        // Phase 2: Spectral frequency analysis (adds frequency-based color map)
                        tokio::task::spawn_blocking(move || {
                            if let Some(spectral_colors) = little_clip_core::compute_spectral_colors(
                                &path_p2,
                                200,
                                &audio_cfg_p2,
                            ) {
                                tokio::spawn(async move {
                                    let updated_spectral = {
                                        let mut mgr = ctx_p2.manager.write().await;
                                        mgr.attach_audio_spectral_colors(item_id, spectral_colors)
                                    };
                                    if updated_spectral {
                                        ctx_p2.refresh_display().await;
                                    }
                                });
                            }
                        });
                    });
                } else {
                    tracing::warn!("Failed to decode audio waveform for {:?}", path_clone);
                    let empty_wf = little_clip_core::AudioWaveform {
                        channels: 2,
                        sample_rate: 44100,
                        duration_ms,
                        ch1_peaks: Vec::new(),
                        ch2_peaks: None,
                        spectral_colors: None,
                        is_snippet: false,
                        snippet_start_ms: 0,
                        snippet_duration_ms: 0,
                    };
                    tokio::spawn(async move {
                        let updated = {
                            let mut mgr = ctx_p1.manager.write().await;
                            mgr.update_audio_waveform(item_id, empty_wf)
                        };
                        if updated {
                            ctx_p1.refresh_display().await;
                        }
                    });
                }
            });
        }
    }
}

fn schedule_animation_generation(item: &ClipItem, ctx: Arc<DaemonContext>) {
    if let little_clip_core::ClipPayload::Animation {
        ref path,
        ref format,
        ref thumbnail,
        ..
    } = item.payload
    {
        if thumbnail.is_none() {
            let item_id = item.id;
            let path_clone = path.clone();
            let format_clone = format.clone();
            let ctx_clone = Arc::clone(&ctx);

            tokio::task::spawn_blocking(move || {
                let anim_thumb = if format_clone == "GIF" || format_clone == "WEBP" {
                    little_clip_core::animation::decode_animated_gif_or_webp(
                        &path_clone,
                        200,
                        150,
                        25,
                    )
                } else if little_clip_core::animation::is_video_path(&path_clone) {
                    little_clip_core::animation::extract_video_preview_frames(
                        &path_clone,
                        200,
                        150,
                        25,
                        2.5,
                        true,
                    )
                } else {
                    None
                };

                if let Some(thumb) = anim_thumb {
                    tokio::spawn(async move {
                        let updated = {
                            let mut mgr = ctx_clone.manager.write().await;
                            mgr.update_animation_thumbnail(item_id, thumb)
                        };
                        if updated {
                            ctx_clone.refresh_display().await;
                        }
                    });
                } else {
                    tracing::warn!(
                        "Failed to generate animation thumbnail for {:?}",
                        path_clone
                    );
                }
            });
        }
    }
}

fn schedule_multi_image_thumbnail_generation(item: &ClipItem, ctx: Arc<DaemonContext>) {
    if let little_clip_core::ClipPayload::MultiImage {
        ref paths,
        ref preview_indices,
        ref thumbnails,
        ..
    } = item.payload
    {
        if thumbnails.is_empty() {
            let item_id = item.id;
            let indices = preview_indices.clone();
            let paths_subset: Vec<PathBuf> = indices
                .iter()
                .filter_map(|&idx| paths.get(idx).cloned())
                .collect();

            tokio::task::spawn_blocking(move || {
                let mut thumbs = Vec::with_capacity(paths_subset.len());
                for path in paths_subset {
                    if let Ok(bytes) = std::fs::read(&path) {
                        if let Some(thumb) =
                            little_clip_core::item::ImageThumbnail::generate_with_mime(
                                &bytes,
                                Some(&little_clip_core::item::guess_image_mime(&path)),
                                200,
                                160,
                            )
                        {
                            thumbs.push(thumb);
                        }
                    }
                }
                if !thumbs.is_empty() {
                    tokio::spawn(async move {
                        let updated = {
                            let mut mgr = ctx.manager.write().await;
                            mgr.update_multi_image_thumbnails(item_id, thumbs)
                        };
                        if updated {
                            ctx.refresh_display().await;
                        }
                    });
                }
            });
        }
    }
}

fn schedule_async_previews(item: &ClipItem, ctx: Arc<DaemonContext>) {
    schedule_thumbnail_generation(item, Arc::clone(&ctx));
    schedule_audio_waveform_generation(item, Arc::clone(&ctx));
    schedule_animation_generation(item, Arc::clone(&ctx));
    schedule_multi_image_thumbnail_generation(item, ctx);
}

async fn run_clipboard_poller(ctx: Arc<DaemonContext>) {
    use std::hash::{Hash, Hasher};

    let mut last_clipboard_text = String::new();
    let mut last_image_hash: Option<u64> = None;
    let mut interval = tokio::time::interval(Duration::from_millis(350));

    loop {
        interval.tick().await;

        // 1. Check clipboard MIME types
        let types_output = tokio::process::Command::new("wl-paste")
            .arg("--list-types")
            .output()
            .await;

        let image_mime = types_output
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| {
                let types = String::from_utf8_lossy(&out.stdout);
                let mut supported = types.lines().map(str::trim).filter(|mime| {
                    little_clip_core::item::image_extension_for_mime(mime).is_some()
                });
                // Prefer PNG when offered, but request the actual advertised format.
                supported
                    .clone()
                    .find(|mime| *mime == "image/png")
                    .or_else(|| supported.next())
                    .map(str::to_owned)
            });

        if let Some(image_mime) = image_mime {
            // Attempt reading raw image bytes
            if let Ok(img_out) = tokio::process::Command::new("wl-paste")
                .arg("-t")
                .arg(&image_mime)
                .output()
                .await
            {
                if img_out.status.success() && !img_out.stdout.is_empty() {
                    let mut hasher = std::collections::hash_map::DefaultHasher::new();
                    img_out.stdout.hash(&mut hasher);
                    let hash = hasher.finish();

                    let self_img_hash = *ctx.self_copied_image_hash.read().await;
                    if self_img_hash == Some(hash) {
                        last_image_hash = Some(hash);
                        continue;
                    }

                    if Some(hash) != last_image_hash {
                        last_image_hash = Some(hash);
                        last_clipboard_text.clear();

                        let (target_hand, restore_os) = ctx.take_route_override().await;

                        if let Some(item) = ClipItem::from_image_bytes(img_out.stdout, &image_mime)
                        {
                            info!(
                                "Captured clipboard image to {:?}: {} ({})",
                                target_hand, item.title, item.subtitle
                            );
                            schedule_async_previews(&item, Arc::clone(&ctx));
                            let prior_left = {
                                let mut mgr = ctx.manager.write().await;
                                let old_left = mgr.left_hand().cloned();
                                mgr.push_to_hand(item, target_hand);
                                old_left
                            };
                            ctx.refresh_display().await;
                            ctx.maybe_restore_os_clipboard_after_route(
                                restore_os,
                                target_hand,
                                prior_left,
                            );
                            continue;
                        }
                    } else {
                        // Same image as last tick, no update needed
                        continue;
                    }
                }
            }
        }

        // 2. Fall back to text / file URI polling
        if let Ok(output) = tokio::process::Command::new("wl-paste")
            .arg("-n")
            .output()
            .await
        {
            if output.status.success() {
                if let Ok(text) = String::from_utf8(output.stdout) {
                    if ctx.is_self_copied_text(&text).await {
                        last_clipboard_text = text;
                        continue;
                    }

                    // Check if there is an active override routing to Right Hand
                    let has_active_right_route = {
                        ctx.route_next_clipboard
                            .read()
                            .await
                            .targets_right(tokio::time::Instant::now())
                    };

                    // Check if clipboard content matches what is already in Left Hand
                    let is_already_in_left = {
                        let mgr = ctx.manager.read().await;
                        if let Some(left_item) = mgr.left_hand() {
                            match &left_item.payload {
                                little_clip_core::ClipPayload::FileBatch(files) => {
                                    let cand =
                                        little_clip_core::item::extract_local_file_paths(&text);
                                    !cand.is_empty() && cand == *files
                                }
                                little_clip_core::ClipPayload::AudioRef { path, .. } => {
                                    let cand =
                                        little_clip_core::item::extract_local_file_paths(&text);
                                    cand.len() == 1 && cand[0] == *path
                                }
                                little_clip_core::ClipPayload::Text(t) => t.trim() == text.trim(),
                                _ => false,
                            }
                        } else {
                            false
                        }
                    };

                    if !has_active_right_route && is_already_in_left {
                        last_clipboard_text = text;
                        continue;
                    }

                    if !text.is_empty() && (text != last_clipboard_text || has_active_right_route) {
                        last_clipboard_text = text.clone();
                        last_image_hash = None;

                        let (target_hand, restore_os) = ctx.take_route_override().await;

                        let (item, original_untracked) = if ctx
                            .config
                            .read()
                            .await
                            .auto_actions
                            .trim_link_tracking
                        {
                            if let Some(cleaned) = little_clip_ops::clean_url_tracking(&text) {
                                info!(
                                    "Auto-Action: Trimmed link tracking from URL (original saved to Undo)"
                                );
                                (
                                    ClipItem::from_text(cleaned),
                                    Some(ClipItem::from_text(text.clone())),
                                )
                            } else {
                                (ClipItem::from_text(text.clone()), None)
                            }
                        } else {
                            (ClipItem::from_text(text.clone()), None)
                        };
                        let was_cleaned = original_untracked.is_some();
                        info!(
                            "Captured clipboard item to {:?}: [{}] {}",
                            target_hand, item.handler_tag, item.title
                        );
                        schedule_async_previews(&item, Arc::clone(&ctx));
                        let prior_left = {
                            let mut mgr = ctx.manager.write().await;
                            let old_left = mgr.left_hand().cloned();
                            if let Some(orig) = original_untracked {
                                mgr.undo = Some(orig);
                            }
                            mgr.push_to_hand(item.clone(), target_hand);
                            old_left
                        };
                        if was_cleaned {
                            if let Some(clean_text) = item.to_clipboard_text() {
                                last_clipboard_text = clean_text.clone();
                                copy_cleaned_clipboard_text(clean_text).await;
                            }
                        }
                        ctx.refresh_display().await;
                        ctx.maybe_restore_os_clipboard_after_route(
                            restore_os,
                            target_hand,
                            prior_left,
                        );
                    }
                }
            }
        }
    }
}

fn extract_clipboard_text(item: &ClipItem) -> Option<String> {
    match &item.payload {
        little_clip_core::ClipPayload::Text(t) => Some(t.clone()),
        little_clip_core::ClipPayload::Markdown(m) => Some(m.clone()),
        little_clip_core::ClipPayload::Url(u) => Some(u.to_string()),
        little_clip_core::ClipPayload::Color { hex, .. } => Some(hex.clone()),
        little_clip_core::ClipPayload::MultiImage { paths, .. } => Some(
            paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        little_clip_core::ClipPayload::FileBatch(files) => Some(
            files
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        little_clip_core::ClipPayload::AudioRef { path, .. } => {
            Some(path.to_string_lossy().to_string())
        }
        _ => None,
    }
}

async fn copy_cleaned_clipboard_text(text: String) {
    match tokio::task::spawn_blocking(move || copy_text_to_system_clipboard(&text)).await {
        Ok(Err(error)) => warn!(%error, "failed to copy cleaned clipboard text"),
        Err(error) => warn!(%error, "clipboard copy worker failed"),
        Ok(Ok(())) => {}
    }
}

fn is_image_file_path(p: &std::path::Path) -> bool {
    little_clip_core::item::is_image_path(p)
}

// Explicit Right Hand pushes collect references/content without replacing the system selection.
async fn publish_pushed_clipboard(item: &ClipItem, hand: HandSide, ctx: &DaemonContext) {
    if hand == HandSide::Left {
        copy_to_os_clipboard(item, ctx).await;
    }
}

async fn copy_to_os_clipboard(item: &ClipItem, ctx: &DaemonContext) {
    match &item.payload {
        little_clip_core::ClipPayload::Image { bytes, mime, .. } => {
            copy_image(bytes, mime, ctx).await
        }
        little_clip_core::ClipPayload::FileBatch(files)
            if files.len() == 1 && is_image_file_path(&files[0]) =>
        {
            copy_image_file(&files[0], ctx).await
        }
        little_clip_core::ClipPayload::Text(text) => copy_text(text, "text", ctx).await,
        little_clip_core::ClipPayload::Markdown(text) => copy_text(text, "Markdown", ctx).await,
        little_clip_core::ClipPayload::Url(url) => copy_text(url.as_str(), "URL", ctx).await,
        little_clip_core::ClipPayload::Color { hex, .. } => copy_text(hex, "color", ctx).await,
        little_clip_core::ClipPayload::MultiImage { paths, .. } => {
            copy_path_text(paths, "image paths", ctx).await
        }
        little_clip_core::ClipPayload::FileBatch(files) => copy_file_batch(files, ctx).await,
        little_clip_core::ClipPayload::AudioRef { path, .. } => copy_audio_path(path, ctx).await,
        _ => {}
    }
}

async fn copy_image(bytes: &[u8], mime: &str, ctx: &DaemonContext) {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    *ctx.self_copied_image_hash.write().await = Some(hasher.finish());
    if let Err(error) = copy_bytes_to_system_clipboard(bytes, mime) {
        warn!(%error, "failed to copy image to system clipboard");
    }
}

async fn copy_image_file(path: &std::path::Path, ctx: &DaemonContext) {
    let Ok(bytes) = std::fs::read(path) else {
        warn!(path = %path.display(), "failed to read image file for clipboard copy");
        return;
    };
    let mime = little_clip_core::item::guess_image_mime(path);
    copy_image(&bytes, &mime, ctx).await;
}

async fn copy_text(text: &str, description: &str, ctx: &DaemonContext) {
    ctx.add_self_copied_text(text.to_owned()).await;
    if let Err(error) = copy_text_to_system_clipboard(text) {
        warn!(%error, %description, "failed to copy to system clipboard");
    }
}

async fn copy_path_text(paths: &[PathBuf], description: &str, ctx: &DaemonContext) {
    let text = paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    copy_text(&text, description, ctx).await;
}

async fn copy_file_batch(files: &[PathBuf], ctx: &DaemonContext) {
    let Ok(uri_list) = file_uri_list(files) else {
        warn!("failed to encode file paths as clipboard URIs");
        return;
    };
    ctx.add_self_copied_text(uri_list.clone()).await;
    ctx.add_self_copied_text(
        files
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .await;
    if let Err(error) = copy_bytes_to_system_clipboard(uri_list.as_bytes(), "text/uri-list") {
        warn!(%error, "failed to copy files to system clipboard");
    }
}

async fn copy_audio_path(path: &std::path::Path, ctx: &DaemonContext) {
    let Ok(uri) = file_uri_list(&[path]) else {
        warn!(path = %path.display(), "failed to encode audio path as clipboard URI");
        return;
    };
    ctx.add_self_copied_text(uri.clone()).await;
    ctx.add_self_copied_text(path.to_string_lossy().into_owned())
        .await;
    if let Err(error) = copy_bytes_to_system_clipboard(uri.as_bytes(), "text/uri-list") {
        warn!(%error, "failed to copy audio path to system clipboard");
    }
}

/// Returns the Unix domain socket path used for IPC communication (`$XDG_RUNTIME_DIR/little-screen-clip.sock`).
pub use little_clip_core::ipc::socket_path as get_socket_path;

/// Returns the filesystem directory for saving rolling audio buffer snapshots.
pub fn get_rolling_snapshot_dir() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir)
            .join("little-screen-clip")
            .join("rolling_snapshots")
    } else {
        PathBuf::from("/tmp/little-screen-clip/rolling_snapshots")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const TRACKED_URL: &str = "https://example.com/path?utm_source=newsletter&keep=value";
    const CLEAN_URL: &str = "https://example.com/path?keep=value";

    fn test_context(manager: SlotManager) -> Arc<DaemonContext> {
        let config = little_clip_core::AppConfig::default();
        let (display_tx, _display_rx) = watch::channel(None);
        // An isolated handler: no daemon startup, USB thread, capture thread,
        // external clipboard process or user's socket is involved.
        Arc::new(DaemonContext {
            config: RwLock::new(config.clone()),
            manager: RwLock::new(manager),
            renderer: RwLock::new(ScreenRenderer::fallback(config.clone())),
            display_tx,
            self_copied_text: RwLock::new(Vec::new()),
            self_copied_image_hash: RwLock::new(None),
            last_action: RwLock::new(None),
            is_locked: AtomicBool::new(false),
            rolling_audio: Arc::new(std::sync::RwLock::new(RollingAudioBuffer::new(1, 1, 1))),
            rolling_capture_config: Arc::new(std::sync::RwLock::new(config.audio)),
            route_next_clipboard: RwLock::new(clipboard_route::ClipboardRoute::default()),
        })
    }

    // The child sees only a fake clipboard adapter, never the user's Wayland/X11 sockets.
    fn isolated_clipboard_test(name: &str) -> bool {
        use std::os::unix::fs::PermissionsExt;
        if std::env::var_os("CLIPD_TEST_CLIPBOARD").is_some() {
            return false;
        }
        let directory = tempfile::tempdir().unwrap();
        let adapter = directory.path().join("wl-copy");
        std::fs::write(
            &adapter,
            r#"#!/bin/sh
/bin/cat > "$CLIPD_TEST_CLIPBOARD/bytes"
printf '%s' "$2" > "$CLIPD_TEST_CLIPBOARD/mime"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&adapter, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--test-threads=1"])
            .env("CLIPD_TEST_CLIPBOARD", directory.path())
            .env("PATH", directory.path())
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DISPLAY")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    #[tokio::test]
    async fn right_hand_pushes_do_not_modify_the_system_clipboard() {
        if isolated_clipboard_test("tests::right_hand_pushes_do_not_modify_the_system_clipboard") {
            return;
        }
        let directory = PathBuf::from(std::env::var_os("CLIPD_TEST_CLIPBOARD").unwrap());
        let path = directory.join("reference.txt");
        std::fs::write(&path, b"reference contents").unwrap();
        let mut manager = SlotManager::default();
        manager.push_left(ClipItem::from_text("original left"));
        let original_left = manager.left_hand().unwrap().id;
        let ctx = test_context(manager);
        let (client, server) = UnixStream::pair().unwrap();
        let handler = tokio::spawn(handle_client(server, Arc::clone(&ctx)));
        let (reader, mut writer) = client.into_split();
        let mut lines = BufReader::new(reader).lines();
        let hand = HandSide::Right;
        let requests = [
            IpcRequest::PushText {
                text: "reference text".into(),
                hand,
            },
            IpcRequest::PushImage {
                bytes: vec![
                    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0,
                    0, 1, 8, 2, 0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156,
                    99, 248, 207, 192, 0, 0, 3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69,
                    78, 68, 174, 66, 96, 130,
                ],
                mime: "image/png".into(),
                hand,
            },
            IpcRequest::PushFile {
                path: path.clone(),
                hand,
            },
            IpcRequest::PushFiles {
                paths: vec![path.clone()],
                hand,
            },
            IpcRequest::PushFileReferences {
                paths: vec![path],
                hand,
            },
        ];
        for request in requests {
            writer
                .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
                .await
                .unwrap();
            let response = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(
                matches!(
                    serde_json::from_str::<IpcResponse>(&response).unwrap(),
                    IpcResponse::Success { .. }
                ),
                "{response}"
            );
            assert_eq!(
                ctx.manager.read().await.left_hand().unwrap().id,
                original_left
            );
            assert!(ctx.manager.read().await.get(SlotId::RightHand).is_some());
            assert!(
                !directory.join("bytes").exists(),
                "a Right Hand push wrote the clipboard"
            );
            assert!(ctx.self_copied_text.read().await.is_empty());
            assert!(ctx.self_copied_image_hash.read().await.is_none());
        }
        // The same handler must still publish an explicit Left Hand push.
        let request = IpcRequest::PushText {
            text: "new left".into(),
            hand: HandSide::Left,
        };
        writer
            .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
            .await
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(
            serde_json::from_str::<IpcResponse>(&response).unwrap(),
            IpcResponse::Success { .. }
        ));
        assert_eq!(std::fs::read(directory.join("bytes")).unwrap(), b"new left");
        assert_eq!(
            std::fs::read_to_string(directory.join("mime")).unwrap(),
            "text/plain;charset=utf-8"
        );
        drop(writer);
        drop(lines);
        handler.await.unwrap();
    }

    #[tokio::test]
    async fn non_utf8_ipc_returns_error_and_preserves_next_request() {
        let ctx = test_context(SlotManager::default());
        let (client, server) = UnixStream::pair().unwrap();
        let handler = tokio::spawn(handle_client(server, ctx));
        let (reader, mut writer) = client.into_split();
        let mut lines = BufReader::new(reader).lines();
        writer.write_all(b"\xff\n").await.unwrap();
        let request = serde_json::to_string(&IpcRequest::GetStateSummary).unwrap();
        writer
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();
        for expect_error in [true, false] {
            let response = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
                .await
                .expect("every framed request needs a response")
                .unwrap()
                .unwrap();
            let response = serde_json::from_str::<IpcResponse>(&response).unwrap();
            if expect_error {
                assert!(
                    matches!(response, IpcResponse::Error { .. }),
                    "{response:?}"
                );
            } else {
                assert!(
                    matches!(response, IpcResponse::StateSummary(_)),
                    "{response:?}"
                );
            }
        }
        drop(writer);
        drop(lines);
        handler.await.unwrap();
    }

    #[tokio::test]
    async fn pipeline_ipc_reports_completed_saves_on_failure_without_changing_slots() {
        use little_clip_ops::pipeline::{
            Connection, NodeGraph, find_template, instantiate_template,
        };
        let fixture = tempfile::tempdir().unwrap();
        let input = fixture.path().join("first.bin");
        std::fs::write(&input, b"completed bytes").unwrap();
        let missing = fixture.path().join("missing.bin");
        let item = ClipItem::from_files(vec![input, missing.clone()]);
        let id = item.id;
        let mut manager = SlotManager::default();
        manager.push_left(item);
        let ctx = test_context(manager);
        let source = instantiate_template(find_template("source_lh").unwrap(), [0.0, 0.0]);
        let mut sink = instantiate_template(find_template("sink_save_disk").unwrap(), [100.0, 0.0]);
        let destination = fixture.path().join("output");
        sink.props
            .insert("destination".into(), destination.to_str().unwrap().into());
        let graph = NodeGraph {
            connections: vec![Connection {
                from_node: source.id,
                from_pin: "Out".into(),
                to_node: sink.id,
                to_pin: "In".into(),
            }],
            nodes: vec![source, sink],
            ..Default::default()
        };
        let request = IpcRequest::ExecuteGraphYaml {
            yaml: serde_yaml::to_string(&graph).unwrap(),
        };
        let (client, server) = UnixStream::pair().unwrap();
        let handler = tokio::spawn(handle_client(server, Arc::clone(&ctx)));
        let (reader, mut writer) = client.into_split();
        let mut reader = BufReader::new(reader);
        let mut encoded = serde_json::to_vec(&request).unwrap();
        encoded.push(b'\n');
        writer.write_all(&encoded).await.unwrap();
        let mut response = String::new();
        reader.read_line(&mut response).await.unwrap();
        let completed = destination.join("first.bin");
        match serde_json::from_str::<IpcResponse>(&response).unwrap() {
            IpcResponse::Error { error } => {
                assert!(error.contains(completed.to_str().unwrap()), "{error}");
                assert!(error.contains(missing.to_str().unwrap()), "{error}");
            }
            other => panic!("expected a partial-save failure, got {other:?}"),
        }
        assert_eq!(std::fs::read(completed).unwrap(), b"completed bytes");
        assert_eq!(ctx.manager.read().await.left_hand().unwrap().id, id);
        drop(writer);
        drop(reader);
        handler.await.unwrap();
    }

    #[test]
    fn push_text_cleans_tracking_url_into_requested_hand() {
        let mut manager = SlotManager::default();
        manager.push_left(ClipItem::from_text("previous left item"));

        let (item, original) = prepare_push_text(TRACKED_URL.to_owned(), true);
        apply_prepared_push_text(&mut manager, item, original, HandSide::Left);

        assert_eq!(
            manager
                .left_hand()
                .and_then(ClipItem::to_clipboard_text)
                .as_deref(),
            Some(CLEAN_URL)
        );
        assert_eq!(
            manager
                .get(SlotId::LeftPrior1)
                .and_then(ClipItem::to_clipboard_text)
                .as_deref(),
            Some("previous left item")
        );
    }

    #[test]
    fn push_text_preserves_original_tracking_url_in_undo() {
        let mut manager = SlotManager::default();

        let (item, original) = prepare_push_text(TRACKED_URL.to_owned(), true);
        apply_prepared_push_text(&mut manager, item, original, HandSide::Left);

        assert_eq!(
            manager
                .undo_slot()
                .and_then(ClipItem::to_clipboard_text)
                .as_deref(),
            Some(TRACKED_URL)
        );
    }

    #[tokio::test]
    async fn bounded_ipc_reader_preserves_line_boundaries() {
        let mut reader = BufReader::new(Cursor::new(b"first\nsecond\n".to_vec()));
        let mut request = Vec::new();

        assert_eq!(
            read_bounded_ipc_line(&mut reader, &mut request)
                .await
                .unwrap(),
            Some(6)
        );
        assert_eq!(request, b"first\n");

        assert_eq!(
            read_bounded_ipc_line(&mut reader, &mut request)
                .await
                .unwrap(),
            Some(7)
        );
        assert_eq!(request, b"second\n");
    }

    #[tokio::test]
    async fn bounded_ipc_reader_rejects_oversized_request() {
        let bytes = vec![b'x'; MAX_IPC_REQUEST_BYTES + 1];
        let mut reader = BufReader::new(Cursor::new(bytes));
        let mut request = Vec::new();

        let error = read_bounded_ipc_line(&mut reader, &mut request)
            .await
            .expect_err("oversized requests must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
