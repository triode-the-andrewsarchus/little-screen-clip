// SPDX-License-Identifier: MPL-2.0
//! tooltip.go: positioned tooltips painted above every diagram object.

use d2_rs_scene::{
    Box as SceneBox, LineCap, LineJoin, Node, Paint, Path, PathCommand, Primitive, Rect, Stroke,
};
use d2_rs_target::Shape;

use crate::Builder;
use crate::golib::geo;
use crate::markdown::MAX_MARKDOWN_PRIMITIVES;
use crate::target_ext::calculate_tooltip_position;
use crate::util::*;

const PADDING: f64 = 10.0;
const RADIUS: f64 = 4.0;
const TAIL_SIZE: f64 = 8.0;

impl Builder<'_> {
    pub(crate) fn build_positioned_tooltip(&mut self, s: &Shape) -> Result<Option<Node>> {
        if s.tooltip.is_empty() || s.tooltip_position.is_empty() {
            return Ok(None);
        }
        let object = format!("shape {} positioned tooltip", q(&s.id));
        let (primary, mono) = self.markdown_font_families_for(&s.text.font_family);
        let layout =
            crate::markdown::layout(self.markdown_ruler(), &s.tooltip, &primary, &mono, 16)
                .map_err(|e| Error(format!("scene: {object} Markdown layout: {e}")))?;
        if layout.primitives.len() > MAX_MARKDOWN_PRIMITIVES {
            return err(format!(
                "scene: {object} Markdown primitive count {} exceeds limit {MAX_MARKDOWN_PRIMITIVES}",
                layout.primitives.len()
            ));
        }
        let tooltip_width = layout.width + 2 * PADDING as i64;
        let tooltip_height = layout.height + 2 * PADDING as i64;
        let (x, y) = calculate_tooltip_position(
            s.pos.x as f64,
            s.pos.y as f64,
            s.width as f64,
            s.height as f64,
            tooltip_width,
            tooltip_height,
            &s.tooltip_position,
        );
        if !finite(x) || !finite(y) || tooltip_width <= 0 || tooltip_height <= 0 {
            return Err(invalid_field(
                &object,
                "bounds",
                None,
                "must resolve to finite positive dimensions",
            ));
        }
        let fill = self.paint("N7", &format!("{object} background"))?;
        let stroke_paint = self.paint("N5", &format!("{object} border"))?;
        let stroke = Stroke {
            paint: stroke_paint,
            width: 1.0,
            cap: LineCap::BUTT,
            join: LineJoin::MITER,
            miter_limit: 4.0,
            ..Default::default()
        };
        let mut root = node(format!("{}:positioned-tooltip", s.id), None);
        let b = SceneBox {
            x,
            y,
            width: tooltip_width as f64,
            height: tooltip_height as f64,
        };
        let background = node(
            format!("{}:background", root.id),
            Some(Primitive::Rect(Rect {
                r#box: b,
                radius_x: RADIUS,
                radius_y: RADIUS,
                fill: fill.clone(),
                stroke: Some(stroke.clone()),
            })),
        );
        let mut tail = positioned_tooltip_tail(&s.tooltip_position, b, fill, stroke)
            .map_err(|e| Error(format!("scene: {object} tail: {e}")))?;
        tail.id = format!("{}:tail", root.id);
        let content = self.build_markdown_label(
            &object,
            &format!("{}:markdown", root.id),
            &s.tooltip,
            &s.text.font_family,
            16,
            layout.width,
            layout.height,
            "N1",
            "",
            false,
            false,
            geo::Point::new(x + PADDING, y + PADDING),
        )?;
        root.children = Some(vec![background, tail, content]);
        Ok(Some(root))
    }
}

/// positionedTooltipTail. The ARM64 contractions at tooltip.go:96–107 fuse
/// `Width/2` and `Height/2` (exact halvings), so they are written plainly.
fn positioned_tooltip_tail(
    position: &str,
    b: SceneBox,
    fill: Option<Paint>,
    stroke: Stroke,
) -> std::result::Result<Node, String> {
    let half_width = b.x + b.width / 2.0;
    let (mut tail_x, mut tail_y) = (half_width, b.y + b.height);
    let mut direction = "bottom";
    match position {
        "top-left" => (tail_x, tail_y) = (b.x + 20.0, b.y + b.height),
        "top-center" => {}
        "top-right" => (tail_x, tail_y) = (b.x + b.width - 20.0, b.y + b.height),
        "center-left" => {
            (direction, tail_x, tail_y) = ("right", b.x + b.width, b.y + b.height / 2.0)
        }
        "center-right" => (direction, tail_x, tail_y) = ("left", b.x, b.y + b.height / 2.0),
        "bottom-left" => (direction, tail_x, tail_y) = ("top", b.x + 20.0, b.y),
        "bottom-center" => (direction, tail_x, tail_y) = ("top", half_width, b.y),
        "bottom-right" => (direction, tail_x, tail_y) = ("top", b.x + b.width - 20.0, b.y),
        _ => {}
    }
    let size = TAIL_SIZE;
    let commands = match direction {
        "top" => vec![
            PathCommand::move_to(tail_x - size / 2.0, tail_y),
            PathCommand::line_to(tail_x + size / 2.0, tail_y),
            PathCommand::line_to(tail_x, tail_y - size),
            PathCommand::close(),
        ],
        "bottom" => vec![
            PathCommand::move_to(tail_x - size / 2.0, tail_y),
            PathCommand::line_to(tail_x + size / 2.0, tail_y),
            PathCommand::line_to(tail_x, tail_y + size),
            PathCommand::close(),
        ],
        "left" => vec![
            PathCommand::move_to(tail_x, tail_y - size / 2.0),
            PathCommand::line_to(tail_x, tail_y + size / 2.0),
            PathCommand::line_to(tail_x - size, tail_y),
            PathCommand::close(),
        ],
        "right" => vec![
            PathCommand::move_to(tail_x, tail_y - size / 2.0),
            PathCommand::line_to(tail_x, tail_y + size / 2.0),
            PathCommand::line_to(tail_x + size, tail_y),
            PathCommand::close(),
        ],
        other => return Err(format!("unknown direction {}", q(other))),
    };
    Ok(Node::new(Some(Primitive::Path(Path {
        commands,
        fill,
        stroke: Some(stroke),
        ..Default::default()
    }))))
}
