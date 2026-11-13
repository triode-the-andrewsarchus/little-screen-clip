//! Single-element resize plans share the existing bound-path update mechanism.
use super::edits::{binding, path_values, points, set_bounds};
use super::*;

#[derive(Clone, Debug)]
pub struct ResizePlan {
    pub bounds: Bounds,
    pub movement: Movement,
    pub font_size: Option<f64>,
}

impl Scene {
    /// Plans one resize without mutation; unsupported compositions fail atomically.
    pub fn resize(&self, id: &str, bounds: Bounds) -> Result<ResizePlan, Error> {
        let item = self.editable_item(id)?;
        let value = &elements(&self.document)?[item.index];
        validate_target(item, value, bounds)?;
        let scale = [
            bounds.width / item.bounds.width,
            bounds.height / item.bounds.height,
        ];
        let font_size = if item.kind == Kind::Text {
            self.text_info(id)?;
            if value.get("containerId").is_some_and(|v| !v.is_null())
                || (scale[0] - scale[1]).abs() > 1e-6
            {
                return Err(invalid(
                    "Resize unbound text proportionally; resize its container for a bound label",
                ));
            }
            let size = text::text_style(value)?.size * scale[0];
            if !(1.0..=512.0).contains(&size) {
                return Err(invalid("Resized text font exceeds native editing limits"));
            }
            Some(size)
        } else {
            None
        };
        if PATH_KINDS.contains(&item.kind) {
            validate_linear(item, value)?;
        }
        let mut movement = Movement::default();
        self.resize_labels(item, bounds, &mut movement)?;
        self.resize_connections(item, bounds, &mut movement)?;
        Ok(ResizePlan {
            bounds,
            movement,
            font_size,
        })
    }

    pub(super) fn resize_items(
        &self,
        document: &mut Document,
        id: &str,
        bounds: Bounds,
    ) -> Result<(), Error> {
        let plan = self.resize(id, bounds)?;
        let item = self.editable_item(id)?;
        if item.bounds == bounds {
            return Ok(());
        }
        let values = elements(&self.document)?;
        let linear = resized_path(item, &values[item.index], bounds)?;
        edit(document, item.index, |shape| {
            set_bounds(shape, bounds)?;
            if let Some(points) = linear {
                shape.set(element::POINTS, path_values(points)?)?;
            }
            if let Some(size) = plan.font_size {
                shape.set(element::FONT_SIZE, Number::from_f64(size)?)?;
            }
            Ok(())
        })?;
        self.apply_movement(document, &plan.movement)
    }

    fn resize_labels(
        &self,
        item: &Item,
        bounds: Bounds,
        movement: &mut Movement,
    ) -> Result<(), Error> {
        let values = elements(&self.document)?;
        for label in &self.items {
            let value = &values[label.index];
            if value.get("containerId").and_then(Value::as_str) != Some(item.id.as_str()) {
                continue;
            }
            self.text_container(label)?;
            self.text_info(&label.id)?;
            let old = item.bounds;
            let position = [
                anchored(
                    label.bounds.x,
                    old.x,
                    old.width,
                    bounds.x,
                    bounds.width,
                    value.get("textAlign").and_then(Value::as_str),
                ),
                anchored(
                    label.bounds.y,
                    old.y,
                    old.height,
                    bounds.y,
                    bounds.height,
                    value.get("verticalAlign").and_then(Value::as_str),
                ),
            ];
            text::fit_label(bounds, position, [label.bounds.width, label.bounds.height])?;
            movement.offsets.insert(
                label.id.clone(),
                [position[0] - label.bounds.x, position[1] - label.bounds.y],
            );
        }
        Ok(())
    }

    fn resize_connections(
        &self,
        item: &Item,
        bounds: Bounds,
        movement: &mut Movement,
    ) -> Result<(), Error> {
        let values = elements(&self.document)?;
        for arrow in self.items.iter().filter(|item| item.kind == Kind::Arrow) {
            let value = &values[arrow.index];
            let keys = ["startBinding", "endBinding"];
            let attached = keys.map(|key| binding(value, key) == Some(item.id.as_str()));
            if !attached.into_iter().any(|bound| bound) {
                continue;
            }
            if item.kind != Kind::Rectangle || arrow.locked || arrow.bounds.angle != 0.0 {
                return Err(invalid(
                    "This connected resize needs source editing; only unrotated rectangular attachments are supported",
                ));
            }
            let reciprocal = values[item.index]
                .get("boundElements")
                .and_then(Value::as_array)
                .is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        entry.get("id").and_then(Value::as_str) == Some(&arrow.id)
                            && entry.get("type").and_then(Value::as_str) == Some("arrow")
                    })
                });
            if !reciprocal {
                return Err(invalid(
                    "Arrow/container bindings must agree before resizing",
                ));
            }
            let path = points(value)?;
            let indices = [0, path.len() - 1];
            let mut delta = [[0.0; 2]; 2];
            for endpoint in 0..2 {
                if !attached[endpoint] {
                    continue;
                }
                let binding = &value[keys[endpoint]];
                if binding.get("mode").is_some()
                    || binding
                        .get("fixedPoint")
                        .is_some_and(|point| !point.is_null())
                {
                    return Err(invalid(
                        "Fixed-point/snapshot bindings need source editing during resize",
                    ));
                }
                let point = [
                    arrow.bounds.x + path[indices[endpoint]][0],
                    arrow.bounds.y + path[indices[endpoint]][1],
                ];
                let target = rectangle_attachment(item.bounds, bounds, point);
                delta[endpoint] = [target[0] - point[0], target[1] - point[1]];
            }
            self.stretch_arrow(movement, arrow, value, [0.0; 2], delta)?;
        }
        Ok(())
    }
}

fn resized_path(
    item: &Item,
    value: &Value,
    bounds: Bounds,
) -> Result<Option<Vec<[f64; 2]>>, Error> {
    if !PATH_KINDS.contains(&item.kind) {
        return Ok(None);
    }
    Ok(Some(
        points(value)?
            .into_iter()
            .map(|[x, y]| {
                [
                    x * bounds.width / item.bounds.width,
                    y * bounds.height / item.bounds.height,
                ]
            })
            .collect(),
    ))
}

fn validate_target(item: &Item, value: &Value, bounds: Bounds) -> Result<(), Error> {
    if !bounds.valid()
        || bounds.width < 1.0
        || bounds.height < 1.0
        || item.bounds.width < 1.0
        || item.bounds.height < 1.0
        || bounds.angle != 0.0
        || item.bounds.angle != 0.0
    {
        return Err(invalid(
            "Resize requires finite, positive, unrotated bounds; degenerate paths need source editing",
        ));
    }
    if value
        .get("groupIds")
        .is_some_and(|groups| !groups.is_array())
    {
        return Err(invalid("Malformed group membership needs source editing"));
    }
    if !matches!(
        item.kind,
        Kind::Rectangle
            | Kind::Ellipse
            | Kind::Diamond
            | Kind::Image
            | Kind::Line
            | Kind::Arrow
            | Kind::Freedraw
            | Kind::Text
    ) || groups(value).next().is_some()
    {
        return Err(invalid(
            "Groups, frames and unsupported element kinds need source editing for resize",
        ));
    }
    Ok(())
}
fn validate_linear(item: &Item, value: &Value) -> Result<(), Error> {
    if ["startBinding", "endBinding", "fixedSegments"]
        .iter()
        .any(|key| value.get(key).is_some_and(|v| !v.is_null()))
        || value.get("elbowed").and_then(Value::as_bool) == Some(true)
    {
        return Err(invalid(
            "Bound, fixed-segment or elbowed path resize needs source editing",
        ));
    }
    let points = points(value)?;
    if points
        .iter()
        .any(|p| p[0] < 0.0 || p[1] < 0.0 || p[0] > item.bounds.width || p[1] > item.bounds.height)
    {
        return Err(invalid(
            "Paths outside their declared box need source editing for resize",
        ));
    }
    Ok(())
}
fn anchored(
    position: f64,
    old: f64,
    old_size: f64,
    new: f64,
    new_size: f64,
    align: Option<&str>,
) -> f64 {
    let factor = match align {
        Some("center" | "middle") => 0.5,
        Some("right" | "bottom") => 1.0,
        _ => 0.0,
    };
    new + (position - old) + (new_size - old_size) * factor
}
fn rectangle_attachment(old: Bounds, new: Bounds, point: [f64; 2]) -> [f64; 2] {
    let local = [point[0] - old.x, point[1] - old.y];
    let clamped = [
        local[0].clamp(0.0, old.width),
        local[1].clamp(0.0, old.height),
    ];
    [
        new.x + clamped[0] / old.width * new.width + local[0] - clamped[0],
        new.y + clamped[1] / old.height * new.height + local[1] - clamped[1],
    ]
}
