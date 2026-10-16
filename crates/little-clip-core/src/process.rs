//! Bounded execution of local external commands.
//!
//! This module is the shared boundary for tools launched by operations, the daemon, and the
//! workbench. It bounds captured output, kills timed-out children, and reports launch failures
//! consistently. Commands run with the application's user permissions; callers must treat script
//! paths and arguments as a local-code trust boundary.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::process::CommandExt,
};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;
use wait_timeout::ChildExt;

/// Default maximum number of bytes retained from each command output stream.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 256 * 1024;

/// Immutable settings for one local command execution.
#[derive(Debug, Clone)]
pub struct CommandRequest {
    program: OsString,
    args: Vec<OsString>,
    content_inputs: Vec<std::sync::Arc<std::fs::File>>,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
    max_output_bytes: usize,
    capture_stdout: bool,
    capture_stderr: bool,
}

impl CommandRequest {
    /// Creates a request with a 30-second timeout and bounded stdout/stderr capture.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            content_inputs: Vec::new(),
            stdin: None,
            timeout: Duration::from_secs(30),
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            capture_stdout: true,
            capture_stderr: true,
        }
    }

    /// Appends one opaque operating-system argument.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Pins an ordinary content input and appends its inherited descriptor path.
    ///
    /// Device nodes and kernel-interface files are rejected before opening their
    /// contents. The child keeps the original inode even if the source path changes.
    /// Inputs remain open for the lifetime of this request and are inherited only
    /// by the spawned child, without changing the parent's close-on-exec flags.
    ///
    /// # Errors
    /// Returns an I/O error if the path is unavailable, is not ordinary file
    /// content, or its descriptor cannot be duplicated.
    pub fn arg_content_file(self, path: impl AsRef<Path>) -> std::io::Result<Self> {
        self.arg_content_file_with_suffix(path, "")
    }
    /// Adds a pinned input with a tool-specific selector, such as ImageMagick's `[0]`.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::arg_content_file`].
    pub fn arg_content_file_with_suffix(
        self,
        path: impl AsRef<Path>,
        suffix: &str,
    ) -> std::io::Result<Self> {
        let source = crate::kernel_interface::open_content(path.as_ref(), true)?;
        self.arg_content_handle(source, OsStr::new(suffix))
    }

    /// Appends an output child path under an inherited ordinary directory handle.
    ///
    /// Use this for tool-owned files in private staging directories. The request
    /// retains the parent through command execution, including after pathname
    /// replacement. Existing protected targets are rejected without opening their
    /// contents. This does not lock directory entries or install a final output;
    /// callers retain staging ownership and enforce their publication policy.
    ///
    /// # Examples
    /// ```no_run
    /// use little_clip_core::process::CommandRequest;
    /// let staging = tempfile::tempdir()?;
    /// CommandRequest::new("qpdf")
    ///     .arg_content_file("input.pdf")?
    ///     .arg_content_output(staging.path().join("edited.pdf"))?
    ///     .run()?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    ///
    /// # Errors
    /// Returns an I/O error for an invalid filename, protected or unavailable
    /// parent/target, or descriptor duplication failure.
    pub fn arg_content_output(self, path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let name = path.file_name().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Missing output filename")
        })?;
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let directory = crate::kernel_interface::pin_content_directory(parent)?;
        let target = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name);
        match crate::kernel_interface::require_content(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut suffix = OsString::from("/");
        suffix.push(name);
        self.arg_content_handle(directory, &suffix)
    }

    fn arg_content_handle(
        mut self,
        source: std::fs::File,
        suffix: &OsStr,
    ) -> std::io::Result<Self> {
        // SAFETY: source owns a live descriptor; F_DUPFD_CLOEXEC returns a new
        // owned descriptor above stdio, which is transferred into File exactly once.
        let fd = unsafe { libc::fcntl(source.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let source = unsafe { std::fs::File::from_raw_fd(fd) };
        let mut argument = OsString::from(format!("/proc/self/fd/{fd}"));
        argument.push(suffix);
        self.args.push(argument);
        self.content_inputs.push(std::sync::Arc::new(source));
        Ok(self)
    }

    /// Appends multiple opaque operating-system arguments.
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
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

    /// Disables stdout capture, directing stdout to null.
    pub fn discard_stdout(mut self) -> Self {
        self.capture_stdout = false;
        self
    }

    /// Disables stderr capture, directing stderr to null.
    pub fn discard_stderr(mut self) -> Self {
        self.capture_stderr = false;
        self
    }

    /// Disables both stdout and stderr capture.
    pub fn discard_output(mut self) -> Self {
        self.capture_stdout = false;
        self.capture_stderr = false;
        self
    }

    /// Runs this request, collecting bounded output while the child executes.
    ///
    /// # Errors
    ///
    /// Returns [`CommandError::Spawn`] if the process executable cannot be started,
    /// [`CommandError::Writer`] if standard input writing fails,
    /// [`CommandError::Wait`] if waiting for the child process fails,
    /// or [`CommandError::TimedOut`] if the command exceeds the configured timeout duration.
    pub fn run(&self) -> Result<CommandOutput, CommandError> {
        self.run_inner(None, None)
    }

    /// Runs a read-only command until completion, cancellation, or timeout.
    ///
    /// Checks cancellation every 50 ms, kills and reaps the child, and drains
    /// output readers before returning. Accepted writes should use [`Self::run`]
    /// so cancellation cannot hide effects that already happened.
    ///
    /// # Errors
    ///
    /// Returns [`CommandError::Cancelled`] when the token is set, or the same
    /// launch, I/O, and timeout errors as [`Self::run`].
    pub fn run_cancellable(&self, cancelled: &AtomicBool) -> Result<CommandOutput, CommandError> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(CommandError::Cancelled);
        }
        self.run_inner(Some(cancelled), None)
    }

    /// Runs a cancellable read-only command, polling progress on the calling worker.
    ///
    /// The callback runs at most once per 50 ms wait interval. Keep it bounded;
    /// it can inspect immutable helper snapshots without another polling thread.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::run_cancellable`].
    pub fn run_cancellable_with_progress(
        &self,
        cancelled: &AtomicBool,
        mut progress: impl FnMut(),
    ) -> Result<CommandOutput, CommandError> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(CommandError::Cancelled);
        }
        self.run_inner(Some(cancelled), Some(&mut progress))
    }

    fn inherit_content_inputs(&self, command: &mut Command) {
        let inputs = self.content_inputs.clone();
        if !inputs.is_empty() {
            // SAFETY: only async-signal-safe fcntl calls run after fork. Parent
            // descriptors remain CLOEXEC; inheritance is enabled only in this child.
            unsafe {
                command.pre_exec(move || {
                    for input in &inputs {
                        let fd = input.as_raw_fd();
                        let flags = libc::fcntl(fd, libc::F_GETFD);
                        if flags < 0
                            || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                        {
                            return Err(std::io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
        }
    }

    /// Builds a command whose pinned inputs remain owned through spawning.
    ///
    /// The caller owns child lifetime, pipe handling, cancellation and timeouts.
    /// Use [`Self::run`] for ordinary bounded commands. This is for persistent
    /// decoders and players with their own existing lifecycle management.
    pub fn build_command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        self.inherit_content_inputs(&mut command);
        command
    }

    fn run_inner(
        &self,
        cancelled: Option<&AtomicBool>,
        progress: Option<&mut dyn FnMut()>,
    ) -> Result<CommandOutput, CommandError> {
        let mut command = self.build_command();
        command
            .stdin(
                self.stdin
                    .as_ref()
                    .map_or(Stdio::null(), |_| Stdio::piped()),
            )
            .stdout(if self.capture_stdout {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stderr(if self.capture_stderr {
                Stdio::piped()
            } else {
                Stdio::null()
            });
        let mut child = command.spawn().map_err(|source| CommandError::Spawn {
            program: self.program.clone(),
            source,
        })?;

        let limit = self.max_output_bytes;
        let stdout_reader = child
            .stdout
            .take()
            .map(|stdout| thread::spawn(move || read_capped(stdout, limit)));
        let stderr_reader = child
            .stderr
            .take()
            .map(|stderr| thread::spawn(move || read_capped(stderr, limit)));

        let stdin_writer = self.stdin.clone().map(|bytes| {
            let mut stdin = child.stdin.take().expect("stdin was piped");
            thread::spawn(move || stdin.write_all(&bytes))
        });

        let status = match self.wait(&mut child, cancelled, progress) {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                drain_threads(stdout_reader, stderr_reader, stdin_writer);
                return Err(error);
            }
        };

        let (stdout, stdout_truncated) = stdout_reader
            .map(|reader| reader.join().map_err(|_| CommandError::Reader))
            .transpose()?
            .unwrap_or_default();
        let (stderr, stderr_truncated) = stderr_reader
            .map(|reader| reader.join().map_err(|_| CommandError::Reader))
            .transpose()?
            .unwrap_or_default();

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

    fn wait(
        &self,
        child: &mut std::process::Child,
        cancelled: Option<&AtomicBool>,
        mut progress: Option<&mut dyn FnMut()>,
    ) -> Result<ExitStatus, CommandError> {
        let start = Instant::now();
        loop {
            if cancelled.is_some_and(|token| token.load(Ordering::Relaxed)) {
                return Err(CommandError::Cancelled);
            }
            if let Some(callback) = progress.as_deref_mut() {
                callback();
            }
            let remaining = self.timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Err(CommandError::TimedOut {
                    program: self.program.clone(),
                    timeout: self.timeout,
                });
            }
            let interval = if cancelled.is_some() {
                remaining.min(Duration::from_millis(50))
            } else {
                remaining
            };
            if let Some(status) = child.wait_timeout(interval).map_err(CommandError::Wait)? {
                return Ok(status);
            }
        }
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
    /// The caller cancelled a read-only command; its child has been reaped.
    #[error("command cancelled")]
    Cancelled,
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
    stdout: Option<thread::JoinHandle<(Vec<u8>, bool)>>,
    stderr: Option<thread::JoinHandle<(Vec<u8>, bool)>>,
    stdin: Option<thread::JoinHandle<std::io::Result<()>>>,
) {
    if let Some(stdout) = stdout {
        let _ = stdout.join();
    }
    if let Some(stderr) = stderr {
        let _ = stderr.join();
    }
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
    #[test]
    fn progress_polling_preserves_cancellation_and_reaps_the_child() {
        let cancelled = AtomicBool::new(false);
        let mut polls = 0;
        let result = CommandRequest::new("sh")
            .args(["-c", "exec sleep 1"])
            .run_cancellable_with_progress(&cancelled, || {
                polls += 1;
                if polls == 2 {
                    cancelled.store(true, Ordering::Relaxed);
                }
            });
        assert!(matches!(result, Err(CommandError::Cancelled)));
        assert_eq!(polls, 2);
    }
}

#[cfg(test)]
mod pinned_input_tests {
    use super::*;
    #[test]
    fn prepared_output_command_keeps_its_directory_and_rejects_protected_targets() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("staging");
        std::fs::create_dir(&original).unwrap();
        let path = original.join("output.pdf");
        let request = CommandRequest::new("sh")
            .args(["-c", "printf 'completed output' > \"$1\"", "sh"])
            .arg_content_output(&path)
            .unwrap();
        let mut command = request.build_command();
        drop(request);
        let moved = root.path().join("moved");
        std::fs::rename(&original, &moved).unwrap();
        std::os::unix::fs::symlink("/proc", &original).unwrap();
        assert!(command.status().unwrap().success());
        assert_eq!(
            std::fs::read(moved.join("output.pdf")).unwrap(),
            b"completed output"
        );
        assert!(CommandRequest::new("sh").arg_content_output(&path).is_err());
        let protected = moved.join("protected");
        std::os::unix::fs::symlink("/dev/null", &protected).unwrap();
        assert!(
            CommandRequest::new("sh")
                .arg_content_output(&protected)
                .is_err()
        );
    }

    #[test]
    fn prepared_command_owns_inputs_after_request_drop_and_path_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ordinary");
        std::fs::write(&path, b"stable command input").unwrap();
        let request = CommandRequest::new("cat").arg_content_file(&path).unwrap();
        let fd = request.content_inputs[0].as_raw_fd();
        let mut command = request.build_command();
        drop(request);
        std::fs::rename(&path, directory.path().join("original")).unwrap();
        std::os::unix::fs::symlink("/dev/null", &path).unwrap();
        // SAFETY: the command's pre-exec closure owns this still-live descriptor.
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        let output = command.output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"stable command input");
    }
    #[test]
    fn child_reads_pinned_inode_after_path_is_replaced_by_kernel_alias() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ordinary");
        std::fs::write(&path, b"pinned ordinary content").unwrap();
        let command = CommandRequest::new("cat").arg_content_file(&path).unwrap();
        std::fs::rename(&path, directory.path().join("original")).unwrap();
        std::os::unix::fs::symlink("/proc/uptime", &path).unwrap();
        let output = command.run().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"pinned ordinary content");
        assert!(CommandRequest::new("cat").arg_content_file(&path).is_err());
    }
}
