// SPDX-License-Identifier: MPL-2.0
//! labels.go: the diagram-wide connection label mask, shape label placement
//! and connection labels.

use d2_rs_scene::{
    Bounds, Box as SceneBox, Font, Mask, MaskType, Matrix, Node, Nrgba, Paint, Point, Primitive,
    Rect, TextAnchor, TextRun,
};
use d2_rs_target::{Connection, Shape};

use crate::golib::geo::{self, go_fmadd};
use crate::golib::label::{self, Position};
use crate::golib::shape::Shape as _;
use crate::target_ext::*;
use crate::util::*;
use crate::{Builder, target_geometry};

impl Builder<'_> {
    /// buildConnectionMask: one diagram-wide luminance mask.
    pub(crate) fn build_connection_mask(&self, view_box: SceneBox) -> Result<Option<Mask>> {
        let black = Paint::solid(Nrgba::new(0, 0, 0, 0xff));
        let white = Paint::solid(Nrgba::new(0xff, 0xff, 0xff, 0xff));
        let mut holes = Vec::new();
        for s in self.diagram.shapes() {
            let position = Position::from_string(&s.label_position);
            if s.text.label.is_empty() || !position.is_border() {
                continue;
            }
            if s.type_ == SHAPE_CLASS || s.type_ == SHAPE_SQL_TABLE {
                continue;
            }
            let (b, top_left) = shape_label_placement(s);
            let Some(hole_box) = border_label_mask_box(
                position,
                top_left,
                s.text.label_width,
                s.text.label_height,
                &b,
                s.stroke_width,
            )
            .filter(|h| h.width > 0.0 && h.height > 0.0) else {
                continue;
            };
            holes.push(node(
                format!("{}:border-label-mask-hole", s.id),
                Some(Primitive::Rect(Rect {
                    r#box: hole_box,
                    fill: Some(black.clone()),
                    ..Default::default()
                })),
            ));
        }
        for c in self.diagram.connections() {
            if c.text.label.is_empty() && c.icon.is_none() {
                continue;
            }
            let hole_box;
            let mut position = c.icon_position.as_str();
            if !c.text.label.is_empty() {
                let Some(top_left) = connection_label_top_left(c) else {
                    return Err(invalid_field(
                        &format!("connection {}", q(&c.id)),
                        "labelPosition",
                        Some(&c.label_position),
                        "does not resolve on the route",
                    ));
                };
                let mut x = top_left.x.round() - 2.0;
                let mut width = c.text.label_width + 4;
                if c.icon.is_some() {
                    x -= (CONNECTION_ICON_LABEL_GAP + DEFAULT_ICON_SIZE) as f64;
                    width += CONNECTION_ICON_LABEL_GAP + DEFAULT_ICON_SIZE;
                }
                hole_box = SceneBox {
                    x,
                    y: top_left.y.round(),
                    width: width as f64,
                    height: c.text.label_height as f64,
                };
                position = &c.label_position;
            } else {
                let Some(top_left) = connection_icon_position(c) else {
                    return Err(invalid_field(
                        &format!("connection {}", q(&c.id)),
                        "iconPosition",
                        Some(&c.icon_position),
                        "does not resolve on the route",
                    ));
                };
                hole_box = SceneBox {
                    x: top_left.x - 2.0,
                    y: top_left.y,
                    width: (DEFAULT_ICON_SIZE + 4) as f64,
                    height: DEFAULT_ICON_SIZE as f64,
                };
            }
            let mut hole = node(
                format!("{}:label-mask-hole", c.id),
                Some(Primitive::Rect(Rect {
                    r#box: hole_box,
                    fill: Some(black.clone()),
                    ..Default::default()
                })),
            );
            if !Position::from_string(position).is_on_edge() {
                hole.opacity = 0.75;
            }
            holes.push(hole);
        }
        if holes.is_empty() {
            return Ok(None);
        }
        let mut root = node("connection-label-mask", None);
        let base = node(
            "connection-label-mask:base",
            Some(Primitive::Rect(Rect {
                r#box: view_box,
                fill: Some(white),
                ..Default::default()
            })),
        );
        let mut children = vec![base];
        children.extend(holes);
        root.children = Some(children);
        Ok(Some(Mask {
            r#type: MaskType::LUMINANCE,
            root: Some(std::sync::Arc::new(root)),
            transform: Matrix::identity(),
        }))
    }

    /// buildConnectionLabels.
    pub(crate) fn build_connection_labels(&mut self, c: &Connection) -> Result<Vec<Node>> {
        let mut nodes = Vec::new();
        if !c.text.label.is_empty() {
            let Some(top_left) = connection_label_top_left(c) else {
                return Err(invalid_field(
                    &format!("connection {}", q(&c.id)),
                    "labelPosition",
                    Some(&c.label_position),
                    "does not resolve on the route",
                ));
            };
            let top_left = geo::Point::new(top_left.x.round(), top_left.y.round());
            if c.text.language == "latex" {
                let latex = self.build_latex_label_node(
                    &format!("connection {}", q(&c.id)),
                    &format!("{}:label:0", c.id),
                    &c.text.label,
                    &c.text.color,
                    Some(top_left),
                )?;
                nodes.push(latex);
            } else if c.text.language == "markdown" {
                nodes.push(self.build_connection_markdown(c, top_left)?);
            } else if !c.text.language.is_empty() {
                let object = format!("connection {}", q(&c.id));
                let dark = self.theme_is_dark();
                let runs = self.build_code_text_runs(
                    &object,
                    &format!("{}:code", c.id),
                    &c.text,
                    dark,
                    Point::new(top_left.x, top_left.y),
                )?;
                nodes.extend(runs);
            } else {
                if !c.fill.is_empty() {
                    let fill =
                        self.paint(&c.fill, &format!("connection {} label fill", q(&c.id)))?;
                    nodes.push(node(
                        format!("{}:label-fill", c.id),
                        Some(Primitive::Rect(Rect {
                            r#box: SceneBox {
                                x: top_left.x - 4.0,
                                y: top_left.y - 3.0,
                                width: (c.text.label_width + 8) as f64,
                                height: (c.text.label_height + 6) as f64,
                            },
                            radius_x: 10.0,
                            radius_y: 10.0,
                            fill,
                            stroke: None,
                        })),
                    ));
                }
                let font = self
                    .font(&c.text)
                    .map_err(|e| Error(format!("scene: connection {} label: {e}", q(&c.id))))?;
                let mut font_color = connection_font_color(c);
                let mut underline = c.text.underline;
                if !c.link.is_empty() {
                    font_color = "blue".into();
                    underline = true;
                }
                let fill =
                    self.paint(&font_color, &format!("connection {} label color", q(&c.id)))?;
                nodes.extend(centered_text_runs(
                    &format!("{}:label", c.id),
                    &c.text.label,
                    top_left,
                    c.text.label_width,
                    c.text.label_height,
                    c.text.font_size,
                    &font,
                    fill,
                    underline,
                ));
            }
        }
        for (name, text, is_dst) in [("src", &c.src_label, false), ("dst", &c.dst_label, true)] {
            let Some(text) = text.as_ref().filter(|t| !t.label.is_empty()) else {
                continue;
            };
            let mut font_text = c.text.clone();
            font_text.bold = false;
            font_text.italic = true;
            font_text.underline = false;
            font_text.font_family = "default".into();
            let font = self.font(&font_text).map_err(|e| {
                Error(format!(
                    "scene: connection {} {name} arrowhead label: {e}",
                    q(&c.id)
                ))
            })?;
            let color_value = if text.color.is_empty() {
                FG_COLOR.to_owned()
            } else {
                text.color.clone()
            };
            let fill = self.paint(
                &color_value,
                &format!("connection {} {name} arrowhead label color", q(&c.id)),
            )?;
            let top_left = connection_arrowhead_label_position(c, is_dst);
            nodes.extend(centered_text_runs(
                &format!("{}:{name}-label", c.id),
                &text.label,
                top_left,
                text.label_width,
                text.label_height,
                c.text.font_size,
                &font,
                fill,
                false,
            ));
        }
        Ok(nodes)
    }

    pub(crate) fn theme_is_dark(&self) -> bool {
        self.theme.id >= 200 && self.theme.id < 300
    }
}

/// borderLabelMaskBox.
pub(crate) fn border_label_mask_box(
    position: Position,
    top_left: geo::Point,
    label_width: i64,
    label_height: i64,
    shape_box: &geo::Box,
    stroke_width: i64,
) -> Option<SceneBox> {
    use Position::*;
    let sw = stroke_width as f64;
    Some(match position {
        BorderTopLeft | BorderTopCenter | BorderTopRight => SceneBox {
            x: top_left.x - 2.0,
            y: shape_box.top_left.y - sw / 2.0,
            width: (label_width + 4) as f64,
            height: sw,
        },
        BorderBottomLeft | BorderBottomCenter | BorderBottomRight => SceneBox {
            x: top_left.x - 2.0,
            y: shape_box.top_left.y + shape_box.height - sw / 2.0,
            width: (label_width + 4) as f64,
            height: sw,
        },
        BorderLeftTop | BorderLeftMiddle | BorderLeftBottom => SceneBox {
            x: shape_box.top_left.x - sw / 2.0,
            y: top_left.y - 2.0,
            width: sw,
            height: (label_height + 4) as f64,
        },
        BorderRightTop | BorderRightMiddle | BorderRightBottom => SceneBox {
            x: shape_box.top_left.x + shape_box.width - sw / 2.0,
            y: top_left.y - 2.0,
            width: sw,
            height: (label_height + 4) as f64,
        },
        _ => return None,
    })
}

/// The label box with 3D/multiple expansion for outside and border labels.
pub(crate) fn expanded_label_box(s: &Shape, position: Position) -> geo::Box {
    let geometry = target_geometry(s);
    if position.is_outside() || position.is_border() {
        let mut b = geometry.get_box().copy();
        if s.three_dee {
            let offset_y = if s.type_ == SHAPE_HEXAGON {
                THREE_DEE_OFFSET / 2
            } else {
                THREE_DEE_OFFSET
            };
            b.top_left.y -= offset_y as f64;
            b.height += offset_y as f64;
            b.width += THREE_DEE_OFFSET as f64;
        } else if s.multiple {
            b.top_left.y -= MULTIPLE_OFFSET as f64;
            b.height += MULTIPLE_OFFSET as f64;
            b.width += MULTIPLE_OFFSET as f64;
        }
        b
    } else {
        geometry.get_inner_box()
    }
}

/// shapeLabelPlacement.
pub(crate) fn shape_label_placement(s: &Shape) -> (geo::Box, geo::Point) {
    let position = Position::from_string(&s.label_position);
    let b = expanded_label_box(s, position);
    let top_left = position.get_point_on_box(
        &b,
        label::PADDING,
        s.text.label_width as f64,
        s.text.label_height as f64,
    );
    (b, top_left)
}

/// centeredTextRuns.
#[allow(clippy::too_many_arguments)]
pub(crate) fn centered_text_runs(
    id_prefix: &str,
    text: &str,
    top_left: geo::Point,
    width: i64,
    height: i64,
    font_size: i64,
    font: &Font,
    fill: Option<Paint>,
    underline: bool,
) -> Vec<Node> {
    let lines: Vec<&str> = text.split('\n').collect();
    let line_advance = height as f64 / lines.len() as f64;
    let mut nodes = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        // ARM64 labels.go:280/285: FMADDD.
        let line_top = go_fmadd(index as f64, line_advance, top_left.y);
        let run = TextRun {
            text: (*line).to_owned(),
            origin: Point::new(
                top_left.x + width as f64 / 2.0,
                go_fmadd(index as f64, line_advance, top_left.y + font_size as f64),
            ),
            anchor: TextAnchor::MIDDLE,
            font: font.clone(),
            fill: fill.clone(),
            underline,
            ink: Bounds::new(
                top_left.x,
                line_top,
                top_left.x + width as f64,
                line_top + line_advance,
            ),
            ..Default::default()
        };
        nodes.push(node(
            format!("{id_prefix}:{index}"),
            Some(Primitive::TextRun(run)),
        ));
    }
    nodes
}
