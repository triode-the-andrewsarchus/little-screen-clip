//! Folder-owned visibility rules use a separate bounded read worker.
mod rules;
use super::filesystem::{Directory, Entry};
use little_clip_core::{BackgroundJobs, JobError, jobs::task::Task};
pub(super) use rules::Rules;
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Duration,
};
static IO: OnceLock<BackgroundJobs> = OnceLock::new();
#[derive(Clone, PartialEq, Eq)]
struct Request {
    path: PathBuf,
    object: Option<(u64, u64)>,
    mount: Option<u64>,
}
#[derive(Default)]
pub(super) struct Visibility {
    pub snapshot: Arc<Rules>,
    pub ignored: bool,
    pub error: Option<String>,
    request: Option<Request>,
    queued: bool,
    pending: Option<Task<Result<Rules, String>>>,
}
impl Rules {
    pub fn visible(&self, entry: &Entry, hidden: bool, ignored: bool) -> bool {
        if !ignored && let Some(effect) = self.effect(&entry.path, entry.directory) {
            return effect.show;
        }
        hidden || !entry.hidden
    }
}
impl Visibility {
    pub fn request(&mut self, directory: &Directory, ordinary: bool) {
        self.pending = None;
        self.error = None;
        if ordinary && !directory.system {
            let request = Request {
                path: directory.path.clone(),
                object: directory.object,
                mount: directory.mount,
            };
            if self.request.as_ref() != Some(&request) {
                self.snapshot = Arc::default();
            }
            self.request = Some(request);
            self.queued = true;
        } else {
            self.request = None;
            self.queued = false;
            self.snapshot = Arc::default();
        }
    }
    pub fn active(&self) -> bool {
        self.queued || self.pending.is_some()
    }
    pub fn reload(&mut self) {
        self.pending = None;
        self.queued = self.request.is_some();
        self.error = None;
    }
    pub fn tick(&mut self) -> bool {
        let mut changed = false;
        if let Some(result) = self.pending.as_ref().and_then(|task| task.poll()) {
            self.pending = None;
            match result {
                Err(JobError::QueueFull) => self.queued = true,
                Ok(Ok(snapshot)) => {
                    self.snapshot = Arc::new(snapshot);
                    self.error = None;
                    changed = true;
                }
                other => {
                    self.snapshot = Arc::default();
                    self.error = Some(match other {
                        Ok(Err(e)) => e,
                        Err(e) => e.to_string(),
                        _ => unreachable!(),
                    });
                    changed = true;
                }
            }
        }
        if self.queued
            && self.pending.is_none()
            && let Some(request) = self.request.clone()
        {
            self.queued = false;
            self.pending = Some(
                Task::start(
                    IO.get_or_init(|| BackgroundJobs::new(1, 2)),
                    Duration::from_secs(10),
                    move |cancel| Rules::load(&request.path, request.object, request.mount, cancel),
                )
                .with_response_deadline(Duration::from_secs(10)),
            );
        }
        changed
    }
}
impl super::FileManager {
    pub(super) fn tick_visibility(&mut self) {
        if self.visibility.tick() {
            self.rebuild_visible();
        }
    }
}
