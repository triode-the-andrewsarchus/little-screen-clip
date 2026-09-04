use little_clip_core::{
    ipc::{IpcRequest, IpcResponse},
    ClipItem, SlotManager,
};
use little_clip_driver::TrofeoDriver;
use little_clip_render::ScreenRenderer;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

pub struct DaemonContext {
    pub manager: RwLock<SlotManager>,
    pub renderer: RwLock<ScreenRenderer>,
    pub display_tx: mpsc::Sender<Vec<u8>>,
    pub self_copied_text: RwLock<String>,
    pub self_copied_image_hash: RwLock<Option<u64>>,
    pub last_action: RwLock<Option<little_clip_core::ipc::LastAction>>,
}


fn get_last_action_path() -> PathBuf {
    if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
        PathBuf::from(config_home).join("little-screen-clip").join("last_action.json")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".config").join("little-screen-clip").join("last_action.json")
    } else {
        PathBuf::from("/tmp/little-clip-last-action.json")
    }
}

async fn save_last_action(action: &little_clip_core::ipc::LastAction, ctx: &DaemonContext) {
    let mut lock = ctx.last_action.write().await;
    *lock = Some(action.clone());
    let path = get_last_action_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(action) {
        let _ = std::fs::write(path, json);
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

impl DaemonContext {
    pub async fn refresh_display(&self) {
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
        let _ = self.display_tx.send(frame).await;
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
        ScreenRenderer::fallback(config)
    });

    let driver = match TrofeoDriver::open() {
        Ok(drv) => {
            info!(
                "Connected to Trofeo Vision display (PM={}, SUB={}, cmd={})",
                drv.info.pm, drv.info.sub, drv.info.chunk_cmd
            );
            Some(drv)
        }
        Err(e) => {
            warn!("Trofeo USB display not connected or accessible: {}. Running in virtual mode.", e);
            None
        }
    };

    let (display_tx, display_rx) = mpsc::channel::<Vec<u8>>(16);

    // Spawn display worker with 800ms keepalive loop for volatile LY panel
    tokio::spawn(async move {
        run_display_worker(driver, display_rx).await;
    });

    let ctx = Arc::new(DaemonContext {
        manager: RwLock::new(SlotManager::default()),
        renderer: RwLock::new(renderer),
        display_tx,
        self_copied_text: RwLock::new(String::new()),
        self_copied_image_hash: RwLock::new(None),
        last_action: RwLock::new(load_last_action_from_disk()),
    });


    // Push initial blank state to screen
    ctx.refresh_display().await;

    let clipboard_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        run_clipboard_poller(clipboard_ctx).await;
    });

    // Start animation ticker loop (paced to protect embedded MCU from hardware watchdog/overrun)
    let anim_ctx = Arc::clone(&ctx);
    tokio::spawn(async move {
        loop {
            let (interval_ms, has_anim) = {
                let r = anim_ctx.renderer.read().await;
                let mgr = anim_ctx.manager.read().await;
                (r.config.display.animation_interval_ms.max(50), mgr.has_active_animation())
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

async fn run_display_worker(mut driver: Option<TrofeoDriver>, mut rx: mpsc::Receiver<Vec<u8>>) {
    let mut last_frame: Option<Vec<u8>> = None;
    let mut last_send = tokio::time::Instant::now();
    let keepalive_interval = Duration::from_millis(800);
    // Lower bound between USB writes to prevent overwhelming the embedded JPEG decoder:
    let min_usb_gap = Duration::from_millis(100);

    loop {
        tokio::select! {
            maybe_frame = rx.recv() => {
                match maybe_frame {
                    Some(mut new_frame) => {
                        // Drain any queued stale frames to avoid burst flooding the USB bus
                        while let Ok(freshest) = rx.try_recv() {
                            new_frame = freshest;
                        }

                        // Ensure Trofeo MCU has finished decoding prior frame
                        let elapsed = last_send.elapsed();
                        if elapsed < min_usb_gap {
                            tokio::time::sleep(min_usb_gap - elapsed).await;
                        }

                        if let Some(drv) = driver.as_mut() {
                            if let Err(e) = drv.send_frame(&new_frame) {
                                warn!("USB send_frame error: {}. Attempting reconnect in 1s...", e);
                                tokio::time::sleep(Duration::from_millis(1000)).await;
                                match TrofeoDriver::open() {
                                    Ok(new_drv) => {
                                        info!("Successfully reconnected to Trofeo USB display");
                                        *drv = new_drv;
                                    }
                                    Err(re_err) => {
                                        warn!("USB Reconnect failed: {}", re_err);
                                    }
                                }
                            }
                        }
                        last_frame = Some(new_frame);
                        last_send = tokio::time::Instant::now();
                    }
                    None => break,
                }
            }
            _ = tokio::time::sleep(keepalive_interval) => {
                if last_send.elapsed() >= keepalive_interval {
                    if let (Some(drv), Some(ref frame)) = (driver.as_mut(), last_frame.as_ref()) {
                        let _ = drv.send_frame(frame);
                        last_send = tokio::time::Instant::now();
                    }
                }
            }
        }
    }
}

async fn handle_client(stream: UnixStream, ctx: Arc<DaemonContext>) {
    info!("Client connected to IPC socket");
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        info!("IPC request received: {}", line);
        let req: Result<IpcRequest, _> = serde_json::from_str(&line);
        let resp = match req {
            Ok(IpcRequest::GetState) => {
                info!("Processing GetState...");
                let mgr = ctx.manager.read().await;
                info!("Got manager read lock for GetState");
                IpcResponse::State(mgr.clone())
            }
            Ok(IpcRequest::PushText { text, hand }) => {
                let item = ClipItem::from_text(text);
                schedule_async_previews(&item, Arc::clone(&ctx));
                {
                    let mut mgr = ctx.manager.write().await;
                    mgr.push_to_hand(item, hand);
                }
                ctx.refresh_display().await;
                IpcResponse::Success {
                    message: format!("Pushed to {:?}", hand),
                }
            }
            Ok(IpcRequest::PushImage { bytes, mime, hand }) => {
                if let Some(item) = ClipItem::from_image_bytes(bytes, mime) {
                    schedule_async_previews(&item, Arc::clone(&ctx));
                    {
                        let mut mgr = ctx.manager.write().await;
                        mgr.push_to_hand(item, hand);
                    }
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
                let item = ClipItem::from_single_file(&path);
                schedule_async_previews(&item, Arc::clone(&ctx));
                {
                    let mut mgr = ctx.manager.write().await;
                    mgr.push_to_hand(item, hand);
                }
                ctx.refresh_display().await;
                IpcResponse::Success {
                    message: format!("Pushed file {:?} to {:?}", path, hand),
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
                let result = {
                    let mgr = ctx.manager.read().await;
                    if let Some(ref slots) = selected_slots {
                        let items: Vec<&ClipItem> = slots.iter().filter_map(|s| mgr.get(*s)).collect();
                        if items.is_empty() {
                            Err(little_clip_ops::OpError::ExecutionFailed(
                                "Selected slots are empty".to_string(),
                            ))
                        } else {
                            little_clip_ops::execute_multi_op(&op_name, &items, separator.as_deref())
                        }
                    } else {
                        let left_item = mgr.left_hand().cloned();
                        if let Some(left) = left_item {
                            if let Some(sec_slot) = secondary_slot {
                                if let Some(sec_item) = mgr.get(sec_slot) {
                                    little_clip_ops::execute_binary_op(&op_name, &left, sec_item, separator.as_deref())
                                } else {
                                    Err(little_clip_ops::OpError::ExecutionFailed(format!(
                                        "Slot {} is empty",
                                        sec_slot.label()
                                    )))
                                }
                            } else if let Some(right) = mgr.right_hand() {
                                // Try binary with RightHand as natural default operand, or fallback to unary
                                little_clip_ops::execute_binary_op(&op_name, &left, right, separator.as_deref())
                                    .or_else(|_| little_clip_ops::execute_unary_op(&op_name, &left))
                            } else {
                                little_clip_ops::execute_unary_op(&op_name, &left)
                            }
                        } else {
                            Err(little_clip_ops::OpError::ExecutionFailed(
                                "Left Hand is empty".to_string(),
                            ))
                        }
                    }
                };

                match result {
                    Ok(new_item) => {
                        save_last_action(
                            &little_clip_core::ipc::LastAction::Op {
                                op_name: op_name.clone(),
                                selected_slots: selected_slots.clone(),
                                secondary_slot,
                                separator: separator.clone(),
                            },
                            &ctx,
                        )
                        .await;

                        schedule_async_previews(&new_item, Arc::clone(&ctx));
                        let text_opt = extract_clipboard_text(&new_item);
                        if let Some(ref t) = text_opt {
                            let mut self_text = ctx.self_copied_text.write().await;
                            *self_text = t.clone();
                        }
                        copy_to_os_clipboard(&new_item, &ctx).await;
                        {
                            let mut mgr = ctx.manager.write().await;
                            mgr.apply_op_result(new_item.clone());
                        }

                        ctx.refresh_display().await;
                        IpcResponse::OpResult { item: new_item }
                    }
                    Err(e) => IpcResponse::Error {
                        error: e.to_string(),
                    },
                }
            }
            Ok(IpcRequest::ExecuteRecipe { recipe_name }) => {
                if let Some(recipe) = little_clip_ops::find_recipe_by_name(&recipe_name) {
                    let mgr_snapshot = { ctx.manager.read().await.clone() };
                    let executor = little_clip_ops::PipelineExecutor::new(&recipe.graph, &mgr_snapshot);
                    match executor.execute() {
                        Ok(pipe_res) => {
                            save_last_action(
                                &little_clip_core::ipc::LastAction::Recipe {
                                    recipe_name: recipe_name.clone(),
                                },
                                &ctx,
                            )
                            .await;
                            apply_pipeline_result(pipe_res, &ctx).await
                        }
                        Err(e) => IpcResponse::Error {
                            error: format!("Recipe execution failed: {}", e),
                        },
                    }
                } else {
                    IpcResponse::Error {
                        error: format!("Recipe not found: {}", recipe_name),
                    }
                }
            }
            Ok(IpcRequest::ExecuteRecipeWithPrompt { recipe_name, prompt_text }) => {
                if let Some(recipe) = little_clip_ops::find_recipe_by_name(&recipe_name) {
                    let mgr_snapshot = { ctx.manager.read().await.clone() };
                    let executor = little_clip_ops::PipelineExecutor::new(&recipe.graph, &mgr_snapshot)
                        .with_single_prompt(prompt_text);
                    match executor.execute() {
                        Ok(pipe_res) => {
                            save_last_action(
                                &little_clip_core::ipc::LastAction::Recipe {
                                    recipe_name: recipe_name.clone(),
                                },
                                &ctx,
                            )
                            .await;
                            apply_pipeline_result(pipe_res, &ctx).await
                        }
                        Err(e) => IpcResponse::Error {
                            error: format!("Recipe execution failed: {}", e),
                        },
                    }
                } else {
                    IpcResponse::Error {
                        error: format!("Recipe not found: {}", recipe_name),
                    }
                }
            }
            Ok(IpcRequest::GetLastAction) => {
                let act = ctx.last_action.read().await.clone();
                IpcResponse::LastAction(act)
            }
            Ok(IpcRequest::ExecuteLastAction { prompt_text }) => {
                let action = ctx.last_action.read().await.clone();
                match action {
                    Some(little_clip_core::ipc::LastAction::Recipe { recipe_name }) => {
                        if let Some(recipe) = little_clip_ops::find_recipe_by_name(&recipe_name) {
                            let mgr_snapshot = { ctx.manager.read().await.clone() };
                            let executor = little_clip_ops::PipelineExecutor::new(&recipe.graph, &mgr_snapshot)
                                .with_single_prompt(prompt_text);
                            match executor.execute() {
                                Ok(pipe_res) => apply_pipeline_result(pipe_res, &ctx).await,
                                Err(e) => IpcResponse::Error {
                                    error: format!("Recipe execution failed: {}", e),
                                },
                            }
                        } else {
                            IpcResponse::Error {
                                error: format!("Recipe not found: {}", recipe_name),
                            }
                        }
                    }
                    Some(little_clip_core::ipc::LastAction::Op { op_name, selected_slots, secondary_slot, separator }) => {
                        let mgr = ctx.manager.read().await;
                        let result: Result<little_clip_core::ClipItem, little_clip_ops::OpError> = {
                            if let Some(slots) = selected_slots {
                                let items: Vec<&little_clip_core::ClipItem> = slots.iter().filter_map(|s| mgr.get(*s)).collect();
                                if items.is_empty() {
                                    Err(little_clip_ops::OpError::ExecutionFailed("Selected slots are empty".to_string()))
                                } else {
                                    little_clip_ops::execute_multi_op(&op_name, &items, separator.as_deref())
                                }
                            } else {
                                let left_item = mgr.left_hand().cloned();
                                if let Some(left) = left_item {
                                    if let Some(sec_slot) = secondary_slot {
                                        if let Some(sec_item) = mgr.get(sec_slot) {
                                            little_clip_ops::execute_binary_op(&op_name, &left, sec_item, separator.as_deref())
                                        } else {
                                            Err(little_clip_ops::OpError::ExecutionFailed(format!(
                                                "Slot {} is empty",
                                                sec_slot.label()
                                            )))
                                        }
                                    } else if let Some(right) = mgr.right_hand() {
                                        little_clip_ops::execute_binary_op(&op_name, &left, right, separator.as_deref())
                                            .or_else(|_| little_clip_ops::execute_unary_op(&op_name, &left))
                                    } else {
                                        little_clip_ops::execute_unary_op(&op_name, &left)
                                    }
                                } else {
                                    Err(little_clip_ops::OpError::ExecutionFailed("Left Hand is empty".to_string()))
                                }
                            }
                        };

                        match result {
                            Ok(new_item) => {
                                schedule_async_previews(&new_item, Arc::clone(&ctx));
                                let text_opt = extract_clipboard_text(&new_item);
                                if let Some(ref t) = text_opt {
                                    let mut self_text = ctx.self_copied_text.write().await;
                                    *self_text = t.clone();
                                }
                                copy_to_os_clipboard(&new_item, &ctx).await;
                                {
                                    let mut mgr_mut = ctx.manager.write().await;
                                    mgr_mut.apply_op_result(new_item.clone());
                                }

                                ctx.refresh_display().await;
                                IpcResponse::OpResult { item: new_item }
                            }
                            Err(e) => IpcResponse::Error {
                                error: e.to_string(),
                            },
                        }
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
                        let executor = little_clip_ops::PipelineExecutor::new(&graph, &mgr_snapshot);
                        match executor.execute() {
                            Ok(pipe_res) => apply_pipeline_result(pipe_res, &ctx).await,
                            Err(e) => IpcResponse::Error {
                                error: format!("Graph execution failed: {}", e),
                            },
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
                {
                    let mut r = ctx.renderer.write().await;
                    r.set_config(new_config);
                }
                ctx.refresh_display().await;
                IpcResponse::Success {
                    message: "Configuration reloaded and applied".to_string(),
                }
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
            let mut self_text = ctx.self_copied_text.write().await;
            *self_text = t.clone();
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
            message: format!("Pipeline executed successfully (saved {} files)", res.saved_files.len()),
        }
    }
}

fn simulate_paste_action() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let _ = std::process::Command::new("wtype")
            .args(["-M", "ctrl", "-k", "v", "-m", "ctrl"])
            .status();
    });
}

fn schedule_thumbnail_generation(item: &ClipItem, ctx: Arc<DaemonContext>) {

    if let little_clip_core::ClipPayload::Image { ref bytes, ref thumbnail, .. } = item.payload {
        if thumbnail.is_none() {
            let item_id = item.id;
            let bytes = bytes.clone();
            tokio::task::spawn_blocking(move || {
                if let Some(thumb) = little_clip_core::item::ImageThumbnail::generate(&bytes, 320, 220) {
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
                if let Some(plain_wf) = little_clip_core::compute_plain_waveform(&path_clone, 200, &audio_cfg) {
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
                            if let Some(spectral_colors) = little_clip_core::compute_spectral_colors(&path_p2, 200, &audio_cfg_p2) {
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
                    little_clip_core::animation::decode_animated_gif_or_webp(&path_clone, 200, 150, 25)
                } else if little_clip_core::animation::is_video_path(&path_clone) {
                    little_clip_core::animation::extract_video_preview_frames(&path_clone, 200, 150, 25, 2.5, true)
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
                    tracing::warn!("Failed to generate animation thumbnail for {:?}", path_clone);
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
                        if let Some(thumb) = little_clip_core::item::ImageThumbnail::generate(&bytes, 200, 160) {
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

        let has_image_mime = if let Ok(ref out) = types_output {
            if out.status.success() {
                let types_str = String::from_utf8_lossy(&out.stdout);
                types_str.lines().any(|l| l.trim() == "image/png" || l.trim() == "image/jpeg")
            } else {
                false
            }
        } else {
            false
        };

        if has_image_mime {
            // Attempt reading raw image bytes
            if let Ok(img_out) = tokio::process::Command::new("wl-paste")
                .arg("-t")
                .arg("image/png")
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

                        if let Some(item) = ClipItem::from_image_bytes(img_out.stdout, "image/png") {
                            info!("Captured clipboard image to Left Hand: {} ({})", item.title, item.subtitle);
                            schedule_async_previews(&item, Arc::clone(&ctx));
                            {
                                let mut mgr = ctx.manager.write().await;
                                mgr.push_left(item);
                            }
                            ctx.refresh_display().await;
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
                    let self_text = ctx.self_copied_text.read().await;
                    if *self_text == text {
                        last_clipboard_text = text;
                        continue;
                    }
                    drop(self_text);

                    if !text.is_empty() && text != last_clipboard_text {
                        last_clipboard_text = text.clone();
                        last_image_hash = None;

                        let item = ClipItem::from_text(text);
                        info!("Captured clipboard item to Left Hand: [{}] {}", item.handler_tag, item.title);
                        schedule_async_previews(&item, Arc::clone(&ctx));
                        {
                            let mut mgr = ctx.manager.write().await;
                            mgr.push_left(item);
                        }
                        ctx.refresh_display().await;
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
        _ => None,
    }
}

fn is_image_file_path(p: &std::path::Path) -> bool {
    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "svg" | "gif" | "bmp")
}

async fn copy_to_os_clipboard(item: &ClipItem, ctx: &DaemonContext) {
    use std::hash::{Hash, Hasher};

    match &item.payload {
        little_clip_core::ClipPayload::Image { bytes, mime, .. } => {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            bytes.hash(&mut hasher);
            let hash = hasher.finish();
            {
                let mut guard = ctx.self_copied_image_hash.write().await;
                *guard = Some(hash);
            }

            if let Ok(mut child) = std::process::Command::new("wl-copy")
                .arg("-t")
                .arg(mime)
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = std::io::Write::write_all(&mut stdin, bytes);
                }
                let _ = child.wait();
            }
        }
        little_clip_core::ClipPayload::FileBatch(files) if files.len() == 1 && is_image_file_path(&files[0]) => {
            let path = &files[0];
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png")
                .to_lowercase();
            let mime = if ext == "jpg" || ext == "jpeg" {
                "image/jpeg"
            } else if ext == "webp" {
                "image/webp"
            } else if ext == "svg" {
                "image/svg+xml"
            } else {
                "image/png"
            };
            if let Ok(bytes) = std::fs::read(path) {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                bytes.hash(&mut hasher);
                let hash = hasher.finish();
                {
                    let mut guard = ctx.self_copied_image_hash.write().await;
                    *guard = Some(hash);
                }

                if let Ok(mut child) = std::process::Command::new("wl-copy")
                    .arg("-t")
                    .arg(mime)
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                {
                    if let Some(mut stdin) = child.stdin.take() {
                        let _ = std::io::Write::write_all(&mut stdin, &bytes);
                    }
                    let _ = child.wait();
                }
            }
        }
        little_clip_core::ClipPayload::Text(t) => {
            let _ = std::process::Command::new("wl-copy").arg(t).spawn();
        }
        little_clip_core::ClipPayload::Markdown(m) => {
            let _ = std::process::Command::new("wl-copy").arg(m).spawn();
        }
        little_clip_core::ClipPayload::Url(u) => {
            let _ = std::process::Command::new("wl-copy").arg(u.as_str()).spawn();
        }
        little_clip_core::ClipPayload::Color { hex, .. } => {
            let _ = std::process::Command::new("wl-copy").arg(hex).spawn();
        }
        little_clip_core::ClipPayload::MultiImage { paths, .. } => {
            let text = paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let _ = std::process::Command::new("wl-copy").arg(&text).spawn();
        }
        little_clip_core::ClipPayload::FileBatch(files) => {
            let text = files
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            let _ = std::process::Command::new("wl-copy").arg(&text).spawn();
        }
        _ => {}
    }
}


pub fn get_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("little-screen-clip.sock")
    } else {
        PathBuf::from("/tmp/little-screen-clip.sock")
    }
}
