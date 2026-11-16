//! Bounded inotify wakeups with periodic reconciliation as the loss recovery path.
use crate::Result;
use std::{
    collections::HashSet,
    ffi::CString,
    fs::{self, File},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
};

pub(crate) struct Watch {
    file: File,
    paths: HashSet<PathBuf>,
}
impl Watch {
    pub fn new() -> Result<Self> {
        // SAFETY: inotify_init1 returns a fresh owned nonblocking descriptor.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self {
            file: unsafe { File::from_raw_fd(fd) },
            paths: HashSet::new(),
        })
    }
    pub fn add(&mut self, path: &Path) {
        if self.paths.contains(path) {
            return;
        }
        let mut stack = vec![path.to_owned()];
        while let Some(path) = stack.pop() {
            if self.paths.len() >= 8192 {
                return;
            }
            let Ok(c) = CString::new(path.as_os_str().as_bytes()) else {
                continue;
            };
            let mask = libc::IN_CLOSE_WRITE
                | libc::IN_MOVED_TO
                | libc::IN_MOVED_FROM
                | libc::IN_CREATE
                | libc::IN_DELETE
                | libc::IN_DELETE_SELF
                | libc::IN_MOVE_SELF;
            // SAFETY: path is terminated and the borrowed descriptor remains live.
            if unsafe { libc::inotify_add_watch(self.file.as_raw_fd(), c.as_ptr(), mask) } < 0 {
                continue;
            }
            self.paths.insert(path.clone());
            if let Ok(reader) = fs::read_dir(&path) {
                for entry in reader.flatten() {
                    if entry.file_type().is_ok_and(|t| t.is_dir()) {
                        stack.push(entry.path());
                        if stack.len() > 8192 {
                            break;
                        }
                    }
                }
            }
        }
    }
    pub fn changed(&mut self) -> bool {
        let mut buffer = [0_u8; 65536];
        let mut changed = false;
        loop {
            // SAFETY: buffer is writable for its full length; fd is nonblocking.
            let read = unsafe {
                libc::read(
                    self.file.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if read <= 0 {
                break;
            }
            changed = true;
        }
        changed
    }
}
