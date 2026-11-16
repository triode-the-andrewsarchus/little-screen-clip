//! Owned FUSE helper; never runs filesystem operations in the clipboard daemon.
fn main() {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        eprintln!("Usage: clip-drive-mount STATE_DIRECTORY CATALOG_ID MOUNT_DIRECTORY");
        std::process::exit(2);
    }
    let Some(id) = args[1].to_str() else {
        eprintln!("Invalid catalog identifier");
        std::process::exit(2);
    };
    if let Err(error) = clip_drives::mount_catalog(
        std::path::Path::new(&args[0]),
        id,
        std::path::Path::new(&args[2]),
    ) {
        eprintln!("Catalog mount failed: {error}");
        std::process::exit(1);
    }
}
