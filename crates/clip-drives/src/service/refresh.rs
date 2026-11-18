//! One retained status observer; fresh execution admission remains independent.
use super::*;
use little_clip_core::jobs::task::Task;

pub(super) struct Observation {
    drives: Result<Vec<Drive>>,
    ram_unavailable: Option<String>,
}
fn observe(inventory: &Inventory, config: &DriveConfig) -> Observation {
    let drives = inventory();
    let ram_unavailable = ram_reason(config).err().map(|error| error.to_string());
    Observation {
        drives,
        ram_unavailable,
    }
}

pub(super) struct Refresh {
    inventory: Inventory,
    jobs: little_clip_core::jobs::BackgroundJobs,
    desired: Option<DriveConfig>,
    pending: Option<(DriveConfig, Task<Result<Observation>>)>,
    again: bool,
}
impl Refresh {
    pub(super) fn new(inventory: Inventory) -> Self {
        Self {
            inventory,
            jobs: little_clip_core::jobs::BackgroundJobs::new_named(1, 1, "drive-status"),
            desired: None,
            pending: None,
            again: false,
        }
    }
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some() || self.again
    }
    pub(super) fn request(&mut self, config: &DriveConfig, fresh: bool) {
        self.desired = Some(config.clone());
        if let Some((active, _)) = &self.pending {
            self.again |= fresh || active != config;
        } else {
            let inventory = self.inventory.clone();
            let worker_config = config.clone();
            let task = write_task(&self.jobs, move || Ok(observe(&inventory, &worker_config)));
            self.pending = Some((config.clone(), task));
        }
    }
    fn poll(&mut self) -> Option<(DriveConfig, Result<Observation>)> {
        let delivered = self.pending.as_ref()?.1.poll()?;
        let (config, _) = self.pending.take().expect("delivered drive refresh");
        if std::mem::take(&mut self.again) {
            let desired = self.desired.clone().expect("queued refresh configuration");
            self.request(&desired, false);
        }
        Some((config, write_result(delivered)))
    }
}

impl Owner {
    // Initial startup still establishes the original inventory before returning.
    // Recovery/startup scheduling is a separate remaining admission stage.
    pub(super) fn discover(&mut self) {
        self.refresh.desired = Some(self.config.clone());
        let result = observe(&self.inventory, &self.config);
        self.accept_refresh(&self.config.clone(), Ok(result));
    }
    pub(super) fn request_refresh(&mut self, fresh: bool) {
        if self.closing.load(Ordering::Acquire) && !fresh {
            return;
        }
        if self.refresh.desired.as_ref().is_some_and(|old| {
            old.ram_directory != self.config.ram_directory
                || old.ram_min_available != self.config.ram_min_available
        }) && let Ok(mut status) = self.shared.lock()
        {
            status.ram_unavailable = Some("Checking RAM storage availability".into());
        }
        self.refresh.request(&self.config, fresh);
    }
    pub(super) fn poll_refresh(&mut self) {
        let Some((config, result)) = self.refresh.poll() else {
            return;
        };
        if self.accept_refresh(&config, result) {
            self.sync_roots.clear();
            self.sync_epoch = self.sync_epoch.wrapping_add(1);
            if !self.closing.load(Ordering::Acquire) {
                self.automatic();
            }
        }
        let _ = self.publish_policy(false);
    }
    fn accept_refresh(&mut self, config: &DriveConfig, result: Result<Observation>) -> bool {
        let mut status = match self.shared.lock() {
            Ok(status) => status,
            Err(_) => return false,
        };
        let previous = status.drives.clone();
        match result {
            Ok(observed) => {
                match observed.drives {
                    Ok(drives) => {
                        status.drives = drives;
                        status.discovery_error = None;
                    }
                    Err(error) => {
                        status.drives.clear();
                        status.discovery_error = Some(error.to_string());
                    }
                }
                if config == &self.config {
                    status.ram_unavailable = observed.ram_unavailable;
                }
            }
            Err(error) => {
                status.drives.clear();
                status.discovery_error = Some(error.to_string());
                if config == &self.config {
                    status.ram_unavailable = Some("RAM availability observation failed".into());
                }
            }
        }
        status.backup_directory = self.config.backup_directory.clone();
        status.config = self.config.clone();
        previous != status.drives
    }
}
