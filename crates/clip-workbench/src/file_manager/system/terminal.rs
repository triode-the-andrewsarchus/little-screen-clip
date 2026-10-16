//! Passive VT status; an inactive VT is not necessarily unallocated.
use super::{
    Document,
    attributes::{Format, Rule},
    buses,
    collector::Reads,
    identity::MountIdentity,
    model::{Fact, Value},
};
use std::{io, path::Path};

pub(super) fn rules(path: &Path) -> Vec<Rule> {
    match path.file_name().and_then(|name| name.to_str()) {
        Some("tty0") => vec![Rule::new("active", "Current virtual console", Format::Text)],
        Some("console") => vec![Rule::new(
            "active",
            "Configured system consoles",
            Format::Text,
        )],
        _ => Vec::new(),
    }
}

fn vt_number(name: &str) -> Option<u8> {
    let digits = name.strip_prefix("tty")?;
    let number: u8 = digits.parse().ok()?;
    (number <= 63 && digits == number.to_string()).then_some(number)
}

pub(super) fn collect(document: &mut Document, sys: &Path, reads: &mut Reads) {
    if buses::subsystem(sys).as_deref() != Some("tty") {
        return;
    }
    let Some(number) = sys
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(vt_number)
    else {
        return;
    };
    let source = Path::new("/sys/class/tty/tty0/active").to_path_buf();
    let result = reads.read(&source).and_then(|value| {
        vt_number(&value)
            .filter(|number| *number != 0)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid active virtual-console identity",
                )
            })
    });
    let selected = if number == 0 {
        result.as_ref().ok().copied()
    } else {
        Some(number)
    };
    let fact = match result {
        Ok(active) => {
            let status = if number == 0 {
                format!("Current-console alias → tty{active}")
            } else if number == active {
                format!("tty{number} · active virtual console")
            } else {
                format!("tty{number} · not active; current VT is tty{active}")
            };
            document.title = if number == 0 {
                "Current virtual-console interface".into()
            } else {
                format!("Virtual console {number}")
            };
            let mut fact = Fact::text("Virtual-console activity", status, source);
            fact.semantic = Some(Value::Integer(active.into()));
            fact
        }
        Err(error) => Fact::failure("Virtual-console activity", source, error),
    };
    document.facts.insert(0, fact);
    match selected {
        Some(selected) => {
            let root = Path::new("/sys/class/vc").join(format!("vcs{selected}"));
            allocation(document, &root, selected, reads);
        }
        None => document.facts.push(Fact::text(
            "Virtual-console allocation",
            "Not determined: current-console identity is unavailable. No console device was opened.",
            sys.into(),
        )),
    }
}

/// Resolve only a current-console alias from a successful, unretained observation.
pub(super) fn active_vt(document: &Document) -> Option<u8> {
    use little_clip_core::kernel_interface::{Filesystem, Kind};
    let alias = match document.kind {
        Kind::Character { major: 4, minor: 0 } => true,
        Kind::Kernel {
            filesystem: Filesystem::Sysfs,
        } => {
            document.interface_directory
                && document.path.file_name().is_some_and(|name| name == "tty0")
        }
        _ => false,
    };
    if !alias {
        return None;
    }
    document.facts.iter().find_map(|fact| {
        if fact.label != "Virtual-console activity"
            || fact.source != Path::new("/sys/class/tty/tty0/active")
            || fact.retained
            || fact.outcome != super::model::Outcome::Observed
        {
            return None;
        }
        match fact.semantic {
            Some(Value::Integer(number)) if (1..=63).contains(&number) => Some(number as u8),
            _ => None,
        }
    })
}

fn allocation(document: &mut Document, root: &Path, number: u8, reads: &mut Reads) {
    let source = root.join("dev");
    let result = (|| {
        let before = MountIdentity::directory(root)?;
        if buses::subsystem(root).as_deref() != Some("vc") {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Not a VC registration",
            ));
        }
        let reported = reads.read(&source)?;
        if reported != format!("7:{number}") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "VC registration identity mismatch",
            ));
        }
        let physical_source = root.canonicalize()?.join("dev");
        if MountIdentity::directory(root)? != before {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "VC registration changed during observation",
            ));
        }
        Ok(physical_source)
    })();
    let fact = match result {
        Ok(physical_source) => Fact::text(
            "Virtual-console allocation",
            format!(
                "tty{number} · allocated console (observed kernel VC registration). No screen contents read."
            ),
            physical_source,
        ),
        Err(error) => {
            let mut fact = Fact::failure("Virtual-console allocation", source, error);
            let reason = fact.error.as_deref().unwrap_or("Registration unavailable");
            fact.error = Some(format!(
                "Not determined: {reason}. Missing registration does not prove an unused console."
            ));
            fact
        }
    };
    document.facts.push(fact);
}

#[cfg(test)]
mod tests {
    use super::super::{collector::registered_fixture, model::Outcome};
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn virtual_console_identity_rejects_serial_ptys_and_noncanonical_names() {
        for name in ["tty0", "tty1", "tty63"] {
            assert!(vt_number(name).is_some());
        }
        for name in [
            "tty64", "tty01", "tty+1", "ttyUSB0", "ttyS0", "pts/1", "tty",
        ] {
            assert!(vt_number(name).is_none());
        }
    }

    #[test]
    fn activity_is_scoped_to_current_vt_without_inferring_allocation() {
        for (name, expected) in [
            ("tty0", "Current-console alias → tty2"),
            ("tty2", "tty2 · active virtual console"),
            ("tty3", "tty3 · not active; current VT is tty2"),
        ] {
            let (_temp, root, _, mut document) = registered_fixture("tty", name, &[]);
            document.facts.clear();
            let source = Path::new("/sys/class/tty/tty0/active").to_path_buf();
            let mut reads = Reads::fixture(BTreeMap::from([(source.clone(), b"tty2\n".to_vec())]));
            collect(&mut document, &root, &mut reads);
            assert_eq!(document.facts[0].value.as_deref(), Some(expected));
            assert_eq!(document.facts[0].source, source);
            assert_eq!(document.facts[0].outcome, Outcome::Observed);
            assert_eq!(document.facts[1].label, "Virtual-console allocation");
            assert_eq!(
                document.facts[1].source,
                Path::new("/sys/class/vc")
                    .join(format!("vcs{}/dev", if name == "tty3" { 3 } else { 2 }))
            );
        }
    }

    #[test]
    fn unavailable_or_malformed_activity_remains_unknown() {
        let (_temp, root, _, mut document) = registered_fixture("tty", "tty1", &[]);
        for raw in [None, Some("tty0"), Some("tty1 tty2"), Some("tty64")] {
            document.facts.clear();
            let mut reads = Reads::fixture(
                raw.into_iter()
                    .map(|value| {
                        (
                            Path::new("/sys/class/tty/tty0/active").to_path_buf(),
                            value.as_bytes().to_vec(),
                        )
                    })
                    .collect(),
            );
            collect(&mut document, &root, &mut reads);
            assert!(document.facts[0].value.is_none());
            assert_eq!(
                document.facts[0].outcome,
                if raw.is_none() {
                    Outcome::Missing
                } else {
                    Outcome::Malformed
                }
            );
        }
    }

    #[test]
    fn active_alias_context_rejects_stale_invalid_and_specific_terminal_facts() {
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.kind = little_clip_core::kernel_interface::Kind::Character { major: 4, minor: 0 };
        let mut fact = Fact::text(
            "Virtual-console activity",
            "fixture",
            "/sys/class/tty/tty0/active".into(),
        );
        fact.semantic = Some(Value::Integer(2));
        document.facts = vec![fact];
        assert_eq!(active_vt(&document), Some(2));
        document.facts[0].retained = true;
        assert_eq!(active_vt(&document), None);
        document.facts[0].retained = false;
        document.facts[0].outcome = Outcome::Malformed;
        assert_eq!(active_vt(&document), None);
        document.facts[0].outcome = Outcome::Observed;
        for number in [0, 64, -1] {
            document.facts[0].semantic = Some(Value::Integer(number));
            assert_eq!(active_vt(&document), None);
        }
        document.facts[0].semantic = Some(Value::Integer(2));
        document.kind = little_clip_core::kernel_interface::Kind::Character { major: 4, minor: 1 };
        assert_eq!(active_vt(&document), None);
        document.kind = little_clip_core::kernel_interface::Kind::Kernel {
            filesystem: little_clip_core::kernel_interface::Filesystem::Sysfs,
        };
        document.path = "/sys/devices/virtual/tty/tty0".into();
        document.interface_directory = true;
        assert_eq!(active_vt(&document), Some(2));
        document.interface_directory = false;
        assert_eq!(active_vt(&document), None);
    }

    #[test]
    fn allocation_requires_matching_registered_screen_identity() {
        for (raw, expected) in [
            ("7:2", Outcome::Observed),
            ("7:3", Outcome::Malformed),
            ("7:02", Outcome::Malformed),
        ] {
            let (_temp, root, mut reads, mut document) =
                registered_fixture("vc", "vcs2", &[("dev", raw)]);
            document.facts.clear();
            allocation(&mut document, &root, 2, &mut reads);
            assert_eq!(document.facts[0].outcome, expected);
            assert_eq!(document.facts[0].source, root.join("dev"));
            if expected == Outcome::Observed {
                assert!(
                    document.facts[0]
                        .value
                        .as_deref()
                        .unwrap()
                        .contains("allocated console")
                );
            } else {
                assert!(document.facts[0].value.is_none());
                assert!(
                    document.facts[0]
                        .error
                        .as_deref()
                        .unwrap()
                        .contains("Not determined")
                );
            }
        }
    }

    #[test]
    fn absent_and_wrong_subsystem_registrations_never_claim_unused() {
        let (temp, root, mut reads, mut document) =
            registered_fixture("tty", "vcs2", &[("dev", "7:2")]);
        document.facts.clear();
        allocation(&mut document, &root, 2, &mut reads);
        assert_eq!(document.facts[0].outcome, Outcome::Unsupported);
        assert!(reads.attempts().next().is_none());
        document.facts.clear();
        allocation(&mut document, &temp.path().join("absent"), 2, &mut reads);
        assert_eq!(document.facts[0].outcome, Outcome::Missing);
        assert!(document.facts[0].value.is_none());
        assert!(
            document.facts[0]
                .error
                .as_deref()
                .unwrap()
                .contains("does not prove an unused console")
        );
    }

    #[test]
    fn only_documented_active_attributes_are_allowed() {
        for (name, count) in [("tty0", 1), ("console", 1), ("tty1", 0), ("ttyUSB0", 0)] {
            assert_eq!(rules(Path::new(name)).len(), count);
        }
    }
}
