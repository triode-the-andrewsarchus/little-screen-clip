// SPDX-License-Identifier: MPL-2.0
//! legend.go: legend preflight, measurement, viewbox expansion and nodes.
//! ARM64: the two inlined Matrix.Mul of the icon wrappers fuse like
//! d2scene.Matrix.Mul.

use d2_rs_scene::{
    AnimationProperty, BlendMode, Box as SceneBox, DropShadow, Easing, EasingKind, Filter,
    Keyframe, LineCap, LineJoin, Matrix, Node, Nrgba, Path, PathCommand, Point, Primitive, Rect,
    Stroke, TextAnchor, TextRun, Track,
};
use d2_rs_target::{Connection, GeoPoint, Point as TargetPoint, Shape, Text};
use d2_rs_text::d2fonts::FontStyle;

use crate::golib::geo;
use crate::golib::markdown::ruler::{FontSpec, Ruler};
use crate::shape_effects::{shadow_value, transform_value};
use crate::target_ext::*;
use crate::util::*;
use crate::{Builder, validate_opacity};

const LEGEND_PADDING: i64 = 20;
const LEGEND_ITEM_SPACING: i64 = 15;
const LEGEND_ICON_SIZE: i64 = 24;
const LEGEND_FONT_SIZE: i64 = 14;
const LEGEND_CORNER_PADDING: i64 = 10;
const LEGEND_SHAPE_SCALE_FACTOR: i64 = 5;
const LEGEND_CONNECTION_SCALE_FACTOR: i64 = 2;

#[derive(Clone, Copy, Default)]
pub(crate) struct ItemDimensions {
    width: i64,
    height: i64,
}

pub(crate) struct LegendLayout {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
    max_label_width: i64,
    item_count: usize,
    shape_dimensions: Vec<ItemDimensions>,
    connection_dimensions: Vec<ItemDimensions>,
    title_dimensions: ItemDimensions,
}

/// d2graph.GetTextDimensions for a plain (non-code) MText.
pub(crate) fn text_dimensions(
    ruler: &mut Ruler,
    text: &str,
    font_size: i64,
    bold: bool,
    italic: bool,
    family: Option<&str>,
) -> std::result::Result<(i64, i64), String> {
    let style = if bold {
        FontStyle::Bold
    } else if italic {
        FontStyle::Italic
    } else {
        FontStyle::Regular
    };
    let family = ruler.family(family.unwrap_or("SourceSansPro"))?;
    Ok(ruler.measure(FontSpec::new(family, font_size, style), text))
}

fn legend_shape_object(index: usize, id: &str) -> String {
    format!("legend shape[{index}] {}", q(id))
}

fn legend_connection_object(index: usize, id: &str) -> String {
    format!("legend connection[{index}] {}", q(id))
}

pub(crate) fn legend_shape_icon_target(index: usize, s: &Shape) -> Shape {
    let mut icon = s.clone();
    icon.id = format!("legend:shape:{index}:icon");
    icon.pos = TargetPoint::default();
    icon.width = LEGEND_ICON_SIZE * LEGEND_SHAPE_SCALE_FACTOR;
    icon.height = LEGEND_ICON_SIZE * LEGEND_SHAPE_SCALE_FACTOR;
    icon.text.label.clear();
    icon
}

fn legend_connection_icon_target(index: usize, c: &Connection) -> Connection {
    let mut icon = base_connection();
    icon.id = format!("legend:connection:{index}:icon");
    icon.src_arrow = c.src_arrow.clone();
    icon.dst_arrow = c.dst_arrow.clone();
    icon.stroke_dash = c.stroke_dash;
    icon.stroke_width = c.stroke_width;
    icon.stroke = c.stroke.clone();
    icon.fill = c.fill.clone();
    icon.border_radius = c.border_radius;
    icon.opacity = c.opacity;
    icon.animated = c.animated;
    let width = (LEGEND_ICON_SIZE * LEGEND_CONNECTION_SCALE_FACTOR) as f64;
    icon.route = Some(vec![
        Some(GeoPoint { x: 0.0, y: 0.0 }),
        Some(GeoPoint { x: width, y: 0.0 }),
    ]);
    icon
}

/// legendFontToFamily: D2_FONT_TO_FAMILY lookup (case-sensitive).
fn legend_font_to_family(font_family: &str) -> Option<&'static str> {
    match font_family {
        "default" => Some("SourceSansPro"),
        "mono" => Some("SourceCodePro"),
        _ => None,
    }
}

/// legendRenderedText: SVG whitespace collapsing.
pub(crate) fn legend_rendered_text(value: &str) -> String {
    let mut result = String::new();
    let mut pending_space = false;
    for r in value.chars() {
        match r {
            ' ' | '\t' | '\n' | '\r' => {
                if !result.is_empty() {
                    pending_space = true;
                }
            }
            _ => {
                if pending_space {
                    result.push(' ');
                    pending_space = false;
                }
                result.push(r);
            }
        }
    }
    result
}

fn legend_text_ink(x: f64, baseline: f64, d: ItemDimensions) -> d2_rs_scene::Bounds {
    let width = geo::go_max(1.0, d.width as f64);
    let height = geo::go_max(1.0, d.height as f64);
    d2_rs_scene::Bounds::new(x, baseline - height, x + width, baseline)
}

/// expandViewBox.
pub(crate) fn expand_view_box(
    layout: Option<&LegendLayout>,
    left: i64,
    top: i64,
    width: i64,
    height: i64,
    pad: i64,
) -> Result<(i64, i64, i64, i64)> {
    let Some(layout) = layout.filter(|l| l.max_label_width > 0) else {
        return Ok((left, top, width, height));
    };
    let legend_right = checked_add(layout.x, layout.width, MIN_INT, MAX_INT).ok_or_else(|| {
        Error("scene: legend right edge exceeds the platform integer domain".into())
    })?;
    let right = checked_add(left, width, MIN_INT, MAX_INT).ok_or_else(|| {
        Error("scene: legend viewbox right edge exceeds the platform integer domain".into())
    })?;
    let (left64, mut top64, mut width64, mut height64) = (left, top, width, height);
    if right < legend_right {
        width64 = checked_sub(legend_right, left64, MIN_INT, MAX_INT)
            .and_then(|w| checked_add(w, pad / 2, MIN_INT, MAX_INT))
            .ok_or_else(|| {
                Error("scene: legend width expansion exceeds the platform integer domain".into())
            })?;
    }
    if layout.y < top64 {
        let difference = checked_sub(top64, layout.y, MIN_INT, MAX_INT).ok_or_else(|| {
            Error("scene: legend top expansion exceeds the platform integer domain".into())
        })?;
        top64 = layout.y;
        height64 = checked_add(height64, difference, MIN_INT, MAX_INT).ok_or_else(|| {
            Error("scene: legend height expansion exceeds the platform integer domain".into())
        })?;
    }
    let legend_bottom =
        checked_add(layout.y, layout.height, MIN_INT, MAX_INT).ok_or_else(|| {
            Error("scene: legend bottom edge exceeds the platform integer domain".into())
        })?;
    let bottom = checked_add(top64, height64, MIN_INT, MAX_INT).ok_or_else(|| {
        Error("scene: legend viewbox bottom edge exceeds the platform integer domain".into())
    })?;
    if bottom < legend_bottom {
        height64 = checked_sub(legend_bottom, top64, MIN_INT, MAX_INT)
            .and_then(|h| checked_add(h, pad / 2, MIN_INT, MAX_INT))
            .ok_or_else(|| {
                Error("scene: legend bottom expansion exceeds the platform integer domain".into())
            })?;
    }
    if width64 < 0 || height64 < 0 {
        return Err(crate::invalid_padding_error(pad));
    }
    Ok((left64, top64, width64, height64))
}

fn legend_shape_paints_shadow(shape_type: &str) -> bool {
    !matches!(
        shape_type,
        SHAPE_TEXT | SHAPE_CODE | SHAPE_CLASS | SHAPE_SQL_TABLE
    )
}

fn linear() -> Easing {
    Easing {
        kind: EasingKind::LINEAR,
        ..Default::default()
    }
}

fn add_legend_shape_animation(n: &mut Node) {
    let transparent = DropShadow::default();
    n.filters.get_or_insert_with(Vec::new).extend([
        Filter::DropShadow(transparent),
        Filter::DropShadow(transparent),
    ]);
    let shadow_track = |index: i64, middle: DropShadow| Track {
        property: AnimationProperty::DROP_SHADOW,
        target_index: index,
        delay: 0,
        duration: 1_000_000_000,
        repeat: true,
        keyframes: Some(vec![
            Keyframe {
                offset: 0.0,
                value: shadow_value(transparent),
                easing: linear(),
            },
            Keyframe {
                offset: 0.5,
                value: shadow_value(middle),
                easing: linear(),
            },
            Keyframe {
                offset: 1.0,
                value: shadow_value(transparent),
                ..Default::default()
            },
        ]),
    };
    n.animations.get_or_insert_with(Vec::new).extend([
        Track {
            property: AnimationProperty::TRANSFORM,
            target_index: 0,
            delay: 0,
            duration: 1_000_000_000,
            repeat: true,
            keyframes: Some(vec![
                Keyframe {
                    offset: 0.0,
                    value: transform_value(Matrix::identity()),
                    easing: linear(),
                },
                Keyframe {
                    offset: 0.5,
                    value: transform_value(Matrix::translate(0.0, -4.0)),
                    easing: linear(),
                },
                Keyframe {
                    offset: 1.0,
                    value: transform_value(Matrix::identity()),
                    ..Default::default()
                },
            ]),
        },
        shadow_track(
            0,
            DropShadow {
                offset_x: 0.0,
                offset_y: 12.6,
                sigma_x: 25.2,
                sigma_y: 25.2,
                color: Nrgba::new(50, 50, 93, 64),
            },
        ),
        shadow_track(
            1,
            DropShadow {
                offset_x: 0.0,
                offset_y: 7.56,
                sigma_x: 15.12,
                sigma_y: 15.12,
                color: Nrgba::new(0, 0, 0, 26),
            },
        ),
    ]);
}

impl Builder<'_> {
    pub(crate) fn preflight_legend(&self) -> Result<()> {
        let Some(legend) = self.diagram.legend.as_ref() else {
            return Ok(());
        };
        let shapes = legend.shapes.as_deref().unwrap_or_default();
        let connections = legend.connections.as_deref().unwrap_or_default();
        if shapes.is_empty() && connections.is_empty() {
            return Ok(());
        }
        for (index, s) in shapes.iter().enumerate() {
            if s.text.label.is_empty() {
                continue;
            }
            let object = legend_shape_object(index, &s.id);
            if !s.link.is_empty()
                || !s.pretty_link.is_empty()
                || !s.tooltip.is_empty()
                || !s.tooltip_position.is_empty()
            {
                return Err(unsupported(
                    &object,
                    "link/tooltip metadata on a legend icon",
                ));
            }
            validate_opacity(&object, s.opacity)?;
            let mut icon = legend_shape_icon_target(index, s);
            icon.opacity = 1.0;
            icon.text.language.clear();
            icon.shadow = false;
            icon.blend = false;
            icon.animated = false;
            let embedded = matches!(
                icon.type_.as_str(),
                SHAPE_IMAGE | SHAPE_CLASS | SHAPE_SQL_TABLE
            );
            if !embedded && s.opacity == 0.0 {
                icon.icon = None;
            }
            self.preflight_shape(&icon)
                .map_err(|e| Error(format!("scene: {object} icon: {e}")))?;
        }
        for (index, c) in connections.iter().enumerate() {
            if c.text.label.is_empty() {
                continue;
            }
            let object = legend_connection_object(index, &c.id);
            if c.icon.is_some() {
                return Err(unsupported(
                    &object,
                    "connection icon asset (svg legend connections render only route and arrow styles)",
                ));
            }
            if c.src_label.as_ref().is_some_and(|t| !t.label.is_empty())
                || c.dst_label.as_ref().is_some_and(|t| !t.label.is_empty())
            {
                return Err(unsupported(
                    &object,
                    "endpoint label (svg legend connections render only route and arrow styles)",
                ));
            }
            if !c.link.is_empty() || !c.pretty_link.is_empty() || !c.tooltip.is_empty() {
                return Err(unsupported(
                    &object,
                    "link/tooltip metadata on a legend connection",
                ));
            }
            let icon = legend_connection_icon_target(index, c);
            self.preflight_connection(&icon)
                .map_err(|e| Error(format!("scene: {object} icon: {e}")))?;
        }
        Ok(())
    }

    pub(crate) fn measure_legend(
        &self,
        tl: TargetPoint,
        br: TargetPoint,
    ) -> Result<Option<LegendLayout>> {
        let Some(legend) = self.diagram.legend.as_ref() else {
            return Ok(None);
        };
        let shapes = legend.shapes.as_deref().unwrap_or_default();
        let connections = legend.connections.as_deref().unwrap_or_default();
        if shapes.is_empty() && connections.is_empty() {
            return Ok(None);
        }
        let mut ruler = Ruler::new();
        let mut layout = LegendLayout {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            max_label_width: 0,
            item_count: 0,
            shape_dimensions: vec![ItemDimensions::default(); shapes.len()],
            connection_dimensions: vec![ItemDimensions::default(); connections.len()],
            title_dimensions: ItemDimensions::default(),
        };
        let mut total_height = LEGEND_PADDING + LEGEND_FONT_SIZE + LEGEND_ITEM_SPACING;
        let mut item_count = 0;
        let measure = |ruler: &mut Ruler,
                       object: &str,
                       value: &str,
                       family: &str|
         -> Result<ItemDimensions> {
            let (width, height) = text_dimensions(
                ruler,
                value,
                LEGEND_FONT_SIZE,
                false,
                false,
                legend_font_to_family(family),
            )
            .map_err(|_| {
                Error(format!(
                    "scene: {object} has invalid measured text dimensions"
                ))
            })?;
            if width < 0 || height < 0 {
                return err(format!(
                    "scene: {object} has invalid measured text dimensions"
                ));
            }
            Ok(ItemDimensions { width, height })
        };
        let mut add_item = |layout: &mut LegendLayout,
                            total_height: &mut i64,
                            object: &str,
                            d: ItemDimensions|
         -> Result<()> {
            let row_height = d.height.max(LEGEND_ICON_SIZE);
            let increment = row_height + LEGEND_ITEM_SPACING;
            if *total_height > MAX_INT - increment {
                return err(format!(
                    "scene: {object} causes legend height to exceed the platform integer domain"
                ));
            }
            *total_height += increment;
            layout.max_label_width = layout.max_label_width.max(d.width);
            item_count += 1;
            Ok(())
        };
        for (index, s) in shapes.iter().enumerate() {
            if s.text.label.is_empty() {
                continue;
            }
            let object = legend_shape_object(index, &s.id);
            let d = measure(&mut ruler, &object, &s.text.label, &s.text.font_family)?;
            layout.shape_dimensions[index] = d;
            add_item(&mut layout, &mut total_height, &object, d)?;
        }
        for (index, c) in connections.iter().enumerate() {
            if c.text.label.is_empty() {
                continue;
            }
            let object = legend_connection_object(index, &c.id);
            let d = measure(&mut ruler, &object, &c.text.label, &c.text.font_family)?;
            layout.connection_dimensions[index] = d;
            add_item(&mut layout, &mut total_height, &object, d)?;
        }
        if item_count > 0 {
            total_height -= LEGEND_ITEM_SPACING / 2;
        }
        if item_count > 0 && !connections.is_empty() {
            total_height += LEGEND_PADDING * 3 / 2;
        } else {
            total_height += LEGEND_PADDING * 6 / 5;
        }
        if total_height <= 0 {
            return err("scene: legend height exceeds the platform integer domain");
        }
        let mut legend_width = LEGEND_PADDING * 2 + LEGEND_ICON_SIZE + LEGEND_PADDING;
        if layout.max_label_width > MAX_INT - legend_width {
            return err("scene: legend width exceeds the platform integer domain");
        }
        legend_width += layout.max_label_width;
        let legend_x =
            checked_add(br.x, LEGEND_CORNER_PADDING, MIN_INT, MAX_INT).ok_or_else(|| {
                Error("scene: legend x position exceeds the platform integer domain".into())
            })?;
        let mut legend_y = checked_sub(br.y, total_height, MIN_INT, MAX_INT).ok_or_else(|| {
            Error("scene: legend y position exceeds the platform integer domain".into())
        })?;
        if legend_y < tl.y {
            legend_y = tl.y;
        }
        (layout.x, layout.y, layout.width, layout.height) =
            (legend_x, legend_y, legend_width, total_height);
        for (name, value, delta) in [
            ("right edge", legend_x, legend_width),
            ("bottom edge", legend_y, total_height),
            ("title x position", legend_x, LEGEND_PADDING),
            (
                "title y position",
                legend_y,
                LEGEND_PADDING + LEGEND_FONT_SIZE,
            ),
            (
                "item y position",
                legend_y,
                LEGEND_PADDING * 2 + LEGEND_FONT_SIZE,
            ),
        ] {
            if checked_add(value, delta, MIN_INT, MAX_INT).is_none() {
                return err(format!(
                    "scene: legend {name} exceeds the platform integer domain"
                ));
            }
        }
        let title = if legend.label.is_empty() {
            "Legend"
        } else {
            legend.label.as_str()
        };
        let (width, height) = text_dimensions(
            &mut ruler,
            title,
            LEGEND_FONT_SIZE + 2,
            true,
            false,
            self.diagram.font_family.as_deref(),
        )
        .map_err(|_| Error("scene: legend title has invalid measured text dimensions".into()))?;
        if width < 0 || height < 0 {
            return err("scene: legend title has invalid measured text dimensions");
        }
        layout.title_dimensions = ItemDimensions { width, height };
        layout.item_count = item_count;
        Ok(Some(layout))
    }

    pub(crate) fn build_legend(&mut self, layout: Option<&LegendLayout>) -> Result<Option<Node>> {
        let Some(layout) = layout else {
            return Ok(None);
        };
        let shadow_fill = self.paint("#F7F7FA", "legend shadow fill")?;
        let panel_fill = self.paint("#ffffff", "legend panel fill")?;
        let border_paint = self.paint("#DEE1EB", "legend border")?;
        let text_paint = self.paint("#000000", "legend text")?;
        let border = || Stroke {
            paint: border_paint.clone(),
            width: 1.0,
            cap: LineCap::BUTT,
            join: LineJoin::MITER,
            miter_limit: 4.0,
            ..Default::default()
        };
        let b = SceneBox {
            x: layout.x as f64,
            y: layout.y as f64,
            width: layout.width as f64,
            height: layout.height as f64,
        };
        let mut legend = node("legend", None);
        let mut shadow = node(
            "legend:shadow",
            Some(Primitive::Rect(Rect {
                r#box: b,
                radius_x: 4.0,
                radius_y: 4.0,
                fill: shadow_fill,
                stroke: Some(border()),
            })),
        );
        shadow.filters = Some(vec![Filter::DropShadow(DropShadow {
            offset_x: 0.0,
            offset_y: 2.0,
            sigma_x: 3.0,
            sigma_y: 3.0,
            color: Nrgba::new(0, 0, 0, 26),
        })]);
        let panel = node(
            "legend:panel",
            Some(Primitive::Rect(Rect {
                r#box: b,
                radius_x: 4.0,
                radius_y: 4.0,
                fill: panel_fill,
                stroke: Some(border()),
            })),
        );
        let mut children = vec![shadow, panel];
        let legend_spec = self.diagram.legend.as_ref().expect("measured legend");
        let title = if legend_spec.label.is_empty() {
            "Legend"
        } else {
            legend_spec.label.as_str()
        };
        let title_font = self
            .font(&Text {
                font_size: LEGEND_FONT_SIZE + 2,
                bold: true,
                ..Default::default()
            })
            .map_err(|e| Error(format!("scene: legend title: {e}")))?;
        let title_x = (layout.x + LEGEND_PADDING) as f64;
        let title_y = (layout.y + LEGEND_PADDING + LEGEND_FONT_SIZE) as f64;
        children.push(node(
            "legend:title",
            Some(Primitive::TextRun(TextRun {
                text: legend_rendered_text(title),
                origin: Point::new(title_x, title_y),
                anchor: TextAnchor::START,
                font: title_font,
                fill: text_paint.clone(),
                ink: legend_text_ink(title_x, title_y, layout.title_dimensions),
                ..Default::default()
            })),
        ));
        let item_font = if layout.item_count > 0 {
            Some(
                self.font(&Text {
                    font_size: LEGEND_FONT_SIZE,
                    ..Default::default()
                })
                .map_err(|e| Error(format!("scene: legend item: {e}")))?,
            )
        } else {
            None
        };
        let mut current_y = layout.y + LEGEND_PADDING * 2 + LEGEND_FONT_SIZE;
        let mut shape_count = 0;
        let shapes = legend_spec.shapes.as_deref().unwrap_or_default();
        let connections = legend_spec.connections.as_deref().unwrap_or_default();
        for (index, s) in shapes.iter().enumerate() {
            if s.text.label.is_empty() {
                continue;
            }
            let (icon_x, icon_y) = (layout.x + LEGEND_PADDING, current_y);
            children.push(self.build_legend_shape_icon(index, s, icon_x, icon_y)?);
            let d = layout.shape_dimensions[index];
            let row_height = d.height.max(LEGEND_ICON_SIZE);
            let text_x = (icon_x + LEGEND_ICON_SIZE + LEGEND_PADDING) as f64;
            let text_y = (current_y + row_height / 2 + (d.height as f64 * 0.3) as i64) as f64;
            children.push(node(
                format!("legend:shape:{index}:label"),
                Some(Primitive::TextRun(TextRun {
                    text: legend_rendered_text(&s.text.label),
                    origin: Point::new(text_x, text_y),
                    anchor: TextAnchor::START,
                    font: item_font.clone().unwrap_or_default(),
                    fill: text_paint.clone(),
                    ink: legend_text_ink(text_x, text_y, d),
                    ..Default::default()
                })),
            ));
            current_y += row_height + LEGEND_ITEM_SPACING;
            shape_count += 1;
        }
        if shape_count > 0 && !connections.is_empty() {
            current_y += LEGEND_ITEM_SPACING / 2;
            let mut separator_stroke = border();
            separator_stroke.dashes = Some(vec![2.0, 2.0]);
            children.push(node(
                "legend:separator",
                Some(Primitive::Path(Path {
                    commands: vec![
                        PathCommand::move_to((layout.x + LEGEND_PADDING) as f64, current_y as f64),
                        PathCommand::line_to(
                            (layout.x + layout.width - LEGEND_PADDING) as f64,
                            current_y as f64,
                        ),
                    ],
                    stroke: Some(separator_stroke),
                    ..Default::default()
                })),
            ));
            current_y += LEGEND_ITEM_SPACING;
        }
        for (index, c) in connections.iter().enumerate() {
            if c.text.label.is_empty() {
                continue;
            }
            let icon_x = layout.x + LEGEND_PADDING;
            let icon_y = current_y + LEGEND_ICON_SIZE / 2;
            children.push(self.build_legend_connection_icon(index, c, icon_x, icon_y)?);
            let d = layout.connection_dimensions[index];
            let row_height = d.height.max(LEGEND_ICON_SIZE);
            let text_x = (icon_x + LEGEND_ICON_SIZE + LEGEND_PADDING) as f64;
            let text_y = (current_y + row_height / 2 + (d.height as f64 * 0.2) as i64) as f64;
            children.push(node(
                format!("legend:connection:{index}:label"),
                Some(Primitive::TextRun(TextRun {
                    text: legend_rendered_text(&c.text.label),
                    origin: Point::new(text_x, text_y),
                    anchor: TextAnchor::START,
                    font: item_font.clone().unwrap_or_default(),
                    fill: text_paint.clone(),
                    ink: legend_text_ink(text_x, text_y, d),
                    ..Default::default()
                })),
            ));
            current_y += row_height + LEGEND_ITEM_SPACING;
        }
        legend.children = Some(children);
        Ok(Some(legend))
    }

    fn build_legend_shape_icon(&mut self, index: usize, s: &Shape, x: i64, y: i64) -> Result<Node> {
        let target = legend_shape_icon_target(index, s);
        let mut geometry_target = target.clone();
        geometry_target.opacity = 1.0;
        geometry_target.animated = false;
        geometry_target.shadow = false;
        geometry_target.blend = false;
        let embedded = matches!(
            target.type_.as_str(),
            SHAPE_IMAGE | SHAPE_CLASS | SHAPE_SQL_TABLE
        );
        if !embedded {
            geometry_target.icon = None;
        }
        let mut geometry = self
            .build_shape(&geometry_target)
            .map_err(|e| Error(format!("scene: {}: {e}", legend_shape_object(index, &s.id))))?;
        geometry.id = format!("{}:shape", target.id);
        geometry.classes = None;
        if s.shadow && legend_shape_paints_shadow(&s.type_) {
            geometry
                .filters
                .get_or_insert_with(Vec::new)
                .push(Filter::DropShadow(DropShadow {
                    offset_x: 3.0,
                    offset_y: 5.0,
                    sigma_x: 1.7,
                    sigma_y: 1.7,
                    color: Nrgba::new(0x3d, 0x45, 0x74, 102),
                }));
        }
        if s.blend {
            geometry.blend = BlendMode::MULTIPLY;
            geometry.opacity *= 0.5;
        }
        let mut target_node = node(target.id.clone(), None);
        target_node.classes = s.classes.clone().filter(|c| !c.is_empty());
        target_node.opacity = s.opacity;
        let mut target_children = vec![geometry];
        if !embedded && s.icon.is_some() && s.opacity != 0.0 {
            let mut icon_target = target.clone();
            icon_target.opacity = 1.0;
            if let Some(icon) = self
                .build_shape_icon(&icon_target, false)
                .map_err(|e| Error(format!("scene: {}: {e}", legend_shape_object(index, &s.id))))?
            {
                target_children.push(icon);
            }
        }
        target_node.children = Some(target_children);
        if s.animated {
            add_legend_shape_animation(&mut target_node);
        }
        let mut wrapper = node(format!("legend:shape:{index}"), None);
        wrapper.transform = Matrix::translate(x as f64, y as f64).mul(Matrix::scale(
            1.0 / LEGEND_SHAPE_SCALE_FACTOR as f64,
            1.0 / LEGEND_SHAPE_SCALE_FACTOR as f64,
        ));
        wrapper.children = Some(vec![target_node]);
        Ok(wrapper)
    }

    fn build_legend_connection_icon(
        &mut self,
        index: usize,
        c: &Connection,
        x: i64,
        y: i64,
    ) -> Result<Node> {
        let target = legend_connection_icon_target(index, c);
        let connection_mask = self.connection_mask.take();
        let id_to_shape = self.id_to_shape.take();
        let result = self.build_connection(&target);
        self.connection_mask = connection_mask;
        self.id_to_shape = id_to_shape;
        let n = result.map_err(|e| {
            Error(format!(
                "scene: {}: {e}",
                legend_connection_object(index, &c.id)
            ))
        })?;
        let mut wrapper = node(format!("legend:connection:{index}"), None);
        wrapper.transform = Matrix::translate(x as f64, y as f64).mul(Matrix::scale(
            1.0 / LEGEND_CONNECTION_SCALE_FACTOR as f64,
            1.0 / LEGEND_CONNECTION_SCALE_FACTOR as f64,
        ));
        wrapper.children = Some(vec![n]);
        Ok(wrapper)
    }
}
