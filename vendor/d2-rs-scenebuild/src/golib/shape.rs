// SPDX-License-Identifier: MPL-2.0
// Port of D2 v0.9.0 lib/shape (shape.go and every shape_*.go), plus the
// d2target.DSL_SHAPE_TO_SHAPE_TYPE table (d2target/d2target.go).
//! Faithful port of D2's `lib/shape` package.
//!
//! Go models each shape as a struct embedding `*baseShape` whose
//! `GetInsidePlacement` dispatches back through `FullShape`; here one struct
//! carries the shape kind and every method matches on it, so the override
//! set of each Go type is listed explicitly in each method.
//!
//! Float evaluation follows the pinned Go 1.27 ARM64 listing
//! (`go build -gcflags=-S`); each contraction is marked `ARM64:`. Go
//! constant expressions (exact rational arithmetic, rounded once) are written
//! as their compiled bit patterns where a two-step f64 evaluation could differ.

#![allow(dead_code)]

use super::geo::{self, Intersectable, Point, go_fmadd, go_fmsub, go_fnmsub, gomath};
use super::svgpath::{PathCommand, SvgPathContext};

pub const SQUARE_TYPE: &str = "Square";
pub const REAL_SQUARE_TYPE: &str = "RealSquare";
pub const PARALLELOGRAM_TYPE: &str = "Parallelogram";
pub const DOCUMENT_TYPE: &str = "Document";
pub const CYLINDER_TYPE: &str = "Cylinder";
pub const QUEUE_TYPE: &str = "Queue";
pub const PAGE_TYPE: &str = "Page";
pub const PACKAGE_TYPE: &str = "Package";
pub const STEP_TYPE: &str = "Step";
pub const CALLOUT_TYPE: &str = "Callout";
pub const STORED_DATA_TYPE: &str = "StoredData";
pub const PERSON_TYPE: &str = "Person";
pub const C4_PERSON_TYPE: &str = "C4Person";
pub const DIAMOND_TYPE: &str = "Diamond";
pub const OVAL_TYPE: &str = "Oval";
pub const CIRCLE_TYPE: &str = "Circle";
pub const HEXAGON_TYPE: &str = "Hexagon";
pub const CLOUD_TYPE: &str = "Cloud";
pub const TABLE_TYPE: &str = "Table";
pub const CLASS_TYPE: &str = "Class";
pub const TEXT_TYPE: &str = "Text";
pub const CODE_TYPE: &str = "Code";
pub const IMAGE_TYPE: &str = "Image";

/// shape.go `defaultPadding`.
const DEFAULT_PADDING: f64 = 40.0;

/// d2target.go `DSL_SHAPE_TO_SHAPE_TYPE`; unknown DSL shapes map to "" (Go's
/// missing-map-key zero value).
pub fn dsl_shape_to_shape_type(dsl: &str) -> &'static str {
    match dsl {
        "" | "rectangle" => SQUARE_TYPE,
        "square" => REAL_SQUARE_TYPE,
        "page" => PAGE_TYPE,
        "parallelogram" => PARALLELOGRAM_TYPE,
        "document" => DOCUMENT_TYPE,
        "cylinder" => CYLINDER_TYPE,
        "queue" => QUEUE_TYPE,
        "package" => PACKAGE_TYPE,
        "step" => STEP_TYPE,
        "callout" => CALLOUT_TYPE,
        "stored_data" => STORED_DATA_TYPE,
        "person" => PERSON_TYPE,
        "c4-person" => C4_PERSON_TYPE,
        "diamond" => DIAMOND_TYPE,
        "oval" => OVAL_TYPE,
        "circle" => CIRCLE_TYPE,
        "hexagon" => HEXAGON_TYPE,
        "cloud" => CLOUD_TYPE,
        "text" => TEXT_TYPE,
        "code" => CODE_TYPE,
        "class" => CLASS_TYPE,
        "sql_table" => TABLE_TYPE,
        "image" => IMAGE_TYPE,
        "sequence_diagram" | "hierarchy" => SQUARE_TYPE,
        _ => "",
    }
}

/// shape.go `Shape` interface (plus `PathCommandProvider`).
pub trait Shape {
    /// `Is`.
    fn is(&self, shape_type: &str) -> bool {
        self.get_type() == shape_type
    }
    /// `GetType`. Go's NewShape default branch keeps an arbitrary type string,
    /// so this borrows from the shape.
    fn get_type(&self) -> &str;
    /// `AspectRatio1`.
    fn aspect_ratio1(&self) -> bool;
    /// `IsRectangular`.
    fn is_rectangular(&self) -> bool;
    /// `GetBox`.
    fn get_box(&self) -> geo::Box;
    /// `GetInnerBox`.
    fn get_inner_box(&self) -> geo::Box;
    /// `GetInnerBoxForContent`; Go returns nil except for clouds.
    fn get_inner_box_for_content(&self, width: f64, height: f64) -> Option<geo::Box>;
    /// `SetInnerBoxAspectRatio` (only clouds store it).
    fn set_inner_box_aspect_ratio(&mut self, aspect_ratio: f64);
    /// `GetInsidePlacement`: position of a padded rectangle inside the shape.
    fn get_inside_placement(
        &self,
        width: f64,
        height: f64,
        padding_x: f64,
        padding_y: f64,
    ) -> Point;
    /// `GetDimensionsToFit`.
    fn get_dimensions_to_fit(
        &self,
        width: f64,
        height: f64,
        padding_x: f64,
        padding_y: f64,
    ) -> (f64, f64);
    /// `GetDefaultPadding`.
    fn get_default_padding(&self) -> (f64, f64);
    /// `Perimeter`; Go returns nil for primitive shapes (empty here).
    fn perimeter(&self) -> Vec<Intersectable>;
    /// `GetSVGPathData`; Go returns nil for primitive shapes (empty here).
    fn get_svg_path_data(&self) -> Vec<String>;
    /// shape.go `GetPathCommands(s)`: None when the shape is not a
    /// `PathCommandProvider`.
    fn get_path_commands(&self) -> Option<Vec<Vec<PathCommand>>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// shapeSquare, including NewShape's default branch.
    Square,
    RealSquare,
    Parallelogram,
    Document,
    Cylinder,
    Queue,
    Page,
    Package,
    Step,
    Callout,
    StoredData,
    Person,
    C4Person,
    Diamond,
    Oval,
    Circle,
    Hexagon,
    Cloud,
    Table,
    Class,
    Text,
    Code,
    Image,
}

/// One D2 shape (Go's `shapeXxx{*baseShape}` values).
#[derive(Clone, Debug, PartialEq)]
pub struct D2Shape {
    kind: Kind,
    shape_type: String,
    b: geo::Box,
    /// shapeCloud.innerBoxAspectRatio (starts at 0).
    inner_box_aspect_ratio: f64,
}

/// shape.go `NewShape`.
pub fn new_shape(shape_type: &str, b: geo::Box) -> std::boxed::Box<dyn Shape> {
    std::boxed::Box::new(new_d2_shape(shape_type, b))
}

/// [`new_shape`] returning the concrete type.
pub fn new_d2_shape(shape_type: &str, b: geo::Box) -> D2Shape {
    let kind = match shape_type {
        CALLOUT_TYPE => Kind::Callout,
        CIRCLE_TYPE => Kind::Circle,
        CLASS_TYPE => Kind::Class,
        CLOUD_TYPE => Kind::Cloud,
        CODE_TYPE => Kind::Code,
        CYLINDER_TYPE => Kind::Cylinder,
        DIAMOND_TYPE => Kind::Diamond,
        DOCUMENT_TYPE => Kind::Document,
        HEXAGON_TYPE => Kind::Hexagon,
        IMAGE_TYPE => Kind::Image,
        OVAL_TYPE => Kind::Oval,
        PACKAGE_TYPE => Kind::Package,
        PAGE_TYPE => Kind::Page,
        PARALLELOGRAM_TYPE => Kind::Parallelogram,
        PERSON_TYPE => Kind::Person,
        C4_PERSON_TYPE => Kind::C4Person,
        QUEUE_TYPE => Kind::Queue,
        REAL_SQUARE_TYPE => Kind::RealSquare,
        STEP_TYPE => Kind::Step,
        STORED_DATA_TYPE => Kind::StoredData,
        SQUARE_TYPE => Kind::Square,
        TABLE_TYPE => Kind::Table,
        TEXT_TYPE => Kind::Text,
        _ => Kind::Square,
    };
    D2Shape {
        kind,
        shape_type: shape_type.to_owned(),
        b,
        inner_box_aspect_ratio: 0.0,
    }
}

impl Shape for D2Shape {
    fn get_type(&self) -> &str {
        &self.shape_type
    }

    fn aspect_ratio1(&self) -> bool {
        matches!(self.kind, Kind::RealSquare | Kind::Circle)
    }

    fn is_rectangular(&self) -> bool {
        matches!(
            self.kind,
            Kind::Square
                | Kind::RealSquare
                | Kind::Image
                | Kind::Class
                | Kind::Table
                | Kind::Text
                | Kind::Code
        )
    }

    fn get_box(&self) -> geo::Box {
        self.b
    }

    fn get_inner_box(&self) -> geo::Box {
        let b = &self.b;
        match self.kind {
            Kind::C4Person => c4_person_inner_box(b),
            Kind::Callout => {
                let tip_height = get_tip_height(b);
                geo::Box::new(b.top_left, b.width, b.height - tip_height)
            }
            Kind::Circle | Kind::Oval => {
                // shape_circle.go / shape_oval.go GetInnerBox.
                let mut width = b.width;
                let mut height = b.height;
                let inside_tl = self.get_inside_placement(width, height, 0.0, 0.0);
                let tl = b.top_left;
                width -= 2.0 * (inside_tl.x - tl.x);
                height -= 2.0 * (inside_tl.y - tl.y);
                geo::Box::new(inside_tl, width, height)
            }
            Kind::Cloud => {
                if self.inner_box_aspect_ratio != 0.0 {
                    self.cloud_inner_box_for_content(self.inner_box_aspect_ratio, 1.0)
                } else {
                    self.cloud_inner_box_for_content(b.width, b.height)
                }
            }
            Kind::Cylinder => {
                let mut height = b.height;
                let mut tl = b.top_left;
                let arc = get_arc_height(b);
                // ARM64: height - 3*arc is FMSUBD.
                height = go_fmsub(3.0, arc, height);
                tl.y += 2.0 * arc;
                geo::Box::new(tl, b.width, height)
            }
            Kind::Diamond => {
                let mut width = b.width;
                let mut height = b.height;
                let mut tl = b.top_left;
                tl.x += width / 4.0;
                tl.y += height / 4.0;
                width /= 2.0;
                height /= 2.0;
                geo::Box::new(tl, width, height)
            }
            Kind::Document => {
                let height = b.height * DOC_PATH_INNER_BOTTOM / DOC_PATH_HEIGHT;
                geo::Box::new(b.top_left, b.width, height)
            }
            Kind::Hexagon => {
                let mut width = b.width;
                let mut height = b.height;
                let mut tl = b.top_left;
                tl.x += width / 6.0;
                width /= 1.5;
                tl.y += height / 6.0;
                height /= 1.5;
                geo::Box::new(tl, width, height)
            }
            Kind::Package => {
                let mut tl = b.top_left;
                let mut height = b.height;
                let (_, top_height) = get_top_dimensions(b);
                tl.y += top_height;
                height -= top_height;
                geo::Box::new(tl, b.width, height)
            }
            Kind::Page => {
                let mut width = b.width;
                if b.height < f64::from_bits(0x404e85a1cac08312) {
                    // 3*pageCornerHeight
                    width -= PAGE_CORNER_WIDTH;
                }
                geo::Box::new(b.top_left, width, b.height)
            }
            Kind::Parallelogram => {
                let mut tl = b.top_left;
                let width = b.width - 2.0 * PARALLEL_WEDGE_WIDTH;
                tl.x += PARALLEL_WEDGE_WIDTH;
                geo::Box::new(tl, width, b.height)
            }
            Kind::Person => {
                let mut width = b.width;
                let mut tl = b.top_left;
                let shoulder_width = PERSON_SHOULDER_WIDTH_FACTOR * width;
                // ARM64: tl.X + shoulderWidth re-fuses factor*width, and
                // `shoulderWidth*2` is lowered to sw + sw with the second
                // product re-fused.
                tl.x = go_fmadd(width, PERSON_SHOULDER_WIDTH_FACTOR, tl.x);
                width -= go_fmadd(width, PERSON_SHOULDER_WIDTH_FACTOR, shoulder_width);
                geo::Box::new(tl, width, b.height)
            }
            Kind::Queue => {
                let mut width = b.width;
                let mut tl = b.top_left;
                let arc_width = get_arc_width(b);
                // ARM64: width - 3*arcWidth is FMSUBD.
                width = go_fmsub(3.0, arc_width, width);
                tl.x += arc_width;
                geo::Box::new(tl, width, b.height)
            }
            Kind::Step => {
                let mut width = b.width;
                let mut tl = b.top_left;
                width -= 2.0 * STEP_WEDGE_WIDTH;
                tl.x += STEP_WEDGE_WIDTH;
                geo::Box::new(tl, width, b.height)
            }
            Kind::StoredData => {
                let mut width = b.width;
                let mut tl = b.top_left;
                width -= 2.0 * STORED_DATA_WEDGE_WIDTH;
                tl.x += STORED_DATA_WEDGE_WIDTH;
                geo::Box::new(tl, width, b.height)
            }
            Kind::Square
            | Kind::RealSquare
            | Kind::Table
            | Kind::Class
            | Kind::Text
            | Kind::Code
            | Kind::Image => *b,
        }
    }

    fn get_inner_box_for_content(&self, width: f64, height: f64) -> Option<geo::Box> {
        match self.kind {
            Kind::Cloud => Some(self.cloud_inner_box_for_content(width, height)),
            _ => None,
        }
    }

    fn set_inner_box_aspect_ratio(&mut self, aspect_ratio: f64) {
        if self.kind == Kind::Cloud {
            self.inner_box_aspect_ratio = aspect_ratio;
        }
    }

    fn get_inside_placement(
        &self,
        width: f64,
        height: f64,
        padding_x: f64,
        padding_y: f64,
    ) -> Point {
        let b = &self.b;
        match self.kind {
            Kind::Circle => {
                // shape_circle.go GetInsidePlacement. The ARM64 contractions
                // here only fuse exact products by 0.5.
                let r = b.width / 2.0;
                let half_length = r * std::f64::consts::SQRT_2 / 2.0;
                Point::new(
                    b.top_left.x + (r - half_length + padding_x / 2.0).ceil(),
                    b.top_left.y + (r - half_length + padding_y / 2.0).ceil(),
                )
            }
            Kind::Oval => {
                // shape_oval.go GetInsidePlacement.
                let rx = b.width / 2.0;
                let ry = b.height / 2.0;
                let theta = gomath::atan2(ry, rx) as f32 as f64;
                let sin = gomath::sin(theta);
                let cos = gomath::cos(theta);
                let r = rx * ry / (gomath::pow(rx * sin, 2.0) + gomath::pow(ry * cos, 2.0)).sqrt();
                // ARM64: rx - cos*(r - px/2) is FMSUBD.
                Point::new(
                    b.top_left.x + go_fmsub(cos, r - padding_x / 2.0, rx).ceil(),
                    b.top_left.y + go_fmsub(sin, r - padding_y / 2.0, ry).ceil(),
                )
            }
            Kind::Cloud => {
                let r = b;
                let width = width + padding_x;
                let height = height + padding_y;
                let aspect_ratio = width / height;
                // The ARM64 contractions here only fuse exact `p/2` products.
                let (ix, iy) = if aspect_ratio > CLOUD_WIDE_ASPECT_BOUNDARY {
                    (CLOUD_WIDE_INNER_X, CLOUD_WIDE_INNER_Y)
                } else if aspect_ratio < CLOUD_TALL_ASPECT_BOUNDARY {
                    (CLOUD_TALL_INNER_X, CLOUD_TALL_INNER_Y)
                } else {
                    (CLOUD_SQUARE_INNER_X, CLOUD_SQUARE_INNER_Y)
                };
                Point::new(
                    r.top_left.x + (r.width * ix + padding_x / 2.0).ceil(),
                    r.top_left.y + (r.height * iy + padding_y / 2.0).ceil(),
                )
            }
            _ => {
                // shape.go baseShape.GetInsidePlacement via FullShape.
                let inner_tl = self.get_inner_box().top_left;
                Point::new(inner_tl.x + padding_x / 2.0, inner_tl.y + padding_y / 2.0)
            }
        }
    }

    fn get_dimensions_to_fit(
        &self,
        width: f64,
        height: f64,
        padding_x: f64,
        padding_y: f64,
    ) -> (f64, f64) {
        match self.kind {
            Kind::C4Person => c4_person_dimensions_to_fit(width, height, padding_x, padding_y),
            Kind::Callout => {
                let mut base_height = height + padding_y;
                if base_height < DEFAULT_TIP_HEIGHT {
                    base_height *= 2.0;
                } else {
                    base_height += DEFAULT_TIP_HEIGHT;
                }
                ((width + padding_x).ceil(), base_height.ceil())
            }
            Kind::Circle => {
                let length = geo::go_max(width + padding_x, height + padding_y);
                let diameter = (std::f64::consts::SQRT_2 * length).ceil();
                (diameter, diameter)
            }
            Kind::Cloud => {
                let width = width + padding_x;
                let height = height + padding_y;
                let aspect_ratio = width / height;
                let (w, h) = if aspect_ratio > CLOUD_WIDE_ASPECT_BOUNDARY {
                    (CLOUD_WIDE_INNER_WIDTH, CLOUD_WIDE_INNER_HEIGHT)
                } else if aspect_ratio < CLOUD_TALL_ASPECT_BOUNDARY {
                    (CLOUD_TALL_INNER_WIDTH, CLOUD_TALL_INNER_HEIGHT)
                } else {
                    (CLOUD_SQUARE_INNER_WIDTH, CLOUD_SQUARE_INNER_HEIGHT)
                };
                ((width / w).ceil(), (height / h).ceil())
            }
            Kind::Cylinder => {
                // height + paddingY + 3*defaultArcDepth (constant 72).
                let total_height = height + padding_y + 3.0 * DEFAULT_ARC_DEPTH;
                ((width + padding_x).ceil(), total_height.ceil())
            }
            Kind::Diamond => {
                let total_width = 2.0 * (width + padding_x);
                let total_height = 2.0 * (height + padding_y);
                (total_width.ceil(), total_height.ceil())
            }
            Kind::Document => {
                let base_height = (height + padding_y) * DOC_PATH_HEIGHT / DOC_PATH_INNER_BOTTOM;
                ((width + padding_x).ceil(), base_height.ceil())
            }
            Kind::Hexagon => {
                let total_width = 1.5 * (width + padding_x);
                let total_height = 1.5 * (height + padding_y);
                (total_width.ceil(), total_height.ceil())
            }
            Kind::Oval => {
                let theta = gomath::atan2(height, width) as f32 as f64;
                // ARM64: width + paddingX*cos(theta) is FMADDD.
                let padded_width = go_fmadd(padding_x, gomath::cos(theta), width);
                let padded_height = go_fmadd(padding_y, gomath::sin(theta), height);
                let total_width = (std::f64::consts::SQRT_2 * padded_width).ceil();
                let total_height = (std::f64::consts::SQRT_2 * padded_height).ceil();
                limit_ar(total_width, total_height, OVAL_AR_LIMIT)
            }
            Kind::Package => {
                let inner_height = height + padding_y;
                let top_height =
                    inner_height * PACKAGE_VERTICAL_SCALAR / (1.0 - PACKAGE_VERTICAL_SCALAR);
                let total_height = inner_height + geo::go_min(top_height, PACKAGE_TOP_MAX_HEIGHT);
                ((width + padding_x).ceil(), total_height.ceil())
            }
            Kind::Page => {
                let mut total_width = width + padding_x;
                let mut total_height = height + padding_y;
                if total_height < f64::from_bits(0x404e85a1cac08312) {
                    total_width += PAGE_CORNER_WIDTH;
                }
                total_width = geo::go_max(total_width, 2.0 * PAGE_CORNER_WIDTH);
                total_height = geo::go_max(total_height, PAGE_CORNER_HEIGHT);
                (total_width.ceil(), total_height.ceil())
            }
            Kind::Parallelogram => {
                let total_width = width + padding_x + PARALLEL_WEDGE_WIDTH * 2.0;
                (total_width.ceil(), (height + padding_y).ceil())
            }
            Kind::Person => {
                let mut total_width = width + padding_x;
                // totalWidth*f / (1 - 2*f): the divisor is a Go constant.
                let shoulder_width =
                    total_width * PERSON_SHOULDER_WIDTH_FACTOR / f64::from_bits(0x3fda24bb6892eda2);
                total_width += 2.0 * shoulder_width;
                let total_height = height + padding_y;
                let (w, h) = limit_ar(total_width, total_height, PERSON_AR_LIMIT);
                (w.ceil(), h.ceil())
            }
            Kind::Queue => {
                let total_width = 3.0 * DEFAULT_ARC_DEPTH + width + padding_x;
                (total_width.ceil(), (height + padding_y).ceil())
            }
            Kind::RealSquare => {
                let side_length = geo::go_max(width + padding_x, height + padding_y).ceil();
                (side_length, side_length)
            }
            Kind::Step => {
                let total_width = width + padding_x + 2.0 * STEP_WEDGE_WIDTH;
                (total_width.ceil(), (height + padding_y).ceil())
            }
            Kind::StoredData => {
                let total_width = width + padding_x + 2.0 * STORED_DATA_WEDGE_WIDTH;
                (total_width.ceil(), (height + padding_y).ceil())
            }
            Kind::Square | Kind::Table | Kind::Class | Kind::Text | Kind::Code | Kind::Image => {
                ((width + padding_x).ceil(), (height + padding_y).ceil())
            }
        }
    }

    fn get_default_padding(&self) -> (f64, f64) {
        match self.kind {
            Kind::C4Person => (10.0, DEFAULT_PADDING),
            Kind::Callout => (DEFAULT_PADDING, DEFAULT_PADDING / 2.0),
            // defaultPadding / math.Sqrt2 (Go constant, 28.284271247461902).
            Kind::Circle => (
                f64::from_bits(0x403c48c6001f0ac0),
                f64::from_bits(0x403c48c6001f0ac0),
            ),
            Kind::Class | Kind::Code | Kind::Table | Kind::Text | Kind::Image => (0.0, 0.0),
            Kind::Cloud => (DEFAULT_PADDING, DEFAULT_PADDING / 2.0),
            Kind::Cylinder => (DEFAULT_PADDING, DEFAULT_PADDING / 2.0),
            Kind::Diamond => (DEFAULT_PADDING / 4.0, DEFAULT_PADDING / 2.0),
            // defaultPadding * docPathInnerBottom / docPathHeight (constant).
            Kind::Document => (DEFAULT_PADDING, f64::from_bits(0x403d972a45aa1a61)),
            Kind::Hexagon => (DEFAULT_PADDING / 2.0, DEFAULT_PADDING / 2.0),
            Kind::Package => (DEFAULT_PADDING, 0.8 * DEFAULT_PADDING),
            // pageCornerHeight + defaultPadding (constant 60.348).
            Kind::Page => (DEFAULT_PADDING, f64::from_bits(0x404e2c8b43958106)),
            Kind::Person => (10.0, DEFAULT_PADDING),
            Kind::Queue => (DEFAULT_PADDING / 2.0, DEFAULT_PADDING),
            Kind::Step => (DEFAULT_PADDING / 4.0, DEFAULT_PADDING + STEP_WEDGE_WIDTH),
            Kind::StoredData => (DEFAULT_PADDING - 10.0, DEFAULT_PADDING),
            Kind::Square | Kind::RealSquare | Kind::Oval | Kind::Parallelogram => {
                (DEFAULT_PADDING, DEFAULT_PADDING)
            }
        }
    }

    fn perimeter(&self) -> Vec<Intersectable> {
        let b = &self.b;
        match self.kind {
            Kind::C4Person => {
                let width = b.width;
                let mut perimeter = body_path(b).path;
                let head_radius = width * HEAD_RADIUS_FACTOR;
                let head_center_x = b.top_left.x + width / 2.0;
                // ARM64: TopLeft.Y + headRadius re-fuses width*0.22.
                let head_center_y = go_fmadd(width, HEAD_RADIUS_FACTOR, b.top_left.y);
                perimeter.push(Intersectable::Ellipse(geo::Ellipse::new(
                    Point::new(head_center_x, head_center_y),
                    head_radius,
                    head_radius,
                )));
                perimeter
            }
            Kind::Circle | Kind::Oval => vec![Intersectable::Ellipse(geo::Ellipse::new(
                b.center(),
                b.width / 2.0,
                b.height / 2.0,
            ))],
            _ => match self.paths().into_iter().next() {
                Some(outer) => outer.path,
                None => Vec::new(),
            },
        }
    }

    fn get_svg_path_data(&self) -> Vec<String> {
        self.paths().iter().map(SvgPathContext::path_data).collect()
    }

    fn get_path_commands(&self) -> Option<Vec<Vec<PathCommand>>> {
        let paths = self.paths();
        if paths.is_empty() {
            return None;
        }
        Some(paths.iter().map(SvgPathContext::path_commands).collect())
    }
}

impl D2Shape {
    /// The custom path contexts in GetSVGPathData order (empty for shapes
    /// drawn as primitives).
    fn paths(&self) -> Vec<SvgPathContext> {
        let b = &self.b;
        match self.kind {
            Kind::C4Person => vec![body_path(b), head_path(b)],
            Kind::Callout => vec![callout_path(b)],
            Kind::Cloud => vec![cloud_path(b)],
            Kind::Cylinder => vec![cylinder_outer_path(b), cylinder_inner_path(b)],
            Kind::Diamond => vec![diamond_path(b)],
            Kind::Document => vec![document_path(b)],
            Kind::Hexagon => vec![hexagon_path(b)],
            Kind::Package => vec![package_path(b)],
            Kind::Page => vec![page_outer_path(b), page_inner_path(b)],
            Kind::Parallelogram => vec![parallelogram_path(b)],
            Kind::Person => vec![person_path(b)],
            Kind::Queue => vec![queue_outer_path(b), queue_inner_path(b)],
            Kind::Step => vec![step_path(b)],
            Kind::StoredData => vec![stored_data_path(b)],
            Kind::Square
            | Kind::RealSquare
            | Kind::Table
            | Kind::Class
            | Kind::Text
            | Kind::Code
            | Kind::Image
            | Kind::Oval
            | Kind::Circle => Vec::new(),
        }
    }

    /// shape_cloud.go `GetInnerBoxForContent`.
    fn cloud_inner_box_for_content(&self, width: f64, height: f64) -> geo::Box {
        let inside_tl = self.get_inside_placement(width, height, 0.0, 0.0);
        let aspect_ratio = width / height;
        let mut width = self.b.width;
        let mut height = self.b.height;
        if aspect_ratio > CLOUD_WIDE_ASPECT_BOUNDARY {
            width *= CLOUD_WIDE_INNER_WIDTH;
            height *= CLOUD_WIDE_INNER_HEIGHT;
        } else if aspect_ratio < CLOUD_TALL_ASPECT_BOUNDARY {
            width *= CLOUD_TALL_INNER_WIDTH;
            height *= CLOUD_TALL_INNER_HEIGHT;
        } else {
            width *= CLOUD_SQUARE_INNER_WIDTH;
            height *= CLOUD_SQUARE_INNER_HEIGHT;
        }
        geo::Box::new(inside_tl, width, height)
    }
}

/// shape.go `TraceToShapeBorder`: the point on the shape's border along the
/// ray prevPoint -> rectBorderPoint (rectangular shapes return
/// `rect_border_point` unchanged).
pub fn trace_to_shape_border(
    shape: &dyn Shape,
    rect_border_point: Point,
    prev_point: Point,
) -> Point {
    if shape.is("") || shape.is_rectangular() {
        return rect_border_point;
    }

    let mut scale_size = shape.get_box().width;
    if prev_point.x == rect_border_point.x {
        scale_size = shape.get_box().height;
    }
    let vector = prev_point.vector_to(&rect_border_point);
    let vector = vector.add_length(scale_size);
    let extended_segment = geo::Segment::new(prev_point, prev_point.add_vector(&vector));

    let mut closest_d = f64::INFINITY;
    let mut closest_point = rect_border_point;

    for perimeter_segment in shape.perimeter() {
        for intersecting_point in perimeter_segment.intersections(&extended_segment) {
            let d = geo::euclidean_distance(
                rect_border_point.x,
                rect_border_point.y,
                intersecting_point.x,
                intersecting_point.y,
            );
            if d < closest_d {
                closest_d = d;
                closest_point = intersecting_point;
            }
        }
    }

    closest_point.truncate_float32();
    Point::new(closest_point.x.round(), closest_point.y.round())
}

/// shape.go `LimitAR`.
pub fn limit_ar(mut width: f64, mut height: f64, aspect_ratio: f64) -> (f64, f64) {
    if width > aspect_ratio * height {
        height = (width / aspect_ratio).round();
    } else if height > aspect_ratio * width {
        width = (height / aspect_ratio).round();
    }
    (width, height)
}

// ------------------------------------------------------- shape_c4_person.go

const C4_PERSON_AR_LIMIT: f64 = 1.5;
const HEAD_RADIUS_FACTOR: f64 = 0.22;
const BODY_TOP_FACTOR: f64 = 0.8;
const CORNER_RADIUS_FACTOR: f64 = 0.175;

/// `4*(math.Sqrt(2)-1)/3`, folded by the Go compiler in float64 steps.
fn c4_kappa() -> f64 {
    4.0 * (2f64.sqrt() - 1.0) / 3.0
}

fn c4_person_inner_box(b: &geo::Box) -> geo::Box {
    let width = b.width;
    let height = b.height;
    let head_radius = width * HEAD_RADIUS_FACTOR;
    // ARM64: headCenterY + headRadius*0.8 rounds headRadius*0.8 and re-fuses
    // headCenterY's width*0.22.
    let body_top = go_fmadd(width, HEAD_RADIUS_FACTOR, head_radius * BODY_TOP_FACTOR);
    let horizontal_padding = width * 0.05;
    let vertical_padding = height * 0.03;

    let mut tl = b.top_left;
    // ARM64: tl.X + horizontalPadding re-fuses width*0.05.
    tl.x = go_fmadd(width, 0.05, tl.x);
    // ARM64: bodyTop + verticalPadding re-fuses height*0.03.
    tl.y += go_fmadd(height, 0.03, body_top);

    // ARM64: `x*2` is lowered to x + x with the product re-fused.
    let inner_width = width - go_fmadd(width, 0.05, horizontal_padding);
    let inner_height = height - body_top - go_fmadd(height, 0.03, vertical_padding);

    geo::Box::new(tl, inner_width, inner_height)
}

/// shape_c4_person.go `bodyPath`.
fn body_path(b: &geo::Box) -> SvgPathContext {
    let width = b.width;
    let height = b.height;
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);

    let head_radius = width * HEAD_RADIUS_FACTOR;
    // ARM64: headRadius + headRadius*0.8 is FMADDD.
    let body_top = go_fmadd(head_radius, BODY_TOP_FACTOR, head_radius);
    let body_width = width;
    let body_height = height - body_top;
    let body_left = 0.0;

    let corner_radius = geo::go_min(width * CORNER_RADIUS_FACTOR, body_height * 0.25);
    let k = c4_kappa();

    pc.start_at(pc.absolute(body_left, body_top + corner_radius));
    pc.c(
        true,
        0.0,
        -k * corner_radius,
        k * corner_radius,
        -corner_radius,
        corner_radius,
        -corner_radius,
    );
    pc.h(true, body_width - 2.0 * corner_radius);
    pc.c(
        true,
        k * corner_radius,
        0.0,
        corner_radius,
        k * corner_radius,
        corner_radius,
        corner_radius,
    );
    pc.v(true, body_height - 2.0 * corner_radius);
    pc.c(
        true,
        0.0,
        k * corner_radius,
        -k * corner_radius,
        corner_radius,
        -corner_radius,
        corner_radius,
    );
    pc.h(true, -(body_width - 2.0 * corner_radius));
    pc.c(
        true,
        -k * corner_radius,
        0.0,
        -corner_radius,
        -k * corner_radius,
        -corner_radius,
        -corner_radius,
    );
    pc.z();
    pc
}

/// shape_c4_person.go `headPath`.
fn head_path(b: &geo::Box) -> SvgPathContext {
    let width = b.width;
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);

    let head_radius = width * HEAD_RADIUS_FACTOR;
    let head_center_x = width / 2.0;
    let head_center_y = head_radius;
    // headRadius*4*(math.Sqrt(2)-1)/3, left to right.
    let kr = head_radius * 4.0 * (2f64.sqrt() - 1.0) / 3.0;

    // ARM64: every `headCenterY±...` re-fuses headCenterY's width*0.22 and
    // every `±headRadius` re-fuses headRadius's width*0.22 (FMADDD, FMSUBD,
    // FNMSUBD); headCenterX = width*0.5 is exact.
    let cy_minus_r = go_fmsub(width, HEAD_RADIUS_FACTOR, head_center_y);
    let cy_plus_r = go_fmadd(width, HEAD_RADIUS_FACTOR, head_center_y);
    let cx_plus_r = go_fmadd(width, HEAD_RADIUS_FACTOR, head_center_x);
    let cx_minus_r = go_fmsub(width, HEAD_RADIUS_FACTOR, head_center_x);
    let cy_minus_k = go_fnmsub(width, HEAD_RADIUS_FACTOR, kr);
    let cy_plus_k = go_fmadd(width, HEAD_RADIUS_FACTOR, kr);
    let cx_plus_k = head_center_x + kr;
    let cx_minus_k = head_center_x - kr;

    pc.start_at(pc.absolute(head_center_x, cy_minus_r));
    pc.c(
        false,
        cx_plus_k,
        cy_minus_r,
        cx_plus_r,
        cy_minus_k,
        cx_plus_r,
        head_center_y,
    );
    pc.c(
        false,
        cx_plus_r,
        cy_plus_k,
        cx_plus_k,
        cy_plus_r,
        head_center_x,
        cy_plus_r,
    );
    pc.c(
        false,
        cx_minus_k,
        cy_plus_r,
        cx_minus_r,
        cy_plus_k,
        cx_minus_r,
        head_center_y,
    );
    pc.c(
        false,
        cx_minus_r,
        cy_minus_k,
        cx_minus_k,
        cy_minus_r,
        head_center_x,
        cy_minus_r,
    );
    pc
}

fn c4_person_dimensions_to_fit(
    width: f64,
    height: f64,
    padding_x: f64,
    padding_y: f64,
) -> (f64, f64) {
    let content_width = width + padding_x;
    let content_height = height + padding_y;
    let mut total_width = content_width / 0.9;
    let head_radius = total_width * HEAD_RADIUS_FACTOR;
    // ARM64: headRadius + headRadius*0.8 is FMADDD.
    let body_top = go_fmadd(head_radius, BODY_TOP_FACTOR, head_radius);
    // ARM64: (contentHeight + bodyTop) + totalWidth*0.06 is FMADDD.
    let mut total_height = go_fmadd(total_width, 0.06, content_height + body_top);
    let min_height = total_width * 0.95;
    if total_height < min_height {
        total_height = min_height;
    }
    (total_width, total_height) = limit_ar(total_width, total_height, C4_PERSON_AR_LIMIT);
    (total_width.ceil(), total_height.ceil())
}

// --------------------------------------------------------- shape_callout.go

const DEFAULT_TIP_WIDTH: f64 = 30.0;
const DEFAULT_TIP_HEIGHT: f64 = 45.0;

fn get_tip_width(b: &geo::Box) -> f64 {
    let mut tip_width = DEFAULT_TIP_WIDTH;
    if b.width < tip_width * 2.0 {
        tip_width = b.width / 2.0;
    }
    tip_width
}

fn get_tip_height(b: &geo::Box) -> f64 {
    let mut tip_height = DEFAULT_TIP_HEIGHT;
    if b.height < tip_height * 2.0 {
        tip_height = b.height / 2.0;
    }
    tip_height
}

fn callout_path(b: &geo::Box) -> SvgPathContext {
    let tip_width = get_tip_width(b);
    let tip_height = get_tip_height(b);
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);
    pc.start_at(pc.absolute(0.0, 0.0));
    pc.v(true, b.height - tip_height);
    pc.h(true, b.width / 2.0);
    pc.v(true, tip_height);
    pc.l(true, tip_width, -tip_height);
    pc.h(true, b.width / 2.0 - tip_width);
    pc.v(true, -(b.height - tip_height));
    pc.h(true, -b.width);
    pc.z();
    pc
}

// ----------------------------------------------------------- shape_cloud.go

const CLOUD_WIDE_INNER_X: f64 = 0.085;
const CLOUD_WIDE_INNER_Y: f64 = 0.409;
const CLOUD_WIDE_INNER_WIDTH: f64 = 0.819;
const CLOUD_WIDE_INNER_HEIGHT: f64 = 0.548;
/// `(1 + CLOUD_WIDE_INNER_WIDTH/CLOUD_WIDE_INNER_HEIGHT) / 2` (Go constant).
const CLOUD_WIDE_ASPECT_BOUNDARY: f64 = f64::from_bits(0x3ff3f4c9cf6a82ce);
const CLOUD_TALL_INNER_X: f64 = 0.228;
const CLOUD_TALL_INNER_Y: f64 = 0.179;
const CLOUD_TALL_INNER_WIDTH: f64 = 0.549;
const CLOUD_TALL_INNER_HEIGHT: f64 = 0.820;
/// `(1 + CLOUD_TALL_INNER_WIDTH/CLOUD_TALL_INNER_HEIGHT) / 2` (Go constant).
const CLOUD_TALL_ASPECT_BOUNDARY: f64 = f64::from_bits(0x3feab6526b6526b6);
const CLOUD_SQUARE_INNER_X: f64 = 0.167;
const CLOUD_SQUARE_INNER_Y: f64 = 0.335;
const CLOUD_SQUARE_INNER_WIDTH: f64 = 0.663;
const CLOUD_SQUARE_INNER_HEIGHT: f64 = 0.663;

fn cloud_path(b: &geo::Box) -> SvgPathContext {
    let mut pc = SvgPathContext::new(b.top_left, b.width / 834.0, b.height / 523.0);
    pc.start_at(pc.absolute(137.833, 182.833));
    pc.c(true, 0.0, 5.556, -5.556, 11.111, -11.111, 11.111);
    pc.c(true, -70.833, 6.944, -126.389, 77.778, -126.389, 163.889);
    pc.c(true, 0.0, 91.667, 62.5, 165.278, 141.667, 165.278);
    pc.h(true, 537.5);
    pc.c(true, 84.723, 0.0, 154.167, -79.167, 154.167, -175.0);
    pc.c(true, 0.0, -91.667, -63.89, -168.056, -144.444, -173.611);
    pc.c(true, -5.556, 0.0, -11.111, -4.167, -12.5, -11.111);
    pc.c(true, -18.056, -93.055, -101.39, -162.5, -198.611, -162.5);
    pc.c(true, -63.889, 0.0, -120.834, 29.167, -156.944, 75.0);
    pc.c(true, -4.167, 5.556, -11.111, 6.945, -15.278, 5.556);
    pc.c(true, -13.889, -5.556, -29.166, -8.333, -45.833, -8.333);
    pc.c(false, 196.167, 71.722, 143.389, 120.333, 137.833, 182.833);
    pc.z();
    pc
}

// -------------------------------------------------------- shape_cylinder.go

const DEFAULT_ARC_DEPTH: f64 = 24.0;

fn get_arc_height(b: &geo::Box) -> f64 {
    let mut arc_height = DEFAULT_ARC_DEPTH;
    if b.height < arc_height * 2.0 {
        arc_height = b.height / 2.0;
    }
    arc_height
}

fn cylinder_outer_path(b: &geo::Box) -> SvgPathContext {
    let arc_height = get_arc_height(b);
    let multiplier = 0.45;
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);
    // ARM64: box.Width - box.Width*multiplier is FMSUBD.
    let w_minus = go_fmsub(b.width, multiplier, b.width);
    pc.start_at(pc.absolute(0.0, arc_height));
    pc.c(
        false,
        0.0,
        0.0,
        b.width * multiplier,
        0.0,
        b.width / 2.0,
        0.0,
    );
    pc.c(false, w_minus, 0.0, b.width, 0.0, b.width, arc_height);
    pc.v(true, b.height - arc_height * 2.0);
    pc.c(
        false,
        b.width,
        b.height,
        w_minus,
        b.height,
        b.width / 2.0,
        b.height,
    );
    pc.c(
        false,
        b.width * multiplier,
        b.height,
        0.0,
        b.height,
        0.0,
        b.height - arc_height,
    );
    pc.v(true, -(b.height - arc_height * 2.0));
    pc.z();
    pc
}

fn cylinder_inner_path(b: &geo::Box) -> SvgPathContext {
    let arc_height = get_arc_height(b);
    let multiplier = 0.45;
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);
    pc.start_at(pc.absolute(0.0, arc_height));
    pc.c(
        false,
        0.0,
        arc_height * 2.0,
        b.width * multiplier,
        arc_height * 2.0,
        b.width / 2.0,
        arc_height * 2.0,
    );
    // ARM64: box.Width - box.Width*multiplier is FMSUBD.
    pc.c(
        false,
        go_fmsub(b.width, multiplier, b.width),
        arc_height * 2.0,
        b.width,
        arc_height * 2.0,
        b.width,
        arc_height,
    );
    pc
}

// --------------------------------------------------------- shape_diamond.go

fn diamond_path(b: &geo::Box) -> SvgPathContext {
    let mut pc = SvgPathContext::new(b.top_left, b.width / 77.0, b.height / 76.9);
    pc.start_at(pc.absolute(38.5, 76.9));
    pc.c(true, -0.3, 0.0, -0.5, -0.1, -0.7, -0.3);
    pc.l(false, 0.3, 39.2);
    pc.c(true, -0.4, -0.4, -0.4, -1.0, 0.0, -1.4);
    pc.l(false, 37.8, 0.3);
    pc.c(true, 0.4, -0.4, 1.0, -0.4, 1.4, 0.0);
    pc.l(true, 37.5, 37.5);
    pc.c(true, 0.4, 0.4, 0.4, 1.0, 0.0, 1.4);
    pc.l(false, 39.2, 76.6);
    pc.c(false, 39.0, 76.8, 38.8, 76.9, 38.5, 76.9);
    pc.z();
    pc
}

// -------------------------------------------------------- shape_document.go

const DOC_PATH_HEIGHT: f64 = 18.925;
const DOC_PATH_INNER_BOTTOM: f64 = 14.0;
/// `docPathBottom/docPathHeight` (Go constant).
const DOC_BOTTOM_RATIO: f64 = f64::from_bits(0x3feb8fb9a8c015a5);
/// `12.8/docPathHeight` (Go constant).
const DOC_CONTROL_HIGH: f64 = f64::from_bits(0x3fe5a4b1346add2b);
/// `19.8/docPathHeight` (Go constant).
const DOC_CONTROL_LOW: f64 = f64::from_bits(0x3ff0bd610e8aa70f);

fn document_path(b: &geo::Box) -> SvgPathContext {
    let mut pc = SvgPathContext::new(b.top_left, b.width, b.height);
    pc.start_at(pc.absolute(0.0, DOC_BOTTOM_RATIO));
    pc.l(false, 0.0, 0.0);
    pc.l(false, 1.0, 0.0);
    pc.l(false, 1.0, DOC_BOTTOM_RATIO);
    pc.c(
        false,
        5.0 / 6.0,
        DOC_CONTROL_HIGH,
        2.0 / 3.0,
        DOC_CONTROL_HIGH,
        1.0 / 2.0,
        DOC_BOTTOM_RATIO,
    );
    pc.c(
        false,
        1.0 / 3.0,
        DOC_CONTROL_LOW,
        1.0 / 6.0,
        DOC_CONTROL_LOW,
        0.0,
        DOC_BOTTOM_RATIO,
    );
    pc.z();
    pc
}

// --------------------------------------------------------- shape_hexagon.go

fn hexagon_path(b: &geo::Box) -> SvgPathContext {
    // 43.6 / 87.3 (Go constant).
    let half_y_factor = f64::from_bits(0x3fdff69dc3979b20);
    let mut pc = SvgPathContext::new(b.top_left, b.width, b.height);
    pc.start_at(pc.absolute(0.25, 0.0));
    pc.l(false, 0.0, half_y_factor);
    pc.l(false, 0.25, 1.0);
    pc.l(false, 0.75, 1.0);
    pc.l(false, 1.0, half_y_factor);
    pc.l(false, 0.75, 0.0);
    pc.z();
    pc
}

// ------------------------------------------------------------ shape_oval.go

const OVAL_AR_LIMIT: f64 = 3.0;

// --------------------------------------------------------- shape_package.go

const PACKAGE_TOP_MIN_HEIGHT: f64 = 34.0;
const PACKAGE_TOP_MAX_HEIGHT: f64 = 55.0;
const PACKAGE_TOP_MIN_WIDTH: f64 = 50.0;
const PACKAGE_TOP_MAX_WIDTH: f64 = 150.0;
const PACKAGE_HORIZONTAL_SCALAR: f64 = 0.5;
const PACKAGE_VERTICAL_SCALAR: f64 = 0.2;

fn get_top_dimensions(b: &geo::Box) -> (f64, f64) {
    let mut width = b.width * PACKAGE_HORIZONTAL_SCALAR;
    if b.width >= 2.0 * PACKAGE_TOP_MIN_WIDTH {
        width = geo::go_min(
            PACKAGE_TOP_MAX_WIDTH,
            geo::go_max(PACKAGE_TOP_MIN_WIDTH, width),
        );
    }
    let height = geo::go_min(PACKAGE_TOP_MAX_HEIGHT, b.height * PACKAGE_VERTICAL_SCALAR);
    (width, height)
}

fn package_path(b: &geo::Box) -> SvgPathContext {
    let (top_width, top_height) = get_top_dimensions(b);
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);
    pc.start_at(pc.absolute(0.0, 0.0));
    pc.l(false, top_width, 0.0);
    pc.l(false, top_width, top_height);
    pc.l(false, b.width, top_height);
    pc.l(false, b.width, b.height);
    pc.l(false, 0.0, b.height);
    pc.z();
    pc
}

// ------------------------------------------------------------ shape_page.go

const PAGE_CORNER_WIDTH: f64 = 20.8164;
const PAGE_CORNER_HEIGHT: f64 = 20.348;

fn page_outer_path(b: &geo::Box) -> SvgPathContext {
    let (w, h) = (b.width, b.height);
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);
    pc.start_at(pc.absolute(0.5, 0.0));
    pc.h(false, w - 20.8164);
    pc.c(
        false,
        w - 19.6456,
        0.0,
        w - 18.521,
        0.456297,
        w - 17.6811,
        1.27202,
    );
    pc.l(false, w - 1.3647, 17.12);
    pc.c(false, w - 0.4923, 17.9674, w, 19.1318, w, 20.348);
    pc.v(false, h - 0.5);
    pc.c(false, w, h - 0.2239, w - 0.2239, h, w - 0.5, h);
    pc.h(false, 0.499999);
    pc.c(false, 0.223857, h, 0.0, h - 0.2239, 0.0, h - 0.5);
    pc.v(false, 0.499999);
    pc.c(false, 0.0, 0.223857, 0.223857, 0.0, 0.5, 0.0);
    pc.z();
    pc
}

fn page_inner_path(b: &geo::Box) -> SvgPathContext {
    let (w, h) = (b.width, b.height);
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);
    pc.start_at(pc.absolute(w - 1.08197, h));
    pc.h(false, 1.08196);
    pc.c(true, -0.64918, 0.0, -1.08196, -0.43287, -1.08196, -1.08219);
    pc.v(false, 1.08219);
    pc.c(true, 0.0, -0.64931, 0.43278, -1.08219, 1.08196, -1.08219);
    pc.h(true, w - 22.72132);
    pc.c(true, 0.64918, 0.0, 1.08196, 0.43287, 1.08196, 1.08219);
    pc.v(true, 17.09863);
    pc.c(true, 0.0, 1.29863, 0.86557, 2.38082, 2.38032, 2.38082);
    pc.h(false, w - 1.08197);
    pc.c(true, 0.64918, 0.0, 1.08196, 0.43287, 1.08196, 1.08196);
    pc.v(false, h - 1.0822);
    pc.c(false, w - 1.0, h - 0.43288, w - 0.43279, h, w - 1.08197, h);
    pc.z();
    pc
}

// --------------------------------------------------- shape_parallelogram.go

const PARALLEL_WEDGE_WIDTH: f64 = 26.0;

fn parallelogram_path(b: &geo::Box) -> SvgPathContext {
    let mut wedge_width = PARALLEL_WEDGE_WIDTH;
    if b.width <= wedge_width {
        wedge_width = b.width / 2.0;
    }
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);
    pc.start_at(pc.absolute(wedge_width, 0.0));
    pc.l(false, b.width, 0.0);
    pc.l(false, b.width - wedge_width, b.height);
    pc.l(false, 0.0, b.height);
    pc.l(false, 0.0, b.height);
    pc.z();
    pc
}

// ---------------------------------------------------------- shape_person.go

const PERSON_AR_LIMIT: f64 = 1.5;
/// `20.2 / 68.3` (Go constant).
const PERSON_SHOULDER_WIDTH_FACTOR: f64 = f64::from_bits(0x3fd2eda24bb6892f);

fn person_path(b: &geo::Box) -> SvgPathContext {
    let mut pc = SvgPathContext::new(b.top_left, b.width / 68.3, b.height / 77.4);
    pc.start_at(pc.absolute(68.3, 77.4));
    pc.h(false, 0.0);
    pc.v(true, -1.1);
    pc.c(true, 0.0, -13.2, 7.5, -25.1, 19.3, -30.8);
    pc.c(false, 12.8, 40.9, 8.9, 33.4, 8.9, 25.2);
    pc.c(false, 8.9, 11.3, 20.2, 0.0, 34.1, 0.0);
    pc.c(true, 13.9, 0.0, 25.2, 11.3, 25.2, 25.2);
    pc.c(true, 0.0, 8.2, -3.8, 15.6, -10.4, 20.4);
    pc.c(true, 11.8, 5.7, 19.3, 17.6, 19.3, 30.8);
    pc.v(true, 1.0);
    pc.h(false, 68.3);
    pc.z();
    pc
}

// ----------------------------------------------------------- shape_queue.go

fn get_arc_width(b: &geo::Box) -> f64 {
    let mut arc_width = DEFAULT_ARC_DEPTH;
    if b.width < arc_width * 2.0 {
        arc_width = b.width / 2.0;
    }
    arc_width
}

fn queue_outer_path(b: &geo::Box) -> SvgPathContext {
    let arc_width = get_arc_width(b);
    let multiplier = 0.45;
    let (w, h) = (b.width, b.height);
    // ARM64: box.Height - box.Height*multiplier is FMSUBD.
    let h_minus = go_fmsub(h, multiplier, h);
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);
    pc.start_at(pc.absolute(arc_width, 0.0));
    pc.h(true, w - 2.0 * arc_width);
    pc.c(false, w, 0.0, w, h * multiplier, w, h / 2.0);
    pc.c(false, w, h_minus, w, h, w - arc_width, h);
    pc.h(true, -(w - 2.0 * arc_width));
    pc.c(false, 0.0, h, 0.0, h_minus, 0.0, h / 2.0);
    pc.c(false, 0.0, h * multiplier, 0.0, 0.0, arc_width, 0.0);
    pc.z();
    pc
}

fn queue_inner_path(b: &geo::Box) -> SvgPathContext {
    let arc_width = get_arc_width(b);
    let multiplier = 0.45;
    let (w, h) = (b.width, b.height);
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);
    pc.start_at(pc.absolute(w - arc_width, 0.0));
    pc.c(
        false,
        w - 2.0 * arc_width,
        0.0,
        w - 2.0 * arc_width,
        h * multiplier,
        w - 2.0 * arc_width,
        h / 2.0,
    );
    // ARM64: box.Height - box.Height*multiplier is FMSUBD.
    pc.c(
        false,
        w - 2.0 * arc_width,
        go_fmsub(h, multiplier, h),
        w - 2.0 * arc_width,
        h,
        w - arc_width,
        h,
    );
    pc
}

// ------------------------------------------------------------ shape_step.go

const STEP_WEDGE_WIDTH: f64 = 35.0;

fn step_path(b: &geo::Box) -> SvgPathContext {
    let mut wedge_width = STEP_WEDGE_WIDTH;
    if b.width <= wedge_width {
        wedge_width = b.width / 2.0;
    }
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);
    pc.start_at(pc.absolute(0.0, 0.0));
    pc.l(false, b.width - wedge_width, 0.0);
    pc.l(false, b.width, b.height / 2.0);
    pc.l(false, b.width - wedge_width, b.height);
    pc.l(false, 0.0, b.height);
    pc.l(false, wedge_width, b.height / 2.0);
    pc.z();
    pc
}

// ----------------------------------------------------- shape_stored_data.go

const STORED_DATA_WEDGE_WIDTH: f64 = 15.0;

fn stored_data_path(b: &geo::Box) -> SvgPathContext {
    let mut wedge_width = STORED_DATA_WEDGE_WIDTH;
    let multiplier = 0.27;
    if b.width < wedge_width * 2.0 {
        wedge_width = b.width / 2.0;
    }
    let (w, h) = (b.width, b.height);
    // ARM64: every `x - y*multiplier` is FMSUBD.
    let w_minus_wm = go_fmsub(wedge_width, multiplier, w);
    let h_minus_hm = go_fmsub(h, multiplier, h);
    let ww_minus_wm = go_fmsub(wedge_width, multiplier, wedge_width);
    let mut pc = SvgPathContext::new(b.top_left, 1.0, 1.0);
    pc.start_at(pc.absolute(wedge_width, 0.0));
    pc.h(true, w - wedge_width);
    pc.c(
        false,
        w_minus_wm,
        0.0,
        w - wedge_width,
        h * multiplier,
        w - wedge_width,
        h / 2.0,
    );
    pc.c(false, w - wedge_width, h_minus_hm, w_minus_wm, h, w, h);
    pc.h(true, -(w - wedge_width));
    pc.c(false, ww_minus_wm, h, 0.0, h_minus_hm, 0.0, h / 2.0);
    pc.c(
        false,
        0.0,
        h * multiplier,
        ww_minus_wm,
        0.0,
        wedge_width,
        0.0,
    );
    pc.z();
    pc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_constant_expressions() {
        // Go folds these exactly; the bit patterns come from the pinned build.
        assert_eq!(c4_kappa().to_bits(), 0x3fe1ac5111534a23);
        assert_eq!((2f64.sqrt() - 1.0).to_bits(), 0x3fda827999fcef34);
        assert_eq!((5.0f64 / 6.0).to_bits(), 0x3feaaaaaaaaaaaab);
        assert_eq!(DOC_PATH_HEIGHT.to_bits(), 0x4032eccccccccccd);
    }

    #[test]
    fn custom_shapes_provide_path_commands() {
        // lib/shape/path_commands_test.go TestCustomShapesProvideTypedPathCommands.
        for (shape_type, path_count) in [
            (CALLOUT_TYPE, 1),
            (CLOUD_TYPE, 1),
            (CYLINDER_TYPE, 2),
            (DIAMOND_TYPE, 1),
            (DOCUMENT_TYPE, 1),
            (HEXAGON_TYPE, 1),
            (PACKAGE_TYPE, 1),
            (PAGE_TYPE, 2),
            (PARALLELOGRAM_TYPE, 1),
            (PERSON_TYPE, 1),
            (C4_PERSON_TYPE, 2),
            (QUEUE_TYPE, 2),
            (STEP_TYPE, 1),
            (STORED_DATA_TYPE, 1),
        ] {
            let s = new_shape(
                shape_type,
                geo::Box::new(Point::new(7.5, -4.25), 120.0, 90.0),
            );
            let paths = s.get_path_commands().expect(shape_type);
            assert_eq!(paths.len(), path_count, "{shape_type}");
            assert_eq!(s.get_svg_path_data().len(), path_count, "{shape_type}");
        }
        for shape_type in [SQUARE_TYPE, OVAL_TYPE, CIRCLE_TYPE, TEXT_TYPE, IMAGE_TYPE] {
            let s = new_shape(shape_type, geo::Box::new(Point::new(0.0, 0.0), 10.0, 10.0));
            assert!(s.get_path_commands().is_none(), "{shape_type}");
        }
    }
}

// The d2-rs-render original also runs the lib/geo and lib/shape oracle and
// golden suites (crates/d2-rs-render/tests/d2svg_geometry); this copy is
// covered by those through its identical source and by the scenebuild
// scoreboard.
