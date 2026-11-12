//! Isolates opaque conversion-output decoding so a stale preview can stop it.

use crate::image_preview::{
    ImageToolProcessControl, clear_image_tool_pid_if_owned, spawn_image_tool_child,
    terminate_image_tool_process,
};
use image::RgbaImage;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::Duration;

const COPY_CHUNK_BYTES: usize = 64 * 1024;

/// Runs only in the hidden helper process. Status 0 is the existing decode-failure fallback.
pub fn run_helper(input: &Path, output: &Path) -> Result<(), String> {
    let encoded = std::fs::read(input).map_err(|error| error.to_string())?;
    let decoded = image::load_from_memory(&encoded).map(|image| image.to_rgba8());
    let partial = output.with_extension("partial");
    let mut writer =
        BufWriter::new(std::fs::File::create(&partial).map_err(|error| error.to_string())?);
    match decoded {
        Ok(image) => {
            writer.write_all(&[1]).map_err(|error| error.to_string())?;
            writer
                .write_all(&image.width().to_le_bytes())
                .and_then(|()| writer.write_all(&image.height().to_le_bytes()))
                .and_then(|()| writer.write_all(image.as_raw()))
                .map_err(|error| error.to_string())?;
        }
        Err(_) => writer.write_all(&[0]).map_err(|error| error.to_string())?,
    }
    writer.flush().map_err(|error| error.to_string())?;
    drop(writer);
    std::fs::rename(partial, output).map_err(|error| error.to_string())
}

/// Optimizes PNG bytes in an isolated host process, retaining metadata and pixels.
///
/// # Errors
/// Returns guarded-input, PNG-validation, resource-budget, optimization or write errors.
pub fn run_png_optimizer(input: &Path, output: &Path) -> Result<(), String> {
    const ENCODED_LIMIT: usize = 128 * 1024 * 1024;
    let encoded = little_clip_core::file_source::read_file_bytes(input, ENCODED_LIMIT, true, false)
        .map_err(|error| error.to_string())?;
    let limits = little_clip_core::item::raster_preview_limits();
    let (width, height) =
        image::ImageReader::with_format(std::io::Cursor::new(&encoded), image::ImageFormat::Png)
            .into_dimensions()
            .map_err(|error| error.to_string())?;
    if limits.max_image_width.is_some_and(|limit| width > limit)
        || limits.max_image_height.is_some_and(|limit| height > limit)
    {
        return Err("PNG optimization exceeds raster dimension limits".into());
    }
    let mut options = oxipng::Options::from_preset(2);
    options.timeout = Some(Duration::from_secs(30));
    options.max_decompressed_size = limits
        .max_alloc
        .map(|limit| limit.min(usize::MAX as u64) as usize);
    // Keep chunks and hidden RGB data; this operation is strictly lossless.
    options.strip = oxipng::StripChunks::None;
    options.optimize_alpha = false;
    let optimized =
        oxipng::optimize_from_memory(&encoded, &options).map_err(|error| error.to_string())?;
    let result = if optimized.len() < encoded.len() {
        &optimized
    } else {
        &encoded
    };
    let parent = output.parent().unwrap_or(Path::new("."));
    let directory = little_clip_core::kernel_interface::pin_content_directory(parent)
        .map_err(|error| error.to_string())?;
    little_clip_core::file_source::create_file_in(output, result, &directory).map(drop)
}

fn kill_and_reap(child: &mut std::process::Child, control: &ImageToolProcessControl) {
    let pid = child.id();
    terminate_image_tool_process(pid);
    let _ = child.wait();
    clear_image_tool_pid_if_owned(&control.child_pid, pid);
}

/// A canceled decode never returns pixels, including if the child exits concurrently.
pub(crate) fn decode_in_child(
    encoded: &[u8],
    expected_pixel_bytes: usize,
    program: &Path,
    control: &ImageToolProcessControl,
) -> Result<Option<RgbaImage>, String> {
    let cancelled = || control.cancel.load(Ordering::Acquire);
    if cancelled() {
        return Err("cancelled".into());
    }
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let input = directory.path().join("input.image");
    let output = directory.path().join("output.rgba");
    let mut input_file = std::fs::File::create(&input).map_err(|error| error.to_string())?;
    for chunk in encoded.chunks(COPY_CHUNK_BYTES) {
        if cancelled() {
            return Err("cancelled".into());
        }
        input_file
            .write_all(chunk)
            .map_err(|error| error.to_string())?;
    }
    drop(input_file);
    if cancelled() {
        return Err("cancelled".into());
    }

    let mut command = Command::new(program);
    command
        .arg("--internal-conversion-decode-worker")
        .arg(&input)
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = spawn_image_tool_child(command, control)?;
    let pid = child.id();
    loop {
        if cancelled() {
            kill_and_reap(&mut child, control);
            return Err("cancelled".into());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                clear_image_tool_pid_if_owned(&control.child_pid, pid);
                if !status.success() {
                    return Err(format!("conversion output decoder exited with {status}"));
                }
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(error) => {
                kill_and_reap(&mut child, control);
                return Err(error.to_string());
            }
        }
    }
    if cancelled() {
        return Err("cancelled".into());
    }
    let size = std::fs::metadata(&output)
        .map_err(|error| error.to_string())?
        .len();
    let max_size = (expected_pixel_bytes as u64).saturating_add(9);
    if size > max_size {
        return Err("conversion output decode exceeded image size".into());
    }
    let mut file = std::fs::File::open(&output).map_err(|error| error.to_string())?;
    let mut status = [0_u8; 1];
    file.read_exact(&mut status)
        .map_err(|error| error.to_string())?;
    if status[0] == 0 && size == 1 {
        return if cancelled() {
            Err("cancelled".into())
        } else {
            Ok(None)
        };
    }
    if status[0] != 1 || size < 9 {
        return Err("invalid conversion output decode status".into());
    }
    let mut dimensions = [0_u8; 8];
    file.read_exact(&mut dimensions)
        .map_err(|error| error.to_string())?;
    let width = u32::from_le_bytes(dimensions[..4].try_into().unwrap());
    let height = u32::from_le_bytes(dimensions[4..].try_into().unwrap());
    let pixel_bytes = (width as usize)
        .checked_mul(height as usize)
        .and_then(|count| count.checked_mul(4))
        .ok_or("conversion output dimensions overflow")?;
    if size != (pixel_bytes as u64).saturating_add(9) {
        return Err("invalid conversion output dimensions".into());
    }
    let mut pixels = vec![0_u8; pixel_bytes];
    for chunk in pixels.chunks_mut(COPY_CHUNK_BYTES) {
        if cancelled() {
            return Err("cancelled".into());
        }
        file.read_exact(chunk).map_err(|error| error.to_string())?;
    }
    if cancelled() {
        return Err("cancelled".into());
    }
    RgbaImage::from_raw(width, height, pixels)
        .map(Some)
        .ok_or_else(|| "invalid conversion output pixels".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn png_optimizer_preserves_pixels_hidden_rgb_and_text_metadata() {
        let image = image::RgbaImage::from_fn(37, 19, |x, y| {
            image::Rgba([
                (x * 5) as u8,
                (y * 11) as u8,
                73,
                if x % 2 == 0 { 0 } else { 255 },
            ])
        });
        let mut encoded = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut encoded, image.width(), image.height());
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .add_text_chunk("Comment".into(), "keep this metadata".into())
                .unwrap();
            encoder
                .write_header()
                .unwrap()
                .write_image_data(image.as_raw())
                .unwrap();
        }
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.png");
        let output = directory.path().join("output.png");
        std::fs::write(&input, &encoded).unwrap();
        run_png_optimizer(&input, &output).unwrap();
        let optimized = std::fs::read(output).unwrap();
        assert!(optimized.len() <= encoded.len());
        assert_eq!(
            image::load_from_memory(&optimized).unwrap().to_rgba8(),
            image
        );
        let decoder = png::Decoder::new(std::io::Cursor::new(&optimized));
        let reader = decoder.read_info().unwrap();
        assert!(
            reader
                .info()
                .uncompressed_latin1_text
                .iter()
                .any(|chunk| chunk.keyword == "Comment" && chunk.text == "keep this metadata")
        );
    }

    #[test]
    fn png_optimizer_rejects_malformed_or_oversized_inputs_without_publishing() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.png");
        let output = directory.path().join("output.png");
        std::fs::write(&output, b"previous output").unwrap();
        std::fs::write(&input, b"invalid PNG").unwrap();
        assert!(run_png_optimizer(&input, &output).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"previous output");
        let image = image::RgbaImage::new(16_385, 1);
        image.save(&input).unwrap();
        assert!(
            run_png_optimizer(&input, &output)
                .unwrap_err()
                .contains("dimension limits")
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"previous output");
    }

    #[test]
    fn supervisor_reads_and_validates_child_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("decoder.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '\\001\\001\\000\\000\\000\\001\\000\\000\\000\\377\\000\\000\\377' > \"$3\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let actual = decode_in_child(b"encoded", 4, &script, &ImageToolProcessControl::new())
            .unwrap()
            .unwrap();

        assert_eq!(actual.dimensions(), (1, 1));
        assert_eq!(actual.as_raw(), &[255, 0, 0, 255]);
    }

    #[test]
    fn cancelled_decode_kills_and_reaps_the_helper() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("decoder.sh");
        let pid_file = directory.path().join("pid");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s' \"$$\" > '{}'\nsleep 30\n",
                pid_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let control = Arc::new(ImageToolProcessControl::new());
        let worker_control = control.clone();
        let handle =
            std::thread::spawn(move || decode_in_child(b"encoded", 64, &script, &worker_control));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !pid_file.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(pid_file.exists(), "decode helper did not start");
        let pid: i32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);

        control.cancel();
        assert_eq!(handle.join().unwrap().unwrap_err(), "cancelled");
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }
}
