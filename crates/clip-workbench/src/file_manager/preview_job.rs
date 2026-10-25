//! Cancellable bounded jobs shared by file preview renderers.
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub(super) struct Task<T> {
    ticket: JobTicket,
    pub(super) token: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<T, JobError>>,
    response_deadline: Option<Instant>,
}
impl<T: Send + 'static> Task<T> {
    pub fn start(
        pool: &BackgroundJobs,
        timeout: Duration,
        work: impl FnOnce(&AtomicBool) -> T + Send + 'static,
    ) -> Self {
        let token = Arc::new(AtomicBool::new(false));
        let worker = token.clone();
        let (ticket, receiver) = pool.submit(timeout, move || work(&worker));
        Self {
            ticket,
            token,
            receiver,
            response_deadline: None,
        }
    }
    /// Starts work without a time limit; dropping the task still requests cancellation.
    pub fn start_unbounded(
        pool: &BackgroundJobs,
        work: impl FnOnce(&AtomicBool) -> T + Send + 'static,
    ) -> Self {
        Self::start(pool, Duration::MAX, work)
    }
    pub fn poll(&self) -> Option<Result<T, JobError>> {
        let result = poll(&self.receiver, self.response_deadline);
        if matches!(result, Some(Err(JobError::TimedOut))) {
            self.token.store(true, Ordering::Relaxed);
            self.ticket.cancel();
        }
        result
    }
    pub fn with_response_deadline(mut self, timeout: Duration) -> Self {
        // An unrepresentably distant deadline cannot expire within this process lifetime.
        self.response_deadline = Instant::now().checked_add(timeout);
        self
    }
}
pub(super) fn poll<T>(
    receiver: &mpsc::Receiver<Result<T, JobError>>,
    deadline: Option<Instant>,
) -> Option<Result<T, JobError>> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Some(Err(JobError::TimedOut));
    }
    match receiver.try_recv() {
        Ok(result) => Some(result),
        Err(mpsc::TryRecvError::Empty) => None,
        Err(_) => Some(Err(JobError::Cancelled)),
    }
}
impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.token.store(true, Ordering::Relaxed);
        self.ticket.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_deadline_cancels_waiting_without_waiting_for_blocked_work() {
        let pool = BackgroundJobs::new(1, 2);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let task = Task::start(&pool, Duration::from_secs(30), move |_| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            finished_tx.send(()).unwrap();
            42
        })
        .with_response_deadline(Duration::ZERO);
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let started = Instant::now();
        assert!(matches!(task.poll(), Some(Err(JobError::TimedOut))));
        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(task.token.load(Ordering::Relaxed));
        assert!(matches!(
            finished_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(
            matches!(task.poll(), Some(Err(JobError::TimedOut))),
            "late completion must remain rejected"
        );
    }

    #[test]
    fn polling_without_response_deadline_preserves_existing_completion_behavior() {
        let (sender, receiver) = mpsc::channel();
        assert!(poll(&receiver, None).is_none());
        sender.send(Ok(42)).unwrap();
        assert!(matches!(poll(&receiver, None), Some(Ok(42))));
        sender.send(Ok(99)).unwrap();
        assert!(matches!(
            poll(&receiver, Some(Instant::now())),
            Some(Err(JobError::TimedOut))
        ));
    }

    #[test]
    fn overflowing_response_deadline_preserves_completion() {
        let pool = BackgroundJobs::new(1, 2);
        let task = Task::start(&pool, Duration::MAX, |_| 42).with_response_deadline(Duration::MAX);
        assert!(task.response_deadline.is_none());
        assert_eq!(
            task.receiver
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .unwrap(),
            42
        );
    }

    #[test]
    fn unbounded_task_has_no_response_deadline_and_completes() {
        let pool = BackgroundJobs::new(1, 2);
        let task = Task::start_unbounded(&pool, |_| 42);
        assert!(task.response_deadline.is_none());
        let started = Instant::now();
        loop {
            if let Some(result) = task.poll() {
                assert_eq!(result.unwrap(), 42);
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::yield_now();
        }
    }
}
