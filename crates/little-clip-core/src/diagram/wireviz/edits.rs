//! Source-preserving harness transactions with independent circuit validation.
use super::*;
use oneyaml::{Patch, PatchOp};

mod wiring;

const MAX_OPERATIONS: usize = 4096;
const MAX_PATCH_BYTES: usize = 512 * 1024;

/// One supported graphical change, addressing an immutable harness snapshot.
#[derive(Clone, Debug)]
pub enum Edit {
    /// Adds a direct connection between an already-visible pin and conductor.
    Connect { from: Endpoint, to: Endpoint },
    /// Removes one cable termination or loop by its immutable snapshot index.
    ///
    /// Mating annotations require source editing.
    Disconnect { link: usize },
    PinLabel {
        connector: usize,
        pin: usize,
        label: String,
    },
    WireColor {
        cable: usize,
        wire: usize,
        color: Option<String>,
    },
}

/// An immutable source revision and its resolved circuit, prepared on a worker.
///
/// Transactions return source text for the host's existing undo/save owner.
/// Parsing and applying belong on bounded workers, never on a UI callback.
pub struct Editable {
    source: String,
    harness: Harness,
}
impl Editable {
    /// Parses a source revision using the same limits and semantics as previews.
    ///
    /// # Errors
    /// Returns the syntax, budget and circuit errors reported by [`super::parse`].
    pub fn parse(source: &str) -> Result<Self, String> {
        let harness = super::parse(source)?;
        Ok(Self {
            source: source.into(),
            harness,
        })
    }

    /// Returns the immutable circuit used to address graphical changes.
    pub fn harness(&self) -> &Harness {
        &self.harness
    }

    /// Applies one edit while preserving source and electrical relationships.
    ///
    /// Label/color references are rewritten to stable pin IDs or wire numbers.
    /// Both the complete YAML value and the normalized circuit are checked after
    /// editing, so a shared anchor cannot silently modify unrelated components.
    /// Wiring changes additionally prove the remaining link multiset and retain
    /// explicitly visible singleton components when a path is disconnected.
    /// A host publishing a background result must check the source revision
    /// again; this method can compare only the captured `current` argument.
    ///
    /// # Examples
    ///
    /// ```
    /// use little_clip_core::diagram::wireviz::{parse, edits::{Editable, Edit}};
    /// let source = "connectors:\n  J: {pincount: 1, pinlabels: [OLD]}\ncables:\n  W: {wirecount: 1}\nconnections:\n  - [{J: OLD}, {W: 1}]\n";
    /// let snapshot = Editable::parse(source)?;
    /// let updated = snapshot.apply(source, Edit::PinLabel {
    ///     connector: 0, pin: 0, label: "Ground".into(),
    /// })?;
    /// assert_eq!(parse(&updated)?.links, snapshot.harness().links);
    /// # Ok::<(), String>(())
    /// ```
    ///
    /// # Errors
    /// Refuses stale source, invalid targets, ambiguous changes, alias/merge
    /// writes and budget violations. The snapshot and input are never mutated.
    pub fn apply(&self, current: &str, edit: Edit) -> Result<String, String> {
        if current != self.source {
            return Err("Harness source changed; wait for the editor to refresh".into());
        }
        if matches!(edit, Edit::Connect { .. } | Edit::Disconnect { .. }) {
            return wiring::apply(self, current, edit);
        }
        let (mut yaml, value) = document(current)?;
        if yaml.to_string() != current {
            return Err("This YAML cannot be edited losslessly; use the source editor".into());
        }
        let mut plan = Plan {
            value,
            changes: Changes {
                ops: Vec::new(),
                bytes: 0,
            },
        };
        let mut intended = self.harness.clone();
        let target = self.target(edit, &mut intended, &mut plan)?;
        if intended == self.harness {
            return Ok(current.to_owned());
        }
        plan.references(&target)?;
        yaml.apply_patch(&Patch::new(plan.changes.ops))
            .map_err(|e| format!("Cannot preserve this YAML edit; use the source editor: {e}"))?;
        let output = yaml.to_string();
        let (_, actual) = document(&output)?;
        if actual != plan.value {
            return Err(
                "Edit would change shared YAML data outside the selection; use the source editor"
                    .into(),
            );
        }
        if model(&actual)? != intended {
            return Err("Edit would change the harness circuit or unrelated components; use the source editor".into());
        }
        Ok(output)
    }

    fn target<'a>(
        &'a self,
        edit: Edit,
        intended: &mut Harness,
        plan: &mut Plan,
    ) -> Result<Target<'a>, String> {
        match edit {
            Edit::Connect { .. } | Edit::Disconnect { .. } => {
                unreachable!("wiring edits have a separate source plan")
            }
            Edit::PinLabel {
                connector,
                pin,
                label,
            } => {
                if label.len() > 1024 || label.contains('\0') {
                    return Err("Pin labels must fit 1 KiB and contain no NUL".into());
                }
                let owner = self
                    .harness
                    .connectors
                    .get(connector)
                    .ok_or("Unknown connector")?;
                owner.pins.get(pin).ok_or("Unknown connector pin")?;
                intended.connectors[connector].pins[pin].label = Some(label);
                let labels: Vec<_> = intended.connectors[connector]
                    .pins
                    .iter()
                    .map(|p| optional(&p.label))
                    .collect();
                plan.item("connectors", &owner.name, "pinlabels", pin, &labels[..=pin])?;
                Ok(Target::Pin(owner, intended.connectors[connector].clone()))
            }
            Edit::WireColor { cable, wire, color } => {
                if let Some(color) = &color {
                    colors::bands(color)?;
                }
                let owner = self.harness.cables.get(cable).ok_or("Unknown cable")?;
                owner.wires.get(wire).ok_or("Unknown cable wire")?;
                intended.cables[cable].wires[wire].color = color;
                let colors: Vec<_> = intended.cables[cable]
                    .wires
                    .iter()
                    .map(|w| optional(&w.color))
                    .collect();
                // Materialize repeated colors before changing one conductor;
                // changing a short repeating list would otherwise recolor others.
                plan.item("cables", &owner.name, "colors", wire, &colors)?;
                Ok(Target::Wire(owner, intended.cables[cable].clone()))
            }
        }
    }
}

enum Target<'a> {
    Pin(&'a Connector, Connector),
    Wire(&'a Cable, Cable),
}
impl Target<'_> {
    fn name(&self) -> &str {
        match self {
            Self::Pin(c, _) => &c.name,
            Self::Wire(c, _) => &c.name,
        }
    }
    fn reference(&self, value: &Value) -> Result<Option<Value>, String> {
        let canonical = match self {
            Self::Pin(before, after) => {
                let pin = before.pin(value)?;
                if after.pin(value).is_ok_and(|index| index == pin) {
                    return Ok(None);
                }
                match &before.pins[pin].id {
                    PinId::Number(n) => Value::from(*n),
                    PinId::Text(s) => Value::from(s.clone()),
                }
            }
            Self::Wire(before, after) => {
                let wire = before.wire(value)?;
                if after.wire(value).is_ok_and(|index| index == wire) {
                    return Ok(None);
                }
                match wire {
                    Conductor::Wire(index) => Value::from((index + 1) as u64),
                    Conductor::Shield => Value::from("s"),
                }
            }
        };
        Ok((canonical != *value).then_some(canonical))
    }
}

struct Plan {
    value: Value,
    changes: Changes,
}
struct Changes {
    ops: Vec<PatchOp>,
    bytes: usize,
}
impl Plan {
    fn item(
        &mut self,
        section: &str,
        name: &str,
        field: &str,
        index: usize,
        expanded: &[Value],
    ) -> Result<(), String> {
        let attrs = component_mut(&mut self.value, section, name)?;
        let key = Value::from(field);
        let base = format!("/{section}/{}/{field}", token(name));
        let Some(value) = attrs.get_mut(&key) else {
            let value = Value::from(expanded.to_vec());
            attrs.insert(key, value.clone());
            return self.changes.push(base, value, true);
        };
        let list = value
            .as_sequence_mut()
            .ok_or("Edited attribute is not a list")?;
        let old_len = list.len();
        for value in expanded.iter().skip(old_len) {
            list.push(value.clone());
            self.changes
                .push(format!("{base}/-"), value.clone(), true)?;
        }
        let item = list
            .get_mut(index)
            .ok_or("Edited item is outside its attribute list")?;
        if index < old_len && *item != expanded[index] {
            *item = expanded[index].clone();
            self.changes
                .push(format!("{base}/{index}"), item.clone(), false)?;
        }
        Ok(())
    }

    fn references(&mut self, target: &Target<'_>) -> Result<(), String> {
        let sets = self
            .value
            .as_mapping_mut()
            .and_then(|m| m.get_mut(&Value::from("connections")))
            .and_then(Value::as_sequence_mut)
            .ok_or("Missing connection sets")?;
        for (set, items) in sets.iter_mut().enumerate() {
            let items = items
                .as_sequence_mut()
                .ok_or("Connection set is not a list")?;
            for (step, item) in items.iter_mut().enumerate() {
                let Some(selector) = item
                    .as_mapping_mut()
                    .and_then(|m| m.get_mut(&Value::from(target.name())))
                else {
                    continue;
                };
                let path = format!("/connections/{set}/{step}/{}", token(target.name()));
                rewrite(selector, path, target, true, &mut self.changes)?;
            }
        }
        if let Target::Pin(owner, _) = target {
            let attrs = component_mut(&mut self.value, "connectors", &owner.name)?;
            if let Some(loops) = attrs.get_mut(&Value::from("loops")) {
                for (pair, pins) in loops
                    .as_sequence_mut()
                    .ok_or("Connector loops are not a list")?
                    .iter_mut()
                    .enumerate()
                {
                    rewrite(
                        pins,
                        format!("/connectors/{}/loops/{pair}", token(&owner.name)),
                        target,
                        false,
                        &mut self.changes,
                    )?;
                }
            }
        }
        Ok(())
    }
}

impl Changes {
    fn push(&mut self, path: String, value: Value, add: bool) -> Result<(), String> {
        let bytes = path.len() + value_size(&value);
        let operation = if add {
            PatchOp::add(path, value)
        } else {
            PatchOp::replace(path, value)
        };
        self.record(operation, bytes)
    }

    fn record(&mut self, operation: PatchOp, bytes: usize) -> Result<(), String> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.ops.len() >= MAX_OPERATIONS || self.bytes > MAX_PATCH_BYTES {
            return Err("Harness edit exceeds the transaction budget".into());
        }
        self.ops.push(operation);
        Ok(())
    }
}

fn rewrite(
    value: &mut Value,
    path: String,
    target: &Target<'_>,
    ranges: bool,
    changes: &mut Changes,
) -> Result<(), String> {
    if let Some(items) = value.as_sequence_mut() {
        for (index, item) in items.iter_mut().enumerate() {
            if ranges && connections::numeric_range(item).is_some() {
                continue;
            }
            replace_reference(item, format!("{path}/{index}"), target, changes)?;
        }
    } else {
        replace_reference(value, path, target, changes)?;
    }
    Ok(())
}

fn replace_reference(
    value: &mut Value,
    path: String,
    target: &Target<'_>,
    changes: &mut Changes,
) -> Result<(), String> {
    if let Some(canonical) = target.reference(value)? {
        changes.push(path, canonical.clone(), false)?;
        *value = canonical;
    }
    Ok(())
}

fn component_mut<'a>(
    value: &'a mut Value,
    section: &str,
    name: &str,
) -> Result<&'a mut Mapping, String> {
    value
        .as_mapping_mut()
        .and_then(|m| m.get_mut(&Value::from(section)))
        .and_then(Value::as_mapping_mut)
        .and_then(|m| m.get_mut(&Value::from(name)))
        .and_then(Value::as_mapping_mut)
        .ok_or("Missing component attributes".into())
}

fn optional(value: &Option<String>) -> Value {
    value
        .as_ref()
        .map_or(Value::Null, |s| Value::from(s.clone()))
}
fn token(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn value_size(value: &Value) -> usize {
    let mut size = 0_usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        size = size.saturating_add(16 + value.as_str().map_or(0, str::len));
        if let Some(items) = value.as_sequence() {
            pending.extend(items.iter());
        }
        if let Some(items) = value.as_mapping() {
            for (key, value) in items.iter() {
                pending.push(key);
                pending.push(value);
            }
        }
    }
    size
}

#[cfg(test)]
mod tests;
