// SPDX-License-Identifier: MPL-2.0
//! appendix.go: static badges and appendix rows for link/tooltip metadata.
//! ARM64 appendix.go:420/421: the row baselines fuse `index*lineHeight`.

use d2_rs_scene::{
    Bounds, DropShadow, Ellipse, Filter, LineCap, LineJoin, Node, Nrgba, Path, PathCommand, Point,
    Primitive, Stroke, TextAnchor, TextRun,
};
use d2_rs_target::{Diagram, Point as TargetPoint, Shape, Text};

use crate::golib::geo::{self, go_fmadd};
use crate::golib::markdown::ruler::Ruler;
use crate::golib::shape::{self as libshape, Shape as _};
use crate::legend::{legend_rendered_text, text_dimensions};
use crate::util::*;
use crate::{AppendixItem, Builder, target_geometry};

const PAD_TOP: i64 = 50;
const PAD_SIDES: i64 = 40;
const SPACER: i64 = 20;
const FONT_SIZE: i64 = 16;
const ICON_RADIUS: i64 = 16;

struct AppendixRow {
    number: usize,
    text: String,
    y: i64,
    width: i64,
    height: i64,
    line_height: f64,
}

pub(crate) struct AppendixLayout {
    top_left: TargetPoint,
    bottom_right: TargetPoint,
    max_width: i64,
    height: i64,
    rows: Vec<AppendixRow>,
}

fn max_items(budget_regions: i64) -> i64 {
    if budget_regions <= i64::MAX / 2 {
        budget_regions * 2
    } else {
        i64::MAX
    }
}

fn checked_appendix(value: i64, delta: i64) -> Option<i64> {
    checked_add(value, delta, MIN_INT, MAX_INT)
}

impl Builder<'_> {
    /// addAppendixItem.
    pub(crate) fn add_appendix_item(
        &mut self,
        object: &str,
        field: &str,
        text: &str,
    ) -> Result<()> {
        if !self.options.appendix || text.is_empty() {
            return Ok(());
        }
        let budget = self.options.link_budget;
        if budget.max_regions <= 0 || budget.max_string_bytes <= 0 {
            return Ok(());
        }
        let max = max_items(budget.max_regions);
        if self.appendix_items.len() as i64 >= max {
            return err(format!("scene: appendix item count exceeds limit {max}"));
        }
        crate::metadata::validate_link_string(object, field, text)?;
        if text.len() as i64 > budget.max_string_bytes - self.appendix_string_bytes {
            return err(format!(
                "scene: appendix string bytes exceed limit {}",
                budget.max_string_bytes
            ));
        }
        self.appendix_items.push(AppendixItem {
            object: object.to_owned(),
            field: field.to_owned(),
            text: text.to_owned(),
        });
        self.appendix_string_bytes += text.len() as i64;
        Ok(())
    }

    /// preflightAppendix.
    pub(crate) fn preflight_appendix(&self) -> Result<()> {
        if !self.options.appendix {
            return Ok(());
        }
        let budget = self.options.link_budget;
        if budget.max_regions <= 0 || budget.max_string_bytes <= 0 {
            return Ok(());
        }
        let max = max_items(budget.max_regions);
        let (mut items, mut string_bytes) = (0i64, 0i64);
        for (index, s) in self.diagram.shapes().iter().enumerate() {
            for (field, value, paint) in [
                ("tooltip", &s.tooltip, !s.tooltip.is_empty()),
                ("prettyLink", &s.pretty_link, !s.link.is_empty()),
            ] {
                if paint {
                    items += 1;
                    if items > max {
                        return err(format!("scene: appendix item count exceeds limit {max}"));
                    }
                }
                if value.is_empty() {
                    continue;
                }
                let object = format!("shape[{index}] {}", q(&s.id));
                crate::metadata::validate_link_string(&object, field, value)?;
                if value.len() as i64 > budget.max_string_bytes - string_bytes {
                    return err(format!(
                        "scene: appendix string bytes exceed limit {}",
                        budget.max_string_bytes
                    ));
                }
                string_bytes += value.len() as i64;
            }
        }
        Ok(())
    }

    pub(crate) fn measure_appendix(
        &self,
        top_left: TargetPoint,
        bottom_right: TargetPoint,
    ) -> Result<Option<AppendixLayout>> {
        if !self.options.appendix || self.appendix_items.is_empty() {
            return Ok(None);
        }
        let mut ruler = Ruler::new();
        let mut layout = AppendixLayout {
            top_left,
            bottom_right,
            max_width: 0,
            height: 0,
            rows: Vec::new(),
        };
        let mut total_height = 0i64;
        for (index, item) in self.appendix_items.iter().enumerate() {
            let invalid = || {
                Error(format!(
                    "scene: {} appendix {} has invalid measured text dimensions",
                    item.object, item.field
                ))
            };
            let (width, height) =
                text_dimensions(&mut ruler, &item.text, FONT_SIZE, false, false, None)
                    .map_err(|_| invalid())?;
            if width < 0 || height < 0 {
                return Err(invalid());
            }
            let row_height = height.max(ICON_RADIUS * 2);
            let row_offset = checked_appendix(total_height, PAD_TOP * 2).ok_or_else(|| {
                Error("scene: appendix row offset exceeds the platform integer domain".into())
            })?;
            let y = checked_appendix(bottom_right.y, row_offset).ok_or_else(|| {
                Error("scene: appendix row y position exceeds the platform integer domain".into())
            })?;
            checked_appendix(y, 5).ok_or_else(|| {
                Error(
                    "scene: appendix row text baseline exceeds the platform integer domain".into(),
                )
            })?;
            let row_width = checked_appendix(width, ICON_RADIUS * 3).ok_or_else(|| {
                Error("scene: appendix row width exceeds the platform integer domain".into())
            })?;
            let lines = (item.text.matches('\n').count() + 1).max(1);
            layout.rows.push(AppendixRow {
                number: index + 1,
                text: item.text.clone(),
                y,
                width: row_width,
                height: row_height,
                line_height: height as f64 / lines as f64,
            });
            layout.max_width = layout.max_width.max(row_width);
            let increment = checked_appendix(row_height, SPACER).ok_or_else(|| {
                Error("scene: appendix row height exceeds the platform integer domain".into())
            })?;
            total_height = checked_appendix(total_height, increment).ok_or_else(|| {
                Error("scene: appendix total height exceeds the platform integer domain".into())
            })?;
        }
        if layout.rows.is_empty() {
            return Ok(None);
        }
        layout.height = checked_appendix(total_height, SPACER).ok_or_else(|| {
            Error("scene: appendix total height exceeds the platform integer domain".into())
        })?;
        Ok(Some(layout))
    }

    pub(crate) fn build_appendix_icons(
        &mut self,
        layout: Option<&AppendixLayout>,
        s: &Shape,
        shape_index: usize,
        tooltip_number: usize,
        link_number: usize,
    ) -> Result<Vec<Node>> {
        if layout.is_none() || s.tooltip.is_empty() && s.link.is_empty() {
            return Ok(Vec::new());
        }
        let (tooltip_center, link_center) = appendix_icon_centers(s).map_err(|e| {
            Error(format!(
                "scene: shape[{shape_index}] {} appendix icons: {e}",
                q(&s.id)
            ))
        })?;
        let mut nodes = Vec::with_capacity(2);
        if !s.tooltip.is_empty() && s.tooltip_position.is_empty() {
            nodes.push(self.build_numbered_appendix_icon(
                &format!("appendix:shape:{shape_index}:tooltip"),
                tooltip_number,
                tooltip_center,
            )?);
        }
        if !s.link.is_empty() {
            nodes.push(self.build_numbered_appendix_icon(
                &format!("appendix:shape:{shape_index}:link"),
                link_number,
                link_center,
            )?);
        }
        Ok(nodes)
    }

    fn build_numbered_appendix_icon(
        &mut self,
        id: &str,
        number: usize,
        center: Point,
    ) -> Result<Node> {
        let white = self.paint("#ffffff", &format!("{id} fill"))?;
        let border_paint = self.paint("#DEE1EB", &format!("{id} border"))?;
        let text_paint = self.paint("#000000", &format!("{id} number"))?;
        let font = self
            .font(&Text {
                font_size: FONT_SIZE,
                bold: true,
                ..Default::default()
            })
            .map_err(|e| Error(format!("scene: {id}: {e}")))?;
        let mut group = node(id, None);
        group.classes = Some(vec!["appendix-icon".into()]);
        group.filters = Some(vec![Filter::DropShadow(DropShadow {
            offset_x: 0.0,
            offset_y: 0.0,
            sigma_x: 32.0,
            sigma_y: 32.0,
            color: Nrgba::new(31, 36, 58, 26),
        })]);
        let r = ICON_RADIUS as f64;
        let circle = node(
            format!("{id}:circle"),
            Some(Primitive::Ellipse(Ellipse {
                center,
                radius_x: r,
                radius_y: r,
                fill: white,
                stroke: Some(Stroke {
                    paint: border_paint,
                    width: 1.0,
                    cap: LineCap::BUTT,
                    join: LineJoin::MITER,
                    miter_limit: 4.0,
                    ..Default::default()
                }),
            })),
        );
        let text = node(
            format!("{id}:number"),
            Some(Primitive::TextRun(TextRun {
                text: number.to_string(),
                origin: Point::new(center.x, center.y + 5.0),
                anchor: TextAnchor::MIDDLE,
                font,
                fill: text_paint,
                ink: Bounds::new(center.x - r, center.y - r, center.x + r, center.y + r),
                ..Default::default()
            })),
        );
        group.children = Some(vec![circle, text]);
        Ok(group)
    }

    pub(crate) fn build_appendix(
        &mut self,
        layout: Option<&AppendixLayout>,
    ) -> Result<Option<Node>> {
        let Some(layout) = layout else {
            return Ok(None);
        };
        let separator_paint = self.paint("B2", "appendix separator")?;
        let text_paint = self.paint("N1", "appendix text")?;
        let regular = self
            .font(&Text {
                font_size: FONT_SIZE,
                ..Default::default()
            })
            .map_err(|e| Error(format!("scene: appendix text: {e}")))?;
        let mut root = node("appendix", None);
        root.classes = Some(vec!["appendix".into()]);
        let separator_y = checked_appendix(layout.bottom_right.y, PAD_TOP).ok_or_else(|| {
            Error("scene: appendix separator y exceeds the platform integer domain".into())
        })?;
        let separator_left = checked_appendix(layout.top_left.x, -PAD_SIDES).ok_or_else(|| {
            Error("scene: appendix separator left edge exceeds the platform integer domain".into())
        })?;
        let separator_right = checked_appendix(
            layout.max_width.max(layout.bottom_right.x),
            PAD_SIDES,
        )
        .ok_or_else(|| {
            Error("scene: appendix separator right edge exceeds the platform integer domain".into())
        })?;
        let mut children = vec![node(
            "appendix:separator",
            Some(Primitive::Path(Path {
                commands: vec![
                    PathCommand::move_to(separator_left as f64, separator_y as f64),
                    PathCommand::line_to(separator_right as f64, separator_y as f64),
                ],
                stroke: Some(Stroke {
                    paint: separator_paint,
                    width: 1.0,
                    cap: LineCap::BUTT,
                    join: LineJoin::MITER,
                    miter_limit: 4.0,
                    ..Default::default()
                }),
                ..Default::default()
            })),
        )];
        for row in &layout.rows {
            let row_id = format!("appendix:row:{}", row.number);
            let mut row_node = node(row_id.clone(), None);
            let mut row_children = vec![self.build_numbered_appendix_icon(
                &format!("{row_id}:icon"),
                row.number,
                Point::new(ICON_RADIUS as f64, row.y as f64),
            )?];
            let lines: Vec<&str> = row.text.split('\n').collect();
            for (line_index, line) in lines.iter().enumerate() {
                let mut rendered = legend_rendered_text(line);
                if rendered.is_empty() && lines.len() > 1 {
                    rendered = " ".into();
                }
                let baseline = go_fmadd(line_index as f64, row.line_height, row.y as f64 + 5.0);
                let line_top = go_fmadd(
                    line_index as f64,
                    row.line_height,
                    row.y as f64 + 5.0 - row.height as f64,
                );
                let line_bottom = line_top + geo::go_max(1.0, row.line_height);
                row_children.push(node(
                    format!("{row_id}:text:{line_index}"),
                    Some(Primitive::TextRun(TextRun {
                        text: rendered,
                        origin: Point::new((ICON_RADIUS * 3) as f64, baseline),
                        anchor: TextAnchor::START,
                        font: regular.clone(),
                        fill: text_paint.clone(),
                        ink: Bounds::new(
                            (ICON_RADIUS * 3) as f64,
                            line_top,
                            (ICON_RADIUS * 3 + 1).max(row.width) as f64,
                            line_bottom,
                        ),
                        ..Default::default()
                    })),
                ));
            }
            row_node.children = Some(row_children);
            children.push(row_node);
        }
        root.children = Some(children);
        Ok(Some(root))
    }
}

/// expandViewBox (appendix.Append's post-serialization arithmetic).
pub(crate) fn expand_view_box(
    layout: Option<&AppendixLayout>,
    left: i64,
    top: i64,
    width: i64,
    height: i64,
) -> Result<(i64, i64, i64, i64)> {
    let Some(layout) = layout else {
        return Ok((left, top, width, height));
    };
    let required_width = checked_sub(layout.max_width, left, MIN_INT, MAX_INT)
        .and_then(|w| checked_add(w, PAD_SIDES * 2, MIN_INT, MAX_INT))
        .ok_or_else(|| {
            Error("scene: appendix width expansion exceeds the platform integer domain".into())
        })?;
    let width64 = width.max(required_width);
    let height64 = checked_add(layout.height, PAD_TOP, MIN_INT, MAX_INT)
        .and_then(|h| checked_add(height, h, MIN_INT, MAX_INT));
    match height64 {
        Some(height64) if width64 >= 0 && height64 >= 0 => Ok((left, top, width64, height64)),
        _ => err("scene: appendix height expansion exceeds the platform integer domain"),
    }
}

/// appendixIconCenters.
fn appendix_icon_centers(s: &Shape) -> std::result::Result<(Point, Point), String> {
    let geometry = target_geometry(s);
    let both_icons = !s.tooltip.is_empty() && !s.link.is_empty();
    let mut corner = geo::Point::new((s.pos.x + s.width) as f64, s.pos.y as f64);
    let mut center = geo::Point::new(
        s.pos.x as f64 + s.width as f64 / 2.0,
        s.pos.y as f64 + s.height as f64 / 2.0,
    );
    let offset = geo::Vector::new(&[-2.0 * ICON_RADIUS as f64, 0.0]);
    let mut left_on_shape = false;
    match geometry.get_type() {
        libshape::STEP_TYPE
        | libshape::HEXAGON_TYPE
        | libshape::QUEUE_TYPE
        | libshape::PAGE_TYPE => center.y = s.pos.y as f64,
        libshape::PACKAGE_TYPE => center.x = (s.pos.x + s.width) as f64,
        libshape::CIRCLE_TYPE
        | libshape::OVAL_TYPE
        | libshape::DIAMOND_TYPE
        | libshape::PERSON_TYPE
        | libshape::CLOUD_TYPE
        | libshape::CYLINDER_TYPE
            if both_icons =>
        {
            left_on_shape = true;
            corner = corner.add_vector(&offset);
        }
        _ => {}
    }
    let v1 = center.vector_to(&corner);
    let p1 = libshape::trace_to_shape_border(&geometry, corner, corner.add_vector(&v1));
    if !finite(p1.x) || !finite(p1.y) {
        return Err("shape border trace did not resolve to a finite point".into());
    }
    let mut p1 = p1;
    let mut p2 = p1;
    if both_icons {
        if left_on_shape {
            p2 = p1.add_vector(&offset.reverse());
            std::mem::swap(&mut p1, &mut p2);
        } else {
            p2 = p1.add_vector(&offset);
        }
    }
    Ok((
        Point::new(p1.x.ceil(), p1.y.ceil()),
        Point::new(p2.x.ceil(), p2.y.ceil()),
    ))
}

/// appendixIconNumbers.
pub(crate) fn appendix_icon_numbers(diagram: &Diagram) -> (Vec<usize>, Vec<usize>) {
    let shapes = diagram.shapes();
    let (mut tooltips, mut links) = (vec![0; shapes.len()], vec![0; shapes.len()]);
    let mut number = 1;
    for (index, s) in shapes.iter().enumerate() {
        if !s.tooltip.is_empty() {
            tooltips[index] = number;
            number += 1;
        }
        if !s.link.is_empty() {
            links[index] = number;
            number += 1;
        }
    }
    (tooltips, links)
}
