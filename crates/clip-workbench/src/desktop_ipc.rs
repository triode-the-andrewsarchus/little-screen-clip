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

/// An accepted daemon request whose result survives view changes and has no
/// post-write deadline. The owning UI must retain it until completion.
pub(crate) struct PendingRequest {
    receiver:
        std::sync::mpsc::Receiver<Result<Result<IpcResponse, String>, little_clip_core::JobError>>,
}

impl PendingRequest {
    pub(crate) fn submit(request: IpcRequest) -> Self {
        Self::submit_with(
            request,
            little_clip_core::BackgroundJobs::global(),
            |request| {
                self::request(&request, None, u64::MAX, usize::MAX)
                    .map_err(|error| error.to_string())
            },
        )
    }

    pub(crate) fn submit_with(
        request: IpcRequest,
        jobs: &little_clip_core::BackgroundJobs,
        send: impl FnOnce(IpcRequest) -> Result<IpcResponse, String> + Send + 'static,
    ) -> Self {
        Self::start(jobs, move || send(request))
    }

    pub(crate) fn start(
        jobs: &little_clip_core::BackgroundJobs,
        operation: impl FnOnce() -> Result<IpcResponse, String> + Send + 'static,
    ) -> Self {
        let (_, receiver) = jobs.submit(Duration::MAX, operation);
        Self { receiver }
    }

    pub(crate) fn try_result(&self) -> Option<Result<IpcResponse, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(
                result
                    .map_err(|error| error.to_string())
                    .and_then(|result| result),
            ),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(
                "Daemon worker disconnected; operation outcome is unknown".to_owned(),
            )),
        }
    }
}

pub(crate) fn action_result(response: IpcResponse) -> Result<(), String> {
    match response {
        IpcResponse::OpResult { .. } | IpcResponse::Success { .. } => Ok(()),
        IpcResponse::Error { error } => Err(error),
        _ => Err("unexpected response from daemon".to_owned()),
    }
}
