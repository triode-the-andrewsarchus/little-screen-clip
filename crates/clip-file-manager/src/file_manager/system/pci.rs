//! PCI address-space metadata and topology. Never reads config, BARs or ROM.
use super::{
    Document,
    buses::{self, Topology},
    collector::Reads,
    model::Fact,
};
use std::{
    io,
    path::{Path, PathBuf},
};

pub(super) fn collect(document: &mut Document, sys: &Path, reads: &mut Reads) {
    if buses::subsystem(sys).as_deref() != Some("pci") {
        return;
    }
    let mut topology = Topology::new("PCI/PCIe topology", String::new());
    let mut chain = sys
        .ancestors()
        .take(12)
        .filter(|path| buses::subsystem(path).as_deref() == Some("pci"))
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    chain.reverse();
    let parent = buses::ancestor_chain(&mut topology, document, chain, None);
    let name = sys.file_name().unwrap_or_default().to_string_lossy();
    let driver = buses::driver(sys).unwrap_or_else(|| "binding unavailable".into());
    let link = match (
        reads.read(&sys.join("current_link_speed")),
        reads.read(&sys.join("current_link_width")),
    ) {
        (Ok(speed), Ok(width)) => link_summary(&speed, &width).unwrap_or_default(),
        _ => String::new(),
    };
    topology.summary = format!("{name} · {driver}{link}");
    document
        .facts
        .insert(0, Fact::text("PCI address", name.to_string(), sys.into()));
    match address(&name) {
        Ok((domain, bus, device, function)) => {
            for (label, value) in [
                ("PCI domain", format!("0x{domain:04x}")),
                ("PCI bus", format!("0x{bus:02x}")),
                ("PCI device number", format!("0x{device:02x}")),
                ("PCI function", function.to_string()),
            ] {
                document.facts.push(Fact::text(label, value, sys.into()));
            }
        }
        Err(error) => {
            document
                .facts
                .push(Fact::failure("PCI address components", sys.into(), error))
        }
    }
    if let Ok(raw) = reads.read(&sys.join("resource")) {
        match resources(&raw) {
            Ok(values) => {
                for value in values {
                    document.facts.push(Fact::text(
                        "Resource/BAR metadata",
                        value,
                        sys.join("resource"),
                    ));
                }
            }
            Err(error) => document.facts.push(Fact::failure(
                "Resource/BAR metadata",
                sys.join("resource"),
                error,
            )),
        }
    }
    if let Some(group) = iommu(document, sys, reads) {
        topology.summary.push_str(&format!(
            " · IOMMU {}",
            group.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    children(document, sys, &mut topology, parent);
    document.title = "PCI/PCIe device".into();
    document.topology = Some(topology);
}
pub(super) fn iommu(document: &mut Document, sys: &Path, reads: &mut Reads) -> Option<PathBuf> {
    let group = sys.join("iommu_group").canonicalize().ok()?;
    // The PCI inspector and standalone VFIO/group inspector share one collector.
    super::vfio::group(document, &group, reads, &mut |_| {});
    Some(group)
}

fn children(document: &mut Document, root: &Path, topology: &mut Topology, parent: Option<usize>) {
    let (paths, coverage) = buses::walk(root, 2, 128);
    topology.coverage = coverage;
    for path in paths.into_iter().filter(|path| path != root) {
        let Some(subsystem) = buses::subsystem(&path) else {
            continue;
        };
        if !matches!(
            subsystem.as_str(),
            "drm" | "sound" | "net" | "block" | "nvme" | "usb"
        ) {
            continue;
        }
        let label = format!(
            "{subsystem} · {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        buses::add(topology, document, path.clone(), label, parent);
        buses::access_node(document, &path, "PCI access interface");
    }
}
fn address(raw: &str) -> io::Result<(u32, u8, u8, u8)> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid PCI domain/bus/device/function address",
        )
    };
    let (location, function) = raw.rsplit_once('.').ok_or_else(invalid)?;
    let mut parts = location.split(':').collect::<Vec<_>>();
    parts.push(function);
    if parts.len() != 4
        || !(4..=8).contains(&parts[0].len())
        || parts[1].len() != 2
        || parts[2].len() != 2
        || parts[3].len() != 1
        || !parts
            .iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(invalid());
    }
    let number = |value: &str| u32::from_str_radix(value, 16).map_err(|_| invalid());
    let domain = number(parts[0])?;
    let bus = number(parts[1])?;
    let device = number(parts[2])?;
    let function = number(parts[3])?;
    if device > 31 || function > 7 {
        return Err(invalid());
    }
    Ok((domain, bus as u8, device as u8, function as u8))
}
fn link_summary(speed: &str, width: &str) -> Option<String> {
    let width = width.parse::<u32>().ok()?;
    if width == 0 || speed.trim().is_empty() || speed.starts_with("Unknown") {
        return None;
    }
    Some(format!(" · {speed} ×{width}"))
}

fn resources(raw: &str) -> io::Result<Vec<String>> {
    let mut resources = vec![];
    for (index, line) in raw.lines().enumerate() {
        if index >= 32 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "PCI resource metadata exceeds 32 entries",
            ));
        }
        let fields = line.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Malformed PCI resource metadata",
            ));
        }
        let hex = |value: &str| {
            u64::from_str_radix(value.trim_start_matches("0x"), 16)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid resource range"))
        };
        let start = hex(fields[0])?;
        let end = hex(fields[1])?;
        let flags = hex(fields[2])?;
        if start == 0 && end == 0 && flags == 0 {
            continue;
        }
        let size = end
            .checked_sub(start)
            .and_then(|size| size.checked_add(1))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid resource extent"))?;
        let name = if index < 6 {
            format!("BAR{index}")
        } else if index == 6 {
            "Expansion ROM allocation".into()
        } else {
            format!("Resource {index}")
        };
        let kind = match flags & 0x1f00 {
            0x100 => "I/O ports",
            0x200 => "memory",
            _ => "other resource",
        };
        resources.push(format!(
            "{name}: {kind} · 0x{start:x}–0x{end:x} · {} · flags 0x{flags:x}",
            crate::file_manager::filesystem::format_size(size)
        ));
    }
    Ok(resources)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pci_address_components_validate_ranges_and_extended_domains() {
        assert_eq!(address("0000:01:1f.7").unwrap(), (0, 1, 31, 7));
        assert_eq!(
            address("12345678:ff:00.0").unwrap(),
            (0x12345678, 255, 0, 0)
        );
        for raw in [
            "01:00.0",
            "0000:01:20.0",
            "0000:01:00.8",
            "0000:001:00.0",
            "-000:01:00.0",
            "0000:01:00.0.1",
            "0000.01:00:0",
        ] {
            assert!(address(raw).is_err(), "{raw}");
        }
    }
    #[test]
    fn unvalidated_or_absent_link_width_is_not_a_negotiated_summary() {
        assert_eq!(
            link_summary("16.0 GT/s PCIe", "16").as_deref(),
            Some(" · 16.0 GT/s PCIe ×16")
        );
        for width in ["0", "-1", "bad", "4294967296"] {
            assert!(link_summary("16.0 GT/s PCIe", width).is_none());
        }
        assert!(link_summary("Unknown speed", "16").is_none());
        assert!(link_summary("", "16").is_none());
    }

    #[test]
    fn resource_metadata_is_checked_without_accessing_the_resource() {
        let values = resources("0x1000 0x1fff 0x200\n0x2000 0x20ff 0x100\n0 0 0\n").unwrap();
        assert_eq!(values.len(), 2);
        assert!(values[0].contains("BAR0: memory"));
        assert!(values[1].contains("I/O ports"));
        assert!(resources("ff 10 200").is_err());
        assert!(resources("0 ffffffffffffffff 200").is_err());
        assert!(resources("config space is not metadata").is_err());
    }
}
