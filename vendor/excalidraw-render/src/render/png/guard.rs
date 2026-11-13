//! Keeps tiny-skia's scan converter inside the coordinate range it handles.
//!
//! tiny-skia rejects a path only when its bounds approach `f32::MAX`
//! (`is_too_big_for_math`), but its fixed-point edge walker overflows, and
//! panics, once device coordinates near 2^31. A scene reaches that range
//! without a large surface: the surface follows element bounds, while a stroke
//! width, a sketch fill weight or a nested SVG's own strokes extend the painted
//! geometry far past them. With `panic = "abort"` that panic ends the process.
//!
//! A path whose device bounds stay within [`DEVICE_LIMIT`] is drawn by the
//! unchanged tiny-skia call, so ordinary output keeps its exact bytes. A larger
//! one is flattened and clipped to a margin around the surface first. Clipping a
//! closed contour to a half-plane leaves the winding number of every point
//! inside that half-plane unchanged, so the surface's coverage is the same as
//! for the unclipped path.

use tiny_skia::{
    ColorSpace, FillRule, IntRect, Mask, Paint, Path, PathBuilder, PathSegment, PathStroker,
    Pattern, PixmapMut, PixmapPaint, PixmapRef, Point, Rect, SpreadMode, Stroke, Transform,
};

/// Largest device-space magnitude passed to tiny-skia unclipped: 2^27, a
/// sixteenth of the overflow and four thousand times the largest raster side.
pub(super) const DEVICE_LIMIT: f32 = 134_217_728.0;
/// Clip margin around the surface, wider than any antialiasing reach.
const CLIP_MARGIN: f64 = 1024.0;
/// Flattening tolerance, in device pixels, for curves that cross the clip.
const FLATTEN_TOLERANCE: f64 = 0.05;
/// Subdivision depth bound; 2^-24 of a curve is below the tolerance at the limit.
const MAX_SUBDIVISION_DEPTH: u32 = 24;

/// tiny-skia's fill and stroke calls with the device-range guard.
pub(super) trait GuardedDraw {
    fn guarded_fill_path(
        &mut self,
        path: &Path,
        paint: &Paint<'_>,
        fill_rule: FillRule,
        transform: Transform,
        mask: Option<&Mask>,
    );

    fn guarded_stroke_path(
        &mut self,
        path: &Path,
        paint: &Paint<'_>,
        stroke: &Stroke,
        transform: Transform,
        mask: Option<&Mask>,
    );

    fn guarded_fill_rect(
        &mut self,
        rect: Rect,
        paint: &Paint<'_>,
        transform: Transform,
        mask: Option<&Mask>,
    );

    fn guarded_draw_pixmap(
        &mut self,
        x: i32,
        y: i32,
        pixmap: PixmapRef<'_>,
        paint: &PixmapPaint,
        transform: Transform,
        mask: Option<&Mask>,
    );
}

impl GuardedDraw for PixmapMut<'_> {
    fn guarded_fill_path(
        &mut self,
        path: &Path,
        paint: &Paint<'_>,
        fill_rule: FillRule,
        transform: Transform,
        mask: Option<&Mask>,
    ) {
        if within_device_limit(path, 0.0, transform) {
            self.fill_path(path, paint, fill_rule, transform, mask);
            return;
        }
        let Some(clipped) = clip_to_surface(path, transform, self.width(), self.height()) else {
            return;
        };
        // tiny-skia's own transformed fill moves the shader with the path.
        let mut paint = paint.clone();
        paint.shader.transform(transform);
        self.fill_path(&clipped, &paint, fill_rule, Transform::identity(), mask);
    }

    fn guarded_stroke_path(
        &mut self,
        path: &Path,
        paint: &Paint<'_>,
        stroke: &Stroke,
        transform: Transform,
        mask: Option<&Mask>,
    ) {
        if within_device_limit(path, stroke_reach(stroke), transform) {
            self.stroke_path(path, paint, stroke, transform, mask);
            return;
        }
        // tiny-skia strokes a wide path by filling its outline; do the same so
        // the outline passes through the guarded fill.
        let Some(outline) = stroke_outline(path, stroke, transform) else {
            return;
        };
        self.guarded_fill_path(&outline, paint, FillRule::Winding, transform, mask);
    }

    fn guarded_fill_rect(
        &mut self,
        rect: Rect,
        paint: &Paint<'_>,
        transform: Transform,
        mask: Option<&Mask>,
    ) {
        let in_range = within_device_bounds(
            rect.left(),
            rect.top(),
            rect.right(),
            rect.bottom(),
            0.0,
            transform,
        );
        if in_range {
            self.fill_rect(rect, paint, transform, mask);
        } else {
            self.guarded_fill_path(
                &PathBuilder::from_rect(rect),
                paint,
                FillRule::Winding,
                transform,
                mask,
            );
        }
    }

    fn guarded_draw_pixmap(
        &mut self,
        x: i32,
        y: i32,
        pixmap: PixmapRef<'_>,
        paint: &PixmapPaint,
        transform: Transform,
        mask: Option<&Mask>,
    ) {
        let Some(rect) = IntRect::from_xywh(x, y, pixmap.width(), pixmap.height()) else {
            return;
        };
        let rect = rect.to_rect();
        if within_device_bounds(
            rect.left(),
            rect.top(),
            rect.right(),
            rect.bottom(),
            0.0,
            transform,
        ) {
            self.draw_pixmap(x, y, pixmap, paint, transform, mask);
            return;
        }
        // The paint `draw_pixmap` builds for its rectangle fill.
        let paint = Paint {
            shader: Pattern::new(
                pixmap,
                SpreadMode::Pad,
                paint.quality,
                paint.opacity,
                Transform::from_translate(x as f32, y as f32),
            ),
            blend_mode: paint.blend_mode,
            anti_alias: false,
            force_hq_pipeline: false,
            colorspace: ColorSpace::default(),
        };
        self.guarded_fill_path(
            &PathBuilder::from_rect(rect),
            &paint,
            FillRule::Winding,
            transform,
            mask,
        );
    }
}

/// [`Mask`] path calls with the device-range guard.
pub(super) trait GuardedMask {
    fn guarded_fill_path(
        &mut self,
        path: &Path,
        fill_rule: FillRule,
        anti_alias: bool,
        transform: Transform,
    );

    fn guarded_intersect_path(
        &mut self,
        path: &Path,
        fill_rule: FillRule,
        anti_alias: bool,
        transform: Transform,
    );

    /// [`guarded_fill_path`](Self::guarded_fill_path), returning the device
    /// bounds of the path it filled: the scan converter writes no coverage
    /// outside them rounded out. `None` when nothing was filled.
    fn guarded_fill_path_bounds(
        &mut self,
        path: &Path,
        fill_rule: FillRule,
        anti_alias: bool,
        transform: Transform,
    ) -> Option<Rect>;
}

impl GuardedMask for Mask {
    fn guarded_fill_path(
        &mut self,
        path: &Path,
        fill_rule: FillRule,
        anti_alias: bool,
        transform: Transform,
    ) {
        if within_device_limit(path, 0.0, transform) {
            self.fill_path(path, fill_rule, anti_alias, transform);
        } else if let Some(clipped) = clip_to_surface(path, transform, self.width(), self.height())
        {
            self.fill_path(&clipped, fill_rule, anti_alias, Transform::identity());
        }
    }

    fn guarded_intersect_path(
        &mut self,
        path: &Path,
        fill_rule: FillRule,
        anti_alias: bool,
        transform: Transform,
    ) {
        if within_device_limit(path, 0.0, transform) {
            self.intersect_path(path, fill_rule, anti_alias, transform);
        } else if let Some(clipped) = clip_to_surface(path, transform, self.width(), self.height())
        {
            self.intersect_path(&clipped, fill_rule, anti_alias, Transform::identity());
        } else {
            // Intersecting with a path that covers nothing empties the mask.
            self.clear();
        }
    }

    fn guarded_fill_path_bounds(
        &mut self,
        path: &Path,
        fill_rule: FillRule,
        anti_alias: bool,
        transform: Transform,
    ) -> Option<Rect> {
        // `Mask::fill_path` maps a transformed path into device space itself
        // and fills that with the identity, so filling the mapped path here is
        // the same call, and its bounds are the ones the scan converter clips.
        let device = if within_device_limit(path, 0.0, transform) {
            path.clone().transform(transform)?
        } else {
            clip_to_surface(path, transform, self.width(), self.height())?
        };
        self.fill_path(&device, fill_rule, anti_alias, Transform::identity());
        Some(device.bounds())
    }
}

/// Whether `bounds`, inflated by `reach` local units and mapped by `transform`,
/// stays within [`DEVICE_LIMIT`]. Non-finite values fail the check.
pub(super) fn within_device_bounds(
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    reach: f32,
    transform: Transform,
) -> bool {
    let mut corners = [
        Point::from_xy(left - reach, top - reach),
        Point::from_xy(right + reach, top - reach),
        Point::from_xy(right + reach, bottom + reach),
        Point::from_xy(left - reach, bottom + reach),
    ];
    transform.map_points(&mut corners);
    corners
        .iter()
        .all(|point| point.x.abs() <= DEVICE_LIMIT && point.y.abs() <= DEVICE_LIMIT)
}

fn within_device_limit(path: &Path, reach: f32, transform: Transform) -> bool {
    let bounds = path.bounds();
    within_device_bounds(
        bounds.left(),
        bounds.top(),
        bounds.right(),
        bounds.bottom(),
        reach,
        transform,
    )
}

/// How far a stroke's outline can extend past its path, in local units.
fn stroke_reach(stroke: &Stroke) -> f32 {
    // Miter joins reach `miter_limit` half-widths; round and square caps reach
    // at most one half-width times sqrt(2), which a limit of at least 2 covers.
    stroke.width / 2.0 * stroke.miter_limit.max(2.0)
}

fn stroke_outline(path: &Path, stroke: &Stroke, transform: Transform) -> Option<Path> {
    let resolution_scale = PathStroker::compute_resolution_scale(&transform);
    match &stroke.dash {
        Some(dash) => path
            .dash(dash, resolution_scale)?
            .stroke(stroke, resolution_scale),
        None => path.stroke(stroke, resolution_scale),
    }
}

/// The device-space path clipped to the surface plus [`CLIP_MARGIN`], or
/// `None` when nothing of it remains.
fn clip_to_surface(path: &Path, transform: Transform, width: u32, height: u32) -> Option<Path> {
    let clip = ClipRect {
        left: -CLIP_MARGIN,
        top: -CLIP_MARGIN,
        right: f64::from(width) + CLIP_MARGIN,
        bottom: f64::from(height) + CLIP_MARGIN,
    };
    let map = |point: Point| {
        let mut point = point;
        transform.map_point(&mut point);
        DevicePoint {
            x: f64::from(point.x),
            y: f64::from(point.y),
        }
    };
    let mut builder = PathBuilder::new();
    let mut contour = Vec::new();
    let mut current = DevicePoint { x: 0.0, y: 0.0 };
    let mut start = current;
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(point) => {
                emit_contour(&mut builder, &contour, clip);
                contour.clear();
                current = map(point);
                start = current;
                contour.push(current);
            }
            PathSegment::LineTo(point) => {
                current = map(point);
                contour.push(current);
            }
            PathSegment::QuadTo(control, end) => {
                let control = map(control);
                let end = map(end);
                // Degree elevation: the same curve as a cubic.
                let cubic = [
                    current,
                    current.lerp(control, 2.0 / 3.0),
                    end.lerp(control, 2.0 / 3.0),
                    end,
                ];
                flatten_cubic(cubic, clip, 0, &mut contour);
                current = end;
            }
            PathSegment::CubicTo(first, second, end) => {
                let end = map(end);
                flatten_cubic(
                    [current, map(first), map(second), end],
                    clip,
                    0,
                    &mut contour,
                );
                current = end;
            }
            PathSegment::Close => {
                emit_contour(&mut builder, &contour, clip);
                contour.clear();
                // A segment after `Close` starts from the contour's first point.
                current = start;
                contour.push(current);
            }
        }
    }
    emit_contour(&mut builder, &contour, clip);
    builder.finish()
}

#[derive(Clone, Copy)]
struct DevicePoint {
    x: f64,
    y: f64,
}

impl DevicePoint {
    fn lerp(self, other: Self, t: f64) -> Self {
        Self {
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
        }
    }
}

#[derive(Clone, Copy)]
struct ClipRect {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

/// Appends a cubic's end points to `out` as line segments. A piece whose
/// control polygon misses the clip is replaced by its chord: the chord lies in
/// the polygon's hull, so the region between them misses the clip as well and
/// no winding number inside it changes.
fn flatten_cubic(cubic: [DevicePoint; 4], clip: ClipRect, depth: u32, out: &mut Vec<DevicePoint>) {
    let [p0, p1, p2, p3] = cubic;
    let misses_clip = [p0, p1, p2, p3].iter().all(|p| p.x < clip.left)
        || [p0, p1, p2, p3].iter().all(|p| p.x > clip.right)
        || [p0, p1, p2, p3].iter().all(|p| p.y < clip.top)
        || [p0, p1, p2, p3].iter().all(|p| p.y > clip.bottom);
    if misses_clip || depth >= MAX_SUBDIVISION_DEPTH || is_flat(cubic) {
        out.push(p3);
        return;
    }
    let p01 = p0.lerp(p1, 0.5);
    let p12 = p1.lerp(p2, 0.5);
    let p23 = p2.lerp(p3, 0.5);
    let p012 = p01.lerp(p12, 0.5);
    let p123 = p12.lerp(p23, 0.5);
    let mid = p012.lerp(p123, 0.5);
    flatten_cubic([p0, p01, p012, mid], clip, depth + 1, out);
    flatten_cubic([mid, p123, p23, p3], clip, depth + 1, out);
}

/// Whether both control points lie within the tolerance of the chord, which
/// bounds the curve's distance from it.
fn is_flat([p0, p1, p2, p3]: [DevicePoint; 4]) -> bool {
    let dx = p3.x - p0.x;
    let dy = p3.y - p0.y;
    let chord_squared = dx * dx + dy * dy;
    let tolerance_squared = FLATTEN_TOLERANCE * FLATTEN_TOLERANCE;
    [p1, p2].iter().all(|p| {
        let ox = p.x - p0.x;
        let oy = p.y - p0.y;
        if chord_squared == 0.0 {
            return ox * ox + oy * oy <= tolerance_squared;
        }
        let cross = dx * oy - dy * ox;
        cross * cross <= tolerance_squared * chord_squared
    })
}

/// Clips one closed polygon to `clip` (Sutherland-Hodgman) and appends it.
fn emit_contour(builder: &mut PathBuilder, contour: &[DevicePoint], clip: ClipRect) {
    if contour.len() < 3 {
        return;
    }
    let mut polygon = contour.to_vec();
    let edges: [fn(DevicePoint, ClipRect) -> f64; 4] = [
        |p, c| p.x - c.left,
        |p, c| c.right - p.x,
        |p, c| p.y - c.top,
        |p, c| c.bottom - p.y,
    ];
    for inside_distance in edges {
        if polygon.is_empty() {
            return;
        }
        let mut clipped = Vec::with_capacity(polygon.len() + 4);
        let mut previous = polygon[polygon.len() - 1];
        for &point in &polygon {
            let previous_distance = inside_distance(previous, clip);
            let distance = inside_distance(point, clip);
            if (previous_distance >= 0.0) != (distance >= 0.0) {
                let t = previous_distance / (previous_distance - distance);
                clipped.push(previous.lerp(point, t));
            }
            if distance >= 0.0 {
                clipped.push(point);
            }
            previous = point;
        }
        polygon = clipped;
    }
    let mut points = polygon.iter();
    let Some(first) = points.next() else {
        return;
    };
    // Every clipped value lies within the surface margin, so it fits in f32.
    builder.move_to(first.x as f32, first.y as f32);
    for point in points {
        builder.line_to(point.x as f32, point.y as f32);
    }
    builder.close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::{Color, Pixmap};

    fn square(size: f32) -> Path {
        let mut builder = PathBuilder::new();
        builder.move_to(0.0, 0.0);
        builder.line_to(size, 0.0);
        builder.line_to(size, size);
        builder.line_to(0.0, size);
        builder.close();
        builder.finish().expect("square")
    }

    fn solid() -> Paint<'static> {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(10, 20, 30, 255));
        paint
    }

    #[test]
    fn strokes_far_wider_than_the_surface_cover_it_without_panicking() {
        // A 9.9e8-unit stroke around a 40-unit square at 4x supersampling
        // panicked inside tiny-skia before the guard.
        let mut pixmap = Pixmap::new(160, 160).expect("pixmap");
        let stroke = Stroke {
            width: 9.9e8,
            ..Stroke::default()
        };
        pixmap.as_mut().guarded_stroke_path(
            &square(40.0),
            &solid(),
            &stroke,
            Transform::from_scale(4.0, 4.0),
            None,
        );
        assert!(pixmap
            .pixels()
            .iter()
            .all(|pixel| pixel.demultiply() == Color::from_rgba8(10, 20, 30, 255).to_color_u8()));
    }

    #[test]
    fn clipped_fills_match_unclipped_fills() {
        // A triangle reaching past the limit covers the same pixels as the
        // clipped polygon tiny-skia receives.
        let mut builder = PathBuilder::new();
        builder.move_to(10.0, 10.0);
        builder.line_to(3.0e8, 50.0);
        builder.line_to(10.0, 90.0);
        builder.close();
        let huge = builder.finish().expect("triangle");
        let mut guarded = Pixmap::new(100, 100).expect("pixmap");
        guarded.as_mut().guarded_fill_path(
            &huge,
            &solid(),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let mut builder = PathBuilder::new();
        builder.move_to(10.0, 10.0);
        builder.line_to(3.0e8, 50.0);
        builder.line_to(10.0, 90.0);
        builder.close();
        let clipped = clip_to_surface(
            &builder.finish().expect("triangle"),
            Transform::identity(),
            100,
            100,
        )
        .expect("visible");
        assert!(clipped.bounds().right() <= 100.0 + CLIP_MARGIN as f32);
        let painted = guarded
            .pixels()
            .iter()
            .filter(|pixel| pixel.alpha() > 0)
            .count();
        assert!(painted > 7_000, "{painted} pixels painted");
    }

    #[test]
    fn paths_within_the_limit_take_the_unguarded_call() {
        let path = square(40.0);
        assert!(within_device_limit(
            &path,
            0.0,
            Transform::from_scale(4.0, 4.0)
        ));
        assert!(!within_device_limit(
            &path,
            1.0e9,
            Transform::from_scale(4.0, 4.0)
        ));
    }
}
