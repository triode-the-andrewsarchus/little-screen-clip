use clap::{Parser, Subcommand};
use colored::*;
use little_clip_core::ipc::{IpcRequest, IpcResponse};
use little_clip_core::{HandSide, SlotId};
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Parser)]
#[command(name = "clipctl", about = "Control Little Screen Clip daemon")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

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
    /// Force screen redraw
    Refresh,
    /// Reload configuration file and re-render screen
    Reload,
}

#[derive(Subcommand)]
enum RecipeAction {
    /// List all discovered recipes
    List,
    /// Run a recipe by name
    Run { name: String },
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
        Commands::Refresh => IpcRequest::ForceRefreshDisplay,
        Commands::Reload => IpcRequest::ReloadConfig,
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
                print_slot(SlotId::RightPrior2, mgr.get(SlotId::RightPrior2));
                print_slot(SlotId::RightPrior1, mgr.get(SlotId::RightPrior1));
                print_slot(SlotId::RightHand, mgr.get(SlotId::RightHand));
            }
            IpcResponse::RecipeList(recipes) => {
                println!("{}", "=== LITTLE SCREEN CLIP RECIPES ===".cyan().bold());
                if recipes.is_empty() {
                    println!("  No recipes found in ~/.config/little-screen-clip/recipes/");
                } else {
                    for r in recipes {
                        println!("  {} {}", "★".magenta(), r.bold());
                    }
                }
            }
            IpcResponse::Success { message } => {
                println!("{}: {}", "Success".green().bold(), message);
            }
            IpcResponse::OpResult { item } => {
                println!("{}: Transformed -> {}", "Result".green().bold(), item.title);
                println!("  Subtitle: {}", item.subtitle.dimmed());
            }
            IpcResponse::Error { error } => {
                eprintln!("{}: {}", "Error".red().bold(), error);
                std::process::exit(1);
            }
        }
    }


    Ok(())
}

fn print_slot(slot: SlotId, item: Option<&little_clip_core::ClipItem>) {
    let lbl_str = match slot {
        SlotId::Undo => format!("[{:^10}]", slot.label()).bold().yellow(),
        SlotId::LeftHand => format!("[{:^10}]", slot.label()).bold().bright_magenta(),
        SlotId::LeftPrior1 | SlotId::LeftPrior2 => format!("[{:^10}]", slot.label()).bold().bright_cyan(),
        SlotId::RightPrior2 | SlotId::RightPrior1 => format!("[{:^10}]", slot.label()).bold().bright_blue(),
        SlotId::RightHand => format!("[{:^10}]", slot.label()).bold().bright_green(),
    };

    if let Some(item) = item {
        println!("{} {} ({})", lbl_str, item.title.bold(), item.subtitle.dimmed());
    } else {
        println!("{} {}", lbl_str, "<EMPTY>".dimmed());
    }
}

fn parse_slot_str(s: &str) -> Result<SlotId, String> {
    SlotId::from_str_loose(s).ok_or_else(|| {
        format!(
            "Invalid slot identifier: '{}'. Expected 'undo', 'left', 'l1', 'l2', 'r2', 'r1', 'right'",
            s
        )
    })
}

fn get_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("little-screen-clip.sock")
    } else {
        PathBuf::from("/tmp/little-screen-clip.sock")
    }
}
