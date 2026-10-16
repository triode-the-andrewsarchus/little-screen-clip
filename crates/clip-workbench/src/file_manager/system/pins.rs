//! Existing GPIO/PWM registrations and exported configuration; no controls.
use super::{
    Document, Relation,
    attributes::{Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use std::{io, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Object {
    GpioChip(bool),
    GpioLine(u32),
    PwmChip,
    PwmChannel(u32),
}

fn number(name: &str, prefix: &str) -> Option<u32> {
    let suffix = name.strip_prefix(prefix)?;
    (!suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| suffix.parse().ok())
        .flatten()
}
fn object(path: &Path) -> Option<Object> {
    let name = path.file_name()?.to_str()?;
    let subsystem = path.join("subsystem").canonicalize().ok();
    match subsystem
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
    {
        Some("gpio") => {
            let legacy = subsystem.as_ref()?.parent()?.file_name()? == "class";
            if number(name, "gpiochip").is_some() {
                Some(Object::GpioChip(legacy))
            } else if legacy {
                number(name, "gpio")
                    .filter(|id| *id <= i32::MAX as u32)
                    .map(Object::GpioLine)
            } else {
                None
            }
        }
        Some("pwm") if number(name, "pwmchip").is_some() => Some(Object::PwmChip),
        _ if number(name, "pwm").is_some()
            && path.parent().and_then(object) == Some(Object::PwmChip) =>
        {
            number(name, "pwm").map(Object::PwmChannel)
        }
        _ => None,
    }
}

pub(super) fn rules(path: &Path) -> Vec<Rule> {
    use Format::*;
    let specs: &[(&str, &str, Format)] = match object(path) {
        Some(Object::GpioChip(false)) => &[("dev", "GPIO device number", Text)],
        Some(Object::GpioChip(true)) => &[
            ("label", "GPIO controller label", Text),
            ("base", "Legacy GPIO base", Unsigned),
            ("ngpio", "GPIO line count", Unsigned),
        ],
        Some(Object::GpioLine(_)) => &[
            ("direction", "GPIO direction", Text),
            (
                "value",
                "GPIO logical value (active-low applies)",
                Boolean("1", "0"),
            ),
            (
                "active_low",
                "GPIO active-low configuration",
                Boolean("Yes", "No"),
            ),
            ("edge", "GPIO edge configuration", Text),
        ],
        Some(Object::PwmChip) => &[("npwm", "PWM channel count", Unsigned)],
        Some(Object::PwmChannel(_)) => &[
            ("period", "PWM configured period (ns)", Unsigned),
            ("duty_cycle", "PWM configured duty (ns)", Unsigned),
            ("polarity", "PWM configured polarity", Text),
            ("enable", "PWM enable flag", Boolean("Enabled", "Disabled")),
        ],
        None => &[],
    };
    super::attributes::materialize_specs(specs)
}

pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let Some(selected) = object(sys) else {
        return;
    };
    let name = sys.file_name().unwrap_or_default().to_string_lossy();
    document.title = match selected {
        Object::GpioChip(false) => "GPIO character-device registration",
        Object::GpioChip(true) => "Legacy GPIO controller",
        Object::GpioLine(_) => "Exported GPIO line",
        Object::PwmChip => "PWM controller",
        Object::PwmChannel(_) => "Exported PWM channel",
    }
    .into();
    for rule in rules(sys) {
        reads.observe(document, sys, &rule);
    }
    let mut topology = Topology::new("GPIO/PWM registration", name.to_string());
    let parents = buses::ancestors(sys, 12, |path| {
        matches!(
            buses::subsystem(path).as_deref(),
            Some("platform" | "pci" | "i2c" | "spi" | "gpio" | "pwm")
        )
    });
    let parent = buses::ancestor_chain(&mut topology, document, parents, None);
    let anchor = buses::add(
        &mut topology,
        document,
        sys.into(),
        name.to_string(),
        parent,
    );
    for (field, label) in [
        ("dev", "device"),
        ("direction", "direction"),
        ("value", "logical value"),
        ("active_low", "active-low"),
        ("edge", "edge"),
    ] {
        if let Some(value) = document
            .facts
            .iter()
            .find(|fact| {
                fact.source == sys.join(field)
                    && fact.outcome == Outcome::Observed
                    && !fact.retained
            })
            .and_then(|fact| fact.value.as_ref())
        {
            topology.summary.push_str(&format!(" · {label} {value}"));
        }
    }
    buses::access_node(document, sys, "GPIO access interface");
    for field in ["firmware_node", "of_node", "device"] {
        if let Ok(path) = sys.join(field).canonicalize() {
            document
                .relations
                .push(Relation::new(format!("GPIO/PWM {field} binding"), path));
        }
    }
    match selected {
        Object::GpioChip(false) => document.facts.push(Fact::text("GPIO observation policy","Registered access and parent metadata only; chip/line ioctl and event streams not opened",sys.into())),
        Object::GpioChip(true) => {
            if let Ok(label) = reads.read(&sys.join("label")) { topology.summary.push_str(&format!(" · {label}")); }
            document.facts.push(Fact::text("GPIO export scope","Existing legacy exports only; unexported lines and consumers not queried",sys.into()));
        }
        Object::GpioLine(id) => {
            document.facts.insert(0,Fact::text("Legacy exported GPIO number",id.to_string(),sys.into()));
            gpio_controller(document,sys,id,reads,&mut topology,anchor);
        }
        Object::PwmChip => channels(document,sys,&mut topology,anchor,checkpoint),
        Object::PwmChannel(channel) => {
            document.facts.insert(0,Fact::text("Exported PWM channel",channel.to_string(),sys.into()));
            pwm_state(document,sys,reads,&mut topology);
        }
    }
    document.topology = Some(topology);
    document.facts.sort_by_key(|fact| fact.interface);
    checkpoint(document);
}

fn timing(period: u64, duty: u64) -> io::Result<String> {
    if period == 0 && duty == 0 {
        return Ok("No nonzero timing configured".into());
    }
    if period == 0 || duty > period {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Incoherent PWM period/duty observations",
        ));
    }
    let hz = 1e9 / period as f64;
    let frequency = if hz >= 1e6 {
        format!("{:.3} MHz", hz / 1e6)
    } else if hz >= 1e3 {
        format!("{:.3} kHz", hz / 1e3)
    } else if hz >= 1.0 {
        format!("{hz:.3} Hz")
    } else {
        format!("{hz:.3e} Hz")
    };
    Ok(format!(
        "Configured {frequency} · {:.1}% duty",
        100.0 * duty as f64 / period as f64
    ))
}
fn pwm_state(document: &mut Document, sys: &Path, reads: &mut Reads, topology: &mut Topology) {
    let result = reads.unsigned(&sys.join("period")).and_then(|period| {
        reads
            .unsigned(&sys.join("duty_cycle"))
            .and_then(|duty| timing(period, duty))
    });
    match result {
        Ok(summary) => {
            topology.summary.push_str(&format!(" · {summary}"));
            document
                .facts
                .push(Fact::text("PWM timing summary", summary, sys.into()));
        }
        Err(error) => document
            .facts
            .push(Fact::failure("PWM timing summary", sys.into(), error)),
    }
    if let Some(enable) = document
        .facts
        .iter()
        .find(|fact| {
            fact.source == sys.join("enable") && fact.outcome == Outcome::Observed && !fact.retained
        })
        .and_then(|fact| fact.value.as_deref())
    {
        topology.summary.push_str(&format!(" · {enable}"));
    }
    document.facts.push(Fact::text("PWM observation policy","Cached requested configuration, not measured output; capture/export/output controls not invoked",sys.into()));
}

fn channels(
    document: &mut Document,
    sys: &Path,
    topology: &mut Topology,
    anchor: usize,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let entries = match std::fs::read_dir(sys) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                "PWM exported-channel inventory",
                sys.into(),
                error,
            ));
            topology.coverage = Outcome::Unsupported;
            return;
        }
    };
    let (paths, coverage) = buses::select_children(
        entries,
        |entry| matches!(object(&entry.path()), Some(Object::PwmChannel(_))),
        16,
    );
    topology.coverage = coverage.clone();
    let mut fact = Fact::text(
        "PWM exported-channel inventory",
        format!(
            "{} observed exports; not an inventory of all requested or active channels",
            paths.len()
        ),
        sys.into(),
    );
    fact.outcome = coverage;
    document.facts.push(fact);
    for path in paths {
        let label = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        buses::add(topology, document, path.clone(), label, Some(anchor));
        checkpoint(document);
    }
    if let Some(count) = document.observed_raw("PWM channel count") {
        topology
            .summary
            .push_str(&format!(" · {count} supported channels"));
    }
}

fn gpio_controller(
    document: &mut Document,
    sys: &Path,
    line: u32,
    reads: &mut Reads,
    topology: &mut Topology,
    anchor: usize,
) {
    let Some(class) = sys.join("subsystem").canonicalize().ok() else {
        return;
    };
    let entries = match std::fs::read_dir(&class) {
        Ok(entries) => entries,
        Err(error) => {
            let fact = Fact::failure("GPIO controller inventory", class, error);
            topology.coverage = fact.outcome.clone();
            document.facts.push(fact);
            return;
        }
    };
    let (paths, coverage) = buses::select_children(
        entries,
        |entry| object(&entry.path()) == Some(Object::GpioChip(true)),
        16,
    );
    if coverage != Outcome::Observed {
        topology.coverage = coverage;
    }
    for path in paths {
        let range = reads.unsigned(&path.join("base")).and_then(|base| {
            reads
                .unsigned(&path.join("ngpio"))
                .map(|count| (base, count))
        });
        match range.and_then(|(base, count)| line_offset(base, count, line)) {
            Ok(Some(offset)) => {
                let path = path.canonicalize().unwrap_or(path);
                buses::add(
                    topology,
                    document,
                    path.clone(),
                    format!("Legacy controller · line offset {offset}"),
                    Some(anchor),
                );
                return;
            }
            Ok(None) => {}
            Err(error) => {
                let fact = Fact::failure("GPIO controller geometry", path, error);
                topology.coverage = fact.outcome.clone();
                document.facts.push(fact);
            }
        }
    }
    document.facts.push(Fact::text(
        "GPIO controller association",
        "No containing controller observed in the bounded legacy inventory",
        sys.into(),
    ));
}

fn line_offset(base: u64, count: u64, line: u32) -> io::Result<Option<u64>> {
    let end = base
        .checked_add(count)
        .filter(|end| count > 0 && *end <= i32::MAX as u64 + 1)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid legacy GPIO numbering range",
            )
        })?;
    Ok((base <= u64::from(line) && u64::from(line) < end).then(|| u64::from(line) - base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};

    #[test]
    fn exported_gpio_lines_show_logical_configuration_and_controller_offsets() {
        let temp = tempfile::tempdir().unwrap();
        let class = temp.path().join("class/gpio");
        let chip = class.join("gpiochip512");
        let line = class.join("gpio515");
        for path in [&chip, &line] {
            std::fs::create_dir_all(path).unwrap();
            symlink(&class, path.join("subsystem")).unwrap();
        }
        let mut reads = Reads::fixture(BTreeMap::from([
            (chip.join("base"), b"512".to_vec()),
            (chip.join("ngpio"), b"16".to_vec()),
            (line.join("direction"), b"in".to_vec()),
            (line.join("value"), b"1".to_vec()),
            (line.join("active_low"), b"1".to_vec()),
            (line.join("edge"), b"rising".to_vec()),
            (class.join("export"), b"must not be read".to_vec()),
        ]));
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect(&mut doc, &line, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "Exported GPIO line");
        assert!(doc.facts.iter().any(|fact| fact.label
            == "GPIO logical value (active-low applies)"
            && fact.value.as_deref() == Some("1")));
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "GPIO active-low configuration"
                    && fact.value.as_deref() == Some("Yes"))
        );
        assert!(
            doc.topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.path == chip && node.label.contains("offset 3"))
        );
        assert!(
            reads
                .attempts()
                .all(|path| path != class.join("export") && path != class.join("unexport"))
        );
    }

    #[test]
    fn pwm_channels_show_requested_timing_and_exclude_capture_and_exports() {
        let temp = tempfile::tempdir().unwrap();
        let class = temp.path().join("class/pwm");
        let chip = temp.path().join("device/pwmchip0");
        let channel = chip.join("pwm1");
        std::fs::create_dir_all(&class).unwrap();
        std::fs::create_dir_all(&channel).unwrap();
        symlink(&class, chip.join("subsystem")).unwrap();
        let values = BTreeMap::from([
            (chip.join("npwm"), b"4".to_vec()),
            (channel.join("period"), b"50000".to_vec()),
            (channel.join("duty_cycle"), b"17500".to_vec()),
            (channel.join("enable"), b"1".to_vec()),
            (channel.join("polarity"), b"normal".to_vec()),
            (channel.join("capture"), b"must not be read".to_vec()),
        ]);
        let mut reads = Reads::fixture(values.clone());
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect(&mut doc, &channel, &mut reads, &mut |_| {});
        let summary = &doc.topology.as_ref().unwrap().summary;
        assert!(summary.contains("20.000 kHz") && summary.contains("35.0%"));
        assert!(summary.contains("Enabled"));
        let mut disabled_values = values;
        disabled_values.insert(channel.join("enable"), b"0".to_vec());
        let mut disabled = doc.clone();
        disabled.facts.clear();
        let mut disabled_reads = Reads::fixture(disabled_values);
        collect(&mut disabled, &channel, &mut disabled_reads, &mut |_| {});
        assert!(
            disabled
                .topology
                .as_ref()
                .unwrap()
                .summary
                .contains("Disabled")
        );
        let enable = disabled
            .facts
            .iter_mut()
            .find(|fact| fact.source == channel.join("enable"))
            .unwrap();
        enable.retained = true;
        enable.outcome = Outcome::PermissionDenied;
        let mut retained = Topology::new("PWM", String::new());
        pwm_state(&mut disabled, &channel, &mut disabled_reads, &mut retained);
        assert!(!retained.summary.contains("Disabled"));
        assert!(!retained.summary.contains("Enabled"));
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("capture" | "export" | "unexport")
        )));
        collect(&mut doc, &chip, &mut reads, &mut |_| {});
        assert!(
            doc.topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.path == channel)
        );
        assert!(rules(&channel).iter().all(|rule| rule.name != "capture"));
    }

    #[test]
    fn modern_gpio_registration_does_not_claim_legacy_numbers_or_line_state() {
        let temp = tempfile::tempdir().unwrap();
        let bus = temp.path().join("bus/gpio");
        let chip = temp.path().join("platform/gpiochip0");
        std::fs::create_dir_all(&bus).unwrap();
        std::fs::create_dir_all(&chip).unwrap();
        symlink(&bus, chip.join("subsystem")).unwrap();
        assert_eq!(object(&chip), Some(Object::GpioChip(false)));
        let mut reads = Reads::fixture(BTreeMap::from([(chip.join("dev"), b"254:0".to_vec())]));
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect(&mut doc, &chip, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "GPIO character-device registration");
        assert!(rules(&chip).iter().all(|rule| rule.name == "dev"));
        assert!(reads.attempts().all(|path| path == chip.join("dev")));
    }

    #[test]
    fn timing_and_name_validation_do_not_invent_invalid_output_states() {
        assert_eq!(line_offset(512, 16, 515).unwrap(), Some(3));
        assert_eq!(line_offset(512, 16, 528).unwrap(), None);
        assert!(line_offset(u64::MAX, 2, 1).is_err());
        assert!(line_offset(0, 0, 0).is_err());
        assert!(timing(50000, 17500).unwrap().contains("35.0%"));
        assert!(timing(10, 10).unwrap().contains("100.0%"));
        assert!(timing(0, 0).unwrap().contains("No nonzero timing"));
        assert!(timing(0, 1).is_err());
        assert!(timing(10, 11).is_err());
        assert!(timing(u64::MAX, u64::MAX).unwrap().contains("e-"));
        for name in ["gpio-1", "gpio", "gpio2147483648", "pwmchipnotanumber"] {
            assert!(number(name, "gpio").is_none_or(|id| id > i32::MAX as u32));
        }
        assert!(rules(Path::new("/unregistered/pwmchip0/pwm1")).is_empty());
    }
}
