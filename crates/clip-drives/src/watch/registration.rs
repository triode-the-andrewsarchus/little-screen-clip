//! Directory readers and registration syscalls live entirely on retained workers.
use super::*;
use little_clip_core::jobs::{BackgroundJobs, task::Task};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};

#[derive(Clone)]
pub(super) struct Location {
    pub root: Arc<Root>,
    pub relative: PathBuf,
    pub serial: u64,
}
impl Location {
    pub fn path(&self) -> PathBuf {
        self.root.path.join(&self.relative)
    }
}
pub(super) struct Pending {
    pub location: Location,
    pub epoch: u64,
    acknowledgement: mpsc::Receiver<i32>,
    pub task: Task<Result<Inventory>>,
}
pub(super) struct Inventory {
    pub children: Vec<PathBuf>,
    pub complete: bool,
}
impl Pending {
    pub fn start(pool: &BackgroundJobs, file: Arc<File>, location: Location, epoch: u64) -> Self {
        let (sender, acknowledgement) = mpsc::sync_channel(1);
        let worker = location.clone();
        let task = Task::start_unbounded(pool, move |cancel| {
            inventory(&file, &worker, sender, cancel)
        });
        Self {
            location,
            epoch,
            acknowledgement,
            task,
        }
    }
    pub fn acknowledgement(&self) -> Option<i32> {
        self.acknowledgement.try_recv().ok()
    }
    pub fn cancel(&self) {
        self.task.token.store(true, Ordering::Release);
    }
}

fn inventory(
    file: &File,
    location: &Location,
    acknowledgement: mpsc::SyncSender<i32>,
    cancel: &AtomicBool,
) -> Result<Inventory> {
    crate::cancelled(cancel)?;
    let wd = register(file, location)?;
    let _ = acknowledgement.try_send(wd);
    crate::cancelled(cancel)?;
    let mut frame = crate::scan::Frame::new(&location.root, location.relative.clone())?;
    let mut children = Vec::new();
    let mut bytes = 0_usize;
    for child in frame.reader.by_ref() {
        crate::cancelled(cancel)?;
        let child = child?;
        if !child.file_type()?.is_dir() {
            continue;
        }
        let relative = location.relative.join(child.file_name());
        bytes = bytes.saturating_add(watch_bytes(&location.root.path.join(&relative), &relative));
        if children.len() >= 8192 || bytes > 16 * 1024 * 1024 || relative.components().count() > 256
        {
            return Ok(Inventory {
                children,
                complete: false,
            });
        }
        children.push(relative);
    }
    frame.validate(&location.root)?;
    Ok(Inventory {
        children,
        complete: true,
    })
}

fn register(file: &File, location: &Location) -> Result<i32> {
    let pin = location.root.directory(&location.relative, false)?;
    let name = CString::new(anchor(&pin).as_os_str().as_bytes())
        .map_err(|_| "Invalid pinned watch-directory address")?;
    let mask = libc::IN_CLOSE_WRITE
        | libc::IN_ATTRIB
        | libc::IN_MOVED_TO
        | libc::IN_MOVED_FROM
        | libc::IN_CREATE
        | libc::IN_DELETE
        | libc::IN_DELETE_SELF
        | libc::IN_MOVE_SELF
        | libc::IN_ONLYDIR;
    // SAFETY: the descriptor path names the held ordinary directory.
    let wd = unsafe { libc::inotify_add_watch(file.as_raw_fd(), name.as_ptr(), mask) };
    if wd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(wd)
}
