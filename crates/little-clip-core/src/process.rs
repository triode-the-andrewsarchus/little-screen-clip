//! Bounded execution of local external commands.
//!
//! This module is the shared boundary for tools launched by operations, the daemon, and the
//! workbench. It bounds captured output, kills timed-out children, and reports launch failures
//! consistently. Commands run with the application's user permissions; callers must treat script
//! paths and arguments as a local-code trust boundary.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;
use thiserror::Error;
use wait_timeout::ChildExt;

/// Default maximum number of bytes retained from each command output stream.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// Immutable settings for one local command execution.
#[derive(Debug, Clone)]
pub struct CommandRequest {
    program: OsString,
    args: Vec<OsString>,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
    max_output_bytes: usize,
}

impl CommandRequest {
    /// Creates a request with a 30-second timeout and bounded stdout/stderr capture.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            stdin: None,
            timeout: Duration::from_secs(30),
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }

    /// Appends one opaque operating-system argument.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Supplies bytes to the command's standard input and closes it after writing.
    pub fn stdin_bytes(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.stdin = Some(bytes.into());
        self
    }

    /// Sets the maximum time the child may run before it is killed.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the maximum retained bytes for each of stdout and stderr.
    pub fn max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = max_output_bytes;
        self
    }

    /// Runs this request, collecting bounded output while the child executes.
    pub fn run(&self) -> Result<CommandOutput, CommandError> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(
                self.stdin
                    .as_ref()
                    .map_or(Stdio::null(), |_| Stdio::piped()),
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|source| CommandError::Spawn {
            program: self.program.clone(),
            source,
        })?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        let stdout_reader = thread::spawn({
            let limit = self.max_output_bytes;
            move || read_capped(stdout, limit)
        });
        let stderr_reader = thread::spawn({
            let limit = self.max_output_bytes;
            move || read_capped(stderr, limit)
        });
        let stdin_writer = self.stdin.clone().map(|bytes| {
            let mut stdin = child.stdin.take().expect("stdin was piped");
            thread::spawn(move || stdin.write_all(&bytes))
        });

        let status = match child
            .wait_timeout(self.timeout)
            .map_err(CommandError::Wait)?
        {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                drain_threads(stdout_reader, stderr_reader, stdin_writer);
                return Err(CommandError::TimedOut {
                    program: self.program.clone(),
                    timeout: self.timeout,
                });
            }
        };
        let (stdout, stdout_truncated) = stdout_reader.join().map_err(|_| CommandError::Reader)?;
        let (stderr, stderr_truncated) = stderr_reader.join().map_err(|_| CommandError::Reader)?;
        if let Some(writer) = stdin_writer {
            writer
                .join()
                .map_err(|_| CommandError::Writer(std::io::Error::other("stdin writer panicked")))?
                .map_err(CommandError::Writer)?;
        }

        Ok(CommandOutput {
            status,
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
        })
    }
}

/// Bounded command output and exit status.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    /// Process exit status.
    pub status: ExitStatus,
    /// Retained stdout bytes, potentially capped at the request limit.
    pub stdout: Vec<u8>,
    /// Retained stderr bytes, potentially capped at the request limit.
    pub stderr: Vec<u8>,
    /// Whether stdout contained more bytes than retained.
    pub stdout_truncated: bool,
    /// Whether stderr contained more bytes than retained.
    pub stderr_truncated: bool,
}

impl CommandOutput {
    /// Returns stdout decoded lossily for diagnostics or text-oriented commands.
    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Returns stderr decoded lossily, including a truncation marker when applicable.
    pub fn stderr_lossy(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.stderr).into_owned();
        if self.stderr_truncated {
            text.push_str("\n[stderr truncated]");
        }
        text
    }
}

/// Failure to launch, wait for, or collect one local command.
#[derive(Debug, Error)]
pub enum CommandError {
    /// The executable could not be launched.
    #[error("failed to spawn {program:?}: {source}")]
    Spawn {
        /// Requested executable path or name.
        program: OsString,
        /// Operating-system launch error.
        source: std::io::Error,
    },
    /// Waiting for the child failed.
    #[error("failed while waiting for command: {0}")]
    Wait(#[source] std::io::Error),
    /// Writing supplied stdin failed.
    #[error("failed while writing command stdin: {0}")]
    Writer(#[source] std::io::Error),
    /// A reader thread unexpectedly panicked.
    #[error("command output reader panicked")]
    Reader,
    /// The child was killed after exceeding its requested limit.
    #[error("{program:?} exceeded the {} second timeout", timeout.as_secs())]
    TimedOut {
        /// Requested executable path or name.
        program: OsString,
        /// Time limit that was exceeded.
        timeout: Duration,
    },
}

/// Resolves `program` through `PATH`, or accepts an executable path directly.
pub fn command_available(program: impl AsRef<OsStr>) -> Option<PathBuf> {
    let program = program.as_ref();
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|directory| {
        let candidate = directory.join(program);
        candidate.is_file().then_some(candidate)
    })
}

fn read_capped(mut reader: impl Read, limit: usize) -> (Vec<u8>, bool) {
    let mut retained = Vec::with_capacity(limit.min(8 * 1024));
    let mut buffer = [0_u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let Ok(read) = reader.read(&mut buffer) else {
            break;
        };
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(retained.len());
        let copied = read.min(remaining);
        retained.extend_from_slice(&buffer[..copied]);
        truncated |= copied < read;
    }
    (retained, truncated)
}

fn drain_threads(
    stdout: thread::JoinHandle<(Vec<u8>, bool)>,
    stderr: thread::JoinHandle<(Vec<u8>, bool)>,
    stdin: Option<thread::JoinHandle<std::io::Result<()>>>,
) {
    let _ = stdout.join();
    let _ = stderr.join();
    if let Some(stdin) = stdin {
        let _ = stdin.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_bounded_output_without_blocking_the_child() {
        let output = CommandRequest::new("sh")
            .arg("-c")
            .arg("printf 1234567890; printf abcdefghij >&2")
            .max_output_bytes(4)
            .run()
            .unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout, b"1234");
        assert_eq!(output.stderr, b"abcd");
        assert!(output.stdout_truncated);
        assert!(output.stderr_truncated);
    }

    #[test]
    fn kills_command_that_exceeds_timeout() {
        let error = CommandRequest::new("sh")
            .arg("-c")
            .arg("sleep 1")
            .timeout(Duration::from_millis(10))
            .run()
            .unwrap_err();
        assert!(matches!(error, CommandError::TimedOut { .. }));
    }
}
