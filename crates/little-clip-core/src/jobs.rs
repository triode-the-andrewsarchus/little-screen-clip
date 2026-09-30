//! Bounded background work with revision-aware cancellation and timeouts.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use std::time::{Duration, Instant};

const DEFAULT_WORKERS: usize = 4;
const DEFAULT_QUEUE: usize = 16;

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Reports why a background job did not produce a value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    /// The caller cancelled this revision before it completed.
    #[error("job was cancelled")]
    Cancelled,
    /// The operation exceeded its caller-provided time limit.
    #[error("job timed out")]
    TimedOut,
    /// The shared queue is saturated.
    #[error("background job queue is full")]
    QueueFull,
}

/// A cancellable revision identifier for work that may become stale.
#[derive(Debug, Clone)]
pub struct JobTicket {
    revision: u64,
    cancelled: Arc<AtomicBool>,
}

impl JobTicket {
    /// Returns this ticket's monotonic revision number.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Requests cancellation before a queued or running job publishes its result.
    ///
    /// Cancellation does not interrupt an operation that is already running, and
    /// it cannot revoke a result that has already been sent to the receiver.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// Bounded executor shared by daemon and workbench background operations.
pub struct BackgroundJobs {
    sender: mpsc::SyncSender<Job>,
    next_revision: AtomicU64,
}

impl BackgroundJobs {
    /// Returns the application-wide bounded executor.
    pub fn global() -> &'static Self {
        static JOBS: LazyLock<BackgroundJobs> =
            LazyLock::new(|| BackgroundJobs::new(DEFAULT_WORKERS, DEFAULT_QUEUE));
        &JOBS
    }

    /// Creates a bounded executor with a fixed worker count and queue length.
    pub fn new(workers: usize, queue: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Job>(queue);
        let receiver = Arc::new(Mutex::new(receiver));
        for _ in 0..workers.max(1) {
            let receiver = Arc::clone(&receiver);
            std::thread::spawn(move || {
                loop {
                    let job = match receiver.lock().expect("job queue mutex poisoned").recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    };
                    job();
                }
            });
        }
        Self {
            sender,
            next_revision: AtomicU64::new(1),
        }
    }

    /// Queues work and returns a cancellation ticket plus a single completion receiver.
    ///
    /// Admission is nonblocking: a full queue produces [`JobError::QueueFull`].
    /// Cancellation is checked before starting the operation and after it returns;
    /// operations that need prompt interruption must also check their own token.
    /// The timeout is checked after the operation returns and does not interrupt it.
    pub fn submit<T, F>(
        &self,
        timeout: Duration,
        operation: F,
    ) -> (JobTicket, mpsc::Receiver<Result<T, JobError>>)
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let ticket = JobTicket {
            revision: self.next_revision.fetch_add(1, Ordering::Relaxed),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let worker_sender = result_sender.clone();
        let cancelled = Arc::clone(&ticket.cancelled);
        let job = Box::new(move || {
            if cancelled.load(Ordering::Acquire) {
                let _ = worker_sender.send(Err(JobError::Cancelled));
                return;
            }
            let started = Instant::now();
            let value = operation();
            let result = if cancelled.load(Ordering::Acquire) {
                Err(JobError::Cancelled)
            } else if started.elapsed() > timeout {
                Err(JobError::TimedOut)
            } else {
                Ok(value)
            };
            let _ = worker_sender.send(result);
        });
        if self.sender.try_send(job).is_err() {
            let _ = result_sender.send(Err(JobError::QueueFull));
        }
        (ticket, result_receiver)
    }
}

#[cfg(test)]
mod tests {
    use super::{BackgroundJobs, JobError};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    #[test]
    fn bounded_queue_rejects_excess_work_and_skips_cancelled_queued_work() {
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (running_ticket, running_result) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            1
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let queued_runs = Arc::new(AtomicUsize::new(0));
        let queued_counter = Arc::clone(&queued_runs);
        let (queued_ticket, queued_result) = jobs.submit(Duration::MAX, move || {
            queued_counter.fetch_add(1, Ordering::SeqCst);
            2
        });
        queued_ticket.cancel();

        let (_, rejected_result) = jobs.submit(Duration::MAX, || 3);
        assert_eq!(
            rejected_result
                .recv_timeout(Duration::from_secs(5))
                .unwrap(),
            Err(JobError::QueueFull)
        );

        release_tx.send(()).unwrap();
        assert_eq!(
            running_result.recv_timeout(Duration::from_secs(5)).unwrap(),
            Ok(1)
        );
        assert_eq!(
            queued_result.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(JobError::Cancelled)
        );
        assert_eq!(queued_runs.load(Ordering::SeqCst), 0);
        drop(running_ticket);
    }

    #[test]
    fn cancellation_while_running_suppresses_result_publication() {
        let jobs = BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (ticket, result) = jobs.submit(Duration::MAX, move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            "finished after cancellation"
        });

        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        ticket.cancel();
        release_tx.send(()).unwrap();

        assert_eq!(
            result.recv_timeout(Duration::from_secs(5)).unwrap(),
            Err(JobError::Cancelled)
        );
    }
}
