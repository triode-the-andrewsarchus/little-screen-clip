//! A collector owns one bounded read budget and records per-field outcomes.
use super::{
    Document,
    attributes::{self, Context, Family, Format, Rule},
    model::{Fact, Outcome},
};
use little_clip_core::kernel_interface::{self, Filesystem, Kind};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

pub(super) struct Reads {
    #[cfg(test)]
    fixture: Option<BTreeMap<PathBuf, Vec<u8>>>,
    stable: BTreeMap<PathBuf, Fact>,
    probes: usize,
    count: usize,
    bytes: usize,
    cache: BTreeMap<PathBuf, Result<String, (io::ErrorKind, String)>>,
}
impl Reads {
    pub fn new() -> Self {
        Self {
            #[cfg(test)]
            fixture: None,
            stable: BTreeMap::new(),
            probes: 0,
            count: 0,
            bytes: 0,
            cache: BTreeMap::new(),
        }
    }
    fn seed_stable(&mut self, previous: Option<&Document>, sys: &Path) {
        self.stable = previous
            .into_iter()
            .flat_map(|document| &document.facts)
            .filter(|fact| {
                !fact.retained
                    && fact.outcome == Outcome::Observed
                    && fact.origin == "documented sysfs attribute"
                    && fact.source.parent() == Some(sys)
            })
            .map(|fact| (fact.source.clone(), fact.clone()))
            .collect();
    }
    pub fn read(&mut self, path: &Path) -> io::Result<String> {
        self.read_from(path, Filesystem::Sysfs)
    }
    /// Re-observes a changing attribute while retaining the same read/probe budgets.
    pub(super) fn read_fresh(&mut self, path: &Path) -> io::Result<String> {
        self.cache.remove(path);
        self.read(path)
    }
    /// Checked unsigned metadata shared by geometry and timing calculations.
    pub(super) fn unsigned(&mut self, path: &Path) -> io::Result<u64> {
        self.read(path)?.parse().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid unsigned system metadata",
            )
        })
    }
    pub(super) fn kernel_state(
        &mut self,
        path: &Path,
        filesystem: Filesystem,
    ) -> io::Result<String> {
        if !super::kernel_state::approved(path, filesystem) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Not approved kernel-state metadata",
            ));
        }
        self.read_from(path, filesystem)
    }
    pub(super) fn cgroup(&mut self, path: &Path) -> io::Result<String> {
        if !super::cgroups::approved(path) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Not approved cgroup metadata",
            ));
        }
        self.read_from(path, Filesystem::Cgroup)
    }
    pub(super) fn asound(&mut self, path: &Path) -> io::Result<String> {
        if !super::sound::proc_metadata(path) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Not approved ALSA metadata",
            ));
        }
        self.read_from(path, Filesystem::Proc)
    }
    fn read_from(&mut self, path: &Path, filesystem: Filesystem) -> io::Result<String> {
        if let Some(result) = self.cache.get(path) {
            return result
                .clone()
                .map_err(|(kind, message)| io::Error::new(kind, message));
        }
        let result = self.read_uncached(path, filesystem);
        self.cache.insert(
            path.to_path_buf(),
            result
                .as_ref()
                .map(Clone::clone)
                .map_err(|e| (e.kind(), e.to_string())),
        );
        result
    }
    fn read_uncached(&mut self, path: &Path, filesystem: Filesystem) -> io::Result<String> {
        let bytes = self.attribute_payload(path, usize::MAX, filesystem)?;
        let value = String::from_utf8(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Non-UTF-8 attribute"))?;
        if value.chars().count() > 4096 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Attribute exceeds display-data budget",
            ));
        }
        Ok(value.trim().to_string())
    }
    fn payload(
        &mut self,
        path: &Path,
        limit: usize,
        filesystem: Filesystem,
    ) -> io::Result<Vec<u8>> {
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            let bytes = fixture
                .get(path)
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            self.count += 1;
            return Ok(bytes.iter().take(limit).copied().collect());
        }
        let file = kernel_file(path, filesystem)?;
        self.count += 1;
        let mut bytes = Vec::new();
        file.take(limit as u64).read_to_end(&mut bytes)?;
        Ok(bytes)
    }
    /// Reads the kernel's live loop backing path without trimming filename bytes.
    pub(super) fn loop_backing(&mut self, root: &Path) -> io::Result<PathBuf> {
        use std::os::unix::ffi::OsStringExt;
        #[cfg(test)]
        let fixture = self.fixture.is_some();
        #[cfg(not(test))]
        let fixture = false;
        if !fixture
            && kernel_interface::classify(root)?
                != (Kind::Kernel {
                    filesystem: Filesystem::Sysfs,
                })
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Loop metadata root is not sysfs",
            ));
        }
        let context = Context::discover(root);
        if context.family != Family::Block
            || !context.direct
            || !super::storage::loops::is_loop(root)
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Not a registered loop device",
            ));
        }
        let mut bytes =
            self.attribute_payload(&root.join("loop/backing_file"), 4097, Filesystem::Sysfs)?;
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.is_empty() || bytes.contains(&0) || bytes.ends_with(b" (deleted)") {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Backing pathname unavailable or deleted",
            ));
        }
        let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Backing pathname is not absolute",
            ));
        }
        Ok(path)
    }
    pub(super) fn edid(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        self.binary_metadata(path, "edid", "drm", 64 * 1024)
    }
    pub(super) fn hid_descriptor(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        self.binary_metadata(path, "report_descriptor", "hid", 16 * 1024)
    }
    pub(super) fn device_tree(&mut self, path: &Path, root: &Path) -> io::Result<Vec<u8>> {
        if !super::device_tree::approved(path, root) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Not approved device-tree property",
            ));
        }
        self.probes += 1;
        if self.probes > 256 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Device-tree probe budget exhausted",
            ));
        }
        self.bounded_payload(path, self.remaining_limit()?.min(16 * 1024))
    }
    fn binary_metadata(
        &mut self,
        path: &Path,
        leaf: &str,
        subsystem: &str,
        maximum: usize,
    ) -> io::Result<Vec<u8>> {
        if path.file_name().is_none_or(|name| name != leaf)
            || path
                .parent()
                .is_none_or(|parent| super::buses::subsystem(parent).as_deref() != Some(subsystem))
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Not approved subsystem binary metadata",
            ));
        }
        self.bounded_payload(path, self.remaining_limit()?.min(maximum))
    }
    fn remaining_limit(&self) -> io::Result<usize> {
        if self.count >= 64 || self.bytes >= 1024 * 1024 {
            Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Observation read budget exhausted",
            ))
        } else {
            Ok((1024 * 1024 - self.bytes).min(64 * 1024))
        }
    }
    fn attribute_payload(
        &mut self,
        path: &Path,
        maximum: usize,
        filesystem: Filesystem,
    ) -> io::Result<Vec<u8>> {
        let limit = self.remaining_limit()?.min(maximum);
        self.probes += 1;
        if self.probes > 256 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Attribute probe budget exhausted",
            ));
        }
        self.bounded_payload_from(path, limit, filesystem)
    }
    fn bounded_payload(&mut self, path: &Path, limit: usize) -> io::Result<Vec<u8>> {
        self.bounded_payload_from(path, limit, Filesystem::Sysfs)
    }
    fn bounded_payload_from(
        &mut self,
        path: &Path,
        limit: usize,
        filesystem: Filesystem,
    ) -> io::Result<Vec<u8>> {
        let bytes = self.payload(path, limit, filesystem)?;
        self.bytes += bytes.len();
        if bytes.len() == limit {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Attribute reached the bounded read limit",
            ));
        }
        Ok(bytes)
    }
    #[cfg(test)]
    pub(super) fn fixture(values: BTreeMap<PathBuf, Vec<u8>>) -> Self {
        let mut reads = Self::new();
        reads.fixture = Some(values);
        reads
    }
    #[cfg(test)]
    pub(super) fn attempts(&self) -> impl Iterator<Item = &Path> {
        self.cache.keys().map(|path| path.as_path())
    }
    pub(super) fn observe(&mut self, document: &mut Document, root: &Path, rule: &Rule) -> Fact {
        if let Some(fact) = document
            .facts
            .iter()
            .find(|fact| fact.source == root.join(&rule.name))
        {
            return fact.clone();
        }
        let fact = self.fact(root, rule);
        document.facts.push(fact.clone());
        fact
    }
    pub(super) fn fact(&mut self, root: &Path, rule: &Rule) -> Fact {
        let source = root.join(&rule.name);
        if let Some(fact) = self.stable.get(&source) {
            if fact.label == rule.label
                && attributes::mostly_static(&Context::discover(root), &rule.name)
            {
                return fact.clone();
            }
        }
        Fact::attribute(
            &rule.label,
            source.clone(),
            "documented sysfs attribute",
            self.read(&source),
            |raw| attributes::parse(rule, raw),
        )
    }
    pub(super) fn mountinfo(&mut self) -> io::Result<Vec<u8>> {
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            return fixture
                .get(Path::new("/proc/self/mountinfo"))
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound));
        }
        let mut bytes = Vec::new();
        kernel_file(Path::new("/proc/self/mountinfo"), Filesystem::Proc)?
            .take(4 * 1024 * 1024)
            .read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}
pub(super) fn kernel_file(path: &Path, filesystem: Filesystem) -> io::Result<File> {
    let pinned = kernel_interface::pin(path, true)?;
    if kernel_interface::classify_handle(&pinned)? != (Kind::Kernel { filesystem })
        || !pinned.metadata()?.is_file()
    {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Not an allowed kernel attribute",
        ));
    }
    File::open(format!("/proc/self/fd/{}", pinned.as_raw_fd()))
}
/// Shared sysfs routing guard for directory/filesystem schemas without a bus family.
fn scoped_collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) -> bool {
    if document.kind
        != (Kind::Kernel {
            filesystem: Filesystem::Sysfs,
        })
    {
        return false;
    }
    type ScopedCollector = fn(&mut Document, &Path, &mut Reads, &mut dyn FnMut(&Document)) -> bool;
    let scoped: [ScopedCollector; 2] = [super::directories::collect, super::btrfs::collect];
    scoped
        .into_iter()
        .any(|collect| collect(document, sys, reads, checkpoint))
}
pub(super) fn attributes(
    document: &mut Document,
    sys: &Path,
    checkpoint: &mut dyn FnMut(&Document),
    previous: Option<&Document>,
) {
    let context = Context::discover(sys);
    let mut reads = Reads::new();
    reads.seed_stable(previous, sys);
    if matches!(
        document.kind,
        Kind::Kernel {
            filesystem: Filesystem::Sysfs
        }
    ) && let Some(label) = context.label()
    {
        document.title = format!(
            "{} {}",
            label,
            if document.path.is_file() {
                "attribute"
            } else {
                "object"
            }
        );
    }
    if scoped_collect(document, sys, &mut reads, checkpoint) {
        return;
    }
    let requested = document.requested_attribute();
    if super::device_tree::inspect(document, sys, requested.as_deref(), &mut reads, checkpoint) {
        return;
    }
    if super::boot_firmware::attribute(document, sys, requested.as_deref(), &mut reads) {
        checkpoint(document);
        return;
    }
    if binary_attribute(
        document,
        sys,
        &context,
        requested.as_deref(),
        &mut reads,
        checkpoint,
    ) {
        checkpoint(document);
        return;
    }
    if let Some(name) = requested {
        if let Some(mut rule) = attributes::rule(&context, &name) {
            super::telemetry::seed(document, sys, &context);
            checkpoint(document);
            sensor_metadata(&context, sys, &mut rule, &mut reads);
            let fact = reads.fact(sys, &rule);
            let cpu_mask = context.family == Family::Cpu
                && matches!(fact.semantic, Some(super::model::Value::CpuList(_)));
            document.facts.insert(0, fact);
            if cpu_mask {
                super::cpu::collect(document, sys, &mut reads, checkpoint);
            }
            collect_shared_context(document, sys, &context, &mut reads, checkpoint);
        } else {
            let mut fact = Fact::text(
                "Attribute policy",
                "Unknown attribute; content not read",
                document.path.clone(),
            );
            fact.outcome = Outcome::NotRequested;
            document.facts.insert(0, fact);
        }
    } else {
        super::telemetry::seed(document, sys, &context);
        collect_directory(document, sys, &context, &mut reads, checkpoint);
        collect_shared_context(document, sys, &context, &mut reads, checkpoint);
        if context.family == Family::Cpu {
            super::cpu::collect(document, sys, &mut reads, checkpoint);
        }
        if context.family == Family::Block {
            super::storage::collect(document, sys, &mut reads, checkpoint);
        }
        if context.family == Family::Nvme {
            nvme_children(document, sys);
        }
        super::network::collect(document, sys, checkpoint, &mut reads);
        super::sound::collect(document, sys, &mut reads, checkpoint);
        super::video::collect(document, sys, &mut reads, checkpoint);
        super::input::collect(document, sys, &mut reads, checkpoint);
        super::peripheral_bus::collect(document, sys, &mut reads, checkpoint);
        super::flash::collect(document, sys, &mut reads, checkpoint);
        super::vfio::collect(document, sys, &mut reads, checkpoint);
        super::modules::collect(document, sys, checkpoint);
        super::firmware::collect(document, sys, &mut reads, checkpoint);
        super::boot_firmware::collect(document, sys, &mut reads, checkpoint);
        super::pins::collect(document, sys, &mut reads, checkpoint);
        super::clock_watchdog::collect(document, sys, &mut reads, checkpoint);
        super::connectors::collect(document, sys, &mut reads, checkpoint);
        super::illumination::collect(document, sys, &mut reads, checkpoint);
        ancestry(document, sys, &mut reads, checkpoint);
        super::buses::collect(document, sys, &mut reads);
        super::pci::collect(document, sys, &mut reads);
        super::drm::collect(document, sys, &mut reads);
        super::terminal::collect(document, sys, &mut reads);
        super::directories::child_devices(document, sys);
    }
    checkpoint(document);
}
// Directory and explicitly allowed attribute previews share the same semantic context.
fn collect_shared_context(
    document: &mut Document,
    sys: &Path,
    context: &Context,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    super::telemetry::collect(document, sys, context, reads);
    super::numa::collect(document, sys, reads, checkpoint);
    super::power_controls::collect(document, sys, checkpoint);
    super::device_controls::collect(document, sys, reads, checkpoint);
    super::remote::collect(document, sys, reads, checkpoint);
    super::precision_clock::collect(document, sys, reads, checkpoint);
    super::storage_bus::collect(document, sys, reads, checkpoint);
    super::iio::collect(document, sys, reads, checkpoint);
}
fn binary_attribute(
    document: &mut Document,
    sys: &Path,
    context: &Context,
    requested: Option<&str>,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) -> bool {
    match (context.family, requested) {
        (Family::Drm, Some("edid")) => {
            super::drm::edid_facts(document, sys, reads);
        }
        (Family::Hid, Some("report_descriptor")) => {
            super::input::collect(document, sys, reads, checkpoint)
        }
        _ => return false,
    }
    true
}
fn nvme_children(document: &mut Document, sys: &Path) {
    if sys.join("hwmon").is_dir() {
        document
            .relations
            .push(super::Relation::new("Exposed sensors", sys.join("hwmon")));
    }
    let entries = match std::fs::read_dir(sys) {
        Ok(entries) => entries,
        Err(error) => {
            document
                .facts
                .push(Fact::failure("NVMe child inventory", sys.into(), error));
            return;
        }
    };
    for (index, entry) in entries.enumerate().take(129) {
        if index == 128 {
            document.facts.push(Fact::failure(
                "NVMe child inventory",
                sys.into(),
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "NVMe child inventory capped at 128 entries",
                ),
            ));
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.path().is_dir() {
            continue;
        }
        let subsystem = entry.path().join("subsystem").canonicalize().ok();
        let name = subsystem
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str());
        let label = match name {
            Some("block") => "Namespace",
            Some("nvme") => "NVMe controller",
            Some("hwmon") => "Exposed sensors",
            _ => continue,
        };
        if !matches!(
            kernel_interface::classify(&entry.path()),
            Ok(Kind::Kernel {
                filesystem: Filesystem::Sysfs
            })
        ) {
            continue;
        }
        let target = entry.path().canonicalize().unwrap_or_else(|_| entry.path());
        document.relations.push(super::Relation::new(label, target));
    }
}
fn collect_directory(
    document: &mut Document,
    sys: &Path,
    context: &Context,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let mut rules = attributes::rules(context);
    if context.family == Family::Cpu
        && context.leaf.starts_with("cpu")
        && context.leaf[3..].chars().all(|c| c.is_ascii_digit())
        && context.leaf.len() > 3
    {
        for rule in &mut rules {
            if rule.name.starts_with("scaling_")
                || rule.name.starts_with("cpuinfo_")
                || matches!(rule.name.as_str(), "related_cpus" | "affected_cpus")
            {
                rule.name = format!("cpufreq/{}", rule.name);
            }
        }
    }
    if matches!(context.family, Family::Hwmon | Family::Thermal) {
        rules.extend(dynamic_rules(document, sys, context));
        let mut seen = std::collections::BTreeSet::new();
        rules.retain(|rule| seen.insert(rule.name.clone()));
        if rules.len() > 48 {
            document.facts.push(Fact::failure(
                "Attribute inventory",
                sys.into(),
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Attribute inventory capped at 48 fields",
                ),
            ));
        }
    }
    for mut rule in rules.into_iter().take(48) {
        if context.family == Family::Hid && rule.name == "uevent" {
            continue;
        }
        sensor_metadata(context, sys, &mut rule, reads);
        let fact = reads.fact(sys, &rule);
        let observed = fact.outcome != Outcome::Missing;
        document.facts.push(fact);
        if observed {
            checkpoint(document);
        }
    }
    // Present the useful observed subsystem values before generic access metadata.
    document
        .facts
        .sort_by_key(|fact| matches!(fact.label.as_str(), "Permissions" | "Owner" | "Access"));
}
fn dynamic_rules(document: &mut Document, sys: &Path, context: &Context) -> Vec<Rule> {
    let entries = match std::fs::read_dir(sys) {
        Ok(entries) => entries,
        Err(error) => {
            document
                .facts
                .push(Fact::failure("Attribute inventory", sys.into(), error));
            return Vec::new();
        }
    };
    let mut rules = Vec::new();
    for (index, entry) in entries.enumerate().take(513) {
        if index == 512 {
            document.facts.push(Fact::failure(
                "Attribute inventory",
                sys.into(),
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Attribute inventory capped at 512 entries",
                ),
            ));
            break;
        }
        match entry {
            Ok(entry) => {
                if let Some(rule) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| attributes::rule(context, name))
                {
                    rules.push(rule);
                }
            }
            Err(error) => {
                document
                    .facts
                    .push(Fact::failure("Attribute inventory", sys.into(), error))
            }
        }
    }
    rules.sort_by(|a, b| a.name.cmp(&b.name));
    rules
}
pub(super) fn sensor_metadata(context: &Context, sys: &Path, rule: &mut Rule, reads: &mut Reads) {
    if context.family != Family::Hwmon {
        return;
    }
    let Some((channel, field)) = rule.name.split_once('_') else {
        return;
    };
    if field == "label" || field == "type" {
        return;
    }
    if let Ok(label) = reads.read(&sys.join(format!("{channel}_label"))) {
        rule.label = format!("{label} · {field}");
    }
    // Sensor type alone cannot identify the unit for every thermistor driver.
    if channel.starts_with("temp") && matches!(rule.format, Format::Scale(_, "°C")) {
        let sensor = reads.read(&sys.join(format!("{channel}_type")));
        let opaque = match sensor {
            Ok(value) if value == "4" => !reads
                .read(&sys.join("name"))
                .is_ok_and(|name| name == "ntc_thermistor"),
            Ok(value) => !value
                .parse::<u8>()
                .is_ok_and(|kind| (1..=6).contains(&kind)),
            Err(error) => error.kind() != io::ErrorKind::NotFound,
        };
        if opaque {
            rule.format = Format::Text;
            rule.label = format!("{} · raw reading (unit unverified)", rule.label);
        }
    }
}

fn ancestry(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let mut current = Some(sys);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..8 {
        let Some(path) = current else {
            break;
        };
        if let Ok(device) = udev::Device::from_syspath(path) {
            for (property, label) in [
                ("ID_VENDOR_FROM_DATABASE", "Vendor (device database)"),
                ("ID_MODEL_FROM_DATABASE", "Model (device database)"),
                ("ID_VENDOR", "Vendor"),
                ("ID_MODEL", "Model"),
                ("ID_SERIAL_SHORT", "Interface serial number"),
                ("ID_BUS", "Bus"),
            ] {
                if !seen.contains(property) {
                    if let Some(fact) = super::buses::database_fact(&device, path, property, label)
                    {
                        seen.insert(property.to_string());
                        document.facts.push(fact);
                        checkpoint(document);
                    }
                }
            }
            if let Some(node) = device.devnode() {
                if !document.relations.iter().any(|r| r.target == node) {
                    document
                        .relations
                        .push(super::Relation::new("Device node", node.to_path_buf()));
                }
            }
        }
        let context = Context::discover(path);
        if path != sys && matches!(context.family, Family::Usb | Family::Pci) {
            for rule in attributes::rules(&context).into_iter().take(16) {
                if document
                    .facts
                    .iter()
                    .any(|f| f.label == rule.label && f.value.is_some())
                {
                    continue;
                }
                let fact = reads.fact(path, &rule);
                if fact.outcome != Outcome::Missing {
                    document.facts.push(fact);
                    checkpoint(document);
                }
            }
        }
        current = path.parent();
    }
    document
        .facts
        .sort_by_key(|fact| matches!(fact.label.as_str(), "Permissions" | "Owner" | "Access"));
}

pub(super) fn device_directory(document: &mut Document) {
    let reader = match std::fs::read_dir(&document.path) {
        Ok(reader) => reader,
        Err(error) => {
            document.facts.insert(
                0,
                Fact::failure("Interface inventory", document.path.clone(), error),
            );
            return;
        }
    };
    let (mut characters, mut blocks, mut aliases, mut directories, mut unreadable) =
        (0, 0, 0, 0, 0);
    let mut count = 0;
    for entry in reader.take(513) {
        count += 1;
        if count > 512 {
            break;
        }
        let Ok(entry) = entry else {
            unreadable += 1;
            continue;
        };
        if entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
            aliases += 1;
        }
        match kernel_interface::classify(&entry.path()) {
            Ok(Kind::Character { .. }) => characters += 1,
            Ok(Kind::Block { .. }) => blocks += 1,
            Ok(_) => {
                if entry.path().is_dir() {
                    directories += 1;
                }
            }
            Err(_) => unreadable += 1,
        }
    }
    let summary = format!(
        "{characters} character interfaces · {blocks} block interfaces · {aliases} aliases · {directories} directories{}",
        if unreadable > 0 {
            format!(" · {unreadable} unavailable")
        } else {
            String::new()
        }
    );
    let mut fact = Fact::text("Device interface inventory", summary, document.path.clone());
    if count > 512 {
        fact.outcome = Outcome::Truncated;
    }
    document.facts.insert(0, fact);
}

#[cfg(test)]
pub(super) fn registered_fixture(
    subsystem: &str,
    name: &str,
    values: &[(&str, &str)],
) -> (tempfile::TempDir, std::path::PathBuf, Reads, Document) {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let class = temp.path().join("class").join(subsystem);
    let root = temp.path().join(name);
    std::fs::create_dir_all(&class).unwrap();
    std::fs::create_dir(&root).unwrap();
    symlink(class, root.join("subsystem")).unwrap();
    for (name, raw) in values {
        std::fs::write(root.join(name), raw).unwrap();
    }
    let reads = Reads::fixture(
        values
            .iter()
            .map(|(name, raw)| (root.join(name), raw.as_bytes().to_vec()))
            .collect(),
    );
    let mut document = super::worker::interface(Path::new("/proc/uptime")).unwrap();
    document.interface_directory = false;
    (temp, root, reads, document)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_metadata_dispatch_rejects_other_filesystems_before_callbacks() {
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        let mut reads = Reads::fixture(BTreeMap::new());
        assert!(!scoped_collect(
            &mut document,
            Path::new("/sys/fs/btrfs"),
            &mut reads,
            &mut |_| {}
        ));
        assert_eq!(reads.attempts().count(), 0);
        document.kind = Kind::Content;
        assert!(!scoped_collect(
            &mut document,
            Path::new("/sys/fs/btrfs"),
            &mut reads,
            &mut |_| {}
        ));
        assert_eq!(reads.attempts().count(), 0);
    }

    #[test]
    fn stable_identity_reuses_original_observation_while_status_is_read_again() {
        let (_temp, root, mut initial, mut previous) = registered_fixture(
            "usb",
            "1-2",
            &[("idVendor", "1234"), ("bConfigurationValue", "1")],
        );
        let context = Context::discover(&root);
        let identity = attributes::rule(&context, "idVendor").unwrap();
        let status = attributes::rule(&context, "bConfigurationValue").unwrap();
        let mut old_identity = initial.fact(&root, &identity);
        old_identity.observed_ms = 17;
        previous.facts = vec![old_identity, initial.fact(&root, &status)];
        let mut next = Reads::fixture(BTreeMap::from([
            (root.join("idVendor"), b"9999".to_vec()),
            (root.join("bConfigurationValue"), b"0".to_vec()),
        ]));
        next.seed_stable(Some(&previous), &root);
        let reused = next.fact(&root, &identity);
        assert_eq!(reused.raw.as_deref(), Some("1234"));
        assert_eq!(reused.observed_ms, 17);
        assert_eq!(next.count, 0);
        assert_eq!(next.fact(&root, &status).raw.as_deref(), Some("0"));
        assert_eq!(next.count, 1);
        let mut manual =
            Reads::fixture(BTreeMap::from([(root.join("idVendor"), b"9999".to_vec())]));
        assert_eq!(manual.fact(&root, &identity).raw.as_deref(), Some("9999"));
        assert_eq!(manual.count, 1);
    }

    #[test]
    fn failed_or_retained_identity_fields_do_not_seed_fresh_observations() {
        let (_temp, root, mut reads, mut previous) =
            registered_fixture("usb", "1-2", &[("idVendor", "1234")]);
        let rule = attributes::rule(&Context::discover(&root), "idVendor").unwrap();
        let identity = reads.fact(&root, &rule);
        for retained in [false, true] {
            let mut old = identity.clone();
            if retained {
                old.retained = true;
            } else {
                old.outcome = Outcome::PermissionDenied;
            }
            previous.facts = vec![old];
            let mut next =
                Reads::fixture(BTreeMap::from([(root.join("idVendor"), b"5678".to_vec())]));
            next.seed_stable(Some(&previous), &root);
            assert_eq!(next.fact(&root, &rule).raw.as_deref(), Some("5678"));
            assert_eq!(next.count, 1);
        }
    }

    #[test]
    fn fresh_attribute_observations_bypass_cache_without_resetting_read_budgets() {
        let path = std::path::PathBuf::from("/fixture/online");
        let mut reads = Reads::fixture(BTreeMap::from([(path.clone(), b"0".to_vec())]));
        assert_eq!(reads.read(&path).unwrap(), "0");
        reads
            .fixture
            .as_mut()
            .unwrap()
            .insert(path.clone(), b"0,7".to_vec());
        assert_eq!(reads.read(&path).unwrap(), "0");
        assert_eq!(reads.read_fresh(&path).unwrap(), "0,7");
        assert_eq!(reads.count, 2);
    }
    #[test]
    fn attribute_reader_rejects_devices_and_ordinary_files_before_content_open() {
        let temp = tempfile::tempdir().unwrap();
        let ordinary = temp.path().join("temperature");
        std::fs::write(&ordinary, b"42000").unwrap();
        let mut reader = Reads::new();
        for path in [
            ordinary.as_path(),
            Path::new("/dev/null"),
            Path::new("/proc/uptime"),
        ] {
            assert_eq!(
                reader.read(path).unwrap_err().kind(),
                io::ErrorKind::Unsupported
            );
        }
        assert_eq!(reader.count, 0);
        assert_eq!(reader.bytes, 0);
        assert_eq!(reader.probes, 3);
        assert!(reader.read(&ordinary).is_err());
        assert_eq!(
            reader.probes, 3,
            "repeat requests reuse the failure outcome"
        );
    }

    #[test]
    fn exhausted_budgets_reject_requests_before_probing_the_source() {
        let mut reader = Reads::new();
        reader.bytes = 1024 * 1024;
        assert_eq!(
            reader.read(Path::new("/dev/null")).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(reader.probes, 0);
        let mut reader = Reads::new();
        reader.count = 64;
        assert!(reader.read(Path::new("/proc/uptime")).is_err());
        assert_eq!(reader.probes, 0);
    }
}
