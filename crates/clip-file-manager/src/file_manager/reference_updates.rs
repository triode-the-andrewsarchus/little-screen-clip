//! Deferred metadata work after an already completed filesystem relocation.
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug)]
pub(super) struct Relocation {
    pub source: PathBuf,
    pub destination: PathBuf,
}
#[derive(Default)]
pub(super) struct Deferred {
    queue: VecDeque<Relocation>,
    failed: Option<Vec<Relocation>>,
    pub error: Option<String>,
}
impl Deferred {
    pub fn push(&mut self, source: &Path, destination: &Path) {
        if source != destination {
            self.queue.push_back(Relocation {
                source: source.into(),
                destination: destination.into(),
            });
        }
    }
    pub fn active(&self) -> bool {
        self.failed.is_none() && !self.queue.is_empty()
    }
    pub fn writing(&self) -> bool {
        self.failed.is_some() || !self.queue.is_empty()
    }
    pub fn failed(&self) -> bool {
        self.failed.is_some()
    }
    pub fn next(&mut self) -> Option<Vec<Relocation>> {
        if self.failed.is_none() {
            (!self.queue.is_empty())
                .then(|| self.queue.drain(..self.queue.len().min(128)).collect())
        } else {
            None
        }
    }
    pub fn reject(&mut self, change: Vec<Relocation>, error: String) {
        self.failed = Some(change);
        self.error = Some(error);
    }
    pub fn retry(&mut self) {
        if let Some(change) = self.failed.take() {
            for change in change.into_iter().rev() {
                self.queue.push_front(change);
            }
        }
        self.error = None;
    }
    pub fn skip(&mut self) {
        self.queue.clear();
        self.failed = None;
        self.error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_batches_preserve_order_through_explicit_retry_and_skip() {
        let mut changes = Deferred::default();
        for n in 0..300 {
            changes.push(
                Path::new(&format!("/source/{n}")),
                Path::new(&format!("/destination/{n}")),
            );
        }
        let first = changes.next().unwrap();
        assert_eq!(first.len(), 128);
        assert_eq!(first[0].source, Path::new("/source/0"));
        changes.reject(first, "locked".into());
        assert!(changes.writing());
        assert!(!changes.active());
        assert!(changes.next().is_none());
        changes.retry();
        let first = changes.next().unwrap();
        assert_eq!(first[127].source, Path::new("/source/127"));
        assert_eq!(changes.next().unwrap()[0].source, Path::new("/source/128"));
        let last = changes.next().unwrap();
        assert_eq!(last.len(), 44);
        changes.reject(last, "locked again".into());
        changes.skip();
        assert!(!changes.writing());
        assert!(changes.error.is_none());
    }
}
