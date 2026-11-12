//! Bounded drawing snapshots and semantic edits; no rendering or file I/O.
use excalidraw_document::{Document, Element, Number, Profile, element};
pub use excalidraw_document::{ElementKind as Kind, Error};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashSet};

mod edits;

const MAX_ELEMENTS: usize = 2048;
const MAX_COORDINATE: f64 = 1e9;

/// Native creation tools offered by the canvas; unknown kinds remain preserved.
pub const TOOLS: &[Kind] = &[
    Kind::Rectangle,
    Kind::Ellipse,
    Kind::Diamond,
    Kind::Arrow,
    Kind::Line,
    Kind::Freedraw,
];
const PATH_KINDS: &[Kind] = &[Kind::Line, Kind::Arrow, Kind::Freedraw];

/// Source-space box. Rotation is in radians about its center.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub angle: f64,
}
impl Bounds {
    fn read(value: &Value) -> Option<Self> {
        let bounds = Self {
            x: value.get("x")?.as_f64()?,
            y: value.get("y")?.as_f64()?,
            width: value.get("width")?.as_f64()?,
            height: value.get("height")?.as_f64()?,
            angle: value.get("angle").map_or(Some(0.0), Value::as_f64)?,
        };
        bounds.valid().then_some(bounds)
    }
    pub fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height, self.angle]
            .into_iter()
            .all(|value| value.is_finite() && value.abs() <= MAX_COORDINATE)
            && self.width >= 0.0
            && self.height >= 0.0
    }
    /// Converts unrotated local coordinates into source coordinates.
    pub fn to_world(self, point: [f64; 2]) -> [f64; 2] {
        let (sin, cos) = self.angle.sin_cos();
        let [x, y] = [point[0] - self.width / 2.0, point[1] - self.height / 2.0];
        [
            self.x + self.width / 2.0 + x * cos - y * sin,
            self.y + self.height / 2.0 + x * sin + y * cos,
        ]
    }
    /// Converts source coordinates into the element's unrotated local space.
    pub fn to_local(self, point: [f64; 2]) -> [f64; 2] {
        let (sin, cos) = self.angle.sin_cos();
        let [x, y] = [
            point[0] - self.x - self.width / 2.0,
            point[1] - self.y - self.height / 2.0,
        ];
        [
            x * cos + y * sin + self.width / 2.0,
            -x * sin + y * cos + self.height / 2.0,
        ]
    }
}

/// One selectable native element. Unknown records stay in the backing document.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    pub bounds: Bounds,
    pub locked: bool,
    pub points: Vec<[f64; 2]>,
    pub text: Option<String>,
    pub style: Style,
    index: usize,
}
impl Item {
    fn read(index: usize, value: &Value) -> Option<Self> {
        let invalid_points = value.get("points").is_some_and(|points| {
            points
                .as_array()
                .is_none_or(|points| points.iter().any(|point| read_point(point).is_none()))
        });
        if value.get("isDeleted").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        Some(Self {
            id: value.get("id")?.as_str()?.into(),
            kind: Kind::from_wire(value.get("type")?.as_str()?),
            bounds: Bounds::read(value)?,
            // Invalid lock data must not accidentally unlock an element.
            locked: invalid_points
                || value
                    .get("locked")
                    .is_some_and(|v| v.as_bool() != Some(false)),
            points: value
                .get("points")
                .and_then(Value::as_array)
                .map(|points| points.iter().filter_map(read_point).collect())
                .unwrap_or_default(),
            text: value.get("text").and_then(Value::as_str).map(str::to_owned),
            style: Style::read(value),
            index,
        })
    }
}

/// Basic paint controls. Existing advanced properties are preserved by edits.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub stroke: String,
    pub fill: String,
    pub width: f64,
    pub roughness: f64,
    pub opacity: u8,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            stroke: "#1e1e1e".into(),
            fill: "transparent".into(),
            width: 2.0,
            roughness: 1.0,
            opacity: 100,
        }
    }
}
impl Style {
    fn read(value: &Value) -> Self {
        let defaults = Self::default();
        Self {
            stroke: value
                .get("strokeColor")
                .and_then(Value::as_str)
                .unwrap_or(&defaults.stroke)
                .into(),
            fill: value
                .get("backgroundColor")
                .and_then(Value::as_str)
                .unwrap_or(&defaults.fill)
                .into(),
            width: value
                .get("strokeWidth")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .unwrap_or(defaults.width)
                .clamp(0.1, 32.0),
            roughness: value
                .get("roughness")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite())
                .unwrap_or(defaults.roughness)
                .clamp(0.0, 3.0),
            opacity: value
                .get("opacity")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .min(100) as u8,
        }
    }
    fn apply(&self, shape: &mut Element) -> Result<(), Error> {
        for paint in [
            Paint::Stroke(self.stroke.clone()),
            Paint::Fill(self.fill.clone()),
            Paint::Width(self.width),
            Paint::Roughness(self.roughness),
            Paint::Opacity(self.opacity),
        ] {
            paint.apply(shape)?;
        }
        Ok(())
    }
}

/// One paint property, so changing a group's stroke never resets its mixed fills.
#[derive(Clone, Debug)]
pub enum Paint {
    Stroke(String),
    Fill(String),
    Width(f64),
    Roughness(f64),
    Opacity(u8),
}
impl Paint {
    fn apply(&self, shape: &mut Element) -> Result<(), Error> {
        let (key, value) = match self {
            Self::Stroke(value) => ("strokeColor", json!(value)),
            Self::Fill(value) => ("backgroundColor", json!(value)),
            Self::Width(value) if value.is_finite() && (0.1..=32.0).contains(value) => {
                ("strokeWidth", json!(value))
            }
            Self::Roughness(value) if value.is_finite() && (0.0..=3.0).contains(value) => {
                ("roughness", json!(value))
            }
            Self::Opacity(value) if *value <= 100 => ("opacity", json!(value)),
            _ => return Err(invalid("Paint controls exceed the drawing limits")),
        };
        if value.as_str().is_some_and(|v| v.len() > 64) {
            return Err(invalid("Invalid paint color"));
        }
        let previous = shape.as_object().get(key);
        let equal = previous.is_some_and(|old| {
            old == &value || old.is_number() && value.is_number() && old.as_f64() == value.as_f64()
        });
        if !equal {
            shape.set_raw(key, value);
        }
        Ok(())
    }
}

/// A single gesture or inspector change. Applying it never changes the snapshot.
#[derive(Clone, Debug)]
pub enum Change {
    Move {
        ids: Vec<String>,
        by: [f64; 2],
    },
    Paint {
        ids: Vec<String>,
        paint: Paint,
    },
    Delete {
        ids: Vec<String>,
    },
    Create {
        id: String,
        kind: Kind,
        bounds: Bounds,
        points: Vec<[f64; 2]>,
        style: Style,
    },
}

/// An immutable, worker-prepared scene. The native document retains unknown
/// fields, exact numbers, malformed neighbors, tombstones and unused resources.
#[derive(Clone, Debug)]
pub struct Scene {
    pub items: Vec<Item>,
    document: Document,
    source: String,
}
impl Scene {
    pub fn parse(source: &str) -> Result<Self, Error> {
        if source.len() > super::MAX_DRAWING_SOURCE_BYTES {
            return Err(invalid("Drawing exceeds the 2 MiB source budget"));
        }
        let document = Document::from_slice(source.trim_start_matches('\u{feff}').as_bytes())?;
        let values = elements(&document)?;
        if values.len() > MAX_ELEMENTS {
            return Err(invalid("Drawing exceeds the 2048-element canvas budget"));
        }
        let mut ids = HashSet::new();
        for id in values
            .iter()
            .filter_map(|v| v.get("id").and_then(Value::as_str))
        {
            if !ids.insert(id) {
                return Err(invalid(
                    "Duplicate element identities require source editing",
                ));
            }
        }
        let items = values
            .iter()
            .enumerate()
            .filter_map(|(i, v)| Item::read(i, v))
            .collect();
        Ok(Self {
            items,
            document,
            source: source.into(),
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Resolves groups and bound labels for selection feedback without mutation.
    pub fn composition(&self, ids: &[String]) -> Result<Vec<&Item>, Error> {
        let indices = self.selection(ids)?;
        Ok(self
            .items
            .iter()
            .filter(|item| indices.contains(&item.index))
            .collect())
    }

    /// Returns one new source revision, or `None` for a semantic no-op.
    /// Failure leaves the entire original document untouched.
    pub fn apply(&self, change: Change) -> Result<Option<String>, Error> {
        let mut next = self.document.clone();
        match change {
            Change::Move { ids, by } => self.move_items(&mut next, &ids, by)?,
            Change::Paint { ids, paint } => {
                for index in self.selection(&ids)? {
                    edit(&mut next, index, |shape| paint.apply(shape))?;
                }
            }
            Change::Delete { ids } => self.delete_items(&mut next, &ids)?,
            Change::Create {
                id,
                kind,
                bounds,
                points,
                style,
            } => {
                self.create(&mut next, id, kind, bounds, points, style)?;
            }
        }
        if next == self.document {
            return Ok(None);
        }
        let bytes = next.to_vec_pretty()?;
        let mut text =
            String::from_utf8(bytes).map_err(|_| invalid("Invalid serialized drawing"))?;
        if self.source.starts_with('\u{feff}') {
            text.insert(0, '\u{feff}');
        }
        if self.source.ends_with('\n') {
            text.push('\n');
        }
        if text.len() > super::MAX_DRAWING_SOURCE_BYTES {
            return Err(invalid("Edited drawing exceeds the 2 MiB source budget"));
        }
        Ok(Some(text))
    }

    fn selection(&self, ids: &[String]) -> Result<BTreeSet<usize>, Error> {
        let values = elements(&self.document)?;
        let mut selected = BTreeSet::new();
        for id in ids {
            let item = self
                .items
                .iter()
                .find(|item| &item.id == id)
                .ok_or_else(|| invalid("The selection no longer exists"))?;
            selected.insert(item.index);
        }
        expand_composition(values, &mut selected);
        for index in &selected {
            let item = self
                .items
                .iter()
                .find(|item| item.index == *index)
                .ok_or_else(|| {
                    invalid("This composition contains unsupported records; use source editing")
                })?;
            if matches!(item.kind, Kind::Unknown(_) | Kind::Selection | Kind::Draw) {
                return Err(invalid("Unsupported elements require source editing"));
            }
            if item.locked {
                return Err(invalid("The selection contains locked elements"));
            }
        }
        Ok(selected)
    }
}

fn expand_composition(values: &[Value], selected: &mut BTreeSet<usize>) {
    // Groups, labels and frame descendants form one composition; arrow
    // bindings deliberately do not pull other connected boxes along.
    loop {
        let before = selected.len();
        let ids: HashSet<_> = selected
            .iter()
            .filter_map(|i| values[*i].get("id").and_then(Value::as_str))
            .collect();
        let selected_groups: HashSet<_> =
            selected.iter().flat_map(|i| groups(&values[*i])).collect();
        let containers: HashSet<_> = selected
            .iter()
            .filter_map(|i| values[*i].get("containerId").and_then(Value::as_str))
            .collect();
        for (i, value) in values.iter().enumerate() {
            if value.get("isDeleted").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let linked = ["containerId", "frameId"].iter().any(|key| {
                value
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|id| ids.contains(id))
            });
            if linked
                || groups(value).any(|group| selected_groups.contains(group))
                || value
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| containers.contains(id))
            {
                selected.insert(i);
            }
        }
        if selected.len() == before {
            break;
        }
    }
}

fn elements(document: &Document) -> Result<&[Value], Error> {
    document
        .as_object()
        .get("elements")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| invalid("Drawing has no elements array"))
}
fn groups(value: &Value) -> impl Iterator<Item = &str> {
    value
        .get("groupIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}
fn read_point(value: &Value) -> Option<[f64; 2]> {
    let a = value.as_array()?;
    if a.len() != 2 {
        return None;
    }
    let point = [a[0].as_f64()?, a[1].as_f64()?];
    point
        .into_iter()
        .all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE)
        .then_some(point)
}
fn invalid(message: impl Into<String>) -> Error {
    Error {
        path: "/elements".into(),
        message: message.into(),
    }
}

// Revision metadata advances only for an actual semantic change, so clicking a
// control without changing it neither reformats the source nor creates undo work.
fn edit(
    document: &mut Document,
    index: usize,
    change: impl FnOnce(&mut Element) -> Result<(), Error>,
) -> Result<(), Error> {
    document.edit_element(index, |shape| {
        let before = shape.clone();
        change(shape)?;
        if *shape != before {
            let version = before
                .as_object()
                .get("version")
                .map_or(Some(0), Value::as_u64)
                .and_then(|n| n.checked_add(1))
                .ok_or_else(|| invalid("Invalid element revision"))?;
            shape.set_raw("version", json!(version));
            shape.set_raw("versionNonce", json!(uuid::Uuid::new_v4().as_u128() as u32));
            shape.set_raw("updated", json!(timestamp()));
        }
        Ok(())
    })
}
fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| time.as_millis().min(u128::from(u64::MAX)) as u64)
}

#[cfg(test)]
mod tests;
