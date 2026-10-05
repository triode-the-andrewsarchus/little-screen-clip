//! Shared synchronous desktop IPC transport; callers keep it off the UI thread.

use little_clip_core::ipc::{IpcRequest, IpcResponse};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub use little_clip_core::ipc::socket_path;

pub fn request(
    request: &IpcRequest,
    timeout: Option<Duration>,
    max_bytes: u64,
    max_request_bytes: usize,
) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    let mut encoded = BoundedRequest {
        bytes: Vec::new(),
        limit: max_request_bytes,
    };
    serde_json::to_writer(&mut encoded, request)?;
    let mut stream = UnixStream::connect(socket_path())?;
    stream.set_read_timeout(timeout)?;
    stream.set_write_timeout(timeout)?;
    stream.write_all(&encoded.bytes)?;
    stream.write_all(b"\n")?;
    let mut reader = BufReader::new(stream).take(max_bytes.saturating_add(1));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.len() as u64 > max_bytes {
        return Err("Clipboard response exceeds the input limit".into());
    }
    Ok(serde_json::from_str(&line)?)
}

struct BoundedRequest {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for BoundedRequest {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other(
                "Shared hand request exceeds the daemon's 1 MiB limit; place this content as a file instead",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
