//! Typed observations preserve uncertainty independently of formatted UI values.
use serde::{Deserialize, Serialize};
use std::{
    fmt, io,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::file_manager) enum Outcome {
    Observed,
    PermissionDenied,
    Missing,
    Unsupported,
    Disappeared,
    TimedOut,
    Truncated,
    Malformed,
    NotRequested,
}
impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Observed => "Observed",
                Self::PermissionDenied => "Permission denied",
                Self::Missing => "Not exposed",
                Self::Unsupported => "Unsupported",
                Self::Disappeared => "Object disappeared",
                Self::TimedOut => "Timed out",
                Self::Truncated => "Truncated",
                Self::Malformed => "Invalid attribute data",
                Self::NotRequested => "Not requested",
            }
        )
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::file_manager) enum Value {
    Text(String),
    Boolean(bool),
    Integer(i128),
    Measurement {
        value: f64,
        unit: String,
    },
    CpuList(Vec<(u32, u32)>),
    Selection {
        selected: Option<String>,
        available: Vec<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Fact {
    #[serde(default)]
    pub retained: bool,
    #[serde(default)]
    pub interface: bool,
    pub label: String,
    pub value: Option<String>,
    pub raw: Option<String>,
    pub semantic: Option<Value>,
    pub outcome: Outcome,
    pub error: Option<String>,
    #[serde(with = "super::path_serde")]
    pub source: PathBuf,
    pub origin: String,
    pub observed_ms: u64,
    #[serde(default)]
    pub observed_generation: Option<u64>,
    // Preserved through helper JSON; older snapshots default to unknown provenance.
    #[serde(default)]
    generation_pending: bool,
}
impl Fact {
    /// Preserve source provenance, including unknown generations from older snapshots.
    pub(super) fn inherit_observation(&mut self, source: &Self) {
        self.observed_ms = source.observed_ms;
        self.observed_generation = source.observed_generation;
        self.generation_pending = source.generation_pending;
    }
    fn same_observation_key(&self, other: &Self) -> bool {
        self.source == other.source
            && self.origin == other.origin
            && (self.origin == "documented sysfs attribute" || self.label == other.label)
    }
    pub fn text(label: impl Into<String>, value: impl Into<String>, source: PathBuf) -> Self {
        let value = value.into();
        Self {
            retained: false,
            interface: false,
            label: label.into(),
            value: Some(value.clone()),
            raw: Some(value.clone()),
            semantic: Some(Value::Text(value)),
            outcome: Outcome::Observed,
            error: None,
            source,
            origin: "metadata".into(),
            observed_ms: now_ms(),
            observed_generation: None,
            generation_pending: true,
        }
    }
    /// Materializes a bounded read while retaining raw data on parse failure.
    pub(super) fn attribute(
        label: &str,
        source: PathBuf,
        origin: impl Into<String>,
        read: io::Result<String>,
        parse: impl FnOnce(&str) -> io::Result<(Value, String)>,
    ) -> Self {
        let mut fact = match read {
            Ok(raw) => {
                let mut fact = match parse(&raw) {
                    Ok((semantic, display)) => {
                        let mut fact = Self::text(label, display, source);
                        fact.semantic = Some(semantic);
                        fact
                    }
                    Err(error) => Self::failure(label, source, error),
                };
                fact.raw = Some(raw);
                fact
            }
            Err(error) => Self::failure(label, source, error),
        };
        fact.origin = origin.into();
        fact
    }
    pub fn failure(label: impl Into<String>, source: PathBuf, error: io::Error) -> Self {
        let outcome = match error.kind() {
            io::ErrorKind::PermissionDenied => Outcome::PermissionDenied,
            io::ErrorKind::NotFound => Outcome::Missing,
            io::ErrorKind::TimedOut => Outcome::TimedOut,
            io::ErrorKind::UnexpectedEof => Outcome::Truncated,
            io::ErrorKind::InvalidData => Outcome::Malformed,
            _ => Outcome::Unsupported,
        };
        Self {
            retained: false,
            interface: false,
            label: label.into(),
            value: None,
            raw: None,
            semantic: None,
            outcome,
            error: Some(error.to_string()),
            source,
            origin: "metadata".into(),
            observed_ms: now_ms(),
            observed_generation: None,
            generation_pending: true,
        }
    }
}
pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl super::Document {
    /// Assigns provenance only to newly observed facts in an accepted generation.
    pub(super) fn stamp_fact_generations(&mut self) {
        if self.generation == 0 {
            return;
        }
        for fact in self
            .facts
            .iter_mut()
            .filter(|fact| !fact.retained && fact.generation_pending)
        {
            fact.observed_generation.get_or_insert(self.generation);
            fact.generation_pending = false;
        }
    }
    /// Uses the pinned interface kind; access-node names are not attributes.
    pub(super) fn requested_attribute(&self) -> Option<String> {
        if self.interface_directory
            || !matches!(
                self.kind,
                little_clip_core::kernel_interface::Kind::Kernel { .. }
            )
        {
            return None;
        }
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }

    fn retain_failed_previews(&mut self, previous: &Self) {
        self.storage = self.storage.take().or_else(|| {
            previous.storage.clone().map(|mut storage| {
                storage.retained = true;
                storage
            })
        });
        self.cpu = self.cpu.take().or_else(|| {
            previous.cpu.clone().map(|mut cpu| {
                cpu.retained = true;
                cpu
            })
        });
        self.telemetry = self.telemetry.take().or_else(|| previous.telemetry.clone());
        self.numa = self.numa.take().or_else(|| {
            previous.numa.clone().map(|mut data| {
                data.coverage = Outcome::Unsupported;
                for row in &mut data.rows {
                    row.retained = true;
                }
                data
            })
        });
        self.topology = self.topology.take().or_else(|| {
            previous.topology.clone().map(|mut topology| {
                topology.retained = true;
                topology
            })
        });
    }
    /// Reuses known provenance for the same retained observation at publication.
    /// Updated failure status and display formatting belong to the new result.
    pub(in crate::file_manager) fn inherit_retained_provenance(&mut self, previous: &Self) {
        if self.generation == 0
            || self.generation != previous.generation
            || !self.namespace.known()
            || self.namespace != previous.namespace
        {
            return;
        }
        for fact in self
            .facts
            .iter_mut()
            .filter(|fact| fact.retained && fact.observed_generation.is_none())
        {
            if let Some(old) = previous
                .facts
                .iter()
                .find(|old| fact.same_observation_key(old) && fact.observed_ms == old.observed_ms)
            {
                fact.inherit_observation(old);
            }
        }
    }
    pub(super) fn retain_failed_values(&mut self, previous: &Self) {
        self.inherit_retained_provenance(previous);
        if self.generation != previous.generation || self.namespace != previous.namespace {
            return;
        }
        if self.last_error.is_some() {
            self.retain_failed_previews(previous);
        }
        if let (Some(current), Some(old)) = (&mut self.numa, &previous.numa) {
            current.retain(old);
        }
        for old in previous
            .facts
            .iter()
            .filter(|fact| !fact.interface && fact.value.is_some())
        {
            let current = self
                .facts
                .iter_mut()
                .find(|fact| fact.same_observation_key(old));
            match current {
                Some(fact) if fact.outcome == Outcome::Observed || fact.value.is_some() => {}
                Some(fact) => {
                    fact.value = old.value.clone();
                    fact.raw = old.raw.clone();
                    fact.semantic = old.semantic.clone();
                    fact.inherit_observation(old);
                    fact.retained = true;
                }
                None => {
                    if let Some(error) = &self.last_error {
                        let mut retained = old.clone();
                        retained.outcome = self
                            .facts
                            .iter()
                            .find(|fact| fact.label == "Collection completion")
                            .map_or(Outcome::Unsupported, |fact| fact.outcome.clone());
                        retained.error = Some(error.clone());
                        retained.retained = true;
                        self.facts.push(retained);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod observation_tests {
    use super::*;
    #[test]
    fn rematerialized_retained_facts_preserve_known_generation_and_latest_failure() {
        let previous = super::super::retained_provenance_fixture();
        let mut current = previous.clone();
        let mut retained = Fact::failure(
            "Terminal",
            "/proc/42/stat".into(),
            io::Error::new(io::ErrorKind::TimedOut, "refresh failed"),
        );
        retained.value = Some("PID 42 (retained display)".into());
        retained.observed_ms = 123_000;
        retained.retained = true;
        current.facts = vec![retained];
        current.retain_failed_values(&previous);
        assert_eq!(current.facts[0].observed_generation, Some(40));
        assert_eq!(current.facts[0].observed_ms, 123_000);
        assert_eq!(current.facts[0].outcome, Outcome::TimedOut);
        assert_eq!(current.facts[0].error.as_deref(), Some("refresh failed"));
        assert_eq!(
            current.facts[0].value.as_deref(),
            Some("PID 42 (retained display)")
        );

        for case in 0..4 {
            let mut unrelated = current.clone();
            unrelated.facts[0].observed_generation = None;
            match case {
                0 => unrelated.generation += 1,
                1 => unrelated.facts[0].observed_ms += 1,
                2 => unrelated.namespace.pid = None,
                _ => unrelated.generation = 0,
            }
            unrelated.inherit_retained_provenance(&previous);
            assert_eq!(unrelated.facts[0].observed_generation, None);
        }
    }

    #[test]
    fn fact_generations_preserve_old_provenance_and_unknown_retained_values() {
        let mut document =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        document.generation = 17;
        let mut reused = Fact::text("Cached", "old", document.path.clone());
        reused.observed_generation = Some(12);
        document.facts.push(reused);
        let mut unknown = Fact::text("Legacy retained", "old", document.path.clone());
        unknown.retained = true;
        document.facts.push(unknown);
        document.stamp_fact_generations();
        assert!(
            document
                .facts
                .iter()
                .filter(|f| f.interface)
                .all(|f| f.observed_generation == Some(17))
        );
        assert_eq!(
            document
                .facts
                .iter()
                .find(|f| f.label == "Cached")
                .unwrap()
                .observed_generation,
            Some(12)
        );
        assert_eq!(
            document
                .facts
                .iter()
                .find(|f| f.label == "Legacy retained")
                .unwrap()
                .observed_generation,
            None
        );
        document.generation = 18;
        document.stamp_fact_generations();
        assert!(
            document
                .facts
                .iter()
                .filter(|f| f.interface)
                .all(|f| f.observed_generation == Some(17))
        );
    }

    #[test]
    fn retained_failed_fact_copies_original_generation_and_time() {
        let mut old =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        old.generation = 21;
        old.facts.clear();
        let mut value = Fact::text("Value", "old", old.path.clone());
        value.observed_generation = Some(19);
        value.observed_ms = 123;
        old.facts.push(value);
        let mut current = old.clone();
        current.facts = vec![Fact::failure(
            "Value",
            current.path.clone(),
            io::ErrorKind::PermissionDenied.into(),
        )];
        current.retain_failed_values(&old);
        current.stamp_fact_generations();
        assert_eq!(current.facts[0].observed_generation, Some(19));
        assert_eq!(current.facts[0].observed_ms, 123);
        assert!(current.facts[0].retained);
        assert_eq!(current.facts[0].outcome, Outcome::PermissionDenied);
    }

    #[test]
    fn fresh_helper_fact_roundtrip_is_stamped_only_after_acceptance() {
        let fact = Fact::text("Fresh", "value", "/fixture".into());
        let wire = serde_json::to_vec(&fact).unwrap();
        let fact: Fact = serde_json::from_slice(&wire).unwrap();
        let mut document =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        document.facts = vec![fact];
        document.stamp_fact_generations();
        assert_eq!(document.facts[0].observed_generation, None);
        document.generation = 77;
        document.stamp_fact_generations();
        assert_eq!(document.facts[0].observed_generation, Some(77));
        assert!(!document.facts[0].generation_pending);
    }

    #[test]
    fn old_fact_json_reports_unknown_generation_without_inventing_one() {
        let fact = Fact::text("Value", "old", "/fixture".into());
        let mut json = serde_json::to_value(fact).unwrap();
        json.as_object_mut().unwrap().remove("observed_generation");
        json.as_object_mut().unwrap().remove("generation_pending");
        let old: Fact = serde_json::from_value(json).unwrap();
        assert_eq!(old.observed_generation, None);
        let mut document =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        document.generation = 50;
        document.facts = vec![old];
        document.stamp_fact_generations();
        assert_eq!(document.facts[0].observed_generation, None);
    }

    #[test]
    fn requested_attributes_follow_pinned_kind_not_device_node_names() {
        let mut document =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        document.path = "/fixture/lockdown".into();
        assert_eq!(document.requested_attribute().as_deref(), Some("lockdown"));
        document.interface_directory = true;
        assert_eq!(document.requested_attribute(), None);
        document.interface_directory = false;
        document.kind = little_clip_core::kernel_interface::Kind::Character { major: 1, minor: 3 };
        assert_eq!(document.requested_attribute(), None);
    }
    #[test]
    fn parsed_attributes_preserve_raw_values_and_read_failure_provenance() {
        let source = PathBuf::from("/fixture/attribute");
        let malformed = Fact::attribute(
            "Number",
            source.clone(),
            "fixture",
            Ok("invalid".into()),
            |_| Err(io::Error::from(io::ErrorKind::InvalidData)),
        );
        assert_eq!(malformed.outcome, Outcome::Malformed);
        assert_eq!(malformed.raw.as_deref(), Some("invalid"));
        assert_eq!(malformed.source, source);
        assert_eq!(malformed.origin, "fixture");
        let denied = Fact::attribute(
            "Number",
            source,
            "fixture",
            Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            |_| panic!("denied reads must not be parsed"),
        );
        assert_eq!(denied.outcome, Outcome::PermissionDenied);
        assert_eq!(denied.raw, None);
    }
}
