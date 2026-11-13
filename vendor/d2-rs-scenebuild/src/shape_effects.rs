// SPDX-License-Identifier: MPL-2.0
//! shape_effects.go: shadow/blend nesting, animated shapes, 3D, multiple and
//! double-border geometry, and ordinary outlines.

use d2_rs_scene::{
    AnimationProperty, AnimationValue, AnimationValueKind, BlendMode, Bounds, Box as SceneBox,
    DropShadow, Ellipse, Filter, Keyframe, Matrix, Node, Nrgba, Paint, Path, PathCommand, Point,
    Primitive, Rect, Stroke, Track,
};
use d2_rs_target::{Diagram, Shape};

use crate::golib::shape::Shape as _;
use crate::target_ext::*;
use crate::util::*;
use crate::{Builder, scene_path, target_geometry};

const ANIMATED_SHAPE_DURATION: i64 = 1_000_000_000;
/// Go's untyped constant `(12.6 + 3*25.2) + (7.56 + 3*15.12) + 2`.
const ANIMATED_SHAPE_MAX_FILTER_EXPANSION: f64 = 143.12;

fn svg_static_shape_shadow() -> Filter {
    Filter::DropShadow(DropShadow {
        offset_x: 3.0,
        offset_y: 5.0,
        sigma_x: 0.0,
        sigma_y: 0.0,
        color: Nrgba::new(0x3d, 0x45, 0x74, 102),
    })
}

fn shadow_midpoints() -> [DropShadow; 2] {
    [
        DropShadow {
            offset_x: 0.0,
            offset_y: 12.6,
            sigma_x: 25.2,
            sigma_y: 25.2,
            color: Nrgba::new(50, 50, 93, 64),
        },
        DropShadow {
            offset_x: 0.0,
            offset_y: 7.56,
            sigma_x: 15.12,
            sigma_y: 15.12,
            color: Nrgba::new(0, 0, 0, 26),
        },
    ]
}

fn shadow_endpoints() -> [DropShadow; 2] {
    [
        DropShadow {
            color: Nrgba::new(50, 50, 93, 0),
            ..Default::default()
        },
        DropShadow::default(),
    ]
}

fn filter_declarations() -> [DropShadow; 2] {
    let endpoints = shadow_endpoints();
    [
        DropShadow {
            offset_x: 0.0,
            offset_y: 12.6,
            sigma_x: 25.2,
            sigma_y: 25.2,
            color: endpoints[0].color,
        },
        DropShadow {
            offset_x: 0.0,
            offset_y: 7.56,
            sigma_x: 15.12,
            sigma_y: 15.12,
            color: endpoints[1].color,
        },
    ]
}

pub(crate) fn transform_value(m: Matrix) -> AnimationValue {
    AnimationValue {
        kind: AnimationValueKind::TRANSFORM,
        transform: m,
        ..Default::default()
    }
}

pub(crate) fn shadow_value(s: DropShadow) -> AnimationValue {
    AnimationValue {
        kind: AnimationValueKind::SHADOW,
        shadow: s,
        ..Default::default()
    }
}

pub(crate) fn number_value(v: f64) -> AnimationValue {
    AnimationValue {
        kind: AnimationValueKind::NUMBER,
        number: v,
        ..Default::default()
    }
}

impl Builder<'_> {
    /// appendShapeGeometry: nest geometry under a filter/blend group.
    pub(crate) fn append_shape_geometry(&self, group: &mut Node, s: &Shape, children: Vec<Node>) {
        let shadow = s.shadow && shape_supports_shadow(&s.type_);
        if !shadow && !s.blend {
            append_children(group, children);
            return;
        }
        let mut inner = node(format!("{}:shape", s.id), None);
        append_children(&mut inner, children);
        if shadow {
            inner.filters = Some(vec![svg_static_shape_shadow()]);
        }
        if s.blend {
            inner.blend = BlendMode::MULTIPLY;
            inner.opacity = 0.5;
        }
        append_children(group, vec![inner]);
    }

    pub(crate) fn build_shape_effects(
        &mut self,
        s: &Shape,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    ) -> Result<Vec<Node>> {
        if s.three_dee {
            if s.type_ == SHAPE_HEXAGON {
                return self.build_three_dee_hexagon(s, fill, stroke);
            }
            return self.build_three_dee_rectangle(s, fill, stroke);
        }
        if s.double_border {
            return self.build_double_border_shape(s, fill, stroke);
        }
        if s.multiple {
            let mut multiple = s.clone();
            multiple.pos.x += MULTIPLE_OFFSET;
            multiple.pos.y -= MULTIPLE_OFFSET;
            let mut nodes = self.build_ordinary_shape_outline(
                &multiple,
                fill.clone(),
                stroke.clone(),
                &format!("{}:multiple", s.id),
            )?;
            nodes.extend(self.build_ordinary_shape_outline(
                s,
                fill,
                stroke,
                &format!("{}:main", s.id),
            )?);
            return Ok(nodes);
        }
        err(format!("scene: shape {} has no shape effect", q(&s.id)))
    }

    pub(crate) fn build_ordinary_shape_outline(
        &self,
        s: &Shape,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
        id_prefix: &str,
    ) -> Result<Vec<Node>> {
        let b = SceneBox {
            x: s.pos.x as f64,
            y: s.pos.y as f64,
            width: s.width as f64,
            height: s.height as f64,
        };
        let mut nodes = Vec::new();
        match s.type_.as_str() {
            SHAPE_TEXT => return Ok(Vec::new()),
            SHAPE_OVAL | SHAPE_CIRCLE => nodes.push(node(
                "",
                Some(Primitive::Ellipse(Ellipse {
                    center: Point::new(b.x + b.width / 2.0, b.y + b.height / 2.0),
                    radius_x: b.width / 2.0,
                    radius_y: b.height / 2.0,
                    fill,
                    stroke,
                })),
            )),
            SHAPE_RECTANGLE | SHAPE_SQUARE | SHAPE_SEQUENCE_DIAGRAM | SHAPE_HIERARCHY | "" => {
                let radius = clamp_border_radius(s.border_radius as f64, b.width, b.height);
                nodes.push(node(
                    "",
                    Some(Primitive::Rect(Rect {
                        r#box: b,
                        radius_x: radius,
                        radius_y: radius,
                        fill,
                        stroke,
                    })),
                ));
            }
            _ => {
                let geometry = target_geometry(s);
                let paths = geometry.get_path_commands().unwrap_or_default();
                if paths.is_empty() {
                    return Err(unsupported(
                        &format!("shape {}", q(&s.id)),
                        &format!("typed geometry for {}", s.type_),
                    ));
                }
                let count = paths.len();
                for (path_index, commands) in paths.iter().enumerate() {
                    let path = scene_path(commands, fill.clone(), stroke.clone());
                    let mut n = node("", Some(Primitive::Path(path)));
                    if !id_prefix.is_empty() {
                        n.id = if count == 1 {
                            id_prefix.to_owned()
                        } else {
                            format!("{id_prefix}:path:{path_index}")
                        };
                    }
                    nodes.push(n);
                }
            }
        }
        if !id_prefix.is_empty() && nodes.len() == 1 && nodes[0].id.is_empty() {
            nodes[0].id = id_prefix.to_owned();
        }
        Ok(nodes)
    }

    fn build_double_border_shape(
        &self,
        s: &Shape,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    ) -> Result<Vec<Node>> {
        let mut nodes = Vec::new();
        if s.multiple {
            let mut multiple = s.clone();
            multiple.pos.x += MULTIPLE_OFFSET;
            multiple.pos.y -= MULTIPLE_OFFSET;
            nodes.extend(double_border_pair(
                &multiple,
                fill.clone(),
                fill.clone(),
                stroke.clone(),
                &format!("{}:multiple", s.id),
            ));
        }
        let transparent = self.paint(
            "transparent",
            &format!("shape {} double-border inner fill", q(&s.id)),
        )?;
        let inner_fill = if s.type_ == SHAPE_OVAL || s.type_ == SHAPE_CIRCLE {
            fill.clone()
        } else {
            transparent
        };
        nodes.extend(double_border_pair(
            s,
            fill,
            inner_fill,
            stroke,
            &format!("{}:double-border", s.id),
        ));
        Ok(nodes)
    }

    fn build_three_dee_rectangle(
        &self,
        s: &Shape,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    ) -> Result<Vec<Node>> {
        let (x, y, w, h) = (
            s.pos.x as f64,
            s.pos.y as f64,
            s.width as f64,
            s.height as f64,
        );
        let offset = THREE_DEE_OFFSET as f64;
        let main = node(
            format!("{}:3d-main", s.id),
            Some(Primitive::Rect(Rect {
                r#box: SceneBox {
                    x,
                    y,
                    width: w,
                    height: h,
                },
                fill,
                ..Default::default()
            })),
        );
        let dark = self.three_dee_side_paint(s)?;
        let sides = node(
            format!("{}:3d-sides", s.id),
            Some(Primitive::Path(polygon_path(
                &[
                    Point::new(x, y),
                    Point::new(x + offset, y - offset),
                    Point::new(x + w + offset, y - offset),
                    Point::new(x + w + offset, y + h - offset),
                    Point::new(x + w, y + h),
                    Point::new(x + w, y),
                ],
                dark,
                None,
            ))),
        );
        let border = node(
            format!("{}:3d-border", s.id),
            Some(Primitive::Path(Path {
                commands: vec![
                    PathCommand::move_to(x, y),
                    PathCommand::line_to(x + offset, y - offset),
                    PathCommand::line_to(x + w + offset, y - offset),
                    PathCommand::line_to(x + w + offset, y + h - offset),
                    PathCommand::line_to(x + w, y + h),
                    PathCommand::line_to(x, y + h),
                    PathCommand::line_to(x, y),
                    PathCommand::line_to(x + w, y),
                    PathCommand::line_to(x + w, y + h),
                    PathCommand::move_to(x + w, y),
                    PathCommand::line_to(x + w + offset, y - offset),
                ],
                stroke,
                ..Default::default()
            })),
        );
        Ok(vec![main, sides, border])
    }

    fn build_three_dee_hexagon(
        &self,
        s: &Shape,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    ) -> Result<Vec<Node>> {
        let (x, y, w, h) = (s.pos.x, s.pos.y, s.width, s.height);
        let scale = |n: i64, factor: f64| (n as f64 * factor) as i64;
        let quarter_x = scale(w, 0.25);
        let three_quarter_x = scale(w, 0.75);
        // Go folds the untyped constant 43.6/87.3 exactly before rounding.
        let middle_y = scale(h, 0.499_427_262_313_860_25);
        let y_offset = THREE_DEE_OFFSET / 2;
        let absolute = |px: i64, py: i64| Point::new((x + px) as f64, (y + py) as f64);
        let main_points = [
            absolute(quarter_x, 0),
            absolute(three_quarter_x, 0),
            absolute(w, middle_y),
            absolute(three_quarter_x, h),
            absolute(quarter_x, h),
            absolute(0, middle_y),
        ];
        let main = node(
            format!("{}:3d-main", s.id),
            Some(Primitive::Path(polygon_path(&main_points, fill, None))),
        );
        let dark = self.three_dee_side_paint(s)?;
        let side_points = [
            absolute(quarter_x + THREE_DEE_OFFSET, -y_offset),
            absolute(three_quarter_x + THREE_DEE_OFFSET, -y_offset),
            absolute(w + THREE_DEE_OFFSET, middle_y - y_offset),
            absolute(three_quarter_x + THREE_DEE_OFFSET, h - y_offset),
            absolute(three_quarter_x, h),
            absolute(w, middle_y),
            absolute(three_quarter_x, 0),
            absolute(quarter_x, 0),
        ];
        let sides = node(
            format!("{}:3d-sides", s.id),
            Some(Primitive::Path(polygon_path(&side_points, dark, None))),
        );
        let mut commands = vec![PathCommand::move_to(main_points[0].x, main_points[0].y)];
        for p in [
            side_points[0],
            side_points[1],
            side_points[2],
            side_points[3],
            main_points[3],
            main_points[4],
            main_points[5],
            main_points[0],
            main_points[1],
            main_points[2],
            main_points[3],
        ] {
            commands.push(PathCommand::line_to(p.x, p.y));
        }
        for p in [main_points[1], main_points[2], main_points[3]] {
            commands.push(PathCommand::move_to(p.x, p.y));
            commands.push(PathCommand::line_to(
                p.x + THREE_DEE_OFFSET as f64,
                p.y - y_offset as f64,
            ));
        }
        let border = node(
            format!("{}:3d-border", s.id),
            Some(Primitive::Path(Path {
                commands,
                stroke,
                ..Default::default()
            })),
        );
        Ok(vec![main, sides, border])
    }

    fn three_dee_side_paint(&self, s: &Shape) -> Result<Option<Paint>> {
        let darker = crate::golib::color::darken(&s.fill).unwrap_or_else(|_| s.fill.clone());
        self.paint(&darker, &format!("shape {} 3d side fill", q(&s.id)))
    }
}

fn shape_supports_shadow(shape_type: &str) -> bool {
    !matches!(
        shape_type,
        SHAPE_TEXT | SHAPE_CODE | SHAPE_CLASS | SHAPE_SQL_TABLE
    )
}

fn shape_effect_track(
    property: AnimationProperty,
    target_index: i64,
    start: AnimationValue,
    midpoint: AnimationValue,
    end: AnimationValue,
) -> Track {
    Track {
        property,
        target_index,
        delay: 0,
        duration: ANIMATED_SHAPE_DURATION,
        repeat: true,
        keyframes: Some(vec![
            Keyframe {
                offset: 0.0,
                value: start,
                ..Default::default()
            },
            Keyframe {
                offset: 0.5,
                value: midpoint,
                ..Default::default()
            },
            Keyframe {
                offset: 1.0,
                value: end,
                ..Default::default()
            },
        ]),
    }
}

/// configureAnimatedShape.
pub(crate) fn configure_animated_shape(group: &mut Node) {
    let declarations = filter_declarations();
    group.filters = Some(vec![
        Filter::DropShadow(declarations[0]),
        Filter::DropShadow(declarations[1]),
    ]);
    let (midpoints, endpoints) = (shadow_midpoints(), shadow_endpoints());
    group.animations = Some(vec![
        shape_effect_track(
            AnimationProperty::TRANSFORM,
            0,
            transform_value(Matrix::identity()),
            transform_value(Matrix::translate(0.0, -4.0)),
            transform_value(Matrix::identity()),
        ),
        shape_effect_track(
            AnimationProperty::DROP_SHADOW,
            0,
            shadow_value(endpoints[0]),
            shadow_value(midpoints[0]),
            shadow_value(endpoints[0]),
        ),
        shape_effect_track(
            AnimationProperty::DROP_SHADOW,
            1,
            shadow_value(endpoints[1]),
            shadow_value(midpoints[1]),
            shadow_value(endpoints[1]),
        ),
    ]);
}

/// finishShape: an animated shape keeps a no-op clip after its filters.
pub(crate) fn finish_shape(mut group: Node, s: &Shape) -> Node {
    if !s.animated || group.children().is_empty() {
        return group;
    }
    let mut visual = s.clone();
    visual.link.clear();
    visual.pretty_link.clear();
    visual.tooltip.clear();
    visual.tooltip_position.clear();
    let diagram = Diagram {
        shapes: Some(vec![visual]),
        ..Default::default()
    };
    let (top_left, bottom_right) = bounding_box(&diagram);
    if bottom_right.x <= top_left.x || bottom_right.y <= top_left.y {
        return group;
    }
    let bounds = Bounds::new(
        top_left.x as f64,
        top_left.y as f64,
        bottom_right.x as f64,
        bottom_right.y as f64,
    );
    group.clip = Some(box_clip(
        bounds
            .expand(
                ANIMATED_SHAPE_MAX_FILTER_EXPANSION,
                ANIMATED_SHAPE_MAX_FILTER_EXPANSION,
            )
            .to_box(),
    ));
    group
}

/// validateShapeEffects.
pub(crate) fn validate_shape_effects(object: &str, s: &Shape) -> Result<()> {
    if s.three_dee
        && !matches!(
            s.type_.as_str(),
            "" | SHAPE_RECTANGLE | SHAPE_SQUARE | SHAPE_HEXAGON
        )
    {
        return Err(unsupported(
            object,
            &format!("3d for shape type {}", s.type_),
        ));
    }
    if !s.three_dee && s.double_border {
        if !matches!(
            s.type_.as_str(),
            "" | SHAPE_RECTANGLE | SHAPE_SQUARE | SHAPE_OVAL | SHAPE_CIRCLE
        ) {
            return Err(unsupported(
                object,
                &format!("double border for shape type {}", s.type_),
            ));
        }
        let minimum = 2 * INNER_BORDER_OFFSET;
        if s.width < minimum {
            return Err(invalid_field(
                object,
                "width",
                Some(&s.width.to_string()),
                &format!("must be at least {minimum} for double border"),
            ));
        }
        if s.height < minimum {
            return Err(invalid_field(
                object,
                "height",
                Some(&s.height.to_string()),
                &format!("must be at least {minimum} for double border"),
            ));
        }
    }
    if !s.three_dee
        && s.multiple
        && matches!(
            s.type_.as_str(),
            SHAPE_TEXT | SHAPE_CODE | SHAPE_CLASS | SHAPE_SQL_TABLE | SHAPE_IMAGE
        )
    {
        return Err(unsupported(
            object,
            &format!("multiple for shape type {}", s.type_),
        ));
    }
    validate_shape_effect_integer_bounds(object, s)
}

fn validate_shape_effect_integer_bounds(object: &str, s: &Shape) -> Result<()> {
    if !s.three_dee && !s.multiple && !s.shadow {
        return Ok(());
    }
    let check = |field: &str, initial: i64, deltas: &[i64]| -> Result<()> {
        let mut value = initial;
        for delta in deltas {
            value = checked_add(value, *delta, MIN_INT, MAX_INT).ok_or_else(|| {
                invalid_field(
                    object,
                    field,
                    Some(&initial.to_string()),
                    "must fit raster effect bounds arithmetic",
                )
            })?;
        }
        Ok(())
    };
    if s.three_dee {
        let offset_y = if s.type_ == SHAPE_HEXAGON {
            THREE_DEE_OFFSET / 2
        } else {
            THREE_DEE_OFFSET
        };
        check(
            "pos.x",
            s.pos.x,
            &[s.width, THREE_DEE_OFFSET, s.stroke_width],
        )?;
        check("pos.y", s.pos.y, &[-offset_y, -s.stroke_width])?;
    }
    if !s.three_dee && s.multiple {
        check(
            "pos.x",
            s.pos.x,
            &[s.width, MULTIPLE_OFFSET, s.stroke_width],
        )?;
        check("pos.y", s.pos.y, &[-MULTIPLE_OFFSET, -s.stroke_width])?;
    }
    if s.shadow {
        let half_stroke = s.stroke_width / 2 + s.stroke_width % 2;
        check("pos.x", s.pos.x, &[s.width, half_stroke, SHADOW_SIZE_X])?;
        check("pos.y", s.pos.y, &[s.height, half_stroke, SHADOW_SIZE_Y])?;
    }
    Ok(())
}

pub(crate) fn double_border_pair(
    s: &Shape,
    outer_fill: Option<Paint>,
    inner_fill: Option<Paint>,
    stroke: Option<Stroke>,
    id_prefix: &str,
) -> Vec<Node> {
    let outer_box = SceneBox {
        x: s.pos.x as f64,
        y: s.pos.y as f64,
        width: s.width as f64,
        height: s.height as f64,
    };
    let offset = INNER_BORDER_OFFSET as f64;
    let inner_box = SceneBox {
        x: outer_box.x + offset,
        y: outer_box.y + offset,
        width: outer_box.width - 2.0 * offset,
        height: outer_box.height - 2.0 * offset,
    };
    let (mut outer, mut inner) = if s.type_ == SHAPE_OVAL || s.type_ == SHAPE_CIRCLE {
        let ellipse = |b: SceneBox, fill: Option<Paint>| {
            Node::new(Some(Primitive::Ellipse(Ellipse {
                center: Point::new(b.x + b.width / 2.0, b.y + b.height / 2.0),
                radius_x: b.width / 2.0,
                radius_y: b.height / 2.0,
                fill,
                stroke: stroke.clone(),
            })))
        };
        (
            ellipse(outer_box, outer_fill),
            ellipse(inner_box, inner_fill),
        )
    } else {
        let outer_radius =
            clamp_border_radius(s.border_radius as f64, outer_box.width, outer_box.height);
        let inner_radius =
            clamp_border_radius(s.border_radius as f64, inner_box.width, inner_box.height);
        (
            Node::new(Some(Primitive::Rect(Rect {
                r#box: outer_box,
                radius_x: outer_radius,
                radius_y: outer_radius,
                fill: outer_fill,
                stroke: stroke.clone(),
            }))),
            Node::new(Some(Primitive::Rect(Rect {
                r#box: inner_box,
                radius_x: inner_radius,
                radius_y: inner_radius,
                fill: inner_fill,
                stroke: stroke.clone(),
            }))),
        )
    };
    outer.id = format!("{id_prefix}:outer");
    inner.id = format!("{id_prefix}:inner");
    vec![outer, inner]
}

/// polygonPath.
pub(crate) fn polygon_path(points: &[Point], fill: Option<Paint>, stroke: Option<Stroke>) -> Path {
    let mut path = Path {
        fill,
        stroke,
        ..Default::default()
    };
    if points.is_empty() {
        return path;
    }
    path.commands
        .push(PathCommand::move_to(points[0].x, points[0].y));
    for p in &points[1..] {
        path.commands.push(PathCommand::line_to(p.x, p.y));
    }
    path.commands.push(PathCommand::close());
    path
}
