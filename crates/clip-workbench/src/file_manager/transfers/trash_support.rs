use std::collections::BTreeSet;
use std::ffi::OsString;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use super::super::filesystem::{identity, rename_at_noreplace};
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
fn restore_source(item: &trash::TrashItem) -> Result<std::path::PathBuf, String> {
    if Path::new(&item.name).file_name() != Some(item.name.as_os_str()) {
        return Err("Invalid restore filename".into());
    }
    let info = Path::new(&item.id);
    let root = info
        .parent()
        .and_then(Path::parent)
        .ok_or("Invalid trash entry")?;
    if info.extension() != Some(std::ffi::OsStr::new("trashinfo")) {
        return Err("Invalid trash metadata name".into());
    }
    let name = info.file_stem().ok_or("Invalid trash metadata name")?;
    if Path::new(name).file_name() != Some(name) {
        return Err("Invalid trash payload name".into());
    }
    Ok(root.join("files").join(name))
}

pub(super) fn preflight_restore(
    item: &trash::TrashItem,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    let source = restore_source(item)?;
    little_clip_core::kernel_interface::require_content(Path::new(&item.id))
        .map_err(|e| e.to_string())?;
    let mut parent = item.original_parent.as_path();
    while !parent.exists() {
        parent = parent.parent().ok_or("Missing restore ancestor")?;
    }
    little_clip_core::kernel_interface::require_content(parent).map_err(|e| e.to_string())?;
    super::super::system_route::tree(&source, cancelled)
}

pub(super) fn restore(item: &trash::TrashItem) -> Result<(), String> {
    preflight_restore(item, &std::sync::atomic::AtomicBool::new(false))?;
    let directory =
        little_clip_core::kernel_interface::create_content_directory(&item.original_parent)
            .map_err(|e| e.to_string())?;
    restore_in(item, &directory)
}

// The destination handle remains owned through installation and metadata cleanup.
pub(super) fn restore_in(item: &trash::TrashItem, directory: &std::fs::File) -> Result<(), String> {
    let info = Path::new(&item.id);
    let source = restore_source(item)?;
    let source_name = source.file_name().ok_or("Missing trash filename")?;
    let info_name = info.file_name().ok_or("Missing trash metadata filename")?;
    let source_parent = little_clip_core::kernel_interface::pin_content_directory(
        source.parent().ok_or("Missing trash directory")?,
    )
    .map_err(|e| e.to_string())?;
    let info_parent = little_clip_core::kernel_interface::pin_content_directory(
        info.parent().ok_or("Missing trash metadata directory")?,
    )
    .map_err(|e| e.to_string())?;
    let source =
        Path::new(&format!("/proc/self/fd/{}", source_parent.as_raw_fd())).join(source_name);
    let info = Path::new(&format!("/proc/self/fd/{}", info_parent.as_raw_fd())).join(info_name);
    little_clip_core::kernel_interface::require_content(&info).map_err(|e| e.to_string())?;
    let expected = identity(&std::fs::symlink_metadata(&source).map_err(|e| e.to_string())?);
    super::super::system_route::tree(&source, &std::sync::atomic::AtomicBool::new(false))?;
    if identity(&std::fs::symlink_metadata(&source).map_err(|e| e.to_string())?) != expected {
        return Err("Trash payload changed before restore".into());
    }
    rename_at_noreplace(
        source_parent.as_raw_fd(),
        source_name,
        directory.as_raw_fd(),
        &item.name,
    )
    .map_err(|e| format!("Restore refused: {e}"))?;
    std::fs::remove_file(&info).map_err(|e| {
        let original = std::fs::read_link(format!("/proc/self/fd/{}", directory.as_raw_fd()))
            .unwrap_or_else(|_| item.original_parent.clone())
            .join(&item.name);
        format!(
            "File restored at {}; trash metadata cleanup failed: {e}",
            super::super::display_path(&original)
        )
    })
}
