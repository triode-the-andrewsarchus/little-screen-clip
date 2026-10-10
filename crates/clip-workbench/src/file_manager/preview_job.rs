//! Cancellable bounded jobs shared by file preview renderers.
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub(super) struct Task<T> {
    ticket: JobTicket,
    pub(super) token: Arc<AtomicBool>,
    receiver: mpsc::Receiver<Result<T, JobError>>,
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
        }
    }
    pub fn poll(&self) -> Option<Result<T, JobError>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(_) => Some(Err(JobError::Cancelled)),
        }
    }
}
impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.token.store(true, Ordering::Relaxed);
        self.ticket.cancel();
    }
}
