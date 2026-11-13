//! Coordinate admission at semantic boundaries. A source box is not a bound on
//! its rotated corners, a curve's controls, or an image's uncropped placement.
//! Check those values independently, before bounds/painting consume them
//! (spec §13; the invariant is derived in
//! `docs/research/coordinate-admission.md`).

use crate::normalization::{NormalizedElement, NormalizedElementKind};
use crate::portable_math::sin_cos;
use crate::{FiniteValue, RenderRequest, ResourceValue};

use super::geometry::{source_layout_bounds, GeneratedGeometry, Path, PathOperation, Point};
use super::resolve::{ElementLocalCrossing, ResolvedImageProjection, ResolvedTransform};

#[derive(Clone, Copy)]
pub(super) struct CoordinateRange(pub(super) f64);

impl CoordinateRange {
    pub(super) fn value(self, value: f64) -> Result<(), f64> {
        if value.is_finite() && value.abs() <= self.0 {
            Ok(())
        } else {
            Err(value.abs())
        }
    }

    pub(super) fn values(self, values: impl IntoIterator<Item = f64>) -> Result<(), f64> {
        for value in values {
            self.value(value)?;
        }
        Ok(())
    }

    pub(super) fn point(self, point: Point) -> Result<(), f64> {
        self.values([point.x, point.y])
    }

    /// Check the decomposed transform and its composed affine translation.
    /// The latter can cross even when translation and centre individually fit.
    pub(super) fn transform(self, t: ResolvedTransform) -> Result<(), f64> {
        self.values([t.translation_x, t.translation_y, t.center_x, t.center_y])?;
        let (sin, cos) = sin_cos(t.angle_radians);
        self.values([
            t.translation_x + t.center_x - cos * t.center_x + sin * t.center_y,
            t.translation_y + t.center_y - sin * t.center_x - cos * t.center_y,
        ])
    }

    pub(super) fn placed(self, point: Point, t: ResolvedTransform) -> Result<(), f64> {
        self.point(point)?;
        let (sin, cos) = sin_cos(t.angle_radians);
        let dx = point.x - t.center_x;
        let dy = point.y - t.center_y;
        self.values([dx, dy])?;
        let x = t.center_x + dx * cos - dy * sin;
        let y = t.center_y + dx * sin + dy * cos;
        self.values([x, y, t.translation_x + x, t.translation_y + y])
    }

    pub(super) fn rect(self, x: f64, y: f64, width: f64, height: f64) -> Result<(), f64> {
        self.values([x, y, width, height, x + width, y + height])
    }

    pub(super) fn box_at(self, width: f64, height: f64, t: ResolvedTransform) -> Result<(), f64> {
        self.transform(t)?;
        self.values([width, height])?;
        for (x, y) in [(0.0, 0.0), (width, 0.0), (width, height), (0.0, height)] {
            self.placed(Point { x, y }, t)?;
        }
        Ok(())
    }

    pub(super) fn operation(
        self,
        operation: &PathOperation,
        t: ResolvedTransform,
    ) -> Result<(), f64> {
        match *operation {
            PathOperation::Move(p) | PathOperation::Line(p) => self.placed(p, t),
            PathOperation::Cubic {
                control_1,
                control_2,
                end,
            } => {
                self.placed(control_1, t)?;
                self.placed(control_2, t)?;
                self.placed(end, t)
            }
        }
    }

    pub(super) fn path(self, path: &Path, t: ResolvedTransform) -> Result<(), f64> {
        for operation in &path.operations {
            self.operation(operation, t)?;
        }
        Ok(())
    }

    pub(super) fn drawing(
        self,
        drawing: &super::rough::Drawing,
        t: ResolvedTransform,
    ) -> Result<(), f64> {
        self.path(&drawing.outline, t)?;
        if let Some(fill) = &drawing.fill {
            self.path(&fill.path, t)?;
        }
        for decoration in &drawing.decorations {
            self.drawing(&decoration.drawing, t)?;
        }
        Ok(())
    }

    pub(super) fn freedraw(
        self,
        path: &super::freedraw::Path,
        t: ResolvedTransform,
    ) -> Result<(), f64> {
        use super::freedraw::PathOperation;
        for operation in &path.operations {
            match *operation {
                PathOperation::Move(p) | PathOperation::Line(p) => self.placed(p, t)?,
                PathOperation::Quadratic { control, end } => {
                    self.placed(control, t)?;
                    self.placed(end, t)?;
                }
                PathOperation::Close => {}
            }
        }
        Ok(())
    }

    fn geometry(
        self,
        geometry: &GeneratedGeometry,
        transform: ResolvedTransform,
    ) -> Result<(), f64> {
        self.transform(transform)?;
        match geometry {
            GeneratedGeometry::Rough(rough) => {
                self.drawing(&rough.drawing, transform)?;
                if let Some(path) = &rough.extent_outline {
                    self.path(path, transform)?;
                }
            }
            GeneratedGeometry::Freedraw(free) => {
                self.freedraw(&free.path, transform)?;
                if let Some(fill) = &free.background_fill {
                    self.path(&fill.path, transform)?;
                }
            }
        }
        Ok(())
    }
}

pub(super) const IDENTITY: ResolvedTransform = ResolvedTransform {
    translation_x: 0.0,
    translation_y: 0.0,
    angle_radians: 0.0,
    center_x: 0.0,
    center_y: 0.0,
};

pub(super) fn crossing(value: f64, request: &RenderRequest) -> ElementLocalCrossing {
    ElementLocalCrossing {
        limit: crate::diagnostic_registry::LocalLimit::CoordinateMagnitude,
        configured: ResourceValue::Finite(
            FiniteValue::new(request.resource_limits().max_coordinate_magnitude())
                .expect("admitted limit"),
        ),
        // An overflow has no finite attempted value. The diagnostic uses this
        // finite lower bound, marked as such rather than silently admitting NaN.
        attempted: measurement(value, request.resource_limits().max_coordinate_magnitude()),
        pointer_field: None,
        message: if value.is_finite() {
            "element exceeds the derived coordinate limit"
        } else {
            "element coordinate arithmetic overflowed"
        },
    }
}

pub(super) fn measurement(value: f64, limit: f64) -> crate::ResourceMeasurement {
    match FiniteValue::new(value.abs()) {
        Some(value) => crate::Measurement::Attempted {
            value: ResourceValue::Finite(value),
        },
        None => crate::Measurement::AtLeast {
            value: ResourceValue::Finite(
                FiniteValue::new(limit.next_up()).expect("finite lower bound"),
            ),
        },
    }
}

pub(super) fn source_derived(
    source: &NormalizedElement,
    range: CoordinateRange,
) -> Result<(), f64> {
    let (min_x, min_y, max_x, max_y) = source_layout_bounds(source);
    range.values([min_x, min_y, max_x, max_y, max_x - min_x, max_y - min_y])?;
    let t = ResolvedTransform {
        translation_x: source.x,
        translation_y: source.y,
        angle_radians: source.angle,
        center_x: (min_x + max_x) / 2.0,
        center_y: (min_y + max_y) / 2.0,
    };
    range.transform(t)?;
    for (x, y) in [
        (min_x, min_y),
        (max_x, min_y),
        (max_x, max_y),
        (min_x, max_y),
    ] {
        range.values([source.x + x, source.y + y])?;
        range.placed(Point { x, y }, t)?;
    }
    range.value(source.stroke_width)?;
    if source.stroke_style != "solid" {
        range.value(source.stroke_width + 0.5)?;
    }
    if matches!(
        source.kind,
        NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_)
    ) {
        let (x1, y1, x2, y2) = super::frame_target::normalized_element_aabb(source);
        range.values([x1, y1, x2, y2, x2 - x1, y2 - y1])?;
    }
    match &source.kind {
        NormalizedElementKind::Text(text) => {
            range.values([text.font_size, text.font_size * text.line_height])?;
        }
        NormalizedElementKind::Image(image) => {
            if let Some(crop) = image.crop {
                range.rect(crop.x, crop.y, crop.width, crop.height)?;
                range.values([crop.natural_width, crop.natural_height])?;
            }
            let projection =
                ResolvedImageProjection::for_image(source.width, source.height, image.crop);
            projection_values(projection, range)?;
            range.image_projection(projection, image.scale, source.width, source.height, t)?;
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn projection_values(
    projection: ResolvedImageProjection,
    range: CoordinateRange,
) -> Result<(), f64> {
    range.values([projection.viewport_width, projection.viewport_height])?;
    let (x, y, width, height) = projection.content_rect();
    range.rect(x, y, width, height)?;
    let (x, y, width, height) = projection.viewport_rect();
    range.rect(x, y, width, height)
}

pub(super) fn generated(
    source: &NormalizedElement,
    geometry: &GeneratedGeometry,
    request: &RenderRequest,
) -> Result<(), f64> {
    let range = CoordinateRange(request.resource_limits().max_coordinate_magnitude());
    // Local paths must be admitted before canvas_bounds evaluates any curves.
    range.geometry(geometry, IDENTITY)?;
    range.geometry(
        geometry,
        ResolvedTransform {
            translation_x: source.x,
            translation_y: source.y,
            ..IDENTITY
        },
    )?;
    let (cx, cy) = super::resolve::outline::rotation_center(source, geometry, request.format());
    let t = ResolvedTransform {
        translation_x: source.x,
        translation_y: source.y,
        angle_radians: source.angle,
        center_x: cx,
        center_y: cy,
    };
    range.transform(t)?;
    range.geometry(geometry, t)?;
    // Layout reads the cold points-box centre even where the canvas renderer
    // rotates about the generated curve's centre. Admit that consumer too.
    let (center_x, center_y) =
        super::resolve::outline::rotation_center(source, geometry, crate::OutputFormat::Svg);
    range.geometry(
        geometry,
        ResolvedTransform {
            center_x,
            center_y,
            ..t
        },
    )?;
    Ok(())
}

impl CoordinateRange {
    pub(super) fn offset_text(
        self,
        text: &super::text::ResolvedText,
        offset: f64,
        mut transform: ResolvedTransform,
    ) -> Result<(), f64> {
        self.value(offset)?;
        let (sin, cos) = sin_cos(transform.angle_radians);
        transform.translation_x -= sin * offset;
        transform.translation_y += cos * offset;
        self.transform(transform)?;
        self.text(text, transform)
    }

    pub(super) fn image_projection(
        self,
        projection: ResolvedImageProjection,
        scale: [i8; 2],
        width: f64,
        height: f64,
        transform: ResolvedTransform,
    ) -> Result<(), f64> {
        let (x, y, w, h) = projection.content_rect();
        for (x, y) in [(x, y), (x + w, y), (x + w, y + h), (x, y + h)] {
            self.placed(
                Point {
                    x: if scale[0] < 0 { width - x } else { x },
                    y: if scale[1] < 0 { height - y } else { y },
                },
                transform,
            )?;
        }
        Ok(())
    }

    pub(super) fn label(
        self,
        label: &super::text::ResolvedLabel,
        t: ResolvedTransform,
    ) -> Result<(), f64> {
        use super::text::LabelPathOperation as Op;
        self.value(label.baseline)?;
        for glyph in &label.glyphs {
            self.placed(
                Point {
                    x: glyph.x,
                    y: glyph.y,
                },
                t,
            )?;
        }
        for op in &label.path.operations {
            match *op {
                Op::Move { x, y } | Op::Line { x, y } => self.placed(Point { x, y }, t)?,
                Op::Quadratic {
                    control_x,
                    control_y,
                    x,
                    y,
                } => {
                    self.placed(
                        Point {
                            x: control_x,
                            y: control_y,
                        },
                        t,
                    )?;
                    self.placed(Point { x, y }, t)?;
                }
                Op::Close => {}
            }
        }
        Ok(())
    }

    pub(super) fn text(
        self,
        text: &super::text::ResolvedText,
        t: ResolvedTransform,
    ) -> Result<(), f64> {
        self.value(text.font_size)?;
        for label in &text.lines {
            self.label(label, t)?;
        }
        Ok(())
    }

    fn clips(self, clips: &[super::resolve::ResolvedClip]) -> Result<(), f64> {
        for clip in clips {
            self.box_at(clip.width, clip.height, clip.transform)?;
            self.value(clip.radius)?;
            self.path(&clip.path, clip.transform)?;
        }
        Ok(())
    }
}

/// Exhaustive graph gate before accounting or any backend projection. In
/// particular a small crop/clip must never conceal an oversized content path.
pub(super) fn resolved(
    element: &super::resolve::ResolvedElement,
    range: CoordinateRange,
) -> Result<(), f64> {
    use super::resolve::ResolvedElement as E;
    let (width, height, transform, clips) = match element {
        E::Outline(e) => (e.width, e.height, e.transform, &e.frame_clips),
        E::Freedraw(e) => (e.width, e.height, e.transform, &e.frame_clips),
        E::Frame(e) => (e.width, e.height, e.transform, &e.frame_clips),
        E::Text(e) => (e.width, e.height, e.transform, &e.frame_clips),
        E::Image(e) => (e.width, e.height, e.transform, &e.frame_clips),
        E::ImageFallback(e) => (e.width, e.height, e.transform, &e.frame_clips),
        E::Fallback(e) => (e.width, e.height, e.transform, &e.frame_clips),
    };
    range.box_at(width, height, transform)?;
    range.clips(clips)?;
    match element {
        E::Outline(e) => {
            range.drawing(&e.drawing, transform)?;
            range.values([e.stroke_width, e.source_stroke_width])?;
            if let Some(mask) = e.label_mask {
                range.rect(mask.x, mask.y, mask.width, mask.height)?;
            }
            if let Some(text) = &e.generated_label {
                range.offset_text(text, e.generated_label_offset_y, transform)?;
            }
        }
        E::Freedraw(e) => {
            range.freedraw(&e.path, transform)?;
            if let Some(fill) = &e.background_fill {
                range.path(&fill.path, transform)?;
            }
        }
        E::Text(e) => range.text(&e.text, transform)?,
        E::Frame(e) => {
            if let Some(outline) = &e.outline {
                range.path(&outline.path, transform)?;
            }
            if let Some(name) = &e.name {
                range.transform(name.transform)?;
                range.label(&name.label, name.transform)?;
            }
        }
        E::Image(e) => {
            let src = e.source_rect;
            range.rect(src.x, src.y, src.width, src.height)?;
            projection_values(e.projection, range)?;
            range.image_projection(e.projection, e.scale, e.width, e.height, transform)?;
            if let Some(clip) = &e.corner_clip {
                range.value(clip.radius)?;
                range.path(&clip.path, transform)?;
            }
        }
        E::Fallback(e) => {
            if let Some(label) = &e.label {
                range.label(label, transform)?;
            }
        }
        E::ImageFallback(_) => {}
    }
    Ok(())
}
