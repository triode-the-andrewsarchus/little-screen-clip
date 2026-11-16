//! Daemon-owned portable media synchronization, backup and offline catalog services.
mod backup;
mod catalog;
mod discovery;
mod mounts;
mod ram;
mod root;
mod scan;
mod service;
mod store;
mod sync;
mod watch;

pub use catalog::mount_catalog;
pub use discovery::discover;
pub use little_clip_core::drives::*;
pub use service::Service;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Sql(#[from] rusqlite::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Message(String),
}
impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}
impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Message(value.into())
    }
}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub(crate) fn cancelled(token: &std::sync::atomic::AtomicBool) -> Result<()> {
    if token.load(std::sync::atomic::Ordering::Relaxed) {
        Err("Cancelled; completed writes have been retained".into())
    } else {
        Ok(())
    }
}

impl From<rustix::io::Errno> for Error {
    fn from(value: rustix::io::Errno) -> Self {
        Self::Io(value.into())
    }
}

#[cfg(test)]
mod sync_edge_tests;
