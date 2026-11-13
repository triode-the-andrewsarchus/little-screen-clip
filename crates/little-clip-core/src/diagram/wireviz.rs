//! Native electrical harness data; no rendering, file access or source execution.
//!
//! This is an independent implementation of the published WireViz syntax. The
//! Python implementation is not included or translated. Parsing belongs on a
//! bounded worker; callers retain the original source for editing and saving.
use oneyaml::{Compat, Document, Limits, Mapping, Value};
use std::collections::HashSet;

mod attributes;
pub mod colors;
mod connections;
pub mod edits;

const MAX_SOURCE: usize = 256 * 1024;
const MAX_COMPONENTS: usize = 256;
const MAX_TERMINALS: usize = 8192;
const MAX_PINS: usize = 512;
const MAX_CONNECTIONS: usize = 8192;

/// An authored pin identity, preserving the distinction between `01` and `"01"`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PinId {
    Number(i64),
    Text(String),
}
impl PinId {
    fn read(value: &Value) -> Result<Self, String> {
        if let Some(text) = value.as_str() {
            Ok(Self::Text(text.to_owned()))
        } else if let Some(number) = value.as_i64() {
            Ok(Self::Number(number))
        } else {
            Err("Pin IDs must be integers or strings; quote boolean-like names such as NO".into())
        }
    }
}

/// One physical connector terminal, in declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub id: PinId,
    pub label: Option<String>,
    pub color: Option<String>,
}

/// One connector definition and its resolved terminal identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connector {
    pub name: String,
    pub description: Option<String>,
    pub notes: Option<String>,
    pub pins: Vec<Pin>,
    pub simple: bool,
    /// Definitions are displayed only when referenced by a connection set.
    pub used: bool,
}
impl Connector {
    fn pin(&self, value: &Value) -> Result<usize, String> {
        let id = PinId::read(value)?;
        if let Some(index) = self.pins.iter().position(|pin| pin.id == id) {
            return Ok(index);
        }
        let candidates: Vec<_> = self
            .pins
            .iter()
            .enumerate()
            .filter(|(_, pin)| {
                value
                    .as_str()
                    .is_some_and(|label| pin.label.as_deref() == Some(label))
            })
            .map(|(index, _)| index)
            .collect();
        match candidates.as_slice() {
            [index] => Ok(*index),
            [] => Err(format!("Unknown pin {value:?} on {}", self.name)),
            _ => Err(format!("Ambiguous pin label {value:?} on {}", self.name)),
        }
    }
}

/// One numbered cable conductor, with an optional authored label and color.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wire {
    pub label: Option<String>,
    pub color: Option<String>,
}

/// A cable or wire bundle; conductor numbers start at one in source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cable {
    pub name: String,
    pub description: Option<String>,
    pub notes: Option<String>,
    pub wires: Vec<Wire>,
    pub shield: Option<String>,
    pub bundle: bool,
    pub used: bool,
}
impl Cable {
    fn wire(&self, value: &Value) -> Result<Conductor, String> {
        if value.as_str() == Some("s") {
            return self
                .shield
                .as_ref()
                .map(|_| Conductor::Shield)
                .ok_or_else(|| format!("{} has no shield", self.name));
        }
        if let Some(number) = value.as_u64() {
            return usize::try_from(number)
                .ok()
                .and_then(|n| n.checked_sub(1))
                .filter(|index| *index < self.wires.len())
                .map(Conductor::Wire)
                .ok_or_else(|| format!("Unknown wire {number} on {}", self.name));
        }
        let label = value
            .as_str()
            .ok_or("Wire references must be numbers, labels or colors")?;
        // An authored label takes precedence over a coincidentally identical
        // color abbreviation on another conductor.
        let by_label = self
            .wires
            .iter()
            .any(|wire| wire.label.as_deref() == Some(label));
        let candidates: Vec<_> = self
            .wires
            .iter()
            .enumerate()
            .filter(|(_, wire)| {
                if by_label {
                    wire.label.as_deref() == Some(label)
                } else {
                    wire.color.as_deref() == Some(label)
                }
            })
            .map(|(index, _)| index)
            .collect();
        match candidates.as_slice() {
            [index] => Ok(Conductor::Wire(*index)),
            [] => Err(format!("Unknown wire {label:?} on {}", self.name)),
            _ => Err(format!(
                "Ambiguous wire label/color {label:?} on {}",
                self.name
            )),
        }
    }
}

/// A cable conductor, using a zero-based index for ordinary wires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Conductor {
    Wire(usize),
    Shield,
}

/// A resolved source endpoint; indices address the owning [`Harness`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Endpoint {
    Pin { connector: usize, pin: usize },
    Cable { cable: usize, conductor: Conductor },
    Connector(usize),
}

/// Arrowheads used by pin-to-pin or whole-connector mating annotations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    None,
    Forward,
    Backward,
    Both,
}

/// Electrical links and mating annotations retain their distinct meanings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkKind {
    Cable,
    PinMating(Direction),
    ConnectorMating(Direction),
    Loop,
}

/// Location of a normalized link in its authoritative source structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Connection {
        set: usize,
        step: usize,
        lane: usize,
    },
    Loop {
        connector: usize,
        pair: usize,
    },
}

/// One resolved pair, including its source lane for selection and diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub endpoints: [Endpoint; 2],
    pub kind: LinkKind,
    pub origin: Origin,
}

/// A normalized harness shared by rendering and future graphical transactions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Harness {
    pub connectors: Vec<Connector>,
    pub cables: Vec<Cable>,
    pub links: Vec<Link>,
    /// Resource omissions and unused definitions, never omitted electrical links.
    pub warnings: Vec<String>,
}

/// Parses and resolves bounded WireViz data without reading linked resources.
///
/// Scalar identities use explicit YAML 1.1 rules, including merge keys, matching
/// the format's established scalar conventions. Quoted IDs remain strings.
/// Unsupported circuit semantics are errors rather than silently missing wires.
///
/// # Examples
///
/// ```
/// use little_clip_core::diagram::wireviz::{parse, Conductor, Endpoint};
/// let harness = parse("connectors:\n  J: {pincount: 2}\ncables:\n  W: {colors: [RD, BK]}\nconnections:\n  - [{J: [1, 2]}, {W: [1, 2]}]\n")?;
/// assert_eq!(harness.links.len(), 2);
/// assert_eq!(harness.links[1].endpoints[1], Endpoint::Cable {
///     cable: 0, conductor: Conductor::Wire(1),
/// });
/// # Ok::<(), String>(())
/// ```
///
/// # Errors
/// Returns syntax, duplicate-key, resource-budget, ambiguous endpoint or
/// unsupported-feature errors. Template expansion and automatic cable color
/// codes are not implemented yet and are explicitly refused.
pub fn parse(source: &str) -> Result<Harness, String> {
    let (_, value) = document(source)?;
    model(&value)
}

fn document(source: &str) -> Result<(Document, Value), String> {
    if source.len() > MAX_SOURCE || source.contains('\0') {
        return Err("WireViz exceeds the source budget or contains NUL".into());
    }
    let limits = Limits::untrusted()
        .max_depth(64)
        .max_typed_depth(64)
        .max_document_size(MAX_SOURCE)
        .max_scalar_len(32 * 1024)
        .max_documents(1)
        .alias_budget(32_768)
        .alias_byte_budget(1024 * 1024)
        .max_diagnostics(32);
    let settings = Compat::yaml_1_1().limits(limits);
    let document = Document::parse_with(source, &settings).map_err(|e| e.to_string())?;
    let value = document.to_value().map_err(|e| e.to_string())?;
    reject_tags(&value)?;
    Ok((document, value))
}

fn model(value: &Value) -> Result<Harness, String> {
    let root = mapping(value, "WireViz document")?;
    let mut harness = attributes::read(root)?;
    connections::read(root, &mut harness)?;
    Ok(harness)
}

/// Recognizes a small, complete harness in generic YAML or clipboard text.
///
/// This deliberately excludes incomplete, empty and unsupported diagrams. Their
/// explicit extensions and Markdown fences remain available for diagnostics.
pub fn signature(source: &str) -> bool {
    source.len() <= 32 * 1024
        && source.contains("connections:")
        && (source.contains("connectors:") || source.contains("cables:"))
        && parse(source).is_ok_and(|harness| {
            harness.connectors.iter().any(|c| c.used) || harness.cables.iter().any(|c| c.used)
        })
}

fn mapping<'a>(value: &'a Value, context: &str) -> Result<&'a Mapping, String> {
    value
        .as_mapping()
        .ok_or_else(|| format!("{context} must be a mapping"))
}

fn sequence<'a>(value: &'a Value, context: &str) -> Result<&'a [Value], String> {
    value
        .as_sequence()
        .map(|items| items.as_slice())
        .ok_or_else(|| format!("{context} must be a list"))
}

fn reject_tags(value: &Value) -> Result<(), String> {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Tagged(_) => return Err("Custom YAML tags are not supported in WireViz".into()),
            Value::Mapping(map) => pending.extend(map.iter().flat_map(|(key, value)| [key, value])),
            Value::Sequence(items) => pending.extend(items.iter()),
            _ => (),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
