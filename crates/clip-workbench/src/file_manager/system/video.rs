//! Passive V4L2/media interface associations. No streaming or device ioctls.
use super::{
    Document, Relation,
    attributes::{self, Context, Family},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use std::path::{Path, PathBuf};

fn role(path: &Path) -> Option<&'static str> {
    match buses::subsystem(path).as_deref() {
        Some("media") => Some("Media controller"),
        Some("video4linux") => {
            let name = path.file_name()?.to_str()?;
            for (prefix, role) in [
                ("v4l-subdev", "V4L2 subdevice"),
                ("video", "V4L2 video interface"),
                ("vbi", "VBI interface"),
                ("swradio", "Software radio interface"),
                ("radio", "Radio interface"),
            ] {
                if name.strip_prefix(prefix).is_some_and(|suffix| {
                    !suffix.is_empty() && suffix.bytes().all(|c| c.is_ascii_digit())
                }) {
                    return Some(role);
                }
            }
            Some("V4L2 interface")
        }
        _ => None,
    }
}
/// Nearest registered parent. Never group devices by a shared name or bus host.
fn owner(sys: &Path) -> Option<PathBuf> {
    if let Ok(device) = sys.join("device").canonicalize() {
        if role(&device).is_none() && device != sys {
            return Some(device);
        }
    }
    sys.ancestors()
        .skip(1)
        .take(12)
        .find(|parent| {
            matches!(
                buses::subsystem(parent).as_deref(),
                Some("usb" | "pci" | "platform" | "i2c" | "spi" | "virtio" | "auxiliary")
            )
        })
        .map(Path::to_path_buf)
}
fn group_root(owner: &Path) -> PathBuf {
    buses::usb_parent(owner).unwrap_or_else(|| owner.to_path_buf())
}

fn name(document: &mut Document, path: &Path, reads: &mut Reads) -> String {
    let context = Context::discover(path);
    let rule = attributes::rules(&context)
        .into_iter()
        .find(|rule| rule.name == "name" || rule.name == "model");
    rule.and_then(|rule| reads.observe(document, path, &rule).value)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
}
fn cached_capabilities(document: &mut Document, sys: &Path) -> Option<String> {
    let device = udev::Device::from_syspath(sys).ok()?;
    let raw = device.property_value("ID_V4L_CAPABILITIES")?.to_str()?;
    let capabilities = raw
        .split(':')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" / ");
    if capabilities.is_empty() {
        return None;
    }
    let mut fact = Fact::text(
        "Cached V4L2 capabilities",
        capabilities.chars().take(256).collect::<String>(),
        sys.into(),
    );
    fact.origin = "udev property ID_V4L_CAPABILITIES; cached observation, not a live query".into();
    if capabilities.chars().count() > 256 {
        fact.outcome = Outcome::Truncated;
    }
    let summary = fact.value.clone();
    document.facts.push(fact);
    summary
}
pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let Some(selected_role) = role(sys) else {
        return;
    };
    let context = Context::discover(sys);
    if !matches!(context.family, Family::Video | Family::Media) {
        return;
    }
    document.title = selected_role.into();
    let selected_name = name(document, sys, reads);
    let parent = owner(sys);
    let binding = parent
        .as_deref()
        .and_then(|owner| buses::driver(owner).map(|driver| (owner.join("driver"), driver)));
    if let Some((source, driver)) = &binding {
        document
            .facts
            .push(Fact::text("Media driver", driver, source.clone()));
    }
    let driver = binding.as_ref().map(|(_, driver)| driver.as_str());
    let capabilities = cached_capabilities(document, sys);
    let mut summary = format!(
        "{selected_name} · {}",
        driver.unwrap_or("driver unavailable")
    );
    if let Some(capabilities) = capabilities {
        summary.push_str(&format!(" · cached: {capabilities}"));
    }
    let mut topology = Topology::new("Media access interfaces", summary);
    let root = parent.as_deref().map(group_root);
    let mut anchor = None;
    if let Some(root) = &root {
        let bus = buses::subsystem(root).unwrap_or_else(|| "registered parent".into());
        let label = format!(
            "{bus} · {}",
            root.file_name().unwrap_or_default().to_string_lossy()
        );
        anchor = Some(buses::add(
            &mut topology,
            document,
            root.clone(),
            label,
            None,
        ));
        document.facts.push(Fact::text(
            "Media parent connection",
            format!("{bus} · {}", root.display()),
            root.clone(),
        ));
    }
    if let Some(parent) = &parent {
        if root.as_ref() != Some(parent) {
            anchor = Some(buses::add(
                &mut topology,
                document,
                parent.clone(),
                format!(
                    "{} · {}",
                    parent.file_name().unwrap_or_default().to_string_lossy(),
                    driver.unwrap_or("driver unavailable")
                ),
                anchor,
            ));
        }
        document
            .relations
            .push(Relation::new("Registered media parent", parent.clone()));
    }
    buses::add(
        &mut topology,
        document,
        sys.into(),
        format!(
            "{} · {selected_role} · {selected_name}",
            sys.file_name().unwrap_or_default().to_string_lossy()
        ),
        anchor,
    );
    buses::access_node(document, sys, selected_role);
    document.topology = Some(topology);
    checkpoint(document);
    if let Some(root) = root {
        related(document, &root, sys, reads, checkpoint);
    }
    document.facts.push(Fact::text("Media association scope", "Registered parent associations; related-node scan is bounded to depth 4 and 256 directories, with at most 32 peers. Driver-specific signal topology is not queried.", sys.into()));
    document.facts.push(Fact::text("Media inspection policy", "Passive names, registered parent associations and cached udev metadata only. No device opened, frames captured, streaming, controls or capability ioctls requested. Associations do not establish a sensor-to-capture signal graph.", sys.into()));
    checkpoint(document);
}
fn related(
    document: &mut Document,
    root: &Path,
    selected: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let (related, coverage) =
        buses::select_descendants(root, 4, 32, |path| path != selected && role(path).is_some());
    if let Some(topology) = &mut document.topology {
        topology.coverage = coverage;
    }
    for path in related {
        // Check owner identity rather than merely accepting a node under a PCI host.
        if owner(&path).as_deref().map(group_root).as_ref() != Some(&root.to_path_buf()) {
            continue;
        }
        let Some(peer_role) = role(&path) else {
            if let Some(topology) = &mut document.topology {
                topology.coverage = Outcome::Disappeared;
            }
            continue;
        };
        let label = format!(
            "{} · {peer_role} · {}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            name(document, &path, reads)
        );
        let Some(mut topology) = document.topology.take() else {
            return;
        };
        let parent = owner(&path)
            .and_then(|owner| topology.nodes.iter().position(|node| node.path == owner))
            .or(Some(0));
        buses::add(&mut topology, document, path.clone(), label.clone(), parent);
        buses::access_node(document, &path, &label);
        document.topology = Some(topology);
        checkpoint(document);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};
    struct Fixture {
        dir: tempfile::TempDir,
        physical: PathBuf,
        interface: PathBuf,
        video: PathBuf,
        peer: PathBuf,
        media: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let physical = dir.path().join("1-2");
            let interface = physical.join("1-2:1.0");
            let video = interface.join("video4linux/video0");
            let peer = interface.join("video4linux/video1");
            let media = interface.join("media0");
            for bus in ["usb", "video4linux", "media"] {
                std::fs::create_dir(dir.path().join(bus)).unwrap();
            }
            for path in [&physical, &interface, &video, &peer, &media] {
                std::fs::create_dir_all(path).unwrap();
            }
            for path in [&physical, &interface] {
                symlink(dir.path().join("usb"), path.join("subsystem")).unwrap();
            }
            for path in [&video, &peer] {
                symlink(dir.path().join("video4linux"), path.join("subsystem")).unwrap();
                symlink(&interface, path.join("device")).unwrap();
            }
            symlink(dir.path().join("media"), media.join("subsystem")).unwrap();
            std::fs::write(physical.join("idVendor"), "metadata marker only").unwrap();
            Self {
                dir,
                physical,
                interface,
                video,
                peer,
                media,
            }
        }
        fn reads(&self) -> Reads {
            Reads::fixture(BTreeMap::from([
                (self.video.join("name"), b"Camera".to_vec()),
                (self.peer.join("name"), b"Camera metadata".to_vec()),
                (self.media.join("model"), b"Camera controller".to_vec()),
            ]))
        }
    }
    #[test]
    fn video_and_media_share_hardware_associations_without_streaming() {
        let fixture = Fixture::new();
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.facts.clear();
        let mut reads = fixture.reads();
        collect(&mut document, &fixture.video, &mut reads, &mut |_| {});
        let topology = document.topology.as_ref().unwrap();
        assert_eq!(topology.nodes[0].path, fixture.physical);
        assert_eq!(document.title, "V4L2 video interface");
        assert!(topology.nodes.iter().any(|node| node.path == fixture.peer));
        assert!(topology.nodes.iter().any(|node| node.path == fixture.media));
        assert!(reads.attempts().all(|path| matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("name" | "model")
        )));
        assert!(
            !document
                .facts
                .iter()
                .any(|fact| fact.label.contains("capture capabilities"))
        );
        collect(&mut document, &fixture.media, &mut reads, &mut |_| {});
        assert_eq!(document.title, "Media controller");
        assert!(
            document
                .topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.path == fixture.video)
        );
    }
    #[test]
    fn node_prefix_does_not_imply_capture_or_physical_hardware() {
        let fixture = Fixture::new();
        let unrelated = fixture.dir.path().join("video42");
        std::fs::create_dir(&unrelated).unwrap();
        assert!(role(&unrelated).is_none());
        let virtual_node = fixture.dir.path().join("virtual/video4linux/video8");
        std::fs::create_dir_all(&virtual_node).unwrap();
        symlink(
            fixture.dir.path().join("video4linux"),
            virtual_node.join("subsystem"),
        )
        .unwrap();
        assert_eq!(role(&virtual_node), Some("V4L2 video interface"));
        assert!(owner(&virtual_node).is_none());
        assert_eq!(owner(&fixture.media), Some(fixture.interface));
    }
    #[test]
    fn attribute_policy_is_scoped_and_excludes_debug_controls() {
        let video = Context::from_parts(
            Path::new("/sys/class/video4linux/video0"),
            Some("video4linux"),
        );
        let media = Context::from_parts(Path::new("/sys/devices/device/media0"), Some("media"));
        assert!(attributes::rule(&video, "name").is_some());
        assert!(attributes::rule(&video, "index").is_some());
        assert!(attributes::rule(&video, "dev_debug").is_none());
        assert!(attributes::rule(&video, "model").is_none());
        assert!(attributes::rule(&media, "model").is_some());
        assert!(attributes::rule(&media, "name").is_none());
    }
    #[test]
    fn missing_names_do_not_become_successful_hardware_identity() {
        let fixture = Fixture::new();
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.facts.clear();
        let mut reads = Reads::fixture(BTreeMap::new());
        collect(&mut document, &fixture.video, &mut reads, &mut |_| {});
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Video device name" && fact.outcome == Outcome::Missing)
        );
        assert!(
            document
                .topology
                .as_ref()
                .unwrap()
                .summary
                .starts_with("video0")
        );
        assert!(
            !document
                .facts
                .iter()
                .any(|fact| fact.label == "Video device name" && fact.outcome == Outcome::Observed)
        );
    }
    #[test]
    fn peer_inventory_is_bounded_and_does_not_follow_directory_aliases() {
        let fixture = Fixture::new();
        let escaped = fixture.dir.path().join("escape/video99");
        std::fs::create_dir_all(&escaped).unwrap();
        symlink(
            fixture.dir.path().join("video4linux"),
            escaped.join("subsystem"),
        )
        .unwrap();
        symlink(&escaped, fixture.interface.join("escape-link")).unwrap();
        for index in 2..42 {
            let node = fixture.interface.join(format!("video4linux/video{index}"));
            std::fs::create_dir(&node).unwrap();
            symlink(
                fixture.dir.path().join("video4linux"),
                node.join("subsystem"),
            )
            .unwrap();
            symlink(&fixture.interface, node.join("device")).unwrap();
        }
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        let mut reads = fixture.reads();
        collect(&mut document, &fixture.video, &mut reads, &mut |_| {});
        let topology = document.topology.as_ref().unwrap();
        assert_eq!(topology.coverage, Outcome::Truncated);
        assert_eq!(topology.nodes.len(), 35); // Physical device, parent, selected + 32 peers.
        assert!(!topology.nodes.iter().any(|node| node.path == escaped));
        assert!(reads.attempts().count() <= 33);
    }
    #[test]
    fn media_driver_comes_from_owner_binding_not_bus_host_driver() {
        let fixture = Fixture::new();
        let host_driver = fixture.dir.path().join("host-controller");
        let media_driver = fixture.dir.path().join("uvcvideo");
        std::fs::create_dir(&host_driver).unwrap();
        std::fs::create_dir(&media_driver).unwrap();
        symlink(&host_driver, fixture.physical.join("driver")).unwrap();
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        let mut reads = fixture.reads();
        collect(&mut document, &fixture.video, &mut reads, &mut |_| {});
        assert!(
            !document
                .facts
                .iter()
                .any(|fact| fact.label == "Media driver")
        );
        symlink(&media_driver, fixture.interface.join("driver")).unwrap();
        collect(&mut document, &fixture.video, &mut reads, &mut |_| {});
        let fact = document
            .facts
            .iter()
            .find(|fact| fact.label == "Media driver")
            .unwrap();
        assert_eq!(fact.value.as_deref(), Some("uvcvideo"));
        assert_eq!(fact.source, fixture.interface.join("driver"));
    }
}
