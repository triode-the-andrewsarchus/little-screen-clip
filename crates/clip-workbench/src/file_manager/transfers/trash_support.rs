use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use super::super::filesystem::rename_noreplace;
use super::super::{BackgroundJobs, JobError};

type ListingResult = mpsc::Receiver<Result<Result<Vec<trash::TrashItem>, String>, JobError>>;

#[derive(Default)]
pub(super) struct TrashBrowser {
    pub open: bool,
    pub items: Vec<trash::TrashItem>,
    pub selected: BTreeSet<OsString>,
    pub error: Option<String>,
    pub pending: Option<ListingResult>,
    reload: bool,
}

impl TrashBrowser {
    pub fn show(&mut self) {
        self.open = true;
        self.invalidate();
    }
    pub fn invalidate(&mut self) {
        self.reload = true;
    }
    pub fn tick(&mut self, jobs: &BackgroundJobs) {
        if !self.open {
            return;
        }
        if self.reload && self.pending.is_none() {
            self.reload = false;
            let (_, receiver) = jobs.submit(Duration::from_secs(30), || {
                let mut items = trash::os_limited::list().map_err(|e| e.to_string())?;
                items.sort_by_key(|item| std::cmp::Reverse(item.time_deleted));
                Ok(items)
            });
            self.pending = Some(receiver);
        }
        let Some(receiver) = &self.pending else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(Ok(result)) => result,
            Ok(Err(JobError::QueueFull)) => {
                self.pending = None;
                self.reload = true;
                return;
            }
            Ok(Err(error)) => Err(error.to_string()),
            Err(mpsc::TryRecvError::Disconnected) => Err("Trash worker disconnected".into()),
            Err(mpsc::TryRecvError::Empty) => return,
        };
        self.pending = None;
        match result {
            Ok(items) => {
                self.selected
                    .retain(|id| items.iter().any(|item| item.id == *id));
                self.items = items;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }
}

// trash-rs supplies mount-aware trash storage and metadata discovery. Its restore
// implementation checks existence then uses ordinary rename, so the final move
// here uses Linux's atomic no-overwrite variant instead.
pub(super) fn restore(item: &trash::TrashItem) -> Result<(), String> {
    let info = Path::new(&item.id);
    let root = info
        .parent()
        .and_then(Path::parent)
        .ok_or("Invalid trash entry")?;
    if info.extension() != Some(std::ffi::OsStr::new("trashinfo")) {
        return Err("Invalid trash metadata name".into());
    }
    let source = root
        .join("files")
        .join(info.file_stem().ok_or("Invalid trash metadata name")?);
    let original = item.original_path();
    std::fs::create_dir_all(&item.original_parent).map_err(|e| e.to_string())?;
    rename_noreplace(&source, &original).map_err(|e| format!("Restore refused: {e}"))?;
    std::fs::remove_file(info).map_err(|e| {
        format!(
            "File restored at {}; trash metadata cleanup failed: {e}",
            super::super::display_path(&original)
        )
    })
}
