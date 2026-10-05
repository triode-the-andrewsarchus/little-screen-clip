//! Command-line control utility for Little Screen Clip daemon (`clipctl`).
//!
//! Provides CLI commands to inspect slots, push text, swap cards, run transformations,
//! execute pipeline recipes, and simulate capture/paste actions.

use clap::{Parser, Subcommand};
use colored::*;
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use little_clip_core::{
    HandSide, SlotId, copy_bytes_to_system_clipboard, copy_text_to_system_clipboard,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Top-level CLI arguments parser for `clipctl`.
#[derive(Parser)]
#[command(name = "clipctl", about = "Control Little Screen Clip daemon")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Available CLI subcommands for controlling daemon state and operations.
#[derive(Subcommand)]
enum Commands {
    /// Show current slots state across both hands and stacks
    Status,
    /// Push text to a hand (left or right, defaults to left)
    Push {
        text: String,
        #[arg(long, default_value = "left")]
        hand: String,
    },
    /// Swap two slots (e.g. "left" and "right", or "lh" and "l1")
    Swap { a: String, b: String },
    /// Undo last Left Hand operation
    Undo,
    /// Clear a slot
    Clear { slot: String },
    /// Execute an operation on Left Hand (and optional secondary slot, e.g. Right Hand)
    Op {
        name: String,
        #[arg(short, long)]
        slot: Option<String>,
        #[arg(long)]
        sep: Option<String>,
    },
    /// Manage and run visual pipeline recipes
    Recipe {
        #[command(subcommand)]
        action: RecipeAction,
    },
    /// Re-run the last executed operation or recipe
    Last {
        /// Optional prompt text to pass if the recipe or action requests input
        #[arg(short, long)]
        prompt: Option<String>,
    },
    /// Force screen redraw
    Refresh,
    /// Reload configuration file and re-render screen
    Reload,
    /// Capture currently selected text/item from active window to a hand (defaults to right)
    Copy {
        #[arg(long, default_value = "right")]
        hand: String,
        /// Do not restore the OS clipboard to Left Hand afterwards
        #[arg(long)]
        no_restore: bool,
    },
    /// Paste content from a hand slot into the active window (defaults to right)
    Paste {
        #[arg(long, default_value = "right")]
        hand: String,
    },
    /// Blank/turn off the screen display for screen-saver or lock
    Lock,
    /// Restore the screen display upon unlock
    Unlock,
    /// Check whether the screen display is currently blanked/locked
    IsLocked,
    /// Capture a snapshot of the rolling system audio buffer
    RollingAudio,
}

/// Subcommands for inspecting and executing pipeline recipes.
#[derive(Subcommand)]
enum RecipeAction {
    /// List all discovered recipes
    List,
    /// Run a recipe by name
    Run { name: String },
}

/// Sends a JSON-serialized IPC request over Unix domain socket and reads the daemon's response.
async fn send_ipc_req(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    request: &IpcRequest,
) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    let req_json = serde_json::to_string(request)?;
    writer.write_all(req_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    if let Some(line) = lines.next_line().await? {
        let resp: IpcResponse = serde_json::from_str(&line)?;
        Ok(resp)
    } else {
        Err("Daemon closed connection without responding".into())
    }
}

async fn copy_text_to_os_clipboard(text: String) {
    match tokio::task::spawn_blocking(move || copy_text_to_system_clipboard(&text)).await {
        Ok(Err(error)) => eprintln!("Clipboard copy failed: {error}"),
        Err(error) => eprintln!("Clipboard copy worker failed: {error}"),
        Ok(Ok(())) => {}
    }
}

async fn copy_bytes_to_os_clipboard(bytes: Vec<u8>, mime: String) {
    match tokio::task::spawn_blocking(move || copy_bytes_to_system_clipboard(&bytes, &mime)).await {
        Ok(Err(error)) => eprintln!("Clipboard copy failed: {error}"),
        Err(error) => eprintln!("Clipboard copy worker failed: {error}"),
        Ok(Ok(())) => {}
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let sock_path = get_socket_path();

    let stream = match UnixStream::connect(&sock_path).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "{}: Cannot connect to daemon at {}: {}",
                "Error".red().bold(),
                sock_path.display(),
                e
            );
            eprintln!("Is {} running?", "little-clipd".yellow().bold());
            std::process::exit(1);
        }
    };

    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    let request = match cli.command {
        Commands::Copy { hand, no_restore } => {
            let hand_side = if hand.to_lowercase().starts_with('l') {
                HandSide::Left
            } else {
                HandSide::Right
            };

            let route_req = IpcRequest::RouteNextClipboard {
                hand: hand_side,
                timeout_ms: 2000,
                restore_os_clipboard: !no_restore,
            };
            let _ = send_ipc_req(&mut writer, &mut lines, &route_req).await?;

            let initial_cb = tokio::process::Command::new("wl-paste")
                .arg("-n")
                .output()
                .await
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .unwrap_or_default();

            tokio::time::sleep(std::time::Duration::from_millis(40)).await;

            let _ = tokio::process::Command::new("wtype")
                .args(["-m", "alt", "-M", "ctrl", "-k", "c", "-m", "ctrl"])
                .status()
                .await;

            let mut captured = false;
            let start = tokio::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_millis(350) {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                if let Ok(out) = tokio::process::Command::new("wl-paste")
                    .arg("-n")
                    .output()
                    .await
                {
                    if out.status.success() && !out.stdout.is_empty() {
                        if let Ok(text) = String::from_utf8(out.stdout) {
                            if text != initial_cb {
                                captured = true;
                                break;
                            }
                        }
                    }
                }
            }

            if captured {
                println!(
                    "{}: Captured selection to {:?}",
                    "Success".green().bold(),
                    hand_side
                );
                return Ok(());
            }

            if let Ok(out) = tokio::process::Command::new("wl-paste")
                .arg("--primary")
                .arg("-n")
                .output()
                .await
            {
                if out.status.success() && !out.stdout.is_empty() {
                    if let Ok(text) = String::from_utf8(out.stdout) {
                        let push_req = IpcRequest::PushText {
                            text: text.clone(),
                            hand: hand_side,
                        };
                        let _ = send_ipc_req(&mut writer, &mut lines, &push_req).await?;
                        println!(
                            "{}: Captured primary selection to {:?}",
                            "Success".green().bold(),
                            hand_side
                        );
                        return Ok(());
                    }
                }
            }

            println!(
                "{}: No selection detected in active window.",
                "Notice".yellow().bold()
            );
            return Ok(());
        }
        Commands::Paste { hand } => {
            let slot = if hand.to_lowercase().starts_with('l') {
                SlotId::LeftHand
            } else {
                SlotId::RightHand
            };

            let get_req = IpcRequest::GetSlotItem(slot);
            let resp = send_ipc_req(&mut writer, &mut lines, &get_req).await?;
            if let IpcResponse::SlotItem(Some(item)) = resp {
                if let Some(text) = item.to_clipboard_text() {
                    copy_text_to_os_clipboard(text).await;
                } else if let little_clip_core::ClipPayload::Image { bytes, mime, .. } =
                    item.payload
                {
                    copy_bytes_to_os_clipboard(bytes, mime).await;
                }

                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                let _ = tokio::process::Command::new("wtype")
                    .args(["-m", "alt", "-M", "ctrl", "-k", "v", "-m", "ctrl"])
                    .status()
                    .await;

                if slot == SlotId::RightHand {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let left_req = IpcRequest::GetSlotItem(SlotId::LeftHand);
                    if let Ok(IpcResponse::SlotItem(Some(left_item))) =
                        send_ipc_req(&mut writer, &mut lines, &left_req).await
                    {
                        if let Some(left_text) = left_item.to_clipboard_text() {
                            copy_text_to_os_clipboard(left_text).await;
                        }
                    }
                }

                println!("{}: Pasted from {}", "Success".green().bold(), slot.label());
                return Ok(());
            } else {
                println!("{}: {} is empty.", "Warning".yellow().bold(), slot.label());
                return Ok(());
            }
        }
        Commands::Status => IpcRequest::GetState,
        Commands::Push { text, hand } => {
            let hand_side = if hand.to_lowercase().starts_with('r') {
                HandSide::Right
            } else {
                HandSide::Left
            };
            IpcRequest::PushText {
                text,
                hand: hand_side,
            }
        }
        Commands::Swap { a, b } => {
            let slot_a = parse_slot_str(&a)?;
            let slot_b = parse_slot_str(&b)?;
            IpcRequest::SwapSlots(slot_a, slot_b)
        }
        Commands::Undo => IpcRequest::Undo,
        Commands::Clear { slot } => {
            let s = parse_slot_str(&slot)?;
            IpcRequest::Clear(s)
        }
        Commands::Op { name, slot, sep } => {
            let secondary_slot = match slot {
                Some(ref s) => Some(parse_slot_str(s)?),
                None => None,
            };
            IpcRequest::ExecuteOp {
                op_name: name,
                secondary_slot,
                selected_slots: None,
                separator: sep,
            }
        }
        Commands::Recipe { action } => match action {
            RecipeAction::List => IpcRequest::ListRecipes,
            RecipeAction::Run { name } => IpcRequest::ExecuteRecipe { recipe_name: name },
        },
        Commands::Last { prompt } => IpcRequest::ExecuteLastAction {
            prompt_text: prompt,
        },
        Commands::Refresh => IpcRequest::ForceRefreshDisplay,
        Commands::Reload => IpcRequest::ReloadConfig,
        Commands::Lock => IpcRequest::Lock,
        Commands::Unlock => IpcRequest::Unlock,
        Commands::IsLocked => IpcRequest::GetLockStatus,
        Commands::RollingAudio => IpcRequest::GetRollingAudio,
    };

    let req_json = serde_json::to_string(&request)?;
    writer.write_all(req_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    if let Some(line) = lines.next_line().await? {
        let resp: IpcResponse = serde_json::from_str(&line)?;
        match resp {
            IpcResponse::State(mgr) => {
                println!("{}", "=== LITTLE SCREEN CLIP SLOTS ===".cyan().bold());
                print_slot(SlotId::Undo, mgr.get(SlotId::Undo));
                print_slot(SlotId::LeftHand, mgr.get(SlotId::LeftHand));
                print_slot(SlotId::LeftPrior1, mgr.get(SlotId::LeftPrior1));
                print_slot(SlotId::LeftPrior2, mgr.get(SlotId::LeftPrior2));
                print_slot(SlotId::LeftPrior3, mgr.get(SlotId::LeftPrior3));
                print_slot(SlotId::RightPrior2, mgr.get(SlotId::RightPrior2));
                print_slot(SlotId::RightPrior1, mgr.get(SlotId::RightPrior1));
                print_slot(SlotId::RightHand, mgr.get(SlotId::RightHand));
            }
            IpcResponse::StateSummary(_) => {
                eprintln!("Received a polling-only state summary where full state was expected.");
                std::process::exit(1);
            }
            IpcResponse::RecipeList(recipes) => {
                println!("{}", "=== LITTLE SCREEN CLIP RECIPES ===".cyan().bold());
                if recipes.is_empty() {
                    println!("  No recipes found in ~/.config/little-screen-clip/recipes/");
                } else {
                    for r in recipes {
                        println!("  {} {}", "*".magenta(), r.bold());
                    }
                }
            }
            IpcResponse::LastAction(action) => match action {
                Some(little_clip_core::ipc::LastAction::Op { op_name, .. }) => {
                    println!("Last action was operation: {}", op_name.bold());
                }
                Some(little_clip_core::ipc::LastAction::Recipe { recipe_name }) => {
                    println!("Last action was recipe: {}", recipe_name.bold());
                }
                None => {
                    println!("No prior action recorded.");
                }
            },
            IpcResponse::NeedsPrompt { message, .. } => {
                println!("{}: {}", "Prompt Required".yellow().bold(), message);
            }
            IpcResponse::Success { message } => {
                println!("{}: {}", "Success".green().bold(), message);
            }
            IpcResponse::OpResult { item } => {
                println!("{}: Transformed -> {}", "Result".green().bold(), item.title);
                println!("  Subtitle: {}", item.subtitle.dimmed());
            }
            IpcResponse::PipelineExecuted {
                node_previews,
                output_title,
                duration_ms,
            } => {
                println!(
                    "{} in {}ms{}",
                    "Pipeline executed".green().bold(),
                    duration_ms,
                    output_title
                        .as_deref()
                        .map(|title| format!("; output: {title}"))
                        .unwrap_or_default()
                );
                if !node_previews.is_empty() {
                    println!("  Node previews:");
                    let mut previews: Vec<_> = node_previews.into_iter().collect();
                    previews.sort_unstable_by_key(|(id, _)| *id);
                    for (id, preview) in previews {
                        let truncated = preview.chars().count() > 120;
                        let preview = preview.chars().take(120).collect::<String>();
                        println!(
                            "    {}: {}{}",
                            id,
                            preview,
                            if truncated { "…" } else { "" }
                        );
                    }
                }
            }
            IpcResponse::LockStatus(locked) => {
                if locked {
                    println!("{}", "LOCKED (Screen blanked)".yellow().bold());
                } else {
                    println!("{}", "UNLOCKED (Screen active)".green().bold());
                }
            }
            IpcResponse::RollingAudio {
                path,
                duration_secs,
                sample_rate,
                channels,
            } => {
                println!(
                    "{}: {:.2}s ({}Hz, {}ch) -> {}",
                    "Rolling Audio Snapshot".cyan().bold(),
                    duration_secs,
                    sample_rate,
                    channels,
                    path.display()
                );
            }
            IpcResponse::SlotItem(item) => {
                if let Some(i) = item {
                    println!("Item: {} ({})", i.title.bold(), i.subtitle.dimmed());
                } else {
                    println!("<EMPTY>");
                }
            }
            IpcResponse::AutoActions(actions) => {
                println!("{}", "=== AUTO-ACTIONS ===".cyan().bold());
                for (k, v) in actions {
                    let st = if v {
                        "ENABLED".green().bold()
                    } else {
                        "DISABLED".yellow()
                    };
                    println!("  {} : {}", k.bold(), st);
                }
            }
            IpcResponse::ScenePreviews(previews) => {
                println!("{} completed scene previews", previews.len());
            }
            IpcResponse::Error { error } => {
                eprintln!("{}: {}", "Error".red().bold(), error);
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

/// Prints a formatted status line for a single slot and its contents.
fn print_slot(slot: SlotId, item: Option<&little_clip_core::ClipItem>) {
    let lbl_str = match slot {
        SlotId::Undo => format!("[{:^10}]", slot.label()).bold().yellow(),
        SlotId::LeftHand => format!("[{:^10}]", slot.label()).bold().bright_magenta(),
        SlotId::LeftPrior1 | SlotId::LeftPrior2 | SlotId::LeftPrior3 => {
            format!("[{:^10}]", slot.label()).bold().bright_cyan()
        }
        SlotId::RightPrior2 | SlotId::RightPrior1 => {
            format!("[{:^10}]", slot.label()).bold().bright_blue()
        }
        SlotId::RightHand => format!("[{:^10}]", slot.label()).bold().bright_green(),
    };

    if let Some(item) = item {
        println!(
            "{} {} ({})",
            lbl_str,
            item.title.bold(),
            item.subtitle.dimmed()
        );
    } else {
        println!("{} {}", lbl_str, "<EMPTY>".dimmed());
    }
}

/// Parses user slot name input loosely into a [`SlotId`].
fn parse_slot_str(s: &str) -> Result<SlotId, String> {
    SlotId::from_str_loose(s).ok_or_else(|| {
        format!(
            "Invalid slot identifier: '{}'. Expected 'undo', 'left', 'l1', 'l2', 'r2', 'r1', 'right'",
            s
        )
    })
}

/// Returns the active daemon Unix socket path (`$XDG_RUNTIME_DIR/little-screen-clip.sock`).
use little_clip_core::ipc::socket_path as get_socket_path;
