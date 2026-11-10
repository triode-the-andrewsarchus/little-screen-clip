//! Isolated barcode decoder protocol and its cancellable process supervisor.

use crate::image_preview::{
    ImageToolProcessControl, clear_image_tool_pid_if_owned, spawn_image_tool_child,
    terminate_image_tool_process,
};
use image::RgbaImage;
use little_clip_ops::barcode::BarcodeDetection;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const OUTPUT_HEADROOM_BYTES: u64 = 8 * 1024 * 1024;
const COPY_CHUNK_BYTES: usize = 64 * 1024;
const SCAN_TIMEOUT: Duration = Duration::from_secs(60);

fn output_limit(image_bytes: usize) -> u64 {
    (image_bytes as u64)
        .saturating_mul(4)
        .saturating_add(OUTPUT_HEADROOM_BYTES)
}

struct LimitedWriter<W> {
    inner: W,
    remaining: u64,
    overflowed: bool,
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() as u64 > self.remaining {
            self.overflowed = true;
            return Err(std::io::Error::other("barcode scan output exceeds limit"));
        }
        let written = self.inner.write(bytes)?;
        self.remaining -= written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn write_bounded_json<T: serde::Serialize>(
    output: &Path,
    value: &T,
    limit: u64,
) -> Result<(), String> {
    let partial = output.with_extension("partial");
    let file = std::fs::File::create(&partial).map_err(|error| error.to_string())?;
    let mut writer = LimitedWriter {
        inner: BufWriter::new(file),
        remaining: limit,
        overflowed: false,
    };
    let result = serde_json::to_writer(&mut writer, value);
    if writer.overflowed {
        return Err("barcode scan output exceeds limit".into());
    }
    result.map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())?;
    drop(writer);
    std::fs::rename(partial, output).map_err(|error| error.to_string())
}

fn write_input(image: &RgbaImage, path: &Path, cancelled: &AtomicBool) -> Result<(), String> {
    let pixels = image.as_raw();
    let expected = (image.width() as usize)
        .checked_mul(image.height() as usize)
        .and_then(|size| size.checked_mul(4))
        .ok_or("barcode image dimensions overflow")?;
    if expected != pixels.len() {
        return Err("invalid barcode image dimensions".into());
    }
    let mut file = std::fs::File::create(path).map_err(|error| error.to_string())?;
    file.write_all(&image.width().to_le_bytes())
        .and_then(|()| file.write_all(&image.height().to_le_bytes()))
        .map_err(|error| error.to_string())?;
    for chunk in pixels.chunks(COPY_CHUNK_BYTES) {
        if cancelled.load(Ordering::Acquire) {
            return Err("barcode scan cancelled".into());
        }
        file.write_all(chunk).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Runs only in the hidden helper process. Errors never enter the scan result protocol.
pub fn run_helper(input: &Path, output: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(input).map_err(|error| error.to_string())?;
    if metadata.len() < 8 {
        return Err("invalid barcode scan input size".into());
    }
    let mut file = std::fs::File::open(input).map_err(|error| error.to_string())?;
    let mut header = [0_u8; 8];
    file.read_exact(&mut header)
        .map_err(|error| error.to_string())?;
    let width = u32::from_le_bytes(header[..4].try_into().unwrap());
    let height = u32::from_le_bytes(header[4..].try_into().unwrap());
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|size| size.checked_mul(4))
        .ok_or("barcode image dimensions overflow")?;
    let expected_file_size = (expected as u64)
        .checked_add(8)
        .ok_or("barcode image dimensions overflow")?;
    if metadata.len() != expected_file_size {
        return Err("invalid barcode scan input dimensions".into());
    }
    let mut pixels = vec![0; expected];
    file.read_exact(&mut pixels)
        .map_err(|error| error.to_string())?;
    let image =
        RgbaImage::from_raw(width, height, pixels).ok_or("could not reconstruct barcode image")?;
    let detections = little_clip_ops::barcode::detect_barcodes(&image);
    write_bounded_json(output, &detections, output_limit(expected))
}

fn kill_group_and_reap(child: &mut std::process::Child, control: &ImageToolProcessControl) {
    let pid = child.id();
    terminate_image_tool_process(pid);
    let _ = child.wait();
    clear_image_tool_pid_if_owned(&control.child_pid, pid);
}

/// A cancelled scan never returns detections, even if the child exits concurrently.
pub(crate) fn scan_in_child(
    image: RgbaImage,
    program: &Path,
    control: &ImageToolProcessControl,
) -> Result<Vec<BarcodeDetection>, String> {
    let image_bytes = image.as_raw().len();
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let input = directory.path().join("input.rgba");
    let output = directory.path().join("output.json");
    write_input(&image, &input, &control.cancel)?;
    drop(image);
    if control.cancel.load(Ordering::Acquire) {
        return Err("barcode scan cancelled".into());
    }
    let mut command = Command::new(program);
    command
        .arg("--internal-barcode-worker")
        .arg(&input)
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = spawn_image_tool_child(command, control)?;
    let pid = child.id();
    let started = Instant::now();
    loop {
        if control.cancel.load(Ordering::Acquire) {
            kill_group_and_reap(&mut child, control);
            return Err("barcode scan cancelled".into());
        }
        if started.elapsed() >= SCAN_TIMEOUT {
            kill_group_and_reap(&mut child, control);
            return Err("barcode scan timed out".into());
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                kill_group_and_reap(&mut child, control);
                return Err(error.to_string());
            }
        };
        match status {
            Some(status) if status.success() => {
                clear_image_tool_pid_if_owned(&control.child_pid, pid);
                break;
            }
            Some(status) => {
                clear_image_tool_pid_if_owned(&control.child_pid, pid);
                let error_path = output.with_extension("error");
                let detail = std::fs::metadata(&error_path)
                    .ok()
                    .filter(|metadata| metadata.len() <= 4096)
                    .and_then(|_| std::fs::read_to_string(error_path).ok());
                return Err(
                    detail.unwrap_or_else(|| format!("barcode worker exited with {status}"))
                );
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    if control.cancel.load(Ordering::Acquire) {
        return Err("barcode scan cancelled".into());
    }
    let size = std::fs::metadata(&output)
        .map_err(|error| error.to_string())?
        .len();
    if size > output_limit(image_bytes) {
        return Err("barcode scan output exceeds limit".into());
    }
    let bytes = std::fs::read(output).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    fn qr_image(payload: &str) -> RgbaImage {
        use rxing::Writer;
        let bits = rxing::qrcode::QRCodeWriter
            .encode(payload, &rxing::BarcodeFormat::QR_CODE, 96, 96)
            .unwrap();
        RgbaImage::from_fn(96, 96, |x, y| {
            if bits.get(x, y) {
                image::Rgba([0, 0, 0, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        })
    }

    #[test]
    fn helper_preserves_unicode_qr_detection() {
        let image = qr_image("clipboard 日本語 ✓");
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.rgba");
        let output = directory.path().join("output.json");
        write_input(&image, &input, &AtomicBool::new(false)).unwrap();
        run_helper(&input, &output).unwrap();
        let decoded: Vec<BarcodeDetection> =
            serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(decoded, little_clip_ops::barcode::detect_barcodes(&image));
        assert_eq!(decoded[0].text, "clipboard 日本語 ✓");
    }

    #[test]
    fn helper_rejects_dimensions_that_overflow_buffer_length() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.rgba");
        let output = directory.path().join("output.json");
        let mut header = Vec::new();
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        header.extend_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&input, header).unwrap();

        assert_eq!(
            run_helper(&input, &output).unwrap_err(),
            "barcode image dimensions overflow"
        );
        assert!(!output.exists());
    }

    #[test]
    fn cancellation_kills_and_reaps_running_helper() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("worker.sh");
        let pid_path = directory.path().join("pid");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nsleep 30 &\nchild=$!\nprintf '%s %s' \"$$\" \"$child\" > {:?}\nwait \"$child\"\n",
                pid_path
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let control = Arc::new(ImageToolProcessControl::new());
        let worker_control = Arc::clone(&control);
        let worker = std::thread::spawn(move || {
            scan_in_child(RgbaImage::new(1, 1), &script, &worker_control)
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while !pid_path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(pid_path.exists(), "helper did not start");
        let pids = std::fs::read_to_string(pid_path).unwrap();
        let mut pids = pids
            .split_whitespace()
            .map(|pid| pid.parse::<i32>().unwrap());
        let helper_pid = pids.next().unwrap();
        let child_pid = pids.next().unwrap();
        assert_eq!(unsafe { libc::kill(helper_pid, 0) }, 0);
        assert_eq!(unsafe { libc::kill(child_pid, 0) }, 0);
        control.cancel();
        assert_eq!(
            worker.join().unwrap().unwrap_err(),
            "barcode scan cancelled"
        );
        assert_eq!(unsafe { libc::kill(helper_pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while unsafe { libc::kill(child_pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::kill(child_pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[test]
    fn oversized_output_fails_visibly_without_partial_detections() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("oversized.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\ndd if=/dev/zero of=\"$3\" bs=1048576 count=9 status=none\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let error = scan_in_child(
            RgbaImage::new(1, 1),
            &script,
            &ImageToolProcessControl::new(),
        )
        .unwrap_err();
        assert_eq!(error, "barcode scan output exceeds limit");
    }

    #[test]
    fn bounded_serializer_stops_before_limit_and_never_publishes_partial_json() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("output.json");
        let detection = BarcodeDetection {
            format: "QR Code".into(),
            text: "x".repeat(1024),
            points: Vec::new(),
            bbox: [0.0; 4],
            is_url: false,
        };
        let error = write_bounded_json(&output, &vec![detection], 128).unwrap_err();
        assert_eq!(error, "barcode scan output exceeds limit");
        assert!(!output.exists());
        let partial_size = std::fs::metadata(output.with_extension("partial"))
            .unwrap()
            .len();
        assert!(partial_size <= 128);
    }
}
