// SPDX-License-Identifier: MPL-2.0
// Port of D2 v0.9.0 lib/label/label.go.
//! Faithful port of D2's `lib/label` package: label positions and placement
//! on shape boxes and connection routes.

#![allow(dead_code)]

use super::geo::{self, Point};

/// label.go `LEFT_LABEL_POSITION`.
pub const LEFT_LABEL_POSITION: f64 = 1.0 / 4.0;
/// label.go `CENTER_LABEL_POSITION`.
pub const CENTER_LABEL_POSITION: f64 = 2.0 / 4.0;
/// label.go `RIGHT_LABEL_POSITION`.
pub const RIGHT_LABEL_POSITION: f64 = 3.0 / 4.0;

/// label.go `PADDING`: space between a node border and its outside label.
/// (An untyped Go constant; used both as int and as float64.)
pub const PADDING: f64 = 5.0;

/// label.go `Position` (int8 iota order preserved).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(i8)]
pub enum Position {
    #[default]
    Unset,
    OutsideTopLeft,
    OutsideTopCenter,
    OutsideTopRight,
    OutsideLeftTop,
    OutsideLeftMiddle,
    OutsideLeftBottom,
    OutsideRightTop,
    OutsideRightMiddle,
    OutsideRightBottom,
    OutsideBottomLeft,
    OutsideBottomCenter,
    OutsideBottomRight,
    InsideTopLeft,
    InsideTopCenter,
    InsideTopRight,
    InsideMiddleLeft,
    InsideMiddleCenter,
    InsideMiddleRight,
    InsideBottomLeft,
    InsideBottomCenter,
    InsideBottomRight,
    BorderTopLeft,
    BorderTopCenter,
    BorderTopRight,
    BorderLeftTop,
    BorderLeftMiddle,
    BorderLeftBottom,
    BorderRightTop,
    BorderRightMiddle,
    BorderRightBottom,
    BorderBottomLeft,
    BorderBottomCenter,
    BorderBottomRight,
    UnlockedTop,
    UnlockedMiddle,
    UnlockedBottom,
}

impl Position {
    /// label.go `FromString`; unknown strings map to `Unset`.
    pub fn from_string(s: &str) -> Position {
        match s {
            "OUTSIDE_TOP_LEFT" => Position::OutsideTopLeft,
            "OUTSIDE_TOP_CENTER" => Position::OutsideTopCenter,
            "OUTSIDE_TOP_RIGHT" => Position::OutsideTopRight,
            "OUTSIDE_LEFT_TOP" => Position::OutsideLeftTop,
            "OUTSIDE_LEFT_MIDDLE" => Position::OutsideLeftMiddle,
            "OUTSIDE_LEFT_BOTTOM" => Position::OutsideLeftBottom,
            "OUTSIDE_RIGHT_TOP" => Position::OutsideRightTop,
            "OUTSIDE_RIGHT_MIDDLE" => Position::OutsideRightMiddle,
            "OUTSIDE_RIGHT_BOTTOM" => Position::OutsideRightBottom,
            "OUTSIDE_BOTTOM_LEFT" => Position::OutsideBottomLeft,
            "OUTSIDE_BOTTOM_CENTER" => Position::OutsideBottomCenter,
            "OUTSIDE_BOTTOM_RIGHT" => Position::OutsideBottomRight,
            "INSIDE_TOP_LEFT" => Position::InsideTopLeft,
            "INSIDE_TOP_CENTER" => Position::InsideTopCenter,
            "INSIDE_TOP_RIGHT" => Position::InsideTopRight,
            "INSIDE_MIDDLE_LEFT" => Position::InsideMiddleLeft,
            "INSIDE_MIDDLE_CENTER" => Position::InsideMiddleCenter,
            "INSIDE_MIDDLE_RIGHT" => Position::InsideMiddleRight,
            "INSIDE_BOTTOM_LEFT" => Position::InsideBottomLeft,
            "INSIDE_BOTTOM_CENTER" => Position::InsideBottomCenter,
            "INSIDE_BOTTOM_RIGHT" => Position::InsideBottomRight,
            "BORDER_TOP_LEFT" => Position::BorderTopLeft,
            "BORDER_TOP_CENTER" => Position::BorderTopCenter,
            "BORDER_TOP_RIGHT" => Position::BorderTopRight,
            "BORDER_LEFT_TOP" => Position::BorderLeftTop,
            "BORDER_LEFT_MIDDLE" => Position::BorderLeftMiddle,
            "BORDER_LEFT_BOTTOM" => Position::BorderLeftBottom,
            "BORDER_RIGHT_TOP" => Position::BorderRightTop,
            "BORDER_RIGHT_MIDDLE" => Position::BorderRightMiddle,
            "BORDER_RIGHT_BOTTOM" => Position::BorderRightBottom,
            "BORDER_BOTTOM_LEFT" => Position::BorderBottomLeft,
            "BORDER_BOTTOM_CENTER" => Position::BorderBottomCenter,
            "BORDER_BOTTOM_RIGHT" => Position::BorderBottomRight,
            "UNLOCKED_TOP" => Position::UnlockedTop,
            "UNLOCKED_MIDDLE" => Position::UnlockedMiddle,
            "UNLOCKED_BOTTOM" => Position::UnlockedBottom,
            _ => Position::Unset,
        }
    }

    /// label.go `Position.String`; `Unset` is "".
    pub fn string(self) -> &'static str {
        match self {
            Position::OutsideTopLeft => "OUTSIDE_TOP_LEFT",
            Position::OutsideTopCenter => "OUTSIDE_TOP_CENTER",
            Position::OutsideTopRight => "OUTSIDE_TOP_RIGHT",
            Position::OutsideLeftTop => "OUTSIDE_LEFT_TOP",
            Position::OutsideLeftMiddle => "OUTSIDE_LEFT_MIDDLE",
            Position::OutsideLeftBottom => "OUTSIDE_LEFT_BOTTOM",
            Position::OutsideRightTop => "OUTSIDE_RIGHT_TOP",
            Position::OutsideRightMiddle => "OUTSIDE_RIGHT_MIDDLE",
            Position::OutsideRightBottom => "OUTSIDE_RIGHT_BOTTOM",
            Position::OutsideBottomLeft => "OUTSIDE_BOTTOM_LEFT",
            Position::OutsideBottomCenter => "OUTSIDE_BOTTOM_CENTER",
            Position::OutsideBottomRight => "OUTSIDE_BOTTOM_RIGHT",
            Position::InsideTopLeft => "INSIDE_TOP_LEFT",
            Position::InsideTopCenter => "INSIDE_TOP_CENTER",
            Position::InsideTopRight => "INSIDE_TOP_RIGHT",
            Position::InsideMiddleLeft => "INSIDE_MIDDLE_LEFT",
            Position::InsideMiddleCenter => "INSIDE_MIDDLE_CENTER",
            Position::InsideMiddleRight => "INSIDE_MIDDLE_RIGHT",
            Position::InsideBottomLeft => "INSIDE_BOTTOM_LEFT",
            Position::InsideBottomCenter => "INSIDE_BOTTOM_CENTER",
            Position::InsideBottomRight => "INSIDE_BOTTOM_RIGHT",
            Position::BorderTopLeft => "BORDER_TOP_LEFT",
            Position::BorderTopCenter => "BORDER_TOP_CENTER",
            Position::BorderTopRight => "BORDER_TOP_RIGHT",
            Position::BorderLeftTop => "BORDER_LEFT_TOP",
            Position::BorderLeftMiddle => "BORDER_LEFT_MIDDLE",
            Position::BorderLeftBottom => "BORDER_LEFT_BOTTOM",
            Position::BorderRightTop => "BORDER_RIGHT_TOP",
            Position::BorderRightMiddle => "BORDER_RIGHT_MIDDLE",
            Position::BorderRightBottom => "BORDER_RIGHT_BOTTOM",
            Position::BorderBottomLeft => "BORDER_BOTTOM_LEFT",
            Position::BorderBottomCenter => "BORDER_BOTTOM_CENTER",
            Position::BorderBottomRight => "BORDER_BOTTOM_RIGHT",
            Position::UnlockedTop => "UNLOCKED_TOP",
            Position::UnlockedMiddle => "UNLOCKED_MIDDLE",
            Position::UnlockedBottom => "UNLOCKED_BOTTOM",
            Position::Unset => "",
        }
    }

    /// label.go `Position.IsShapePosition`.
    pub fn is_shape_position(self) -> bool {
        self.is_outside() || self.is_border() || self.is_inside()
    }

    fn is_inside(self) -> bool {
        use Position::*;
        matches!(
            self,
            InsideTopLeft
                | InsideTopCenter
                | InsideTopRight
                | InsideMiddleLeft
                | InsideMiddleCenter
                | InsideMiddleRight
                | InsideBottomLeft
                | InsideBottomCenter
                | InsideBottomRight
        )
    }

    /// label.go `Position.IsEdgePosition`.
    pub fn is_edge_position(self) -> bool {
        use Position::*;
        matches!(
            self,
            OutsideTopLeft
                | OutsideTopCenter
                | OutsideTopRight
                | InsideMiddleLeft
                | InsideMiddleCenter
                | InsideMiddleRight
                | OutsideBottomLeft
                | OutsideBottomCenter
                | OutsideBottomRight
                | UnlockedTop
                | UnlockedMiddle
                | UnlockedBottom
        )
    }

    /// label.go `Position.IsOutside`.
    pub fn is_outside(self) -> bool {
        use Position::*;
        matches!(
            self,
            OutsideTopLeft
                | OutsideTopCenter
                | OutsideTopRight
                | OutsideBottomLeft
                | OutsideBottomCenter
                | OutsideBottomRight
                | OutsideLeftTop
                | OutsideLeftMiddle
                | OutsideLeftBottom
                | OutsideRightTop
                | OutsideRightMiddle
                | OutsideRightBottom
        )
    }

    /// label.go `Position.IsUnlocked`.
    pub fn is_unlocked(self) -> bool {
        matches!(
            self,
            Position::UnlockedTop | Position::UnlockedMiddle | Position::UnlockedBottom
        )
    }

    /// label.go `Position.IsBorder`.
    pub fn is_border(self) -> bool {
        use Position::*;
        matches!(
            self,
            BorderTopLeft
                | BorderTopCenter
                | BorderTopRight
                | BorderLeftTop
                | BorderLeftMiddle
                | BorderLeftBottom
                | BorderRightTop
                | BorderRightMiddle
                | BorderRightBottom
                | BorderBottomLeft
                | BorderBottomCenter
                | BorderBottomRight
        )
    }

    /// label.go `Position.IsOnEdge`.
    pub fn is_on_edge(self) -> bool {
        use Position::*;
        matches!(
            self,
            InsideMiddleLeft | InsideMiddleCenter | InsideMiddleRight | UnlockedMiddle
        )
    }

    /// label.go `Position.Mirrored`.
    pub fn mirrored(self) -> Position {
        match self {
            Position::OutsideTopLeft => Position::OutsideBottomRight,
            Position::OutsideTopCenter => Position::OutsideBottomCenter,
            Position::OutsideTopRight => Position::OutsideBottomLeft,
            Position::OutsideLeftTop => Position::OutsideRightBottom,
            Position::OutsideLeftMiddle => Position::OutsideRightMiddle,
            Position::OutsideLeftBottom => Position::OutsideRightTop,
            Position::OutsideRightTop => Position::OutsideLeftBottom,
            Position::OutsideRightMiddle => Position::OutsideLeftMiddle,
            Position::OutsideRightBottom => Position::OutsideLeftTop,
            Position::OutsideBottomLeft => Position::OutsideTopRight,
            Position::OutsideBottomCenter => Position::OutsideTopCenter,
            Position::OutsideBottomRight => Position::OutsideTopLeft,
            Position::InsideTopLeft => Position::InsideBottomRight,
            Position::InsideTopCenter => Position::InsideBottomCenter,
            Position::InsideTopRight => Position::InsideBottomLeft,
            Position::InsideMiddleLeft => Position::InsideMiddleRight,
            Position::InsideMiddleCenter => Position::InsideMiddleCenter,
            Position::InsideMiddleRight => Position::InsideMiddleLeft,
            Position::InsideBottomLeft => Position::InsideTopRight,
            Position::InsideBottomCenter => Position::InsideTopCenter,
            Position::InsideBottomRight => Position::InsideTopLeft,
            Position::BorderTopLeft => Position::BorderBottomRight,
            Position::BorderTopCenter => Position::BorderBottomCenter,
            Position::BorderTopRight => Position::BorderBottomLeft,
            Position::BorderLeftTop => Position::BorderRightBottom,
            Position::BorderLeftMiddle => Position::BorderRightMiddle,
            Position::BorderLeftBottom => Position::BorderRightTop,
            Position::BorderRightTop => Position::BorderLeftBottom,
            Position::BorderRightMiddle => Position::BorderLeftMiddle,
            Position::BorderRightBottom => Position::BorderLeftTop,
            Position::BorderBottomLeft => Position::BorderTopRight,
            Position::BorderBottomCenter => Position::BorderTopCenter,
            Position::BorderBottomRight => Position::BorderTopLeft,
            Position::UnlockedTop => Position::UnlockedBottom,
            Position::UnlockedBottom => Position::UnlockedTop,
            Position::UnlockedMiddle => Position::UnlockedMiddle,
            Position::Unset => Position::Unset,
        }
    }

    /// label.go `Position.GetPointOnBox`: top-left of a width x height label
    /// at this position relative to `box`. `Unset`/unlocked return TopLeft.
    pub fn get_point_on_box(&self, b: &geo::Box, padding: f64, width: f64, height: f64) -> Point {
        use Position::*;
        let mut p = b.top_left;
        let box_center = b.center();
        // Products below are by 0.5 (exact), so ARM64 FMSUBD contraction of
        // `center - w/2` is unobservable.
        match self {
            OutsideTopLeft => {
                p.x -= padding;
                p.y -= padding + height;
            }
            OutsideTopCenter => {
                p.x = box_center.x - width / 2.0;
                p.y -= padding + height;
            }
            OutsideTopRight => {
                p.x += b.width - width - padding;
                p.y -= padding + height;
            }
            OutsideLeftTop => {
                p.x -= padding + width;
                p.y += padding;
            }
            OutsideLeftMiddle => {
                p.x -= padding + width;
                p.y = box_center.y - height / 2.0;
            }
            OutsideLeftBottom => {
                p.x -= padding + width;
                p.y += b.height - height - padding;
            }
            OutsideRightTop => {
                p.x += b.width + padding;
                p.y += padding;
            }
            OutsideRightMiddle => {
                p.x += b.width + padding;
                p.y = box_center.y - height / 2.0;
            }
            OutsideRightBottom => {
                p.x += b.width + padding;
                p.y += b.height - height - padding;
            }
            OutsideBottomLeft => {
                p.x += padding;
                p.y += b.height + padding;
            }
            OutsideBottomCenter => {
                p.x = box_center.x - width / 2.0;
                p.y += b.height + padding;
            }
            OutsideBottomRight => {
                p.x += b.width - width - padding;
                p.y += b.height + padding;
            }
            InsideTopLeft => {
                p.x += padding;
                p.y += padding;
            }
            InsideTopCenter => {
                p.x = box_center.x - width / 2.0;
                p.y += padding;
            }
            InsideTopRight => {
                p.x += b.width - width - padding;
                p.y += padding;
            }
            InsideMiddleLeft => {
                p.x += padding;
                p.y = box_center.y - height / 2.0;
            }
            InsideMiddleCenter => {
                p.x = box_center.x - width / 2.0;
                p.y = box_center.y - height / 2.0;
            }
            InsideMiddleRight => {
                p.x += b.width - width - padding;
                p.y = box_center.y - height / 2.0;
            }
            InsideBottomLeft => {
                p.x += padding;
                p.y += b.height - height - padding;
            }
            InsideBottomCenter => {
                p.x = box_center.x - width / 2.0;
                p.y += b.height - height - padding;
            }
            InsideBottomRight => {
                p.x += b.width - width - padding;
                p.y += b.height - height - padding;
            }
            BorderTopLeft => {
                p.x += padding;
                p.y -= height / 2.0;
            }
            BorderTopCenter => {
                p.x = box_center.x - width / 2.0;
                p.y -= height / 2.0;
            }
            BorderTopRight => {
                p.x += b.width - width - padding;
                p.y -= height / 2.0;
            }
            BorderLeftTop => {
                p.x -= width / 2.0;
                p.y += padding;
            }
            BorderLeftMiddle => {
                p.x -= width / 2.0;
                p.y = box_center.y - height / 2.0;
            }
            BorderLeftBottom => {
                p.x -= width / 2.0;
                p.y += b.height - height - padding;
            }
            BorderRightTop => {
                p.x += b.width - width / 2.0;
                p.y += padding;
            }
            BorderRightMiddle => {
                p.x += b.width - width / 2.0;
                p.y = box_center.y - height / 2.0;
            }
            BorderRightBottom => {
                p.x += b.width - width / 2.0;
                p.y += b.height - height - padding;
            }
            BorderBottomLeft => {
                p.x += padding;
                p.y += b.height - height / 2.0;
            }
            BorderBottomCenter => {
                p.x = box_center.x - width / 2.0;
                p.y += b.height - height / 2.0;
            }
            BorderBottomRight => {
                p.x += b.width - width - padding;
                p.y += b.height - height / 2.0;
            }
            Unset | UnlockedTop | UnlockedMiddle | UnlockedBottom => {}
        }
        p
    }

    /// label.go `Position.GetPointOnRoute`: top-left of a width x height
    /// label at this position on `route`, plus the route segment index.
    /// Go returns `(nil, -1)` for non-edge positions; that is `None` here.
    /// Requires a route of at least two points, as Go does.
    pub fn try_get_point_on_route(
        &self,
        route: &[Point],
        stroke_width: f64,
        label_percentage: f64,
        width: f64,
        height: f64,
    ) -> Option<(Point, usize)> {
        use Position::*;
        let total_length = geo::route_length(route);
        let left_position = LEFT_LABEL_POSITION * total_length;
        let center_position = CENTER_LABEL_POSITION * total_length;
        let right_position = RIGHT_LABEL_POSITION * total_length;
        let unlocked_position = label_percentage * total_length;

        let get_offset_label_position =
            |base_point: Point, norm_start: Point, norm_end: Point, flip: bool| {
                let (mut normal_x, mut normal_y) =
                    geo::get_unit_normal_vector(norm_start.x, norm_start.y, norm_end.x, norm_end.y);
                if flip {
                    normal_x *= -1.0;
                    normal_y *= -1.0;
                }
                // strokeWidth/2 + float64(PADDING) + width/2; the halvings are
                // exact so ARM64's FMADDD contraction is unobservable.
                let offset_x = stroke_width / 2.0 + PADDING + width / 2.0;
                let offset_y = stroke_width / 2.0 + PADDING + height / 2.0;
                // ARM64: basePoint.X + normalX*offsetX is FMADDD (FMSUBD with the
                // un-negated normal when flipped; identical result).
                Point::new(
                    geo::go_fmadd(normal_x, offset_x, base_point.x),
                    geo::go_fmadd(normal_y, offset_y, base_point.y),
                )
            };
        let offset_at = |distance: f64, flip: bool| {
            let (base_point, index) = geo::route_get_point_at_distance(route, distance);
            (
                get_offset_label_position(base_point, route[index], route[index + 1], flip),
                index,
            )
        };

        let (mut label_center, index) = match self {
            InsideMiddleLeft => geo::route_get_point_at_distance(route, left_position),
            InsideMiddleCenter => geo::route_get_point_at_distance(route, center_position),
            InsideMiddleRight => geo::route_get_point_at_distance(route, right_position),
            OutsideTopLeft => offset_at(left_position, true),
            OutsideTopCenter => offset_at(center_position, true),
            OutsideTopRight => offset_at(right_position, true),
            OutsideBottomLeft => offset_at(left_position, false),
            OutsideBottomCenter => offset_at(center_position, false),
            OutsideBottomRight => offset_at(right_position, false),
            UnlockedTop => offset_at(unlocked_position, true),
            UnlockedMiddle => geo::route_get_point_at_distance(route, unlocked_position),
            UnlockedBottom => offset_at(unlocked_position, false),
            _ => return None,
        };
        // Go's case blocks shadow `index` with `:=`, so offset cases return
        // the named result's zero value.
        let index = match self {
            InsideMiddleLeft | InsideMiddleCenter | InsideMiddleRight | UnlockedMiddle => index,
            _ => 0,
        };
        label_center.x = chop_precision(label_center.x - width / 2.0);
        label_center.y = chop_precision(label_center.y - height / 2.0);
        Some((label_center, index))
    }

    /// [`Self::try_get_point_on_route`] for callers that, like D2's
    /// `GetLabelTopLeft`/`GetIconTopLeft`, expect an edge position. Non-edge
    /// positions (Go's nil point) yield `(Point::default(), usize::MAX)`.
    pub fn get_point_on_route(
        &self,
        route: &[Point],
        stroke_width: f64,
        label_percentage: f64,
        width: f64,
        height: f64,
    ) -> (Point, usize) {
        self.try_get_point_on_route(route, stroke_width, label_percentage, width, height)
            .unwrap_or((Point::default(), usize::MAX))
    }
}

/// label.go `chopPrecision`: round through float32 at 1e-4 scale, then to an
/// integer; negative zero becomes positive zero.
pub fn chop_precision(f: f64) -> f64 {
    let result = (((f * 10000.0) as f32 as f64) / 10000.0).round();
    if result == 0.0 {
        return 0.0;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_round_trip() {
        const ALL: [Position; 36] = [
            Position::OutsideTopLeft,
            Position::OutsideTopCenter,
            Position::OutsideTopRight,
            Position::OutsideLeftTop,
            Position::OutsideLeftMiddle,
            Position::OutsideLeftBottom,
            Position::OutsideRightTop,
            Position::OutsideRightMiddle,
            Position::OutsideRightBottom,
            Position::OutsideBottomLeft,
            Position::OutsideBottomCenter,
            Position::OutsideBottomRight,
            Position::InsideTopLeft,
            Position::InsideTopCenter,
            Position::InsideTopRight,
            Position::InsideMiddleLeft,
            Position::InsideMiddleCenter,
            Position::InsideMiddleRight,
            Position::InsideBottomLeft,
            Position::InsideBottomCenter,
            Position::InsideBottomRight,
            Position::BorderTopLeft,
            Position::BorderTopCenter,
            Position::BorderTopRight,
            Position::BorderLeftTop,
            Position::BorderLeftMiddle,
            Position::BorderLeftBottom,
            Position::BorderRightTop,
            Position::BorderRightMiddle,
            Position::BorderRightBottom,
            Position::BorderBottomLeft,
            Position::BorderBottomCenter,
            Position::BorderBottomRight,
            Position::UnlockedTop,
            Position::UnlockedMiddle,
            Position::UnlockedBottom,
        ];
        for p in ALL {
            assert_eq!(Position::from_string(p.string()), p);
            assert_eq!(p.mirrored().mirrored(), p);
        }
        assert_eq!(Position::from_string("nope"), Position::Unset);
        assert_eq!(Position::Unset.string(), "");
    }
}
