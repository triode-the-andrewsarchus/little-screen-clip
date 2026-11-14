//! Command-line control utility for Little Screen Clip daemon (`clipctl`).
//!
//! Provides CLI commands to inspect slots, push text, swap cards, run transformations,
//! execute pipeline recipes, and simulate capture/paste actions.

mod copy;

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
    /// Execute an operation on the Left Hand or on explicitly selected slots
    Op {
        name: String,
        /// Secondary operand when the Left Hand is the primary operand
        #[arg(short, long, value_parser = parse_slot_str, conflicts_with = "slots")]
        slot: Option<SlotId>,
        /// Comma-separated input slots, in the order the operation should use them
        #[arg(long, value_delimiter = ',', value_parser = parse_slot_str)]
        slots: Vec<SlotId>,
        #[arg(long)]
        sep: Option<String>,
    },
    /// Manage and run visual pipeline recipes
    Recipe {
        #[command(subcommand)]
        action: RecipeAction,
    },
    /// Re-run or inspect the last executed operation or recipe
    Last {
        /// Show the recorded action without running it
        #[arg(long, conflicts_with = "prompt")]
        show: bool,
        /// Optional prompt text to pass when re-running the action
        #[arg(short, long)]
        prompt: Option<String>,
    },
    /// Inspect and configure automatic clipboard actions
    AutoActions {
        #[command(subcommand)]
        action: AutoActionCommand,
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
    /// Capture a temporary WAV snapshot of the rolling system audio buffer
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

/// Subcommands for the daemon's automatic clipboard actions.
#[derive(Subcommand)]
enum AutoActionCommand {
    /// List available actions and their current settings
    List,
    /// Enable or disable one action
    Set {
        key: String,
        #[arg(action = clap::ArgAction::Set)]
        enabled: bool,
    },
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

async fn copy_item_to_os_clipboard(item: little_clip_core::ClipItem) {
    let result = tokio::task::spawn_blocking(move || {
        if let Some(text) = item.to_clipboard_text() {
            copy_text_to_system_clipboard(&text)
        } else if let little_clip_core::ClipPayload::Image { bytes, mime, .. } = item.payload {
            copy_bytes_to_system_clipboard(&bytes, &mime)
        } else {
            Ok(())
        }
    })
    .await;
    match result {
        Ok(Err(error)) => eprintln!("Clipboard copy failed: {error}"),
        Err(error) => eprintln!("Clipboard copy worker failed: {error}"),
        Ok(Ok(())) => {}
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if little_clip_core::notebook::run_worker_cli() {
        return Ok(());
    }
    run_cli()
}

#[tokio::main]
async fn run_cli() -> Result<(), Box<dyn std::error::Error>> {
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

            copy::copy_to_hand(&mut writer, &mut lines, hand_side, !no_restore).await?;
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
                copy_item_to_os_clipboard(item).await;

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
                        copy_item_to_os_clipboard(left_item).await;
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
        Commands::Op {
            name,
            slot,
            slots,
            sep,
        } => IpcRequest::ExecuteOp {
            op_name: name,
            secondary_slot: slot,
            selected_slots: (!slots.is_empty()).then_some(slots),
            separator: sep,
        },
        Commands::Recipe { action } => match action {
            RecipeAction::List => IpcRequest::ListRecipes,
            RecipeAction::Run { name } => IpcRequest::ExecuteRecipe { recipe_name: name },
        },
        Commands::Last { show: true, .. } => IpcRequest::GetLastAction,
        Commands::Last { prompt, .. } => IpcRequest::ExecuteLastAction {
            prompt_text: prompt,
        },
        Commands::AutoActions { action } => match action {
            AutoActionCommand::List => IpcRequest::GetAutoActions,
            AutoActionCommand::Set { key, enabled } => IpcRequest::SetAutoAction { key, enabled },
        },
        Commands::Refresh => IpcRequest::ForceRefreshDisplay,
        Commands::Reload => IpcRequest::ReloadConfig,
        Commands::Lock => IpcRequest::Lock,
        Commands::Unlock => IpcRequest::Unlock,
        Commands::IsLocked => IpcRequest::GetLockStatus,
        Commands::RollingAudio => IpcRequest::GetRollingAudio,
    };

    let resp = send_ipc_req(&mut writer, &mut lines, &request).await?;
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
        IpcResponse::ClipboardRoute { status, .. } => println!("Clipboard route: {status:?}"),
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
        IpcResponse::ScenePreviews(_) => return Err("Unexpected scene preview response".into()),
        IpcResponse::Error { error } => {
            eprintln!("{}: {}", "Error".red().bold(), error);
            std::process::exit(1);
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
            "Invalid slot identifier: '{}'. Expected 'undo', 'left', 'l1', 'l2', 'l3', 'r2', 'r1', 'right'",
            s
        )
    })
}

/// Returns the active daemon Unix socket path (`$XDG_RUNTIME_DIR/little-screen-clip.sock`).
use little_clip_core::ipc::socket_path as get_socket_path;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_accepts_ordered_slots_and_rejects_ambiguous_operands() {
        let cli =
            Cli::try_parse_from(["clipctl", "op", "concat", "--slots", "l3,right,left"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Op { slot: None, slots, .. }
                if slots == [SlotId::LeftPrior3, SlotId::RightHand, SlotId::LeftHand]
        ));
        assert!(
            Cli::try_parse_from([
                "clipctl", "op", "concat", "--slot", "right", "--slots", "left,l1",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["clipctl", "op", "concat", "--slots", "left,unknown"]).is_err()
        );
    }

    #[tokio::test]
    async fn ipc_request_reports_peer_closure_instead_of_silent_success() {
        let (client, server) = UnixStream::pair().unwrap();
        let (reader, mut writer) = client.into_split();
        let mut lines = BufReader::new(reader).lines();
        let peer = tokio::spawn(async move {
            let mut requests = BufReader::new(server).lines();
            let request = requests.next_line().await.unwrap().unwrap();
            assert!(matches!(
                serde_json::from_str::<IpcRequest>(&request).unwrap(),
                IpcRequest::GetState
            ));
        });
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            send_ipc_req(&mut writer, &mut lines, &IpcRequest::GetState),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("closed connection without responding")
        );
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn clipboard_item_copy_preserves_image_bytes_and_mime_for_paste_and_restore() {
        use std::os::unix::fs::PermissionsExt;
        if let Some(directory) = std::env::var_os("CLIPCTL_TEST_CLIPBOARD") {
            let directory = std::path::PathBuf::from(directory);
            for mime in ["image/png", "image/webp"] {
                let bytes = vec![0, 255, 128, 1, 0];
                copy_item_to_os_clipboard(little_clip_core::ClipItem::from_image(
                    bytes.clone(),
                    1,
                    1,
                    mime,
                ))
                .await;
                assert_eq!(std::fs::read(directory.join("bytes")).unwrap(), bytes);
                assert_eq!(
                    std::fs::read_to_string(directory.join("mime")).unwrap(),
                    mime
                );
            }
            copy_item_to_os_clipboard(little_clip_core::ClipItem::from_text("restored text")).await;
            assert_eq!(
                std::fs::read(directory.join("bytes")).unwrap(),
                b"restored text"
            );
            assert_eq!(
                std::fs::read_to_string(directory.join("mime")).unwrap(),
                "text/plain;charset=utf-8"
            );
            return;
        }
        // Only the isolated child sees the fake adapter. No host clipboard or input is touched.
        let directory = tempfile::tempdir().unwrap();
        let adapter = directory.path().join("wl-copy");
        std::fs::write(&adapter, "#!/bin/sh\nprintf '%s' \"$2\" > \"$CLIPCTL_TEST_CLIPBOARD/mime\"\n/bin/cat > \"$CLIPCTL_TEST_CLIPBOARD/bytes\"\n").unwrap();
        std::fs::set_permissions(&adapter, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::clipboard_item_copy_preserves_image_bytes_and_mime_for_paste_and_restore",
                "--test-threads=1",
            ])
            .env("CLIPCTL_TEST_CLIPBOARD", directory.path())
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
    }
}
