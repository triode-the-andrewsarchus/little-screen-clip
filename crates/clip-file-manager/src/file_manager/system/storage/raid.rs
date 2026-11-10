//! Passive MD member metadata, shared topology and explicit partial coverage.
use super::super::{
    Document, Relation,
    attributes::{Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::Fact,
};
use std::path::Path;

pub(super) fn collect(document: &mut Document, root: &Path, reads: &mut Reads) {
    let md = root.join("md");
    if !md.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(&md) {
        Ok(entries) => entries,
        Err(error) => {
            document
                .facts
                .push(Fact::failure("MD member inventory", md, error));
            return;
        }
    };
    let (members, coverage) = buses::select_children(
        entries,
        |entry| {
            entry.file_name().to_string_lossy().starts_with("dev-")
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
        },
        16,
    );
    let summary = ["RAID level", "Array state", "Degraded RAID roles"]
        .into_iter()
        .map(|label| {
            format!(
                "{label}: {}",
                document.observed_raw(label).unwrap_or("unavailable")
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let mut topology = Topology::new("MD RAID members", summary.clone());
    topology.coverage = coverage;
    let array = buses::add(&mut topology, document, root.into(), summary, None);
    for member in members {
        let name = member.file_name().unwrap_or_default().to_string_lossy();
        let state = reads.fact(
            &member,
            &Rule::new("state", &format!("{name} state"), Format::Text),
        );
        let slot = reads.fact(
            &member,
            &Rule::new("slot", &format!("{name} role"), Format::Text),
        );
        let label = format!(
            "{} · {} · role {}",
            name.trim_start_matches("dev-"),
            state.value.as_deref().unwrap_or("state unavailable"),
            slot.value.as_deref().unwrap_or("unavailable")
        );
        document.facts.extend([state, slot]);
        buses::add(&mut topology, document, member.clone(), label, Some(array));
        if let Ok(target) = member.join("block").canonicalize() {
            document
                .relations
                .push(Relation::new(format!("{name} block device"), target));
        }
    }
    document.facts.push(Fact::text("MD member interpretation",
        "Observed roles and state flags are snapshots; a spare or missing role is not proof of failed hardware. Recovery progress uses sectors, not bytes. No array controls or member devices were opened.", md));
    document.topology = Some(topology);
}

#[cfg(test)]
mod tests {
    use super::super::super::{
        attributes::{self, Context},
        collector,
        model::Outcome,
    };
    use super::*;
    use std::{collections::BTreeMap, path::PathBuf};

    fn fixture(count: usize) -> (tempfile::TempDir, PathBuf, Reads, Document) {
        let (temp, root, _, mut document) = collector::registered_fixture("block", "md0", &[]);
        let md = root.join("md");
        std::fs::create_dir(&md).unwrap();
        let mut values = BTreeMap::new();
        for (name, raw) in [
            ("level", "raid1"),
            ("array_state", "active"),
            ("raid_disks", "2"),
            ("degraded", "1"),
            ("sync_action", "recover"),
            ("sync_completed", "128 / 1024"),
        ] {
            values.insert(md.join(name), raw.as_bytes().to_vec());
        }
        for index in 0..count {
            let member = md.join(format!("dev-disk{index}"));
            std::fs::create_dir(&member).unwrap();
            values.insert(
                member.join("state"),
                if index == 0 {
                    b"in_sync".to_vec()
                } else {
                    b"spare".to_vec()
                },
            );
            values.insert(
                member.join("slot"),
                if index == 0 {
                    b"0".to_vec()
                } else {
                    b"none".to_vec()
                },
            );
        }
        let mut reads = Reads::fixture(values);
        let context = Context::discover(&root);
        for rule in attributes::rules(&context)
            .into_iter()
            .filter(|rule| rule.name.starts_with("md/"))
        {
            document.facts.push(reads.fact(&root, &rule));
        }
        (temp, root, reads, document)
    }

    #[test]
    fn array_member_map_preserves_spare_roles_and_recovery_sector_snapshot() {
        let (_temp, root, mut reads, mut document) = fixture(2);
        collect(&mut document, &root, &mut reads);
        assert_eq!(document.observed_raw("RAID level"), Some("raid1"));
        assert_eq!(document.observed_raw("Degraded RAID roles"), Some("1"));
        assert_eq!(
            document.observed_raw("Recovery/check action"),
            Some("recover")
        );
        assert_eq!(
            document.observed_raw("Recovery/check sectors"),
            Some("128 / 1024")
        );
        let topology = document.topology.as_ref().unwrap();
        assert_eq!(topology.coverage, Outcome::Observed);
        assert_eq!(topology.nodes.len(), 3);
        assert!(topology.nodes[1].label.contains("in_sync · role 0"));
        assert!(topology.nodes[2].label.contains("spare · role none"));
        assert_eq!(topology.nodes[2].parent, Some(0));
        assert!(reads.attempts().all(|path| matches!(
            path.file_name().unwrap().to_str().unwrap(),
            "level"
                | "array_state"
                | "raid_disks"
                | "degraded"
                | "sync_action"
                | "sync_completed"
                | "state"
                | "slot"
        )));
    }

    #[test]
    fn member_inventory_is_bounded_and_discloses_partial_coverage() {
        let (_temp, root, mut reads, mut document) = fixture(20);
        collect(&mut document, &root, &mut reads);
        let topology = document.topology.as_ref().unwrap();
        assert_eq!(topology.coverage, Outcome::Truncated);
        assert_eq!(topology.nodes.len(), 17);
        assert_eq!(
            document
                .facts
                .iter()
                .filter(|fact| fact.label.ends_with(" role"))
                .count(),
            16
        );
    }

    #[test]
    fn md_directory_and_member_schemas_exclude_controls_and_unrelated_attributes() {
        let (_temp, root, _reads, _document) = fixture(1);
        let array = Context::discover(&root.join("md"));
        assert!(attributes::rule(&array, "array_state").is_some());
        assert!(attributes::rule(&array, "level").is_some());
        assert!(attributes::rule(&array, "size").is_none());
        for name in ["new_dev", "safe_mode_delay", "remove", "bitmap/location"] {
            assert!(attributes::rule(&array, name).is_none());
        }
        let member = Context::discover(&root.join("md/dev-disk0"));
        assert!(attributes::rule(&member, "state").is_some());
        assert!(attributes::rule(&member, "slot").is_some());
        assert!(attributes::rule(&member, "size").is_none());
        assert!(attributes::rule(&member, "array_state").is_none());
    }
}
