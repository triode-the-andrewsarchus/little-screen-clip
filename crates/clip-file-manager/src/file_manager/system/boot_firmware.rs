//! Boot firmware identity and interface presence; no tables or variable payloads.
use super::{
    Document, Relation,
    attributes::{self, Context},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome, Value},
};
use std::{
    io,
    path::{Path, PathBuf},
};

struct Locations {
    firmware: PathBuf,
    dmi: PathBuf,
}
impl Locations {
    fn current() -> Self {
        Self {
            firmware: "/sys/firmware".into(),
            dmi: "/sys/class/dmi/id".into(),
        }
    }
}

pub(super) fn same_object(path: &Path, reference: &Path) -> bool {
    let identity = |path: &Path| {
        std::fs::metadata(path)
            .map(|metadata| crate::file_manager::filesystem::identity(&metadata))
            .ok()
    };
    identity(path)
        .zip(identity(reference))
        .is_some_and(|(left, right)| left == right)
}

pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    collect_at(document, sys, &Locations::current(), reads, checkpoint);
}

/// Handles the known EFI attributes before the generic unknown-attribute policy.
pub(super) fn attribute(
    document: &mut Document,
    sys: &Path,
    requested: Option<&str>,
    reads: &mut Reads,
) -> bool {
    let locations = Locations::current();
    if matches!(
        document.kind,
        little_clip_core::kernel_interface::Kind::Kernel {
            filesystem: little_clip_core::kernel_interface::Filesystem::Efi
        }
    ) {
        variables(document);
        return true;
    }
    if same_object(sys, &locations.firmware.join("efi")) && requested == Some("fw_platform_size") {
        document.title = "EFI firmware ABI attribute".into();
        width(document, sys, reads);
        return true;
    }
    false
}

fn collect_at(
    document: &mut Document,
    sys: &Path,
    locations: &Locations,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    if buses::subsystem(sys).as_deref() == Some("dmi") {
        dmi(document, sys, sys, reads);
    } else if same_object(sys, &locations.firmware.join("dmi")) {
        dmi(document, sys, &locations.dmi, reads);
    } else if same_object(sys, &locations.firmware.join("efi")) {
        efi(document, sys, reads);
    } else if same_object(sys, &locations.firmware.join("efi/efivars")) {
        variables(document);
    } else if same_object(sys, &locations.firmware) {
        directory(document, sys, locations);
    } else {
        return;
    }
    document.facts.sort_by_key(|fact| fact.interface);
    checkpoint(document);
}

fn dmi(document: &mut Document, interface: &Path, root: &Path, reads: &mut Reads) {
    document.title = "DMI/SMBIOS machine identity".into();
    let context = Context::discover(root);
    if context.family != attributes::Family::Dmi || !context.direct {
        document.facts.push(Fact::failure(
            "DMI identity source",
            root.into(),
            io::Error::new(
                io::ErrorKind::Unsupported,
                "Registered DMI class association unavailable; payload not read",
            ),
        ));
        document.topology = Some(Topology::new(
            "Machine firmware identity",
            "DMI identity unavailable".into(),
        ));
        return;
    }
    for rule in attributes::rules(&context) {
        reads.observe(document, root, &rule);
    }
    let mut summary = String::new();
    for name in ["sys_vendor", "product_name", "bios_version"] {
        if let Ok(value) = reads.read(&root.join(name)) {
            if !value.is_empty() {
                if !summary.is_empty() {
                    summary.push_str(" · ");
                }
                summary.push_str(&value);
            }
        }
    }
    if summary.is_empty() {
        summary = "Machine identity not exposed".into();
    }
    let mut topology = Topology::new("Machine firmware identity", summary);
    let parent = buses::add(
        &mut topology,
        document,
        interface.into(),
        "DMI/SMBIOS interface".into(),
        None,
    );
    if !same_object(interface, root) {
        buses::add(
            &mut topology,
            document,
            root.into(),
            "Cached machine identity".into(),
            Some(parent),
        );
    }
    document.facts.push(Fact::text("DMI collection policy", "Cached identity only; serial numbers, UUIDs, asset tags and raw SMBIOS tables not requested", root.into()));
    document.topology = Some(topology);
}

fn width(document: &mut Document, root: &Path, reads: &mut Reads) -> Option<u32> {
    let source = root.join("fw_platform_size");
    let result = reads
        .read(&source)
        .and_then(|raw| match raw.parse::<u32>() {
            Ok(bits @ (32 | 64)) => Ok((raw, bits)),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "EFI ABI width must be 32 or 64 bits",
            )),
        });
    match result {
        Ok((raw, bits)) => {
            let mut fact = Fact::text("EFI firmware ABI width", format!("{bits}-bit"), source);
            fact.raw = Some(raw);
            fact.semantic = Some(Value::Integer(bits.into()));
            fact.origin = "documented sysfs attribute".into();
            document.facts.push(fact);
            Some(bits)
        }
        Err(error) => {
            let mut fact = Fact::failure("EFI firmware ABI width", source.clone(), error);
            fact.raw = reads.read(&source).ok();
            fact.origin = "documented sysfs attribute".into();
            document.facts.push(fact);
            None
        }
    }
}

fn efi(document: &mut Document, root: &Path, reads: &mut Reads) {
    document.title = "EFI firmware interfaces".into();
    document.facts.insert(
        0,
        Fact::text(
            "EFI presence",
            "EFI sysfs interface exposed in this namespace view",
            root.into(),
        ),
    );
    let bits = width(document, root, reads);
    let summary = bits
        .map(|bits| format!("EFI · {bits}-bit firmware ABI"))
        .unwrap_or_else(|| "EFI · ABI width unavailable".into());
    let mut topology = Topology::new("EFI interfaces", summary);
    let anchor = buses::add(
        &mut topology,
        document,
        root.into(),
        "EFI firmware".into(),
        None,
    );
    for (name, label) in [
        ("runtime", "Runtime-service table metadata"),
        ("runtime-map", "Runtime memory-map interface"),
        ("efivars", "EFI variable interface"),
        ("esrt", "EFI system-resource interface"),
    ] {
        let path = root.join(name);
        match std::fs::metadata(&path) {
            Ok(_) => {
                document.facts.push(Fact::text(
                    label,
                    "Interface exposed; payload not read",
                    path.clone(),
                ));
                buses::add(&mut topology, document, path, label.into(), Some(anchor));
            }
            Err(error) => document.facts.push(Fact::failure(label, path, error)),
        }
    }
    let mut fact = Fact::text(
        "EFI runtime-service availability",
        "Unverified; an exposed table address does not establish enabled services",
        root.join("runtime"),
    );
    fact.outcome = Outcome::NotRequested;
    document.facts.push(fact);
    document.facts.push(Fact::text(
        "EFI variable policy",
        "Variable names/payloads not scanned or indexed; no firmware service or write invoked",
        root.join("efivars"),
    ));
    document.topology = Some(topology);
}

fn variables(document: &mut Document) {
    let mounted = matches!(
        document.kind,
        little_clip_core::kernel_interface::Kind::Kernel {
            filesystem: little_clip_core::kernel_interface::Filesystem::Efi
        }
    );
    document.title = if document.interface_directory {
        if mounted {
            "EFI variable filesystem"
        } else {
            "EFI variable mountpoint (metadata only)"
        }
    } else {
        "EFI variable (metadata only)"
    }
    .into();
    let mut fact = Fact::text(
        "EFI variable policy",
        "Variable payload not requested, copied, hashed or indexed",
        document.path.clone(),
    );
    fact.outcome = Outcome::NotRequested;
    document.facts.insert(0, fact);
    document.facts.push(Fact::text(
        "EFI variable interface state",
        if mounted {
            "efivarfs observed in this namespace view"
        } else {
            "EFI mountpoint observed; efivarfs not observed at this path"
        },
        document.path.clone(),
    ));
}

fn directory(document: &mut Document, root: &Path, locations: &Locations) {
    document.title = "Kernel firmware interfaces".into();
    let mut topology = Topology::new(
        "Firmware interfaces",
        "Observed kernel firmware interfaces".into(),
    );
    let anchor = buses::add(
        &mut topology,
        document,
        root.into(),
        "Kernel firmware".into(),
        None,
    );
    for (name, label) in [
        ("dmi", "DMI/SMBIOS"),
        ("efi", "EFI"),
        ("acpi", "ACPI firmware"),
        ("devicetree", "Device-tree interface"),
        ("memmap", "Firmware memory-map interface"),
    ] {
        let path = root.join(name);
        match std::fs::metadata(&path) {
            Ok(_) => {
                buses::add(
                    &mut topology,
                    document,
                    path.clone(),
                    label.into(),
                    Some(anchor),
                );
                document.facts.push(Fact::text(
                    label,
                    "Interface exposed; payload not read",
                    path,
                ));
            }
            Err(error) => document.facts.push(Fact::failure(label, path, error)),
        }
    }
    if locations.dmi.exists() {
        document.relations.push(Relation::new(
            "Cached DMI machine identity",
            locations.dmi.clone(),
        ));
    }
    document.topology = Some(topology);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};

    #[test]
    fn dmi_identity_fields_exclude_serials_uuids_and_asset_tags() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("id");
        let subsystem = temp.path().join("dmi");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&subsystem).unwrap();
        symlink(&subsystem, root.join("subsystem")).unwrap();
        let locations = Locations {
            firmware: temp.path().join("firmware"),
            dmi: root.clone(),
        };
        let mut values = BTreeMap::from([
            (root.join("sys_vendor"), b"Example manufacturer".to_vec()),
            (root.join("product_name"), b"Example machine".to_vec()),
            (root.join("bios_version"), b"1.2.3".to_vec()),
            (root.join("board_name"), b"Example board".to_vec()),
        ]);
        for name in [
            "product_serial",
            "product_uuid",
            "board_serial",
            "board_asset_tag",
            "chassis_serial",
            "chassis_asset_tag",
            "modalias",
        ] {
            values.insert(root.join(name), b"private identity".to_vec());
        }
        let mut reads = Reads::fixture(values);
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect_at(&mut doc, &root, &locations, &mut reads, &mut |_| {});
        assert!(doc.topology.unwrap().summary.contains("Example machine"));
        assert!(
            doc.facts.iter().any(|fact| fact.label == "Board model"
                && fact.value.as_deref() == Some("Example board"))
        );
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(
                "product_serial"
                    | "product_uuid"
                    | "board_serial"
                    | "board_asset_tag"
                    | "chassis_serial"
                    | "chassis_asset_tag"
                    | "modalias"
            )
        )));
        let nested = root.join("private");
        std::fs::create_dir(&nested).unwrap();
        assert!(attributes::rules(&Context::discover(&nested)).is_empty());
    }

    #[test]
    fn efi_interfaces_are_metadata_only_except_the_bounded_abi_width() {
        let temp = tempfile::tempdir().unwrap();
        let firmware = temp.path().join("firmware");
        let root = firmware.join("efi");
        std::fs::create_dir_all(root.join("efivars")).unwrap();
        std::fs::create_dir(root.join("runtime-map")).unwrap();
        std::fs::write(root.join("runtime"), b"physical pointer not read").unwrap();
        let locations = Locations {
            firmware,
            dmi: temp.path().join("id"),
        };
        let mut reads = Reads::fixture(BTreeMap::from([
            (root.join("fw_platform_size"), b"64".to_vec()),
            (root.join("runtime"), b"not read".to_vec()),
            (
                root.join("efivars/SecureBoot-variable"),
                b"not read".to_vec(),
            ),
        ]));
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect_at(&mut doc, &root, &locations, &mut reads, &mut |_| {});
        assert!(doc.topology.as_ref().unwrap().summary.contains("64-bit"));
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "EFI firmware ABI width"
                    && fact.raw.as_deref() == Some("64")
                    && fact.semantic == Some(Value::Integer(64)))
        );
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "EFI runtime-service availability"
                    && fact.outcome == Outcome::NotRequested)
        );
        assert!(
            doc.relations
                .iter()
                .any(|relation| relation.target == root.join("efivars"))
        );
        assert_eq!(
            reads.attempts().collect::<Vec<_>>(),
            vec![&root.join("fw_platform_size")]
        );
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "EFI system-resource interface"
                    && fact.outcome == Outcome::Missing)
        );
    }

    #[test]
    fn firmware_aliases_match_object_identity_and_invalid_widths_fail_explicitly() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("firmware");
        std::fs::create_dir(&root).unwrap();
        let alias = temp.path().join("alias");
        symlink(&root, &alias).unwrap();
        assert!(same_object(&root, &alias));
        let unrelated = temp.path().join("unrelated");
        std::fs::create_dir(&unrelated).unwrap();
        assert!(!same_object(&root, &unrelated));
        assert!(!same_object(&root, &root.join("missing")));
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        let mut reads = Reads::fixture(BTreeMap::from([(
            root.join("fw_platform_size"),
            b"128".to_vec(),
        )]));
        assert_eq!(width(&mut doc, &root, &mut reads), None);
        assert!(doc.facts.iter().any(
            |fact| fact.label == "EFI firmware ABI width" && fact.outcome == Outcome::Malformed
        ));
        let locations = Locations {
            firmware: root.clone(),
            dmi: root.join("id"),
        };
        collect_at(&mut doc, &alias, &locations, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "Kernel firmware interfaces");
    }

    #[test]
    fn efivar_payloads_and_unregistered_dmi_sources_remain_unrequested() {
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        doc.kind = little_clip_core::kernel_interface::Kind::Kernel {
            filesystem: little_clip_core::kernel_interface::Filesystem::Efi,
        };
        let mut reads = Reads::fixture(BTreeMap::new());
        assert!(attribute(
            &mut doc,
            Path::new("/unrelated"),
            Some("variable"),
            &mut reads
        ));
        assert_eq!(doc.title, "EFI variable (metadata only)");
        assert_eq!(reads.attempts().count(), 0);
        let temp = tempfile::tempdir().unwrap();
        dmi(&mut doc, temp.path(), temp.path(), &mut reads);
        assert!(doc.facts.iter().any(
            |fact| fact.label == "DMI identity source" && fact.outcome == Outcome::Unsupported
        ));
        assert_eq!(reads.attempts().count(), 0);
    }
}
