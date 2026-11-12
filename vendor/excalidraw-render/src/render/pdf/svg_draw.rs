//! Nested SVG on a Krilla surface (ADR-0021): a focused port of krilla-svg
//! 0.8.1 for the trees the PDF backend draws. The dependency itself is not
//! used: it enables `tiny-skia`'s and `resvg`'s default features, which would
//! reach the renderer's own rasterizer through feature unification and move
//! PNG bytes, and it decodes images through Krilla's image features, whose
//! JPEG decoder would unify the same way.
//!
//! The trees are sanitized and lowered, so the port draws less than
//! krilla-svg: a filter never reaches it (the raster closures replaced every
//! one, ADR-0009), text is drawn as the outlines usvg flattened it to, and an
//! image is a normalized RGBA8 PNG or another nested SVG. It differs in two
//! places. A `repeat` or `reflect` gradient is drawn as a `pad` gradient over
//! every period the path reaches: Krilla writes other spreads as function
//! shadings sampled once per period by PDF.js. And a radial gradient keeps its
//! focal radius.
//!
//! Every push is popped before an error propagates, so the surface stays
//! balanced for Krilla's drop-time assertions.

use krilla::blend::BlendMode;
use krilla::color::rgb;
use krilla::geom::{Path, PathBuilder, Rect, Size, Transform};
use krilla::image::{BitsPerComponent, CustomImage, Image, ImageColorspace};
use krilla::mask::{Mask, MaskType};
use krilla::num::NormalizedF32;
use krilla::paint::{
    Fill, FillRule, LineCap, LineJoin, LinearGradient, Paint, Pattern, RadialGradient,
    SpreadMethod, Stop, Stroke, StrokeDash,
};
use krilla::surface::Surface;
use std::sync::Arc;
use usvg::tiny_skia_path::PathSegment;

use super::super::FormatEncodeError;

/// The most stops a padded spread gradient may carry. A gradient that would
/// need more keeps its spread and Krilla's function shading.
const MAX_PADDED_STOPS: usize = 4_096;

fn invalid(message: &str) -> FormatEncodeError {
    FormatEncodeError::Encoding(message.to_owned())
}

/// Draws `tree` at its own size, clipped to it.
pub(super) fn draw_tree(
    surface: &mut Surface<'_>,
    tree: &usvg::Tree,
) -> Result<(), FormatEncodeError> {
    let Some(clip) = Rect::from_xywh(0.0, 0.0, tree.size().width(), tree.size().height()) else {
        return Ok(());
    };
    surface.push_clip_path(&rect_path(clip)?, &FillRule::NonZero);
    let result = draw_group(surface, tree.root());
    surface.pop();
    result
}

fn draw_group(surface: &mut Surface<'_>, group: &usvg::Group) -> Result<(), FormatEncodeError> {
    if !group.filters().is_empty() {
        return Err(invalid(
            "a nested-SVG filter reached the PDF writer unlowered",
        ));
    }
    let mut pushes = 0_usize;
    let result = (|| {
        surface.push_transform(&transform(group.transform()));
        pushes += 1;
        if let Some(clip_path) = group.clip_path() {
            pushes += push_clip_path(surface, clip_path)?;
        }
        if let Some(mask) = group.mask() {
            push_mask(surface, mask)?;
            pushes += 1;
        }
        surface.push_blend_mode(blend_mode(group.blend_mode()));
        pushes += 1;
        surface.push_opacity(normalized(group.opacity().get())?);
        pushes += 1;
        // As krilla-svg: a clip path alone does not isolate, which would cost
        // a transparency group per clip without changing what is painted.
        if group.isolate()
            || group.opacity() != usvg::Opacity::ONE
            || group.mask().is_some()
            || blend_mode(group.blend_mode()) != BlendMode::Normal
        {
            surface.push_isolated();
            pushes += 1;
        }
        for child in group.children() {
            draw_node(surface, child)?;
        }
        Ok(())
    })();
    for _ in 0..pushes {
        surface.pop();
    }
    result
}

fn draw_node(surface: &mut Surface<'_>, node: &usvg::Node) -> Result<(), FormatEncodeError> {
    match node {
        usvg::Node::Group(group) => draw_group(surface, group),
        usvg::Node::Path(path) => draw_path(surface, path),
        usvg::Node::Image(image) => draw_image(surface, image),
        usvg::Node::Text(text) => draw_group(surface, text.flattened()),
    }
}

fn draw_path(surface: &mut Surface<'_>, path: &usvg::Path) -> Result<(), FormatEncodeError> {
    if !path.is_visible() {
        return Ok(());
    }
    let Some(shape) = krilla_path(path.data()) else {
        return Ok(());
    };
    let bounds = path.data().bounds();
    let fill = |surface: &mut Surface<'_>| -> Result<Option<Fill>, FormatEncodeError> {
        path.fill()
            .map(|fill| {
                Ok(Fill {
                    paint: paint(surface, fill.paint(), bounds, 0.0)?,
                    opacity: normalized(fill.opacity().get())?,
                    rule: fill_rule(fill.rule()),
                })
            })
            .transpose()
    };
    let stroke = |surface: &mut Surface<'_>| -> Result<Option<Stroke>, FormatEncodeError> {
        path.stroke()
            .map(|stroke| {
                Ok(Stroke {
                    paint: paint(surface, stroke.paint(), bounds, stroke.width().get())?,
                    width: stroke.width().get(),
                    miter_limit: stroke.miterlimit().get(),
                    line_cap: match stroke.linecap() {
                        usvg::LineCap::Butt => LineCap::Butt,
                        usvg::LineCap::Round => LineCap::Round,
                        usvg::LineCap::Square => LineCap::Square,
                    },
                    line_join: match stroke.linejoin() {
                        usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => LineJoin::Miter,
                        usvg::LineJoin::Round => LineJoin::Round,
                        usvg::LineJoin::Bevel => LineJoin::Bevel,
                    },
                    opacity: normalized(stroke.opacity().get())?,
                    dash: stroke.dasharray().map(|array| StrokeDash {
                        array: array.to_vec(),
                        offset: stroke.dashoffset(),
                    }),
                })
            })
            .transpose()
    };
    let draw = |surface: &mut Surface<'_>, fill: Option<Fill>, stroke: Option<Stroke>| {
        // Without either Krilla would fill with black.
        if fill.is_some() || stroke.is_some() {
            surface.set_fill(fill);
            surface.set_stroke(stroke);
            surface.draw_path(&shape);
        }
    };
    match path.paint_order() {
        usvg::PaintOrder::FillAndStroke => {
            let (fill, stroke) = (fill(surface)?, stroke(surface)?);
            draw(surface, fill, stroke);
        }
        usvg::PaintOrder::StrokeAndFill => {
            let stroke = stroke(surface)?;
            draw(surface, None, stroke);
            let fill = fill(surface)?;
            draw(surface, fill, None);
        }
    }
    surface.set_fill(None);
    surface.set_stroke(None);
    Ok(())
}

fn paint(
    surface: &mut Surface<'_>,
    paint: &usvg::Paint,
    bounds: usvg::tiny_skia_path::Rect,
    stroke_width: f32,
) -> Result<Paint, FormatEncodeError> {
    Ok(match paint {
        usvg::Paint::Color(color) => rgb::Color::new(color.red, color.green, color.blue).into(),
        usvg::Paint::LinearGradient(gradient) => {
            let stops = stops(gradient.stops())?;
            let (x1, y1, x2, y2) = (gradient.x1(), gradient.y1(), gradient.x2(), gradient.y2());
            let padded = spread_periods(gradient.spread_method(), &stops, || {
                linear_span(gradient, bounds, stroke_width)
            });
            match padded {
                Some((first, last, stops)) => LinearGradient {
                    x1: x1 + first * (x2 - x1),
                    y1: y1 + first * (y2 - y1),
                    x2: x1 + last * (x2 - x1),
                    y2: y1 + last * (y2 - y1),
                    transform: transform(gradient.transform()),
                    spread_method: SpreadMethod::Pad,
                    stops,
                    anti_alias: false,
                },
                None => LinearGradient {
                    x1,
                    y1,
                    x2,
                    y2,
                    transform: transform(gradient.transform()),
                    spread_method: spread_method(gradient.spread_method()),
                    stops,
                    anti_alias: false,
                },
            }
            .into()
        }
        usvg::Paint::RadialGradient(gradient) => {
            let stops = stops(gradient.stops())?;
            let (fx, fy, fr) = (gradient.fx(), gradient.fy(), gradient.fr().get());
            let (cx, cy, r) = (gradient.cx(), gradient.cy(), gradient.r().get());
            let padded = spread_periods(gradient.spread_method(), &stops, || {
                radial_span(gradient, bounds, stroke_width)
            });
            match padded {
                // Radial periods start at the focal circle.
                Some((_, last, stops)) => RadialGradient {
                    fx,
                    fy,
                    fr,
                    cx: fx + last * (cx - fx),
                    cy: fy + last * (cy - fy),
                    cr: fr + last * (r - fr),
                    transform: transform(gradient.transform()),
                    spread_method: SpreadMethod::Pad,
                    stops,
                    anti_alias: false,
                },
                None => RadialGradient {
                    fx,
                    fy,
                    fr,
                    cx,
                    cy,
                    cr: r,
                    transform: transform(gradient.transform()),
                    spread_method: spread_method(gradient.spread_method()),
                    stops,
                    anti_alias: false,
                },
            }
            .into()
        }
        usvg::Paint::Pattern(pattern) => {
            let mut builder = surface.stream_builder();
            let mut content = builder.surface();
            let result = draw_group(&mut content, pattern.root());
            content.finish();
            let stream = builder.finish();
            result?;
            let rect = pattern.rect();
            Pattern {
                stream,
                transform: transform(
                    pattern
                        .transform()
                        .pre_concat(usvg::Transform::from_translate(rect.x(), rect.y())),
                ),
                width: rect.width(),
                height: rect.height(),
            }
            .into()
        }
    })
}

fn stops(stops: &[usvg::Stop]) -> Result<Vec<Stop>, FormatEncodeError> {
    stops
        .iter()
        .map(|stop| {
            Ok(Stop {
                offset: normalized(stop.offset().get())?,
                color: rgb::Color::new(stop.color().red, stop.color().green, stop.color().blue)
                    .into(),
                opacity: normalized(stop.opacity().get())?,
            })
        })
        .collect()
}

/// For a `repeat` or `reflect` gradient whose painted span is known, the
/// first and last period it reaches and its stops repeated once per period,
/// mirrored on odd periods when reflecting.
fn spread_periods(
    method: usvg::SpreadMethod,
    stops: &[Stop],
    span: impl FnOnce() -> Option<(f32, f32)>,
) -> Option<(f32, f32, Vec<Stop>)> {
    if method == usvg::SpreadMethod::Pad || stops.is_empty() {
        return None;
    }
    let (low, high) = span()?;
    // A bound within rounding of a period boundary does not open a period:
    // normalized transforms carry residues such as a 1e-31 translation.
    const SLACK: f32 = 1e-4;
    // Period indices are only stepped exactly below 2^24: past it `first + 1`
    // can round back to `first`, opening no period or never reaching `last`.
    // The comparisons also reject NaN and infinite bounds.
    const EXACT_INTEGERS: f32 = 16_777_216.0;
    let first = (low + SLACK).floor();
    let last = (high - SLACK).ceil().max(first + 1.0);
    if !(first.abs() < EXACT_INTEGERS && last.abs() < EXACT_INTEGERS) {
        return None;
    }
    let periods = last - first;
    if periods * stops.len() as f32 > MAX_PADDED_STOPS as f32 {
        return None;
    }
    // Both bounds are integers within f32's exact range, so `periods` is a
    // whole count of at least one.
    let count = periods as usize;
    let mut padded = Vec::with_capacity(stops.len() * count);
    for index in 0..count {
        let period = first + index as f32;
        let mirrored = method == usvg::SpreadMethod::Reflect && period.rem_euclid(2.0) == 1.0;
        let ordered: Box<dyn Iterator<Item = &Stop>> = if mirrored {
            Box::new(stops.iter().rev())
        } else {
            Box::new(stops.iter())
        };
        for stop in ordered {
            let local = if mirrored {
                1.0 - stop.offset.get()
            } else {
                stop.offset.get()
            };
            padded.push(Stop {
                offset: NormalizedF32::new(((period - first + local) / periods).clamp(0.0, 1.0))?,
                ..stop.clone()
            });
        }
    }
    // Krilla cannot encode a gradient without stops.
    if padded.is_empty() {
        return None;
    }
    Some((first, last, padded))
}

/// The path's bounds, widened by its stroke, in gradient space.
fn gradient_corners(
    gradient_transform: usvg::Transform,
    bounds: usvg::tiny_skia_path::Rect,
    stroke_width: f32,
) -> Option<[(f32, f32); 4]> {
    let inverse = gradient_transform.invert()?;
    let (left, top, right, bottom) = (
        bounds.left() - stroke_width,
        bounds.top() - stroke_width,
        bounds.right() + stroke_width,
        bounds.bottom() + stroke_width,
    );
    Some(
        [(left, top), (right, top), (right, bottom), (left, bottom)].map(|(x, y)| {
            let mut point = usvg::tiny_skia_path::Point::from_xy(x, y);
            inverse.map_point(&mut point);
            (point.x, point.y)
        }),
    )
}

fn linear_span(
    gradient: &usvg::LinearGradient,
    bounds: usvg::tiny_skia_path::Rect,
    stroke_width: f32,
) -> Option<(f32, f32)> {
    let corners = gradient_corners(gradient.transform(), bounds, stroke_width)?;
    let (x1, y1) = (gradient.x1(), gradient.y1());
    let (dx, dy) = (gradient.x2() - x1, gradient.y2() - y1);
    let length = dx * dx + dy * dy;
    if length.is_nan() || length <= 0.0 {
        return None;
    }
    let ts = corners.map(|(x, y)| ((x - x1) * dx + (y - y1) * dy) / length);
    Some((
        ts.iter().copied().fold(f32::INFINITY, f32::min),
        ts.iter().copied().fold(f32::NEG_INFINITY, f32::max),
    ))
}

/// The span of a radial gradient whose focal circle lies inside its end
/// circle, where each point's `t` is bounded by its distance from the focus;
/// a cone-shaped gradient has no finite cover and keeps its spread.
fn radial_span(
    gradient: &usvg::RadialGradient,
    bounds: usvg::tiny_skia_path::Rect,
    stroke_width: f32,
) -> Option<(f32, f32)> {
    let corners = gradient_corners(gradient.transform(), bounds, stroke_width)?;
    let (fx, fy, fr) = (gradient.fx(), gradient.fy(), gradient.fr().get());
    let (dx, dy) = (gradient.cx() - fx, gradient.cy() - fy);
    let growth = gradient.r().get() - fr;
    let offset = (dx * dx + dy * dy).sqrt();
    if growth.is_nan() || offset.is_nan() || growth <= offset {
        return None;
    }
    let reach = corners
        .iter()
        .map(|(x, y)| ((x - fx) * (x - fx) + (y - fy) * (y - fy)).sqrt())
        .fold(0.0, f32::max);
    Some((0.0, (reach + fr) / (growth - offset)))
}

/// Pushes a clip path and returns how many pushes it took: native clip paths
/// when the clip is simple enough for PDF's, else an alpha mask (krilla-svg's
/// rule).
fn push_clip_path(
    surface: &mut Surface<'_>,
    clip_path: &usvg::ClipPath,
) -> Result<usize, FormatEncodeError> {
    let rules = clip_rules(clip_path.root());
    let native = is_simple_clip(clip_path.root())
        && (rules.iter().all(|rule| *rule == usvg::FillRule::NonZero)
            || (rules.len() == 1 && rules[0] == usvg::FillRule::EvenOdd));
    if native {
        let clips = native_clips(
            clip_path,
            rules.first().copied().unwrap_or(usvg::FillRule::NonZero),
        )?;
        let count = clips.len();
        for (path, rule) in clips {
            surface.push_clip_path(&path, &rule);
        }
        return Ok(count);
    }
    let mut builder = surface.stream_builder();
    let mut content = builder.surface();
    let mut pushes = 0_usize;
    let result = (|| {
        if let Some(inner) = clip_path.clip_path() {
            pushes += push_clip_path(&mut content, inner)?;
        }
        content.push_transform(&transform(clip_path.transform()));
        pushes += 1;
        draw_group(&mut content, clip_path.root())
    })();
    for _ in 0..pushes {
        content.pop();
    }
    content.finish();
    let stream = builder.finish();
    result?;
    surface.push_mask(Mask::new(stream, MaskType::Alpha));
    Ok(1)
}

fn native_clips(
    clip_path: &usvg::ClipPath,
    rule: usvg::FillRule,
) -> Result<Vec<(Path, FillRule)>, FormatEncodeError> {
    let mut clips = Vec::new();
    if let Some(inner) = clip_path.clip_path() {
        clips.extend(native_clips(inner, rule)?);
    }
    // A leading move keeps a clip whose children are all hidden a clip that
    // hides everything.
    let mut builder = PathBuilder::new();
    builder.move_to(0.0, 0.0);
    clip_segments(clip_path.root(), clip_path.transform(), &mut builder);
    let path = match builder.finish() {
        Some(path) => path,
        None => {
            let mut empty = PathBuilder::new();
            empty.move_to(0.0, 0.0);
            empty.line_to(0.0, 0.0);
            empty
                .finish()
                .ok_or_else(|| invalid("empty nested-SVG clip has no path"))?
        }
    };
    clips.push((path, fill_rule(rule)));
    Ok(clips)
}

fn clip_segments(group: &usvg::Group, base: usvg::Transform, builder: &mut PathBuilder) {
    for child in group.children() {
        match child {
            usvg::Node::Path(path) if path.is_visible() => {
                for segment in path.data().segments() {
                    match segment {
                        PathSegment::MoveTo(mut point) => {
                            base.map_point(&mut point);
                            builder.move_to(point.x, point.y);
                        }
                        PathSegment::LineTo(mut point) => {
                            base.map_point(&mut point);
                            builder.line_to(point.x, point.y);
                        }
                        PathSegment::QuadTo(control, end) => {
                            let mut points = [control, end];
                            base.map_points(&mut points);
                            builder.quad_to(points[0].x, points[0].y, points[1].x, points[1].y);
                        }
                        PathSegment::CubicTo(first, second, end) => {
                            let mut points = [first, second, end];
                            base.map_points(&mut points);
                            builder.cubic_to(
                                points[0].x,
                                points[0].y,
                                points[1].x,
                                points[1].y,
                                points[2].x,
                                points[2].y,
                            );
                        }
                        PathSegment::Close => builder.close(),
                    }
                }
            }
            usvg::Node::Group(inner) => {
                clip_segments(inner, base.pre_concat(inner.transform()), builder);
            }
            usvg::Node::Text(text) => clip_segments(text.flattened(), base, builder),
            _ => {}
        }
    }
}

fn is_simple_clip(group: &usvg::Group) -> bool {
    group.children().iter().all(|node| match node {
        usvg::Node::Group(inner) => inner.clip_path().is_none() && is_simple_clip(inner),
        _ => true,
    })
}

fn clip_rules(group: &usvg::Group) -> Vec<usvg::FillRule> {
    let mut rules = Vec::new();
    for node in group.children() {
        match node {
            usvg::Node::Path(path) => {
                if let Some(fill) = path.fill() {
                    rules.push(fill.rule());
                }
            }
            usvg::Node::Text(text) => rules.extend(clip_rules(text.flattened())),
            usvg::Node::Group(inner) => rules.extend(clip_rules(inner)),
            usvg::Node::Image(_) => {}
        }
    }
    rules
}

fn push_mask(surface: &mut Surface<'_>, mask: &usvg::Mask) -> Result<(), FormatEncodeError> {
    let mut builder = surface.stream_builder();
    let mut content = builder.surface();
    let mut pushes = 0_usize;
    let result = (|| {
        if let Some(inner) = mask.mask() {
            push_mask(&mut content, inner)?;
            pushes += 1;
        }
        let rect = mask.rect();
        if let Some(clip) = Rect::from_ltrb(rect.left(), rect.top(), rect.right(), rect.bottom()) {
            content.push_clip_path(&rect_path(clip)?, &FillRule::NonZero);
            pushes += 1;
        }
        draw_group(&mut content, mask.root())
    })();
    for _ in 0..pushes {
        content.pop();
    }
    content.finish();
    let stream = builder.finish();
    result?;
    surface.push_mask(Mask::new(
        stream,
        match mask.kind() {
            usvg::MaskType::Luminance => MaskType::Luminosity,
            usvg::MaskType::Alpha => MaskType::Alpha,
        },
    ));
    Ok(())
}

fn draw_image(surface: &mut Surface<'_>, image: &usvg::Image) -> Result<(), FormatEncodeError> {
    if !image.is_visible() {
        return Ok(());
    }
    let Some(size) = Size::from_wh(image.size().width(), image.size().height()) else {
        return Ok(());
    };
    match image.kind() {
        usvg::ImageKind::PNG(data) => {
            surface.draw_image(png_image(data)?, size);
            Ok(())
        }
        usvg::ImageKind::SVG(tree) => draw_tree(surface, tree),
        _ => Err(invalid("a sanitized nested SVG carries a non-PNG raster")),
    }
}

/// One normalized RGBA8 PNG as separate color and alpha planes; an opaque
/// image carries no alpha plane, so it gets no soft mask.
#[derive(Clone, Hash)]
struct Planes {
    rgb: Arc<[u8]>,
    alpha: Option<Arc<[u8]>>,
    width: u32,
    height: u32,
}

impl CustomImage for Planes {
    fn color_channel(&self) -> &[u8] {
        &self.rgb
    }
    fn alpha_channel(&self) -> Option<&[u8]> {
        self.alpha.as_deref()
    }
    fn bits_per_component(&self) -> BitsPerComponent {
        BitsPerComponent::Eight
    }
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    fn icc_profile(&self) -> Option<&[u8]> {
        None
    }
    fn color_space(&self) -> ImageColorspace {
        ImageColorspace::Rgb
    }
}

fn png_image(data: &[u8]) -> Result<Image, FormatEncodeError> {
    let decoder = png::Decoder::new(std::io::Cursor::new(data));
    let mut reader = decoder
        .read_info()
        .map_err(|_| invalid("nested raster is not a normalized PNG"))?;
    let info = reader.info();
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(invalid("nested raster is not normalized RGBA8"));
    }
    let (width, height) = (info.width, info.height);
    let size = reader
        .output_buffer_size()
        .ok_or_else(super::raster::memory)?;
    let mut rgba = Vec::new();
    rgba.try_reserve_exact(size)
        .map_err(|_| super::raster::memory())?;
    rgba.resize(size, 0);
    reader
        .next_frame(&mut rgba)
        .map_err(|_| invalid("nested raster did not decode"))?;
    let pixels = (width as usize) * (height as usize);
    if rgba.len() != pixels * 4 {
        return Err(invalid("nested raster disagrees with its dimensions"));
    }
    let mut rgb = Vec::new();
    let mut alpha = Vec::new();
    rgb.try_reserve_exact(pixels * 3)
        .map_err(|_| super::raster::memory())?;
    alpha
        .try_reserve_exact(pixels)
        .map_err(|_| super::raster::memory())?;
    for pixel in rgba.as_chunks::<4>().0 {
        rgb.extend_from_slice(&pixel[..3]);
        alpha.push(pixel[3]);
    }
    let opaque = alpha.iter().all(|&sample| sample == u8::MAX);
    Image::from_custom(
        Planes {
            rgb: rgb.into(),
            alpha: (!opaque).then(|| alpha.into()),
            width,
            height,
        },
        false,
    )
    .map_err(|error| invalid(&format!("nested raster is not embeddable: {error}")))
}

fn krilla_path(data: &usvg::tiny_skia_path::Path) -> Option<Path> {
    let mut builder = PathBuilder::new();
    for segment in data.segments() {
        match segment {
            PathSegment::MoveTo(point) => builder.move_to(point.x, point.y),
            PathSegment::LineTo(point) => builder.line_to(point.x, point.y),
            PathSegment::QuadTo(control, end) => {
                builder.quad_to(control.x, control.y, end.x, end.y)
            }
            PathSegment::CubicTo(first, second, end) => {
                builder.cubic_to(first.x, first.y, second.x, second.y, end.x, end.y)
            }
            PathSegment::Close => builder.close(),
        }
    }
    builder.finish()
}

fn rect_path(rect: Rect) -> Result<Path, FormatEncodeError> {
    let mut builder = PathBuilder::new();
    builder.move_to(rect.left(), rect.top());
    builder.line_to(rect.right(), rect.top());
    builder.line_to(rect.right(), rect.bottom());
    builder.line_to(rect.left(), rect.bottom());
    builder.close();
    builder
        .finish()
        .ok_or_else(|| invalid("nested-SVG clip rectangle has no path"))
}

fn transform(transform: usvg::Transform) -> Transform {
    Transform::from_row(
        transform.sx,
        transform.ky,
        transform.kx,
        transform.sy,
        transform.tx,
        transform.ty,
    )
}

fn normalized(value: f32) -> Result<NormalizedF32, FormatEncodeError> {
    NormalizedF32::new(value).ok_or_else(|| invalid("nested-SVG opacity is outside zero to one"))
}

fn fill_rule(rule: usvg::FillRule) -> FillRule {
    match rule {
        usvg::FillRule::NonZero => FillRule::NonZero,
        usvg::FillRule::EvenOdd => FillRule::EvenOdd,
    }
}

fn spread_method(method: usvg::SpreadMethod) -> SpreadMethod {
    match method {
        usvg::SpreadMethod::Pad => SpreadMethod::Pad,
        usvg::SpreadMethod::Reflect => SpreadMethod::Reflect,
        usvg::SpreadMethod::Repeat => SpreadMethod::Repeat,
    }
}

fn blend_mode(mode: usvg::BlendMode) -> BlendMode {
    match mode {
        usvg::BlendMode::Normal => BlendMode::Normal,
        usvg::BlendMode::Multiply => BlendMode::Multiply,
        usvg::BlendMode::Screen => BlendMode::Screen,
        usvg::BlendMode::Overlay => BlendMode::Overlay,
        usvg::BlendMode::Darken => BlendMode::Darken,
        usvg::BlendMode::Lighten => BlendMode::Lighten,
        usvg::BlendMode::ColorDodge => BlendMode::ColorDodge,
        usvg::BlendMode::ColorBurn => BlendMode::ColorBurn,
        usvg::BlendMode::HardLight => BlendMode::HardLight,
        usvg::BlendMode::SoftLight => BlendMode::SoftLight,
        usvg::BlendMode::Difference => BlendMode::Difference,
        usvg::BlendMode::Exclusion => BlendMode::Exclusion,
        usvg::BlendMode::Hue => BlendMode::Hue,
        usvg::BlendMode::Saturation => BlendMode::Saturation,
        usvg::BlendMode::Color => BlendMode::Color,
        usvg::BlendMode::Luminosity => BlendMode::Luminosity,
        // PDF has no additive blend mode. usvg before 0.49 parsed
        // `plus-lighter` as `normal`, so this keeps the PDF it drew then.
        usvg::BlendMode::PlusLighter => BlendMode::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(offset: f32, red: u8) -> Stop {
        Stop {
            offset: NormalizedF32::new(offset).unwrap(),
            color: rgb::Color::new(red, 0, 0).into(),
            opacity: NormalizedF32::ONE,
        }
    }

    #[test]
    fn a_repeating_span_pads_every_period_it_reaches() {
        let stops = [stop(0.0, 255), stop(1.0, 0)];
        let (first, last, padded) =
            spread_periods(usvg::SpreadMethod::Repeat, &stops, || Some((0.0, 3.0))).unwrap();
        assert_eq!((first, last), (0.0, 3.0));
        let offsets: Vec<f32> = padded.iter().map(|stop| stop.offset.get()).collect();
        assert_eq!(
            offsets,
            [0.0, 1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0, 1.0]
        );
    }

    #[test]
    fn a_reflecting_span_mirrors_its_odd_periods() {
        let stops = [stop(0.0, 255), stop(1.0, 0)];
        let (_, _, padded) =
            spread_periods(usvg::SpreadMethod::Reflect, &stops, || Some((0.0, 2.0))).unwrap();
        let reds: Vec<Stop> = padded;
        assert_eq!(reds.len(), 4);
        assert_eq!(reds[2].offset.get(), 0.5);
        assert_eq!(reds[3].offset.get(), 1.0);
        assert_eq!(reds[2].color, stop(0.0, 0).color);
        assert_eq!(reds[3].color, stop(0.0, 255).color);
    }

    #[test]
    fn a_rounding_residue_does_not_open_a_period() {
        let stops = [stop(0.0, 255), stop(1.0, 0)];
        let (first, last, _) = spread_periods(usvg::SpreadMethod::Repeat, &stops, || {
            Some((-1e-6, 1.0 + 1e-6))
        })
        .unwrap();
        assert_eq!((first, last), (0.0, 1.0));
    }

    #[test]
    fn pad_gradients_and_unbounded_spans_keep_their_spread() {
        let stops = [stop(0.0, 255), stop(1.0, 0)];
        assert!(spread_periods(usvg::SpreadMethod::Pad, &stops, || Some((0.0, 2.0))).is_none());
        assert!(spread_periods(usvg::SpreadMethod::Repeat, &stops, || None).is_none());
        assert!(spread_periods(usvg::SpreadMethod::Repeat, &stops, || Some((0.0, 1e6))).is_none());
    }

    #[test]
    fn periods_past_exact_f32_integers_keep_their_spread() {
        // Past 2^24 adding one period no longer changes an f32, so a span
        // there cannot be stepped through period by period.
        let stops = [stop(0.0, 255), stop(1.0, 0)];
        for method in [usvg::SpreadMethod::Repeat, usvg::SpreadMethod::Reflect] {
            assert!(spread_periods(method, &stops, || Some((2.0e7, 2.0e7 + 100.0))).is_none());
            assert!(spread_periods(method, &stops, || Some((-2.0e7 - 100.0, -2.0e7))).is_none());
        }
    }

    #[test]
    fn a_span_that_opens_no_period_keeps_its_spread() {
        // There `first + 1` rounds back to `first`: no period, no stops,
        // which Krilla cannot encode.
        let stops = [stop(0.0, 255), stop(1.0, 0)];
        let bound = 33_554_432.0_f32;
        assert!(
            spread_periods(usvg::SpreadMethod::Repeat, &stops, || Some((bound, bound))).is_none()
        );
    }

    #[test]
    fn periods_just_below_the_exact_f32_integers_still_pad() {
        let stops = [stop(0.0, 255), stop(1.0, 0)];
        let low = 16_777_213.0_f32;
        let (first, last, padded) = spread_periods(usvg::SpreadMethod::Reflect, &stops, || {
            Some((low, low + 2.0))
        })
        .unwrap();
        assert_eq!((first, last), (low, low + 2.0));
        assert_eq!(padded.len(), 4);
        // 2^24 - 3 is odd: the first period is mirrored.
        assert_eq!(padded[0].color, stop(0.0, 0).color);
    }
}
