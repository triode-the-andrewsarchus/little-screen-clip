//! Resolve the ordered, flattened artwork stream into filled regions.

use super::{CHORD_MM, GRID_MM, intersections};
use crate::eda::{check_cancelled, gerber::GerberDocument};
use i_overlay::{
    core::{
        fill_rule::FillRule,
        overlay::{Overlay, ShapeType},
        overlay_rule::OverlayRule,
    },
    i_float::int::point::IntPoint,
    i_shape::int::shape::IntShapes,
    mesh::{
        outline::offset::OutlineOffset,
        style::{LineJoin, OutlineStyle},
    },
};
use lib_gerber_edit::flatten::{
    Affine2, ApertureImage, ExpandedObject, FlatLayer, Path, Point, Polarity, Segment, Shape,
    SpecFeature,
};
use lib_gerber_edit::gerber_types::Aperture;
use std::{f64::consts::TAU, sync::atomic::AtomicBool};

pub(super) type Coverage = IntShapes;
type Contour = Vec<IntPoint>;

const MAX_GRID_COORD: f64 = 100_000_000.0;
const MAX_POINTS: usize = 500_000;
const MAX_TOTAL_POINTS: usize = 3_000_000;
const MAX_OFFSET_POINTS: usize = 120_000;
const CANCELLED: &str = "Artwork comparison cancelled";

/// Apply runs of same-polarity objects in source order. Within a run,
/// nonzero winding unions the filled objects while retaining their holes.
pub(super) fn compose(doc: &GerberDocument, budget: &mut Budget) -> Result<Coverage, String> {
    if let Some(issue) = doc.issues.first() {
        return Err(format!("Incomplete artwork geometry: {issue}"));
    }
    if doc.flat.features.contains_key(&SpecFeature::ApertureBlock) {
        return Err("Geometric comparison does not support aperture blocks".into());
    }
    if doc.flat.negative {
        return Err(
            "Legacy IPNEG artwork needs a finite image boundary for geometric comparison".into(),
        );
    }
    if doc.aperture_chord_mm > CHORD_MM {
        return Err("Artwork comparison requires comparison-precision aperture geometry".into());
    }

    let mut covered = Coverage::new();
    let mut run = Coverage::new();
    let mut polarity = Polarity::Dark;
    for (index, object) in doc.flat.iter_expanded().enumerate() {
        if index.is_multiple_of(128) {
            budget.check()?;
        }
        if object.polarity != polarity {
            covered = apply_run(&covered, &run, polarity, budget)?;
            run.clear();
            polarity = object.polarity;
        }
        append_object(&doc.flat, object, &mut run, budget)?;
    }
    apply_run(&covered, &run, polarity, budget)
}

fn apply_run(
    covered: &Coverage,
    run: &Coverage,
    polarity: Polarity,
    budget: &mut Budget,
) -> Result<Coverage, String> {
    let rule = match polarity {
        Polarity::Dark => OverlayRule::Union,
        Polarity::Clear => OverlayRule::Difference,
    };
    budget.overlay(covered, run, rule)
}

fn append_object(
    layer: &FlatLayer,
    object: ExpandedObject<'_>,
    run: &mut Coverage,
    budget: &mut Budget,
) -> Result<(), String> {
    let aperture = || {
        object
            .object
            .aperture
            .and_then(|code| layer.apertures.get(&code))
            .ok_or_else(|| "Missing artwork aperture geometry".to_owned())
    };
    let xf = object.transform;
    match &object.object.shape {
        Shape::Flash => {
            let image = aperture()?;
            budget.add_source_points(image.shapes.iter().flatten().map(Vec::len).sum())?;
            for shape in &image.shapes {
                let shape = shape
                    .iter()
                    .enumerate()
                    .map(|(i, contour)| snap_contour(contour.iter().map(|&p| xf.apply(p)), i == 0))
                    .collect::<Result<Vec<_>, _>>()?;
                if shape.first().is_some_and(|outer| outer.len() >= 3) {
                    run.push(shape.into_iter().filter(|c| c.len() >= 3).collect());
                }
            }
        }
        Shape::Path(path) => append_stroke(path, aperture()?, xf, run, budget)?,
        Shape::Region { contours } => {
            for contour in contours {
                let points = region_points(contour, budget)?;
                let contour = snap_contour(points.into_iter().map(|p| xf.apply(p)), true)?;
                if contour.len() >= 3 {
                    run.push(vec![contour]);
                }
            }
        }
        _ => return Err("Unsupported artwork object geometry".into()),
    }
    Ok(())
}

fn append_stroke(
    path: &Path,
    aperture: &ApertureImage,
    xf: Affine2,
    run: &mut Coverage,
    budget: &mut Budget,
) -> Result<(), String> {
    let [polygons] = aperture.shapes.as_slice() else {
        return Err("Geometric strokes require one solid convex aperture".into());
    };
    let [outline] = polygons.as_slice() else {
        return Err("Geometric strokes with aperture holes are unsupported".into());
    };
    if matches!(aperture.aperture, Aperture::Macro(..)) {
        return Err("Geometric strokes with macro apertures are unsupported".into());
    }
    let points = flatten_path(path, budget)?;
    budget.add_source_points(outline.len().saturating_mul(points.len().saturating_mul(2)))?;
    // Sweeping a convex aperture over one line is the convex hull of the
    // aperture at its two endpoints. All chords in the stroke are united.
    let offset = |p: Point| -> Result<Contour, String> {
        outline
            .iter()
            .map(|q| snap(xf.apply([p[0] + q[0], p[1] + q[1]])))
            .collect()
    };
    let mut first = offset(points[0])?;
    for (i, &p) in points[1..].iter().enumerate() {
        if i.is_multiple_of(128) {
            budget.check()?;
        }
        let next = offset(p)?;
        first.extend_from_slice(&next);
        let hull = convex_hull(first);
        if hull.len() >= 3 {
            run.push(vec![hull]);
        }
        first = next;
    }
    if points.len() == 1 && first.len() >= 3 {
        orient(&mut first, true);
        run.push(vec![first]);
    }
    Ok(())
}

/// Preflight the dependency's chord count before it allocates the path.
fn flatten_path(path: &Path, budget: &mut Budget) -> Result<Vec<Point>, String> {
    let mut from = path.start;
    let mut vertices = 1;
    for (index, segment) in path.segments.iter().enumerate() {
        if index.is_multiple_of(128) {
            budget.check()?;
        }
        let count = match *segment {
            Segment::Line { .. } => 1,
            Segment::Arc { to, center, ccw } => arc_vertices(from, to, center, ccw)?,
            _ => return Err("Unsupported artwork path segment".into()),
        };
        vertices += count;
        if vertices > MAX_POINTS {
            return Err(limit());
        }
        from = segment.end();
    }
    budget.add_source_points(vertices)?;
    Ok(path.to_polyline(CHORD_MM))
}

fn region_points(path: &Path, budget: &mut Budget) -> Result<Vec<Point>, String> {
    if let Some((center, radius)) = circle_region(path, budget)? {
        // Use the same world-angle phase and segment count as a circular flash.
        let count = circle_vertices(radius)?;
        budget.add_source_points(count)?;
        return Ok((0..count)
            .map(|i| {
                let angle = TAU * i as f64 / count as f64;
                [
                    center[0] + radius * angle.cos(),
                    center[1] + radius * angle.sin(),
                ]
            })
            .collect());
    }
    flatten_path(path, budget)
}

fn circle_region(path: &Path, budget: &Budget) -> Result<Option<(Point, f64)>, String> {
    let Some(Segment::Arc { center, ccw, .. }) = path.segments.first() else {
        return Ok(None);
    };
    // This epsilon only absorbs floating-point conversion noise. It is
    // four orders of magnitude smaller than the comparison grid.
    const EPSILON_MM: f64 = 1e-8;
    if distance(path.start, path.end()) > EPSILON_MM {
        return Ok(None);
    }
    let radius = distance(path.start, *center);
    let mut from = path.start;
    let mut sweep = 0.0;
    for (index, segment) in path.segments.iter().enumerate() {
        if index.is_multiple_of(128) {
            budget.check()?;
        }
        let Segment::Arc {
            to,
            center: next,
            ccw: direction,
        } = *segment
        else {
            return Ok(None);
        };
        if direction != *ccw
            || distance(*center, next) > EPSILON_MM
            || (distance(to, *center) - radius).abs() > EPSILON_MM
        {
            return Ok(None);
        }
        sweep += sweep_angle(from, to, *center, *ccw);
        from = to;
    }
    Ok(((sweep - TAU).abs() < 1e-9).then_some((*center, radius)))
}

fn distance(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn arc_vertices(from: Point, to: Point, center: Point, ccw: bool) -> Result<usize, String> {
    let full = circle_vertices(distance(from, center))?;
    Ok((sweep_angle(from, to, center, ccw) / TAU * full as f64)
        .ceil()
        .max(1.0) as usize)
}

fn sweep_angle(from: Point, to: Point, center: Point, ccw: bool) -> f64 {
    let angle = |p: Point| (p[1] - center[1]).atan2(p[0] - center[0]);
    let delta = angle(to) - angle(from);
    let sweep = (if ccw { delta } else { -delta }).rem_euclid(TAU);
    if sweep == 0.0 { TAU } else { sweep }
}

fn circle_vertices(radius: f64) -> Result<usize, String> {
    if !radius.is_finite() {
        return Err("Invalid artwork arc".into());
    }
    let full = if radius <= CHORD_MM {
        8.0
    } else {
        (TAU / (2.0 * (1.0 - CHORD_MM / radius).acos()))
            .ceil()
            .max(8.0)
    };
    if full > 4096.0 {
        return Err("Artwork arc exceeds the comparison chord budget".into());
    }
    Ok(full as usize)
}

fn snap(point: Point) -> Result<IntPoint, String> {
    let [x, y] = point.map(|value| (value / GRID_MM).round());
    if x.abs() <= MAX_GRID_COORD && y.abs() <= MAX_GRID_COORD {
        Ok(IntPoint::new(x as i32, y as i32))
    } else {
        Err("Artwork coordinates exceed the geometric comparison range".into())
    }
}

fn snap_contour(points: impl Iterator<Item = Point>, outer: bool) -> Result<Contour, String> {
    let mut contour = points.map(snap).collect::<Result<Contour, _>>()?;
    contour.dedup();
    if contour.first() == contour.last() {
        contour.pop();
    }
    orient(&mut contour, outer);
    Ok(contour)
}

fn orient(contour: &mut [IntPoint], outer: bool) {
    if (double_area(contour) < 0) == outer {
        contour.reverse();
    }
}

fn double_area(contour: &[IntPoint]) -> i128 {
    contour
        .iter()
        .zip(contour.iter().cycle().skip(1))
        .map(|(a, b)| i128::from(a.x) * i128::from(b.y) - i128::from(a.y) * i128::from(b.x))
        .sum()
}

pub(super) fn area_mm2(coverage: &Coverage) -> f64 {
    let area: i128 = coverage.iter().flatten().map(|c| double_area(c)).sum();
    area.abs() as f64 * GRID_MM * GRID_MM * 0.5
}

fn convex_hull(mut points: Contour) -> Contour {
    points.sort_unstable_by_key(|p| (p.x, p.y));
    points.dedup();
    if points.len() < 3 {
        return points;
    }
    fn half(points: impl Iterator<Item = IntPoint>) -> Contour {
        let mut stack: Contour = Vec::new();
        for p in points {
            while let [.., a, b] = stack.as_slice() {
                let [dx, dy] = [i64::from(b.x - a.x), i64::from(b.y - a.y)];
                let cross = dx * i64::from(p.y - a.y) - dy * i64::from(p.x - a.x);
                if cross > 0 {
                    break;
                }
                stack.pop();
            }
            stack.push(p);
        }
        stack.pop();
        stack
    }
    let mut hull = half(points.iter().copied());
    hull.extend(half(points.into_iter().rev()));
    hull
}

pub(super) fn mm(point: IntPoint) -> Point {
    [f64::from(point.x) * GRID_MM, f64::from(point.y) * GRID_MM]
}

pub(super) struct Budget<'a> {
    source_points: usize,
    boolean_points: usize,
    cancel: &'a AtomicBool,
}

impl<'a> Budget<'a> {
    pub(super) fn new(cancel: &'a AtomicBool) -> Self {
        Self {
            source_points: 0,
            boolean_points: 0,
            cancel,
        }
    }

    fn check(&self) -> Result<(), String> {
        check_cancelled(self.cancel, CANCELLED)
    }

    fn add_source_points(&mut self, count: usize) -> Result<(), String> {
        self.source_points = self.source_points.saturating_add(count);
        if self.source_points > MAX_POINTS {
            return Err(limit());
        }
        Ok(())
    }

    pub(super) fn overlay(
        &mut self,
        a: &Coverage,
        b: &Coverage,
        rule: OverlayRule,
    ) -> Result<Coverage, String> {
        self.check()?;
        let count = point_count(a) + point_count(b);
        self.spend(count)?;
        intersections::check(a, b, count, 0, self.cancel)?;
        let mut overlay = Overlay::new(count);
        overlay.add_shapes(a, ShapeType::Subject);
        overlay.add_shapes(b, ShapeType::Clip);
        let result = overlay.overlay(rule, FillRule::NonZero);
        self.check()?;
        if point_count(&result) > MAX_POINTS {
            return Err(limit());
        }
        Ok(result)
    }

    pub(super) fn dilate(&mut self, shape: &Coverage, distance: f64) -> Result<Coverage, String> {
        self.check()?;
        let count = point_count(shape);
        if count > MAX_OFFSET_POINTS {
            return Err("Artwork exceeds the geometric edge-tolerance budget. Try 0 µm.".into());
        }
        self.spend(count)?;
        intersections::check(
            shape,
            &Coverage::new(),
            count,
            (distance / GRID_MM).ceil() as i32,
            self.cancel,
        )?;
        let paths: Vec<Vec<Vec<Point>>> = shape
            .iter()
            .map(|polygon| {
                polygon
                    .iter()
                    .map(|contour| contour.iter().copied().map(mm).collect())
                    .collect()
            })
            .collect();
        let angle = 2.0 * (1.0 - (CHORD_MM / distance).min(1.0)).acos();
        let style = OutlineStyle::new(distance).line_join(LineJoin::Round(angle));
        let result = paths
            .outline_fixed_scale(&style, GRID_MM.recip())
            .map_err(|error| format!("Artwork edge tolerance: {error:?}"))?;
        self.check()?;
        let mut coverage = Coverage::new();
        let count: usize = result.iter().flatten().map(Vec::len).sum();
        self.spend(count)?;
        for polygon in result {
            let polygon = polygon
                .into_iter()
                .enumerate()
                .map(|(index, contour)| snap_contour(contour.into_iter(), index == 0))
                .collect::<Result<Vec<_>, _>>()?;
            coverage.push(polygon);
        }
        Ok(coverage)
    }

    fn spend(&mut self, count: usize) -> Result<(), String> {
        self.boolean_points += count;
        if count > MAX_POINTS || self.boolean_points > MAX_TOTAL_POINTS {
            return Err(limit());
        }
        Ok(())
    }
}

fn point_count(shapes: &Coverage) -> usize {
    shapes.iter().flatten().map(Vec::len).sum()
}

fn limit() -> String {
    "Artwork exceeds the geometric comparison budget".into()
}
