use std::path::PathBuf;

fn main() -> Result<(), eframe::Error> {
    if clip_file_manager::worker_cli::run() {
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
            "Usage: clip-file-manager [DIRECTORY | FILE]\n       clip-file-manager --map-location LOCATION\n       clip-file-manager --map-file PATH\n\nBrowse folders or open a file in its viewer.\nDefaults to your home directory. Ctrl+Q closes the application.\n\nDownload elevation data: --download-nasadem [--config FILE] [--plan] [--limit N]"
        );
        return Ok(());
    }
    if first.as_deref().is_some_and(|arg| arg == "--map-location") {
        let location = args
            .next()
            .and_then(|arg| little_clip_core::geography::places::resolve(arg.to_str()?));
        if let Some(location) = location.filter(|_| args.next().is_none()) {
            return clip_file_manager::file_manager::run_location(location);
        }
        eprintln!("Expected a coordinate pair or a recognizable place name");
        std::process::exit(2);
    }
    if first.as_deref().is_some_and(|arg| arg == "--map-file") {
        if let Some(path) = args.next().filter(|_| args.next().is_none()) {
            return clip_file_manager::file_manager::run_file(PathBuf::from(path));
        }
        eprintln!("Expected a map file or Minecraft Java save folder");
        std::process::exit(2);
    }
    if args.next().is_some() {
        eprintln!("Usage: clip-file-manager [DIRECTORY | FILE]");
        std::process::exit(2);
    }
    clip_file_manager::file_manager::run(first.map(PathBuf::from))
}
