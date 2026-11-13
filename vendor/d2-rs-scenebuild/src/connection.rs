// SPDX-License-Identifier: MPL-2.0
//! connection.go: routes, animated dash tracks and their split, arrowheads.
//! ARM64 contraction sites (fma-sites-foundations.txt connection.go:276–614):
//! the real (inexact-product) ones are the split interpolation (281/282),
//! refX for target arrows (441) and diamonds (539/541) and the crow's-foot
//! offset (601); the others multiply by powers of two.

use d2_rs_scene::{
    AnimationProperty, Easing, EasingKind, Ellipse, Keyframe, LineCap, LineJoin, Matrix, Node,
    Paint, Path, PathCommand, PathCommandKind, Point, Primitive, Stroke, Track,
};
use d2_rs_target::{Connection, Shape};

use crate::golib::geo::{self, go_fmadd, go_fmsub, gomath};
use crate::golib::svgpath;
use crate::shape_effects::number_value;
use crate::target_ext::*;
use crate::util::*;
use crate::{Builder, validate_dash};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Provenance {
    Line,
    Cubic,
    SmoothCubic,
}

struct Segment {
    kind: Provenance,
    start: Point,
    command: PathCommand,
}

impl Builder<'_> {
    pub(crate) fn build_connection(&mut self, c: &Connection) -> Result<Node> {
        let stroke_paint = self.paint(&c.stroke, &format!("connection {} stroke", q(&c.id)))?;
        let mut stroke_dash = c.stroke_dash;
        if c.animated && stroke_dash == 0.0 {
            // Preserve Connection.CSSStyle's implicit animated dash pattern.
            stroke_dash = 5.0;
        }
        let mut stroke = self.stroke(
            &c.stroke,
            c.stroke_width,
            stroke_dash,
            LineCap::ROUND,
            LineJoin::ROUND,
            &format!("connection {} stroke", q(&c.id)),
        )?;
        let (src_adjustment, dst_adjustment) = arrowhead_adjustments(c, self.id_to_shape.as_ref());
        let (mut path, segments, start, end) = connection_path(c, src_adjustment, dst_adjustment);
        let mut forward_track = None;
        let mut reverse_track = None;
        if let Some(stroke) = stroke.as_mut().filter(|_| c.animated) {
            let (base_offset, duration) = animated_connection_timing(c)
                .map_err(|e| Error(format!("scene: connection {} animation: {e}", q(&c.id))))?;
            stroke.dash_offset = base_offset;
            forward_track = Some(animated_connection_track(base_offset, duration, false));
            reverse_track = Some(animated_connection_track(base_offset, duration, true));
        }
        path.stroke = stroke;
        if self.options.sketch && !c.animated {
            path = self.sketch_connection_path(c, path)?;
        }

        let mut group = node(c.id.clone(), None);
        group.classes = c.classes.clone().filter(|v| !v.is_empty());
        group.opacity = c.opacity;
        let mut group_children = Vec::new();
        if let Some(icon) = self.build_connection_icon(c)? {
            group_children.push(icon);
        }
        let points = route_points(c);
        let mut src_arrow_node = None;
        let mut dst_arrow_node = None;
        if c.src_arrow != NO_ARROWHEAD {
            src_arrow_node = Some(self.arrowhead(
                c,
                &c.src_arrow,
                false,
                start,
                points[0].vector_to(&points[1]),
                stroke_paint.clone(),
            )?);
        }
        if c.dst_arrow != NO_ARROWHEAD {
            let last = points.len() - 1;
            dst_arrow_node = Some(self.arrowhead(
                c,
                &c.dst_arrow,
                true,
                end,
                points[last - 1].vector_to(&points[last]),
                stroke_paint.clone(),
            )?);
        }

        let mut geometry_children = Vec::new();
        let split_directions =
            c.animated && (c.src_arrow == NO_ARROWHEAD) == (c.dst_arrow == NO_ARROWHEAD);
        if split_directions {
            let (reverse_path, forward_path) = split_animated_connection_path(&path, &segments)
                .map_err(|e| {
                    Error(format!(
                        "scene: connection {} animation split: {e}",
                        q(&c.id)
                    ))
                })?;
            geometry_children.push(animated_connection_path_node(
                format!("{}:path:reverse", c.id),
                reverse_path,
                reverse_track,
            ));
            geometry_children.extend(src_arrow_node);
            geometry_children.push(animated_connection_path_node(
                format!("{}:path:forward", c.id),
                forward_path,
                forward_track,
            ));
            geometry_children.extend(dst_arrow_node);
        } else {
            geometry_children.push(animated_connection_path_node(
                format!("{}:path", c.id),
                path,
                forward_track,
            ));
            geometry_children.extend(src_arrow_node);
            geometry_children.extend(dst_arrow_node);
        }
        match &self.connection_mask {
            None => group_children.extend(geometry_children),
            Some(mask) => {
                let mut geometry = node(format!("{}:geometry", c.id), None);
                geometry.mask = Some(mask.clone());
                geometry.children = opt_vec(geometry_children);
                group_children.push(geometry);
            }
        }
        group_children.extend(self.build_connection_labels(c)?);
        append_children(&mut group, group_children);
        Ok(group)
    }

    #[allow(clippy::too_many_arguments)]
    fn arrowhead(
        &mut self,
        c: &Connection,
        arrowhead: &str,
        target: bool,
        endpoint: Point,
        tangent: geo::Vector,
        stroke_paint: Option<Paint>,
    ) -> Result<Node> {
        if tangent.length() == 0.0 {
            return err(format!(
                "scene: connection {} has zero-length arrowhead tangent",
                q(&c.id)
            ));
        }
        if self.options.sketch {
            return self.sketch_arrowhead(c, arrowhead, target, endpoint, tangent);
        }
        let stroke_width = c.stroke_width as f64;
        let (width, height) = arrowhead_dimensions(arrowhead, stroke_width);
        let mut ref_x = 1.5 * stroke_width;
        if target {
            // ARM64 connection.go:441: FMSUBD.
            ref_x = go_fmsub(1.5, stroke_width, width);
        }
        let background = self.paint(
            BG_COLOR,
            &format!("connection {} arrowhead background", q(&c.id)),
        )?;
        let endpoint_name = if target { "dst" } else { "src" };
        let arrow_id = format!("{}:{endpoint_name}-arrowhead", c.id);
        let polygon = |points: &[Point]| {
            let mut commands = vec![PathCommand::move_to(points[0].x, points[0].y)];
            for p in &points[1..] {
                commands.push(PathCommand::line_to(p.x, p.y));
            }
            commands.push(PathCommand::close());
            Path {
                commands,
                fill: stroke_paint.clone(),
                ..Default::default()
            }
        };
        let polyline = |points: &[Point]| {
            let mut commands = vec![PathCommand::move_to(points[0].x, points[0].y)];
            for p in &points[1..] {
                commands.push(PathCommand::line_to(p.x, p.y));
            }
            Path {
                commands,
                ..Default::default()
            }
        };
        let outline_stroke = || Stroke {
            paint: stroke_paint.clone(),
            width: stroke_width,
            cap: LineCap::BUTT,
            join: LineJoin::MITER,
            miter_limit: 4.0,
            ..Default::default()
        };
        let round_stroke = || Stroke {
            paint: stroke_paint.clone(),
            width: stroke_width,
            join: LineJoin::ROUND,
            ..Default::default()
        };
        let child = |suffix: &str, primitive: Primitive, transform: Matrix| {
            let mut n = node(format!("{arrow_id}:{suffix}"), Some(primitive));
            n.transform = transform;
            n
        };
        let p = Point::new;
        let mut primitive = None;
        let mut children = Vec::new();
        match arrowhead {
            "arrow" => {
                primitive = Some(Primitive::Path(if target {
                    polygon(&[
                        p(0.0, 0.0),
                        p(width, height / 2.0),
                        p(0.0, height),
                        p(width / 4.0, height / 2.0),
                    ])
                } else {
                    polygon(&[
                        p(0.0, height / 2.0),
                        p(width, 0.0),
                        p(width * 0.75, height / 2.0),
                        p(width, height),
                    ])
                }));
            }
            "triangle" => {
                primitive = Some(Primitive::Path(if target {
                    polygon(&[p(0.0, 0.0), p(width, height / 2.0), p(0.0, height)])
                } else {
                    polygon(&[p(width, 0.0), p(0.0, height / 2.0), p(width, height)])
                }));
            }
            "unfilled-triangle" => {
                let inset = stroke_width / 2.0;
                let mut path = if target {
                    polygon(&[
                        p(inset, inset),
                        p(width - inset, height / 2.0),
                        p(inset, height - inset),
                    ])
                } else {
                    polygon(&[
                        p(width - inset, inset),
                        p(inset, height / 2.0),
                        p(width - inset, height - inset),
                    ])
                };
                path.fill = background.clone();
                path.stroke = Some(round_stroke());
                primitive = Some(Primitive::Path(path));
            }
            "line" => {
                let inset = stroke_width / 2.0;
                let points = if target {
                    [
                        p(inset, inset),
                        p(width - inset, height / 2.0),
                        p(inset, height - inset),
                    ]
                } else {
                    [
                        p(width - inset, inset),
                        p(inset, height / 2.0),
                        p(width - inset, height - inset),
                    ]
                };
                let mut path = polyline(&points);
                path.stroke = Some(outline_stroke());
                primitive = Some(Primitive::Path(path));
            }
            "filled-diamond" => {
                primitive = Some(Primitive::Path(polygon(&[
                    p(0.0, height / 2.0),
                    p(width / 2.0, 0.0),
                    p(width, height / 2.0),
                    p(width / 2.0, height),
                ])));
            }
            "diamond" => {
                let mut path = if target {
                    polygon(&[
                        p(0.0, height / 2.0),
                        p(width / 2.0, height / 8.0),
                        p(width, height / 2.0),
                        p(width / 2.0, height * 0.9),
                    ])
                } else {
                    polygon(&[
                        p(width / 8.0, height / 2.0),
                        p(width * 0.6, height / 8.0),
                        p(width * 1.1, height / 2.0),
                        p(width * 0.6, height * 7.0 / 8.0),
                    ])
                };
                path.fill = background.clone();
                path.stroke = Some(round_stroke());
                // ARM64 connection.go:539/541.
                ref_x = if target {
                    go_fmsub(0.6, stroke_width, width)
                } else {
                    go_fmadd(0.6, stroke_width, width / 8.0)
                };
                primitive = Some(Primitive::Path(path));
            }
            "filled-circle" | "circle" => {
                let radius = width / 2.0;
                let center_x = if target {
                    radius + stroke_width / 2.0
                } else {
                    radius - stroke_width / 2.0
                };
                let mut ellipse = Ellipse {
                    center: p(center_x, radius),
                    radius_x: radius - stroke_width / 2.0,
                    radius_y: radius - stroke_width / 2.0,
                    fill: stroke_paint.clone(),
                    stroke: None,
                };
                if arrowhead == "circle" {
                    ellipse.radius_x = radius - stroke_width;
                    ellipse.radius_y = radius - stroke_width;
                    ellipse.fill = background.clone();
                    ellipse.stroke = Some(round_stroke());
                }
                primitive = Some(Primitive::Ellipse(ellipse));
            }
            "cross" => {
                let inset = stroke_width / 8.0;
                let mut cross = polygon(&[
                    p(0.0, height / 2.0 + inset),
                    p(width / 2.0 - inset, height / 2.0 + inset),
                    p(width / 2.0 - inset, height),
                    p(width / 2.0 + inset, height),
                    p(width / 2.0 + inset, height / 2.0 + inset),
                    p(width, height / 2.0 + inset),
                    p(width, height / 2.0 - inset),
                    p(width / 2.0 + inset, height / 2.0 - inset),
                    p(width / 2.0 + inset, 0.0),
                    p(width / 2.0 - inset, 0.0),
                    p(width / 2.0 - inset, height / 2.0 - inset),
                    p(0.0, height / 2.0 - inset),
                ]);
                cross.fill = background.clone();
                cross.stroke = Some(outline_stroke());
                let origin = p(width / 2.0, height / 2.0);
                let rotated_origin = Matrix::rotate(std::f64::consts::PI / 4.0).point(origin);
                let cross_transform = Matrix::translate(
                    -rotated_origin.x + width / 2.0,
                    -rotated_origin.y + height / 2.0,
                )
                .mul(Matrix::rotate(std::f64::consts::PI / 4.0));
                children.push(child("cross", Primitive::Path(cross), cross_transform));
                let stem_end_x = if target { width } else { 0.0 };
                let mut stem =
                    polyline(&[p(width / 2.0, height / 2.0), p(stem_end_x, height / 2.0)]);
                stem.fill = background.clone();
                stem.stroke = Some(outline_stroke());
                children.push(child("stem", Primitive::Path(stem), Matrix::identity()));
            }
            "filled-box" | "box" => {
                let inset = if arrowhead == "box" {
                    stroke_width / 2.0
                } else {
                    0.0
                };
                let mut path = polygon(&[
                    p(inset, inset),
                    p(inset, height - inset),
                    p(width - inset, height - inset),
                    p(width - inset, inset),
                ]);
                if arrowhead == "box" {
                    path.fill = background.clone();
                    path.stroke = Some(Stroke {
                        paint: stroke_paint.clone(),
                        width: stroke_width,
                        join: LineJoin::MITER,
                        miter_limit: 4.0,
                        ..Default::default()
                    });
                }
                primitive = Some(Primitive::Path(path));
            }
            "cf-one" | "cf-many" | "cf-one-required" | "cf-many-required" => {
                // ARM64 connection.go:601: FMADDD.
                let offset = go_fmadd(stroke_width, 1.8, 3.0);
                let internal_transform = if target {
                    Matrix::identity()
                } else {
                    Matrix::scale(-1.0, -1.0).mul(Matrix::translate(-width, -height))
                };
                if arrowhead == "cf-one-required" || arrowhead == "cf-many-required" {
                    let mut modifier = polyline(&[p(offset, 0.0), p(offset, height)]);
                    modifier.fill = background.clone();
                    modifier.stroke = Some(outline_stroke());
                    children.push(child(
                        "modifier",
                        Primitive::Path(modifier),
                        internal_transform,
                    ));
                } else {
                    let modifier = Ellipse {
                        center: p(offset / 2.0 + 2.0, height / 2.0),
                        radius_x: offset / 2.0,
                        radius_y: offset / 2.0,
                        fill: background.clone(),
                        stroke: Some(outline_stroke()),
                    };
                    children.push(child(
                        "modifier",
                        Primitive::Ellipse(modifier),
                        internal_transform,
                    ));
                }
                let commands = if arrowhead == "cf-many" || arrowhead == "cf-many-required" {
                    vec![
                        PathCommand::move_to(width - 3.0, height / 2.0),
                        PathCommand::line_to(width + offset, height / 2.0),
                        PathCommand::move_to(offset + 3.0, height / 2.0),
                        PathCommand::line_to(width + offset, 0.0),
                        PathCommand::move_to(offset + 3.0, height / 2.0),
                        PathCommand::line_to(width + offset, height),
                    ]
                } else {
                    vec![
                        PathCommand::move_to(width - 3.0, height / 2.0),
                        PathCommand::line_to(width + offset, height / 2.0),
                        PathCommand::move_to(offset * 2.0, 0.0),
                        PathCommand::line_to(offset * 2.0, height),
                    ]
                };
                let marks = Path {
                    commands,
                    fill: background.clone(),
                    stroke: Some(outline_stroke()),
                    ..Default::default()
                };
                children.push(child("marks", Primitive::Path(marks), internal_transform));
            }
            other => {
                return Err(unsupported(
                    &format!("connection {}", q(&c.id)),
                    &format!("arrowhead {other}"),
                ));
            }
        }
        let angle = gomath::atan2(tangent.0[1], tangent.0[0]);
        let mut n = node(arrow_id, primitive);
        n.children = opt_vec(children);
        n.transform = Matrix::translate(endpoint.x, endpoint.y)
            .mul(Matrix::rotate(angle))
            .mul(Matrix::translate(-ref_x, -height / 2.0));
        Ok(n)
    }
}

fn animated_connection_path_node(id: String, path: Path, track: Option<Track>) -> Node {
    let mut n = node(id, Some(Primitive::Path(path)));
    if let Some(track) = track {
        n.animations = Some(vec![track]);
    }
    n
}

/// validateAnimatedConnection.
pub(crate) fn validate_animated_connection(object: &str, c: &Connection) -> Result<()> {
    let stroke_dash = if c.stroke_dash == 0.0 {
        5.0
    } else {
        c.stroke_dash
    };
    validate_dash(object, stroke_dash, c.stroke_width)?;
    if c.stroke_width == 0 {
        return Ok(());
    }
    if let Err(e) = animated_connection_timing(c) {
        return Err(invalid_field(
            object,
            "strokeDash",
            Some(&fv(c.stroke_dash)),
            &e,
        ));
    }
    Ok(())
}

/// animatedConnectionTiming: the dash offset and duration in nanoseconds.
fn animated_connection_timing(c: &Connection) -> std::result::Result<(f64, i64), String> {
    let stroke_dash = if c.stroke_dash == 0.0 {
        5.0
    } else {
        c.stroke_dash
    };
    let (dash_size, gap_size) =
        svgpath::get_stroke_dash_attributes(c.stroke_width as f64, stroke_dash);
    let period = dash_size + gap_size;
    if !finite(period) || period < 0.0 {
        return Err("must produce a finite non-negative dash period".into());
    }
    let direction = if c.src_arrow != NO_ARROWHEAD && c.dst_arrow == NO_ARROWHEAD {
        10.0
    } else {
        -10.0
    };
    let base_offset = direction * period;
    if !finite(base_offset) {
        return Err("must produce a finite dash offset".into());
    }
    let duration_nanoseconds = gap_size * 0.5 * 1e9;
    if !finite(duration_nanoseconds)
        || duration_nanoseconds < 1.0
        || duration_nanoseconds > i64::MAX as f64
    {
        return Err(
            "must produce an animation duration within [1ns,2562047h47m16.854775807s]".into(),
        );
    }
    let duration = duration_nanoseconds as i64;
    if duration <= 0 {
        return Err("must produce a positive animation duration".into());
    }
    Ok((base_offset, duration))
}

fn animated_connection_track(base_offset: f64, duration: i64, reverse: bool) -> Track {
    let (mut start, mut end) = (0.0, base_offset);
    if reverse {
        (start, end) = (end, start);
    }
    Track {
        property: AnimationProperty::STROKE_DASH_OFFSET,
        target_index: 0,
        delay: 0,
        duration,
        repeat: true,
        keyframes: Some(vec![
            Keyframe {
                offset: 0.0,
                value: number_value(start),
                easing: Easing {
                    kind: EasingKind::LINEAR,
                    ..Default::default()
                },
            },
            Keyframe {
                offset: 1.0,
                value: number_value(end),
                ..Default::default()
            },
        ]),
    }
}

fn connection_segment_endpoint(command: &PathCommand) -> std::result::Result<Point, String> {
    match command.kind {
        PathCommandKind::LINE => Ok(command.p1),
        PathCommandKind::CUBIC => Ok(command.p3),
        other => Err(format!("unsupported path command {}", other.0)),
    }
}

/// splitAnimatedConnectionPath: svg.SplitPath at 50% over typed commands.
fn split_animated_connection_path(
    path: &Path,
    segments: &[Segment],
) -> std::result::Result<(Path, Path), String> {
    if path
        .commands
        .first()
        .is_none_or(|c| c.kind != PathCommandKind::MOVE)
        || path.commands.len() != segments.len() + 1
    {
        return Err("invalid typed connection path".into());
    }
    let mut total_length = 0.0;
    let mut lengths = Vec::with_capacity(segments.len());
    for (i, segment) in segments.iter().enumerate() {
        let end = connection_segment_endpoint(&segment.command)
            .map_err(|e| format!("segment {i}: {e}"))?;
        let length = gomath::hypot(end.x - segment.start.x, end.y - segment.start.y);
        if !finite(length) {
            return Err(format!("segment {i} has non-finite chord length"));
        }
        lengths.push(length);
        total_length += length;
        if !finite(total_length) {
            return Err("cumulative chord length is non-finite".into());
        }
    }
    if total_length <= 0.0 {
        return Err("path has zero painted length".into());
    }
    let mut first = Path {
        fill_rule: path.fill_rule,
        fill: path.fill.clone(),
        stroke: path.stroke.clone(),
        commands: Vec::new(),
    };
    let mut second = first.clone();
    first.commands.push(path.commands[0]);
    let half_length = total_length * 0.5;
    let mut cumulative = 0.0;
    let mut past_half = false;
    for (i, segment) in segments.iter().enumerate() {
        let length = lengths[i];
        cumulative += length;
        if past_half {
            second.commands.push(segment.command);
            continue;
        }
        if cumulative < half_length {
            first.commands.push(segment.command);
            continue;
        }
        let end = connection_segment_endpoint(&segment.command)
            .map_err(|e| format!("segment {i}: {e}"))?;
        match segment.kind {
            Provenance::Line => {
                if length == 0.0 {
                    return Err("line split has zero chord length".into());
                }
                let t = (half_length - cumulative + length) / length;
                if !finite(t) || !(0.0..=1.0).contains(&t) {
                    return Err(format!("line split parameter {} is outside [0,1]", fv(t)));
                }
                // ARM64 connection.go:281/282: FMADDD.
                let split = Point::new(
                    go_fmadd(end.x - segment.start.x, t, segment.start.x),
                    go_fmadd(end.y - segment.start.y, t, segment.start.y),
                );
                first.commands.push(PathCommand::line_to(split.x, split.y));
                second.commands.push(PathCommand::move_to(split.x, split.y));
                second.commands.push(PathCommand::line_to(end.x, end.y));
            }
            Provenance::Cubic => {
                if segment.command.kind != PathCommandKind::CUBIC {
                    return Err("C provenance has non-cubic command".into());
                }
                let p1 = geo::Point::new(segment.start.x, segment.start.y);
                let p2 = geo::Point::new(segment.command.p1.x, segment.command.p1.y);
                let p3 = geo::Point::new(segment.command.p2.x, segment.command.p2.y);
                let p4 = geo::Point::new(end.x, end.y);
                let (_, q2, q3, q4) = svgpath::bezier_curve_segment(&p1, &p2, &p3, &p4, 0.0, 0.5);
                first
                    .commands
                    .push(PathCommand::cubic_to(q2.x, q2.y, q3.x, q3.y, q4.x, q4.y));
                let (q1, q2, q3, q4) = svgpath::bezier_curve_segment(&p1, &p2, &p3, &p4, 0.5, 1.0);
                second.commands.push(PathCommand::move_to(q1.x, q1.y));
                second
                    .commands
                    .push(PathCommand::cubic_to(q2.x, q2.y, q3.x, q3.y, q4.x, q4.y));
            }
            Provenance::SmoothCubic => {
                first.commands.push(segment.command);
                second.commands.push(PathCommand::move_to(end.x, end.y));
            }
        }
        past_half = true;
    }
    if !past_half || second.commands.is_empty() {
        return Err("path did not cross its halfway point".into());
    }
    Ok((first, second))
}

/// arrowheadAdjustment.
fn arrowhead_adjustment(
    start: geo::Point,
    end: geo::Point,
    arrowhead: &str,
    edge_stroke_width: i64,
    shape_stroke_width: i64,
) -> geo::Point {
    let mut distance = (edge_stroke_width as f64 + shape_stroke_width as f64) / 2.0;
    if arrowhead != NO_ARROWHEAD {
        distance += edge_stroke_width as f64;
    }
    let vector = geo::Vector::new(&[end.x - start.x, end.y - start.y]);
    if vector.length() == 0.0 {
        return geo::Point::new(0.0, 0.0);
    }
    vector.unit().multiply(-distance).to_point()
}

fn arrowhead_adjustments(
    c: &Connection,
    shapes: Option<&std::collections::HashMap<String, Shape>>,
) -> (geo::Point, geo::Point) {
    let stroke_of = |id: &str| shapes.and_then(|m| m.get(id)).map_or(0, |s| s.stroke_width);
    let route = route_points(c);
    let src = arrowhead_adjustment(
        route[1],
        route[0],
        &c.src_arrow,
        c.stroke_width,
        stroke_of(&c.src),
    );
    let last = route.len() - 1;
    let dst = arrowhead_adjustment(
        route[last - 1],
        route[last],
        &c.dst_arrow,
        c.stroke_width,
        stroke_of(&c.dst),
    );
    (src, dst)
}

/// connectionPath: the typed route with rounded corners and provenance.
fn connection_path(
    c: &Connection,
    src_adjustment: geo::Point,
    dst_adjustment: geo::Point,
) -> (Path, Vec<Segment>, Point, Point) {
    let route = route_points(c);
    let start = Point::new(route[0].x + src_adjustment.x, route[0].y + src_adjustment.y);
    let last_point = route[route.len() - 1];
    let end = Point::new(
        last_point.x + dst_adjustment.x,
        last_point.y + dst_adjustment.y,
    );
    let mut path = Path {
        commands: vec![PathCommand::move_to(start.x, start.y)],
        ..Default::default()
    };
    let mut segments: Vec<Segment> = Vec::with_capacity(route.len());
    let mut append =
        |path: &mut Path, kind: Provenance, segment_start: Point, command: PathCommand| {
            path.commands.push(command);
            segments.push(Segment {
                kind,
                start: segment_start,
                command,
            });
        };
    if c.is_curve {
        let mut current = start;
        let mut i = 1;
        while i < route.len() {
            let mut endpoint = route[i + 2];
            if i + 2 == route.len() - 1 {
                endpoint =
                    geo::Point::new(endpoint.x + dst_adjustment.x, endpoint.y + dst_adjustment.y);
            }
            let command = PathCommand::cubic_to(
                route[i].x,
                route[i].y,
                route[i + 1].x,
                route[i + 1].y,
                endpoint.x,
                endpoint.y,
            );
            append(&mut path, Provenance::Cubic, current, command);
            current = command.p3;
            i += 3;
        }
        return (path, segments, start, end);
    }
    let mut current = start;
    let mut i = 1;
    while i + 1 < route.len() {
        let previous_source = route[i - 1];
        let corner = route[i];
        let next = route[i + 1];
        let previous_vector = previous_source.vector_to(&corner);
        let current_vector = corner.vector_to(&next);
        if previous_vector.length() == 0.0 || current_vector.length() == 0.0 {
            append(
                &mut path,
                Provenance::Line,
                current,
                PathCommand::line_to(corner.x, corner.y),
            );
            current = Point::new(corner.x, corner.y);
            i += 1;
            continue;
        }
        let distance = geo::euclidean_distance(corner.x, corner.y, next.x, next.y);
        let units = geo::go_min(c.border_radius, distance / 2.0);
        let previous_translation = previous_vector.unit().multiply(units).to_point();
        let current_translation = current_vector.unit().multiply(units).to_point();
        let line_end = Point::new(
            corner.x - previous_translation.x,
            corner.y - previous_translation.y,
        );
        append(
            &mut path,
            Provenance::Line,
            current,
            PathCommand::line_to(line_end.x, line_end.y),
        );
        current = line_end;
        if units < c.border_radius && i < route.len() - 2 {
            let next_target = route[i + 2];
            let next_vector = next.vector_to(&next_target);
            if next_vector.length() == 0.0 {
                i += 1;
                continue;
            }
            i += 1;
            let next_translation = next_vector.unit().multiply(units).to_point();
            let curve_end = Point::new(next.x + next_translation.x, next.y + next_translation.y);
            let command = PathCommand::cubic_to(
                corner.x + previous_translation.x,
                corner.y + previous_translation.y,
                next.x - next_translation.x,
                next.y - next_translation.y,
                curve_end.x,
                curve_end.y,
            );
            append(&mut path, Provenance::Cubic, current, command);
            current = curve_end;
        } else {
            let curve_end = Point::new(
                corner.x + current_translation.x,
                corner.y + current_translation.y,
            );
            let command = PathCommand::cubic_to(
                current.x,
                current.y,
                corner.x,
                corner.y,
                curve_end.x,
                curve_end.y,
            );
            append(&mut path, Provenance::SmoothCubic, current, command);
            current = curve_end;
        }
        i += 1;
    }
    append(
        &mut path,
        Provenance::Line,
        current,
        PathCommand::line_to(end.x, end.y),
    );
    (path, segments, start, end)
}
