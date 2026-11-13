//! TrueType outlines as ttf-parser 0.25 emits them, over read-fonts' `glyf`
//! and `loca`. The renderer measured, rasterized and counted geometry with
//! ttf-parser's pen stream, so this keeps its contour starts, midpoints, the
//! explicit closing segment before every close, its composite transforms in
//! f32, and its control-point bounding box.

use write_fonts::read::tables::glyf::{Anchor, Component, Glyf, Glyph};
use write_fonts::read::tables::loca::Loca;
use write_fonts::read::types::{BigEndian, GlyphId as ReadGlyphId};

/// Receives a glyph outline in font units.
pub(crate) trait OutlineBuilder {
    fn move_to(&mut self, x: f32, y: f32);
    fn line_to(&mut self, x: f32, y: f32);
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32);
    fn close(&mut self);
}

/// An outline's bounding box in font units, over its emitted points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x_min: i16,
    pub(crate) y_min: i16,
    pub(crate) x_max: i16,
    pub(crate) y_max: i16,
}

// ttf-parser's own limit; the format sets none.
const MAX_COMPONENTS: u8 = 32;

/// Outlines `glyph` into `builder`. `None` when the glyph is missing, empty,
/// malformed, or nests components too deeply.
pub(super) fn outline(
    loca: &Loca<'_>,
    glyf: &Glyf<'_>,
    glyph: u16,
    builder: &mut dyn OutlineBuilder,
) -> Option<Rect> {
    let data = loca.get_glyf(ReadGlyphId::from(glyph), glyf).ok()??;
    let mut builder = Builder::new(Transform::default(), Bounds::new(), builder);
    outline_glyph(loca, glyf, &data, 0, &mut builder)?
}

fn outline_glyph(
    loca: &Loca<'_>,
    glyf: &Glyf<'_>,
    glyph: &Glyph<'_>,
    depth: u8,
    builder: &mut Builder<'_>,
) -> Option<Option<Rect>> {
    if depth >= MAX_COMPONENTS {
        return None;
    }
    match glyph {
        Glyph::Simple(simple) => {
            let ends = simple.end_pts_of_contours();
            if let Some(last) = ends.last() {
                let total = usize::from(last.get().checked_add(1)?);
                // A one-point glyph draws nothing but is not an error.
                if total != 1 {
                    // read-fonts yields no points for malformed point data,
                    // which ttf-parser rejects before drawing.
                    if simple.points().count() != total {
                        return None;
                    }
                    let mut contour_ends = ContourEnds::new(ends)?;
                    for point in simple.points() {
                        builder.push_point(
                            f32::from(point.x),
                            f32::from(point.y),
                            point.on_curve,
                            contour_ends.next(),
                        );
                    }
                }
            }
        }
        Glyph::Composite(composite) => {
            for component in composite.components() {
                // A component that is missing or empty draws nothing.
                let Ok(Some(data)) = loca.get_glyf(component.glyph, glyf) else {
                    continue;
                };
                let transform = Transform::combine(builder.transform, Transform::of(&component));
                let mut child = Builder::new(transform, builder.bbox, &mut *builder.builder);
                outline_glyph(loca, glyf, &data, depth + 1, &mut child)?;
                builder.bbox = child.bbox;
            }
        }
    }
    if builder.bbox.is_empty() {
        return Some(None);
    }
    Some(builder.bbox.to_rect())
}

/// Marks each contour's last point from the contour end indices, saturating
/// over malformed (decreasing) ends as ttf-parser does.
struct ContourEnds<'a> {
    ends: &'a [BigEndian<u16>],
    index: usize,
    left: u16,
}

impl<'a> ContourEnds<'a> {
    fn new(ends: &'a [BigEndian<u16>]) -> Option<Self> {
        Some(Self {
            ends,
            index: 1,
            left: ends.first()?.get(),
        })
    }

    fn next(&mut self) -> bool {
        if self.left > 0 {
            self.left -= 1;
            return false;
        }
        if let Some(end) = self.ends.get(self.index) {
            let previous = self.ends.get(self.index - 1).map_or(0, BigEndian::get);
            self.left = end.get().saturating_sub(previous).saturating_sub(1);
        }
        self.index += 1;
        true
    }
}

#[derive(Clone, Copy)]
struct Point {
    x: f32,
    y: f32,
}

impl Point {
    fn midpoint(self, other: Point) -> Point {
        Point {
            x: self.x + 0.5 * (other.x - self.x),
            y: self.y + 0.5 * (other.y - self.y),
        }
    }
}

/// A composite transform: `x' = a*x + c*y + e`, `y' = b*x + d*y + f`.
#[derive(Clone, Copy)]
struct Transform {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }
}

impl Transform {
    /// A component's placement. Point-matched anchors place it at the
    /// origin, as ttf-parser does.
    fn of(component: &Component) -> Self {
        let (e, f) = match component.anchor {
            Anchor::Offset { x, y } => (f32::from(x), f32::from(y)),
            Anchor::Point { .. } => (0.0, 0.0),
        };
        let scale = component.transform;
        Self {
            a: scale.xx.to_f32(),
            b: scale.yx.to_f32(),
            c: scale.xy.to_f32(),
            d: scale.yy.to_f32(),
            e,
            f,
        }
    }

    fn combine(outer: Self, inner: Self) -> Self {
        Self {
            a: outer.a * inner.a + outer.c * inner.b,
            b: outer.b * inner.a + outer.d * inner.b,
            c: outer.a * inner.c + outer.c * inner.d,
            d: outer.b * inner.c + outer.d * inner.d,
            e: outer.a * inner.e + outer.c * inner.f + outer.e,
            f: outer.b * inner.e + outer.d * inner.f + outer.f,
        }
    }

    fn is_identity(&self) -> bool {
        self.a == 1.0
            && self.b == 0.0
            && self.c == 0.0
            && self.d == 1.0
            && self.e == 0.0
            && self.f == 0.0
    }

    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
}

#[derive(Clone, Copy)]
struct Bounds {
    x_min: f32,
    y_min: f32,
    x_max: f32,
    y_max: f32,
}

impl Bounds {
    fn new() -> Self {
        Self {
            x_min: f32::MAX,
            y_min: f32::MAX,
            x_max: f32::MIN,
            y_max: f32::MIN,
        }
    }

    fn is_empty(&self) -> bool {
        self.x_min == f32::MAX
            && self.y_min == f32::MAX
            && self.x_max == f32::MIN
            && self.y_max == f32::MIN
    }

    fn extend(&mut self, x: f32, y: f32) {
        self.x_min = self.x_min.min(x);
        self.y_min = self.y_min.min(y);
        self.x_max = self.x_max.max(x);
        self.y_max = self.y_max.max(y);
    }

    fn to_rect(self) -> Option<Rect> {
        Some(Rect {
            x_min: to_i16(self.x_min)?,
            y_min: to_i16(self.y_min)?,
            x_max: to_i16(self.x_max)?,
            y_max: to_i16(self.y_max)?,
        })
    }
}

/// Truncates toward zero, rejecting values outside `i16`.
fn to_i16(value: f32) -> Option<i16> {
    if value >= i32::MIN as f32 && value < i32::MAX as f32 {
        i16::try_from(value as i32).ok()
    } else {
        None
    }
}

struct Builder<'a> {
    builder: &'a mut dyn OutlineBuilder,
    transform: Transform,
    identity: bool,
    bbox: Bounds,
    first_on_curve: Option<Point>,
    first_off_curve: Option<Point>,
    last_off_curve: Option<Point>,
}

impl<'a> Builder<'a> {
    fn new(transform: Transform, bbox: Bounds, builder: &'a mut dyn OutlineBuilder) -> Self {
        Self {
            builder,
            transform,
            identity: transform.is_identity(),
            bbox,
            first_on_curve: None,
            first_off_curve: None,
            last_off_curve: None,
        }
    }

    fn place(&mut self, x: f32, y: f32) -> (f32, f32) {
        let (x, y) = if self.identity {
            (x, y)
        } else {
            self.transform.apply(x, y)
        };
        self.bbox.extend(x, y);
        (x, y)
    }

    fn move_to(&mut self, p: Point) {
        let (x, y) = self.place(p.x, p.y);
        self.builder.move_to(x, y);
    }

    fn line_to(&mut self, p: Point) {
        let (x, y) = self.place(p.x, p.y);
        self.builder.line_to(x, y);
    }

    fn quad_to(&mut self, control: Point, p: Point) {
        let (x1, y1) = self.place(control.x, control.y);
        let (x, y) = self.place(p.x, p.y);
        self.builder.quad_to(x1, y1, x, y);
    }

    fn push_point(&mut self, x: f32, y: f32, on_curve: bool, last_point: bool) {
        let p = Point { x, y };
        if self.first_on_curve.is_none() {
            if on_curve {
                self.first_on_curve = Some(p);
                self.move_to(p);
            } else if let Some(off_curve) = self.first_off_curve {
                let mid = off_curve.midpoint(p);
                self.first_on_curve = Some(mid);
                self.last_off_curve = Some(p);
                self.move_to(mid);
            } else {
                self.first_off_curve = Some(p);
            }
        } else {
            match (self.last_off_curve, on_curve) {
                (Some(off_curve), true) => {
                    self.last_off_curve = None;
                    self.quad_to(off_curve, p);
                }
                (Some(off_curve), false) => {
                    self.last_off_curve = Some(p);
                    self.quad_to(off_curve, off_curve.midpoint(p));
                }
                (None, true) => self.line_to(p),
                (None, false) => self.last_off_curve = Some(p),
            }
        }
        if last_point {
            self.finish_contour();
        }
    }

    fn finish_contour(&mut self) {
        if let (Some(first), Some(last)) = (self.first_off_curve, self.last_off_curve) {
            self.last_off_curve = None;
            self.quad_to(last, last.midpoint(first));
        }
        match (
            self.first_on_curve,
            self.first_off_curve,
            self.last_off_curve,
        ) {
            (Some(start), Some(first), _) => self.quad_to(first, start),
            (Some(start), None, Some(last)) => self.quad_to(last, start),
            (Some(start), None, None) => self.line_to(start),
            (None, ..) => {}
        }
        self.first_on_curve = None;
        self.first_off_curve = None;
        self.last_off_curve = None;
        self.builder.close();
    }
}
