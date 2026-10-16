//! Passive ALSA card and access-interface metadata; never opens sound streams.
use super::{
    Document,
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Interface {
    card: Option<u32>,
    device: Option<u32>,
    role: &'static str,
    pcm: Option<char>,
}
fn number(raw: &str) -> Option<u32> {
    if raw.is_empty() || !raw.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}
fn interface(name: &str) -> Option<Interface> {
    let (card, device, role, pcm) = if let Some(n) = name.strip_prefix("card") {
        (Some(number(n)?), None, "Sound card", None)
    } else if let Some(n) = name.strip_prefix("controlC") {
        (Some(number(n)?), None, "Control", None)
    } else if let Some(n) = name.strip_prefix("pcmC") {
        let (card, rest) = n.split_once('D')?;
        let direction = rest.chars().last()?;
        let role = match direction {
            'p' => "PCM playback",
            'c' => "PCM capture",
            _ => return None,
        };
        (
            Some(number(card)?),
            Some(number(&rest[..rest.len() - 1])?),
            role,
            Some(direction),
        )
    } else if let Some(n) = name
        .strip_prefix("midiC")
        .or_else(|| name.strip_prefix("hwC"))
    {
        let (card, device) = n.split_once('D')?;
        (
            Some(number(card)?),
            Some(number(device)?),
            if name.starts_with("midi") {
                "Raw MIDI"
            } else {
                "Hardware-dependent"
            },
            None,
        )
    } else {
        match name {
            "seq" => (None, None, "Sequencer", None),
            "timer" => (None, None, "Timer", None),
            _ => return None,
        }
    };
    Some(Interface {
        card,
        device,
        role,
        pcm,
    })
}

fn matching_card(sys: &Path, number: u32) -> Option<&Path> {
    let name = format!("card{number}");
    sys.ancestors()
        .take(4)
        .find(|path| path.file_name().and_then(|name| name.to_str()) == Some(&name))
}

/// Shares registered card membership and interface roles with directory inventories.
pub(super) fn directory_group(sys: &Path) -> Option<(Option<PathBuf>, String)> {
    if buses::subsystem(sys).as_deref() != Some("sound") {
        return None;
    }
    let selected = interface(sys.file_name()?.to_str()?)?;
    let card = selected
        .card
        .and_then(|number| matching_card(sys, number))
        .map(Path::to_path_buf);
    Some((card, selected.role.into()))
}
/// Only documented card listing and PCM information, no registers or controls.
pub(super) fn proc_metadata(path: &Path) -> bool {
    if path == Path::new("/proc/asound/cards") {
        return true;
    }
    let Ok(relative) = path.strip_prefix("/proc/asound") else {
        return false;
    };
    let Some(parts) = relative
        .iter()
        .map(|s| s.to_str())
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    if parts.len() != 3 || parts[2] != "info" {
        return false;
    }
    let card = parts[0].strip_prefix("card").and_then(number);
    let Some(pcm) = parts[1].strip_prefix("pcm") else {
        return false;
    };
    let Some(direction) = pcm.chars().last() else {
        return false;
    };
    card.is_some() && matches!(direction, 'p' | 'c') && number(&pcm[..pcm.len() - 1]).is_some()
}
fn card_description(raw: &str, card: u32, id: &str) -> Option<String> {
    for line in raw.lines() {
        let Some((prefix, description)) = line.split_once("]: ") else {
            continue;
        };
        let Some((index, observed_id)) = prefix.split_once('[') else {
            continue;
        };
        if number(index.trim()) == Some(card) && observed_id.trim() == id {
            return Some(description.trim().to_string());
        }
    }
    None
}
fn proc_fact(
    document: &mut Document,
    reads: &mut Reads,
    source: PathBuf,
    label: &str,
) -> Option<String> {
    let fact = match reads.asound(&source) {
        Ok(raw) => {
            let mut fact = Fact::text(label, &raw, source);
            fact.raw = Some(raw);
            fact.origin = "documented ALSA proc metadata".into();
            fact
        }
        Err(error) => Fact::failure(label, source, error),
    };
    let value = fact.raw.clone();
    document.facts.push(fact);
    value
}
pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    if buses::subsystem(sys).as_deref() != Some("sound") {
        return;
    }
    let Some(selected) = sys.file_name().and_then(|n| n.to_str()).and_then(interface) else {
        return;
    };
    document.title = format!("ALSA {}", selected.role.to_lowercase());
    document
        .facts
        .push(Fact::text("ALSA interface", selected.role, sys.into()));
    let Some(card_number) = selected.card else {
        document.topology = Some(Topology::new(
            "ALSA global interface",
            format!("ALSA {} · global, not one sound card", selected.role),
        ));
        checkpoint(document);
        return;
    };
    document.facts.push(Fact::text(
        "ALSA card number",
        card_number.to_string(),
        sys.into(),
    ));
    let Some(card) = matching_card(sys, card_number) else {
        document.facts.push(Fact::failure(
            "ALSA card relationship",
            sys.into(),
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Matching sound-card parent is unavailable in this sysfs view",
            ),
        ));
        return;
    };
    let id = reads
        .observe(
            document,
            card,
            &super::attributes::Rule {
                name: "id".into(),
                label: "ALSA card ID".into(),
                format: super::attributes::Format::Text,
            },
        )
        .raw
        .unwrap_or_default();
    let mut name = id.clone();
    let source = PathBuf::from("/proc/asound/cards");
    match reads.asound(&source) {
        Ok(raw) => match card_description(&raw, card_number, &id) {
            Some(description) => {
                name = description;
                let mut fact = Fact::text("Sound card name", &name, source);
                fact.origin = "ALSA card listing matched by index and ID".into();
                document.facts.push(fact);
            }
            None => document.facts.push(Fact::failure(
                "Sound card name",
                source,
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Card index and ID do not match the observed ALSA card listing",
                ),
            )),
        },
        Err(error) => document
            .facts
            .push(Fact::failure("Sound card name", source, error)),
    }
    if let Some(device) = selected.device {
        document.facts.push(Fact::text(
            "ALSA device number",
            device.to_string(),
            sys.into(),
        ));
    }
    let mut topology = Topology::new(
        "ALSA sound card",
        format!("{name} · card {card_number} · {}", selected.role),
    );
    let parents = card
        .ancestors()
        .skip(1)
        .take(10)
        .filter(|path| {
            matches!(
                buses::subsystem(path).as_deref(),
                Some("usb" | "pci" | "platform")
            )
        })
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    let parent = buses::ancestor_chain(&mut topology, document, parents.into_iter().rev(), None);
    let root = buses::add(
        &mut topology,
        document,
        card.into(),
        format!("Card {card_number} · {name}"),
        parent,
    );
    // Append the chosen interface before its siblings.
    if sys != card {
        buses::add(
            &mut topology,
            document,
            sys.into(),
            selected.role.into(),
            Some(root),
        );
        buses::access_node(document, sys, selected.role);
    }
    inventory(&mut topology, document, card, root, sys);
    document.topology = Some(topology);
    checkpoint(document);
    if let (Some(device), Some(direction)) = (selected.device, selected.pcm) {
        let path = PathBuf::from(format!(
            "/proc/asound/card{card_number}/pcm{device}{direction}/info"
        ));
        proc_fact(document, reads, path, "PCM stream information");
    }
    document.facts.push(Fact::text("Audio inspection policy", "Passive metadata only; no stream opened, no capture, no live meters. PCM information is an observation, not a complete capability query.", sys.into()));
    checkpoint(document);
}
fn inventory(
    topology: &mut Topology,
    document: &mut Document,
    card: &Path,
    parent: usize,
    selected: &Path,
) {
    let entries = match std::fs::read_dir(card) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                "ALSA interface inventory",
                card.into(),
                error,
            ));
            topology.coverage = Outcome::Unsupported;
            return;
        }
    };
    let expected = card
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(interface)
        .and_then(|i| i.card);
    let (paths, coverage) = buses::select_children(
        entries,
        |entry| {
            entry
                .file_name()
                .to_str()
                .and_then(interface)
                .is_some_and(|kind| kind.card == expected)
                && entry.path() != selected
                && entry.file_type().is_ok_and(|t| t.is_dir())
        },
        32,
    );
    if coverage != Outcome::Observed {
        topology.coverage = coverage;
    }
    for path in paths {
        let Some(kind) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(interface)
        else {
            continue;
        };
        let label = format!(
            "{} · {}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            kind.role
        );
        buses::add(topology, document, path.clone(), label, Some(parent));
        buses::access_node(document, &path, kind.role);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roles_require_complete_numeric_names() {
        for (name, role) in [
            ("pcmC2D0p", "PCM playback"),
            ("pcmC2D0c", "PCM capture"),
            ("controlC2", "Control"),
            ("midiC3D0", "Raw MIDI"),
            ("hwC1D0", "Hardware-dependent"),
            ("seq", "Sequencer"),
            ("timer", "Timer"),
        ] {
            assert_eq!(interface(name).unwrap().role, role);
        }
        for name in [
            "pcmC2D0",
            "pcmC2D0x",
            "midiC3D",
            "card",
            "pcmC99999999999D0p",
            "pcmC2D0pp",
        ] {
            assert!(interface(name).is_none(), "{name}");
        }
    }
    #[test]
    fn proc_policy_excludes_registers_controls_and_traversal() {
        for name in [
            "/proc/asound/cards",
            "/proc/asound/card2/pcm0p/info",
            "/proc/asound/card2/pcm0c/info",
        ] {
            assert!(proc_metadata(Path::new(name)));
        }
        for name in [
            "/proc/asound/card2/codec#0",
            "/proc/asound/card2/pcm0p/xrun_debug",
            "/proc/asound/card2/pcm0p/sub0/xrun_injection",
            "/proc/asound/card2/../pcm0p/info",
            "/dev/snd/pcmC2D0p",
        ] {
            assert!(!proc_metadata(Path::new(name)));
        }
    }
    #[test]
    fn card_name_requires_matching_index_and_id() {
        let raw = " 0 [NVidia         ]: HDA-Intel - HDA NVidia\n                      PCI address\n 2 [AF16Rig        ]: USB-Audio - AF16Rig\n                      USB address";
        assert_eq!(
            card_description(raw, 0, "NVidia"),
            Some("HDA-Intel - HDA NVidia".into())
        );
        assert_eq!(
            card_description(raw, 2, "AF16Rig"),
            Some("USB-Audio - AF16Rig".into())
        );
        assert!(card_description(raw, 2, "OldDevice").is_none());
    }
    #[test]
    fn missing_card_parent_preserves_interface_number_without_borrowing_another_card() {
        use std::{collections::BTreeMap, os::unix::fs::symlink};
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().join("sound");
        std::fs::create_dir(&bus).unwrap();
        for parent in ["card9", "unavailable"] {
            let selected = tmp.path().join(parent).join("pcmC2D0p");
            std::fs::create_dir_all(&selected).unwrap();
            symlink(&bus, selected.join("subsystem")).unwrap();
            let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
            document.facts.clear();
            let mut reads = Reads::fixture(BTreeMap::new());
            collect(&mut document, &selected, &mut reads, &mut |_| {});
            let number = document
                .facts
                .iter()
                .find(|fact| fact.label == "ALSA card number")
                .unwrap();
            assert_eq!(number.value.as_deref(), Some("2"));
            assert_eq!(number.source, selected);
            assert!(
                document
                    .facts
                    .iter()
                    .any(|fact| fact.label == "ALSA card relationship"
                        && fact.outcome == Outcome::Missing)
            );
            assert!(
                !document
                    .facts
                    .iter()
                    .any(|fact| fact.label == "ALSA card ID" || fact.label == "Sound card name")
            );
            assert!(document.topology.is_none());
            assert!(reads.attempts().next().is_none());
        }
    }
    #[test]
    fn card_and_pcm_share_passive_metadata_without_opening_access_nodes() {
        use std::{collections::BTreeMap, os::unix::fs::symlink};
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().join("sound");
        let card = tmp.path().join("card2");
        let pcm = card.join("pcmC2D0p");
        std::fs::create_dir(&bus).unwrap();
        std::fs::create_dir_all(&pcm).unwrap();
        for path in [&card, &pcm] {
            symlink(&bus, path.join("subsystem")).unwrap();
        }
        for name in ["pcmC2D0c", "controlC2", "midiC2D1"] {
            std::fs::create_dir(card.join(name)).unwrap();
        }
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = pcm.clone();
        document.facts.clear();
        let mut reads = Reads::fixture(BTreeMap::from([
            (card.join("id"), b"TestCard".to_vec()),
            (
                PathBuf::from("/proc/asound/cards"),
                b" 2 [TestCard ]: USB-Audio - Test Interface".to_vec(),
            ),
            (
                PathBuf::from("/proc/asound/card2/pcm0p/info"),
                b"card: 2\ndevice: 0\nstream: PLAYBACK\nname: USB Audio".to_vec(),
            ),
        ]));
        collect(&mut document, &pcm, &mut reads, &mut |_| {});
        assert_eq!(document.title, "ALSA pcm playback");
        let topology = document.topology.as_ref().unwrap();
        assert!(topology.summary.contains("Test Interface"));
        assert!(
            topology
                .nodes
                .iter()
                .any(|n| n.label.contains("PCM capture"))
        );
        assert!(topology.nodes.iter().any(|n| n.label.contains("Raw MIDI")));
        assert!(
            document
                .facts
                .iter()
                .any(|f| f.label == "PCM stream information"
                    && f.value.as_deref().unwrap_or("").contains("PLAYBACK"))
        );
        assert!(
            reads
                .attempts()
                .all(|path| path == card.join("id") || proc_metadata(path))
        );
        assert!(reads.asound(Path::new("/dev/snd/pcmC2D0p")).is_err());
    }
}
