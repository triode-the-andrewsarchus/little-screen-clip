//! Shared worker entry points for each executable hosting the studios.
/// Runs an internal worker invocation and returns whether it was handled.
///
/// Worker failures exit with status 2. Normal application arguments return false.
pub fn run() -> bool {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(flag) = args.first() else {
        return false;
    };
    if flag == "--internal-system-console-snapshot" {
        let result = if args.len() == 2 {
            args[1]
                .to_str()
                .ok_or("Invalid console request encoding".to_string())
                .and_then(crate::file_manager::system::run_console_worker)
        } else {
            Err("Invalid console worker arguments".into())
        };
        if let Err(error) = result {
            eprintln!("Console snapshot: {error}");
            std::process::exit(2);
        }
        return true;
    }
    if flag == "--internal-system-processes" {
        let result = if args.len() == 2 {
            crate::file_manager::system::run_process_worker(std::path::Path::new(&args[1]))
        } else {
            Err("Invalid process worker arguments".into())
        };
        if let Err(error) = result {
            eprintln!("Process observations: {error}");
            std::process::exit(2);
        }
        return true;
    }
    if flag == "--internal-system-inspector" {
        let result = if args.len() == 3 {
            crate::file_manager::system::run_worker(
                std::path::Path::new(&args[1]),
                std::path::Path::new(&args[2]),
            )
        } else {
            Err("Invalid system inspector arguments".into())
        };
        if let Err(error) = result {
            eprintln!("System inspector: {error}");
            std::process::exit(2);
        }
        return true;
    }
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
    let barcode = flag == "--internal-barcode-worker";
    if !barcode && flag != "--internal-conversion-decode-worker" {
        return false;
    }
    let result = if args.len() == 3 {
        let input = std::path::Path::new(&args[1]);
        let output = std::path::Path::new(&args[2]);
        if barcode {
            crate::barcode_worker::run_helper(input, output)
        } else {
            crate::conversion_decode_worker::run_helper(input, output)
        }
    } else {
        Err("Invalid internal worker arguments".into())
    };
    if let Err(error) = result {
        if barcode && let Some(output) = args.get(2) {
            let _ = std::fs::write(std::path::Path::new(output).with_extension("error"), &error);
        }
        eprintln!("Internal worker: {error}");
        std::process::exit(2);
    }
    true
}
