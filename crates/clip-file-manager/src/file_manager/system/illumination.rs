//! LED/backlight observations and standardized platform-profile interfaces.
use super::{
    Document,
    attributes::{Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::Fact,
};
use std::path::Path;

pub(super) fn legacy_profile_root(path: &Path) -> bool {
    legacy_profile_root_at(path, Path::new("/sys/firmware/acpi"))
}
fn legacy_profile_root_at(path: &Path, reference: &Path) -> bool {
    super::boot_firmware::same_object(path, reference)
        && ["platform_profile", "platform_profile_choices"]
            .iter()
            .any(|name| {
                std::fs::symlink_metadata(path.join(name))
                    .is_ok_and(|metadata| metadata.file_type().is_file())
            })
}
pub(super) fn rules(path: &Path) -> Vec<Rule> {
    rules_at(path, Path::new("/sys/firmware/acpi"))
}
fn rules_at(path: &Path, legacy: &Path) -> Vec<Rule> {
    use Format::{Selection as S, Text as T, Unsigned as U};
    let specs: &[(&str, &str, Format)] = match buses::subsystem(path).as_deref() {
        Some("leds") => &[
            ("brightness", "Reported LED brightness", U),
            ("max_brightness", "Maximum brightness", U),
            ("trigger", "LED trigger and choices", S),
        ],
        Some("backlight") => &[
            ("brightness", "Requested brightness", U),
            ("actual_brightness", "Actual brightness", U),
            ("max_brightness", "Maximum brightness", U),
            ("type", "Backlight type", T),
        ],
        Some("platform-profile") => &[
            ("name", "Profile provider", T),
            ("profile", "Current platform profile", T),
            ("choices", "Available platform profiles", S),
        ],
        _ if legacy_profile_root_at(path, legacy) => &[
            ("platform_profile", "Current platform profile", T),
            ("platform_profile_choices", "Available platform profiles", S),
        ],
        _ => &[],
    };
    super::attributes::materialize_specs(specs)
}
pub(super) fn collect(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let subsystem = buses::subsystem(root);
    let profile = subsystem.as_deref() == Some("platform-profile") || legacy_profile_root(root);
    if !profile && !matches!(subsystem.as_deref(), Some("leds" | "backlight")) {
        return;
    }
    if profile {
        for rule in rules(root) {
            reads.observe(document, root, &rule);
            checkpoint(document);
        }
        document.title = "Platform profile observations".into();
        let mut summary = root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for label in [
            "Profile provider",
            "Current platform profile",
            "Available platform profiles",
        ] {
            if let Some(value) = document.observed_raw(label) {
                summary.push_str(&format!(" · {value}"));
            }
        }
        let mut topology = Topology::new("Platform profile provider", summary);
        let parents = buses::ancestors(root, 12, |path| {
            matches!(
                buses::subsystem(path).as_deref(),
                Some("platform" | "acpi" | "pci")
            )
        });
        let parent = buses::ancestor_chain(&mut topology, document, parents, None);
        buses::add(
            &mut topology,
            document,
            root.into(),
            "Profile interface".into(),
            parent,
        );
        document.topology = Some(topology);
    }
    buses::link_relations(
        document,
        root,
        &["device", "driver", "firmware_node", "of_node"],
        "Illumination/profile",
    );
    document.facts.push(Fact::text("Illumination/profile observation policy", "Documented read-only observations; getters may query hardware or firmware. No brightness, trigger, blanking or platform-profile changes", root.into()));
    checkpoint(document);
}

#[cfg(test)]
mod tests {
    use super::super::{attributes::Context, telemetry};
    use super::*;
    use std::{os::unix::fs::symlink, path::PathBuf};

    fn fixture(
        subsystem: &str,
        name: &str,
        values: &[(&str, &str)],
    ) -> (tempfile::TempDir, PathBuf, Document) {
        let (temp, root, mut reads, mut doc) =
            super::super::collector::registered_fixture(subsystem, name, values);
        telemetry::collect(&mut doc, &root, &Context::discover(&root), &mut reads);
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("bl_power" | "brightness_hw_changed" | "delay_on" | "delay_off")
        )));
        (temp, root, doc)
    }
    #[test]
    fn led_reports_level_and_current_trigger_without_treating_trigger_as_output() {
        let (_temp, root, doc) = fixture(
            "leds",
            "input3::capslock",
            &[
                ("brightness", "0"),
                ("max_brightness", "1"),
                ("trigger", "none [kbd-capslock] timer"),
            ],
        );
        let data = doc.telemetry.as_ref().unwrap();
        assert!(data.summary.contains("Off (reported)"));
        assert!(data.summary.contains("Trigger: kbd-capslock"));
        assert!(
            data.series[0]
                .notes
                .iter()
                .any(|note| note.contains("none / kbd-capslock / timer")
                    || note.contains("available: none kbd-capslock timer"))
        );
        assert_eq!(data.series[0].range, Some((0.0, 1.0)));
        let nested = root.join("private");
        std::fs::create_dir(&nested).unwrap();
        assert!(rules(&nested).is_empty());
    }
    #[test]
    fn requested_and_actual_backlight_levels_are_separate_non_luminance_observations() {
        let (_temp, _, doc) = fixture(
            "backlight",
            "panel",
            &[
                ("brightness", "65"),
                ("actual_brightness", "60"),
                ("max_brightness", "100"),
                ("type", "raw"),
            ],
        );
        let data = doc.telemetry.as_ref().unwrap();
        assert_eq!(data.series.len(), 2);
        assert!(data.series[0].current.display.contains("65.0%"));
        assert!(data.series[1].current.display.contains("60.0%"));
        assert!(data.series[0].notes[0].contains("not calibrated luminance"));
        assert!(data.summary.contains("Requested:"));
        assert!(data.summary.contains("Actual:"));
    }
    #[test]
    fn invalid_brightness_or_missing_maximum_never_gets_a_percentage() {
        for values in [
            vec![("brightness", "2"), ("max_brightness", "1")],
            vec![("brightness", "0"), ("max_brightness", "0")],
            vec![("brightness", "-1"), ("max_brightness", "100")],
            vec![("brightness", "10")],
        ] {
            let (_temp, _, doc) = fixture("backlight", "panel", &values);
            let data = doc.telemetry.as_ref().unwrap();
            assert!(data.series.iter().all(|row| row.range.is_none()));
            assert!(
                data.series
                    .iter()
                    .all(|row| !row.current.display.contains('%'))
            );
        }
    }
    #[test]
    fn profile_class_and_legacy_alias_have_bounded_observational_schemas() {
        let (temp, root, doc) = fixture(
            "platform-profile",
            "platform-profile-0",
            &[
                ("name", "example-driver"),
                ("profile", "balanced"),
                ("choices", "quiet balanced performance"),
            ],
        );
        assert!(doc.topology.as_ref().unwrap().summary.contains("balanced"));
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Available platform profiles")
        );
        let legacy = temp.path().join("acpi");
        std::fs::create_dir(&legacy).unwrap();
        assert!(!legacy_profile_root_at(&legacy, &legacy));
        std::fs::write(legacy.join("platform_profile"), "custom").unwrap();
        let alias = temp.path().join("alias");
        symlink(&legacy, &alias).unwrap();
        assert!(legacy_profile_root_at(&alias, &legacy));
        assert_eq!(rules_at(&alias, &legacy).len(), 2);
        assert!(!legacy_profile_root_at(&root, &legacy));
        let nested = root.join("private");
        std::fs::create_dir(&nested).unwrap();
        assert!(rules(&nested).is_empty());
    }
}
