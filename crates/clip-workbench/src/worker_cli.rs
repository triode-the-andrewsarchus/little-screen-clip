//! Shared worker entry points for each executable hosting the studios.
/// Runs an internal worker invocation and returns whether it was handled.
///
/// Worker failures exit with status 2. Normal application arguments return false.
pub fn run() -> bool {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(flag) = args.first().and_then(|arg| arg.to_str()) else {
        return false;
    };
    if flag == "--internal-fuse-binwalk-daemon" {
        let mut writable = false;
        let mut paths = Vec::new();
        for arg in args.into_iter().skip(1) {
            let s = arg.to_string_lossy();
            if s == "--writable" || s == "-w" {
                writable = true;
            } else {
                paths.push(std::path::PathBuf::from(arg));
            }
        }
        if paths.len() != 2 {
            eprintln!("Usage: --internal-fuse-binwalk-daemon [-w] <binary_path> <mountpoint>");
            std::process::exit(2);
        }
        let result = crate::file_manager::fuse_binwalk::run_fuse_binwalk_daemon(
            &paths[0], &paths[1], writable,
        );
        if let Err(error) = result {
            eprintln!("FUSE binwalk daemon: {error}");
            std::process::exit(2);
        }
        return true;
    }
    let expected = match flag {
        "--internal-file-search-reader" | "--internal-file-extraction-plan" => 1,
        "--internal-system-console-snapshot" | "--internal-system-processes" => 2,
        "--internal-system-inspector"
        | "--internal-barcode-worker"
        | "--internal-conversion-decode-worker" => 3,
        _ => return false,
    };
    let result = if args.len() == expected {
        dispatch(flag, &args[1..])
    } else {
        Err("Invalid internal worker arguments".into())
    };
    if let Err(error) = result {
        if flag == "--internal-barcode-worker"
            && let Some(output) = args.get(2)
        {
            let _ = std::fs::write(std::path::Path::new(output).with_extension("error"), &error);
        }
        eprintln!("Internal worker: {error}");
        std::process::exit(2);
    }
    true
}

fn dispatch(flag: &str, args: &[std::ffi::OsString]) -> Result<(), String> {
    use std::path::Path;
    match flag {
        "--internal-file-search-reader" => crate::file_manager::search::run_reader(),
        "--internal-file-extraction-plan" => crate::file_manager::search::run_extraction(),
        "--internal-system-console-snapshot" => args[0]
            .to_str()
            .ok_or_else(|| "Invalid console request encoding".to_string())
            .and_then(crate::file_manager::system::run_console_worker),
        "--internal-system-processes" => {
            crate::file_manager::system::run_process_worker(Path::new(&args[0]))
        }
        "--internal-system-inspector" => {
            crate::file_manager::system::run_worker(Path::new(&args[0]), Path::new(&args[1]))
        }
        "--internal-barcode-worker" => {
            crate::barcode_worker::run_helper(Path::new(&args[0]), Path::new(&args[1]))
        }
        "--internal-conversion-decode-worker" => {
            crate::conversion_decode_worker::run_helper(Path::new(&args[0]), Path::new(&args[1]))
        }
        _ => Err("Unknown internal worker".into()),
    }
}
