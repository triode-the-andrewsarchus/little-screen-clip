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
    /// Prevents a queued or completed job from publishing its result.
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
