//! Separate FFmpeg library host. The desktop applications do not link this binary.

mod audio;
mod probe;
use std::{fs::File, os::fd::FromRawFd};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.len() != 3 || !matches!(arguments[0].as_str(), "probe" | "decode-audio") {
        return Err(
            "Expected: clip-media-worker probe|decode-audio FORMAT_WHITELIST /proc/self/fd/N"
                .into(),
        );
    }
    // Accept only an inherited descriptor, never a pathname supplied by a decoder.
    let descriptor: i32 = arguments[2]
        .strip_prefix("/proc/self/fd/")
        .ok_or("Expected an inherited input descriptor")?
        .parse()
        .map_err(|_| "Invalid input descriptor")?;
    if descriptor < 3 {
        return Err("Input must be an inherited content descriptor".into());
    }
    // SAFETY: fcntl validates the integer and creates a new owned descriptor;
    // it does not dereference user memory or consume the inherited handle.
    let duplicate = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: the successful fcntl returned a descriptor owned only by us.
    let input = unsafe { File::from_raw_fd(duplicate) };
    if arguments[0] == "decode-audio" {
        return audio::decode(
            input,
            &arguments[1],
            &mut std::io::BufWriter::new(std::io::stdout().lock()),
        );
    }
    let result = probe::probe(input, &arguments[1])?;
    let bytes = serde_json::to_vec(&result).map_err(|error| error.to_string())?;
    if bytes.len() > 64 * 1024 {
        return Err("Media metadata exceeds output budget".into());
    }
    use std::io::Write;
    std::io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|error| error.to_string())
}
