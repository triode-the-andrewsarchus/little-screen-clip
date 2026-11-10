//! Directory grouping uses registered subsystem identities, never access-node names alone.
use super::{
    Document, Filesystem, Kind, ObjectContext, Relation, Topology, buses, finish, inventory,
    unavailable,
};
use crate::file_manager::system::{collector::Reads, cpu, drm, input, sound, storage, vfio};
use std::{
    collections::BTreeMap,
    io,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Family {
    Display,
    Sound,
    Input,
    Cpu,
    Usb,
    Vfio,
    Mapper,
}
impl Family {
    fn heading(self) -> &'static str {
        match self {
            Self::Display => "GPU access interfaces",
            Self::Sound => "Sound-card interfaces",
            Self::Input => "Input-device interfaces",
            Self::Cpu => "CPU access interfaces",
            Self::Usb => "Raw USB bus interfaces",
            Self::Vfio => "VFIO access interfaces",
            Self::Mapper => "Device-mapper interfaces",
        }
    }
    fn objects(self) -> &'static str {
        match self {
            Self::Display => "GPUs/display controllers",
            Self::Sound => "sound cards",
            Self::Input => "input-device groups",
            Self::Cpu => "logical CPUs",
            Self::Usb => "USB buses",
            Self::Vfio => "IOMMU groups",
            Self::Mapper => "mapped block devices",
        }
    }
    fn registration(self, sys: &Path, kind: Kind) -> Option<(Option<PathBuf>, String)> {
        match self {
            Self::Display => drm::directory_group(sys),
            Self::Sound => sound::directory_group(sys),
            Self::Input => input::directory_group(sys),
            Self::Cpu => cpu::directory_group(sys, kind),
            Self::Usb => buses::directory_group(sys),
            Self::Vfio => vfio::directory_group(sys),
            Self::Mapper => storage::directory_group(sys, kind),
        }
    }
}

pub(super) fn collect(document: &mut Document, family: Family) {
    let depth = usize::from(matches!(family, Family::Cpu | Family::Usb | Family::Vfio));
    let (paths, coverage) = inventory(document, depth);
    let mut topology = Topology::new(family.heading(), String::new());
    topology.coverage = coverage;
    let mut groups = BTreeMap::new();
    let mut reads = Reads::new();
    let mut interfaces = 0;
    for path in paths {
        let Some(child) = interface(document, &mut topology, &path, family) else {
            continue;
        };
        interfaces += usize::from(matches!(
            child.kind,
            Kind::Character { .. } | Kind::Block { .. }
        ));
        let cpu_directory = cpu_directory(&child, family);
        let sys = cpu_directory.as_deref().or_else(|| {
            child.shared_object().filter(|_| {
                child.kernel_context.is_some_and(|context| {
                    context.kind
                        == (Kind::Kernel {
                            filesystem: Filesystem::Sysfs,
                        })
                })
            })
        });
        let registration = sys.and_then(|sys| family.registration(sys, child.kind));
        add_registration(
            document,
            &mut topology,
            &mut groups,
            family,
            &path,
            &child,
            registration,
            &mut reads,
        );
    }
    let ungrouped = topology
        .nodes
        .iter()
        .filter(|node| node.parent.is_none())
        .count()
        .saturating_sub(groups.len());
    topology.summary = format!(
        "{interfaces} observed interfaces · {} observed {} · {ungrouped} ungrouped/global entries",
        groups.len(),
        family.objects()
    );
    if family == Family::Display {
        topology.summary.push_str(&format!(
            " · {} observed connected outputs",
            drm::connected_outputs(&topology)
        ));
    }
    if family == Family::Cpu {
        let count = |major: u32| {
            document
                .relations
                .iter()
                .filter(|relation| relation.context.is_some_and(|context| matches!(context.kind, Kind::Character { major: observed, .. } if observed == major)))
                .count()
        };
        topology.summary.push_str(&format!(
            " · {} CPUID nodes · {} MSR nodes (not opened)",
            count(203),
            count(202)
        ));
    }
    finish(document, topology, family.heading(), Some(interfaces));
}

fn cpu_directory(child: &Document, family: Family) -> Option<PathBuf> {
    if family != Family::Cpu
        || !child.interface_directory
        || child.kind
            != (Kind::Kernel {
                filesystem: Filesystem::DeviceNodes,
            })
    {
        return None;
    }
    let name = child.path.file_name()?.to_str()?;
    if !buses::numbered(name, "") {
        return None;
    }
    let sys = Path::new("/sys/devices/system/cpu").join(format!("cpu{name}"));
    let pinned = little_clip_core::kernel_interface::pin(&sys, true).ok()?;
    (ObjectContext::pinned(&pinned).ok()?.kind
        == (Kind::Kernel {
            filesystem: Filesystem::Sysfs,
        }))
    .then_some(sys)
}

fn interface(
    document: &mut Document,
    topology: &mut Topology,
    path: &Path,
    family: Family,
) -> Option<Document> {
    match std::fs::metadata(path) {
        Ok(metadata)
            if !metadata.file_type().is_char_device()
                && !(family == Family::Mapper && metadata.file_type().is_block_device())
                && !(family == Family::Cpu && metadata.is_dir()) =>
        {
            return None;
        }
        Ok(_) => {}
        Err(error) => {
            unavailable(document, topology, path.into(), error);
            return None;
        }
    }
    match crate::file_manager::system::worker::interface(path) {
        Ok(child)
            if matches!(child.kind, Kind::Character { .. })
                || (family == Family::Mapper && matches!(child.kind, Kind::Block { .. }))
                || (family == Family::Cpu
                    && child.interface_directory
                    && child.kind
                        == (Kind::Kernel {
                            filesystem: Filesystem::DeviceNodes,
                        })) =>
        {
            Some(child)
        }
        Ok(_) => {
            unavailable(
                document,
                topology,
                path.into(),
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Interface changed type during observation",
                ),
            );
            None
        }
        Err(error) => {
            unavailable(document, topology, path.into(), io::Error::other(error));
            None
        }
    }
}

fn add_registration(
    document: &mut Document,
    topology: &mut Topology,
    groups: &mut BTreeMap<ObjectContext, usize>,
    family: Family,
    path: &Path,
    child: &Document,
    registration: Option<(Option<PathBuf>, String)>,
    reads: &mut Reads,
) {
    let usb_identity = family == Family::Usb && registration.is_some();
    let (group, mut role) = registration.unwrap_or_else(|| {
        unavailable(
            document,
            topology,
            path.into(),
            io::Error::new(
                io::ErrorKind::NotFound,
                "Matching subsystem registration unavailable",
            ),
        );
        (None, "Interface; registration unavailable".into())
    });
    if usb_identity && let Some(sys) = child.shared_object() {
        role.push_str(&format!(
            " · {}",
            buses::directory_identity(document, sys, reads, topology)
        ));
    }
    let parent =
        group.and_then(|group| group_node(document, topology, groups, family, group, reads));
    let label = format!(
        "{role} · {}",
        crate::file_manager::display_path(path.file_name().map(Path::new).unwrap_or(path))
    );
    buses::add_relation(
        topology,
        document,
        Relation::observed_interface(label, path.into(), child),
        parent,
    );
}

fn group_node(
    document: &mut Document,
    topology: &mut Topology,
    groups: &mut BTreeMap<ObjectContext, usize>,
    family: Family,
    group: PathBuf,
    reads: &mut Reads,
) -> Option<usize> {
    let label = format!(
        "{} · {}",
        family.objects(),
        group.file_name().unwrap_or_default().to_string_lossy()
    );
    let observed_context = match little_clip_core::kernel_interface::pin(&group, true)
        .and_then(|file| ObjectContext::pinned(&file))
    {
        Ok(context) => context,
        Err(error) => {
            unavailable(document, topology, group, error);
            return None;
        }
    };
    let relation = Relation::new(label, group.clone());
    let context = match relation
        .context
        .filter(|context| relation.directory && *context == observed_context)
    {
        Some(context) => context,
        None => {
            unavailable(
                document,
                topology,
                group,
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Registered hardware group changed, is not a directory, or has unavailable mount identity",
                ),
            );
            return None;
        }
    };
    if let Some(index) = groups.get(&context) {
        return Some(*index);
    }
    let parent = buses::add_relation(topology, document, relation, None);
    groups.insert(context, parent);
    match family {
        Family::Display => {
            drm::primary_cards(document, &group, reads, topology, Some(parent), false)
        }
        Family::Input => input::related_into(document, &group, &group, topology, parent),
        Family::Vfio => vfio::members_into(document, &group, reads, &mut |_| {}, topology, parent),
        Family::Mapper => storage::directory_dependencies(document, &group, topology, parent),
        _ => {}
    }
    Some(parent)
}

#[cfg(test)]
mod tests {
    use super::super::{Outcome, tests::document};
    use super::*;
    use crate::file_manager::system::{collector::registered_fixture, worker};
    use std::os::unix::fs::symlink;

    #[test]
    fn nested_vfio_cdev_paths_remain_visible_without_inventing_registration() {
        let temp = tempfile::tempdir().unwrap();
        let devices = temp.path().join("devices");
        std::fs::create_dir(&devices).unwrap();
        let access = devices.join("vfio99");
        symlink("/dev/null", &access).unwrap();
        let mut doc = document(temp.path());
        collect(&mut doc, Family::Vfio);
        let topology = doc.topology.unwrap();
        assert_eq!(topology.nodes.len(), 1);
        assert_eq!(topology.nodes[0].path, access);
        assert!(topology.summary.contains("1 observed interfaces"));
        assert!(topology.summary.contains("0 observed IOMMU groups"));
        assert_eq!(topology.coverage, Outcome::Missing);
    }

    fn grouped_fixture(
        root: &Path,
        family: Family,
        registration: (Option<PathBuf>, String),
        mut reads: Reads,
    ) -> (Document, Topology, Reads) {
        let mut doc = document(root);
        let mut topology = Topology::new(family.heading(), String::new());
        let mut groups = BTreeMap::new();
        let child = worker::interface(Path::new("/dev/null")).unwrap();
        add_registration(
            &mut doc,
            &mut topology,
            &mut groups,
            family,
            Path::new("/dev/null"),
            &child,
            Some(registration),
            &mut reads,
        );
        assert_eq!(groups.len(), 1);
        (doc, topology, reads)
    }

    #[test]
    fn vfio_directory_reuses_membership_and_never_reads_pci_payloads() {
        let (temp, device, _, _) = registered_fixture("pci", "0000:01:00.0", &[]);
        let group = temp.path().join("groups/12");
        std::fs::create_dir_all(group.join("devices")).unwrap();
        symlink(&device, group.join("devices/0000:01:00.0")).unwrap();
        let reads = Reads::fixture(BTreeMap::from([
            (device.join("vendor"), b"0x1234".to_vec()),
            (device.join("device"), b"0xabcd".to_vec()),
            (device.join("class"), b"0x020000".to_vec()),
        ]));
        let (mut doc, topology, mut reads) = grouped_fixture(
            temp.path(),
            Family::Vfio,
            (Some(group.clone()), "VFIO/IOMMU group".into()),
            reads,
        );
        let member = topology
            .nodes
            .iter()
            .find(|node| node.path == device)
            .unwrap();
        assert_eq!(member.parent, Some(0));
        assert!(member.label.contains("0000:01:00.0"));
        assert!(doc.facts.iter().any(|fact| {
            fact.value
                .as_deref()
                .is_some_and(|value| value.contains("do not prove VM assignment"))
        }));
        assert_eq!(reads.attempts().count(), 3);
        assert!(reads.attempts().all(|path| {
            path.file_name()
                .is_some_and(|name| name == "vendor" || name == "device" || name == "class")
        }));
        for number in 0..40 {
            symlink(
                &device,
                group.join("devices").join(format!("member{number:02}")),
            )
            .unwrap();
        }
        symlink(temp.path().join("missing"), group.join("devices/00-broken")).unwrap();
        let mut partial = Topology::new("partial", String::new());
        vfio::members_into(&mut doc, &group, &mut reads, &mut |_| {}, &mut partial, 0);
        assert_eq!(partial.coverage, Outcome::Truncated);
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "IOMMU member" && fact.outcome == Outcome::Missing)
        );
    }

    #[test]
    fn mapper_directory_requires_block_registration_and_shares_bounded_dependencies() {
        let (temp, mapped, _, _) = registered_fixture("block", "dm-0", &[]);
        let class = temp.path().join("class/block");
        let backing = temp.path().join("disk0");
        std::fs::create_dir(&backing).unwrap();
        symlink(class, backing.join("subsystem")).unwrap();
        std::fs::create_dir(mapped.join("slaves")).unwrap();
        symlink(&backing, mapped.join("slaves/disk0")).unwrap();
        let registration = storage::directory_group(
            &mapped,
            Kind::Block {
                major: 253,
                minor: 0,
            },
        )
        .unwrap();
        assert_eq!(registration.0.as_ref(), Some(&mapped));
        assert!(
            storage::directory_group(&mapped, Kind::Character { major: 1, minor: 3 }).is_none()
        );
        let (mut doc, topology, reads) = grouped_fixture(
            temp.path(),
            Family::Mapper,
            registration,
            Reads::fixture(BTreeMap::new()),
        );
        assert_eq!(
            topology
                .nodes
                .iter()
                .find(|node| node.path == backing)
                .unwrap()
                .parent,
            Some(0)
        );
        assert!(
            doc.relations
                .iter()
                .any(|relation| relation.target == backing)
        );
        assert_eq!(reads.attempts().count(), 0);
        symlink(temp.path().join("missing"), mapped.join("slaves/broken")).unwrap();
        let mut partial = Topology::new("missing", String::new());
        storage::directory_dependencies(&mut doc, &mapped, &mut partial, 0);
        assert_eq!(partial.coverage, Outcome::Missing);
        std::fs::remove_file(mapped.join("slaves/broken")).unwrap();
        let ordinary = temp.path().join("ordinary");
        std::fs::write(&ordinary, b"not a registered block object").unwrap();
        symlink(&ordinary, mapped.join("slaves/not-block")).unwrap();
        let mut invalid = Topology::new("invalid", String::new());
        storage::directory_dependencies(&mut doc, &mapped, &mut invalid, 0);
        assert_eq!(invalid.coverage, Outcome::Unsupported);
        std::fs::remove_file(mapped.join("slaves/not-block")).unwrap();
        for number in 0..40 {
            symlink(
                &backing,
                mapped.join("slaves").join(format!("disk{number:02}")),
            )
            .unwrap();
        }
        let mut truncated = Topology::new("truncated", String::new());
        storage::directory_dependencies(&mut doc, &mapped, &mut truncated, 0);
        assert_eq!(truncated.coverage, Outcome::Truncated);
        assert_eq!(truncated.nodes.len(), 32);
    }

    #[test]
    fn mapper_control_is_global_and_unknown_names_do_not_invent_volumes() {
        let (_temp, control, _, _) = registered_fixture("misc", "device-mapper", &[]);
        assert_eq!(
            storage::directory_group(
                &control,
                Kind::Character {
                    major: 10,
                    minor: 236
                }
            )
            .unwrap(),
            (None, "Device-mapper control interface (unopened)".into())
        );
        assert!(storage::directory_group(&control, Kind::Block { major: 8, minor: 0 }).is_none());
        let temp = tempfile::tempdir().unwrap();
        symlink("/dev/null", temp.path().join("fake-volume")).unwrap();
        let mut doc = document(temp.path());
        collect(&mut doc, Family::Mapper);
        assert!(
            doc.topology
                .as_ref()
                .unwrap()
                .summary
                .contains("0 observed mapped block devices")
        );
        assert_eq!(doc.topology.as_ref().unwrap().coverage, Outcome::Missing);
    }

    #[test]
    fn cpu_directory_roles_use_device_identity_and_validate_the_logical_cpu_number() {
        let (temp, original, _, _) = registered_fixture("cpu", "cpu7", &[]);
        let cpus = temp.path().join("cpu");
        std::fs::create_dir(&cpus).unwrap();
        let sys = cpus.join("cpu7");
        std::fs::rename(original, &sys).unwrap();
        for (major, expected) in [
            (202, "MSR interface (unopened)"),
            (203, "CPUID interface (unopened)"),
        ] {
            let registration =
                cpu::directory_group(&sys, Kind::Character { major, minor: 7 }).unwrap();
            assert_eq!(registration, (Some(sys.clone()), expected.into()));
            assert!(cpu::directory_group(&sys, Kind::Character { major, minor: 8 }).is_none());
        }
        assert!(cpu::directory_group(&sys, Kind::Character { major: 1, minor: 7 }).is_none());
        assert!(cpu::directory_group(&sys, Kind::Content).is_none());
        assert_eq!(
            cpu::directory_group(
                &sys,
                Kind::Kernel {
                    filesystem: Filesystem::DeviceNodes
                }
            )
            .unwrap()
            .1,
            "Logical CPU directory"
        );
        let mut ordinary = document(sys.parent().unwrap());
        ordinary.path = sys.parent().unwrap().join("0");
        assert!(cpu_directory(&ordinary, Family::Cpu).is_none());
    }

    #[test]
    fn usb_directory_groups_devices_by_registered_bus_and_limits_identity_reads() {
        let temp = tempfile::tempdir().unwrap();
        let class = temp.path().join("bus/usb");
        std::fs::create_dir_all(&class).unwrap();
        let mut doc = document(temp.path());
        let mut topology = Topology::new("USB directories", String::new());
        let mut groups = BTreeMap::new();
        let mut values = BTreeMap::new();
        let mut devices = Vec::new();
        for (bus, name, product) in [
            ("usb1", "1-2", "0001"),
            ("usb1", "1-3", "0002"),
            ("usb2", "2-1", "0003"),
        ] {
            let root = temp.path().join(bus);
            if !root.exists() {
                std::fs::create_dir(&root).unwrap();
                symlink(&class, root.join("subsystem")).unwrap();
            }
            let sys = root.join(name);
            std::fs::create_dir(&sys).unwrap();
            symlink(&class, sys.join("subsystem")).unwrap();
            std::fs::write(sys.join("idVendor"), "1234").unwrap();
            values.insert(sys.join("idVendor"), b"1234".to_vec());
            values.insert(sys.join("idProduct"), product.as_bytes().to_vec());
            devices.push((root, sys));
        }
        let mut reads = Reads::fixture(values);
        for (index, (root, sys)) in devices.iter().enumerate() {
            let mut child = worker::interface(Path::new("/dev/null")).unwrap();
            child.relations = vec![Relation::new("Kernel object", sys.clone())];
            child.kernel_identity = Some(vec![index as u64]);
            let registration = Family::Usb
                .registration(
                    sys,
                    Kind::Character {
                        major: 189,
                        minor: index as u32,
                    },
                )
                .unwrap();
            assert_eq!(registration.0.as_ref(), Some(root));
            add_registration(
                &mut doc,
                &mut topology,
                &mut groups,
                Family::Usb,
                Path::new("/dev/null"),
                &child,
                Some(registration),
                &mut reads,
            );
        }
        assert_eq!(groups.len(), 2);
        assert_eq!(topology.nodes.len(), 5);
        assert_eq!(topology.coverage, Outcome::Observed);
        assert!(
            topology
                .nodes
                .iter()
                .any(|node| node.label.contains("USB device 1-2 · 1234:0001"))
        );
        assert_eq!(reads.attempts().count(), 6);
        assert!(reads.attempts().all(|path| {
            path.file_name()
                .is_some_and(|name| name == "idVendor" || name == "idProduct")
        }));
    }

    #[test]
    fn missing_and_wrong_type_groups_preserve_their_distinct_outcomes() {
        let temp = tempfile::tempdir().unwrap();
        let mut doc = document(temp.path());
        let mut topology = Topology::new("test", String::new());
        let mut groups = BTreeMap::new();
        let mut reads = Reads::fixture(BTreeMap::new());
        let missing = temp.path().join("missing");
        assert!(
            group_node(
                &mut doc,
                &mut topology,
                &mut groups,
                Family::Sound,
                missing.clone(),
                &mut reads
            )
            .is_none()
        );
        assert_eq!(doc.facts.last().unwrap().source, missing);
        assert_eq!(doc.facts.last().unwrap().outcome, Outcome::Missing);
        let regular = temp.path().join("regular");
        std::fs::write(&regular, b"not a directory").unwrap();
        assert!(
            group_node(
                &mut doc,
                &mut topology,
                &mut groups,
                Family::Sound,
                regular.clone(),
                &mut reads
            )
            .is_none()
        );
        assert_eq!(doc.facts.last().unwrap().source, regular);
        assert_eq!(doc.facts.last().unwrap().outcome, Outcome::Unsupported);
        assert!(groups.is_empty());
        assert!(topology.nodes.is_empty());
        assert_eq!(reads.attempts().count(), 0);
    }

    #[test]
    fn sound_directory_registration_preserves_card_membership_and_interface_roles() {
        let (temp, card, _, _) = registered_fixture("sound", "card2", &[]);
        let class = temp.path().join("class/sound");
        for (name, expected) in [
            ("pcmC2D0p", "PCM playback"),
            ("pcmC2D0c", "PCM capture"),
            ("midiC2D0", "Raw MIDI"),
            ("controlC2", "Control"),
        ] {
            let sys = card.join(name);
            std::fs::create_dir(&sys).unwrap();
            symlink(&class, sys.join("subsystem")).unwrap();
            assert_eq!(
                sound::directory_group(&sys),
                Some((Some(card.clone()), expected.into()))
            );
        }
        let wrong = card.join("controlC3");
        std::fs::create_dir(&wrong).unwrap();
        symlink(&class, wrong.join("subsystem")).unwrap();
        assert_eq!(sound::directory_group(&wrong).unwrap().0, None);
        let global = temp.path().join("seq");
        std::fs::create_dir(&global).unwrap();
        symlink(class, global.join("subsystem")).unwrap();
        assert_eq!(
            sound::directory_group(&global),
            Some((None, "Sequencer".into()))
        );
    }

    #[test]
    fn input_directory_registration_shares_peers_but_keeps_sibling_input_devices_separate() {
        let (temp, first, _, _) = registered_fixture("input", "input0", &[]);
        let class = temp.path().join("class/input");
        let second = temp.path().join("input1");
        std::fs::create_dir(&second).unwrap();
        symlink(&class, second.join("subsystem")).unwrap();
        for (parent, name, role) in [
            (&first, "event0", "Event interface"),
            (&first, "mouse0", "Mouse interface"),
            (&second, "js0", "Joystick interface"),
        ] {
            let sys = parent.join(name);
            std::fs::create_dir(&sys).unwrap();
            symlink(&class, sys.join("subsystem")).unwrap();
            assert_eq!(
                input::directory_group(&sys),
                Some((Some(parent.clone()), role.into()))
            );
        }
        let aggregate = temp.path().join("mice");
        std::fs::create_dir(&aggregate).unwrap();
        symlink(class, aggregate.join("subsystem")).unwrap();
        assert_eq!(
            input::directory_group(&aggregate),
            Some((None, "Aggregate mouse interface".into()))
        );
    }

    #[test]
    fn display_directory_groups_primary_render_and_reads_only_connector_status() {
        let (temp, card, _, _) = registered_fixture("drm", "card0", &[]);
        let class = temp.path().join("class/drm");
        let gpu = temp.path().join("gpu");
        std::fs::create_dir_all(gpu.join("drm")).unwrap();
        let primary = gpu.join("drm/card0");
        std::fs::rename(card, &primary).unwrap();
        symlink(&gpu, primary.join("device")).unwrap();
        let render = gpu.join("drm/renderD128");
        std::fs::create_dir(&render).unwrap();
        symlink(&class, render.join("subsystem")).unwrap();
        symlink(&gpu, render.join("device")).unwrap();
        let connector = primary.join("card0-DP-1");
        std::fs::create_dir(&connector).unwrap();
        symlink(class, connector.join("subsystem")).unwrap();
        let status = connector.join("status");
        let mut reads = Reads::fixture(BTreeMap::from([(status.clone(), b"connected".to_vec())]));
        let mut doc = document(temp.path());
        let mut topology = Topology::new("test", String::new());
        let mut groups = BTreeMap::new();
        for (node, sys) in [("/dev/null", &primary), ("/dev/zero", &render)] {
            let child = worker::interface(Path::new(node)).unwrap();
            let registration = Family::Display.registration(sys, child.kind);
            assert_eq!(registration.as_ref().unwrap().0.as_ref(), Some(&gpu));
            add_registration(
                &mut doc,
                &mut topology,
                &mut groups,
                Family::Display,
                Path::new(node),
                &child,
                registration,
                &mut reads,
            );
        }
        assert_eq!(groups.len(), 1);
        assert_eq!(drm::connected_outputs(&topology), 1);
        assert_eq!(topology.coverage, Outcome::Observed);
        assert_eq!(reads.attempts().collect::<Vec<_>>(), vec![status.as_path()]);
        let primary = topology
            .nodes
            .iter()
            .find(|node| node.label.starts_with("DRM primary access node"))
            .unwrap();
        let render = topology
            .nodes
            .iter()
            .find(|node| node.label.starts_with("DRM render access node"))
            .unwrap();
        assert_eq!(primary.parent, render.parent);
        let mut missing = Reads::fixture(BTreeMap::new());
        let mut missing_topology = Topology::new("missing", String::new());
        drm::primary_cards(
            &mut doc,
            &gpu,
            &mut missing,
            &mut missing_topology,
            None,
            false,
        );
        assert_eq!(missing_topology.coverage, Outcome::Missing);
        assert_eq!(drm::connected_outputs(&missing_topology), 0);
        assert!(missing.attempts().all(|path| path == status));
    }

    #[test]
    fn device_names_cannot_fabricate_sound_registration_or_hardware_groups() {
        let temp = tempfile::tempdir().unwrap();
        symlink("/dev/null", temp.path().join("pcmC99D0p")).unwrap();
        let mut doc = document(temp.path());
        collect(&mut doc, Family::Sound);
        let topology = doc.topology.unwrap();
        assert_eq!(topology.coverage, Outcome::Missing);
        assert_eq!(topology.nodes.len(), 1);
        assert!(topology.nodes[0].parent.is_none());
        assert!(topology.summary.contains("0 observed sound cards"));
        assert!(topology.summary.contains("Not exposed"));
        assert!(topology.nodes[0].label.contains("registration unavailable"));
    }
}
