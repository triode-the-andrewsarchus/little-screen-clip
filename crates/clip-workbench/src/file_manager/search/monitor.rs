//! Bounded directory notifications and reconciliation scheduling.
use super::reader::{self, SourceRoot};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::File,
    io::Read,
    os::fd::{AsRawFd, FromRawFd},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const WATCH_LIMIT: usize = 8192;
const PERIODIC: Duration = Duration::from_secs(300);

struct RootWatch {
    source: SourceRoot,
    last_full: Instant,
    full_scan_inflight: bool,
    due: Option<Instant>,
    full: bool,
    dirty: BTreeSet<PathBuf>,
    failures: u8,
}
pub(super) struct Refresh {
    pub id: i64,
    pub root: SourceRoot,
    /// None requests a full reconciliation; Some limits it to these parents.
    pub directories: Option<Vec<PathBuf>>,
}
#[derive(Default)]
pub(super) struct Monitor {
    fd: Option<Arc<File>>,
    roots: BTreeMap<i64, RootWatch>,
    expected: HashMap<PathBuf, SourceRoot>,
    watches: HashMap<i32, Vec<(i64, PathBuf)>>,
    invalidations: BTreeSet<(i64, Option<PathBuf>)>,
}
impl Monitor {
    /// Retire notification coverage without forgetting the expected sources.
    pub fn reset_watches(&mut self) {
        self.fd = None;
        self.watches.clear();
        self.invalidations.clear();
        self.roots.clear();
    }
    pub fn register(&mut self, id: i64, source: SourceRoot) {
        self.expected
            .entry(source.path.clone())
            .or_insert_with(|| source.clone());
        self.roots.entry(id).or_insert_with(|| RootWatch {
            source,
            last_full: Instant::now(),
            full_scan_inflight: false,
            due: None,
            full: false,
            dirty: BTreeSet::new(),
            failures: 0,
        });
    }
    pub fn started(&mut self, id: i64, source: SourceRoot) {
        self.expected.insert(source.path.clone(), source.clone());
        self.register(id, source.clone());
        let root = self.roots.get_mut(&id).unwrap();
        root.source = source;
        root.last_full = Instant::now();
        root.full_scan_inflight = true;
        root.dirty.clear();
        root.full = false;
        root.due = None;
    }
    pub fn expected(&self, path: &Path) -> Option<&SourceRoot> {
        self.expected.get(path)
    }
    pub fn bind_source(&self, mut stored: SourceRoot) -> SourceRoot {
        stored.mount = self
            .expected(&stored.path)
            .filter(|expected| (expected.device, expected.inode) == (stored.device, stored.inode))
            .and_then(|expected| expected.mount);
        stored.session = self
            .expected(&stored.path)
            .filter(|expected| (expected.device, expected.inode) == (stored.device, stored.inode))
            .and_then(|expected| expected.session.clone());
        stored
    }
    pub fn check_source(&self, source: &SourceRoot) -> Result<(), String> {
        if self.expected(&source.path).is_some_and(|expected| {
            expected.mount.is_some()
                && (expected.mount != source.mount || expected.session != source.session)
        }) {
            return Err("Indexed mount changed; remove its enrollment before choosing this mounted source again".into());
        }
        Ok(())
    }
    pub fn failed(&mut self, id: i64) {
        if let Some(root) = self.roots.get_mut(&id) {
            root.full_scan_inflight = false;
            let seconds = (30u64 << root.failures.min(4)).min(300);
            root.failures = root.failures.saturating_add(1);
            root.full = true;
            root.due = Some(Instant::now() + Duration::from_secs(seconds));
        }
    }
    pub fn succeeded(&mut self, id: i64) {
        if let Some(root) = self.roots.get_mut(&id) {
            if root.full_scan_inflight {
                // A crawl can outlast the backstop. Measure the next interval
                // from completion instead of immediately invalidating it again.
                root.last_full = Instant::now();
                root.full_scan_inflight = false;
            }
            if !root.full {
                root.failures = 0;
            }
        }
    }
    pub fn retry_directory(&mut self, id: i64, path: &Path, delay: Duration) {
        if let Some(root) = self.roots.get_mut(&id)
            && path.starts_with(&root.source.path)
        {
            root.dirty.insert(path.into());
            let due = Instant::now() + delay;
            root.due = Some(root.due.map_or(due, |pending| pending.min(due)));
        }
    }
    pub fn remove(&mut self, id: i64, path: Option<&Path>) {
        let retired = self.roots.remove(&id);
        if let Some(path) = path.or_else(|| retired.as_ref().map(|root| root.source.path.as_path()))
        {
            self.expected.remove(path);
        }
        self.invalidations.retain(|(root, _)| *root != id);
        let mut retired = Vec::new();
        self.watches.retain(|wd, owners| {
            owners.retain(|(root, _)| *root != id);
            if owners.is_empty() {
                retired.push(*wd);
                false
            } else {
                true
            }
        });
        for wd in retired {
            self.remove_watch(wd);
        }
    }
    fn remove_watch(&self, wd: i32) {
        if let Some(fd) = &self.fd {
            // SAFETY: fd owns this inotify instance; wd came from it.
            unsafe {
                libc::inotify_rm_watch(fd.as_raw_fd(), wd);
            }
        }
    }
    pub fn prepare_watch(&mut self, pending: usize) -> Result<Arc<File>, String> {
        // Even a known pathname may now refer to a different inode. Reserve
        // space for every unaccepted registration, including replacements.
        if self.watches.len().saturating_add(pending) >= WATCH_LIMIT {
            return Err("Directory watch limit reached".into());
        }
        if self.fd.is_none() {
            // SAFETY: successful initialization returns a newly owned descriptor.
            let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            self.fd = Some(Arc::new(unsafe { File::from_raw_fd(fd) }));
        }
        Ok(self.fd.as_ref().unwrap().clone())
    }
    pub fn complete_watch(
        &mut self,
        id: i64,
        path: &Path,
        instance: &Arc<File>,
        registration: Result<reader::WatchRegistration, String>,
    ) -> Result<(), String> {
        if !self.owns_instance(instance) {
            return Err("Watch registration belongs to a retired instance".into());
        }
        let wd = self.accept_registration(registration)?;
        // A replaced directory at the same name must not keep the old inode's watch.
        let mut retired = Vec::new();
        self.watches.retain(|old_wd, owners| {
            owners.retain(|(owner, old)| !(*owner == id && old == path));
            if owners.is_empty() && *old_wd != wd {
                retired.push(*old_wd);
                false
            } else {
                true
            }
        });
        for old in retired {
            self.remove_watch(old);
        }
        let owners = self.watches.entry(wd).or_default();
        owners.retain(|(owner, _)| *owner != id);
        owners.push((id, path.into()));
        Ok(())
    }
    fn owns_instance(&self, instance: &Arc<File>) -> bool {
        self.fd
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, instance))
    }
    pub fn abandon_watch(&mut self, instance: &Arc<File>) {
        // Cancellation cannot prove whether the helper installed a watch.
        // Never retire a replacement instance on an old helper's behalf.
        if self.owns_instance(instance) {
            self.retire_instance(Instant::now());
        }
    }
    fn accept_registration(
        &mut self,
        reply: Result<reader::WatchRegistration, String>,
    ) -> Result<i32, String> {
        match reply {
            Ok(reader::WatchRegistration::Ready(wd)) => Ok(wd),
            Ok(reader::WatchRegistration::Unavailable(error)) => Err(error),
            Err(error) => {
                // A lost helper reply or failed post-install validation can
                // conceal a watch change. Retire only these uncertain outcomes.
                self.retire_instance(Instant::now());
                Err(error)
            }
        }
    }
    fn retire_instance(&mut self, now: Instant) {
        self.fd = None;
        self.watches.clear();
        self.gap(now);
    }
    fn gap(&mut self, now: Instant) {
        for (id, root) in &mut self.roots {
            root.full = true;
            root.due = Some(now);
            self.invalidations.insert((*id, None));
        }
    }
    fn event(&mut self, wd: i32, mask: u32, now: Instant) {
        if mask & libc::IN_Q_OVERFLOW != 0 {
            self.gap(now);
            return;
        }
        let Some(owners) = self.watches.get(&wd).cloned() else {
            return;
        };
        let lost = mask
            & (libc::IN_IGNORED | libc::IN_UNMOUNT | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF)
            != 0;
        if lost {
            self.watches.remove(&wd);
            if mask & libc::IN_IGNORED == 0 {
                self.remove_watch(wd);
            }
        }
        for (id, path) in owners {
            let Some(root) = self.roots.get_mut(&id) else {
                continue;
            };
            root.full |= lost;
            if root.dirty.len() < WATCH_LIMIT {
                root.dirty.insert(path.clone());
            } else {
                root.full = true;
            }
            root.due = Some(now + Duration::from_secs(1));
            self.invalidations
                .insert((id, if lost { None } else { Some(path) }));
        }
    }
    fn events(&mut self, bytes: &[u8], now: Instant) {
        let mut cursor = 0;
        while cursor < bytes.len() {
            let Some(header) = bytes.get(cursor..cursor + 16) else {
                self.gap(now);
                return;
            };
            let wd = i32::from_ne_bytes(header[0..4].try_into().unwrap());
            let mask = u32::from_ne_bytes(header[4..8].try_into().unwrap());
            let length = u32::from_ne_bytes(header[12..16].try_into().unwrap()) as usize;
            if length > bytes.len() - cursor - 16 {
                self.gap(now);
                return;
            }
            self.event(wd, mask, now);
            cursor += 16 + length;
        }
    }
    pub fn poll(&mut self) -> Vec<(i64, Option<PathBuf>)> {
        let now = Instant::now();
        let mut bytes = [0u8; 65536];
        for _ in 0..8 {
            let Some(fd) = self.fd.as_ref() else {
                break;
            };
            let mut fd = fd.as_ref();
            match fd.read(&mut bytes) {
                Ok(0) => {
                    self.retire_instance(now);
                    break;
                }
                Ok(count) => self.events(&bytes[..count], now),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.retire_instance(now);
                    break;
                }
            }
        }
        self.take_invalidations()
    }
    pub fn take_invalidations(&mut self) -> Vec<(i64, Option<PathBuf>)> {
        std::mem::take(&mut self.invalidations)
            .into_iter()
            .collect()
    }
    pub fn due(&mut self, busy: &HashSet<i64>, now: Instant) -> Vec<Refresh> {
        let mut refresh = Vec::new();
        for (id, root) in &mut self.roots {
            if busy.contains(id) {
                continue;
            }
            let periodic = now.duration_since(root.last_full) >= PERIODIC && root.failures == 0;
            if !periodic && !root.due.is_some_and(|due| now >= due) {
                continue;
            }
            let directories = if periodic || root.full {
                None
            } else {
                Some(std::mem::take(&mut root.dirty).into_iter().collect())
            };
            root.full = false;
            root.due = None;
            if directories.is_none() {
                root.last_full = now;
            }
            refresh.push(Refresh {
                id: *id,
                root: root.source.clone(),
                directories,
            });
        }
        refresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_deadline_uses_directory_scheduling_and_waits_for_busy_root() {
        let mut monitor = fixture();
        let now = Instant::now();
        monitor.retry_directory(1, Path::new("/content/late"), Duration::from_secs(120));
        monitor.retry_directory(1, Path::new("/content/early"), Duration::from_secs(60));
        monitor.retry_directory(1, Path::new("/elsewhere"), Duration::ZERO);
        assert!(
            monitor
                .due(&HashSet::new(), now + Duration::from_secs(59))
                .is_empty()
        );
        assert!(
            monitor
                .due(&HashSet::from([1]), now + Duration::from_secs(61))
                .is_empty()
        );
        let refresh = monitor.due(&HashSet::new(), now + Duration::from_secs(61));
        assert_eq!(refresh.len(), 1);
        assert_eq!(
            refresh[0].directories.as_ref().unwrap(),
            &vec![
                PathBuf::from("/content/early"),
                PathBuf::from("/content/late")
            ]
        );
    }
    fn fixture() -> Monitor {
        let mut monitor = Monitor::default();
        monitor.register(
            1,
            SourceRoot {
                path: "/content".into(),
                device: 1,
                inode: 2,
                mount: None,
                session: None,
            },
        );
        monitor.watches.insert(7, vec![(1, "/content/leaf".into())]);
        monitor
    }
    #[test]
    fn equal_reusable_mount_numbers_do_not_override_a_changed_source_session() {
        let directory = tempfile::tempdir().unwrap();
        let root = super::super::policy::Root::open(directory.path()).unwrap();
        let source = SourceRoot::from_root(&root);
        let mut monitor = Monitor::default();
        monitor.started(1, source.clone());
        let mut replacement = source.clone();
        let mut session = serde_json::to_value(source.session.as_ref().unwrap()).unwrap();
        session["boot"] = serde_json::json!(uuid::Uuid::new_v4());
        replacement.session = Some(serde_json::from_value(session).unwrap());
        assert_eq!(replacement.mount, source.mount);
        assert!(monitor.check_source(&replacement).is_err());
        monitor.reset_watches();
        assert!(monitor.check_source(&replacement).is_err());
    }

    #[test]
    fn stored_roots_acquire_only_the_observed_current_mount_and_reject_replacements() {
        let mut monitor = fixture();
        let stored = monitor.expected(Path::new("/content")).unwrap().clone();
        assert!(monitor.bind_source(stored.clone()).mount.is_none());
        let mut observed = stored.clone();
        observed.mount = Some(17);
        monitor.started(1, observed.clone());
        assert_eq!(monitor.bind_source(stored.clone()).mount, Some(17));
        assert!(monitor.check_source(&observed).is_ok());
        observed.mount = Some(18);
        assert!(monitor.check_source(&observed).is_err());
        monitor.reset_watches();
        assert!(monitor.watches.is_empty());
        assert!(
            monitor.check_source(&observed).is_err(),
            "A watch reset cannot adopt a new mount"
        );
        let mut replaced = stored;
        replaced.inode += 1;
        assert!(monitor.bind_source(replaced).mount.is_none());
        let original = monitor.expected(Path::new("/content")).unwrap().clone();
        monitor.started(7, original.clone());
        assert_eq!(
            monitor.roots.len(),
            1,
            "A reset must retire old database scheduler IDs"
        );
        assert_eq!(monitor.bind_source(original.clone()).mount, Some(17));
        monitor.remove(7, None);
        assert!(
            monitor.check_source(&observed).is_ok(),
            "Explicit removal releases the old expected mount"
        );
    }
    #[test]
    fn retired_watch_replies_and_cancellation_cannot_change_a_replacement_instance() {
        let mut monitor = fixture();
        let path = Path::new("/content/leaf");
        let old = monitor.prepare_watch(0).unwrap();
        monitor.abandon_watch(&old);
        assert_eq!(monitor.take_invalidations(), [(1, None)]);
        let current = monitor.prepare_watch(0).unwrap();
        assert!(!Arc::ptr_eq(&old, &current));
        monitor
            .complete_watch(1, path, &current, Ok(reader::WatchRegistration::Ready(42)))
            .unwrap();
        assert!(
            monitor
                .complete_watch(1, path, &old, Ok(reader::WatchRegistration::Ready(41)))
                .is_err()
        );
        assert!(
            monitor
                .complete_watch(1, path, &old, Err("Late lost reply".into()))
                .is_err()
        );
        monitor.abandon_watch(&old);
        assert!(monitor.owns_instance(&current));
        assert_eq!(monitor.watches.len(), 1);
        assert_eq!(monitor.watches[&42], [(1, path.into())]);
        assert!(monitor.take_invalidations().is_empty());
        monitor.abandon_watch(&current);
        assert!(monitor.fd.is_none());
        assert_eq!(monitor.take_invalidations(), [(1, None)]);
    }

    #[test]
    fn pending_registrations_reserve_the_remaining_watch_budget() {
        let mut monitor = fixture();
        monitor.watches = (0..WATCH_LIMIT as i32 - 1)
            .map(|wd| (wd, vec![(1, PathBuf::from("/content/known"))]))
            .collect();
        assert!(monitor.prepare_watch(1).is_err());
        assert!(monitor.prepare_watch(0).is_ok());
        monitor.watches.insert(
            WATCH_LIMIT as i32 - 1,
            vec![(1, PathBuf::from("/content/known"))],
        );
        assert!(monitor.prepare_watch(0).is_err());
        assert_eq!(monitor.watches.len(), WATCH_LIMIT);
    }

    #[test]
    fn definite_registration_failure_preserves_coverage_but_unknown_outcomes_retire_it() {
        let mut monitor = fixture();
        let definite = Ok(reader::WatchRegistration::Unavailable(
            "Permission denied".into(),
        ));
        assert!(monitor.accept_registration(definite).is_err());
        assert_eq!(monitor.watches.len(), 1);
        assert!(monitor.invalidations.is_empty());
        assert!(monitor.roots[&1].due.is_none());
        assert!(
            monitor
                .accept_registration(Err("Helper reply lost".into()))
                .is_err()
        );
        assert!(monitor.watches.is_empty());
        assert_eq!(monitor.poll(), [(1, None)]);
        assert!(
            monitor.due(&HashSet::new(), Instant::now())[0]
                .directories
                .is_none()
        );
    }

    #[test]
    fn shared_watch_routes_each_owner_and_survives_removing_the_enclosing_root() {
        let mut monitor = fixture();
        let mut child = monitor.expected(Path::new("/content")).unwrap().clone();
        child.path = "/content/leaf".into();
        monitor.register(2, child);
        monitor
            .watches
            .get_mut(&7)
            .unwrap()
            .push((2, "/content/leaf".into()));
        let now = Instant::now();
        monitor.event(7, libc::IN_CLOSE_WRITE, now);
        assert_eq!(monitor.poll().len(), 2);
        let ready = monitor.due(&HashSet::from([1]), now + Duration::from_secs(2));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, 2);
        monitor.remove(1, None);
        assert_eq!(monitor.watches[&7], [(2, PathBuf::from("/content/leaf"))]);
        monitor.event(7, libc::IN_MODIFY, now + Duration::from_secs(3));
        assert_eq!(monitor.poll(), [(2, Some(PathBuf::from("/content/leaf")))]);
        let ready = monitor.due(&HashSet::new(), now + Duration::from_secs(5));
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].id, 2);
        monitor.event(7, libc::IN_IGNORED, now + Duration::from_secs(6));
        assert!(!monitor.watches.contains_key(&7));
        assert_eq!(monitor.poll(), [(2, None)]);
        assert!(
            monitor.due(&HashSet::new(), now + Duration::from_secs(8))[0]
                .directories
                .is_none()
        );
    }

    #[test]
    fn events_debounce_parent_work_and_never_overlap_a_busy_root() {
        let mut monitor = fixture();
        let now = Instant::now();
        monitor.event(7, libc::IN_MODIFY, now);
        monitor.event(7, libc::IN_CLOSE_WRITE, now + Duration::from_millis(500));
        assert!(
            monitor
                .due(&HashSet::new(), now + Duration::from_secs(1))
                .is_empty()
        );
        assert!(
            monitor
                .due(&HashSet::from([1]), now + Duration::from_secs(2))
                .is_empty()
        );
        let refresh = monitor.due(&HashSet::new(), now + Duration::from_secs(2));
        assert_eq!(refresh.len(), 1);
        assert_eq!(
            refresh[0].directories.as_ref().unwrap(),
            &[PathBuf::from("/content/leaf")]
        );
        assert!(
            monitor
                .due(&HashSet::new(), now + Duration::from_secs(3))
                .is_empty()
        );
        assert!(
            monitor.fd.is_none(),
            "Scheduling alone must not open a watch descriptor"
        );
    }
    #[test]
    fn overflow_and_malformed_packets_require_full_reconciliation() {
        let mut monitor = fixture();
        let now = Instant::now();
        monitor.event(-1, libc::IN_Q_OVERFLOW, now);
        assert!(monitor.invalidations.contains(&(1, None)));
        assert!(monitor.due(&HashSet::new(), now)[0].directories.is_none());
        monitor.events(&[0; 5], now);
        assert!(monitor.due(&HashSet::new(), now)[0].directories.is_none());
        monitor.retire_instance(now);
        assert!(monitor.watches.is_empty());
        assert!(monitor.due(&HashSet::new(), now)[0].directories.is_none());
        monitor.remove(1, None);
        assert!(monitor.watches.is_empty());
        assert!(monitor.due(&HashSet::new(), now + PERIODIC).is_empty());
    }
    #[test]
    fn long_full_scan_restarts_backstop_at_completion_but_deltas_do_not() {
        let mut monitor = fixture();
        let source = monitor.roots[&1].source.clone();
        monitor.started(1, source.clone());
        monitor.roots.get_mut(&1).unwrap().last_full = Instant::now() - PERIODIC;
        monitor.succeeded(1);
        let completed = monitor.roots[&1].last_full;
        assert!(
            monitor
                .due(
                    &HashSet::new(),
                    completed + PERIODIC - Duration::from_secs(1)
                )
                .is_empty()
        );
        monitor.event(7, libc::IN_MODIFY, completed);
        let delta = monitor.due(&HashSet::new(), completed + Duration::from_secs(1));
        assert_eq!(delta.len(), 1);
        assert_eq!(
            delta[0].directories.as_ref().unwrap(),
            &[PathBuf::from("/content/leaf")]
        );
        monitor.succeeded(1);
        assert_eq!(
            monitor.roots[&1].last_full, completed,
            "directory deltas cannot postpone the full backstop"
        );
        assert!(
            monitor.due(&HashSet::new(), completed + PERIODIC)[0]
                .directories
                .is_none()
        );
        monitor.started(1, source);
        monitor.event(-1, libc::IN_Q_OVERFLOW, Instant::now());
        monitor.succeeded(1);
        assert!(
            monitor.due(&HashSet::new(), Instant::now())[0]
                .directories
                .is_none(),
            "completion must not discard a coverage gap received during the scan"
        );
    }

    #[test]
    fn periodic_checks_cannot_be_postponed_forever_by_dirty_events() {
        let mut monitor = fixture();
        let now = Instant::now();
        monitor.roots.get_mut(&1).unwrap().last_full = now - PERIODIC;
        monitor.event(7, libc::IN_MODIFY, now);
        assert!(monitor.due(&HashSet::new(), now)[0].directories.is_none());
        monitor.failed(1);
        assert!(monitor.due(&HashSet::new(), now).is_empty());
        let due = monitor.roots[&1].due.unwrap();
        assert!(monitor.due(&HashSet::new(), due)[0].directories.is_none());
    }
}
