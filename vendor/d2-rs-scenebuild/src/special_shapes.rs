// SPDX-License-Identifier: MPL-2.0
//! special_shapes.go: class and SQL table shapes.

use d2_rs_scene::{
    Box as SceneBox, Font, LineCap, LineJoin, Node, Paint, Path, PathCommand, Point, Primitive,
    Rect, Stroke, TextAnchor, TextRun,
};
use d2_rs_target::Shape;

use crate::Builder;
use crate::golib::geo;
use crate::golib::label::{self, Position};
use crate::labels::centered_text_runs;
use crate::target_ext::*;
use crate::util::*;

impl Builder<'_> {
    pub(crate) fn build_structured_shape(
        &mut self,
        s: &Shape,
        outer_fill: Option<Paint>,
        outer_stroke: Option<Stroke>,
    ) -> Result<Vec<Node>> {
        validate_structured_shape(s)?;
        let b = SceneBox {
            x: s.pos.x as f64,
            y: s.pos.y as f64,
            width: s.width as f64,
            height: s.height as f64,
        };
        let radius = clamp_border_radius(s.border_radius as f64, b.width, b.height);
        let outer = node(
            format!("{}:outline", s.id),
            Some(Primitive::Rect(Rect {
                r#box: b,
                radius_x: radius,
                radius_y: radius,
                fill: outer_fill,
                stroke: outer_stroke,
            })),
        );
        let mut nodes = vec![outer];
        match s.type_.as_str() {
            SHAPE_CLASS => nodes.extend(self.build_class_contents(s, b, radius)?),
            SHAPE_SQL_TABLE => nodes.extend(self.build_sql_table_contents(s, b, radius)?),
            _ => {
                return Err(unsupported(
                    &format!("shape {}", q(&s.id)),
                    &format!("structured shape {}", s.type_),
                ));
            }
        }
        Ok(nodes)
    }

    fn build_class_contents(&mut self, s: &Shape, b: SceneBox, radius: f64) -> Result<Vec<Node>> {
        let fields = s.fields.as_deref().unwrap_or_default();
        let methods = s.methods.as_deref().unwrap_or_default();
        let row_count = 2 + fields.len() + methods.len();
        let row_height = b.height / row_count as f64;
        let header_height = geo::go_max(
            2.0 * row_height,
            s.text.label_height as f64 + 2.0 * label::PADDING,
        );
        let header_box = SceneBox {
            x: b.x,
            y: b.y,
            width: b.width,
            height: header_height,
        };
        let header_fill = self.paint(&s.fill, &format!("shape {} class header", q(&s.id)))?;
        let mut nodes = vec![node(
            format!("{}:class-header", s.id),
            Some(structured_header_primitive(
                header_box,
                b,
                radius,
                header_fill,
            )),
        )];
        if !s.text.label.is_empty() {
            let mut font_text = s.text.clone();
            font_text.font_family = "mono".into();
            font_text.font_size = s.text.font_size + HEADER_FONT_ADD;
            font_text.bold = false;
            font_text.italic = false;
            let font = self
                .font(&font_text)
                .map_err(|e| Error(format!("scene: shape {} class header: {e}", q(&s.id))))?;
            let fill = self.paint(
                &s.font_color(),
                &format!("shape {} class header text", q(&s.id)),
            )?;
            let top_left = Position::InsideMiddleCenter.get_point_on_box(
                &scene_box_to_geo(header_box),
                0.0,
                s.text.label_width as f64,
                s.text.label_height as f64,
            );
            let header_id = format!("{}:class-header-label", s.id);
            let mut runs = centered_text_runs(
                &header_id,
                &s.text.label,
                top_left,
                s.text.label_width,
                s.text.label_height,
                s.text.font_size,
                &font,
                fill,
                false,
            );
            for (index, run) in runs.iter_mut().enumerate() {
                run.id = if index == 0 {
                    header_id.clone()
                } else {
                    format!("{header_id}:{index}")
                };
            }
            nodes.extend(runs);
        }
        let primary = self.paint(
            &s.primary_accent_color,
            &format!("shape {} class primary accent", q(&s.id)),
        )?;
        let name_paint = self.paint(&s.fill, &format!("shape {} class row name", q(&s.id)))?;
        let secondary = self.paint(
            &s.secondary_accent_color,
            &format!("shape {} class secondary accent", q(&s.id)),
        )?;
        let mut row_font_text = s.text.clone();
        row_font_text.font_family = "mono".into();
        row_font_text.bold = false;
        row_font_text.italic = false;
        let row_font = self
            .font(&row_font_text)
            .map_err(|e| Error(format!("scene: shape {} class row: {e}", q(&s.id))))?;
        let mut row_box = SceneBox {
            x: b.x,
            y: b.y + header_height,
            width: b.width,
            height: row_height,
        };
        for (index, field) in fields.iter().enumerate() {
            nodes.extend(build_class_row_nodes(
                &s.id,
                "field",
                index,
                row_box,
                s.text.font_size as f64,
                &row_font,
                field_visibility_token(field),
                &field.name,
                &field.type_,
                field.underline,
                &primary,
                &name_paint,
                &secondary,
            ));
            row_box.y += row_height;
        }
        let (mut separator_start, mut separator_end) = (row_box.x, row_box.x + row_box.width);
        if radius != 0.0 && methods.is_empty() {
            separator_start += radius;
            separator_end -= radius;
        }
        let separator_paint =
            self.paint(&s.fill, &format!("shape {} class separator", q(&s.id)))?;
        nodes.push(node(
            format!("{}:class-separator", s.id),
            Some(Primitive::Path(Path {
                commands: vec![
                    PathCommand::move_to(separator_start, row_box.y),
                    PathCommand::line_to(separator_end, row_box.y),
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
        ));
        for (index, method) in methods.iter().enumerate() {
            nodes.extend(build_class_row_nodes(
                &s.id,
                "method",
                index,
                row_box,
                s.text.font_size as f64,
                &row_font,
                method_visibility_token(method),
                &method.name,
                &method.return_,
                method.underline,
                &primary,
                &name_paint,
                &secondary,
            ));
            row_box.y += row_height;
        }
        Ok(nodes)
    }

    fn build_sql_table_contents(
        &mut self,
        s: &Shape,
        b: SceneBox,
        radius: f64,
    ) -> Result<Vec<Node>> {
        let columns = s.columns.as_deref().unwrap_or_default();
        let row_height = b.height / (1 + columns.len()) as f64;
        let header_box = SceneBox {
            x: b.x,
            y: b.y,
            width: b.width,
            height: row_height,
        };
        let header_fill = self.paint(&s.fill, &format!("shape {} table header", q(&s.id)))?;
        let mut nodes = vec![node(
            format!("{}:table-header", s.id),
            Some(structured_header_primitive(
                header_box,
                b,
                radius,
                header_fill,
            )),
        )];
        if !s.text.label.is_empty() {
            let mut font_text = s.text.clone();
            font_text.font_size = s.text.font_size + HEADER_FONT_ADD;
            font_text.bold = false;
            font_text.italic = false;
            let font = self
                .font(&font_text)
                .map_err(|e| Error(format!("scene: shape {} table header: {e}", q(&s.id))))?;
            let fill = self.paint(
                &s.font_color(),
                &format!("shape {} table header text", q(&s.id)),
            )?;
            let top_left = Position::InsideMiddleLeft.get_point_on_box(
                &scene_box_to_geo(header_box),
                HEADER_PADDING as f64,
                b.width,
                s.text.label_height as f64,
            );
            nodes.push(structured_text_node(
                format!("{}:table-header-label", s.id),
                &s.text.label,
                Point::new(
                    top_left.x,
                    top_left.y + s.text.label_height as f64 * 3.0 / 4.0,
                ),
                TextAnchor::START,
                &font,
                fill,
                false,
                header_box,
            ));
        }
        let mut longest_name_width = 0;
        for column in columns {
            longest_name_width = longest_name_width.max(column.name.label_width);
        }
        let mut row_font_text = s.text.clone();
        row_font_text.bold = false;
        row_font_text.italic = false;
        let row_font = self
            .font(&row_font_text)
            .map_err(|e| Error(format!("scene: shape {} table row: {e}", q(&s.id))))?;
        let name_paint = self.paint(
            &s.primary_accent_color,
            &format!("shape {} table name accent", q(&s.id)),
        )?;
        let type_paint = self.paint(
            &s.neutral_accent_color,
            &format!("shape {} table type accent", q(&s.id)),
        )?;
        let constraint_paint = self.paint(
            &s.secondary_accent_color,
            &format!("shape {} table constraint accent", q(&s.id)),
        )?;
        let separator_paint =
            self.paint(&s.fill, &format!("shape {} table separator", q(&s.id)))?;
        let mut row_box = SceneBox {
            x: b.x,
            y: b.y + row_height,
            width: b.width,
            height: row_height,
        };
        for (index, column) in columns.iter().enumerate() {
            let name_top_left = Position::InsideMiddleLeft.get_point_on_box(
                &scene_box_to_geo(row_box),
                NAME_PADDING as f64,
                0.0,
                s.text.font_size as f64,
            );
            let baseline = name_top_left.y + s.text.font_size as f64 * 3.0 / 4.0;
            let prefix = format!("{}:table-row:{index}", s.id);
            nodes.push(structured_text_node(
                format!("{prefix}:name"),
                &column.name.label,
                Point::new(name_top_left.x, baseline),
                TextAnchor::START,
                &row_font,
                name_paint.clone(),
                false,
                row_box,
            ));
            nodes.push(structured_text_node(
                format!("{prefix}:type"),
                &column.type_.label,
                Point::new(
                    name_top_left.x + (longest_name_width + TYPE_PADDING) as f64,
                    baseline,
                ),
                TextAnchor::START,
                &row_font,
                type_paint.clone(),
                false,
                row_box,
            ));
            nodes.push(structured_text_node(
                format!("{prefix}:constraint"),
                &constraint_abbr(column),
                Point::new(row_box.x + row_box.width - NAME_PADDING as f64, baseline),
                TextAnchor::END,
                &row_font,
                constraint_paint.clone(),
                false,
                row_box,
            ));
            row_box.y += row_height;
            let (mut line_start, mut line_end) = (row_box.x, row_box.x + row_box.width);
            if index == columns.len() - 1 && radius != 0.0 {
                line_start += radius;
                line_end -= radius;
            }
            nodes.push(node(
                format!("{prefix}:separator"),
                Some(Primitive::Path(Path {
                    commands: vec![
                        PathCommand::move_to(line_start, row_box.y),
                        PathCommand::line_to(line_end, row_box.y),
                    ],
                    stroke: Some(Stroke {
                        paint: separator_paint.clone(),
                        width: 2.0,
                        cap: LineCap::BUTT,
                        join: LineJoin::MITER,
                        miter_limit: 4.0,
                        ..Default::default()
                    }),
                    ..Default::default()
                })),
            ));
        }
        Ok(nodes)
    }
}

/// validateStructuredShape.
pub(crate) fn validate_structured_shape(s: &Shape) -> Result<()> {
    let object = format!("shape {}", q(&s.id));
    let fields = s.fields.as_deref().unwrap_or_default();
    let methods = s.methods.as_deref().unwrap_or_default();
    let columns = s.columns.as_deref().unwrap_or_default();
    if s.text.font_size <= 0
        && (!s.text.label.is_empty()
            || !fields.is_empty()
            || !methods.is_empty()
            || !columns.is_empty())
    {
        return Err(invalid_field(
            &object,
            "fontSize",
            Some(&s.text.font_size.to_string()),
            "must be positive for a structured shape with text",
        ));
    }
    if s.width <= 0 || s.height <= 0 {
        return Err(invalid_field(
            &object,
            "dimensions",
            Some(&format!("{}x{}", s.width, s.height)),
            "must be positive for a structured shape",
        ));
    }
    match s.type_.as_str() {
        SHAPE_CLASS => {
            let row_count = 2 + fields.len() + methods.len();
            let row_height = s.height as f64 / row_count as f64;
            let header_height = geo::go_max(
                2.0 * row_height,
                s.text.label_height as f64 + 2.0 * label::PADDING,
            );
            if header_height > s.height as f64 {
                return Err(invalid_field(
                    &object,
                    "labelHeight",
                    Some(&s.text.label_height.to_string()),
                    "does not fit the class height",
                ));
            }
        }
        SHAPE_SQL_TABLE => {}
        _ => {
            return Err(unsupported(
                &object,
                &format!("structured shape {}", s.type_),
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_class_row_nodes(
    shape_id: &str,
    kind: &str,
    index: usize,
    b: SceneBox,
    font_size: f64,
    font: &Font,
    prefix: &str,
    name: &str,
    type_name: &str,
    underline: bool,
    prefix_paint: &Option<Paint>,
    name_paint: &Option<Paint>,
    type_paint: &Option<Paint>,
) -> Vec<Node> {
    let geo_box = scene_box_to_geo(b);
    let prefix_top_left =
        Position::InsideMiddleLeft.get_point_on_box(&geo_box, PREFIX_PADDING, b.width, font_size);
    let type_top_right =
        Position::InsideMiddleRight.get_point_on_box(&geo_box, TYPE_PADDING as f64, 0.0, font_size);
    let baseline = prefix_top_left.y + font_size * 3.0 / 4.0;
    vec![
        structured_text_node(
            format!("{shape_id}:class-{kind}:{index}:prefix"),
            prefix,
            Point::new(prefix_top_left.x, baseline),
            TextAnchor::START,
            font,
            prefix_paint.clone(),
            false,
            b,
        ),
        structured_text_node(
            format!("{shape_id}:class-{kind}:{index}:name"),
            name,
            Point::new(prefix_top_left.x + PREFIX_WIDTH, baseline),
            TextAnchor::START,
            font,
            name_paint.clone(),
            underline,
            b,
        ),
        structured_text_node(
            format!("{shape_id}:class-{kind}:{index}:type"),
            type_name,
            Point::new(type_top_right.x, baseline),
            TextAnchor::END,
            font,
            type_paint.clone(),
            false,
            b,
        ),
    ]
}

/// structuredHeaderPrimitive.
fn structured_header_primitive(
    header: SceneBox,
    outer: SceneBox,
    radius: f64,
    fill: Option<Paint>,
) -> Primitive {
    if radius == 0.0 {
        return Primitive::Rect(Rect {
            r#box: header,
            fill,
            ..Default::default()
        });
    }
    let radius = geo::go_min(radius, geo::go_min(header.width / 2.0, header.height));
    let (x, y, w, h) = (header.x, header.y, header.width, header.height);
    let mut commands = vec![
        PathCommand::move_to(x, y + radius),
        PathCommand::cubic_to(x, y + radius, x, y, x + radius, y),
        PathCommand::line_to(x + w - radius, y),
        PathCommand::cubic_to(x + w - radius, y, x + w, y, x + w, y + radius),
    ];
    if header.height >= outer.height {
        commands.extend([
            PathCommand::line_to(x + w, y + h - radius),
            PathCommand::cubic_to(x + w, y + h - radius, x + w, y + h, x + w - radius, y + h),
            PathCommand::line_to(x + radius, y + h),
            PathCommand::cubic_to(x + radius, y + h, x, y + h, x, y + h - radius),
            PathCommand::line_to(x, y + radius),
            PathCommand::close(),
        ]);
    } else {
        commands.extend([
            PathCommand::line_to(x + w, y + h),
            PathCommand::line_to(x, y + h),
            PathCommand::close(),
        ]);
    }
    Primitive::Path(Path {
        fill,
        commands,
        ..Default::default()
    })
}

#[allow(clippy::too_many_arguments)]
fn structured_text_node(
    id: String,
    text: &str,
    origin: Point,
    anchor: TextAnchor,
    font: &Font,
    fill: Option<Paint>,
    underline: bool,
    ink: SceneBox,
) -> Node {
    node(
        id,
        Some(Primitive::TextRun(TextRun {
            text: text.to_owned(),
            origin,
            anchor,
            font: font.clone(),
            fill,
            underline,
            ink: ink.bounds(),
            ..Default::default()
        })),
    )
}

pub(crate) fn scene_box_to_geo(b: SceneBox) -> geo::Box {
    geo::Box::new(geo::Point::new(b.x, b.y), b.width, b.height)
}
