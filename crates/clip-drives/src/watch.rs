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
    #[cfg(test)]
    kernel_overflow: bool,
}
mod registration;
use registration::{Location, Pending};

pub(crate) struct Watch {
    file: Arc<File>,
    paths: HashMap<PathBuf, i32>,
    locations: HashMap<i32, Vec<Location>>,
    entries: HashMap<PathBuf, Location>,
    lost: bool,
    registration_incomplete: bool,
    rebuild_required: bool,
    tracked_bytes: usize,
    pending: VecDeque<Location>,
    working: Vec<Pending>,
    jobs: little_clip_core::jobs::BackgroundJobs,
    serial: u64,
    epoch: u64,
}
impl Watch {
    pub fn new() -> Result<Self> {
        Ok(Self {
            file: Self::descriptor()?,
            paths: HashMap::new(),
            entries: HashMap::new(),
            locations: HashMap::new(),
            lost: false,
            registration_incomplete: false,
            rebuild_required: false,
            tracked_bytes: 0,
            pending: VecDeque::new(),
            working: Vec::new(),
            jobs: little_clip_core::jobs::BackgroundJobs::new_named(4, 4, "drive-watch"),
            serial: 0,
            epoch: 0,
        })
    }
    pub fn add(&mut self, root: Arc<Root>) {
        if self
            .entries
            .get(&root.path)
            .is_some_and(|entry| entry.root.same_root(&root))
        {
            return;
        }
        self.remove_matching(|prior| {
            prior.root.path == root.path || prior.path().starts_with(&root.path)
        });
        // An ancestor registration can occupy this spelling with a different
        // root lifetime. An explicitly prepared root supersedes that entry.
        self.forget_path(&root.path);
        self.add_tree(root, PathBuf::new());
    }
    /// Retires registrations; matching worker pins remain owned until actual delivery.
    pub fn remove_scope(&mut self, scope: &crate::root::Scope) {
        self.remove_matching(|location| scope.overlap(&location.root.scope()));
    }
    pub fn busy_scope(&self, scope: &crate::root::Scope) -> bool {
        self.working
            .iter()
            .any(|job| scope.overlap(&job.location.root.scope()))
    }
    pub fn registration_pending(&self) -> bool {
        !self.pending.is_empty() || !self.working.is_empty()
    }
    fn remove_matching(&mut self, matches: impl Fn(&Location) -> bool) {
        for job in &self.working {
            if matches(&job.location) {
                job.cancel();
            }
        }
        self.pending.retain(|entry| !matches(entry));
        let paths = self
            .entries
            .iter()
            .filter(|(_, entry)| matches(entry))
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        for path in paths {
            self.forget_path(&path);
        }
    }
    fn forget_path(&mut self, path: &std::path::Path) {
        let Some(entry) = self.entries.remove(path) else {
            return;
        };
        self.tracked_bytes = self
            .tracked_bytes
            .saturating_sub(watch_bytes(path, &entry.relative));
        self.pending
            .retain(|pending| pending.serial != entry.serial);
        for job in &self.working {
            if job.location.serial == entry.serial {
                job.cancel();
            }
        }
        if let Some(wd) = self.paths.remove(path) {
            if let Some(locations) = self.locations.get_mut(&wd) {
                locations.retain(|location| location.serial != entry.serial);
                if !locations.is_empty() {
                    return;
                }
            }
            self.locations.remove(&wd);
            // SAFETY: this descriptor belongs to the current inotify instance.
            unsafe {
                libc::inotify_rm_watch(self.file.as_raw_fd(), wd);
            }
        }
    }
    fn forget(&mut self, wd: i32) {
        if let Some(locations) = self.locations.remove(&wd) {
            for location in locations {
                self.paths.remove(&location.path());
                self.forget_path(&location.path());
            }
            // SAFETY: all aliases of this current-instance descriptor retired.
            unsafe {
                libc::inotify_rm_watch(self.file.as_raw_fd(), wd);
            }
        }
    }
    fn add_tree(&mut self, root: Arc<Root>, relative: PathBuf) {
        let path = root.path.join(&relative);
        if self.entries.contains_key(&path) {
            return;
        }
        let bytes = watch_bytes(&path, &relative);
        if self.entries.len() >= 8192
            || relative.components().count() > 256
            || self.tracked_bytes.saturating_add(bytes) > 16 * 1024 * 1024
        {
            self.lost = true;
            self.registration_incomplete = true;
            return;
        }
        self.serial = self
            .serial
            .checked_add(1)
            .expect("watch registration identity exhausted");
        let location = Location {
            root,
            relative,
            serial: self.serial,
        };
        self.entries.insert(path, location.clone());
        self.tracked_bytes += bytes;
        self.pending.push_back(location);
        self.lost = true;
    }
    fn current(&self, location: &Location, epoch: u64) -> bool {
        epoch == self.epoch
            && self
                .entries
                .get(&location.path())
                .is_some_and(|entry| entry.serial == location.serial)
    }
    fn acknowledge(&mut self, location: Location, epoch: u64, wd: i32) {
        if epoch != self.epoch {
            return;
        }
        if !self.current(&location, epoch) {
            // An unclaimed descriptor may alias a concurrent registration. Rebuild
            // the instance rather than claiming complete coverage for either.
            self.rebuild_required = true;
            return;
        }
        self.paths.insert(location.path(), wd);
        self.locations.entry(wd).or_default().push(location);
    }
    pub fn poll(&mut self) {
        let acknowledgements = self
            .working
            .iter()
            .filter_map(|job| {
                job.acknowledgement()
                    .map(|wd| (job.location.clone(), job.epoch, wd))
            })
            .collect::<Vec<_>>();
        for (location, epoch, wd) in acknowledgements {
            self.acknowledge(location, epoch, wd);
        }
        let mut index = 0;
        while index < self.working.len() {
            let Some(result) = self.working[index].task.poll() else {
                index += 1;
                continue;
            };
            let job = self.working.swap_remove(index);
            // The registration acknowledgement precedes task completion, but it
            // can have arrived after this poll's first nonblocking channel read.
            if let Some(wd) = job.acknowledgement() {
                self.acknowledge(job.location.clone(), job.epoch, wd);
            }
            if !self.current(&job.location, job.epoch) {
                continue;
            }
            self.lost = true;
            match result {
                Ok(Ok(inventory)) => {
                    self.registration_incomplete |= !inventory.complete;
                    for relative in inventory.children {
                        self.add_tree(job.location.root.clone(), relative);
                    }
                }
                _ => self.registration_incomplete = true,
            }
        }
    }
    fn advance(&mut self) {
        self.poll();
        if self.rebuild_required {
            return;
        }
        for _ in 0..self.pending.len() {
            if self.working.len() >= 4 {
                break;
            }
            let Some(location) = self.pending.pop_front() else {
                break;
            };
            // One slow tree cannot occupy every registration slot. Keep other
            // enrolled roots eligible while its directory reader is blocked.
            if self
                .working
                .iter()
                .any(|job| job.location.root.same_root(&location.root))
            {
                self.pending.push_back(location);
                continue;
            }
            if self.current(&location, self.epoch) {
                self.working.push(Pending::start(
                    &self.jobs,
                    self.file.clone(),
                    location,
                    self.epoch,
                ));
            }
        }
    }
    fn event(&mut self, event: libc::inotify_event, name: &[u8], changes: &mut Changes) {
        if event.mask & libc::IN_Q_OVERFLOW != 0 {
            changes.full = true;
            self.rebuild_required = true;
            #[cfg(test)]
            {
                changes.kernel_overflow = true;
            }
        }
        let Some(locations) = self.locations.get(&event.wd).cloned() else {
            changes.full = true;
            return;
        };
        if event.mask
            & (libc::IN_IGNORED | libc::IN_UNMOUNT | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF)
            != 0
        {
            changes.full = true;
            for location in &locations {
                if location.relative.as_os_str().is_empty() {
                    self.remove_matching(|prior| prior.root.same_root(&location.root));
                }
            }
            self.forget(event.wd);
            return;
        }
        for location in locations {
            self.named_event(&location, event.mask, name, changes);
        }
    }
    fn named_event(&mut self, location: &Location, mask: u32, name: &[u8], changes: &mut Changes) {
        // Events are untrusted hints. Root identity/content authority is checked
        // by registration and reconciliation workers, never by this event drain.
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
                changes.paths.insert(path.clone());
                changes.path_bytes += bytes;
            } else {
                changes.full = true;
            }
        }
        if mask & libc::IN_ISDIR != 0 {
            if mask & (libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_DELETE | libc::IN_MOVED_FROM)
                != 0
            {
                self.remove_matching(|prior| prior.path().starts_with(&path));
            }
            if mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 {
                self.add_tree(location.root.clone(), relative);
            }
        }
    }
    pub fn has_complete_coverage(&self) -> bool {
        !self.registration_incomplete
            && !self.rebuild_required
            && !self.lost
            && !self.registration_pending()
    }
    pub fn changes(&mut self, retry_registration: bool) -> Changes {
        self.rebuild_required |= retry_registration && self.registration_incomplete;
        self.advance();
        let lost = std::mem::take(&mut self.lost);
        let mut changes = Changes {
            full: lost,
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
        if self.rebuild_required {
            changes.full = true;
            if let Err(error) = self.rebuild() {
                tracing::warn!("Could not rebuild drive watches after event loss: {error}");
            }
        }
        changes
    }
    pub(crate) fn rebuild(&mut self) -> Result<()> {
        let roots = self
            .entries
            .values()
            .map(|entry| (entry.root.path.clone(), entry.root.clone()))
            .collect::<HashMap<_, _>>();
        let replacement = Self::descriptor()?;
        for job in &self.working {
            job.cancel();
        }
        self.file = replacement;
        self.epoch = self
            .epoch
            .checked_add(1)
            .expect("watch instance identity exhausted");
        self.paths.clear();
        self.locations.clear();
        self.entries.clear();
        self.pending.clear();
        self.tracked_bytes = 0;
        self.registration_incomplete = false;
        self.rebuild_required = false;
        for root in roots.into_values() {
            self.add_tree(root, PathBuf::new());
        }
        Ok(())
    }
    fn descriptor() -> Result<Arc<File>> {
        // SAFETY: a successful inotify_init1 returns a fresh owned descriptor.
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: transfer this descriptor exactly once.
        Ok(Arc::new(unsafe { File::from_raw_fd(fd) }))
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        self.remove_matching(|_| true);
        while !self.working.is_empty() {
            self.poll();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
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
pub(crate) mod overflow_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn moved_directories_and_replaced_names_keep_both_nested_event_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = Arc::new(Root::open(temp.path()).unwrap());
        fs::create_dir_all(root.path.join("child/deep")).unwrap();
        let mut watch = Watch::new().unwrap();
        watch.add(root.clone());
        overflow_tests::settle(&mut watch);
        fs::rename(root.path.join("child"), root.path.join("moved")).unwrap();
        fs::create_dir_all(root.path.join("child/deep")).unwrap();
        watch.changes(false);
        overflow_tests::settle(&mut watch);
        assert!(watch.has_complete_coverage());
        for name in ["child/deep/edit", "moved/deep/edit"] {
            fs::write(root.path.join(name), b"fresh nested event").unwrap();
        }
        let changes = watch.changes(false);
        assert!(!changes.full);
        assert!(changes.paths.contains(&root.path.join("child/deep/edit")));
        assert!(changes.paths.contains(&root.path.join("moved/deep/edit")));
    }
    #[test]
    fn retiring_a_partition_releases_queued_and_active_directory_pins() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("child")).unwrap();
        let root = Arc::new(Root::open(temp.path()).unwrap());
        let mut watch = Watch::new().unwrap();
        watch.add(root.clone());
        assert!(Arc::strong_count(&root) > 1);
        watch.changes(false);
        watch.remove_scope(&root.scope());
        assert!(watch.busy_scope(&root.scope()));
        overflow_tests::settle(&mut watch);
        assert_eq!(Arc::strong_count(&root), 1);
        assert_eq!(watch.tracked_bytes, 0);
        assert!(watch.pending.is_empty());
        assert!(watch.working.is_empty());
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
        overflow_tests::settle(&mut watch);
        fs::create_dir(path.join("child")).unwrap();
        assert!(watch.changes(false).paths.contains(&path.join("child")));
        overflow_tests::settle(&mut watch);
        fs::write(path.join("child/file"), b"changed").unwrap();
        assert!(
            watch
                .changes(false)
                .paths
                .contains(&path.join("child/file"))
        );
        fs::rename(&path, temp.path().join("removed")).unwrap();
        assert!(watch.changes(false).full);
        fs::create_dir(&path).unwrap();
        watch.add(Arc::new(Root::open(&path).unwrap()));
        overflow_tests::settle(&mut watch);
        fs::write(path.join("new"), b"new insertion").unwrap();
        assert!(watch.changes(false).paths.contains(&path.join("new")));
    }
}
