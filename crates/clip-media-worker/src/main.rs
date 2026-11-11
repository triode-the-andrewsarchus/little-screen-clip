//! Separate FFmpeg library host. The desktop applications do not link this binary.

mod audio;
mod audio_filter;
mod audio_render;
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
    if arguments.first().is_some_and(|mode| mode == "render-audio") {
        return render_command(&arguments);
    }
    if arguments.len() != 3 || !matches!(arguments[0].as_str(), "probe" | "decode-audio") {
        return Err(
            "Expected: clip-media-worker probe|decode-audio FORMAT_WHITELIST /proc/self/fd/N"
                .into(),
        );
    }
    let input = inherited_input(&arguments[2])?;
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

fn render_command(arguments: &[String]) -> Result<(), String> {
    let settings = arguments.get(1).ok_or("Missing audio render settings")?;
    if settings.len() > clip_media_protocol::MAX_SETTINGS_BYTES || arguments.len() > 11 {
        return Err("Audio render request exceeds its budget".into());
    }
    let job = serde_json::from_str(settings).map_err(|error| error.to_string())?;
    let (output, offset) = match arguments
        .get(2)
        .and_then(|value| value.strip_prefix("--output="))
    {
        Some(path) => (inherited_input(path)?, 3),
        None => (duplicate_descriptor(1)?, 2),
    };
    let inputs = arguments[offset..]
        .iter()
        .map(|path| inherited_input(path))
        .collect::<Result<Vec<_>, _>>()?;
    audio_render::render(&job, inputs, output)
}

fn inherited_input(path: &str) -> Result<File, String> {
    // Accept only an inherited descriptor, never a decoder-supplied pathname.
    let descriptor: i32 = path
        .strip_prefix("/proc/self/fd/")
        .ok_or("Expected an inherited input descriptor")?
        .parse()
        .map_err(|_| "Invalid input descriptor")?;
    if descriptor < 3 {
        return Err("Input must be an inherited content descriptor".into());
    }
    duplicate_descriptor(descriptor)
}

fn duplicate_descriptor(descriptor: i32) -> Result<File, String> {
    // SAFETY: fcntl validates the integer and creates a new owned descriptor.
    let duplicate = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicate < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: the successful fcntl returned a descriptor owned only by us.
    Ok(unsafe { File::from_raw_fd(duplicate) })
}
