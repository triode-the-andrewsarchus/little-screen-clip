// SPDX-License-Identifier: MPL-2.0
//! D2 master 6a7b48378 lib/localfile/localfile.go and open_unix.go.
//! Explicit local-file policy for imageasset sessions. Root traversal uses
//! retained descriptors, never a canonicalize-then-open security check.
// Rooted policies need descriptor-relative opens and exist only on Unix.
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]
use super::{ImageFileReader, UnrestrictedImageFiles, clean, context, error, limit, quote};
use crate::ImageResolveError;
use d2_rs_core::CancellationToken;
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{Arc, Mutex};

/// Standalone localfile policy failure. Lexical policy denials retain their
/// typed identity, separately from OS errors (including symlink escapes).
#[derive(Clone, Debug)]
pub struct LocalFileError {
    pub message: String,
    pub unavailable: bool,
    pub denied: bool,
}
impl std::fmt::Display for LocalFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for LocalFileError {}
impl From<LocalFileError> for ImageResolveError {
    fn from(e: LocalFileError) -> Self {
        Self {
            message: e.message,
            unavailable: e.unavailable,
        }
    }
}
fn local_error(message: impl Into<String>) -> LocalFileError {
    LocalFileError {
        message: message.into(),
        unavailable: false,
        denied: false,
    }
}

#[derive(Clone, Debug, Default)]
pub enum ImageLocalFiles {
    #[default]
    Denied,
    Unrestricted,
    #[cfg(unix)]
    Rooted(Arc<RootState>),
}

#[cfg(unix)]
#[derive(Debug)]
pub struct RootState {
    path: PathBuf,
    handle: Mutex<(Option<std::fs::File>, Option<LocalFileError>)>,
    scope: [u8; 32],
}

impl ImageLocalFiles {
    #[cfg(unix)]
    pub fn rooted(path: &Path) -> Result<Self, ImageResolveError> {
        use std::os::unix::fs::OpenOptionsExt;
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            std::env::current_dir()
                .map_err(|e| error(e.to_string()))?
                .join(path)
        };
        let absolute = clean(&absolute);
        let handle = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(&absolute)
            .map_err(|e| {
                error(format!(
                    "localfile: open root {}: {e}",
                    quote(&path.to_string_lossy())
                ))
            })?;
        let mut scope = [0; 32];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut scope))
            .map_err(|e| {
                error(format!(
                    "localfile: create cache scope for root {}: {e}",
                    quote(&path.to_string_lossy())
                ))
            })?;
        Ok(Self::Rooted(Arc::new(RootState {
            path: absolute,
            handle: Mutex::new((Some(handle), None)),
            scope,
        })))
    }

    pub fn root_path(&self) -> Option<&Path> {
        match self {
            #[cfg(unix)]
            Self::Rooted(root) => Some(&root.path),
            _ => None,
        }
    }

    /// Copies share one close; already-retained resource bytes remain valid.
    pub fn close(&self) {
        let _ = self.try_close();
    }

    /// Close exactly once across copies, retaining the original close result.
    pub fn try_close(&self) -> Result<(), LocalFileError> {
        #[cfg(unix)]
        if let Self::Rooted(root) = self {
            use std::os::fd::IntoRawFd;
            let mut guard = root.handle.lock().unwrap();
            if let Some(file) = guard.0.take() {
                // SAFETY: ownership leaves File exactly once. No reader can
                // borrow this descriptor while the root mutex is held.
                if unsafe { libc::close(file.into_raw_fd()) } != 0 {
                    guard.1 = Some(local_error(format!(
                        "localfile: close root {}: {}",
                        quote(&root.path.to_string_lossy()),
                        std::io::Error::last_os_error()
                    )));
                }
            }
            return guard.1.clone().map_or(Ok(()), Err);
        }
        Ok(())
    }

    fn resolved(&self, path: &Path) -> Result<PathBuf, LocalFileError> {
        match self {
            Self::Denied => Err(LocalFileError {
                message: "localfile: access denied: local-file access is disabled".into(),
                unavailable: false,
                denied: true,
            }),
            Self::Unrestricted => Ok(clean(&if path.is_absolute() {
                path.to_owned()
            } else {
                std::env::current_dir()
                    .map_err(|e| local_error(e.to_string()))?
                    .join(path)
            })),
            #[cfg(unix)]
            Self::Rooted(root) => {
                let path_clean = lexical_clean(path);
                let relative = if path_clean.is_absolute() {
                    path_clean.strip_prefix(&root.path).ok().map(Path::to_owned)
                } else {
                    Some(path_clean)
                };
                if let Some(relative) = relative {
                    if !relative
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                    {
                        return Ok(if relative.as_os_str().is_empty() {
                            PathBuf::from(".")
                        } else {
                            relative
                        });
                    }
                }
                Err(LocalFileError {
                    message: format!(
                        "localfile: access denied: {} is outside configured root {}",
                        quote(&path.to_string_lossy()),
                        quote(&root.path.to_string_lossy())
                    ),
                    unavailable: false,
                    denied: true,
                })
            }
        }
    }

    /// localfile.Policy.Open: acquire and verify a regular-file descriptor.
    /// Reads and seeks belong to the caller; no image decoding or read limit is
    /// imposed at this lower-level boundary.
    pub fn open(&self, path: &Path) -> Result<std::fs::File, LocalFileError> {
        let resolved = self.resolved(path)?;
        let file = match self {
            Self::Unrestricted => {
                let mut options = std::fs::OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(libc::O_NONBLOCK);
                }
                options.open(&resolved).map_err(|e| LocalFileError {
                    unavailable: e.kind() == std::io::ErrorKind::NotFound,
                    denied: false,
                    message: format!("open {}: {}", resolved.display(), io_detail(&e)),
                })?
            }
            #[cfg(unix)]
            Self::Rooted(root) => {
                let wrap = |unavailable, detail| LocalFileError {
                    unavailable,
                    denied: false,
                    message: format!(
                        "localfile: open {} beneath root {}: {detail}",
                        quote(&path.to_string_lossy()),
                        quote(&root.path.to_string_lossy())
                    ),
                };
                let guard = root.handle.lock().unwrap();
                let handle = guard.0.as_ref().ok_or_else(|| {
                    wrap(
                        false,
                        format!("openat {}: file already closed", resolved.display()),
                    )
                })?;
                open_beneath(handle, &resolved)
                    .map_err(|(missing, detail)| wrap(missing, detail))?
            }
            Self::Denied => unreachable!("denied resolution failed"),
        };
        let metadata = file.metadata().map_err(|e| {
            local_error(format!(
                "localfile: stat opened file {}: {e}",
                quote(&path.to_string_lossy())
            ))
        })?;
        if !metadata.is_file() {
            return Err(local_error(format!(
                "localfile: {} is not a regular file",
                quote(&path.to_string_lossy())
            )));
        }
        Ok(file)
    }

    pub fn policy_cache_key(&self, path: &Path) -> Result<String, LocalFileError> {
        let resolved = self.resolved(path)?;
        Ok(match self {
            Self::Unrestricted => format!("unrestricted:{}", resolved.display()),
            #[cfg(unix)]
            Self::Rooted(root) => {
                use std::fmt::Write;
                let mut scope = String::new();
                for byte in root.scope {
                    write!(scope, "{byte:02x}").unwrap();
                }
                format!("root:{scope}:{}", resolved.display())
            }
            Self::Denied => unreachable!(),
        })
    }
}

fn io_detail(e: &std::io::Error) -> String {
    #[cfg(windows)]
    return d2_rs_core::windows_io::error_message(e);
    #[cfg(not(windows))]
    match e.kind() {
        std::io::ErrorKind::NotFound => "no such file or directory".into(),
        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
        _ => e.to_string(),
    }
}

// filepath.Clean preserves leading parent components in relative names.
fn lexical_clean(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(result.components().next_back(), Some(Component::Normal(_))) {
                    result.pop();
                } else if !path.is_absolute() {
                    result.push("..");
                }
            }
            _ => result.push(component.as_os_str()),
        }
    }
    if result.as_os_str().is_empty() {
        result.push(".");
    }
    result
}

impl ImageFileReader for ImageLocalFiles {
    fn root_path(&self) -> Option<&Path> {
        ImageLocalFiles::root_path(self)
    }

    fn cache_key(&self, path: &Path) -> Result<Option<String>, ImageResolveError> {
        Ok(Some(self.policy_cache_key(path)?))
    }

    fn read(
        &self,
        path: &Path,
        max_bytes: i64,
        cancel: Option<&CancellationToken>,
    ) -> Result<Vec<u8>, ImageResolveError> {
        context(cancel)?;
        let relative = self.resolved(path)?;
        if matches!(self, Self::Unrestricted) {
            return UnrestrictedImageFiles::new().read(&relative, max_bytes, cancel);
        }
        #[cfg(unix)]
        if let Self::Rooted(_) = self {
            let file = self.open(path).map_err(|cause| ImageResolveError {
                unavailable: cause.unavailable,
                message: format!(
                    "open local file {}: {cause}",
                    quote(&path.to_string_lossy())
                ),
            })?;
            let metadata = file.metadata().map_err(|e| error(e.to_string()))?;
            limit(
                "fetched bytes",
                metadata.len().min(i64::MAX as u64) as i64,
                max_bytes,
            )?;
            let mut input = file.take(max_bytes.saturating_add(1) as u64);
            let mut bytes = Vec::new();
            let mut chunk = [0; 32 << 10];
            loop {
                context(cancel)?;
                let n = input.read(&mut chunk).map_err(|e| error(e.to_string()))?;
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&chunk[..n]);
                limit("fetched bytes", bytes.len() as i64, max_bytes)?;
            }
            context(cancel)?;
            return Ok(bytes);
        }
        unreachable!("denied policy failed resolution")
    }
}

/// Go os.Root's descriptor walk: follow relative symlinks within the retained
/// root, bound link traversal and reject every attempt to climb above it.
#[cfg(unix)]
fn open_beneath(root: &std::fs::File, path: &Path) -> Result<std::fs::File, (bool, String)> {
    use std::collections::VecDeque;
    use std::ffi::{CString, OsString};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let io_error = |e: std::io::Error| {
        let detail = match e.kind() {
            std::io::ErrorKind::NotFound => "no such file or directory".into(),
            std::io::ErrorKind::PermissionDenied => "permission denied".into(),
            _ => e.to_string(),
        };
        (
            e.kind() == std::io::ErrorKind::NotFound,
            format!("openat {}: {detail}", path.display()),
        )
    };
    let mut stack = vec![root.try_clone().map_err(io_error)?];
    let mut pending: VecDeque<OsString> = path
        .as_os_str()
        .as_bytes()
        .split(|&b| b == b'/')
        .map(|s| OsString::from_vec(s.to_vec()))
        .collect();
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if stack.len() == 1 {
                return Err((
                    false,
                    format!("openat {}: path escapes from parent", path.display()),
                ));
            }
            stack.pop();
            continue;
        }
        let name = CString::new(part.as_bytes()).map_err(|_| {
            (
                false,
                format!("openat {}: invalid argument", path.display()),
            )
        })?;
        let parent = stack.last().unwrap().as_raw_fd();
        let flags = libc::O_RDONLY
            | libc::O_NONBLOCK
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | if pending.is_empty() {
                0
            } else {
                libc::O_DIRECTORY
            };
        // SAFETY: the borrowed parent descriptor and nul-terminated component
        // are live throughout openat. Successful descriptors acquire File ownership.
        let fd = unsafe { libc::openat(parent, name.as_ptr(), flags) };
        if fd >= 0 {
            let file = unsafe { std::fs::File::from_raw_fd(fd) };
            if pending.is_empty() {
                return Ok(file);
            }
            stack.push(file);
            continue;
        }
        let open_error = std::io::Error::last_os_error();
        // O_DIRECTORY may report ENOTDIR for a symlink. readlinkat never
        // follows it, and only a successful read admits another traversal.
        let mut target = vec![0; 4096];
        let n = unsafe {
            libc::readlinkat(
                parent,
                name.as_ptr(),
                target.as_mut_ptr().cast(),
                target.len(),
            )
        };
        if n < 0 {
            return Err(io_error(open_error));
        }
        links += 1;
        if links > 8 {
            return Err((
                false,
                format!(
                    "openat {}: too many levels of symbolic links",
                    path.display()
                ),
            ));
        }
        target.truncate(n as usize);
        if target.starts_with(b"/") {
            return Err((
                false,
                format!("openat {}: path escapes from parent", path.display()),
            ));
        }
        for part in target.split(|&b| b == b'/').rev() {
            pending.push_front(OsString::from_vec(part.to_vec()));
        }
    }
    stack
        .pop()
        .ok_or_else(|| (false, "invalid rooted policy".into()))
}
