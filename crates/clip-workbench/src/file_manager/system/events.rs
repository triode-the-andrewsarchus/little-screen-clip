//! One bounded udev observation stream shared by every semantic preview.
use std::{
    collections::VecDeque,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Default)]
struct State {
    revision: AtomicU64,
    available: AtomicBool,
    attempted: AtomicBool,
    stop: AtomicBool,
    changes: Mutex<VecDeque<(u64, PathBuf)>>,
}
static STATE: OnceLock<Arc<State>> = OnceLock::new();
fn state() -> &'static Arc<State> {
    STATE.get_or_init(|| {
        let state = Arc::new(State::default());
        let worker = state.clone();
        let _ = std::thread::Builder::new()
            .name("system-udev-events".into())
            .spawn(move || listen(worker));
        state
    })
}
fn listen(state: Arc<State>) {
    while !state.stop.load(Ordering::Acquire) {
        match udev::MonitorBuilder::new().and_then(|builder| builder.listen()) {
            Ok(socket) => {
                state.set_available(true);
                monitor(&state, &socket);
                state.set_available(false);
            }
            Err(_) => state.set_available(false),
        }
        // Retry only on this dedicated thread; shutdown remains responsive.
        for _ in 0..20 {
            if state.stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}
fn monitor(state: &State, socket: &udev::MonitorSocket) {
    while !state.stop.load(Ordering::Acquire) {
        let mut pollfd = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pollfd is initialized and writable for the single-descriptor poll.
        let result = unsafe { libc::poll(&mut pollfd, 1, 250) };
        if result < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
        if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return;
        }
        if pollfd.revents & libc::POLLIN == 0 {
            continue;
        }
        let mut incoming = socket.iter();
        if !state.drain(|| {
            receive(|| incoming.next())
                .map(|event| event.map(|event| event.syspath().to_path_buf()))
        }) {
            return;
        }
    }
}
/// Preserve libudev's errno across the crate's Option-only iterator boundary.
fn receive<T>(next: impl FnOnce() -> Option<T>) -> std::io::Result<Option<T>> {
    // SAFETY: errno is thread-local; next runs synchronously on this thread.
    unsafe {
        *libc::__errno_location() = 0;
    }
    if let Some(event) = next() {
        return Ok(Some(event));
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Ok(None)
    } else if error.raw_os_error() == Some(0) {
        Err(std::io::Error::other(
            "udev reception ended without a diagnostic",
        ))
    } else {
        Err(error)
    }
}

pub(super) fn revision() -> u64 {
    state().revision.load(Ordering::Acquire)
}
pub(super) fn available() -> bool {
    state().available.load(Ordering::Acquire)
}
pub(super) fn affected(since: u64, paths: impl IntoIterator<Item = impl AsRef<Path>>) -> bool {
    state().affected(since, paths)
}
impl State {
    fn set_available(&self, available: bool) {
        let attempted = self.attempted.swap(true, Ordering::AcqRel);
        if self.available.load(Ordering::Acquire) != available {
            // The listener is the sole writer. Publish the gap before its new
            // availability; an empty path invalidates every object's event paths.
            // Initial subscription has no prior coverage to invalidate.
            if !available || attempted || self.revision.load(Ordering::Acquire) != 0 {
                self.record(PathBuf::new());
            }
            self.available.store(available, Ordering::Release);
        }
    }

    fn drain(&self, mut next: impl FnMut() -> std::io::Result<Option<PathBuf>>) -> bool {
        for _ in 0..128 {
            match next() {
                Ok(Some(path)) => self.record(path),
                Ok(None) => return true,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.set_available(false);
                    return false;
                }
            }
        }
        true
    }

    fn record(&self, path: PathBuf) {
        let mut changes = self.changes.lock().unwrap_or_else(|e| e.into_inner());
        let revision = self.revision.load(Ordering::Relaxed).saturating_add(1);
        changes.push_back((revision, path));
        while changes.len() > 1024 {
            changes.pop_front();
        }
        // Publish the epoch only after its invalidation has been recorded.
        self.revision.store(revision, Ordering::Release);
    }
    fn affected(&self, since: u64, paths: impl IntoIterator<Item = impl AsRef<Path>>) -> bool {
        let changes = self.changes.lock().unwrap_or_else(|e| e.into_inner());
        if changes
            .front()
            .is_some_and(|(revision, _)| since.saturating_add(1) < *revision)
        {
            return true;
        }
        let paths = paths
            .into_iter()
            .map(|p| p.as_ref().to_path_buf())
            .collect::<Vec<_>>();
        changes
            .iter()
            .filter(|(revision, _)| *revision > since)
            .any(|(_, changed)| {
                paths
                    .iter()
                    .any(|path| path.starts_with(changed) || changed.starts_with(path))
            })
    }
}
pub(super) fn shutdown() {
    if let Some(state) = STATE.get() {
        state.stop.store(true, Ordering::Release);
    }
    super::worker::shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_preserves_real_errors_and_ignores_stale_errno_on_success() {
        for errno in [libc::ENOBUFS, libc::EIO, libc::EINTR, 0] {
            let result = receive::<()>(|| {
                // SAFETY: this changes only the test thread's errno fixture.
                unsafe {
                    *libc::__errno_location() = errno;
                }
                None
            });
            assert!(result.is_err(), "errno {errno} cannot prove an empty queue");
            if errno != 0 {
                assert_eq!(result.unwrap_err().raw_os_error(), Some(errno));
            }
        }
        assert!(
            receive::<()>(|| {
                // SAFETY: errno is local to this test thread.
                unsafe {
                    *libc::__errno_location() = libc::EAGAIN;
                }
                None
            })
            .unwrap()
            .is_none()
        );
        assert_eq!(
            receive(|| {
                // SAFETY: a successful event takes precedence over thread-local errno.
                unsafe {
                    *libc::__errno_location() = libc::ENOBUFS;
                }
                Some(42)
            })
            .unwrap(),
            Some(42)
        );
    }

    #[test]
    fn overflow_and_fatal_receive_errors_invalidate_prior_and_pending_observations() {
        for errno in [libc::ENOBUFS, libc::EIO, libc::EBADF] {
            let state = State::default();
            state.set_available(true);
            state.record("/sys/devices/observed".into());
            let admitted = state.revision.load(Ordering::Acquire);
            assert!(!state.drain(|| Err(std::io::Error::from_raw_os_error(errno))));
            assert!(!state.available.load(Ordering::Acquire));
            assert!(state.affected(admitted, ["/sys/devices/unrelated"]));
            assert!(state.affected(admitted, ["/dev/alias"]));
            let lost = state.revision.load(Ordering::Acquire);
            state.set_available(true);
            assert!(state.affected(lost, ["/sys/devices/unrelated"]));
        }
    }

    #[test]
    fn first_success_after_subscription_failure_invalidates_offline_observations() {
        let state = State::default();
        state.set_available(false);
        state.set_available(false);
        assert_eq!(
            state.revision.load(Ordering::Acquire),
            0,
            "initial failures do not churn startup epochs"
        );
        state.set_available(true);
        assert!(state.affected(0, ["/sys/devices/offline-observation"]));
        let subscribed = state.revision.load(Ordering::Acquire);
        state.set_available(true);
        assert_eq!(state.revision.load(Ordering::Acquire), subscribed);
    }

    #[test]
    fn event_bursts_are_bounded_and_interruption_or_empty_queues_preserve_coverage() {
        let state = State::default();
        state.set_available(true);
        let mut calls = 0;
        assert!(state.drain(|| {
            calls += 1;
            Ok(Some("/sys/devices/burst".into()))
        }));
        assert_eq!(calls, 128);
        assert_eq!(state.revision.load(Ordering::Acquire), 128);
        let mut first = true;
        assert!(state.drain(|| {
            if std::mem::take(&mut first) {
                Err(std::io::Error::from_raw_os_error(libc::EINTR))
            } else {
                Ok(None)
            }
        }));
        assert_eq!(state.revision.load(Ordering::Acquire), 128);
        assert!(state.available.load(Ordering::Acquire));
    }

    #[test]
    fn secondary_fact_sources_and_unresolved_sysfs_aliases_invalidate_observations() {
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = "/sys/devices/primary".into();
        document.relations.clear();
        document.facts.clear();
        document.facts.push(super::super::model::Fact::text(
            "Secondary metadata",
            "fixture",
            "/sys/devices/secondary/dev".into(),
        ));
        let state = State::default();
        state.record("/sys/devices/unrelated".into());
        assert!(!state.affected(0, document.event_paths()));
        state.record("/sys/devices/secondary".into());
        assert!(state.affected(0, document.event_paths()));
        assert!(!state.affected(2, document.event_paths()));
        document.relations.push(super::super::Relation::new(
            "Related interface",
            "/sys/devices/peer".into(),
        ));
        state.record("/sys/devices/peer".into());
        assert!(state.affected(2, document.event_paths()));
        document.relations.clear();
        assert!(!state.affected(2, document.event_paths()));
        document.facts[0].source = "/sys/class/vc/vcs63/dev".into();
        assert!(state.affected(0, document.event_paths()));
        document.facts.clear();
        document.kind = little_clip_core::kernel_interface::Kind::Kernel {
            filesystem: little_clip_core::kernel_interface::Filesystem::Sysfs,
        };
        document.path = "/fixture/bind-mounted-sysfs".into();
        assert!(state.affected(0, document.event_paths()));
        document.kind = little_clip_core::kernel_interface::Kind::Character { major: 7, minor: 1 };
        document.path = "/dev/vcs1".into();
        document.kernel_identity = Some(vec![1, 2, 3]);
        document.relations.push(super::super::Relation::new(
            "Kernel object",
            "/fixture/bind-mounted-sysfs".into(),
        ));
        assert!(state.affected(0, document.event_paths()));
    }

    #[test]
    fn invalidation_targets_related_objects_and_ignores_old_or_unrelated_events() {
        let state = State::default();
        assert!(!state.available.load(Ordering::Acquire));
        assert!(!state.affected(0, ["/sys/devices/fixture"]));
        state.record("/sys/devices/fixture".into());
        assert_eq!(state.revision.load(Ordering::Acquire), 1);
        assert!(state.affected(0, ["/sys/devices/fixture/temperature"]));
        assert!(state.affected(0, ["/sys/devices"]));
        assert!(!state.affected(1, ["/sys/devices/fixture"]));
        assert!(!state.affected(0, ["/sys/devices/other"]));
    }

    #[test]
    fn subscription_gaps_invalidate_all_prior_objects_and_late_results() {
        let state = State::default();
        state.set_available(true);
        let subscribed = state.revision.load(Ordering::Acquire);
        assert_eq!(
            subscribed, 0,
            "initial subscription must not reject fresh startup observations"
        );
        assert!(!state.affected(subscribed, ["/sys/devices/unchanged"]));
        state.set_available(false);
        let lost = state.revision.load(Ordering::Acquire);
        assert!(lost > subscribed);
        assert!(state.affected(subscribed, ["/sys/devices/unchanged"]));
        assert!(state.affected(subscribed, ["/dev/alias"]));
        assert!(!state.available.load(Ordering::Acquire));
        state.set_available(false);
        assert_eq!(state.revision.load(Ordering::Acquire), lost);
        state.set_available(true);
        let resumed = state.revision.load(Ordering::Acquire);
        assert!(resumed > lost);
        assert!(state.affected(lost, ["/sys/devices/unchanged"]));
        assert!(!state.affected(resumed, ["/sys/devices/unchanged"]));
    }

    #[test]
    fn lost_event_history_invalidates_conservatively_and_stays_bounded() {
        let state = State::default();
        for index in 0..1025 {
            state.record(format!("/sys/devices/fixture{index}").into());
        }
        assert_eq!(state.changes.lock().unwrap().len(), 1024);
        assert!(state.affected(0, ["/sys/devices/unrelated"]));
        assert!(!state.affected(1025, ["/sys/devices/unrelated"]));
    }
}
