//! Home-local folder filters and bounded, pinned directory snapshots.
use super::Pin;
use crate::file_manager::{
    filesystem::{Directory, Entry},
    visibility,
};
use little_clip_core::{BackgroundJobs, JobError, jobs::task::Task, kernel_interface};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, LazyLock, atomic::AtomicBool},
    time::{Duration, Instant, SystemTime},
};
use uuid::Uuid;

static READS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 2));
const LIMIT: usize = 4096;
const CAPACITY: usize = 16;
const FRESH: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::file_manager) enum Presentation {
    #[default]
    List,
    Icons,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager) struct Options {
    pub presentation: Presentation,
    pub name: String,
    /// Comma/space-separated extensions, without changing folder-owned rules.
    pub extensions: String,
    /// Zero disables the relative age filter; unknown/future timestamps do not match.
    pub recent_minutes: u32,
    pub show_hidden: bool,
}
impl Options {
    pub fn valid(&self) -> bool {
        self.name.len() <= 256 && self.extensions.len() <= 256 && self.recent_minutes <= 5_256_000
    }
    pub fn matches(&self, entry: &Entry, now: SystemTime) -> bool {
        if !self.name.is_empty()
            && !entry
                .name
                .to_lowercase()
                .contains(&self.name.to_lowercase())
        {
            return false;
        }
        if !entry.directory && !self.extensions.trim().is_empty() {
            let extension = entry
                .path
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or("");
            if !self
                .extensions
                .split([',', ' '])
                .filter(|v| !v.is_empty())
                .any(|v| extension.eq_ignore_ascii_case(v.trim_start_matches('.')))
            {
                return false;
            }
        }
        self.recent_minutes == 0
            || entry
                .modified_time
                .and_then(|time| now.duration_since(time).ok())
                .is_some_and(|age| age < Duration::from_secs(u64::from(self.recent_minutes) * 60))
    }
}

pub(in crate::file_manager) struct Snapshot {
    pub directory: Directory,
    pub visibility: visibility::Rules,
}
fn read(path: &std::path::Path, cancel: &AtomicBool) -> Result<Arc<Snapshot>, String> {
    let held = kernel_interface::pin_content_directory(path).map_err(|e| e.to_string())?;
    let before =
        crate::file_manager::filesystem::identity(&held.metadata().map_err(|e| e.to_string())?);
    let mount = kernel_interface::MountIdentity::pinned(&held).map_err(|e| e.to_string())?;
    let directory =
        little_clip_core::file_inventory::read_pinned_directory(path, &held, cancel, LIMIT)?;
    let visibility = visibility::Rules::load(path, directory.object, directory.mount, cancel)?;
    let current = kernel_interface::pin_content_directory(path).map_err(|e| e.to_string())?;
    if crate::file_manager::filesystem::identity(&current.metadata().map_err(|e| e.to_string())?)
        != before
        || kernel_interface::MountIdentity::pinned(&current).map_err(|e| e.to_string())? != mount
    {
        return Err("Folder changed while its Home preview was read".into());
    }
    Ok(Arc::new(Snapshot {
        directory,
        visibility,
    }))
}
#[derive(Default)]
pub(in crate::file_manager) struct View {
    path: PathBuf,
    pub snapshot: Option<Arc<Snapshot>>,
    pub error: Option<String>,
    pending: Option<Task<Result<Arc<Snapshot>, String>>>,
    attempted: Option<Instant>,
    seen: u64,
}
#[derive(Default)]
pub(in crate::file_manager) struct Previews {
    entries: BTreeMap<Uuid, View>,
    generation: u64,
}
impl Previews {
    pub fn begin(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn end(&mut self) {
        self.entries.retain(|_, view| view.seen == self.generation);
    }
    pub fn retire(&mut self) {
        self.entries.clear();
    }
    pub fn refresh(&mut self, pin: Uuid) {
        if let Some(view) = self.entries.get_mut(&pin) {
            view.attempted = None;
        }
    }
    pub fn request(&mut self, pin: &Pin) -> Option<&View> {
        if !self.entries.contains_key(&pin.id) && self.entries.len() >= CAPACITY {
            let old = self
                .entries
                .iter()
                .filter(|(_, view)| view.seen != self.generation)
                .min_by_key(|(_, view)| view.seen)
                .map(|(id, _)| *id)?;
            self.entries.remove(&old);
        }
        let reads = self
            .entries
            .values()
            .filter(|view| view.pending.is_some())
            .count();
        let view = self.entries.entry(pin.id).or_default();
        if view.path != pin.path {
            *view = View {
                path: pin.path.clone(),
                ..Default::default()
            };
        }
        view.seen = self.generation;
        if let Some(result) = view.pending.as_ref().and_then(Task::poll) {
            view.pending = None;
            view.attempted = Some(Instant::now());
            match result {
                Ok(Ok(snapshot)) => {
                    view.snapshot = Some(snapshot);
                    view.error = None;
                }
                Err(JobError::QueueFull) => view.attempted = None,
                Ok(Err(error)) => {
                    view.snapshot = None;
                    view.error = Some(error);
                }
                Err(error) => {
                    view.snapshot = None;
                    view.error = Some(error.to_string());
                }
            }
        }
        if view.pending.is_none()
            && reads < 2
            && view.attempted.is_none_or(|at| at.elapsed() >= FRESH)
        {
            let path = view.path.clone();
            view.attempted = Some(Instant::now());
            view.pending = Some(Task::start(
                &READS,
                Duration::from_secs(15),
                move |cancel| read(&path, cancel),
            ));
        }
        Some(view)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pinned_folder_snapshot_applies_existing_visibility_rules_without_changing_them() {
        let root = tempfile::tempdir().unwrap();
        let rules = "version=1\npatterns=['*.secret']\n";
        std::fs::write(root.path().join(".hide"), rules).unwrap();
        std::fs::write(root.path().join("visible.txt"), "visible").unwrap();
        std::fs::write(root.path().join("hidden.secret"), "hidden").unwrap();
        let snapshot = read(root.path(), &AtomicBool::new(false)).unwrap();
        let visible: Vec<_> = snapshot
            .directory
            .entries
            .iter()
            .filter(|entry| snapshot.visibility.visible(entry, false, false))
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(visible, ["visible.txt"]);
        assert_eq!(
            std::fs::read_to_string(root.path().join(".hide")).unwrap(),
            rules
        );
    }
    #[test]
    fn home_filters_expire_by_time_without_modifying_the_inventory_or_folder_rules() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Photo.JPG");
        std::fs::write(&path, "fixture").unwrap();
        let directory =
            little_clip_core::file_inventory::read_directory(root.path(), &AtomicBool::new(false))
                .unwrap();
        let entry = &directory.entries[0];
        let modified = entry.modified_time.unwrap();
        let mut filter = Options {
            name: "photo".into(),
            extensions: ".png, jpg".into(),
            recent_minutes: 60,
            ..Default::default()
        };
        assert!(filter.matches(entry, modified + Duration::from_secs(3599)));
        assert!(!filter.matches(entry, modified + Duration::from_secs(3600)));
        assert!(!filter.matches(entry, modified - Duration::from_secs(1)));
        filter.recent_minutes = 0;
        assert!(filter.matches(entry, modified + Duration::from_secs(7200)));
        filter.extensions = "wav".into();
        assert!(!filter.matches(entry, modified));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "fixture");
        assert!(!root.path().join(".hide").exists());
    }
}
