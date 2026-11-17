//! Bounded, descriptor-backed change notifications; lost coverage requests a full scan.
use crate::{
    Result,
    root::{Root, anchor},
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::CString,
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::PathBuf,
    sync::Arc,
};
pub(crate) const MAX_CHANGED_PATHS: usize = 4096;
pub(crate) const MAX_CHANGED_BYTES: usize = 1024 * 1024;
#[derive(Default)]
pub(crate) struct Changes {
    pub full: bool,
    pub paths: HashSet<PathBuf>,
    path_bytes: usize,
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
    tracked_bytes: usize,
    pending: VecDeque<(Arc<Root>, PathBuf)>,
    walking: Option<(Arc<Root>, crate::scan::Frame)>,
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
            tracked_bytes: 0,
            pending: VecDeque::new(),
            walking: None,
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
    /// Releases retained pins across the partition's filesystem and bind aliases.
    pub fn remove_scope(&mut self, scope: &crate::root::Scope) {
        self.remove_matching(|root| scope.overlap(&root.scope()));
    }
    fn remove_matching(&mut self, matches: impl Fn(&Root) -> bool) {
        self.pending.retain(|(root, _)| !matches(root));
        if self.walking.as_ref().is_some_and(|(root, _)| matches(root)) {
            self.walking = None;
        }
        let descriptors = self
            .locations
            .iter()
            .filter_map(|(wd, location)| matches(&location.root).then_some(*wd))
            .collect::<Vec<_>>();
        for wd in descriptors {
            // SAFETY: this watch descriptor belongs to our live inotify instance.
            unsafe {
                libc::inotify_rm_watch(self.file.as_raw_fd(), wd);
            }
            self.forget(wd);
        }
    }
    fn forget(&mut self, wd: i32) {
        if let Some(location) = self.locations.remove(&wd) {
            let path = location.root.path.join(&location.relative);
            self.paths.remove(&path);
            self.tracked_bytes = self
                .tracked_bytes
                .saturating_sub(watch_bytes(&path, &location.relative));
            self.pending.retain(|(root, relative)| {
                !(root.same_root(&location.root) && *relative == location.relative)
            });
            if self.walking.as_ref().is_some_and(|(root, frame)| {
                root.same_root(&location.root) && frame.relative == location.relative
            }) {
                self.walking = None;
            }
        }
    }
    fn add_tree(&mut self, root: Arc<Root>, relative: PathBuf) {
        let path = root.path.join(&relative);
        if let Some(wd) = self.paths.get(&path).copied() {
            if self
                .locations
                .get(&wd)
                .is_some_and(|l| l.root.validate().is_ok())
            {
                return;
            }
            self.forget(wd);
        }
        let bytes = watch_bytes(&path, &relative);
        if self.paths.len() >= 8192
            || self.pending.len() >= 8192
            || relative.components().count() > 256
            || self.tracked_bytes.saturating_add(bytes) > 16 * 1024 * 1024
        {
            self.lost = true;
            return;
        }
        let Ok(pin) = root.directory(&relative, false) else {
            self.lost = true;
            return;
        };
        let Ok(c) = CString::new(anchor(&pin).as_os_str().as_bytes()) else {
            self.lost = true;
            return;
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
        // SAFETY: the terminated descriptor path resolves to the retained directory.
        let wd = unsafe { libc::inotify_add_watch(self.file.as_raw_fd(), c.as_ptr(), mask) };
        if wd < 0 {
            self.lost = true;
            return;
        }
        self.tracked_bytes += bytes;
        self.paths.insert(path, wd);
        self.locations.insert(
            wd,
            Location {
                root: root.clone(),
                relative: relative.clone(),
            },
        );
        self.pending.push_back((root, relative));
    }
    fn advance(&mut self) {
        let started = std::time::Instant::now();
        for _ in 0..4096 {
            if started.elapsed() >= std::time::Duration::from_millis(5) {
                break;
            }
            if self.walking.is_none() {
                let Some((root, relative)) = self.pending.pop_front() else {
                    break;
                };
                match crate::scan::Frame::new(&root, relative) {
                    Ok(frame) => self.walking = Some((root, frame)),
                    Err(_) => {
                        self.lost = true;
                        continue;
                    }
                }
            }
            let (root, frame) = self.walking.as_mut().expect("active bounded watch walk");
            match frame.reader.next() {
                None => self.walking = None,
                Some(Err(_)) => {
                    self.lost = true;
                    self.walking = None;
                }
                Some(Ok(child)) => match child.file_type() {
                    Ok(kind) if kind.is_dir() => {
                        let root = root.clone();
                        let relative = frame.relative.join(child.file_name());
                        self.add_tree(root, relative);
                    }
                    Err(_) => self.lost = true,
                    _ => (),
                },
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
        let path = location.root.path.join(&relative);
        if !changes.paths.contains(&path) {
            let bytes = path.as_os_str().as_bytes().len();
            if changes.paths.len() < MAX_CHANGED_PATHS
                && changes.path_bytes.saturating_add(bytes) <= MAX_CHANGED_BYTES
            {
                changes.paths.insert(path);
                changes.path_bytes += bytes;
            } else {
                changes.full = true;
            }
        }
        if event.mask & libc::IN_ISDIR != 0
            && event.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0
        {
            self.add_tree(location.root.clone(), relative);
        }
    }
    pub fn changes(&mut self) -> Changes {
        let incomplete = self.walking.is_some() || !self.pending.is_empty();
        self.advance();
        let lost = std::mem::take(&mut self.lost);
        let mut changes = Changes {
            full: incomplete || lost,
            ..Default::default()
        };
        let mut buffer = [0_u8; 65536];
        let started = std::time::Instant::now();
        let mut reads = 0;
        loop {
            if reads >= 32 || started.elapsed() >= std::time::Duration::from_millis(5) {
                changes.full = true;
                break;
            }
            reads += 1;
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
fn watch_bytes(path: &std::path::Path, relative: &std::path::Path) -> usize {
    path.as_os_str()
        .as_bytes()
        .len()
        .saturating_add(relative.as_os_str().as_bytes().len())
        .saturating_mul(4)
        .saturating_add(256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn retiring_a_partition_releases_queued_and_active_directory_pins() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("child")).unwrap();
        let root = Arc::new(Root::open(temp.path()).unwrap());
        let mut watch = Watch::new().unwrap();
        watch.add(root.clone());
        assert!(Arc::strong_count(&root) > 1);
        watch.changes();
        watch.remove_scope(&root.scope());
        assert_eq!(Arc::strong_count(&root), 1);
        assert_eq!(watch.tracked_bytes, 0);
        assert!(watch.pending.is_empty());
        assert!(watch.walking.is_none());
    }
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
