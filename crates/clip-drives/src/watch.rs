//! Bounded, descriptor-backed change notifications; lost coverage requests a full scan.
use crate::{
    Result,
    root::{Root, anchor},
};
use std::{
    collections::{HashMap, HashSet},
    ffi::CString,
    fs::{self, File},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::PathBuf,
    sync::Arc,
};
#[derive(Default)]
pub(crate) struct Changes {
    pub full: bool,
    pub paths: HashSet<PathBuf>,
}
struct Location {
    root: Arc<Root>,
    relative: PathBuf,
}
pub(crate) struct Watch {
    file: File,
    paths: HashMap<PathBuf, i32>,
    locations: HashMap<i32, Location>,
    lost: bool,
}
impl Watch {
    pub fn new() -> Result<Self> {
        // SAFETY: inotify_init1 returns a fresh owned nonblocking descriptor.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self {
            // SAFETY: this fresh descriptor is transferred exactly once.
            file: unsafe { File::from_raw_fd(fd) },
            paths: HashMap::new(),
            locations: HashMap::new(),
            lost: false,
        })
    }
    pub fn add(&mut self, root: Arc<Root>) {
        if let Some(wd) = self.paths.get(&root.path).copied() {
            if self
                .locations
                .get(&wd)
                .is_some_and(|l| l.root.validate().is_ok())
            {
                return;
            }
            self.forget(wd);
        }
        self.add_tree(root, PathBuf::new());
    }
    fn forget(&mut self, wd: i32) {
        if let Some(location) = self.locations.remove(&wd) {
            self.paths
                .remove(&location.root.path.join(location.relative));
        }
    }
    fn add_tree(&mut self, root: Arc<Root>, relative: PathBuf) {
        let mut stack = vec![relative];
        while let Some(relative) = stack.pop() {
            let path = root.path.join(&relative);
            if let Some(wd) = self.paths.get(&path).copied() {
                if self
                    .locations
                    .get(&wd)
                    .is_some_and(|l| l.root.validate().is_ok())
                {
                    continue;
                }
                self.forget(wd);
            }
            if self.paths.len() >= 8192 {
                self.lost = true;
                return;
            }
            let Ok(pin) = root.directory(&relative, false) else {
                self.lost = true;
                continue;
            };
            let Ok(c) = CString::new(anchor(&pin).as_os_str().as_bytes()) else {
                continue;
            };
            let mask = libc::IN_CLOSE_WRITE
                | libc::IN_ATTRIB
                | libc::IN_MOVED_TO
                | libc::IN_MOVED_FROM
                | libc::IN_CREATE
                | libc::IN_DELETE
                | libc::IN_DELETE_SELF
                | libc::IN_MOVE_SELF
                | libc::IN_ONLYDIR;
            // SAFETY: the path is terminated and resolves to the retained directory.
            let wd = unsafe { libc::inotify_add_watch(self.file.as_raw_fd(), c.as_ptr(), mask) };
            if wd < 0 {
                self.lost = true;
                continue;
            }
            self.paths.insert(path, wd);
            self.locations.insert(
                wd,
                Location {
                    root: root.clone(),
                    relative: relative.clone(),
                },
            );
            if let Ok(reader) = fs::read_dir(anchor(&pin)) {
                for child in reader.flatten() {
                    if child.file_type().is_ok_and(|t| t.is_dir()) {
                        stack.push(relative.join(child.file_name()));
                        if stack.len() >= 8192 {
                            self.lost = true;
                            break;
                        }
                    }
                }
            } else {
                self.lost = true;
            }
        }
    }
    fn event(&mut self, event: libc::inotify_event, name: &[u8], changes: &mut Changes) {
        if event.mask & libc::IN_Q_OVERFLOW != 0 {
            changes.full = true;
        }
        let Some(location) = self.locations.get(&event.wd) else {
            return;
        };
        if event.mask
            & (libc::IN_IGNORED | libc::IN_UNMOUNT | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF)
            != 0
        {
            changes.full = true;
            self.forget(event.wd);
            return;
        }
        if location.root.validate().is_err() {
            changes.full = true;
            self.forget(event.wd);
            return;
        }
        let name = std::ffi::OsStr::from_bytes(name.split(|b| *b == 0).next().unwrap_or_default());
        let relative = location.relative.join(name);
        if !crate::valid_relative(&relative) {
            changes.full = true;
            return;
        }
        if changes.paths.len() < 4096 {
            changes.paths.insert(location.root.path.join(&relative));
        } else {
            changes.full = true;
        }
        if event.mask & libc::IN_ISDIR != 0
            && event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0
        {
            self.add_tree(location.root.clone(), relative);
        }
    }
    pub fn changes(&mut self) -> Changes {
        let mut changes = Changes {
            full: std::mem::take(&mut self.lost),
            ..Default::default()
        };
        let mut buffer = [0_u8; 65536];
        loop {
            // SAFETY: the buffer is writable and the descriptor is nonblocking.
            let count = unsafe {
                libc::read(
                    self.file.as_raw_fd(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                )
            };
            if count < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                changes.full |= error.kind() != std::io::ErrorKind::WouldBlock;
                break;
            }
            if count == 0 {
                break;
            }
            let mut offset = 0;
            while offset + std::mem::size_of::<libc::inotify_event>() <= count as usize {
                // SAFETY: the complete event header is in bounds; byte buffers need unaligned reads.
                let event = unsafe {
                    std::ptr::read_unaligned(
                        buffer.as_ptr().add(offset).cast::<libc::inotify_event>(),
                    )
                };
                let start = offset + std::mem::size_of::<libc::inotify_event>();
                let end = start + event.len as usize;
                if end > count as usize {
                    changes.full = true;
                    break;
                }
                self.event(event, &buffer[start..end], &mut changes);
                offset = end;
            }
            if offset != count as usize {
                changes.full = true;
            }
        }
        changes
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_overflow_reports_lost_coverage_even_without_a_known_watch_descriptor() {
        let mut watch = Watch::new().unwrap();
        let mut changes = Changes::default();
        watch.event(
            libc::inotify_event {
                wd: -1,
                mask: libc::IN_Q_OVERFLOW,
                cookie: 0,
                len: 0,
            },
            &[],
            &mut changes,
        );
        assert!(changes.full);
        assert!(changes.paths.is_empty());
    }
    #[test]
    fn tracks_new_subtrees_and_replaced_roots_without_following_links() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("root");
        fs::create_dir(&path).unwrap();
        let mut watch = Watch::new().unwrap();
        watch.add(Arc::new(Root::open(&path).unwrap()));
        fs::create_dir(path.join("child")).unwrap();
        assert!(watch.changes().paths.contains(&path.join("child")));
        fs::write(path.join("child/file"), b"changed").unwrap();
        assert!(watch.changes().paths.contains(&path.join("child/file")));
        fs::rename(&path, temp.path().join("removed")).unwrap();
        assert!(watch.changes().full);
        fs::create_dir(&path).unwrap();
        watch.add(Arc::new(Root::open(&path).unwrap()));
        fs::write(path.join("new"), b"new insertion").unwrap();
        assert!(watch.changes().paths.contains(&path.join("new")));
    }
}
