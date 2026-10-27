use std::path::PathBuf;

fn main() -> Result<(), eframe::Error> {
    if clip_workbench::worker_cli::run() {
        return Ok(());
    }
    if std::env::var("LITTLE_CLIP_SEARCH_TIMINGS").is_ok_and(|value| value == "1") {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("off,little_clip_search_timing=debug")
            .with_ansi(false)
            .with_writer(std::io::stderr)
            .try_init();
    }
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first
        .as_deref()
        .is_some_and(|arg| arg == "--help" || arg == "-h")
    {
        println!(
            "Usage: clip-file-manager [DIRECTORY]\n\nBrowse folders in the standalone Little Screen Clip file manager.\nDefaults to your home directory. Ctrl+Q closes the application."
        );
        return Ok(());
    }
    if args.next().is_some() {
        eprintln!("Usage: clip-file-manager [DIRECTORY]");
        std::process::exit(2);
    }
    clip_workbench::file_manager::run(first.map(PathBuf::from))
}
