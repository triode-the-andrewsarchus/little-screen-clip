//! A worker-owned Timelens process keeps its decoder warm between scrub requests.
use super::{VideoMetadata, backend};
use little_clip_core::ImageThumbnail;
use std::{
    io::{Read, Write},
    os::fd::AsRawFd,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(super) struct Frame {
    pub time_ms: u64,
    pub image: ImageThumbnail,
}
pub(super) struct Decoder {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    width: u32,
    height: u32,
}
impl Decoder {
    pub fn open(path: &Path, metadata: &VideoMetadata, width: u32) -> Result<Self, String> {
        let scale = (width as f64 / metadata.dimensions.0 as f64)
            .min((width * 2 / 3) as f64 / metadata.dimensions.1 as f64);
        let width = (metadata.dimensions.0 as f64 * scale).round().max(1.0) as u32;
        let height = (metadata.dimensions.1 as f64 * scale).round().max(1.0) as u32;
        let mut command =
            little_clip_core::process::CommandRequest::new(backend::helper().into_os_string())
                .arg_content_file(path)
                .map_err(|error| error.to_string())?
                .build_command();
        command.args([
            "--preview-server",
            "--width",
            &width.to_string(),
            "--height",
            &height.to_string(),
            "--video-stream",
            &metadata.stream.to_string(),
        ]);
        if metadata.codec == "hevc" {
            command.arg("--gpu");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Cannot open scrub decoder: {e}"))?;
        Ok(Self {
            input: child.stdin.take().unwrap(),
            output: child.stdout.take().unwrap(),
            child,
            width,
            height,
        })
    }
    pub fn frame(&mut self, time_ms: u64, cancelled: &AtomicBool) -> Result<Frame, String> {
        writeln!(self.input, "{time_ms}").map_err(|e| e.to_string())?;
        let mut bytes = vec![0; 8 + (self.width * self.height * 4) as usize];
        self.read(&mut bytes, cancelled)?;
        let time_ms = u64::from_le_bytes(bytes[..8].try_into().unwrap());
        let mut pixels = bytes.split_off(8);
        // BGRx -> opaque RGBA; no PNG encoding, decoding, or alpha conversion.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        Ok(Frame {
            time_ms,
            image: ImageThumbnail {
                width: self.width,
                height: self.height,
                premultiplied_rgba: pixels,
            },
        })
    }
    fn read(&mut self, mut bytes: &mut [u8], cancelled: &AtomicBool) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !bytes.is_empty() {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Scrub cancelled".into());
            }
            if Instant::now() >= deadline {
                return Err("Scrub decoder timed out".into());
            }
            let mut descriptor = libc::pollfd {
                fd: self.output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: the single descriptor borrows our live pipe, owned by this worker.
            let ready = unsafe { libc::poll(&mut descriptor, 1, 10) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.to_string());
            }
            if ready == 0 {
                continue;
            }
            let count = self.output.read(bytes).map_err(|e| e.to_string())?;
            if count == 0 {
                return Err("Scrub decoder closed; refresh previews to retry".into());
            }
            bytes = &mut bytes[count..];
        }
        Ok(())
    }
}
impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
