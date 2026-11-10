//! Mounted sysfs facts plus explicitly scoped, read-only current-namespace queries.
mod codec;
mod socket;
use super::{
    Document, Relation,
    buses::{self, Topology},
    model::{Fact, Outcome, Value},
};
use std::{
    io,
    path::{Path, PathBuf},
};
fn value<'a>(document: &'a Document, root: &Path, name: &str) -> Option<&'a str> {
    document
        .facts
        .iter()
        .find(|fact| {
            fact.source == root.join(name) && fact.outcome == Outcome::Observed && !fact.retained
        })?
        .value
        .as_deref()
}
fn index(document: &Document, root: &Path) -> Option<u32> {
    document
        .facts
        .iter()
        .find(|fact| {
            fact.source == root.join("ifindex")
                && fact.outcome == Outcome::Observed
                && !fact.retained
        })?
        .semantic
        .as_ref()
        .and_then(|value| match value {
            Value::Integer(index) => u32::try_from(*index).ok().filter(|index| *index > 0),
            _ => None,
        })
}
fn fact(document: &mut Document, root: &Path, label: &str, value: impl Into<String>) {
    let mut fact = Fact::text(label, value, root.into());
    fact.origin = "rtnetlink in current network namespace".into();
    document.facts.push(fact);
}
fn correlated(document: &Document, root: &Path, link: &codec::Link) -> bool {
    link.name == root.file_name().unwrap_or_default().as_encoded_bytes()
        && index(document, root) == Some(link.index)
        && value(document, root, "address")
            .is_some_and(|address| address.eq_ignore_ascii_case(&mac(&link.address)))
}
fn mac(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}
pub(super) fn collect(
    document: &mut Document,
    root: &Path,
    checkpoint: &mut dyn FnMut(&Document),
    reads: &mut super::collector::Reads,
) {
    if buses::subsystem(root).as_deref() != Some("net") {
        return;
    }
    queues(document, root);
    let (kind, summary) = describe(document, root, None);
    let mut topology = Topology::new("Network relationships", summary);
    let selected = buses::add(
        &mut topology,
        document,
        root.into(),
        format!(
            "{} · {kind}",
            root.file_name().unwrap_or_default().to_string_lossy()
        ),
        None,
    );
    metadata_links(document, root, &mut topology, selected);
    document.topology = Some(topology);
    document.title = "Network interface".into();
    document.facts.push(Fact::text("Network interpretation","Sysfs is the mounted view; rtnetlink queries the current network namespace. Netlink enrichment requires verified index/name/address correlation. A configured interface or assigned address does not prove reachability. No traffic counters, historical bandwidth graphs, subscriptions, namespace switching or configuration writes.",root.into()));
    checkpoint(document);
    if !document.namespace.known() {
        let mut field = Fact::text(
            "Netlink policy",
            "Namespace identity is unavailable; no current-namespace query admitted",
            root.into(),
        );
        field.outcome = Outcome::NotRequested;
        document.facts.push(field);
        return;
    }
    let Some(index) = index(document, root) else {
        return;
    };
    let result =
        socket::Query::new().and_then(|mut query| query.link(index).map(|link| (query, link)));
    let (mut query, link) = match result {
        Ok(result) => result,
        Err(error) => {
            document.facts.push(Fact::failure(
                "Current-namespace link query",
                root.into(),
                error,
            ));
            return;
        }
    };
    if !document.namespace.known() || !correlated(document, root, &link) {
        document.facts.push(Fact::failure("Network view correlation",root.into(),io::Error::new(io::ErrorKind::InvalidData,"Mounted sysfs and current-network-namespace index/name/address do not match or namespace identity is unavailable")));
        return;
    }
    let (kind, summary) = describe(document, root, Some(&link));
    if let Some(topology) = &mut document.topology {
        topology.summary = summary;
        if let Some(selected) = topology.nodes.first_mut() {
            selected.label = format!("{} · {kind}", String::from_utf8_lossy(&link.name));
        }
    }
    checkpoint(document);
    addresses(document, root, &mut query, link.index);
    related_link(
        document,
        root,
        &mut query,
        link.master,
        "Bridge/bond/VRF master",
        reads,
    );
    if let Some(namespace) = link.peer_namespace {
        fact(
            document,
            root,
            "Peer namespace reference",
            format!(
                "ifindex {} in relative namespace ID {namespace}; no cross-namespace path is inferred",
                link.parent
                    .map_or_else(|| "unavailable".into(), |index| index.to_string())
            ),
        );
    } else {
        related_link(
            document,
            root,
            &mut query,
            link.local_parent(),
            if link.kind.as_deref() == Some("veth") {
                "Same-namespace veth peer"
            } else {
                "Lower interface"
            },
            reads,
        );
    }
    checkpoint(document);
}
fn describe(document: &mut Document, root: &Path, link: Option<&codec::Link>) -> (String, String) {
    let hardware = link
        .map(|link| link.hardware)
        .or_else(|| value(document, root, "type").and_then(|value| value.parse().ok()));
    let kind = interface_kind(root, link, hardware);
    let driver = buses::driver(&root.join("device")).unwrap_or_else(|| "driver unavailable".into());
    let backing = if root.starts_with("/sys/devices/virtual") {
        "virtual kernel interface"
    } else if root.join("device").exists() {
        "hardware-backed interface"
    } else {
        "hardware association unavailable"
    };
    document.facts.retain(|fact| {
        fact.source != root
            || !matches!(
                fact.label.as_str(),
                "Interface name" | "Interface classification"
            )
    });
    let mut name = Fact::text(
        "Interface name",
        root.file_name().unwrap_or_default().to_string_lossy(),
        root.into(),
    );
    name.origin = "mounted sysfs network metadata".into();
    document.facts.push(name);
    let mut classification = Fact::text(
        "Interface classification",
        format!("{kind} · {backing} · {driver}"),
        root.into(),
    );
    classification.origin = if link.is_some() {
        "correlated mounted sysfs and current-namespace rtnetlink"
    } else {
        "mounted sysfs network metadata"
    }
    .into();
    document.facts.push(classification);
    if let Some(link) = link {
        fact(
            document,
            root,
            "Administrative state",
            if link.flags & 1 != 0 {
                "Up (configured)"
            } else {
                "Down (configured)"
            },
        );
        if let Some(id) = link.vlan {
            fact(document, root, "VLAN ID", id.to_string());
        }
        fact(
            document,
            root,
            "Reported queue counts",
            format!(
                "TX {} · RX {}",
                link.tx_queues
                    .map_or_else(|| "unavailable".into(), |count| count.to_string()),
                link.rx_queues
                    .map_or_else(|| "unavailable".into(), |count| count.to_string())
            ),
        );
    }
    let name = root.file_name().unwrap_or_default().to_string_lossy();
    let mut summary = vec![name.to_string(), kind.clone()];
    if driver != "driver unavailable" {
        summary.push(driver);
    }
    if let Some(speed) = value(document, root, "speed").filter(|speed| *speed != "Unknown") {
        summary.push(
            format!("{speed} {}", value(document, root, "duplex").unwrap_or(""))
                .trim()
                .to_string(),
        );
    }
    summary.push(format!(
        "Carrier {}",
        value(document, root, "carrier").unwrap_or("unavailable")
    ));
    summary.push(format!(
        "MTU {}",
        value(document, root, "mtu").unwrap_or("unavailable")
    ));
    let summary = summary.join(" · ");
    (kind, summary)
}
fn interface_kind(root: &Path, link: Option<&codec::Link>, hardware: Option<u16>) -> String {
    if let Some(kind) = link.and_then(|link| link.kind.as_ref()) {
        return if kind == "tun" {
            "TUN/TAP".into()
        } else {
            kind.clone()
        };
    }
    for (directory, kind) in [("bridge", "Bridge"), ("bonding", "Bond")] {
        if root.join(directory).is_dir() {
            return kind.into();
        }
    }
    if root.join("wireless").is_dir() || root.join("phy80211").exists() {
        return "Wi-Fi".into();
    }
    match hardware {
        Some(1) => "Ethernet".into(),
        Some(772) => "Loopback".into(),
        Some(kind) => format!("ARPHRD {kind}"),
        None => "Interface type unavailable".into(),
    }
}
fn queue_side(entry: &std::fs::DirEntry) -> io::Result<Option<&'static str>> {
    let name = entry.file_name();
    let Some((side, id)) = name.to_str().and_then(|name| name.split_once('-')) else {
        return Ok(None);
    };
    if !matches!(side, "tx" | "rx")
        || id
            .parse::<u32>()
            .ok()
            .is_none_or(|number| number.to_string() != id)
    {
        return Ok(None);
    }
    if !entry.file_type()?.is_dir() {
        return Ok(None);
    }
    Ok(Some(if side == "tx" { "TX" } else { "RX" }))
}

fn queues(document: &mut Document, root: &Path) {
    let mut tx = 0;
    let mut rx = 0;
    let mut partial = false;
    let mut unavailable = None;
    match std::fs::read_dir(root.join("queues")) {
        Ok(entries) => {
            for (index, entry) in entries.enumerate().take(129) {
                if index == 128 {
                    partial = true;
                    break;
                }
                match entry.and_then(|entry| queue_side(&entry)) {
                    Ok(Some("TX")) => tx += 1,
                    Ok(Some("RX")) => rx += 1,
                    Ok(_) => {}
                    Err(error) => {
                        partial = true;
                        document.facts.push(Fact::failure(
                            "Queue inventory",
                            root.join("queues"),
                            error,
                        ));
                    }
                }
            }
        }
        Err(error) => unavailable = Some(error),
    }
    let mut field = match unavailable {
        Some(error) => Fact::failure("Observed queue directories", root.join("queues"), error),
        None => Fact::text(
            "Observed queue directories",
            format!(
                "TX {tx} · RX {rx}{}",
                if partial { " · incomplete" } else { "" }
            ),
            root.join("queues"),
        ),
    };
    if partial {
        field.outcome = Outcome::Truncated;
    }
    document.facts.push(field);
}
fn address_strip(addresses: &[codec::Address]) -> String {
    if addresses.is_empty() {
        return "No address records returned for this interface in the current namespace".into();
    }
    let mut label = addresses
        .iter()
        .take(3)
        .map(|address| {
            format!(
                "{}/{}{}{}",
                address.local,
                address.prefix,
                if address.flags & 64 != 0 {
                    " (tentative)"
                } else {
                    ""
                },
                if address.flags & 8 != 0 {
                    " (DAD failed)"
                } else {
                    ""
                }
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    if addresses.len() > 3 {
        label.push_str(&format!(" · +{} more", addresses.len() - 3));
    }
    label
}
fn addresses(document: &mut Document, root: &Path, query: &mut socket::Query, index: u32) {
    match query.addresses(index) {
        Ok(addresses) => {
            fact(
                document,
                root,
                "Address inventory",
                format!(
                    "{} assigned address records in the current network namespace; showing {}{}",
                    addresses.len(),
                    addresses.len().min(64),
                    if addresses.len() > 64 {
                        " (truncated display)"
                    } else {
                        ""
                    }
                ),
            );
            if let Some(topology) = &mut document.topology {
                if addresses.len() > 64 {
                    topology.coverage = Outcome::Truncated;
                }
                if let Some(node) = topology.nodes.first_mut() {
                    node.label = address_strip(&addresses);
                }
            }
            for address in addresses.into_iter().take(64) {
                let scope = match address.scope {
                    0 => "global",
                    253 => "link",
                    254 => "host",
                    _ => "other scope",
                };
                let detail = format!(
                    "{}/{} · {scope} ({}) · flags 0x{:x}{}{}",
                    address.local,
                    address.prefix,
                    address.scope,
                    address.flags,
                    if address.flags & 64 != 0 {
                        " · tentative"
                    } else {
                        ""
                    },
                    if address.flags & 8 != 0 {
                        " · DAD failed"
                    } else {
                        ""
                    }
                );
                fact(
                    document,
                    root,
                    &format!("Address {}/{}", address.local, address.prefix),
                    detail,
                );
                if let Some(peer) = address.peer {
                    fact(document, root, "Point-to-point peer address", peer);
                }
            }
        }
        Err(error) => {
            let field = Fact::failure("Address inventory", root.into(), error);
            if let Some(topology) = &mut document.topology {
                topology.coverage = field.outcome.clone();
            }
            document.facts.push(field);
        }
    }
}
fn related_link(
    document: &mut Document,
    root: &Path,
    query: &mut socket::Query,
    index: Option<u32>,
    label: &str,
    reads: &mut super::collector::Reads,
) {
    let Some(index) = index.filter(|index| *index > 0) else {
        return;
    };
    match query.link(index) {
        Ok(link) => {
            fact(
                document,
                root,
                label,
                format!("{} · ifindex {index}", String::from_utf8_lossy(&link.name)),
            );
            // Peer namespace indices never arrive here. Check mounted metadata
            // before making a path relationship from a current-namespace name.
            use std::os::unix::ffi::OsStrExt;
            let path =
                PathBuf::from("/sys/class/net").join(std::ffi::OsStr::from_bytes(&link.name));
            if reads
                .read(&path.join("ifindex"))
                .is_ok_and(|value| value.trim().parse::<u32>().ok() == Some(index))
                && reads
                    .read(&path.join("address"))
                    .is_ok_and(|value| value.trim().eq_ignore_ascii_case(&mac(&link.address)))
            {
                document.relations.push(Relation::new(label, path));
            }
        }
        Err(error) => document
            .facts
            .push(Fact::failure(label, root.into(), error)),
    }
}
fn metadata_links(document: &mut Document, root: &Path, topology: &mut Topology, parent: usize) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                "Network relationship inventory",
                root.into(),
                error,
            ));
            topology.coverage = Outcome::Unsupported;
            return;
        }
    };
    for (index, entry) in entries.enumerate().take(129) {
        if index == 128 {
            topology.coverage = Outcome::Truncated;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                document.facts.push(Fact::failure(
                    "Network relationship inventory",
                    root.into(),
                    error,
                ));
                topology.coverage = Outcome::Unsupported;
                continue;
            }
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("lower_") && !name.starts_with("upper_") {
            continue;
        }
        if topology.nodes.len() >= 32 {
            topology.coverage = Outcome::Truncated;
            break;
        }
        match entry.path().canonicalize() {
            Ok(path) => {
                buses::add(topology, document, path, name.into_owned(), Some(parent));
            }
            Err(error) => {
                document
                    .facts
                    .push(Fact::failure("Network relationship", entry.path(), error));
                topology.coverage = Outcome::Disappeared;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Document, codec::Link) {
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = "/sys/devices/virtual/net/fixture0".into();
        document.facts.clear();
        let root = document.path.clone();
        let mut index = Fact::text("Interface index", "7", root.join("ifindex"));
        index.semantic = Some(Value::Integer(7));
        document.facts.push(index);
        document.facts.push(Fact::text(
            "MAC address",
            "00:01:02:03:04:05",
            root.join("address"),
        ));
        let link = codec::Link {
            index: 7,
            name: b"fixture0".to_vec(),
            kind: Some("veth".into()),
            vlan: None,
            hardware: 1,
            flags: 1,
            address: vec![0, 1, 2, 3, 4, 5],
            master: None,
            parent: Some(8),
            peer_namespace: Some(0),
            tx_queues: None,
            rx_queues: None,
        };
        (document, link)
    }
    #[test]
    fn renamed_reused_or_different_mounted_interfaces_reject_netlink_correlation() {
        let (document, mut link) = fixture();
        assert!(correlated(&document, &document.path, &link));
        link.index = 8;
        assert!(!correlated(&document, &document.path, &link));
        link.index = 7;
        link.name = b"other0".to_vec();
        assert!(!correlated(&document, &document.path, &link));
        link.name = b"fixture0".to_vec();
        link.address[5] = 9;
        assert!(!correlated(&document, &document.path, &link));
    }
    #[test]
    fn generic_tun_driver_kind_does_not_guess_tun_versus_tap_mode() {
        let (document, mut link) = fixture();
        link.kind = Some("tun".into());
        for hardware in [1, 65534] {
            link.hardware = hardware;
            assert_eq!(
                interface_kind(&document.path, Some(&link), Some(link.hardware)),
                "TUN/TAP"
            );
        }
    }

    #[test]
    fn unavailable_mounted_mac_does_not_claim_a_verified_namespace_correlation() {
        let (mut document, link) = fixture();
        document
            .facts
            .retain(|fact| !fact.source.ends_with("address"));
        assert!(!correlated(&document, &document.path, &link));
    }
    #[test]
    fn retained_or_failed_index_never_admits_namespace_correlation() {
        let (mut document, link) = fixture();
        document.facts[0].retained = true;
        assert!(index(&document, &document.path).is_none());
        assert!(!correlated(&document, &document.path, &link));
        document.facts[0].retained = false;
        document.facts[0].outcome = Outcome::PermissionDenied;
        assert!(index(&document, &document.path).is_none());
    }

    #[test]
    fn network_identity_and_mtu_attributes_reject_negative_values() {
        let context =
            super::super::attributes::Context::from_parts(Path::new("/fixture/net0"), Some("net"));
        for name in ["ifindex", "iflink", "mtu"] {
            let rule = super::super::attributes::rule(&context, name).unwrap();
            assert!(super::super::attributes::parse(&rule, "-1").is_err());
            assert!(super::super::attributes::parse(&rule, "1500").is_ok());
        }
    }

    #[test]
    fn queue_counts_require_canonical_numbered_directories_and_preserve_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let directory = root.join("queues");
        std::fs::create_dir(&directory).unwrap();
        for name in ["tx-0", "rx-3", "tx-info", "tx-01", "rx-4294967296"] {
            std::fs::create_dir(directory.join(name)).unwrap();
        }
        std::fs::write(directory.join("rx-4"), b"fixture").unwrap();
        std::os::unix::fs::symlink(directory.join("tx-0"), directory.join("tx-9")).unwrap();
        let (mut document, _) = fixture();
        queues(&mut document, root);
        let observed = document
            .facts
            .iter()
            .find(|fact| fact.label == "Observed queue directories")
            .unwrap();
        assert_eq!(observed.value.as_deref(), Some("TX 1 · RX 1"));
        assert_eq!(observed.outcome, Outcome::Observed);
        document.facts.clear();
        queues(&mut document, &root.join("absent"));
        let missing = document
            .facts
            .iter()
            .find(|fact| fact.label == "Observed queue directories")
            .unwrap();
        assert!(missing.value.is_none());
        assert_eq!(missing.outcome, Outcome::Missing);
    }

    #[test]
    fn mounted_type_fallback_remains_available_without_a_netlink_record() {
        let (mut document, link) = fixture();
        let root = document.path.clone();
        document
            .facts
            .push(Fact::text("Interface type", "772", root.join("type")));
        let (kind, _) = describe(&mut document, &root, None);
        assert_eq!(kind, "Loopback");
        assert!(
            !document
                .facts
                .iter()
                .any(|fact| fact.label == "Administrative state"
                    || fact.label == "Reported queue counts")
        );
        describe(&mut document, &root, Some(&link));
        assert_eq!(
            document
                .facts
                .iter()
                .filter(|fact| fact.label == "Interface name")
                .count(),
            1
        );
        assert_eq!(
            document
                .facts
                .iter()
                .filter(|fact| fact.label == "Interface classification")
                .count(),
            1
        );
    }

    #[test]
    fn exposed_bridge_and_bonding_metadata_identifies_virtual_types_without_netlink() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("bridge")).unwrap();
        assert_eq!(interface_kind(temp.path(), None, Some(1)), "Bridge");
        std::fs::remove_dir(temp.path().join("bridge")).unwrap();
        std::fs::create_dir(temp.path().join("bonding")).unwrap();
        assert_eq!(interface_kind(temp.path(), None, Some(1)), "Bond");
    }

    #[test]
    fn unknown_namespace_identity_rejects_collection_before_any_socket_query() {
        let root = tempfile::tempdir().unwrap();
        let device = root.path().join("fixture0");
        let subsystem = root.path().join("net");
        std::fs::create_dir_all(&device).unwrap();
        std::fs::create_dir_all(&subsystem).unwrap();
        std::os::unix::fs::symlink(&subsystem, device.join("subsystem")).unwrap();
        let (mut document, _) = fixture();
        document.namespace.network = None;
        std::fs::create_dir_all(device.join("queues/tx-0")).unwrap();
        collect(
            &mut document,
            &device,
            &mut |_| {},
            &mut super::super::collector::Reads::new(),
        );
        assert!(document.facts.iter().any(|fact|fact.label=="Netlink policy"&&fact.outcome==Outcome::NotRequested));
        assert!(document.topology.is_some());
        assert_eq!(document.title, "Network interface");
        assert!(document.facts.iter().any(
            |fact| fact.label == "Interface name" && fact.value.as_deref() == Some("fixture0")
        ));
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Interface classification"
                    && fact.origin == "mounted sysfs network metadata")
        );
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Observed queue directories"
                    && fact.value.as_deref() == Some("TX 1 · RX 0"))
        );
        assert!(
            !document
                .facts
                .iter()
                .any(|fact| fact.origin == "rtnetlink in current network namespace")
        );
    }
}
