// SPDX-License-Identifier: MPL-2.0
//! Structured sketch geometry. Sources: d2sketch/scene.go and
//! d2scenebuild/{sketch,sketch_shapes}.go at the manifest pin.
//!
//! Conversion lives here because the scene model depends on the SVG renderer,
//! which already depends on rough-go's generator; reversing that edge cycles.

use d2_rs_scene::{
    BlendMode, Box as SceneBox, Ellipse, FillRule, LineCap, LineJoin, Matrix, Node, Nrgba, Paint,
    PaintUnits, Path, PathCommand, PathCommandKind, PatternPaint, Point, Primitive, Rect, Stroke,
};
use d2_rs_sketch::{Drawable, Generator, OpSetType, OpType, Options};
use d2_rs_target::Shape;

use crate::golib::shape::Shape as _;
use crate::golib::{color, label::Position, svgpath};
use crate::patterns;
use crate::target_ext::*;
use crate::util::*;
use crate::{Builder, target_geometry};

struct ScenePath {
    path: Path,
    fill: String,
    stroke: String,
    stroke_width: f64,
    dash: Vec<f64>,
    dash_offset: f64,
}

fn none_if_empty(raw: &str) -> &str {
    if raw.is_empty() { "none" } else { raw }
}

/// d2sketch.CompileScenePaths. Operation counts are checked before conversion;
/// coordinates are truncated numerically, never serialized or parsed as SVG.
fn compile_scene_paths(drawable: &Drawable, max_sets: i64, max_ops: i64) -> Result<Vec<ScenePath>> {
    if max_sets <= 0 || max_ops <= 0 {
        return err("d2sketch: scene conversion limits must be positive");
    }
    if drawable.sets.len() as i64 > max_sets {
        return err(format!(
            "d2sketch: rough operation set count {} exceeds limit {max_sets}",
            drawable.sets.len()
        ));
    }
    let mut total_ops = 0;
    for (index, set) in drawable.sets.iter().enumerate() {
        if set.ops.len() as i64 > max_ops - total_ops {
            return err(format!(
                "d2sketch: rough operation count exceeds limit {max_ops} at set {index}"
            ));
        }
        total_ops += set.ops.len() as i64;
    }
    let options = drawable
        .options
        .as_ref()
        .ok_or_else(|| Error("d2sketch: rough drawable has no resolved options".into()))?;
    if options.fixed_decimal_place_digits.is_some() {
        return err(
            "d2sketch: fixed-decimal rough output is unsupported in typed scene conversion",
        );
    }
    let mut paths = Vec::with_capacity(drawable.sets.len());
    for (set_index, set) in drawable.sets.iter().enumerate() {
        let set_error = |message: String| {
            Error(format!(
                "d2sketch: rough operation set {set_index}: {message}"
            ))
        };
        if let OpSetType::Path2DFill | OpSetType::Path2DPattern = set.kind {
            let kind = if set.kind == OpSetType::Path2DFill {
                "path2Dfill"
            } else {
                "path2Dpattern"
            };
            return err(format!(
                "d2sketch: SVG path2D operation type {kind:?} is unsupported in typed scene conversion"
            ));
        }
        if set.ops.is_empty() {
            continue;
        }
        let mut commands = Vec::with_capacity(set.ops.len());
        for (index, op) in set.ops.iter().enumerate() {
            let verb = match op.op {
                OpType::Move => "move",
                OpType::LineTo => "lineTo",
                OpType::BCurveTo => "bcurveTo",
                OpType::QCurveTo => "qcurveTo",
            };
            let operation_error =
                |message: String| set_error(format!("operation {index} {verb:?}: {message}"));
            let mut data = Vec::with_capacity(op.data.len());
            for (coordinate, value) in op.data.iter().copied().enumerate() {
                if !value.is_finite() {
                    return Err(operation_error(format!(
                        "coordinate {coordinate} is not finite"
                    )));
                }
                let scaled = value * 1_000_000.0;
                if !scaled.is_finite() {
                    return Err(operation_error(format!(
                        "coordinate {coordinate} is too large for six-place normalization"
                    )));
                }
                data.push(scaled.trunc() / 1_000_000.0);
            }
            let want = match op.op {
                OpType::Move | OpType::LineTo => 2,
                OpType::BCurveTo => 6,
                OpType::QCurveTo => 4,
            };
            if data.len() != want {
                return Err(set_error(format!(
                    "operation {index} {verb:?} has {} coordinates, want {want}",
                    data.len()
                )));
            }
            commands.push(match op.op {
                OpType::Move => PathCommand::move_to(data[0], data[1]),
                OpType::LineTo => PathCommand::line_to(data[0], data[1]),
                OpType::BCurveTo => {
                    PathCommand::cubic_to(data[0], data[1], data[2], data[3], data[4], data[5])
                }
                OpType::QCurveTo => PathCommand::quadratic_to(data[0], data[1], data[2], data[3]),
            });
        }
        let mut source = ScenePath {
            path: Path {
                commands,
                ..Default::default()
            },
            fill: String::new(),
            stroke: String::new(),
            stroke_width: 0.0,
            dash: Vec::new(),
            dash_offset: 0.0,
        };
        match set.kind {
            OpSetType::Path => {
                source.fill = "none".into();
                source.stroke = none_if_empty(&options.stroke).into();
                source.stroke_width = options.stroke_width;
                source.dash = options.stroke_line_dash.clone();
                source.dash_offset = options.stroke_line_dash_offset;
            }
            OpSetType::FillPath => {
                source.fill = none_if_empty(&options.fill).into();
                source.stroke = "none".into();
            }
            OpSetType::FillSketch => {
                source.fill = "none".into();
                source.stroke = none_if_empty(&options.fill).into();
                source.stroke_width = if options.fill_weight < 0.0 {
                    options.stroke_width / 2.0
                } else {
                    options.fill_weight
                };
                source.dash = options.fill_line_dash.clone();
                source.dash_offset = options.fill_line_dash_offset;
            }
            _ => unreachable!("path2D rejected above"),
        }
        if !source.stroke_width.is_finite() || source.stroke_width < 0.0 {
            return Err(set_error(
                "stroke width must be finite and non-negative".into(),
            ));
        }
        if !source.dash_offset.is_finite() {
            return Err(set_error("dash offset must be finite".into()));
        }
        for (index, dash) in source.dash.iter().enumerate() {
            if !dash.is_finite() || *dash < 0.0 {
                return Err(set_error(format!(
                    "dash {index} must be finite and non-negative"
                )));
            }
        }
        paths.push(source);
    }
    Ok(paths)
}

fn generator() -> Generator {
    Generator::new(
        Some(&Options {
            seed: Some(1.0),
            ..Default::default()
        }),
        None,
    )
}

fn shape_options(stroke: Option<&Stroke>) -> Options {
    Options {
        fill: Some("#000".into()),
        stroke: Some("#000".into()),
        stroke_width: Some(stroke.map_or(0.0, |s| s.width)),
        fill_weight: Some(2.0),
        hachure_gap: Some(16.0),
        fill_style: Some("solid".into()),
        bowing: Some(2.0),
        seed: Some(1.0),
        stroke_line_dash: stroke.and_then(|s| s.dashes.clone()),
        stroke_line_dash_offset: Some(stroke.map_or(0.0, |s| s.dash_offset)),
        ..Default::default()
    }
}

impl Builder<'_> {
    fn charge_sketch_commands(&mut self, object: &str, count: usize) -> Result<()> {
        let budget = self.options.sketch_budget.max_path_commands;
        if count as i64 > budget - self.sketch_path_commands {
            return err(format!(
                "scene: {object} sketch path command count exceeds limit {budget}"
            ));
        }
        self.sketch_path_commands += count as i64;
        Ok(())
    }

    fn remaining_sketch_operations(&self) -> i64 {
        let b = self.options.sketch_budget;
        (b.max_operations - self.sketch_operations)
            .min(b.max_path_commands - self.sketch_path_commands)
    }

    fn compile_sketch_drawable(
        &mut self,
        object: &str,
        drawable: &Drawable,
    ) -> Result<Vec<ScenePath>> {
        let b = self.options.sketch_budget;
        let sets = b.max_operation_sets - self.sketch_operation_sets;
        let ops = b.max_operations - self.sketch_operations;
        let commands = b.max_path_commands - self.sketch_path_commands;
        for (remaining, name, budget) in [
            (sets, "operation set", b.max_operation_sets),
            (ops, "operation", b.max_operations),
            (commands, "path command", b.max_path_commands),
        ] {
            if remaining <= 0 {
                return err(format!(
                    "scene: {object} sketch {name} count exceeds limit {budget}"
                ));
            }
        }
        let paths = compile_scene_paths(drawable, sets, ops)
            .map_err(|e| Error(format!("scene: {object} sketch geometry: {e}")))?;
        let count = paths.iter().map(|p| p.path.commands.len()).sum();
        if count as i64 > commands {
            return err(format!(
                "scene: {object} sketch path command count exceeds limit {}",
                b.max_path_commands
            ));
        }
        self.sketch_operation_sets += drawable.sets.len() as i64;
        self.sketch_operations += drawable
            .sets
            .iter()
            .map(|s| s.ops.len() as i64)
            .sum::<i64>();
        self.charge_sketch_commands(object, count)?;
        Ok(paths)
    }

    fn mask_sketch_shape(&self, n: &mut Node, s: &Shape) {
        if !s.text.label.is_empty() && Position::from_string(&s.label_position).is_border() {
            n.mask = self.connection_mask.clone();
        }
    }

    fn compile_shape_drawable(
        &mut self,
        s: &Shape,
        object: &str,
        prefix: &str,
        drawable: &Drawable,
        transform: Matrix,
    ) -> Result<Vec<Node>> {
        let paths = self.compile_sketch_drawable(object, drawable)?;
        let fill_raw = if s.type_ == SHAPE_CLASS || s.type_ == SHAPE_SQL_TABLE {
            &s.stroke
        } else {
            &s.fill
        };
        let stroke_raw = if s.type_ == SHAPE_CLASS || s.type_ == SHAPE_SQL_TABLE {
            &s.fill
        } else {
            &s.stroke
        };
        let fill = self.paint(none_if_empty(fill_raw), &format!("{object} fill"))?;
        let mut stroke = self.stroke(
            stroke_raw,
            s.stroke_width,
            s.stroke_dash,
            LineCap::ROUND,
            LineJoin::ROUND,
            &format!("{object} stroke"),
        )?;
        if let Some(stroke) = &mut stroke {
            stroke.miter_limit = 0.0;
        }
        self.paint_sketch_paths(s, prefix, paths, transform, fill, stroke)
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_sketch_paths(
        &mut self,
        s: &Shape,
        prefix: &str,
        paths: Vec<ScenePath>,
        transform: Matrix,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    ) -> Result<Vec<Node>> {
        let mut nodes = Vec::new();
        for (index, source) in paths.into_iter().enumerate() {
            let mut path = source.path;
            path.fill = fill.clone();
            path.stroke = stroke.clone();
            if path.fill.is_none() && path.stroke.is_none() {
                continue;
            }
            let mut n = node(format!("{prefix}:set:{index}"), Some(Primitive::Path(path)));
            n.transform = transform;
            self.mask_sketch_shape(&mut n, s);
            nodes.push(n);
        }
        Ok(nodes)
    }

    // sketch_shapes.go:592–733. Keep the precise row/text construction and
    // replace only its outline, header and separator geometry with rough paths.
    pub(crate) fn build_sketch_structured_shape(
        &mut self,
        s: &Shape,
        outer_fill: Option<Paint>,
        outer_stroke: Option<Stroke>,
    ) -> Result<Vec<Node>> {
        let children = self.build_structured_shape(s, outer_fill, outer_stroke.clone())?;
        let b = SceneBox {
            x: s.pos.x as f64,
            y: s.pos.y as f64,
            width: s.width as f64,
            height: s.height as f64,
        };
        let header_height = if s.type_ == SHAPE_CLASS {
            let rows =
                2 + s.fields.as_ref().map_or(0, Vec::len) + s.methods.as_ref().map_or(0, Vec::len);
            crate::golib::geo::go_max(
                2.0 * (b.height / rows as f64),
                s.text.label_height as f64 + 2.0 * crate::golib::label::PADDING,
            )
        } else {
            b.height / (1 + s.columns.as_ref().map_or(0, Vec::len)) as f64
        };
        let header_box = SceneBox {
            height: header_height,
            ..b
        };
        let object = format!("shape {}", q(&s.id));
        let mut output = Vec::with_capacity(children.len() + 8);
        for child in children {
            if child.id == format!("{}:outline", s.id) {
                let nodes = self.compile_structured_sketch_geometry(
                    s,
                    &format!("{object} structured outline"),
                    &format!("{}:outline:sketch", s.id),
                    b,
                    None,
                    &s.stroke,
                    &s.fill,
                    outer_stroke.as_ref(),
                )?;
                output.extend(self.append_sketch_builtin_pattern(
                    s,
                    nodes,
                    &format!("{}:outline", s.id),
                )?);
            } else if child.id == format!("{}:class-header", s.id)
                || child.id == format!("{}:table-header", s.id)
            {
                self.paint(&s.fill, &format!("{object} structured sketch header"))?;
                let stroke = Stroke {
                    width: 1.0,
                    ..Default::default()
                };
                let nodes = self.compile_structured_sketch_geometry(
                    s,
                    &format!("{object} structured header"),
                    &format!("{}:sketch", child.id),
                    header_box,
                    None,
                    &s.fill,
                    "",
                    Some(&stroke),
                )?;
                output.extend(self.append_sketch_builtin_pattern(s, nodes, &child.id)?);
                if s.type_ == SHAPE_CLASS {
                    output.extend(self.build_sketch_streak_overlays(
                        s,
                        &s.fill,
                        &child.id,
                        vec![Primitive::Rect(Rect {
                            r#box: header_box,
                            ..Default::default()
                        })],
                    )?);
                }
            } else if child.id.ends_with(":separator")
                || child.id == format!("{}:class-separator", s.id)
            {
                let endpoints = match &child.primitive {
                    Some(Primitive::Path(p))
                        if p.commands.len() == 2
                            && p.commands[0].kind == PathCommandKind::MOVE
                            && p.commands[1].kind == PathCommandKind::LINE =>
                    {
                        (p.commands[0].p1, p.commands[1].p1)
                    }
                    _ => {
                        return err(format!(
                            "scene: {object} structured separator {} is not a typed line",
                            q(&child.id)
                        ));
                    }
                };
                let stroke = Stroke {
                    width: if s.type_ == SHAPE_SQL_TABLE { 2.0 } else { 1.0 },
                    ..Default::default()
                };
                let nodes = self.compile_structured_sketch_geometry(
                    s,
                    &format!("{object} structured separator"),
                    &format!("{}:sketch", child.id),
                    b,
                    Some(endpoints),
                    &s.fill,
                    "",
                    Some(&stroke),
                )?;
                output.extend(self.append_sketch_builtin_pattern(s, nodes, &child.id)?);
            } else {
                output.push(child);
            }
        }
        if s.type_ == SHAPE_SQL_TABLE {
            output.extend(self.build_sketch_streak_overlays(
                s,
                &s.fill,
                &format!("{}:table", s.id),
                vec![Primitive::Rect(Rect {
                    r#box: b,
                    ..Default::default()
                })],
            )?);
        }
        Ok(output)
    }

    #[allow(clippy::too_many_arguments)]
    fn compile_structured_sketch_geometry(
        &mut self,
        s: &Shape,
        object: &str,
        prefix: &str,
        b: SceneBox,
        line: Option<(Point, Point)>,
        fill_raw: &str,
        stroke_raw: &str,
        stroke_values: Option<&Stroke>,
    ) -> Result<Vec<Node>> {
        let options = shape_options(stroke_values);
        let (drawable, transform) = if let Some((start, end)) = line {
            (
                generator().line(start.x, start.y, end.x, end.y, Some(&options)),
                Matrix::identity(),
            )
        } else {
            (
                generator().rectangle(0.0, 0.0, b.width, b.height, Some(&options)),
                Matrix::translate(b.x, b.y),
            )
        };
        let paths = self.compile_sketch_drawable(object, &drawable)?;
        let fill = self.paint(none_if_empty(fill_raw), &format!("{object} fill"))?;
        let paint = self.paint(none_if_empty(stroke_raw), &format!("{object} stroke"))?;
        let stroke = stroke_values
            .filter(|s| paint.is_some() && s.width > 0.0)
            .map(|s| Stroke {
                paint,
                width: s.width,
                dashes: s.dashes.clone(),
                dash_offset: s.dash_offset,
                cap: LineCap::ROUND,
                join: LineJoin::ROUND,
                ..Default::default()
            });
        self.paint_sketch_paths(s, prefix, paths, transform, fill, stroke)
    }

    fn build_sketch_outline(
        &mut self,
        s: &Shape,
        prefix: &str,
    ) -> Result<(Vec<Node>, Vec<Primitive>)> {
        if s.type_ == SHAPE_TEXT {
            return Ok((Vec::new(), Vec::new()));
        }
        let prefix = if prefix.is_empty() {
            format!("{}:sketch", s.id)
        } else {
            prefix.to_owned()
        };
        let object = format!("shape {}", q(&s.id));
        let stroke = self.stroke(
            &s.stroke,
            s.stroke_width,
            s.stroke_dash,
            LineCap::ROUND,
            LineJoin::ROUND,
            &format!("{object} sketch stroke"),
        )?;
        let options = shape_options(stroke.as_ref());
        let width = s.width as f64;
        let height = s.height as f64;
        let b = SceneBox {
            x: s.pos.x as f64,
            y: s.pos.y as f64,
            width,
            height,
        };
        let mut nodes;
        let overlays;
        match s.type_.as_str() {
            SHAPE_OVAL | SHAPE_CIRCLE => {
                let drawable =
                    generator().ellipse(width / 2.0, height / 2.0, width, height, Some(&options));
                nodes = self.compile_shape_drawable(
                    s,
                    &object,
                    &prefix,
                    &drawable,
                    Matrix::translate(b.x, b.y),
                )?;
                overlays = vec![Primitive::Ellipse(Ellipse {
                    center: Point::new(b.x + width / 2.0, b.y + height / 2.0),
                    radius_x: width / 2.0,
                    radius_y: height / 2.0,
                    ..Default::default()
                })];
            }
            SHAPE_RECTANGLE | SHAPE_SQUARE | SHAPE_SEQUENCE_DIAGRAM | SHAPE_HIERARCHY | "" => {
                let drawable = generator().rectangle(0.0, 0.0, width, height, Some(&options));
                nodes = self.compile_shape_drawable(
                    s,
                    &object,
                    &prefix,
                    &drawable,
                    Matrix::translate(b.x, b.y),
                )?;
                overlays = vec![Primitive::Rect(Rect {
                    r#box: b,
                    ..Default::default()
                })];
            }
            _ => {
                let paths = target_geometry(s).get_path_commands().unwrap_or_default();
                if paths.is_empty() {
                    return Err(unsupported(
                        &object,
                        &format!("typed sketch geometry for {}", s.type_),
                    ));
                }
                let remaining = self.remaining_sketch_operations();
                let point_limit = (remaining - 32) / 8;
                if remaining <= 0 || point_limit <= 1 {
                    return err(format!(
                        "scene: {object} sketch typed-path expansion exceeds remaining operation budget"
                    ));
                }
                let mut flattened = Vec::new();
                let mut total_points = 0;
                for (index, commands) in paths.iter().enumerate() {
                    let parts =
                        flatten_path(commands, point_limit - total_points).map_err(|e| {
                            Error(format!("scene: {object} typed sketch path {index}: {e}"))
                        })?;
                    for part in parts {
                        total_points += part.points.len() as i64;
                        flattened.push(part);
                    }
                }
                nodes = Vec::new();
                for (index, part) in flattened.iter().enumerate() {
                    if part.points.len() < 2 {
                        return err(format!(
                            "scene: {object} typed sketch path {index} has fewer than two points"
                        ));
                    }
                    let drawable = if part.closed {
                        generator().polygon(&part.points, Some(&options))
                    } else {
                        generator().linear_path(&part.points, Some(&options))
                    };
                    nodes.extend(self.compile_shape_drawable(
                        s,
                        &format!("{object} typed path {index}"),
                        &format!("{prefix}:path:{index}"),
                        &drawable,
                        Matrix::identity(),
                    )?);
                }
                overlays = nodes.iter().filter_map(|n| n.primitive.clone()).collect();
            }
        }
        Ok((nodes, overlays))
    }

    pub(crate) fn build_sketch_ordinary_shape(
        &mut self,
        s: &Shape,
        prefix: &str,
    ) -> Result<Vec<Node>> {
        if s.type_ == SHAPE_TEXT {
            return Ok(Vec::new());
        }
        let prefix = if prefix.is_empty() {
            format!("{}:sketch", s.id)
        } else {
            prefix.to_owned()
        };
        let (mut nodes, overlays) = self.build_sketch_outline(s, &prefix)?;
        nodes = self.append_sketch_builtin_pattern(s, nodes, &prefix)?;
        nodes.extend(self.build_sketch_streak_overlays(s, &s.fill, &prefix, overlays)?);
        Ok(nodes)
    }

    fn append_sketch_builtin_pattern(
        &mut self,
        s: &Shape,
        mut nodes: Vec<Node>,
        prefix: &str,
    ) -> Result<Vec<Node>> {
        let object = format!("shape {}", q(&s.id));
        let pattern = self
            .builtin_pattern(&s.fill_pattern)
            .map_err(|e| Error(format!("scene: {object} fill pattern: {e}")))?;
        if let Some(pattern) = pattern {
            let mut output = Vec::with_capacity(nodes.len() * 2);
            for (index, n) in nodes.into_iter().enumerate() {
                if let Some(Primitive::Path(p)) = &n.primitive {
                    self.charge_sketch_commands(
                        &format!("{object} sketch fill-pattern overlay"),
                        p.commands.len(),
                    )?;
                }
                let overlay = patterns::overlay_pattern_node(
                    &n,
                    &pattern,
                    &format!("{prefix}:fill-pattern:{index}"),
                )?;
                output.push(n);
                output.push(overlay);
            }
            nodes = output;
        }
        Ok(nodes)
    }

    fn build_sketch_streak_overlays(
        &mut self,
        s: &Shape,
        fill: &str,
        prefix: &str,
        mut overlays: Vec<Primitive>,
    ) -> Result<Vec<Node>> {
        let object = format!("shape {}", q(&s.id));
        let mut nodes = Vec::new();
        if !overlays.is_empty() && !fill.is_empty() && !fill.eq_ignore_ascii_case("none") {
            let (pattern, blend, category) = self.sketch_streak_pattern(fill, &object)?;
            for (index, primitive) in overlays.iter_mut().enumerate() {
                let paint = Some(Paint::PatternPaint(pattern.clone()));
                match primitive {
                    Primitive::Rect(p) => {
                        p.fill = paint;
                        p.stroke = None;
                    }
                    Primitive::Ellipse(p) => {
                        p.fill = paint;
                        p.stroke = None;
                    }
                    Primitive::Path(p) => {
                        self.charge_sketch_commands(
                            &format!("{object} sketch streak overlay"),
                            p.commands.len(),
                        )?;
                        p.fill = paint;
                        p.stroke = None;
                    }
                    _ => {
                        return err(format!(
                            "scene: {object} streak overlay {index}: unsupported primitive"
                        ));
                    }
                }
                let mut n = node(format!("{prefix}:streak:{index}"), Some(primitive.clone()));
                n.classes = Some(vec![
                    "sketch-streak-overlay".into(),
                    format!("sketch-streak-{category}"),
                ]);
                n.blend = blend;
                self.mask_sketch_shape(&mut n, s);
                nodes.push(n);
            }
        }
        Ok(nodes)
    }

    /// d2scenebuild/sketch_shapes.go: buildSketchShapeEffects.
    pub(crate) fn build_sketch_shape_effects(
        &mut self,
        s: &Shape,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    ) -> Result<Vec<Node>> {
        if s.three_dee {
            let nodes = self.build_shape_effects(s, fill, stroke)?;
            let pattern = self
                .builtin_pattern(&s.fill_pattern)
                .map_err(|e| Error(format!("scene: shape {} fill pattern: {e}", q(&s.id))))?;
            return patterns::interleave_pattern(nodes, pattern.as_ref(), &s.id, |n| {
                patterns::effect_pattern_node(s, n)
            });
        }
        if s.double_border {
            return self.build_sketch_double_border_shape(s, fill, stroke);
        }
        if s.multiple {
            let mut multiple = s.clone();
            multiple.pos.x += MULTIPLE_OFFSET;
            multiple.pos.y -= MULTIPLE_OFFSET;
            let mut nodes = self.build_ordinary_shape_outline(
                &multiple,
                fill,
                stroke,
                &format!("{}:multiple", s.id),
            )?;
            nodes.extend(self.build_sketch_ordinary_shape(s, &format!("{}:main:sketch", s.id))?);
            return Ok(nodes);
        }
        err(format!(
            "scene: shape {} has no sketch shape effect",
            q(&s.id)
        ))
    }

    fn build_sketch_double_border_shape(
        &mut self,
        s: &Shape,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    ) -> Result<Vec<Node>> {
        let mut nodes = Vec::new();
        if s.multiple {
            let mut multiple = s.clone();
            multiple.pos.x += MULTIPLE_OFFSET;
            multiple.pos.y -= MULTIPLE_OFFSET;
            let mut duplicate = crate::shape_effects::double_border_pair(
                &multiple,
                fill.clone(),
                fill,
                stroke,
                &format!("{}:multiple", s.id),
            );
            if matches!(s.type_.as_str(), "" | SHAPE_RECTANGLE | SHAPE_SQUARE) {
                let pattern = self
                    .builtin_pattern(&s.fill_pattern)
                    .map_err(|e| Error(format!("scene: shape {} fill pattern: {e}", q(&s.id))))?;
                duplicate =
                    patterns::interleave_pattern(duplicate, pattern.as_ref(), &s.id, |n| {
                        n.id == format!("{}:multiple:outer", s.id)
                    })?;
            }
            nodes.extend(duplicate);
        }
        let mut outer = s.clone();
        outer.multiple = false;
        let (outline, _) =
            self.build_sketch_outline(&outer, &format!("{}:double-border:outer:sketch", s.id))?;
        nodes.extend(self.append_sketch_builtin_pattern(
            s,
            outline,
            &format!("{}:double-border:outer", s.id),
        )?);
        let mut inner = outer;
        inner.pos.x += INNER_BORDER_OFFSET;
        inner.pos.y += INNER_BORDER_OFFSET;
        inner.width -= 2 * INNER_BORDER_OFFSET;
        inner.height -= 2 * INNER_BORDER_OFFSET;
        inner.fill = "transparent".into();
        let (outline, _) =
            self.build_sketch_outline(&inner, &format!("{}:double-border:inner:sketch", s.id))?;
        nodes.extend(outline);
        let b = SceneBox {
            x: s.pos.x as f64,
            y: s.pos.y as f64,
            width: s.width as f64,
            height: s.height as f64,
        };
        let primitive = if matches!(s.type_.as_str(), SHAPE_OVAL | SHAPE_CIRCLE) {
            Primitive::Ellipse(Ellipse {
                center: Point::new(b.x + b.width / 2.0, b.y + b.height / 2.0),
                radius_x: b.width / 2.0,
                radius_y: b.height / 2.0,
                ..Default::default()
            })
        } else {
            Primitive::Rect(Rect {
                r#box: b,
                ..Default::default()
            })
        };
        nodes.extend(self.build_sketch_streak_overlays(
            s,
            &s.fill,
            &format!("{}:double-border", s.id),
            vec![primitive],
        )?);
        Ok(nodes)
    }

    fn sketch_streak_pattern(
        &mut self,
        fill: &str,
        object: &str,
    ) -> Result<(PatternPaint, BlendMode, String)> {
        let category = if color::is_gradient(fill) || fill.starts_with("url(#") {
            "normal"
        } else {
            let resolved = self.theme.resolve(fill);
            if resolved.is_empty() {
                return err(format!(
                    "scene: {object} sketch streak fill {}: unknown theme color",
                    q(fill)
                ));
            }
            color::luminance_category(&resolved).map_err(|e| {
                Error(format!(
                    "scene: {object} sketch streak fill {}: {e}",
                    q(fill)
                ))
            })?
        };
        let (blend, ink) = match category {
            "bright" => (BlendMode::DARKEN, Nrgba::new(0, 0, 0, 26)),
            "normal" => (BlendMode::COLOR_BURN, Nrgba::new(0, 0, 0, 41)),
            "dark" => (BlendMode::OVERLAY, Nrgba::new(0, 0, 0, 82)),
            "darker" => (BlendMode::LIGHTEN, Nrgba::new(255, 255, 255, 61)),
            _ => {
                return err(format!(
                    "scene: invalid sketch streak luminance category {}",
                    q(category)
                ));
            }
        };
        if let Some(pattern) = self.sketch_streak_patterns.get(category) {
            return Ok((pattern.clone(), blend, category.into()));
        }
        let commands = crate::pattern_sources::shared_sketch_streak_commands()
            .map_err(|e| Error(format!("scene: load built-in sketch streak: {e}")))?;
        self.charge_sketch_commands(&format!("{object} sketch streak pattern"), commands.len())?;
        let root = streak_root(commands, ink, category)
            .map_err(|e| Error(format!("scene: build built-in sketch streak: {e}")))?;
        let pattern = PatternPaint {
            tile: SceneBox {
                width: 100.0,
                height: 100.0,
                ..Default::default()
            },
            root: Some(std::sync::Arc::new(root)),
            units: PaintUnits::USER_SPACE_ON_USE,
            transform: Matrix::identity(),
        };
        self.sketch_streak_patterns
            .insert(category.into(), pattern.clone());
        Ok((pattern, blend, category.into()))
    }
}

/// Keep the sole nested compound (subpaths 16 and 17) in one even-odd path.
fn streak_root(commands: &[PathCommand], ink: Nrgba, category: &str) -> Result<Node> {
    let mut starts: Vec<usize> = commands
        .iter()
        .enumerate()
        .filter_map(|(i, c)| (c.kind == PathCommandKind::MOVE).then_some(i))
        .collect();
    if starts.len() != 85 || starts.first() != Some(&0) {
        return err(format!(
            "unexpected subpath topology: got {} subpaths, want 85",
            starts.len()
        ));
    }
    starts.push(commands.len());
    for subpath in 0..85 {
        if starts[subpath + 1] <= starts[subpath] + 1
            || commands[starts[subpath + 1] - 1].kind != PathCommandKind::CLOSE
        {
            return err(format!("unexpected open or empty subpath {subpath}"));
        }
    }
    let mut root = node(format!("builtin:sketch-streak:{category}"), None);
    let mut children = Vec::with_capacity(84);
    let mut subpath = 0;
    while subpath < 85 {
        let end = subpath + if subpath == 16 { 2 } else { 1 };
        let p = Path {
            commands: commands[starts[subpath]..starts[end]].to_vec(),
            fill_rule: FillRule::EVEN_ODD,
            fill: Some(Paint::solid(ink)),
            ..Default::default()
        };
        children.push(node(
            format!("{}:subpath:{subpath}", root.id),
            Some(Primitive::Path(p)),
        ));
        subpath = end;
    }
    root.children = Some(children);
    Ok(root)
}

#[derive(Default)]
struct FlatPath {
    points: Vec<[f64; 2]>,
    closed: bool,
}

fn append_point(part: &mut FlatPath, point: [f64; 2], count: &mut i64, limit: i64) -> Result<()> {
    if !point.iter().all(|v| v.is_finite()) {
        return err("non-finite typed point");
    }
    if part.points.last() == Some(&point) {
        return Ok(());
    }
    if *count >= limit {
        return err(format!("flattened point count exceeds limit {limit}"));
    }
    part.points.push(point);
    *count += 1;
    Ok(())
}

fn flatten_path(commands: &[svgpath::PathCommand], limit: i64) -> Result<Vec<FlatPath>> {
    if limit <= 1 {
        return err("flattened point limit must exceed one");
    }
    let mut output = Vec::new();
    let mut part = FlatPath::default();
    let mut current = [0.0; 2];
    let mut start = current;
    let mut have_current = false;
    let mut count = 0;
    for (index, c) in commands.iter().enumerate() {
        let result = match c.kind {
            svgpath::PathCommandKind::Move => {
                if !part.points.is_empty() {
                    output.push(std::mem::take(&mut part));
                }
                start = [c.end.x, c.end.y];
                current = start;
                have_current = true;
                append_point(&mut part, start, &mut count, limit)
            }
            svgpath::PathCommandKind::Line | svgpath::PathCommandKind::Cubic if !have_current => {
                let verb = if c.kind == svgpath::PathCommandKind::Line {
                    "line"
                } else {
                    "cubic"
                };
                err(format!("{verb} has no current point"))
            }
            svgpath::PathCommandKind::Line => {
                current = [c.end.x, c.end.y];
                append_point(&mut part, current, &mut count, limit)
            }
            svgpath::PathCommandKind::Cubic => {
                let mut points = Vec::new();
                flatten_cubic(
                    current,
                    [c.control1.x, c.control1.y],
                    [c.control2.x, c.control2.y],
                    [c.end.x, c.end.y],
                    0,
                    limit - count,
                    &mut points,
                )
                .map_err(|e| Error(format!("command {index}: {e}")))?;
                for p in points {
                    append_point(&mut part, p, &mut count, limit)
                        .map_err(|e| Error(format!("command {index}: {e}")))?;
                }
                current = [c.end.x, c.end.y];
                Ok(())
            }
            svgpath::PathCommandKind::Close => {
                if !have_current {
                    err("close has no current point")
                } else {
                    part.closed = true;
                    current = start;
                    Ok(())
                }
            }
        };
        result.map_err(|e| Error(format!("command {index}: {e}")))?;
    }
    if !part.points.is_empty() {
        output.push(part);
    }
    if output.is_empty() {
        return err("typed path is empty");
    }
    Ok(output)
}

fn distance_squared(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    use crate::golib::geo::{go_fmadd, go_fmsub};
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let denominator = go_fmadd(dx, dx, dy * dy);
    if denominator == 0.0 {
        let x = p[0] - a[0];
        let y = p[1] - a[1];
        return go_fmadd(x, x, y * y);
    }
    let numerator = go_fmsub(dx, p[1], dy * p[0]);
    let numerator = go_fmadd(b[0], a[1], numerator);
    let numerator = go_fmsub(b[1], a[0], numerator);
    numerator * numerator / denominator
}

#[allow(clippy::too_many_arguments)]
fn flatten_cubic(
    a: [f64; 2],
    b: [f64; 2],
    c: [f64; 2],
    d: [f64; 2],
    depth: usize,
    limit: i64,
    points: &mut Vec<[f64; 2]>,
) -> Result<()> {
    if limit <= 0 {
        return err("flattened point count exceeds limit");
    }
    if depth >= 8
        || (distance_squared(b, a, d) <= 0.35 * 0.35 && distance_squared(c, a, d) <= 0.35 * 0.35)
    {
        if points.len() as i64 >= limit {
            return err("flattened point count exceeds limit");
        }
        points.push(d);
        return Ok(());
    }
    let mid = |a: [f64; 2], b: [f64; 2]| [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let ab = mid(a, b);
    let bc = mid(b, c);
    let cd = mid(c, d);
    let abc = mid(ab, bc);
    let bcd = mid(bc, cd);
    let center = mid(abc, bcd);
    flatten_cubic(a, ab, abc, center, depth + 1, limit, points)?;
    flatten_cubic(center, bcd, cd, d, depth + 1, limit, points)
}

// d2scenebuild/sketch_connection.go and d2sketch/scene_connection.go.
impl Builder<'_> {
    pub(crate) fn sketch_connection_path(
        &mut self,
        c: &d2_rs_target::Connection,
        mut source: Path,
    ) -> Result<Path> {
        let object = format!("connection {} route", q(&c.id));
        let drawable = connection_drawable(
            &source,
            self.options.sketch_budget.max_operation_sets - self.sketch_operation_sets,
            self.remaining_sketch_operations(),
        )
        .map_err(|e| Error(format!("scene: {object} sketch geometry: {e}")))?;
        let paths = self.compile_sketch_drawable(&object, &drawable)?;
        source.commands = paths.into_iter().flat_map(|p| p.path.commands).collect();
        if source.commands.is_empty() {
            return err(format!("scene: {object} sketch geometry is empty"));
        }
        Ok(source)
    }

    fn sketch_scene_path(&mut self, object: &str, source: ScenePath) -> Result<Path> {
        let mut path = source.path;
        path.fill = self.paint(&source.fill, &format!("{object} fill"))?;
        let paint = self.paint(&source.stroke, &format!("{object} stroke"))?;
        if paint.is_some() && source.stroke_width > 0.0 {
            path.stroke = Some(Stroke {
                paint,
                width: source.stroke_width,
                cap: LineCap::ROUND,
                join: LineJoin::ROUND,
                dashes: opt_vec(source.dash),
                dash_offset: source.dash_offset,
                ..Default::default()
            });
        }
        Ok(path)
    }

    pub(crate) fn sketch_arrowhead(
        &mut self,
        c: &d2_rs_target::Connection,
        arrowhead: &str,
        target: bool,
        endpoint: Point,
        tangent: crate::golib::geo::Vector,
    ) -> Result<Node> {
        let endpoint_name = if target { "dst" } else { "src" };
        let arrow_id = format!("{}:{endpoint_name}-arrowhead", c.id);
        let drawables =
            arrowhead_drawables(arrowhead, &c.stroke, c.stroke_width).ok_or_else(|| {
                unsupported(
                    &format!("connection {}", q(&c.id)),
                    &format!("sketch arrowhead {arrowhead}"),
                )
            })?;
        let mut n = node(arrow_id.clone(), None);
        let mut children = Vec::new();
        let mut path_index = 0;
        for (index, drawable) in drawables.iter().enumerate() {
            let object = format!(
                "connection {} {endpoint_name} arrowhead drawable {index}",
                q(&c.id)
            );
            for source in self.compile_sketch_drawable(&object, drawable)? {
                let path = self.sketch_scene_path(&object, source)?;
                if path_index == 0 {
                    n.primitive = Some(Primitive::Path(path));
                } else {
                    children.push(node(
                        format!("{arrow_id}:rough:{path_index}"),
                        Some(Primitive::Path(path)),
                    ));
                }
                path_index += 1;
            }
        }
        if path_index == 0 {
            return err(format!(
                "scene: connection {} {endpoint_name} sketch arrowhead is empty",
                q(&c.id)
            ));
        }
        let mut angle = crate::golib::geo::gomath::atan2(tangent.0[1], tangent.0[0]);
        if !target {
            angle += std::f64::consts::PI;
        }
        n.transform = Matrix::translate(endpoint.x, endpoint.y).mul(Matrix::rotate(angle));
        n.children = opt_vec(children);
        Ok(n)
    }
}

fn connection_drawable(path: &Path, max_sets: i64, max_ops: i64) -> Result<Drawable> {
    if max_sets <= 0 || max_ops <= 0 {
        return err("d2sketch: connection scene limits must be positive");
    }
    let mut segments = 0i64;
    let mut have_move = false;
    for (index, command) in path.commands.iter().enumerate() {
        match command.kind {
            PathCommandKind::MOVE => {
                if have_move {
                    return err("d2sketch: connection path has multiple subpaths");
                }
                have_move = true;
            }
            PathCommandKind::LINE | PathCommandKind::CUBIC => {
                if !have_move {
                    return err(format!(
                        "d2sketch: connection path command {index} is before its move"
                    ));
                }
                segments += if command.kind == PathCommandKind::LINE {
                    1
                } else {
                    8
                };
            }
            other => {
                return err(format!(
                    "d2sketch: connection path command {index} has unsupported kind {}",
                    other.0
                ));
            }
        }
        if segments > max_ops / 4 {
            return err(format!(
                "d2sketch: connection input expands beyond operation limit {max_ops}"
            ));
        }
    }
    if !have_move || segments == 0 {
        return err("d2sketch: connection path has no painted segments");
    }
    if (segments + 255) / 256 > max_sets {
        return err(format!(
            "d2sketch: connection input expands beyond operation set limit {max_sets}"
        ));
    }
    let mut points = Vec::with_capacity(segments as usize + 1);
    let mut current = Point::default();
    for command in &path.commands {
        match command.kind {
            PathCommandKind::MOVE | PathCommandKind::LINE => {
                current = command.p1;
                points.push([current.x, current.y]);
            }
            PathCommandKind::CUBIC => {
                let lerp = |a: Point, b: Point, t: f64| {
                    Point::new(
                        crate::golib::geo::go_fmadd(b.x - a.x, t, a.x),
                        crate::golib::geo::go_fmadd(b.y - a.y, t, a.y),
                    )
                };
                for step in 1..=8 {
                    let t = step as f64 / 8.0;
                    let a = lerp(current, command.p1, t);
                    let b = lerp(command.p1, command.p2, t);
                    let c = lerp(command.p2, command.p3, t);
                    let p = lerp(lerp(a, b, t), lerp(b, c, t), t);
                    points.push([p.x, p.y]);
                }
                current = command.p3;
            }
            _ => unreachable!("commands validated before allocation"),
        }
    }
    let mut g = generator();
    let mut combined: Option<Drawable> = None;
    for (index, first) in (0..segments as usize).step_by(256).enumerate() {
        let last = (first + 256).min(segments as usize);
        let drawable = g.linear_path(
            &points[first..=last],
            Some(&Options {
                roughness: Some(0.5),
                seed: Some(1.0 + index as f64),
                ..Default::default()
            }),
        );
        if let Some(combined) = &mut combined {
            combined.sets.extend(drawable.sets);
        } else {
            combined = Some(Drawable {
                shape: "connection",
                ..drawable
            });
        }
    }
    Ok(combined.expect("at least one painted segment"))
}

fn arrow_options(stroke: &str, width: i64, seed: f64) -> Options {
    Options {
        stroke: Some(stroke.into()),
        stroke_width: Some(width as f64),
        seed: Some(seed),
        ..Default::default()
    }
}

fn solid_arrow_options(stroke: &str, fill: &str, width: i64, weight: f64, seed: f64) -> Options {
    Options {
        fill: Some(fill.into()),
        fill_style: Some("solid".into()),
        fill_weight: (weight >= 0.0).then_some(weight),
        ..arrow_options(stroke, width, seed)
    }
}

fn arrowhead_drawables(arrowhead: &str, stroke: &str, width: i64) -> Option<Vec<Drawable>> {
    let mut g = generator();
    let diamond = [
        [-20.0, 0.0],
        [-10.0, 5.0],
        [0.0, 0.0],
        [-10.0, -5.0],
        [-20.0, 0.0],
    ];
    let triangle = [[-10.0, -4.0], [0.0, 0.0], [-10.0, 4.0]];
    let box_points = [[0.0, -10.0], [0.0, 10.0], [-20.0, 10.0], [-20.0, -10.0]];
    Some(match arrowhead {
        "arrow" | "line" => {
            vec![g.linear_path(&triangle, Some(&arrow_options(stroke, width, 3.0)))]
        }
        "triangle" | "unfilled-triangle" => vec![g.polygon(
            &triangle,
            Some(&solid_arrow_options(
                stroke,
                if arrowhead == "triangle" {
                    stroke
                } else {
                    BG_COLOR
                },
                width,
                -1.0,
                2.0,
            )),
        )],
        "diamond" => vec![g.polygon(
            &diamond,
            Some(&solid_arrow_options(stroke, BG_COLOR, width, -1.0, 1.0)),
        )],
        "filled-diamond" => vec![g.polygon(
            &diamond,
            Some(&Options {
                fill_style: Some("zigzag".into()),
                ..solid_arrow_options(stroke, stroke, width, 4.0, 1.0)
            }),
        )],
        "circle" | "filled-circle" => vec![g.circle(
            -2.0,
            -1.0,
            8.0,
            Some(&solid_arrow_options(
                stroke,
                if arrowhead == "circle" {
                    BG_COLOR
                } else {
                    stroke
                },
                width,
                1.0,
                5.0,
            )),
        )],
        "cross" => vec![g.linear_path(
            &[
                [-6.0, -6.0],
                [6.0, 6.0],
                [0.0, 0.0],
                [-6.0, 6.0],
                [0.0, 0.0],
                [6.0, -6.0],
            ],
            Some(&arrow_options(stroke, width, 3.0)),
        )],
        "box" | "filled-box" => vec![g.polygon(
            &box_points,
            Some(&solid_arrow_options(
                stroke,
                if arrowhead == "box" { BG_COLOR } else { stroke },
                width,
                -1.0,
                1.0,
            )),
        )],
        "cf-many-required" => vec![
            g.line(
                -15.0,
                -10.0,
                -15.0,
                10.0,
                Some(&arrow_options(stroke, width, 2.0)),
            ),
            g.linear_path(
                &[[0.0, 10.0], [-15.0, 0.0], [0.0, -10.0]],
                Some(&arrow_options(stroke, width, 2.0)),
            ),
        ],
        "cf-many" => vec![
            g.linear_path(
                &[[0.0, 10.0], [-15.0, 0.0], [0.0, -10.0]],
                Some(&arrow_options(stroke, width, 8.0)),
            ),
            g.circle(
                -20.0,
                0.0,
                8.0,
                Some(&solid_arrow_options(stroke, BG_COLOR, width, 1.0, 4.0)),
            ),
        ],
        "cf-one-required" => vec![
            g.line(
                -15.0,
                -10.0,
                -15.0,
                10.0,
                Some(&arrow_options(stroke, width, 2.0)),
            ),
            g.line(
                -10.0,
                -10.0,
                -10.0,
                10.0,
                Some(&arrow_options(stroke, width, 2.0)),
            ),
        ],
        "cf-one" => vec![
            g.line(
                -10.0,
                -10.0,
                -10.0,
                10.0,
                Some(&arrow_options(stroke, width, 3.0)),
            ),
            g.circle(
                -20.0,
                0.0,
                8.0,
                Some(&solid_arrow_options(stroke, BG_COLOR, width, 1.0, 5.0)),
            ),
        ],
        _ => return None,
    })
}
