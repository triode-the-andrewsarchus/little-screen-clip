// SPDX-License-Identifier: MPL-2.0
//! build.go error constructors, integer domain helpers and Go formatting.

use d2_rs_scene::{Box, Clip, Matrix, Node, Path, PathCommand, Primitive};

/// A scene construction failure carrying Go's exact error text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn err<T>(message: impl Into<String>) -> Result<T> {
    Err(Error(message.into()))
}

/// Go `%q`.
pub(crate) fn q(value: &str) -> String {
    d2_rs_core::quote_go_bytes(value.as_bytes())
}

/// Go `%v` of a float64.
pub(crate) fn fv(value: f64) -> String {
    crate::golib::geo::go_format_v(value)
}

/// unsupported(object, feature).
pub(crate) fn unsupported(object: &str, feature: &str) -> Error {
    Error(format!("scene: {object} uses unsupported {feature}"))
}

/// invalidField(object, field, value, requirement); `None` is a nil value.
pub(crate) fn invalid_field(
    object: &str,
    field: &str,
    value: Option<&str>,
    requirement: &str,
) -> Error {
    match value {
        None => Error(format!("scene: {object} field {} {requirement}", q(field))),
        Some(value) => Error(format!(
            "scene: {object} field {} has invalid value {value}: {requirement}",
            q(field)
        )),
    }
}

pub(crate) fn finite(value: f64) -> bool {
    value.is_finite()
}

pub(crate) const MAX_INT: i64 = i64::MAX;
pub(crate) const MIN_INT: i64 = i64::MIN;

/// checkedAdd over [minInt, maxInt].
pub(crate) fn checked_add(a: i64, value: i64, min_int: i64, max_int: i64) -> Option<i64> {
    if value > 0 && a > max_int - value || value < 0 && a < min_int - value {
        return None;
    }
    Some(a + value)
}

/// checkedSub over [minInt, maxInt].
pub(crate) fn checked_sub(a: i64, value: i64, min_int: i64, max_int: i64) -> Option<i64> {
    if value > 0 && a < min_int + value || value < 0 && a > max_int + value {
        return None;
    }
    Some(a - value)
}

/// clampBorderRadius.
pub(crate) fn clamp_border_radius(radius: f64, width: f64, height: f64) -> f64 {
    use d2_rs_scene::{go_max, go_min};
    go_max(0.0, go_min(radius, go_min(width / 2.0, height / 2.0)))
}

/// d2scene.NewNode with an ID.
pub(crate) fn node(id: impl Into<String>, primitive: Option<Primitive>) -> Node {
    let mut node = Node::new(primitive);
    node.id = id.into();
    node
}

/// Go `append(nil, children...)`: an empty append leaves the slice nil.
pub(crate) fn append_children(node: &mut Node, children: Vec<Node>) {
    if children.is_empty() {
        return;
    }
    node.children.get_or_insert_with(Vec::new).extend(children);
}

pub(crate) fn opt_vec<T>(values: Vec<T>) -> Option<Vec<T>> {
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

/// boxClip (assets.go).
pub(crate) fn box_clip(b: Box) -> Clip {
    Clip {
        path: Path {
            commands: vec![
                PathCommand::move_to(b.x, b.y),
                PathCommand::line_to(b.x + b.width, b.y),
                PathCommand::line_to(b.x + b.width, b.y + b.height),
                PathCommand::line_to(b.x, b.y + b.height),
                PathCommand::close(),
            ],
            ..Default::default()
        },
        transform: Matrix::identity(),
    }
}

/// roundedBoxClip (assets.go).
pub(crate) fn rounded_box_clip(b: Box, radius: f64) -> Clip {
    let (left, top) = (b.x, b.y);
    let (right, bottom) = (b.x + b.width, b.y + b.height);
    Clip {
        path: Path {
            commands: vec![
                PathCommand::move_to(left + radius, top),
                PathCommand::line_to(right - radius, top),
                PathCommand::arc_to(radius, radius, 0.0, false, true, right, top + radius),
                PathCommand::line_to(right, bottom - radius),
                PathCommand::arc_to(radius, radius, 0.0, false, true, right - radius, bottom),
                PathCommand::line_to(left + radius, bottom),
                PathCommand::arc_to(radius, radius, 0.0, false, true, left, bottom - radius),
                PathCommand::line_to(left, top + radius),
                PathCommand::arc_to(radius, radius, 0.0, false, true, left + radius, top),
                PathCommand::close(),
            ],
            ..Default::default()
        },
        transform: Matrix::identity(),
    }
}

/// imageShapeClip (assets.go): smooth cubics whose first control is the
/// current endpoint after each line.
pub(crate) fn image_shape_clip(b: Box, radius: f64) -> Clip {
    let (left, top) = (b.x, b.y);
    let (right, bottom) = (b.x + b.width, b.y + b.height);
    Clip {
        path: Path {
            commands: vec![
                PathCommand::move_to(left, top + radius),
                PathCommand::line_to(left, top + radius),
                PathCommand::cubic_to(left, top + radius, left, top, left + radius, top),
                PathCommand::line_to(right - radius, top),
                PathCommand::line_to(right - radius, top),
                PathCommand::cubic_to(right - radius, top, right, top, right, top + radius),
                PathCommand::line_to(right, bottom - radius),
                PathCommand::cubic_to(
                    right,
                    bottom - radius,
                    right,
                    bottom,
                    right - radius,
                    bottom,
                ),
                PathCommand::line_to(left + radius, bottom),
                PathCommand::cubic_to(left + radius, bottom, left, bottom, left, bottom - radius),
                PathCommand::line_to(left, top + radius),
                PathCommand::close(),
            ],
            ..Default::default()
        },
        transform: Matrix::identity(),
    }
}
