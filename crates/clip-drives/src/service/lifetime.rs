//! Closing admission and draining writes are separate from dropping a client handle.
use super::*;
pub(super) struct Runtime {
    pub closing: Arc<AtomicBool>,
    pub tokens: Tokens,
    pub thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}
impl Runtime {
    pub fn stop(&self) {
        // Admission uses this same lock, so every accepted envelope precedes close.
        let tokens = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        self.closing.store(true, Ordering::Release);
        for cancel in tokens.values() {
            cancel.store(true, Ordering::Release);
        }
    }
    pub fn join(&self) -> Result<()> {
        let mut handle = self.thread.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(thread) = handle.take() {
            thread
                .join()
                .map_err(|_| "Storage coordinator panicked; retained data needs recovery")?;
        }
        Ok(())
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        // Destructors only signal; hosts explicitly join from a blocking context.
        self.stop();
    }
}

impl Service {
    /// Closes admission, requests cooperative cancellation and waits for actual exit.
    ///
    /// Call from a blocking context before the host exits. Completed writes and
    /// operation results are retained; RAM servers flush before their owner retires.
    /// Multiple calls may safely wait for the same shutdown.
    ///
    /// # Errors
    /// Returns a coordinator panic; retained storage is reconciled at the next start.
    pub fn shutdown(&self) -> Result<()> {
        self.runtime.stop();
        self.runtime.join()
    }
}
