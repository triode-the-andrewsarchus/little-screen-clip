//! Read-only source jobs; completion never grants permission to mutate the catalog.
use super::super::reader::{self, Reply, Request};
use crate::file_manager::preview_job::Task;
use little_clip_core::BackgroundJobs;
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;
use std::{
    collections::HashMap,
    fs::File,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

const LIMIT: usize = 2;
type ReadTask = Task<Result<Reply, String>>;

pub(super) struct Readers {
    jobs: BackgroundJobs,
    pending: HashMap<i64, Pending>,
    retired: Vec<(i64, PathBuf, ReadTask)>,
    // Same admission limit as readers; a delivered inventory keeps its slot
    // until its queued application (or cancellation) drops the owned buffer.
    inventories: Vec<Weak<()>>,
}
struct Pending {
    generation: i64,
    path: PathBuf,
    signature: [u8; 32],
    watch: Option<Arc<File>>,
    task: ReadTask,
    ready: Option<Result<Reply, String>>,
}
impl Pending {
    fn affected_watch(&self) -> Option<Arc<File>> {
        if matches!(
            &self.ready,
            Some(Ok(Reply::Watch(reader::WatchRegistration::Unavailable(_))))
        ) {
            None // The typed reply guarantees that no watch was installed.
        } else {
            self.watch.clone()
        }
    }
}
impl Default for Readers {
    fn default() -> Self {
        Self {
            jobs: BackgroundJobs::new(LIMIT, LIMIT),
            pending: HashMap::new(),
            retired: Vec::new(),
            inventories: Vec::new(),
        }
    }
}
impl Readers {
    pub fn inventory_available(&self, root: i64) -> bool {
        self.pending.contains_key(&root)
            || self
                .inventories
                .iter()
                .filter(|slot| slot.strong_count() != 0)
                .count()
                < LIMIT
    }
    fn reserve_inventory(&mut self) -> Option<Arc<()>> {
        self.inventories.retain(|slot| slot.strong_count() != 0);
        if self.inventories.len() >= LIMIT {
            return None;
        }
        let permit = Arc::new(());
        self.inventories.push(Arc::downgrade(&permit));
        Some(permit)
    }
    pub fn can_start(&self, root: i64) -> bool {
        !self.pending.contains_key(&root)
            && !self.retired.iter().any(|(old, _, _)| *old == root)
            && self.pending.len() + self.retired.len() < LIMIT
    }
    pub fn can_run(&self, root: i64, path: &Path) -> bool {
        match self.pending.get(&root) {
            Some(pending) => pending.path == path && pending.ready.is_some(),
            None => {
                self.can_start(root)
                    && !self.pending.values().any(|pending| pending.path == path)
                    && !self.retired.iter().any(|(_, retired, _)| retired == path)
            }
        }
    }
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.retired.is_empty()
    }
    pub fn poll(&mut self) {
        // Cancellation is not completion. Keep admission occupied until the
        // owned command has actually returned after killing/reaping its child.
        self.retired.retain(|(_, _, task)| task.poll().is_none());
        for pending in self.pending.values_mut() {
            if pending.ready.is_none()
                && let Some(reply) = pending.task.poll()
            {
                pending.ready = Some(
                    reply
                        .map_err(|error| error.to_string())
                        .and_then(|reply| reply),
                );
            }
        }
    }
    pub fn request(
        &mut self,
        root: i64,
        generation: i64,
        path: &Path,
        request: Request,
    ) -> Result<Option<Reply>, String> {
        self.submit(root, generation, path, request, None)
    }
    pub fn watch_handle(&self, root: i64) -> Option<Arc<File>> {
        self.pending
            .get(&root)
            .and_then(|pending| pending.watch.clone())
    }
    pub fn watch_count(&self) -> usize {
        self.pending
            .values()
            .filter(|pending| pending.watch.is_some())
            .count()
    }
    pub fn watch(
        &mut self,
        root: i64,
        generation: i64,
        path: &Path,
        source: reader::SourceRoot,
        handle: Arc<File>,
    ) -> Result<Option<reader::WatchRegistration>, String> {
        let request = Request::Watch {
            root: source,
            path: path.into(),
            fd: handle.as_raw_fd(),
        };
        match self.submit(root, generation, path, request, Some(handle))? {
            None => Ok(None),
            Some(Reply::Watch(registration)) => Ok(Some(registration)),
            _ => Err("Invalid watch registration reply".into()),
        }
    }
    fn submit(
        &mut self,
        root: i64,
        generation: i64,
        path: &Path,
        request: Request,
        watch: Option<Arc<File>>,
    ) -> Result<Option<Reply>, String> {
        let signature: [u8; 32] =
            Sha256::digest(serde_json::to_vec(&request).map_err(|error| error.to_string())?).into();
        if let Some(pending) = self.pending.get_mut(&root) {
            if pending.generation != generation
                || pending.path != path
                || pending.signature != signature
                || pending.watch.is_some() != watch.is_some()
            {
                return Err("Search reader request was superseded".into());
            }
            let Some(reply) = pending.ready.take() else {
                return Ok(None);
            };
            self.pending.remove(&root);
            return reply.map(Some);
        }
        if !self.can_run(root, path) {
            return Ok(None);
        }
        let permit = if matches!(&request, Request::List { .. }) {
            let Some(permit) = self.reserve_inventory() else {
                return Ok(None);
            };
            Some(permit)
        } else {
            None
        };
        let owned_watch = watch.clone();
        let task = Task::start_unbounded(&self.jobs, move |cancel| {
            let mut reply = match owned_watch {
                Some(handle) => {
                    let Request::Watch { root, path, .. } = request else {
                        return Err("Invalid owned watch request".into());
                    };
                    reader::watch(&root, &path, &handle, cancel).map(Reply::Watch)
                }
                None => reader::call(&request, cancel),
            };
            if let Ok(Reply::Directory(inventory)) = &mut reply {
                inventory.permit = permit;
            }
            reply
        });
        self.pending.insert(
            root,
            Pending {
                generation,
                path: path.into(),
                signature,
                watch,
                task,
                ready: None,
            },
        );
        Ok(None)
    }
    pub fn cancel_root(&mut self, root: i64) -> Option<Arc<File>> {
        let pending = self.pending.remove(&root)?;
        let watch = pending.affected_watch();
        if pending.ready.is_none() {
            pending.task.token.store(true, Ordering::Relaxed);
            self.retired.push((root, pending.path, pending.task));
        }
        watch
    }
    pub fn clear(&mut self) -> Vec<Arc<File>> {
        let mut watches = Vec::new();
        for (root, pending) in self.pending.drain() {
            watches.extend(pending.affected_watch());
            if pending.ready.is_none() {
                pending.task.token.store(true, Ordering::Relaxed);
                self.retired.push((root, pending.path, pending.task));
            }
        }
        watches
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, atomic::AtomicBool, mpsc},
        time::{Duration, Instant},
    };

    fn held(
        readers: &mut Readers,
        root: i64,
        watch: Option<Arc<File>>,
        inventory: Option<Arc<()>>,
    ) -> (mpsc::Sender<()>, Arc<AtomicBool>) {
        let path = PathBuf::from(format!("/owned/{root}"));
        let request = Request::Probe(path.clone());
        let signature = Sha256::digest(serde_json::to_vec(&request).unwrap()).into();
        let (started_tx, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let owned_watch = watch.clone();
        let task = Task::start_unbounded(&readers.jobs, move |_| {
            started_tx.send(()).unwrap();
            let _ = released.recv();
            drop(owned_watch);
            drop(inventory);
            Ok(Reply::Verified)
        });
        let token = task.token.clone();
        readers.pending.insert(
            root,
            Pending {
                generation: 7,
                path,
                signature,
                watch,
                task,
                ready: None,
            },
        );
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        (release, token)
    }
    fn await_condition(readers: &mut Readers, condition: impl Fn(&Readers) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !condition(readers) {
            assert!(Instant::now() < deadline);
            readers.poll();
            std::thread::yield_now();
        }
    }

    #[test]
    fn inventory_admission_survives_delivery_and_retirement_without_blocking_file_reads() {
        let mut readers = Readers::default();
        let first = readers.reserve_inventory().unwrap();
        let second = readers.reserve_inventory().unwrap();
        let fixture = tempfile::tempdir().unwrap();
        let directory = crate::file_manager::filesystem::read_directory(
            fixture.path(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let delivered = reader::Inventory {
            directory,
            storage_bytes: 0,
            permit: Some(second),
        };
        let encoded = serde_json::to_value(&delivered).unwrap();
        assert!(encoded.get("permit").is_none());
        let decoded: reader::Inventory = serde_json::from_value(encoded).unwrap();
        assert!(
            decoded.permit.is_none(),
            "a helper cannot manufacture an admission permit"
        );
        let (release, _) = held(&mut readers, 1, None, Some(first));
        readers.cancel_root(1);
        assert!(!readers.inventory_available(99));
        assert!(readers.reserve_inventory().is_none());
        assert!(
            readers.can_start(99),
            "ordinary file work retains the other reader slot"
        );
        let root = reader::SourceRoot {
            path: "/never-read".into(),
            device: 0,
            inode: 0,
            mount: None,
            session: None,
        };
        assert!(
            readers
                .request(
                    99,
                    1,
                    &root.path.clone(),
                    Request::List {
                        output: None,
                        path: root.path.clone(),
                        root,
                        hidden: false,
                        exclusions: Vec::new(),
                    }
                )
                .unwrap()
                .is_none()
        );
        assert!(!readers.pending.contains_key(&99));
        release.send(()).unwrap();
        await_condition(&mut readers, Readers::is_empty);
        assert!(readers.inventory_available(99));
        let replacement = readers.reserve_inventory().unwrap();
        assert!(
            !readers.inventory_available(99),
            "delivered inventory still owns its slot"
        );
        drop(delivered);
        assert!(readers.inventory_available(99));
        drop(replacement);
    }

    #[test]
    fn held_root_leaves_other_work_runnable_and_late_replies_need_the_exact_request() {
        let mut readers = Readers::default();
        let (release, _) = held(&mut readers, 1, None, None);
        let path = Path::new("/owned/1");
        assert!(!readers.can_run(1, path));
        assert!(!readers.can_run(1, Path::new("/owned/1/other")));
        assert!(readers.can_run(2, Path::new("/owned/2")));
        release.send(()).unwrap();
        await_condition(&mut readers, |readers| readers.can_run(1, path));
        assert!(
            readers
                .request(1, 8, path, Request::Probe(path.into()))
                .is_err()
        );
        assert!(
            readers
                .request(1, 7, path, Request::Probe("/other".into()))
                .is_err()
        );
        assert!(matches!(
            readers
                .request(1, 7, path, Request::Probe(path.into()))
                .unwrap(),
            Some(Reply::Verified)
        ));
        assert!(readers.is_empty());
    }

    #[test]
    fn definite_no_install_replies_do_not_retire_the_instance_on_cancel_or_clear() {
        let mut readers = Readers::default();
        let instance = Arc::new(tempfile::tempfile().unwrap());
        for root in [1, 2] {
            let (release, _) = held(&mut readers, root, Some(instance.clone()), None);
            release.send(()).unwrap();
            await_condition(&mut readers, |readers| {
                readers.pending[&root].ready.is_some()
            });
            // The worker is genuinely finished; inject its definite protocol
            // outcome before the coordinator consumes or cancels that reply.
            readers.pending.get_mut(&root).unwrap().ready = Some(Ok(Reply::Watch(
                reader::WatchRegistration::Unavailable("Permission denied".into()),
            )));
        }
        assert!(readers.cancel_root(1).is_none());
        assert!(readers.clear().is_empty());
        assert!(readers.is_empty());
        assert_eq!(Arc::strong_count(&instance), 1);
    }

    #[test]
    fn a_new_enrollment_ticket_cannot_overlap_the_same_pending_or_retiring_path() {
        let mut readers = Readers::default();
        let (release, token) = held(&mut readers, -1, None, None);
        let path = Path::new("/owned/-1");
        assert!(!readers.can_run(-2, path));
        assert!(
            readers
                .request(-2, 0, path, Request::Probe(path.into()))
                .unwrap()
                .is_none()
        );
        assert_eq!(readers.pending.len(), 1);
        readers.cancel_root(-1);
        assert!(token.load(Ordering::Relaxed));
        assert!(readers.can_start(-2)); // Global capacity alone is insufficient.
        assert!(!readers.can_run(-2, path));
        assert!(readers.can_run(-2, Path::new("/owned/another")));
        release.send(()).unwrap();
        await_condition(&mut readers, |readers| readers.can_run(-2, path));
    }

    #[test]
    fn cancelled_watch_keeps_its_owned_instance_until_the_worker_finishes() {
        let mut readers = Readers::default();
        let instance = Arc::new(tempfile::tempfile().unwrap());
        let weak = Arc::downgrade(&instance);
        let (release, token) = held(&mut readers, 1, Some(instance.clone()), None);
        assert!(Arc::ptr_eq(&readers.watch_handle(1).unwrap(), &instance));
        let abandoned = readers.cancel_root(1).unwrap();
        assert!(Arc::ptr_eq(&abandoned, &instance));
        assert!(token.load(Ordering::Relaxed));
        drop(abandoned);
        drop(instance);
        assert!(
            weak.upgrade().is_some(),
            "The running job must retain the instance"
        );
        assert!(!readers.can_start(1));
        release.send(()).unwrap();
        await_condition(&mut readers, Readers::is_empty);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn cancellation_retains_admission_until_the_owned_worker_really_finishes() {
        let mut readers = Readers::default();
        let (first, token) = held(&mut readers, 1, None, None);
        let (second, _) = held(&mut readers, 2, None, None);
        assert!(!readers.can_start(3));
        readers.cancel_root(1);
        assert!(token.load(Ordering::Relaxed));
        readers.poll();
        assert_eq!(readers.retired.len(), 1);
        assert!(!readers.can_start(3));
        first.send(()).unwrap();
        await_condition(&mut readers, |readers| readers.can_start(3));
        readers.clear();
        assert_eq!(readers.retired.len(), 1);
        assert!(
            !readers.can_start(2),
            "A cancelled root remains busy until its worker exits"
        );
        assert!(
            readers.can_start(3),
            "Another root can use the remaining slot"
        );
        second.send(()).unwrap();
        await_condition(&mut readers, Readers::is_empty);
    }
}
