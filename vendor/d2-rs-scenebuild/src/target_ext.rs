// SPDX-License-Identifier: MPL-2.0
//! The d2target/d2target.go, class.go and sqltable.go methods and constants
//! that d2scenebuild calls (Diagram.BoundingBox, CalculateTooltipPosition,
//! calculateTooltipBounds, Connection label/icon/arrowhead positions,
//! Arrowhead.Dimensions, GetIconSize, BaseConnection, VisibilityToken,
//! ConstraintAbbr, DSL_SHAPE_TO_SHAPE_TYPE), over the d2-rs-target mirror.
//! Adapted from d2-rs-render's d2svg/target_methods.rs and tooltip.rs.
//!
//! d2target is compiled for ARM64 like the renderer: the `a + x*y` sums in
//! GetArrowheadLabelPosition and Arrowhead.Dimensions are FMADDD (as the
//! d2svg port already records).

use d2_rs_target::{
    ClassField, ClassMethod, Connection, Diagram, GeoPoint, Point as TargetPoint, Shape, SqlColumn,
};

use crate::golib::geo::{self, Point, go_fmadd};
use crate::golib::label::{self, Position};
use crate::golib::shape;

pub(crate) const DEFAULT_ICON_SIZE: i64 = 32;
pub(crate) const MAX_ICON_SIZE: i64 = 64;
pub(crate) const SHADOW_SIZE_X: i64 = 3;
pub(crate) const SHADOW_SIZE_Y: i64 = 5;
pub(crate) const THREE_DEE_OFFSET: i64 = 15;
pub(crate) const MULTIPLE_OFFSET: i64 = 10;
pub(crate) const INNER_BORDER_OFFSET: i64 = 5;
pub(crate) const BG_COLOR: &str = "N7";
pub(crate) const FG_COLOR: &str = "N1";
pub(crate) const MIN_ARROWHEAD_STROKE_WIDTH: f64 = 2.0;
pub(crate) const ARROWHEAD_PADDING: f64 = 2.0;
pub(crate) const CONNECTION_ICON_LABEL_GAP: i64 = 8;

// class.go / sqltable.go
pub(crate) const PREFIX_PADDING: f64 = 10.0;
pub(crate) const PREFIX_WIDTH: f64 = 20.0;
pub(crate) const NAME_PADDING: i64 = 10;
pub(crate) const TYPE_PADDING: i64 = 20;
pub(crate) const HEADER_PADDING: i64 = 10;
pub(crate) const HEADER_FONT_ADD: i64 = 4;

pub(crate) const SHAPE_RECTANGLE: &str = "rectangle";
pub(crate) const SHAPE_SQUARE: &str = "square";
pub(crate) const SHAPE_OVAL: &str = "oval";
pub(crate) const SHAPE_CIRCLE: &str = "circle";
pub(crate) const SHAPE_HEXAGON: &str = "hexagon";
pub(crate) const SHAPE_TEXT: &str = "text";
pub(crate) const SHAPE_CODE: &str = "code";
pub(crate) const SHAPE_CLASS: &str = "class";
pub(crate) const SHAPE_SQL_TABLE: &str = "sql_table";
pub(crate) const SHAPE_IMAGE: &str = "image";
pub(crate) const SHAPE_CLOUD: &str = "cloud";
pub(crate) const SHAPE_C4_PERSON: &str = "c4-person";
pub(crate) const SHAPE_SEQUENCE_DIAGRAM: &str = "sequence_diagram";
pub(crate) const SHAPE_HIERARCHY: &str = "hierarchy";

pub(crate) const NO_ARROWHEAD: &str = "none";

/// Go `int(math.Ceil(float64(n)/2.))`.
fn half_ceil(n: i64) -> i64 {
    (n as f64 / 2.0).ceil() as i64
}

pub(crate) fn geo_point(p: &GeoPoint) -> Point {
    Point::new(p.x, p.y)
}

/// Connection.Route as geo points; preflight rejects nil entries first.
pub(crate) fn route_points(c: &Connection) -> Vec<Point> {
    c.route()
        .iter()
        .map(|p| p.as_ref().map(geo_point).unwrap_or_default())
        .collect()
}

/// DSL_SHAPE_TO_SHAPE_TYPE[dsl] (a missing key reads "").
pub(crate) fn dsl_shape_to_shape_type(dsl: &str) -> &'static str {
    shape::dsl_shape_to_shape_type(dsl)
}

/// Connection.GetLabelTopLeft (nil for a non-edge position).
pub(crate) fn connection_label_top_left(c: &Connection) -> Option<Point> {
    Position::from_string(&c.label_position)
        .try_get_point_on_route(
            &route_points(c),
            c.stroke_width as f64,
            c.label_percentage,
            c.text.label_width as f64,
            c.text.label_height as f64,
        )
        .map(|(p, _)| p)
}

/// Connection.GetIconPosition.
pub(crate) fn connection_icon_position(c: &Connection) -> Option<Point> {
    c.icon.as_ref()?;
    if !c.text.label.is_empty() {
        if let Some(tl) = connection_label_top_left(c) {
            return Some(Point::new(
                tl.x - CONNECTION_ICON_LABEL_GAP as f64 - DEFAULT_ICON_SIZE as f64,
                tl.y + c.text.label_height as f64 / 2.0 - (DEFAULT_ICON_SIZE / 2) as f64,
            ));
        }
    }
    Position::from_string(&c.icon_position)
        .try_get_point_on_route(
            &route_points(c),
            c.stroke_width as f64,
            -1.0,
            DEFAULT_ICON_SIZE as f64,
            DEFAULT_ICON_SIZE as f64,
        )
        .map(|(p, _)| p)
}

/// Connection.GetArrowheadLabelPosition. Go dereferences the label pointer and
/// route unconditionally; callers guarantee both.
pub(crate) fn connection_arrowhead_label_position(c: &Connection, is_dst: bool) -> Point {
    let text = if is_dst {
        c.dst_label.as_ref()
    } else {
        c.src_label.as_ref()
    }
    .expect("arrowhead label");
    let (width, height) = (text.label_width as f64, text.label_height as f64);
    let route = route_points(c);
    let index = if is_dst { route.len() - 2 } else { 0 };
    let (start, end) = (route[index], route[index + 1]);
    let (normal_x, normal_y) = geo::get_unit_normal_vector(end.x, end.y, start.x, start.y);
    // ARM64: the second product is fused into the sum.
    let shift = go_fmadd(
        normal_y.abs(),
        width / 2.0 + label::PADDING,
        normal_x.abs() * (height / 2.0 + label::PADDING),
    );
    let length = geo::route_length(&route);
    let position = if is_dst {
        let mut position = 1.0;
        if length > 0.0 {
            position -= shift / length;
        }
        position
    } else if length > 0.0 {
        shift / length
    } else {
        0.0
    };
    let stroke_width = c.stroke_width as f64;
    let (mut label_tl, _) =
        Position::UnlockedTop.get_point_on_route(&route, stroke_width, position, width, height);
    let mut arrow_size = 0.0;
    if is_dst && c.dst_arrow != NO_ARROWHEAD {
        arrow_size = arrowhead_dimensions(&c.dst_arrow, stroke_width).1;
    } else if c.src_arrow != NO_ARROWHEAD {
        arrow_size = arrowhead_dimensions(&c.src_arrow, stroke_width).1;
    }
    if arrow_size > 0.0 {
        let offset = (arrow_size / 2.0 + ARROWHEAD_PADDING) - stroke_width / 2.0 - label::PADDING;
        if offset > 0.0 {
            label_tl.x = go_fmadd(normal_x, offset, label_tl.x);
            label_tl.y = go_fmadd(normal_y, offset, label_tl.y);
        }
    }
    label_tl
}

/// Arrowhead.Dimensions.
pub(crate) fn arrowhead_dimensions(arrowhead: &str, stroke_width: f64) -> (f64, f64) {
    let (base_width, base_height, width_multiplier, height_multiplier) = match arrowhead {
        "arrow" => (4.0, 4.0, 4.0, 4.0),
        "triangle" => (4.0, 4.0, 3.0, 4.0),
        "unfilled-triangle" => (7.0, 7.0, 3.0, 4.0),
        "line" => (0.0, 0.0, 5.0, 8.0),
        "filled-diamond" => (11.0, 7.0, 5.5, 3.5),
        "diamond" => (11.0, 9.0, 5.5, 4.5),
        "cross" => (7.0, 7.0, 5.0, 5.0),
        "filled-circle" | "circle" => (8.0, 8.0, 5.0, 5.0),
        "filled-box" | "box" => (6.0, 6.0, 5.0, 5.0),
        "cf-one" | "cf-many" | "cf-one-required" | "cf-many-required" => (9.0, 9.0, 4.5, 4.5),
        _ => (0.0, 0.0, 0.0, 0.0),
    };
    let clipped = geo::go_max(MIN_ARROWHEAD_STROKE_WIDTH, stroke_width);
    (
        go_fmadd(clipped, width_multiplier, base_width),
        go_fmadd(clipped, height_multiplier, base_height),
    )
}

/// d2target.GetIconSize.
pub(crate) fn get_icon_size(b: &geo::Box, position: &str) -> i64 {
    let icon_position = Position::from_string(position);
    let min_dimension = geo::go_min(b.width, b.height) as i64;
    let half_min_dimension = (0.5 * min_dimension as f64).ceil() as i64;
    let mut size = if icon_position == Position::InsideMiddleCenter {
        half_min_dimension
    } else {
        min_dimension.min(DEFAULT_ICON_SIZE.max(half_min_dimension))
    };
    size = size.min(MAX_ICON_SIZE);
    if !icon_position.is_outside() {
        let padding = label::PADDING as i64;
        size = size.min(
            ((b.width as i64) - 2 * padding)
                .max(0)
                .min(((b.height as i64) - 2 * padding).max(0)),
        );
    }
    size
}

/// d2target.CalculateTooltipPosition.
pub(crate) fn calculate_tooltip_position(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    tooltip_width: i64,
    tooltip_height: i64,
    position: &str,
) -> (f64, f64) {
    let padding = 10.0;
    let (tw, th) = (tooltip_width as f64, tooltip_height as f64);
    match position {
        "top-left" => (x, y - th - padding),
        "top-center" => (x + width / 2. - tw / 2., y - th - padding),
        "top-right" => (x + width - tw, y - th - padding),
        "center-left" => (x - tw - padding, y + height / 2. - th / 2.),
        "center-right" => (x + width + padding, y + height / 2. - th / 2.),
        "bottom-left" => (x, y + height + padding),
        "bottom-center" => (x + width / 2. - tw / 2., y + height + padding),
        "bottom-right" => (x + width - tw, y + height + padding),
        _ => (x + width / 2. - tw / 2., y - th - padding),
    }
}

/// d2target.calculateTooltipBounds (textmeasure.MeasureMarkdown is
/// LayoutMarkdown's content size).
fn calculate_tooltip_bounds(
    s: &Shape,
    font_family: Option<&str>,
    mono_font_family: Option<&str>,
) -> (i64, i64, i64, i64) {
    if s.tooltip.is_empty() || s.tooltip_position.is_empty() {
        return (0, 0, 0, 0);
    }
    let fallback = || ((s.tooltip.len() as i64 * 8 + 20).min(200), 30);
    let (width, height) = match crate::markdown::measure_markdown(
        &s.tooltip,
        font_family,
        mono_font_family,
        &s.text.font_family,
        16,
    ) {
        Ok((w, h)) => (w + 20, h + 20),
        Err(_) => fallback(),
    };
    let (x, y) = calculate_tooltip_position(
        s.pos.x as f64,
        s.pos.y as f64,
        s.width as f64,
        s.height as f64,
        width,
        height,
        &s.tooltip_position,
    );
    (
        x.floor() as i64,
        y.floor() as i64,
        x.ceil() as i64 + width,
        y.ceil() as i64 + height,
    )
}

fn shape_top_left(s: &Shape) -> Point {
    Point::new(s.pos.x as f64, s.pos.y as f64)
}

/// Diagram.BoundingBox.
pub(crate) fn bounding_box(diagram: &Diagram) -> (TargetPoint, TargetPoint) {
    let shapes = diagram.shapes();
    if shapes.is_empty() {
        return (TargetPoint::default(), TargetPoint::default());
    }
    let (mut x1, mut y1) = (i64::from(i32::MAX), i64::from(i32::MAX));
    let (mut x2, mut y2) = (i64::from(i32::MIN), i64::from(i32::MIN));
    for s in shapes {
        let half = half_ceil(s.stroke_width);
        x1 = x1.min(s.pos.x - half);
        y1 = y1.min(s.pos.y - half);
        x2 = x2.max(s.pos.x + s.width + half);
        y2 = y2.max(s.pos.y + s.height + half);
        if s.type_ == SHAPE_C4_PERSON {
            let head_radius = (s.width as f64 * 0.22) as i64;
            let head_center_y = (s.height as f64 * 0.18) as i64;
            let head_top = s.pos.y + head_center_y - head_radius;
            y1 = y1.min(head_top - s.stroke_width);
        }
        if !s.tooltip.is_empty() || !s.link.is_empty() {
            if !s.tooltip_position.is_empty() {
                let (min_x, min_y, max_x, max_y) = calculate_tooltip_bounds(
                    s,
                    diagram.font_family.as_deref(),
                    diagram.mono_font_family.as_deref(),
                );
                x1 = x1.min(min_x);
                y1 = y1.min(min_y);
                x2 = x2.max(max_x);
                y2 = y2.max(max_y);
            } else {
                y1 = y1.min(s.pos.y - s.stroke_width - 16);
                x2 = x2.max(s.pos.x + s.stroke_width + s.width + 16);
            }
        }
        if s.shadow {
            y2 = y2.max(s.pos.y + s.height + half + SHADOW_SIZE_Y);
            x2 = x2.max(s.pos.x + s.width + half + SHADOW_SIZE_X);
        }
        if s.three_dee {
            let offset_y = if s.type_ == SHAPE_HEXAGON {
                THREE_DEE_OFFSET / 2
            } else {
                THREE_DEE_OFFSET
            };
            y1 = y1.min(s.pos.y - offset_y - s.stroke_width);
            x2 = x2.max(s.pos.x + THREE_DEE_OFFSET + s.width + s.stroke_width);
        }
        if s.multiple {
            y1 = y1.min(s.pos.y - MULTIPLE_OFFSET - s.stroke_width);
            x2 = x2.max(s.pos.x + MULTIPLE_OFFSET + s.width + s.stroke_width);
        }
        if s.icon.is_some() && Position::from_string(&s.icon_position).is_outside() {
            let content_box = geo::Box::new(Point::new(0.0, 0.0), s.width as f64, s.height as f64);
            // BoundingBox passes the DSL type, not DSL_SHAPE_TO_SHAPE_TYPE.
            let shape = shape::new_shape(&s.type_, content_box);
            let size = get_icon_size(&shape.get_inner_box(), &s.icon_position);
            let padding = label::PADDING as i64;
            if s.icon_position.starts_with("OUTSIDE_TOP") {
                y1 = y1.min(s.pos.y - padding - size);
            } else if s.icon_position.starts_with("OUTSIDE_BOTTOM") {
                y2 = y2.max(s.pos.y + s.height + padding + size);
            } else if s.icon_position.starts_with("OUTSIDE_LEFT") {
                x1 = x1.min(s.pos.x - padding - size);
            } else if s.icon_position.starts_with("OUTSIDE_RIGHT") {
                x2 = x2.max(s.pos.x + s.width + padding + size);
            }
        }
        if !s.text.label.is_empty() {
            let position = Position::from_string(&s.label_position);
            let shape = shape::new_shape(
                dsl_shape_to_shape_type(&s.type_),
                geo::Box::new(shape_top_left(s), s.width as f64, s.height as f64),
            );
            let mut tl = position.get_point_on_box(
                &shape.get_box(),
                label::PADDING,
                s.text.label_width as f64,
                s.text.label_height as f64,
            );
            if s.three_dee {
                let offset = if s.type_ == SHAPE_HEXAGON {
                    THREE_DEE_OFFSET / 2
                } else {
                    THREE_DEE_OFFSET
                };
                if s.label_position.starts_with("OUTSIDE_RIGHT") {
                    tl.x += offset as f64;
                }
                if s.label_position.starts_with("OUTSIDE_TOP") {
                    tl.y -= offset as f64;
                }
            }
            x1 = x1.min(tl.x as i64);
            y1 = y1.min(tl.y as i64);
            x2 = x2.max(tl.x as i64 + s.text.label_width);
            y2 = y2.max(tl.y as i64 + s.text.label_height);
        }
    }
    for c in diagram.connections() {
        let half = half_ceil(c.stroke_width);
        for point in route_points(c) {
            x1 = x1.min(point.x.floor() as i64 - half);
            y1 = y1.min(point.y.floor() as i64 - half);
            x2 = x2.max(point.x.ceil() as i64 + half);
            y2 = y2.max(point.y.ceil() as i64 + half);
        }
        if !c.text.label.is_empty() {
            // Go dereferences the nil point for a non-edge position (a panic);
            // preflight rejects those positions first.
            let tl = connection_label_top_left(c).unwrap_or_default();
            x1 = x1.min(tl.x as i64);
            y1 = y1.min(tl.y as i64);
            x2 = x2.max(tl.x as i64 + c.text.label_width);
            y2 = y2.max(tl.y as i64 + c.text.label_height);
        }
        for (is_dst, text) in [(false, &c.src_label), (true, &c.dst_label)] {
            if let Some(text) = text.as_ref().filter(|text| !text.label.is_empty()) {
                let tl = connection_arrowhead_label_position(c, is_dst);
                x1 = x1.min(tl.x as i64);
                y1 = y1.min(tl.y as i64);
                x2 = x2.max(tl.x as i64 + text.label_width);
                y2 = y2.max(tl.y as i64 + text.label_height);
            }
        }
    }
    (TargetPoint { x: x1, y: y1 }, TargetPoint { x: x2, y: y2 })
}

/// d2target.BaseConnection.
pub(crate) fn base_connection() -> Connection {
    let mut c = Connection {
        src_arrow: NO_ARROWHEAD.into(),
        dst_arrow: NO_ARROWHEAD.into(),
        route: Some(Vec::new()),
        opacity: 1.0,
        stroke_width: 2,
        border_radius: 10.0,
        ..Default::default()
    };
    c.text.italic = true;
    c.text.font_family = "DEFAULT".into();
    c
}

/// Connection.GetFontColor.
pub(crate) fn connection_font_color(c: &Connection) -> String {
    if !c.text.color.is_empty() {
        return c.text.color.clone();
    }
    "N1".into()
}

fn visibility_token(visibility: &str) -> &'static str {
    match visibility {
        "protected" => "#",
        "private" => "-",
        _ => "+",
    }
}

/// ClassField.VisibilityToken.
pub(crate) fn field_visibility_token(field: &ClassField) -> &'static str {
    visibility_token(&field.visibility)
}

/// ClassMethod.VisibilityToken.
pub(crate) fn method_visibility_token(method: &ClassMethod) -> &'static str {
    visibility_token(&method.visibility)
}

/// SQLColumn.ConstraintAbbr.
pub(crate) fn constraint_abbr(column: &SqlColumn) -> String {
    column
        .constraint
        .iter()
        .flatten()
        .map(|constraint| match constraint.as_str() {
            "primary_key" => "PK",
            "foreign_key" => "FK",
            "unique" => "UNQ",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(", ")
}
