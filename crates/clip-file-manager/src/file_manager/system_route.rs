//! Mutation admission for kernel interfaces; content readers use core's pinned policy.
use super::JobError;
use little_clip_core::kernel_interface::require_content;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use std::{sync::mpsc, time::Duration};

pub(super) fn tree(path: &Path, cancelled: &AtomicBool) -> Result<(), String> {
    let mut stack = vec![path.to_path_buf()];
    let mut visited = BTreeSet::new();
    let mut count = 0;
    while let Some(path) = stack.pop() {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        count += 1;
        if count > 200_000 {
            return Err("Safety preflight exceeds 200,000 entries; choose a smaller batch".into());
        }
        match require_content(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if metadata.is_dir() {
            use std::os::unix::fs::MetadataExt;
            if !visited.insert((metadata.dev(), metadata.ino())) {
                return Err("Repeated directory in safety preflight".into());
            }
            for child in std::fs::read_dir(&path).map_err(|e| e.to_string())? {
                stack.push(child.map_err(|e| e.to_string())?.path());
            }
        }
    }
    Ok(())
}
pub(super) fn batch(
    sources: &[PathBuf],
    destination: Option<&Path>,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    if let Some(destination) = destination {
        require_content(destination).map_err(|e| e.to_string())?;
    }
    for source in sources {
        tree(source, cancelled)?;
    }
    Ok(())
}

type AdmissionResult = mpsc::Receiver<Result<Result<(), String>, JobError>>;

#[derive(Default)]
pub(super) enum Admission {
    #[default]
    Unchecked,
    Checking(AdmissionResult),
    Accepted,
}

impl Admission {
    /// Admit mutations only after an asynchronous read-only check succeeds.
    pub(super) fn poll(
        &mut self,
        jobs: &super::BackgroundJobs,
        check: impl FnOnce() -> Result<(), String> + Send + 'static,
    ) -> Result<bool, String> {
        if matches!(*self, Admission::Unchecked) {
            let (_, receiver) = jobs.submit(Duration::from_secs(30), check);
            *self = Admission::Checking(receiver);
        }
        let Admission::Checking(receiver) = &*self else {
            return Ok(true);
        };
        match receiver.try_recv() {
            Ok(Ok(result)) => {
                result?;
                *self = Admission::Accepted;
                Ok(true)
            }
            Ok(Err(JobError::QueueFull)) => {
                *self = Admission::Unchecked;
                Ok(false)
            }
            Ok(Err(error)) => Err(format!("Safety check unavailable: {error}")),
            Err(mpsc::TryRecvError::Empty) => Ok(false),
            Err(mpsc::TryRecvError::Disconnected) => Err("Safety worker disconnected".into()),
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::BackgroundJobs;
    use super::*;
    use std::sync::{Arc, atomic::AtomicUsize};

    /// A worker held behind a channel, so saturation tests need no timing guess.
    pub(in crate::file_manager) struct BlockedJobs {
        pub jobs: BackgroundJobs,
        release: Option<mpsc::Sender<()>>,
        done: mpsc::Receiver<Result<(), JobError>>,
    }

    impl BlockedJobs {
        pub fn new() -> Self {
            let jobs = BackgroundJobs::new(1, 1);
            let (started_tx, started_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let (_, done) = jobs.submit(Duration::MAX, move || {
                started_tx.send(()).unwrap();
                let _ = release_rx.recv();
            });
            started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            Self {
                jobs,
                release: Some(release_tx),
                done,
            }
        }

        pub fn release(&mut self) {
            self.release.take().unwrap().send(()).unwrap();
            self.done
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap();
        }
    }

    impl Drop for BlockedJobs {
        fn drop(&mut self) {
            if let Some(release) = self.release.take() {
                let _ = release.send(());
            }
        }
    }

    #[test]
    fn saturated_admission_retries_only_the_rejected_check_and_accepts_once() {
        let mut pool = BlockedJobs::new();
        let (_, queued) = pool.jobs.submit(Duration::MAX, || ());
        let calls = Arc::new(AtomicUsize::new(0));
        let mut admission = Admission::default();
        let count = calls.clone();
        assert!(
            !admission
                .poll(&pool.jobs, move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .unwrap()
        );
        assert!(matches!(admission, Admission::Unchecked));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        pool.release();
        queued
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let count = calls.clone();
        admission
            .poll(&pool.jobs, move || {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !admission
            .poll(&pool.jobs, || panic!("a pending check must not restart"))
            .unwrap()
        {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            admission
                .poll(&pool.jobs, || panic!("an accepted check must not restart"))
                .unwrap()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_or_missing_admission_completions_never_accept_mutation() {
        let jobs = BackgroundJobs::new(1, 1);
        for result in [
            Some(Ok(Err("protected interface".into()))),
            Some(Err(JobError::TimedOut)),
            Some(Err(JobError::Cancelled)),
            None,
        ] {
            let (sender, receiver) = mpsc::channel();
            if let Some(result) = result {
                sender.send(result).unwrap();
            }
            drop(sender);
            let mut admission = Admission::Checking(receiver);
            assert!(
                admission
                    .poll(&jobs, || panic!("failure must not resubmit"))
                    .is_err()
            );
            assert!(!matches!(admission, Admission::Accepted));
        }
    }
}
