//! Shared worker entry points for each executable hosting the studios.
/// Runs an internal worker invocation and returns whether it was handled.
///
/// Worker failures exit with status 2. Normal application arguments return false.
pub fn run() -> bool {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let Some(flag) = args.first() else {
        return false;
    };
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
