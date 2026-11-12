//! Admission on both sides of the SVG viewer boundary: source doubles before
//! narrowing to f32, then the viewer's expanded paths, transforms and subroots.

use super::super::coordinates::CoordinateRange;
use super::ResolveError;

fn failure(value: f64) -> ResolveError {
    ResolveError::Coordinate {
        value_bits: value.to_bits(),
    }
}

pub(super) fn source(svg: &str, limit: f64) -> Result<(), ResolveError> {
    let document = roxmltree::Document::parse(svg).map_err(|_| ResolveError::SvgDecodeFailed)?;
    let range = CoordinateRange(limit);
    for node in document.descendants().filter(|node| node.is_element()) {
        // Matrix translation composition is checked before a viewer can drop
        // an out-of-f32-range node and thereby hide the crossing from us.
        let mut transform = svgtypes::Transform::default();
        for ancestor in node.ancestors().filter(|node| node.is_element()) {
            if let Some(value) = ancestor.attribute("transform") {
                let parent = parse_transform(value, range)?;
                transform = multiply(parent, transform);
                range.values([transform.e, transform.f]).map_err(failure)?;
            }
        }
        for attribute in node.attributes() {
            if matches!(
                attribute.name(),
                "x" | "y"
                    | "dx"
                    | "dy"
                    | "x1"
                    | "y1"
                    | "x2"
                    | "y2"
                    | "cx"
                    | "cy"
                    | "r"
                    | "rx"
                    | "ry"
                    | "width"
                    | "height"
                    | "stroke-width"
                    | "font-size"
            ) {
                if let Ok(length) = attribute.value().parse::<svgtypes::Length>() {
                    use svgtypes::LengthUnit as U;
                    let factor = match length.unit {
                        U::None | U::Px => Some(1.0),
                        U::In => Some(96.0),
                        U::Cm => Some(96.0 / 2.54),
                        U::Mm => Some(96.0 / 25.4),
                        U::Pt => Some(96.0 / 72.0),
                        U::Pc => Some(16.0),
                        U::Em | U::Ex | U::Percent => None,
                    };
                    if let Some(factor) = factor {
                        range.value(length.number * factor).map_err(failure)?;
                    }
                }
            }
            match attribute.name() {
                "d" => source_path(attribute.value(), transform, range)?,
                "points" => {
                    for (x, y) in svgtypes::PointsParser::from(attribute.value()) {
                        source_point((x, y), transform, range)?;
                    }
                }
                "viewBox" => {
                    if let Ok(view) = attribute.value().parse::<svgtypes::ViewBox>() {
                        range
                            .rect(view.x, view.y, view.w, view.h)
                            .map_err(failure)?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn source_point(
    (x, y): (f64, f64),
    t: svgtypes::Transform,
    range: CoordinateRange,
) -> Result<(), ResolveError> {
    range
        .values([x, y, t.a * x + t.c * y + t.e, t.b * x + t.d * y + t.f])
        .map_err(failure)
}

/// Allocation-free token admission. In particular, do not invoke the
/// simplifying parser: it buffers arc subdivisions before returning a point.
fn source_path(
    value: &str,
    t: svgtypes::Transform,
    range: CoordinateRange,
) -> Result<(), ResolveError> {
    use svgtypes::PathSegment as S;
    let mut current = (0.0, 0.0);
    let mut start = current;
    let mut cubic = None;
    let mut quadratic = None;
    for segment in svgtypes::PathParser::from(value) {
        let Ok(segment) = segment else {
            break;
        };
        let point = |abs: bool, x: f64, y: f64| -> Result<(f64, f64), ResolveError> {
            range.values([x, y]).map_err(failure)?;
            let p = if abs {
                (x, y)
            } else {
                (current.0 + x, current.1 + y)
            };
            source_point(p, t, range)?;
            Ok(p)
        };
        let reflect = |control: Option<(f64, f64)>| -> Result<(f64, f64), ResolveError> {
            let p = control.map_or(current, |p| (2.0 * current.0 - p.0, 2.0 * current.1 - p.1));
            source_point(p, t, range)?;
            Ok(p)
        };
        let (next, c, q) = match segment {
            S::MoveTo { abs, x, y } => {
                start = point(abs, x, y)?;
                (start, None, None)
            }
            S::LineTo { abs, x, y } => (point(abs, x, y)?, None, None),
            S::HorizontalLineTo { abs, x } => (
                point(abs, x, if abs { current.1 } else { 0.0 })?,
                None,
                None,
            ),
            S::VerticalLineTo { abs, y } => (
                point(abs, if abs { current.0 } else { 0.0 }, y)?,
                None,
                None,
            ),
            S::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } => {
                point(abs, x1, y1)?;
                let control = point(abs, x2, y2)?;
                (point(abs, x, y)?, Some(control), None)
            }
            S::SmoothCurveTo { abs, x2, y2, x, y } => {
                reflect(cubic)?;
                let control = point(abs, x2, y2)?;
                (point(abs, x, y)?, Some(control), None)
            }
            S::Quadratic { abs, x1, y1, x, y } => {
                let control = point(abs, x1, y1)?;
                (point(abs, x, y)?, None, Some(control))
            }
            S::SmoothQuadratic { abs, x, y } => {
                let control = reflect(quadratic)?;
                (point(abs, x, y)?, None, Some(control))
            }
            S::EllipticalArc {
                abs,
                rx,
                ry,
                x_axis_rotation,
                x,
                y,
                ..
            } => {
                range.values([rx, ry]).map_err(failure)?;
                let end = point(abs, x, y)?;
                if rx != 0.0 && ry != 0.0 && current != end {
                    let (sin, cos) = crate::portable_math::sin_cos(x_axis_rotation.to_radians());
                    let dx = (current.0 - end.0) / 2.0;
                    let dy = (current.1 - end.1) / 2.0;
                    let x = dx * cos + dy * sin;
                    let y = -dx * sin + dy * cos;
                    // SVG scales undersized radii to join the endpoints. Check
                    // that derived ellipse before the viewer subdivides it.
                    let corrected_rx = crate::portable_math::hypot(x, y * rx.abs() / ry.abs());
                    let corrected_ry = crate::portable_math::hypot(x * ry.abs() / rx.abs(), y);
                    range
                        .values([corrected_rx, corrected_ry])
                        .map_err(failure)?;
                }
                (end, None, None)
            }
            S::ClosePath { .. } => (start, None, None),
        };
        current = next;
        cubic = c;
        quadratic = q;
    }
    Ok(())
}

fn multiply(a: svgtypes::Transform, b: svgtypes::Transform) -> svgtypes::Transform {
    svgtypes::Transform::new(
        a.a * b.a + a.c * b.b,
        a.b * b.a + a.d * b.b,
        a.a * b.c + a.c * b.d,
        a.b * b.c + a.d * b.d,
        a.a * b.e + a.c * b.f + a.e,
        a.b * b.e + a.d * b.f + a.f,
    )
}

fn parse_transform(
    value: &str,
    range: CoordinateRange,
) -> Result<svgtypes::Transform, ResolveError> {
    use svgtypes::{Transform as T, TransformListToken as Token};
    let mut transform = T::default();
    for token in svgtypes::TransformListParser::from(value) {
        let Ok(token) = token else {
            break;
        };
        let next = match token {
            Token::Matrix { a, b, c, d, e, f } => T::new(a, b, c, d, e, f),
            Token::Translate { tx, ty } => T::new(1.0, 0.0, 0.0, 1.0, tx, ty),
            Token::Scale { sx, sy } => T::new(sx, 0.0, 0.0, sy, 0.0, 0.0),
            Token::Rotate { angle } => {
                let (sin, cos) = crate::portable_math::sin_cos(angle.to_radians());
                T::new(cos, sin, -sin, cos, 0.0, 0.0)
            }
            Token::SkewX { angle } => {
                let (sin, cos) = crate::portable_math::sin_cos(angle.to_radians());
                T::new(1.0, 0.0, sin / cos, 1.0, 0.0, 0.0)
            }
            Token::SkewY { angle } => {
                let (sin, cos) = crate::portable_math::sin_cos(angle.to_radians());
                T::new(1.0, sin / cos, 0.0, 1.0, 0.0, 0.0)
            }
        };
        range.values([next.e, next.f]).map_err(failure)?;
        transform = multiply(transform, next);
        range.values([transform.e, transform.f]).map_err(failure)?;
        if [transform.a, transform.b, transform.c, transform.d]
            .iter()
            .any(|v| !v.is_finite())
        {
            return Err(failure(f64::INFINITY));
        }
    }
    Ok(transform)
}

pub(super) fn tree(tree: &usvg::Tree, limit: f64) -> Result<(), ResolveError> {
    let range = CoordinateRange(limit);
    range
        .values([
            f64::from(tree.size().width()),
            f64::from(tree.size().height()),
        ])
        .map_err(failure)?;
    group(tree.root(), range, usvg::Transform::identity())
}

fn group(
    root: &usvg::Group,
    range: CoordinateRange,
    extra: usvg::Transform,
) -> Result<(), ResolveError> {
    let mut pending = Vec::new();
    pending.try_reserve(1).map_err(|_| ResolveError::Memory)?;
    pending.push(root);
    while let Some(parent) = pending.pop() {
        for node in parent.children() {
            let t = extra.pre_concat(node.abs_transform());
            range
                .values([f64::from(t.tx), f64::from(t.ty)])
                .map_err(failure)?;
            for bounds in [
                node.bounding_box(),
                node.abs_bounding_box(),
                node.stroke_bounding_box(),
            ] {
                range
                    .rect(
                        f64::from(bounds.x()),
                        f64::from(bounds.y()),
                        f64::from(bounds.width()),
                        f64::from(bounds.height()),
                    )
                    .map_err(failure)?;
            }
            let bounds = node.bounding_box();
            for (x, y) in [
                (bounds.left(), bounds.top()),
                (bounds.right(), bounds.top()),
                (bounds.right(), bounds.bottom()),
                (bounds.left(), bounds.bottom()),
            ] {
                let x = f64::from(x);
                let y = f64::from(y);
                range
                    .values([
                        f64::from(t.sx) * x + f64::from(t.kx) * y + f64::from(t.tx),
                        f64::from(t.ky) * x + f64::from(t.sy) * y + f64::from(t.ty),
                    ])
                    .map_err(failure)?;
            }
            if let usvg::Node::Path(path) = node {
                for paint in path
                    .fill()
                    .map(|fill| fill.paint())
                    .into_iter()
                    .chain(path.stroke().map(|stroke| stroke.paint()))
                {
                    paint_coordinates(paint, t, range)?;
                }
                for point in path.data().points() {
                    let x = f64::from(point.x);
                    let y = f64::from(point.y);
                    range
                        .values([
                            x,
                            y,
                            f64::from(t.sx) * x + f64::from(t.kx) * y + f64::from(t.tx),
                            f64::from(t.ky) * x + f64::from(t.sy) * y + f64::from(t.ty),
                        ])
                        .map_err(failure)?;
                    let mut mapped = *point;
                    t.map_point(&mut mapped);
                    range
                        .values([f64::from(mapped.x), f64::from(mapped.y)])
                        .map_err(failure)?;
                }
            }
            if let usvg::Node::Group(child) = node {
                for filter in child.filters() {
                    let rect = filter.rect();
                    range
                        .rect(
                            f64::from(rect.x()),
                            f64::from(rect.y()),
                            f64::from(rect.width()),
                            f64::from(rect.height()),
                        )
                        .map_err(failure)?;
                    for primitive in filter.primitives() {
                        let rect = primitive.rect();
                        let (dx, dy) = match primitive.kind() {
                            usvg::filter::Kind::Offset(offset) => {
                                (f64::from(offset.dx()), f64::from(offset.dy()))
                            }
                            usvg::filter::Kind::DropShadow(shadow) => {
                                (f64::from(shadow.dx()), f64::from(shadow.dy()))
                            }
                            _ => (0.0, 0.0),
                        };
                        range.values([dx, dy]).map_err(failure)?;
                        let (x, y, w, h) = (
                            f64::from(rect.x()) + dx,
                            f64::from(rect.y()) + dy,
                            f64::from(rect.width()),
                            f64::from(rect.height()),
                        );
                        range.rect(x, y, w, h).map_err(failure)?;
                        for (x, y) in [(x, y), (x + w, y), (x + w, y + h), (x, y + h)] {
                            range
                                .values([
                                    f64::from(t.sx) * x + f64::from(t.kx) * y + f64::from(t.tx),
                                    f64::from(t.ky) * x + f64::from(t.sy) * y + f64::from(t.ty),
                                ])
                                .map_err(failure)?;
                        }
                    }
                }
                pending.try_reserve(1).map_err(|_| ResolveError::Memory)?;
                pending.push(child);
                let mut clip = child.clip_path();
                while let Some(current) = clip {
                    group(current.root(), range, t.pre_concat(current.transform()))?;
                    clip = current.clip_path();
                }
                let mut mask = child.mask();
                while let Some(current) = mask {
                    let rect = current.rect();
                    range
                        .rect(
                            f64::from(rect.x()),
                            f64::from(rect.y()),
                            f64::from(rect.width()),
                            f64::from(rect.height()),
                        )
                        .map_err(failure)?;
                    group(current.root(), range, t)?;
                    mask = current.mask();
                }
            }
            // The viewer's subroots include clip paths, masks, patterns,
            // flattened text, nested SVG images and feImage subtrees.
            let mut result = Ok(());
            node.subroots(|root| {
                if result.is_ok() {
                    result = group(root, range, usvg::Transform::identity());
                }
            });
            result?;
        }
    }
    Ok(())
}

fn paint_coordinates(
    paint: &usvg::Paint,
    transform: usvg::Transform,
    range: CoordinateRange,
) -> Result<(), ResolveError> {
    let point = |x: f64, y: f64, t: usvg::Transform| {
        range
            .values([
                x,
                y,
                f64::from(t.tx),
                f64::from(t.ty),
                f64::from(t.sx) * x + f64::from(t.kx) * y + f64::from(t.tx),
                f64::from(t.ky) * x + f64::from(t.sy) * y + f64::from(t.ty),
            ])
            .map_err(failure)
    };
    match paint {
        usvg::Paint::Color(_) => {}
        usvg::Paint::Pattern(pattern) => {
            let rect = pattern.rect();
            range
                .rect(
                    f64::from(rect.x()),
                    f64::from(rect.y()),
                    f64::from(rect.width()),
                    f64::from(rect.height()),
                )
                .map_err(failure)?;
            group(
                pattern.root(),
                range,
                transform.pre_concat(pattern.transform()),
            )?;
        }
        usvg::Paint::LinearGradient(gradient) => {
            let t = transform.pre_concat(gradient.transform());
            point(f64::from(gradient.x1()), f64::from(gradient.y1()), t)?;
            point(f64::from(gradient.x2()), f64::from(gradient.y2()), t)?;
        }
        usvg::Paint::RadialGradient(gradient) => {
            let t = transform.pre_concat(gradient.transform());
            let x = f64::from(gradient.cx());
            let y = f64::from(gradient.cy());
            let r = f64::from(gradient.r().get());
            range.value(r).map_err(failure)?;
            point(f64::from(gradient.fx()), f64::from(gradient.fy()), t)?;
            for (x, y) in [
                (x - r, y - r),
                (x + r, y - r),
                (x + r, y + r),
                (x - r, y + r),
            ] {
                point(x, y, t)?;
            }
        }
    }
    Ok(())
}
