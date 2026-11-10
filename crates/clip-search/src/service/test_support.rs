//! Owned queued-service fixture for host and backend lifecycle contracts.
use super::*;

/// Creates an owned queued service and receiver without starting a worker.
pub fn queued_service() -> (Service, mpsc::Receiver<Command>) {
    let (commands, receiver) = mpsc::sync_channel(1);
    (
        Service {
            commands,
            dispatch: Default::default(),
            snapshot: Arc::new(Mutex::new(Snapshot::default())),
            stop: Arc::new(AtomicBool::new(false)),
            worker: None,
        },
        receiver,
    )
}
