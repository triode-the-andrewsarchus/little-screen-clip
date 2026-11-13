//! What the drawing layer produces, at the level of pgf's system layer: paths with their stroke and
//! fill, text placed by a transformation, clips and transparency groups, and the picture's bounding
//! box as pgf computes it. All geometry (arrow tips, node shapes, decorations) is done before; the
//! SVG driver only writes these primitives.

use crate::color::Color;
use crate::scene::{Label, Point, Segment, Transform};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    Butt,
    Round,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Join {
    Miter,
    Round,
    Bevel,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub color: Color,
    /// in cm
    pub width: f64,
    /// on, off, … in cm, and the phase
    pub dash: Option<(Vec<f64>, f64)>,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f64,
    pub opacity: f64,
}

/// A color stop of a shading: position from 0 to 1, and the color.
pub type Stop = (f64, Color);

/// A shading as pgf declares it, in its own 100 bp box (−50 to 50), and the transformation that
/// takes that box to the canvas (`\pgfshadepath` fits its middle half to the path's bounding box).
#[derive(Debug, Clone, PartialEq)]
pub enum Shading {
    /// Colors along y, from y = −50 (offset 0) to y = 50 (offset 1).
    Axial { transform: Transform, stops: Vec<Stop> },
    /// Circles from `focus` (radius 0) out to the circle of radius 50 around the origin.
    Radial { transform: Transform, focus: Point, stops: Vec<Stop> },
}

/// A pattern's tile: its items, the same tile when they are the same list.
#[derive(Debug, Clone)]
pub struct TileItems(pub std::rc::Rc<Vec<Item>>);

impl PartialEq for TileItems {
    fn eq(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    Solid(Color),
    /// A pattern by name (`north east lines`), in its color, at the picture's scale.
    Pattern {
        name: String,
        color: Color,
    },
    /// A pattern declared in the document: the items of one tile (canvas cm, the tile from `origin`
    /// to `origin + size`), repeated from the page's origin, the tiling turned by `transform`.
    Tile {
        items: TileItems,
        origin: Point,
        size: Point,
        transform: Transform,
    },
    Shading(Shading),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    pub paint: Paint,
    pub opacity: f64,
    pub even_odd: bool,
}

#[derive(Debug, Clone)]
pub enum Item {
    Path {
        segments: Vec<Segment>,
        fill: Option<Fill>,
        stroke: Option<Stroke>,
    },
    /// Text: the label's first baseline starts at the origin of `place`, which maps label coordinates
    /// (cm, y up) to the canvas.
    Text {
        label: Label,
        place: Transform,
        opacity: f64,
    },
    /// Clips what follows, up to the matching `End`.
    Clip {
        segments: Vec<Segment>,
        even_odd: bool,
    },
    /// A group: what follows, up to the matching `End`, is drawn with this opacity as a whole, on
    /// its own (blend modes inside see what the group draws only).
    Group {
        opacity: f64,
    },
    /// What follows, up to the matching `End`, is blended with what is under it by this mode
    /// (`screen`, `multiply`…, as SVG names them).
    Blend {
        mode: String,
    },
    /// `transform canvas`: what follows, up to the matching `End`, is drawn transformed, line widths
    /// and text included (the picture's size does not follow, as in pgf).
    Transform {
        transform: Transform,
    },
    /// A fading: what follows, up to the matching `End`, is as opaque as the shading is white.
    Fade {
        mask: Shading,
    },
    /// A fading drawn as a picture (`\pgfdeclarefading`): what follows, up to the matching `End`, is
    /// as opaque as the picture is white there, and transparent where it draws nothing.
    FadePicture {
        items: Vec<Item>,
    },
    /// What follows, up to the matching `End`, is drawn without antialiasing, as PDF viewers
    /// draw a mesh shading: triangles side by side leave no seam of what is under them.
    Crisp,
    /// A PNG image: `place` takes its unit square (the lower left corner at the origin, the upper
    /// right at (1, 1)) to the canvas.
    Image {
        png: std::rc::Rc<Vec<u8>>,
        place: Transform,
    },
    End,
    /// What follows, up to the matching `End`, is a point of a plot, drawn on its own.
    Point(DataPoint),
    /// The next path is one bar per subpath, for these points of a plot.
    Bars {
        points: Vec<DataPoint>,
        vertical: bool,
    },
    /// What follows, up to the matching `End`, moves: the animations library played. `center` is
    /// what turns and scalings are around.
    Animated {
        animations: Vec<Animation>,
        center: Point,
    },
    /// Draws nothing: what follows, up to the next one, came from this span of
    /// [`Canvas::sources`]. Only there when the source map was asked for.
    Source(usize),
}

/// A point of a plot: its values, as they read, and where it is on the canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct DataPoint {
    pub x: String,
    pub y: String,
    pub at: Point,
}

/// What an animation changes.
#[derive(Debug, Clone, PartialEq)]
pub enum Moving {
    /// An attribute of each element, by its SVG name; `true` when only text has it.
    Attribute(&'static str, bool),
    Shift,
    Rotation,
    Scaling,
}

/// An animation of the library, in SVG's terms.
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub moving: Moving,
    /// Seconds from `begin`, in order, and the value at each, ready for SVG.
    pub times: Vec<f64>,
    pub values: Vec<String>,
    /// Seconds after the start, or after the event in `on`.
    pub begin: f64,
    /// `click`, `mouseover`… on the object.
    pub on: Option<String>,
    /// How many times, or `indefinite`.
    pub repeats: Option<String>,
    pub freeze: bool,
}

/// A statement's place in the code: the line and column of its first and last characters, from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSpan {
    pub first: (usize, usize),
    pub last: (usize, usize),
}

/// Grows the box `min`–`max` to include `p`.
fn grow(min: &mut Option<Point>, max: &mut Option<Point>, p: Point) {
    *min = Some(match *min {
        Some(m) => Point::new(m.x.min(p.x), m.y.min(p.y)),
        None => p,
    });
    *max = Some(match *max {
        Some(m) => Point::new(m.x.max(p.x), m.y.max(p.y)),
        None => p,
    });
}

#[derive(Debug, Clone, Default)]
pub struct Canvas {
    pub items: Vec<Item>,
    /// The bounding box, as pgf computes it (points and control points of paths, node shapes, text), in cm.
    pub min: Option<Point>,
    pub max: Option<Point>,
    /// The margin around (the standalone `border`), in cm.
    pub margin: f64,
    /// The `local bounding box`es being measured: grown with the picture's.
    pub local: Vec<(Option<Point>, Option<Point>)>,
    /// `overlay` is on: nothing grows the bounding box.
    pub overlay: bool,
    /// Every node and coordinate the author named, where its center is (cm), in the order they
    /// were placed: the points a page can hang handles, hints or answers on. They move with the
    /// picture when it is set on the page.
    pub named: Vec<(String, Point)>,
    /// Where the first picture's origin was set on the page (cm): each picture is moved so that
    /// its box starts where the line does.
    pub origin: Option<Point>,
    /// `\pagecolor`: painted behind the figure, margin included.
    pub page_color: Option<Color>,
    /// The places [`Item::Source`] points to: empty without the source map.
    pub sources: Vec<SourceSpan>,
    /// What each of those statements drew, in words, for the SVG's parts: empty without them.
    pub parts: Vec<String>,
    /// How many slides the overlays of the code have shown: 1 without any, and 0 on a canvas that
    /// is not a whole figure's.
    pub slides: usize,
}

impl Canvas {
    /// Grows the bounding box to include `p`.
    pub fn include(&mut self, p: Point) {
        if self.overlay || !(p.x.is_finite() && p.y.is_finite()) {
            return;
        }
        grow(&mut self.min, &mut self.max, p);
        for (lo, hi) in self.local.iter_mut() {
            grow(lo, hi, p);
        }
    }

    /// Grows the bounding box by every point of a path, control points included, as pgf does.
    pub fn include_path(&mut self, segments: &[Segment]) {
        for s in segments {
            match s {
                Segment::MoveTo(p) | Segment::LineTo(p) => self.include(*p),
                Segment::CurveTo(a, b, p) => {
                    self.include(*a);
                    self.include(*b);
                    self.include(*p);
                }
                Segment::Close => {}
            }
        }
    }

    /// Grows the bounding box by the box of a path's points enlarged by `margin` on every side, as
    /// `\pgfusepath` does by half the line width for a stroked path.
    pub fn include_path_margin(&mut self, segments: &[Segment], margin: f64) {
        let mut b = Canvas::default();
        b.include_path(segments);
        if let (Some(min), Some(max)) = (b.min, b.max) {
            self.include(Point::new(min.x - margin, min.y - margin));
            self.include(Point::new(max.x + margin, max.y + margin));
        }
    }

    pub fn push(&mut self, item: Item) {
        self.items.push(item);
    }
}

impl Item {
    /// Moves the item by `d` (cm).
    pub fn translate(&mut self, d: Point) {
        let t = Transform::translation(d.x, d.y);
        match self {
            Item::Path { segments, fill, .. } => {
                *segments = transform_segments(segments, &t);
                if let Some(Fill { paint: Paint::Shading(Shading::Axial { transform, .. } | Shading::Radial { transform, .. }), .. }) = fill {
                    *transform = t.compose(transform);
                }
            }
            Item::Clip { segments, .. } => *segments = transform_segments(segments, &t),
            Item::Text { place, .. } | Item::Image { place, .. } => *place = t.compose(place),
            Item::Fade { mask: Shading::Axial { transform, .. } | Shading::Radial { transform, .. } } => *transform = t.compose(transform),
            // the items inside move too: the transformation moves with them
            Item::Transform { transform } => *transform = t.compose(transform).compose(&Transform::translation(-d.x, -d.y)),
            Item::FadePicture { items } => items.iter_mut().for_each(|i| i.translate(d)),
            Item::Group { .. } | Item::Blend { .. } | Item::Crisp | Item::End | Item::Source(_) => {}
            Item::Animated { center, .. } => *center = center.plus(d),
            Item::Point(p) => p.at = p.at.plus(d),
            Item::Bars { points, .. } => points.iter_mut().for_each(|p| p.at = p.at.plus(d)),
        }
    }

    /// The item as drawn on a transformed canvas (pgf's low-level transformation): its geometry,
    /// its text and its line widths and dashes, which scale by the transformation's mean scale.
    pub fn transform(&mut self, t: &Transform) {
        let scale = (t.a * t.d - t.b * t.c).abs().sqrt();
        match self {
            Item::Path { segments, fill, stroke } => {
                *segments = transform_segments(segments, t);
                if let Some(Fill { paint: Paint::Shading(Shading::Axial { transform, .. } | Shading::Radial { transform, .. }), .. }) = fill {
                    *transform = t.compose(transform);
                }
                if let Some(s) = stroke {
                    s.width *= scale;
                    if let Some((dash, phase)) = &mut s.dash {
                        dash.iter_mut().for_each(|d| *d *= scale);
                        *phase *= scale;
                    }
                }
            }
            Item::Clip { segments, .. } => *segments = transform_segments(segments, t),
            Item::Text { place, .. } | Item::Image { place, .. } => *place = t.compose(place),
            Item::Fade { mask: Shading::Axial { transform, .. } | Shading::Radial { transform, .. } } => *transform = t.compose(transform),
            // a transformed group inside: the outer transformation around it
            Item::Transform { transform } => *transform = t.compose(transform).compose(&t.inverse()),
            Item::FadePicture { items } => items.iter_mut().for_each(|i| i.transform(t)),
            Item::Group { .. } | Item::Blend { .. } | Item::Crisp | Item::End | Item::Source(_) => {}
            Item::Animated { center, .. } => *center = t.apply(*center),
            Item::Point(p) => p.at = t.apply(p.at),
            Item::Bars { points, .. } => points.iter_mut().for_each(|p| p.at = t.apply(p.at)),
        }
    }
}

/// The segments with a transformation applied.
pub fn transform_segments(segments: &[Segment], t: &Transform) -> Vec<Segment> {
    segments
        .iter()
        .map(|s| match s {
            Segment::MoveTo(p) => Segment::MoveTo(t.apply(*p)),
            Segment::LineTo(p) => Segment::LineTo(t.apply(*p)),
            Segment::CurveTo(a, b, p) => Segment::CurveTo(t.apply(*a), t.apply(*b), t.apply(*p)),
            Segment::Close => Segment::Close,
        })
        .collect()
}
