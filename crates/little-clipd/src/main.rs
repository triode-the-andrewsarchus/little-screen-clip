//! Little Screen Clip background daemon (`little-clipd`).
//!
//! Manages active clipboard items, background audio recording, physical display updates,
//! Wayland clipboard monitoring (`wl-paste`), and IPC communication with `clipctl` and `clip-workbench`.

use little_clip_core::{
    ClipItem, HandSide, SlotId, SlotManager, copy_bytes_to_system_clipboard, file_uri_list,
    ipc::{IpcRequest, IpcResponse},
    rolling_audio::RollingAudioBuffer,
};
use little_clip_driver::TrofeoDriver;
use little_clip_render::ScreenRenderer;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, watch};
use tracing::{error, info, warn};

mod clipboard_events;
mod clipboard_route;
mod display_worker;
mod lock_monitor;
mod pipeline;
mod rolling_capture;
mod rolling_snapshots;

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

/// Independent reasons to blank the display; clearing one never clears another.
#[derive(Clone, Copy)]
#[repr(u8)]
enum LockSource {
    Manual = 1,
    Sleep = 2,
    Session = 4,
    Shell = 8,
}

#[derive(Default)]
struct DisplayLock {
    locked: bool,
    generation: u64,
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
    clipboard: Option<clipboard_events::Clipboard>,
    /// Most recently executed IPC action for repeat / replay commands.
    pub last_action: RwLock<Option<little_clip_core::ipc::LastAction>>,
    display_lock: Mutex<DisplayLock>,
    display_render: Arc<tokio::sync::Mutex<()>>,
    lock_sources: tokio::sync::Mutex<u8>,
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
        self.display_lock.lock().unwrap().locked
    }

    /// Updates manual blanking without overriding active session/sleep locks.
    pub async fn set_locked(&self, locked: bool) {
        self.set_lock_source(LockSource::Manual, locked).await;
    }

    async fn set_lock_source(&self, source: LockSource, locked: bool) {
        let mut sources = self.lock_sources.lock().await;
        if locked {
            *sources |= source as u8;
        } else {
            *sources &= !(source as u8);
        }
        // Serialize source updates through publication of the resulting transition.
        self.apply_lock_state(*sources != 0).await;
    }

    async fn resume_from_sleep(&self, locked_hint: Option<bool>) {
        let mut sources = self.lock_sources.lock().await;
        // The caller supplies a fresh logind hint. Unknown state cannot authorize
        // showing clipboard content; a confirmed unlock retires a missed Lock event.
        if locked_hint.unwrap_or(true) {
            *sources |= LockSource::Session as u8;
        } else {
            *sources &= !(LockSource::Session as u8);
        }
        *sources &= !(LockSource::Sleep as u8);
        self.apply_lock_state(*sources != 0).await;
    }

    async fn apply_lock_state(&self, locked: bool) {
        let generation = {
            let mut state = self.display_lock.lock().unwrap();
            if state.locked == locked {
                return;
            }
            state.locked = locked;
            state.generation = state.generation.wrapping_add(1);
            state.generation
        };

        if locked {
            info!("Lock state engaged -> Blanking hardware display");
            let renderer = self.renderer.read().await.clone();
            match tokio::task::spawn_blocking(move || renderer.render_blank_frame()).await {
                Ok(Ok(frame)) => self.publish_display_frame(generation, frame),
                Ok(Err(error)) => error!(%error, "Could not render blank display frame"),
                Err(error) => error!(%error, "Blank display worker failed"),
            }
        } else {
            info!("Lock state cleared -> Restoring display contents");
            self.refresh_display().await;
        }
    }

    async fn set_auto_action(&self, key: String, enabled: bool) -> IpcResponse {
        if key != "trim_link_tracking" {
            return IpcResponse::Error {
                error: format!("Unknown auto-action: {key}"),
            };
        }
        // Keep reload and other setters behind the same guard through disk and
        // renderer publication, so an acknowledged preference cannot be rolled back.
        let mut config = self.config.write().await;
        let saved = tokio::task::spawn_blocking(move || {
            little_clip_core::AppConfig::save_trim_link_tracking_to_file(enabled)
                .map_err(|error| error.to_string())
        })
        .await;
        match saved {
            Ok(Ok(())) => {
                config.auto_actions.trim_link_tracking = enabled;
                self.renderer.write().await.set_config(config.clone());
                IpcResponse::Success {
                    message: format!("Auto-action '{key}' set to {enabled}"),
                }
            }
            Ok(Err(error)) => IpcResponse::Error {
                error: format!("Could not save auto-action: {error}"),
            },
            Err(error) => IpcResponse::Error {
                error: format!("Auto-action save worker failed: {error}"),
            },
        }
    }

    fn publish_display_frame(&self, generation: u64, frame: Vec<u8>) {
        // Serialize the final check and publication with lock transitions.
        let state = self.display_lock.lock().unwrap();
        if state.generation == generation {
            let _ = self.display_tx.send(Some(frame.into()));
        }
    }

    /// Rerenders the current slot inventory layout and sends the new frame to the display.
    pub async fn refresh_display(&self) {
        if self.is_locked().await {
            return;
        }
        let permit = self.display_render.clone().lock_owned().await;
        let generation = {
            let state = self.display_lock.lock().unwrap();
            if state.locked {
                return;
            }
            state.generation
        };
        let manager = self.manager.read().await.clone();
        let renderer = self.renderer.read().await.clone();
        // Keep the render gate through completion even if the caller is cancelled.
        match tokio::task::spawn_blocking(move || (permit, renderer.render_frame(&manager))).await {
            Ok((_permit, Ok(frame))) => self.publish_display_frame(generation, frame),
            Ok((_, Err(error))) => error!(%error, "Could not render display frame"),
            Err(error) => error!(%error, "Display render worker failed"),
        }
    }

    async fn arm_clipboard_route(
        &self,
        request_id: Option<uuid::Uuid>,
        hand: HandSide,
        timeout_ms: u64,
        restore_os_clipboard: bool,
    ) -> IpcResponse {
        // Observe queued selection events before acknowledging the request that
        // the CLI uses to issue its copy shortcut.
        let barrier = match &self.clipboard {
            Some(clipboard) => match clipboard.barrier().await {
                Ok(true) => Ok(()),
                Ok(false) => Err("Clipboard selection events are unavailable".into()),
                Err(error) => Err(error),
            },
            None => Ok(()),
        };
        if let Err(error) = barrier {
            IpcResponse::Error { error }
        } else {
            let expires =
                tokio::time::Instant::now() + Duration::from_millis(timeout_ms.min(60_000));
            let mut route = self.route_next_clipboard.write().await;
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
    }

    /// Restores the previously active OS clipboard item if a route-with-restore operation was performed.
    pub fn maybe_restore_os_clipboard_after_route(
        self: &Arc<Self>,
        restore_os: bool,
        target_hand: little_clip_core::HandSide,
        prior_left: Option<ClipItem>,
    ) {
        if restore_os
            && target_hand == little_clip_core::HandSide::Right
            && let Some(left_item) = prior_left
        {
            let restore_ctx = Arc::clone(self);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(80)).await;
                copy_to_os_clipboard(&left_item, &restore_ctx).await;
            });
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
    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        warn!(
            "Failed to create directory for last action state at {}: {err}",
            parent.display()
        );
        return;
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

    let clipboard = clipboard_events::Clipboard::spawn()?;
    let (clipboard, selections) = clipboard;
    let mut manager = SlotManager::default();
    manager.apply_config(&config.slots);
    let ctx = Arc::new(DaemonContext {
        config: RwLock::new(config.clone()),
        manager: RwLock::new(manager),
        renderer: RwLock::new(renderer),
        display_tx,
        self_copied_text: RwLock::new(Vec::new()),
        self_copied_image_hash: RwLock::new(None),
        clipboard: Some(clipboard),
        last_action: RwLock::new(load_last_action_from_disk()),
        display_lock: Mutex::default(),
        display_render: Arc::default(),
        lock_sources: tokio::sync::Mutex::new(0),
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
        watch_clipboard_events(clipboard_ctx, selections).await;
    });

    // Start animation ticker loop (paced to protect embedded MCU from hardware watchdog/overrun)
    let anim_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        loop {
            if anim_ctx.is_locked().await {
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
    let action = record_action.then(|| little_clip_core::ipc::LastAction::Op {
        op_name: op_name.clone(),
        selected_slots: selected_slots.clone(),
        secondary_slot,
        separator: separator.clone(),
    });
    let snapshot = ctx.manager.read().await.clone();
    let result = pipeline::execute(move || {
        evaluate_op(
            &snapshot,
            &op_name,
            selected_slots.as_deref(),
            secondary_slot,
            separator.as_deref(),
        )
        .map_err(|error| error.to_string())
    })
    .await;

    match result {
        Ok(new_item) => {
            if let Some(action) = action {
                save_last_action(&action, ctx).await;
            }
            apply_op_result_and_respond(new_item, ctx).await
        }
        Err(error) => IpcResponse::Error { error },
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
                IpcResponse::State(Box::new(mgr.clone()))
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
                                let node_telemetry = pipe_res.node_telemetry.clone();
                                let open_studios = pipe_res.open_studios.clone();
                                let external_opens = pipe_res.external_opens.clone();
                                let output_title = pipe_res
                                    .left_hand_item
                                    .as_ref()
                                    .map(|item| item.title.clone());
                                match apply_pipeline_result(pipe_res, &ctx).await {
                                    IpcResponse::OpResult { .. } | IpcResponse::Success { .. } => {
                                        IpcResponse::PipelineExecuted {
                                            node_previews,
                                            node_telemetry,
                                            output_title,
                                            duration_ms,
                                            open_studios,
                                            external_opens,
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
                let mut config = ctx.config.write().await;
                let new_config = little_clip_core::config::AppConfig::load();
                let new_audio_config = new_config.audio.clone();
                *config = new_config.clone();
                ctx.manager.write().await.apply_config(&new_config.slots);
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
                drop(config);
                ctx.refresh_display().await;
                IpcResponse::Success {
                    message: "Configuration reloaded and applied".to_string(),
                }
            }
            Ok(request @ (IpcRequest::Lock | IpcRequest::Unlock | IpcRequest::SetLocked(_))) => {
                let requested = matches!(request, IpcRequest::Lock | IpcRequest::SetLocked(true));
                ctx.set_locked(requested).await;
                let effective = ctx.is_locked().await;
                IpcResponse::Success {
                    message: if effective {
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
                    let guard = rolling_audio.read().map_err(|_| {
                        std::io::Error::other("rolling audio buffer lock was poisoned")
                    })?;
                    let (duration, snapshot_path) =
                        rolling_snapshots::publish(&get_rolling_snapshot_dir(), &guard)?;
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
                ctx.arm_clipboard_route(request_id, hand, timeout_ms, restore_os_clipboard)
                    .await
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
            Ok(IpcRequest::SetAutoAction { key, enabled }) => {
                ctx.set_auto_action(key, enabled).await
            }
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
        && thumbnail.is_none()
    {
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

fn schedule_audio_waveform_generation(item: &ClipItem, ctx: Arc<DaemonContext>) {
    if let little_clip_core::ClipPayload::AudioRef {
        ref path,
        duration_ms,
        ref waveform,
        ..
    } = item.payload
        && waveform.is_none()
    {
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
                        if let Some(spectral_colors) =
                            little_clip_core::compute_spectral_colors(&path_p2, 200, &audio_cfg_p2)
                        {
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

fn schedule_animation_generation(item: &ClipItem, ctx: Arc<DaemonContext>) {
    if let little_clip_core::ClipPayload::Animation {
        ref path,
        ref format,
        ref thumbnail,
        ..
    } = item.payload
        && thumbnail.is_none()
    {
        let item_id = item.id;
        let path_clone = path.clone();
        let format_clone = format.clone();
        let ctx_clone = Arc::clone(&ctx);

        tokio::task::spawn_blocking(move || {
            let anim_thumb = if format_clone == "GIF" || format_clone == "WEBP" {
                little_clip_core::animation::decode_animated_gif_or_webp(&path_clone, 200, 150, 25)
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

fn schedule_multi_image_thumbnail_generation(item: &ClipItem, ctx: Arc<DaemonContext>) {
    if let little_clip_core::ClipPayload::MultiImage {
        ref paths,
        ref preview_indices,
        ref thumbnails,
        ..
    } = item.payload
        && thumbnails.is_empty()
    {
        let item_id = item.id;
        let indices = preview_indices.clone();
        let paths_subset: Vec<PathBuf> = indices
            .iter()
            .filter_map(|&idx| paths.get(idx).cloned())
            .collect();

        tokio::task::spawn_blocking(move || {
            let mut thumbs = Vec::with_capacity(paths_subset.len());
            for path in paths_subset {
                if let Ok(bytes) = std::fs::read(&path)
                    && let Some(thumb) = little_clip_core::item::ImageThumbnail::generate_with_mime(
                        &bytes,
                        Some(&little_clip_core::item::guess_image_mime(&path)),
                        200,
                        160,
                    )
                {
                    thumbs.push(thumb);
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

fn schedule_async_previews(item: &ClipItem, ctx: Arc<DaemonContext>) {
    schedule_thumbnail_generation(item, Arc::clone(&ctx));
    schedule_audio_waveform_generation(item, Arc::clone(&ctx));
    schedule_animation_generation(item, Arc::clone(&ctx));
    schedule_multi_image_thumbnail_generation(item, ctx);
}

async fn watch_clipboard_events(
    ctx: Arc<DaemonContext>,
    mut selections: watch::Receiver<Option<Arc<clipboard_events::Incoming>>>,
) {
    while selections.changed().await.is_ok() {
        let Some(incoming) = selections.borrow_and_update().clone() else {
            continue;
        };
        match incoming.read().await {
            Ok((bytes, mime)) => {
                capture_clipboard_selection(
                    &ctx,
                    bytes,
                    mime,
                    (!incoming.initial).then_some(incoming.observed),
                    Some(incoming.serial),
                )
                .await;
            }
            Err(error) => warn!(%error, "failed to read clipboard selection"),
        }
    }
    ctx.route_next_clipboard.write().await.retire();
    warn!("Clipboard selection events unavailable; using external clipboard adapter");
    run_clipboard_poller(ctx).await;
}

async fn capture_clipboard_selection(
    ctx: &Arc<DaemonContext>,
    bytes: Vec<u8>,
    mime: String,
    observed: Option<tokio::time::Instant>,
    serial: Option<u64>,
) {
    let trim_tracking = ctx.config.read().await.auto_actions.trim_link_tracking;
    let prepared = tokio::task::spawn_blocking(move || {
        if little_clip_core::item::image_extension_for_mime(&mime).is_some() {
            return ClipItem::from_image_bytes(bytes, &mime).map(|item| (item, None));
        }
        let text = String::from_utf8(bytes).ok()?;
        if text.is_empty() {
            return None;
        }
        if trim_tracking && let Some(cleaned) = little_clip_ops::clean_url_tracking(&text) {
            return Some((
                ClipItem::from_text(cleaned),
                Some(ClipItem::from_text(text)),
            ));
        }
        Some((ClipItem::from_text(text), None))
    })
    .await;
    let Ok(Some((item, original_untracked))) = prepared else {
        return;
    };
    let was_cleaned = original_untracked.is_some();
    let (prior_left, target_hand, restore_os) = {
        let mut manager = ctx.manager.write().await;
        // Decoder completion cannot publish an offer superseded in the meantime.
        if serial.is_some_and(|serial| {
            ctx.clipboard
                .as_ref()
                .is_none_or(|clipboard| !clipboard.is_current(serial))
        }) {
            return;
        }
        let (hand, restore) = match observed {
            Some(observed) => ctx
                .route_next_clipboard
                .write()
                .await
                .take_observed(observed, tokio::time::Instant::now()),
            None => (HandSide::Left, false),
        };
        let prior = manager.left_hand().cloned();
        if let Some(original) = original_untracked {
            manager.undo = Some(original);
        }
        manager.push_to_hand(item.clone(), hand);
        (prior, hand, restore)
    };
    schedule_async_previews(&item, Arc::clone(ctx));
    if was_cleaned && let Some(text) = item.to_clipboard_text() {
        copy_cleaned_clipboard_text(text, ctx).await;
    }
    ctx.refresh_display().await;
    ctx.maybe_restore_os_clipboard_after_route(restore_os, target_hand, prior_left);
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
                && img_out.status.success()
                && !img_out.stdout.is_empty()
            {
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

                    capture_clipboard_selection(
                        &ctx,
                        img_out.stdout,
                        image_mime,
                        Some(tokio::time::Instant::now()),
                        None,
                    )
                    .await;
                    continue;
                } else {
                    // Same image as last tick, no update needed
                    continue;
                }
            }
        }

        // 2. Fall back to text / file URI polling
        if let Ok(output) = tokio::process::Command::new("wl-paste")
            .arg("-n")
            .output()
            .await
            && output.status.success()
            && let Ok(text) = String::from_utf8(output.stdout)
        {
            if ctx.is_self_copied_text(&text).await {
                last_clipboard_text = text;
                continue;
            }

            // Check if there is an active override routing to Right Hand.
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
                            let cand = little_clip_core::item::extract_local_file_paths(&text);
                            !cand.is_empty() && cand == *files
                        }
                        little_clip_core::ClipPayload::AudioRef { path, .. } => {
                            let cand = little_clip_core::item::extract_local_file_paths(&text);
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

            // A pending route cannot turn a previously observed value
            // into a new copy. Polling cannot distinguish equal offers.
            if !text.is_empty() && text != last_clipboard_text {
                last_clipboard_text = text.clone();
                last_image_hash = None;

                capture_clipboard_selection(
                    &ctx,
                    text.into_bytes(),
                    "text/plain;charset=utf-8".into(),
                    Some(tokio::time::Instant::now()),
                    None,
                )
                .await;
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

async fn copy_cleaned_clipboard_text(text: String, ctx: &DaemonContext) {
    ctx.add_self_copied_text(text.clone()).await;
    write_os_clipboard(
        text.into_bytes(),
        "text/plain;charset=utf-8",
        "cleaned text",
        ctx,
    )
    .await;
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
            copy_image(bytes.clone(), mime.clone(), ctx).await
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

async fn write_os_clipboard(
    bytes: Vec<u8>,
    mime: impl Into<String>,
    description: &str,
    ctx: &DaemonContext,
) {
    let mime = mime.into();
    let bytes: Arc<[u8]> = bytes.into();
    if let Some(clipboard) = &ctx.clipboard {
        match clipboard.publish(bytes.clone(), mime.clone()).await {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                warn!(%error, %description, "failed to publish clipboard selection");
                return;
            }
        }
    }
    match tokio::task::spawn_blocking(move || copy_bytes_to_system_clipboard(&bytes, &mime)).await {
        Ok(Err(error)) => warn!(%error, %description, "failed to copy to system clipboard"),
        Err(error) => warn!(%error, %description, "clipboard copy worker failed"),
        Ok(Ok(())) => {}
    }
}

async fn copy_image(bytes: Vec<u8>, mime: String, ctx: &DaemonContext) {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    *ctx.self_copied_image_hash.write().await = Some(hasher.finish());
    write_os_clipboard(bytes, mime, "image", ctx).await;
}

async fn copy_image_file(path: &std::path::Path, ctx: &DaemonContext) {
    let mime = little_clip_core::item::guess_image_mime(path);
    let path = path.to_owned();
    match tokio::task::spawn_blocking(move || std::fs::read(path)).await {
        Ok(Ok(bytes)) => copy_image(bytes, mime, ctx).await,
        Ok(Err(error)) => warn!(%error, "failed to read image file for clipboard copy"),
        Err(error) => warn!(%error, "clipboard image read worker failed"),
    }
}

async fn copy_text(text: &str, description: &str, ctx: &DaemonContext) {
    ctx.add_self_copied_text(text.to_owned()).await;
    write_os_clipboard(
        text.as_bytes().to_vec(),
        "text/plain;charset=utf-8",
        description,
        ctx,
    )
    .await;
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
    write_os_clipboard(uri_list.into_bytes(), "text/uri-list", "files", ctx).await;
}

async fn copy_audio_path(path: &std::path::Path, ctx: &DaemonContext) {
    let Ok(uri) = file_uri_list(&[path]) else {
        warn!(path = %path.display(), "failed to encode audio path as clipboard URI");
        return;
    };
    ctx.add_self_copied_text(uri.clone()).await;
    ctx.add_self_copied_text(path.to_string_lossy().into_owned())
        .await;
    write_os_clipboard(uri.into_bytes(), "text/uri-list", "audio path", ctx).await;
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

    pub(super) fn test_context(mut manager: SlotManager) -> Arc<DaemonContext> {
        let config = little_clip_core::AppConfig::default();
        manager.apply_config(&config.slots);
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
            clipboard: None,
            last_action: RwLock::new(None),
            display_lock: Mutex::default(),
            display_render: Arc::default(),
            lock_sources: tokio::sync::Mutex::new(0),
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
if [ -e "$CLIPD_TEST_CLIPBOARD/wait-for-runtime" ]; then
    attempts=0
    while [ ! -e "$CLIPD_TEST_CLIPBOARD/runtime-progress" ]; do
        [ "$attempts" -lt 100 ] || exit 1
        attempts=$((attempts + 1))
        /bin/sleep 0.01
    done
fi
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
            .env("XDG_CONFIG_HOME", directory.path().join("config-home"))
            .current_dir(directory.path())
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

    async fn test_ipc_request(
        writer: &mut tokio::net::unix::OwnedWriteHalf,
        lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
        request: IpcRequest,
    ) -> IpcResponse {
        writer
            .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
            .await
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str(&response).unwrap()
    }

    #[tokio::test]
    async fn reloaded_history_limits_apply_to_later_ipc_pushes_and_snapshots() {
        if isolated_clipboard_test(
            "tests::reloaded_history_limits_apply_to_later_ipc_pushes_and_snapshots",
        ) {
            return;
        }
        let ctx = test_context(SlotManager::default());
        assert_eq!(
            ctx.manager.read().await.history_config,
            little_clip_core::config::SlotsConfig::default()
        );
        let (client, server) = UnixStream::pair().unwrap();
        let handler = tokio::spawn(handle_client(server, Arc::clone(&ctx)));
        let (reader, mut writer) = client.into_split();
        let mut lines = BufReader::new(reader).lines();
        let priors = [
            SlotId::LeftPrior1,
            SlotId::LeftPrior2,
            SlotId::LeftPrior3,
            SlotId::RightPrior1,
            SlotId::RightPrior2,
        ];
        for (left, right) in [(0, 0), (2, 2), (9, 9), (1, 0)] {
            std::fs::write(
                "config.toml",
                format!("[slots]\nleft_priors = {left}\nright_priors = {right}\n"),
            )
            .unwrap();
            let response =
                test_ipc_request(&mut writer, &mut lines, IpcRequest::ReloadConfig).await;
            assert!(
                matches!(response, IpcResponse::Success { .. }),
                "{response:?}"
            );
            let expected = little_clip_core::config::SlotsConfig {
                left_priors: left.min(3),
                right_priors: right.min(2),
            };
            let IpcResponse::StateSummary(summary) =
                test_ipc_request(&mut writer, &mut lines, IpcRequest::GetStateSummary).await
            else {
                panic!("expected state summary");
            };
            assert_eq!(summary.history_config, expected);
            let enabled = [left >= 1, left >= 2, left >= 3, right >= 1, right >= 2];
            for (slot, enabled) in priors.into_iter().zip(enabled) {
                if !enabled {
                    assert!(
                        ctx.manager.read().await.get(slot).is_none(),
                        "reload retained disabled {slot:?}"
                    );
                }
            }
            for index in 0..4 {
                for hand in [HandSide::Left, HandSide::Right] {
                    let response = test_ipc_request(
                        &mut writer,
                        &mut lines,
                        IpcRequest::PushText {
                            text: format!("history push {index} {hand:?}"),
                            hand,
                        },
                    )
                    .await;
                    assert!(
                        matches!(response, IpcResponse::Success { .. }),
                        "{response:?}"
                    );
                }
            }
            let IpcResponse::State(state) =
                test_ipc_request(&mut writer, &mut lines, IpcRequest::GetState).await
            else {
                panic!("expected full state");
            };
            assert_eq!(state.history_config, expected);
            for (slot, enabled) in priors.into_iter().zip(enabled) {
                assert_eq!(
                    state.get(slot).is_some(),
                    enabled,
                    "incorrect {slot:?} after reload to {left}/{right}"
                );
            }
        }
        handler.abort();
    }

    #[tokio::test]
    async fn auto_action_setter_persists_before_acknowledging_and_survives_reload() {
        if isolated_clipboard_test(
            "tests::auto_action_setter_persists_before_acknowledging_and_survives_reload",
        ) {
            return;
        }
        let path = PathBuf::from("config.toml");
        std::fs::write(&path, "# keep this comment\n[workbench]\nappearance = 'light'\n[auto_actions]\ntrim_link_tracking = true\n").unwrap();
        let ctx = test_context(SlotManager::default());
        let initial = little_clip_core::AppConfig::load_from_path(&path).unwrap();
        *ctx.config.write().await = initial.clone();
        ctx.renderer.write().await.set_config(initial);
        let (client, server) = UnixStream::pair().unwrap();
        let handler = tokio::spawn(handle_client(server, Arc::clone(&ctx)));
        let (reader, mut writer) = client.into_split();
        let mut lines = BufReader::new(reader).lines();
        for enabled in [false, true] {
            let response = test_ipc_request(
                &mut writer,
                &mut lines,
                IpcRequest::SetAutoAction {
                    key: "trim_link_tracking".into(),
                    enabled,
                },
            )
            .await;
            assert!(
                matches!(response, IpcResponse::Success { .. }),
                "{response:?}"
            );
            assert_eq!(
                little_clip_core::AppConfig::load_from_path(&path)
                    .unwrap()
                    .auto_actions
                    .trim_link_tracking,
                enabled
            );
            assert_eq!(
                ctx.config.read().await.auto_actions.trim_link_tracking,
                enabled
            );
            assert_eq!(
                ctx.renderer
                    .read()
                    .await
                    .config
                    .auto_actions
                    .trim_link_tracking,
                enabled
            );
            let response =
                test_ipc_request(&mut writer, &mut lines, IpcRequest::ReloadConfig).await;
            assert!(
                matches!(response, IpcResponse::Success { .. }),
                "{response:?}"
            );
            assert_eq!(
                ctx.config.read().await.auto_actions.trim_link_tracking,
                enabled
            );
            assert_eq!(
                ctx.renderer
                    .read()
                    .await
                    .config
                    .auto_actions
                    .trim_link_tracking,
                enabled
            );
            assert!(
                std::fs::read_to_string(&path)
                    .unwrap()
                    .contains("# keep this comment")
            );
            assert_eq!(
                ctx.config.read().await.workbench.appearance,
                little_clip_core::config::WorkbenchAppearance::Light
            );
        }
        // Failed persistence must not publish an optimistic runtime value.
        std::fs::write(&path, "invalid [ config").unwrap();
        let response = test_ipc_request(
            &mut writer,
            &mut lines,
            IpcRequest::SetAutoAction {
                key: "trim_link_tracking".into(),
                enabled: false,
            },
        )
        .await;
        assert!(matches!(response, IpcResponse::Error { .. }));
        assert!(ctx.config.read().await.auto_actions.trim_link_tracking);
        assert!(
            ctx.renderer
                .read()
                .await
                .config
                .auto_actions
                .trim_link_tracking
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid [ config");
        let response = test_ipc_request(
            &mut writer,
            &mut lines,
            IpcRequest::SetAutoAction {
                key: "unknown_action".into(),
                enabled: false,
            },
        )
        .await;
        assert!(matches!(response, IpcResponse::Error { .. }));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid [ config");
        drop(writer);
        drop(lines);
        handler.await.unwrap();
    }

    #[tokio::test]
    async fn blank_frame_bypasses_content_render_and_rejects_stale_publication() {
        let ctx = test_context(SlotManager::default());
        let frames = ctx.display_tx.subscribe();
        let active_render = ctx.display_render.lock().await;
        let previous_generation = ctx.display_lock.lock().unwrap().generation;
        tokio::time::timeout(Duration::from_secs(2), ctx.set_locked(true))
            .await
            .expect("blanking must bypass the content render gate");
        let blank = frames.borrow().clone().expect("blank frame published");
        ctx.publish_display_frame(previous_generation, vec![1, 2, 3]);
        assert!(Arc::ptr_eq(frames.borrow().as_ref().unwrap(), &blank));

        drop(active_render);
        ctx.set_locked(false).await;
        let unlocked = frames.borrow().clone().expect("unlocked frame published");
        assert!(!Arc::ptr_eq(&unlocked, &blank));
        ctx.publish_display_frame(previous_generation, vec![1, 2, 3]);
        assert!(Arc::ptr_eq(frames.borrow().as_ref().unwrap(), &unlocked));
    }

    #[tokio::test]
    async fn resume_cannot_clear_another_display_lock_source() {
        for source in [LockSource::Manual, LockSource::Shell] {
            let ctx = test_context(SlotManager::default());
            ctx.set_lock_source(source, true).await;
            ctx.set_lock_source(LockSource::Sleep, true).await;
            ctx.resume_from_sleep(Some(false)).await;
            assert!(ctx.is_locked().await, "resume cleared an independent lock");
            ctx.set_lock_source(source, false).await;
            assert!(!ctx.is_locked().await);
        }
        let ctx = test_context(SlotManager::default());
        ctx.set_lock_source(LockSource::Session, true).await;
        ctx.set_lock_source(LockSource::Sleep, true).await;
        ctx.resume_from_sleep(Some(false)).await;
        assert!(
            !ctx.is_locked().await,
            "a fresh unlocked hint should retire a missed unlock event"
        );
        for hint in [Some(true), None] {
            ctx.set_lock_source(LockSource::Sleep, true).await;
            ctx.resume_from_sleep(hint).await;
            assert!(ctx.is_locked().await);
            ctx.set_lock_source(LockSource::Session, false).await;
            assert!(!ctx.is_locked().await);
        }
    }

    #[tokio::test]
    async fn clearing_session_or_manual_lock_during_sleep_keeps_display_blank() {
        let ctx = test_context(SlotManager::default());
        ctx.set_locked(true).await;
        ctx.set_lock_source(LockSource::Session, true).await;
        ctx.set_lock_source(LockSource::Sleep, true).await;
        ctx.set_locked(false).await;
        ctx.set_lock_source(LockSource::Session, false).await;
        assert!(ctx.is_locked().await);
        ctx.resume_from_sleep(Some(false)).await;
        assert!(!ctx.is_locked().await);
        ctx.set_lock_source(LockSource::Shell, true).await;
        ctx.set_locked(false).await;
        assert!(ctx.is_locked().await);
    }

    #[tokio::test]
    async fn cleaned_url_copy_is_registered_for_echo_suppression() {
        if isolated_clipboard_test("tests::cleaned_url_copy_is_registered_for_echo_suppression") {
            return;
        }
        let directory = PathBuf::from(std::env::var_os("CLIPD_TEST_CLIPBOARD").unwrap());
        let ctx = test_context(SlotManager::default());
        copy_cleaned_clipboard_text(CLEAN_URL.to_owned(), &ctx).await;
        assert_eq!(
            std::fs::read(directory.join("bytes")).unwrap(),
            CLEAN_URL.as_bytes()
        );
        assert!(ctx.is_self_copied_text(CLEAN_URL).await);
        assert!(ctx.is_self_copied_text(&format!("  {CLEAN_URL}\r\n")).await);
    }

    #[tokio::test]
    async fn right_route_ignores_previously_observed_clipboard_text() {
        use std::os::unix::fs::PermissionsExt;

        if isolated_clipboard_test("tests::right_route_ignores_previously_observed_clipboard_text")
        {
            return;
        }
        let directory = PathBuf::from(std::env::var_os("CLIPD_TEST_CLIPBOARD").unwrap());
        let adapter = directory.join("wl-paste");
        std::fs::write(
            &adapter,
            r#"#!/bin/sh
[ "$1" = "--list-types" ] && exit 1
/bin/cat "$CLIPD_TEST_CLIPBOARD/selection"
echo . >> "$CLIPD_TEST_CLIPBOARD/reads"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&adapter, std::fs::Permissions::from_mode(0o700)).unwrap();
        let selection = directory.join("selection");
        std::fs::write(&selection, "old selection").unwrap();

        let ctx = test_context(SlotManager::default());
        let watcher = tokio::spawn(run_clipboard_poller(Arc::clone(&ctx)));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let left = ctx
                    .manager
                    .read()
                    .await
                    .left_hand()
                    .and_then(extract_clipboard_text);
                if left.as_deref() == Some("old selection") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        let request_id = uuid::Uuid::new_v4();
        let now = tokio::time::Instant::now();
        ctx.route_next_clipboard.write().await.install(
            Some(request_id),
            HandSide::Right,
            now + Duration::from_secs(10),
            false,
        );
        let reads = directory.join("reads");
        let previous = std::fs::metadata(&reads).unwrap().len();
        tokio::time::timeout(Duration::from_secs(5), async {
            while std::fs::metadata(&reads).unwrap().len() < previous + 4 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let status = ctx
            .route_next_clipboard
            .read()
            .await
            .status(request_id, tokio::time::Instant::now());
        assert_eq!(status, little_clip_core::ipc::ClipboardRouteStatus::Pending);
        assert!(ctx.manager.read().await.right_hand().is_none());

        std::fs::write(&selection, "new selection").unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let right = ctx
                    .manager
                    .read()
                    .await
                    .right_hand()
                    .and_then(extract_clipboard_text);
                if right.as_deref() == Some("new selection") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        watcher.abort();
    }

    #[tokio::test]
    async fn execute_op_publishes_worker_result_to_left_hand_and_clipboard() {
        if isolated_clipboard_test(
            "tests::execute_op_publishes_worker_result_to_left_hand_and_clipboard",
        ) {
            return;
        }
        let mut manager = SlotManager::default();
        manager.push_to_hand(ClipItem::from_text("mixed Case"), HandSide::Left);
        let ctx = test_context(manager);
        let (client, server) = UnixStream::pair().unwrap();
        let handler = tokio::spawn(handle_client(server, Arc::clone(&ctx)));
        let (reader, mut writer) = client.into_split();
        let mut lines = BufReader::new(reader).lines();
        let response = test_ipc_request(
            &mut writer,
            &mut lines,
            IpcRequest::ExecuteOp {
                op_name: "uppercase".into(),
                selected_slots: None,
                secondary_slot: None,
                separator: None,
            },
        )
        .await;
        let IpcResponse::OpResult { item } = response else {
            panic!("Unexpected operation response: {response:?}");
        };
        assert_eq!(extract_clipboard_text(&item).as_deref(), Some("MIXED CASE"));
        assert_eq!(
            ctx.manager
                .read()
                .await
                .left_hand()
                .and_then(extract_clipboard_text)
                .as_deref(),
            Some("MIXED CASE")
        );
        let directory = PathBuf::from(std::env::var_os("CLIPD_TEST_CLIPBOARD").unwrap());
        assert_eq!(
            std::fs::read(directory.join("bytes")).unwrap(),
            b"MIXED CASE"
        );
        drop(writer);
        drop(lines);
        handler.await.unwrap();
    }

    #[tokio::test]
    async fn clipboard_adapter_does_not_block_current_thread_runtime() {
        if isolated_clipboard_test("tests::clipboard_adapter_does_not_block_current_thread_runtime")
        {
            return;
        }
        let directory = PathBuf::from(std::env::var_os("CLIPD_TEST_CLIPBOARD").unwrap());
        std::fs::write(directory.join("wait-for-runtime"), b"").unwrap();
        let ctx = test_context(SlotManager::default());
        tokio::join!(copy_text("test clipboard", "test", &ctx), async {
            std::fs::write(directory.join("runtime-progress"), b"").unwrap();
        });
        assert_eq!(
            std::fs::read(directory.join("bytes")).unwrap(),
            b"test clipboard"
        );
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
            let response = test_ipc_request(&mut writer, &mut lines, request).await;
            assert!(
                matches!(response, IpcResponse::Success { .. }),
                "{response:?}"
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
        let response = test_ipc_request(&mut writer, &mut lines, request).await;
        assert!(
            matches!(response, IpcResponse::Success { .. }),
            "{response:?}"
        );
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
        let mut lines = BufReader::new(reader).lines();
        let response = test_ipc_request(&mut writer, &mut lines, request).await;
        let completed = destination.join("first.bin");
        match response {
            IpcResponse::Error { error } => {
                assert!(error.contains(completed.to_str().unwrap()), "{error}");
                assert!(error.contains(missing.to_str().unwrap()), "{error}");
            }
            other => panic!("expected a partial-save failure, got {other:?}"),
        }
        assert_eq!(std::fs::read(completed).unwrap(), b"completed bytes");
        assert_eq!(ctx.manager.read().await.left_hand().unwrap().id, id);
        drop(writer);
        drop(lines);
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
