//! Internal worker entry points required by every host of these studios.

/// Handles a studio worker invocation before normal application argument parsing.
/// Worker failures exit with status 2; unrelated arguments return false.
pub fn run() -> bool {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(flag) = args.first().and_then(|arg| arg.to_str()) else {
        return false;
    };
    let worker = match flag {
        "--internal-barcode-worker" => crate::barcode_worker::run_helper,
        "--internal-conversion-decode-worker" => crate::conversion_decode_worker::run_helper,
        _ => return false,
    };
    let result = if args.len() == 3 {
        worker(
            std::path::Path::new(&args[1]),
            std::path::Path::new(&args[2]),
        )
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
