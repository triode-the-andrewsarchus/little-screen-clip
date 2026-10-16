//! One shared kernel-object collector and presentation for /dev and /sys.
mod attributes;
mod boot_firmware;
mod btrfs;
mod buses;
mod cache;
mod cgroups;
mod clock_watchdog;
mod collector;
mod connectors;
mod console;
mod cpu;
mod device_controls;
mod device_directories;
mod device_tree;
mod directories;
mod drm;
mod events;
mod firmware;
mod flash;
mod identity;
mod iio;
mod illumination;
mod input;
mod inspector;
mod kernel_state;
mod model;
mod modules;
mod network;
mod numa;
mod path_serde;
mod pci;
mod peripheral_bus;
mod pins;
mod power_controls;
mod precision_clock;
mod processes;
mod pseudo;
mod remote;
mod sound;
mod storage;
mod storage_bus;
mod telemetry;
mod terminal;
mod vfio;
mod video;
mod worker;
use super::{filesystem::Entry, shelf::preview::Ready};
use crate::ui_appearance::WorkbenchColors;
pub(super) use cache::collect;
pub(crate) use console::run as run_console_worker;
pub(super) use inspector::BrowserTarget;
pub(super) use inspector::Inspector;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
pub(super) use worker::jobs;

use model::Fact;
pub(super) const RESPONSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Relation {
    pub label: String,
    #[serde(with = "path_serde")]
    pub target: PathBuf,
    pub identity: Option<Vec<u64>>,
    #[serde(default)]
    pub context: Option<identity::ObjectContext>,
    pub directory: bool,
    #[serde(default)]
    pub system: bool,
    #[serde(default)]
    pub process_start: Option<u64>,
    #[serde(default)]
    pub loop_association: Option<storage::loops::Association>,
    #[serde(default)]
    pub cgroup_hierarchy: Option<cgroups::HierarchyScope>,
}
impl Relation {
    /// Keep navigation bound to the interface actually used for the summary.
    fn observed_interface(label: impl Into<String>, target: PathBuf, observed: &Document) -> Self {
        let mut relation = Self::new(label, target);
        relation.identity = Some(observed.identity.clone());
        relation.context = observed.interface_context;
        relation.directory = observed.interface_directory;
        relation.system = observed.kind.semantic();
        relation
    }
    pub fn new(label: impl Into<String>, target: PathBuf) -> Self {
        let pinned = little_clip_core::kernel_interface::pin(&target, true).ok();
        let metadata = pinned.as_ref().and_then(|file| file.metadata().ok());
        let context = pinned
            .as_ref()
            .and_then(|file| identity::ObjectContext::pinned(file).ok());
        let kind = pinned
            .as_ref()
            .and_then(|file| little_clip_core::kernel_interface::classify_handle(file).ok());
        let identity = metadata.as_ref().map(|metadata| {
            identity::interface_fingerprint(
                kind.unwrap_or(little_clip_core::kernel_interface::Kind::Special),
                metadata,
            )
        });
        let directory = metadata.is_some_and(|m| m.is_dir());
        let system = pinned.as_ref().is_some_and(|file| {
            little_clip_core::kernel_interface::classify_handle(file)
                .is_ok_and(|kind| kind.semantic())
        });
        Self {
            label: label.into(),
            target,
            identity,
            context,
            directory,
            system,
            process_start: None,
            loop_association: None,
            cgroup_hierarchy: None,
        }
    }
    fn verify(&self) -> Result<(), String> {
        let pinned = little_clip_core::kernel_interface::pin(&self.target, true)
            .map_err(|error| error.to_string())?;
        if self.context.is_none() || identity::ObjectContext::pinned(&pinned).ok() != self.context {
            return Err("Relationship mount/type context changed or unavailable; refresh the original object".into());
        }
        let metadata = pinned.metadata().map_err(|error| error.to_string())?;
        if self.identity.as_ref()
            != Some(&identity::interface_fingerprint(
                self.context.unwrap().kind,
                &metadata,
            ))
        {
            return Err("Relationship target changed; refresh the original object".into());
        }
        if let Some(scope) = &self.cgroup_hierarchy {
            scope.verify(&self.target)?;
        }
        if let Some(association) = &self.loop_association {
            association.verify()?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Document {
    #[serde(with = "path_serde")]
    pub path: PathBuf,
    pub interface_directory: bool,
    pub identity: Vec<u64>,
    pub namespace: identity::Namespace,
    pub kernel_identity: Option<Vec<u64>>,
    #[serde(default)]
    pub interface_context: Option<identity::ObjectContext>,
    #[serde(default)]
    pub kernel_context: Option<identity::ObjectContext>,
    pub generation: u64,
    pub event_revision: u64,
    pub last_error: Option<String>,
    pub kind: little_clip_core::kernel_interface::Kind,
    pub title: String,
    pub observed_ms: u64,
    pub facts: Vec<Fact>,
    pub relations: Vec<Relation>,
    #[serde(default)]
    pub storage: Option<storage::Storage>,
    #[serde(default)]
    pub topology: Option<buses::Topology>,
    #[serde(default)]
    pub cpu: Option<cpu::Cpu>,
    #[serde(default)]
    pub telemetry: Option<telemetry::Telemetry>,
    #[serde(default)]
    pub numa: Option<numa::Numa>,
}
impl Document {
    fn observation_context(&self) -> Option<&identity::ObjectContext> {
        if self.shared_object().is_some() {
            self.kernel_context.as_ref()
        } else {
            self.interface_context.as_ref()
        }
    }
    fn observed_fact(&self, label: &str) -> Option<&model::Fact> {
        self.facts.iter().find(|fact| {
            fact.label == label && fact.outcome == model::Outcome::Observed && !fact.retained
        })
    }
    fn observed_raw(&self, label: &str) -> Option<&str> {
        self.observed_fact(label)?.raw.as_deref()
    }
    fn observed_attribute(&self, root: &Path, name: &str) -> Option<&model::Fact> {
        let source = root.join(name);
        self.facts
            .iter()
            .find(|fact| fact.source == source)
            .filter(|fact| fact.outcome == model::Outcome::Observed && !fact.retained)
    }
    /// Returns whether a recorded event invalidated this snapshot.
    pub fn invalidated(&self) -> bool {
        self.event_revision != events::revision()
            && events::affected(self.event_revision, self.event_paths())
    }
    /// Returns the stale-observation label for the caller's refresh interval.
    pub fn observation_status(
        &self,
        age: std::time::Duration,
        stale_after: std::time::Duration,
    ) -> Option<&'static str> {
        if self.invalidated() {
            Some("Observation invalidated")
        } else if self.last_error.is_some() || age >= stale_after {
            Some("Last observation")
        } else {
            None
        }
    }
    fn event_paths(&self) -> impl Iterator<Item = &Path> {
        self.relations
            .iter()
            .map(|relation| relation.target.as_path())
            .chain(std::iter::once(self.path.as_path()))
            .chain(self.facts.iter().map(|fact| fact.source.as_path()))
            .chain(
                self.conservative_sysfs_events()
                    .then_some(Path::new("/sys")),
            )
    }
    fn conservative_sysfs_events(&self) -> bool {
        let alternate_mount = matches!(
            self.kind,
            little_clip_core::kernel_interface::Kind::Kernel {
                filesystem: little_clip_core::kernel_interface::Filesystem::Sysfs
            }
        ) && !self.path.starts_with("/sys")
            || self
                .shared_object()
                .is_some_and(|root| !root.starts_with("/sys"));
        // Directory inventories also depend on devices not present in the last
        // snapshot. A new registration has no individual dependency path yet.
        let device_inventory = self.interface_directory
            && self.kind
                == (little_clip_core::kernel_interface::Kind::Kernel {
                    filesystem: little_clip_core::kernel_interface::Filesystem::DeviceNodes,
                });
        device_inventory
            || alternate_mount
            || self
                .facts
                .iter()
                .map(|fact| fact.source.as_path())
                .chain(
                    self.relations
                        .iter()
                        .map(|relation| relation.target.as_path()),
                )
                .any(|path| {
                    ["/sys/class", "/sys/bus", "/sys/dev"]
                        .iter()
                        .any(|alias| path.starts_with(alias))
                })
    }
    fn shared_object(&self) -> Option<&Path> {
        use little_clip_core::kernel_interface::{Filesystem, Kind};
        let share = match self.kind {
            Kind::Character { .. } | Kind::Block { .. } => true,
            Kind::Kernel {
                filesystem: Filesystem::Sysfs,
            } => self.interface_directory,
            _ => false,
        };
        if self.kernel_identity.is_none() || !share {
            return None;
        }
        self.relations
            .iter()
            .find(|relation| relation.label == "Kernel object")
            .map(|relation| relation.target.as_path())
    }
    pub fn summary(&self) -> String {
        if let Some(data) = &self.numa {
            return data.summary.clone();
        }
        if let Some(storage) = &self.storage {
            return storage.summary();
        }
        if let Some(topology) = &self.topology {
            return topology.summary.clone();
        }
        if let Some(cpu) = &self.cpu {
            return cpu.summary.clone();
        }
        if let Some(telemetry) = &self.telemetry {
            return telemetry.summary.clone();
        }
        self.facts
            .iter()
            .filter_map(|f| {
                f.value.as_ref().map(|value| {
                    if f.retained {
                        format!("{}: {value} (last observation; {})", f.label, f.outcome)
                    } else {
                        format!("{}: {value}", f.label)
                    }
                })
            })
            .take(4)
            .collect::<Vec<_>>()
            .join(" · ")
    }
}
pub(super) fn load(
    path: &Path,
    token: &std::sync::atomic::AtomicBool,
) -> super::media::PreviewData {
    let mut data = super::media::PreviewData::new(
        String::new(),
        "System interface".into(),
        crate::icons::Icon::Settings,
    );
    match cache::preview(path, token) {
        Ok(document) => {
            data.detail = document.title.clone();
            data.system = Some(document);
        }
        Err(error) => data.detail = error,
    }
    data
}
pub(super) fn show(
    ui: &mut egui::Ui,
    row: egui::Rect,
    entry: &Entry,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
    palette: &little_clip_core::palette::AppPalette,
) {
    let body = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("System interface", |r| r.data.detail.as_str()),
        colors,
    );
    let icon = egui::Rect::from_min_size(body.min, egui::vec2(body.height(), body.height()));
    crate::icons::Icon::Eye.paint(ui.painter(), icon.shrink(15.0), colors.text_secondary);
    ui.interact(
        icon,
        egui::Id::new(("system-inspection-icon", &entry.path)),
        egui::Sense::hover(),
    )
    .on_hover_text("Inspect system object");
    let text = egui::Rect::from_min_max(egui::pos2(icon.right() + 10.0, body.top()), body.max);
    let mut summary = ready
        .and_then(|r| r.data.system.as_ref())
        .map(|document| document.summary())
        .unwrap_or_else(|| {
            ready
                .map_or("Inspecting metadata…", |r| r.data.detail.as_str())
                .into()
        });
    if let Some(document) = ready.and_then(|r| r.data.system.as_ref()) {
        let age = model::now_ms().saturating_sub(document.observed_ms) / 1000;
        if let Some(status) = document.observation_status(
            std::time::Duration::from_secs(age),
            std::time::Duration::from_secs(10),
        ) {
            summary = format!("{status} ({age}s old) · {summary}");
        }
    }
    if let Some(storage) = ready
        .and_then(|ready| ready.data.system.as_ref())
        .and_then(|document| document.storage.as_ref())
    {
        let strip = egui::Rect::from_min_max(egui::pos2(text.left(), text.top() + 25.0), text.max);
        storage::paint(ui, strip, storage, colors);
    }
    if let Some(data) = ready
        .and_then(|ready| ready.data.system.as_ref())
        .and_then(|document| document.telemetry.as_ref())
    {
        telemetry::paint(
            ui,
            egui::Rect::from_min_max(egui::pos2(text.left(), text.top() + 25.0), text.max),
            data,
            colors,
            palette,
        );
    }
    if let Some(cpu) = ready
        .and_then(|ready| ready.data.system.as_ref())
        .and_then(|document| document.cpu.as_ref())
    {
        cpu::paint(
            ui,
            egui::Rect::from_min_max(egui::pos2(text.left(), text.top() + 25.0), text.max),
            cpu,
            colors,
            palette,
        );
    }
    if let Some(topology) = ready
        .and_then(|ready| ready.data.system.as_ref())
        .and_then(|document| document.topology.as_ref())
    {
        buses::paint(
            ui,
            egui::Rect::from_min_max(egui::pos2(text.left(), text.top() + 25.0), text.max),
            topology,
            colors,
        );
    }
    if let Some(data) = ready
        .and_then(|ready| ready.data.system.as_ref())
        .and_then(|document| document.numa.as_ref())
    {
        numa::paint(
            ui,
            egui::Rect::from_min_max(egui::pos2(text.left(), text.top() + 25.0), text.max),
            data,
            colors,
            palette,
        );
    }
    let label_y = if ready
        .and_then(|ready| ready.data.system.as_ref())
        .is_some_and(|document| {
            document.storage.is_some()
                || document.topology.is_some()
                || document.cpu.is_some()
                || document.telemetry.is_some()
                || document.numa.is_some()
        }) {
        text.top() + 10.0
    } else {
        text.center().y
    };
    ui.painter()
        .with_clip_rect(text.intersect(ui.clip_rect()))
        .text(
            egui::pos2(text.left(), label_y),
            egui::Align2::LEFT_CENTER,
            summary,
            egui::FontId::proportional(13.0),
            colors.text_secondary,
        );
}
pub fn run_worker(path: &Path, output: &Path) -> Result<(), String> {
    worker::run(path, output)
}

pub fn run_process_worker(output: &Path) -> Result<(), String> {
    processes::run(output)
}

pub(super) fn event_revision() -> u64 {
    events::revision()
}
pub(super) fn shutdown() {
    events::shutdown();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_status_uses_each_views_two_interval_cutoff_and_failure() {
        use std::time::Duration;
        let mut document = super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        document.event_revision = super::events::revision();
        for seconds in [2, 10] {
            let cutoff = Duration::from_secs(seconds);
            assert_eq!(
                document.observation_status(cutoff - Duration::from_millis(1), cutoff),
                None
            );
            assert_eq!(
                document.observation_status(cutoff, cutoff),
                Some("Last observation")
            );
        }
        document.last_error = Some("Collection unavailable".into());
        assert_eq!(
            document.observation_status(Duration::ZERO, Duration::from_secs(10)),
            Some("Last observation")
        );
    }

    #[test]
    fn relationship_from_observed_interface_cannot_adopt_a_retargeted_alias() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original");
        let replacement = temp.path().join("replacement");
        let alias = temp.path().join("alias");
        std::fs::write(&original, b"original").unwrap();
        std::fs::write(&replacement, b"replacement").unwrap();
        symlink(&original, &alias).unwrap();
        let observed = worker::interface(&alias).unwrap();
        std::fs::remove_file(&alias).unwrap();
        symlink(replacement, &alias).unwrap();
        let relation = Relation::observed_interface("Observed target", alias, &observed);
        assert_eq!(relation.identity.as_ref(), Some(&observed.identity));
        assert!(relation.verify().is_err());
    }

    #[test]
    fn device_namespace_inventories_watch_new_registrations_without_ui_path_resolution() {
        let mut document = worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = "/dev/disk".into();
        document.kind = little_clip_core::kernel_interface::Kind::Kernel {
            filesystem: little_clip_core::kernel_interface::Filesystem::DeviceNodes,
        };
        document.interface_directory = true;
        document.relations.clear();
        document.facts.clear();
        assert!(document.event_paths().any(|path| path == Path::new("/sys")));
        document.interface_directory = false;
        assert!(!document.event_paths().any(|path| path == Path::new("/sys")));
    }

    #[test]
    fn relationship_rejects_replaced_and_missing_targets() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("object");
        std::fs::write(&path, b"original").unwrap();
        let relation = Relation::new("Object", path.clone());
        assert!(relation.verify().is_ok());
        std::fs::rename(&path, temp.path().join("old")).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        assert!(relation.verify().is_err());
        std::fs::remove_file(&path).unwrap();
        assert!(relation.verify().is_err());
        assert!(Relation::new("Missing", path).verify().is_err());
    }
}

#[cfg(test)]
pub(in crate::file_manager) fn retained_provenance_fixture() -> Document {
    let mut document = worker::interface(Path::new("/proc/uptime")).unwrap();
    document.generation = 50;
    let mut fact = Fact::text("Terminal", "PID 42", "/proc/42/stat".into());
    fact.observed_ms = 123_000;
    fact.observed_generation = Some(40);
    document.facts = vec![fact];
    document.stamp_fact_generations();
    document
}
