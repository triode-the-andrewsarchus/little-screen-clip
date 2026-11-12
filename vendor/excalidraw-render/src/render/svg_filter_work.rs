//! The raster work and surfaces a nested SVG's filter effects cost resvg.
//!
//! The sanitizer charges a nested SVG for its source, but resvg spends its
//! filter work per painted occurrence, at that occurrence's device size: every
//! primitive costs its result surface, and some cost far more per pixel by a
//! parameter (octaves, kernel order, morphology radius). A one-kilobyte SVG
//! can ask for hours of turbulence or gigabytes of intermediate results. Both
//! backends that rasterize a nested SVG, the PNG canvas and the PDF raster
//! fallback, measure the tree with the transform and surface they are about to
//! hand resvg, and charge that before it runs.
//!
//! The walk follows `resvg::render`'s own traversal (resvg 0.49): the same
//! group isolation, a filtered layer laid out without its transform's
//! rotation and skew, layer bounds clamped to the filter context, filter
//! regions cropped out of their layer, pattern tiles, masks, `feImage`
//! subtrees and nested SVG images, with the same `f32` geometry, so a layer's
//! size is resvg's. Where resvg could do less (an empty
//! input, a failed allocation, a zero blur) the walk still charges, so the
//! measure is an upper bound. Costs are integer and saturating.

use tiny_skia::IntRect;
use usvg::filter::Kind;
use usvg::Transform;

/// Work units per pixel of every primitive's result: the input copy, colour
/// space conversion, subregion clipping and the result surface itself.
const PRIMITIVE_PIXEL_WORK: u64 = 16;
/// Work units per pixel of a filtered group's own layer: painting its content
/// into the layer and compositing the filter result back.
const LAYER_PIXEL_WORK: u64 = 16;
/// Additional work units per pixel of a Gaussian blur resvg approximates with
/// three box passes over each axis (a device deviation of at least 2),
/// independent of the deviation.
const BOX_BLUR_PIXEL_WORK: u64 = 112;
/// Additional work units per pixel of a narrower Gaussian blur, which resvg
/// runs as a recursive (IIR) filter: four steps each way over every channel
/// and axis, in `f64`, column by column.
const IIR_BLUR_PIXEL_WORK: u64 = 496;
/// Additional work units per pixel, channel and octave of `feTurbulence`: one
/// gradient-noise evaluation.
const TURBULENCE_OCTAVE_WORK: u64 = 16;
/// Additional work units per pixel of a lighting primitive: surface normal,
/// light vector and (specular) exponentiation.
const LIGHTING_PIXEL_WORK: u64 = 64;
/// Additional work units per pixel of a per-channel transfer function, which
/// may exponentiate every channel.
const TRANSFER_PIXEL_WORK: u64 = 64;
/// Additional work units per pixel of the remaining per-pixel primitives:
/// colour matrix, displacement map, tile and the two-input blends.
const SIMPLE_PIXEL_WORK: u64 = 16;
/// Additional work units per pixel and kernel cell of `feConvolveMatrix` and
/// per pixel and neighbourhood cell of `feMorphology`.
const KERNEL_CELL_WORK: u64 = 4;
/// Work units per pixel of a filter region resvg copies out of a layer it
/// does not fill: one read and one write per channel.
const REGION_COPY_PIXEL_WORK: u64 = 8;

/// What resvg spends on the filter effects of one rasterization.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct FilterWork {
    /// Deterministic work units, on the scale of the backends' other raster
    /// charges: about one per sample operation.
    pub(super) work_units: u64,
    /// Device pixels of every surface the effects allocate: each filtered
    /// group's layer and each primitive's result.
    pub(super) surface_pixels: u64,
}

impl FilterWork {
    pub(super) const fn is_empty(self) -> bool {
        self.work_units == 0 && self.surface_pixels == 0
    }
}

/// Measures `resvg::render(tree, transform, pixmap)` into a `width` by
/// `height` pixmap. The walk stops once it has found more than `budget` work
/// units (or visited more than `budget` nodes), which already exceeds any
/// admissible charge. PNG draws whole nested trees.
#[cfg(any(feature = "png", test))]
pub(super) fn measure_tree(
    tree: &usvg::Tree,
    transform: Transform,
    width: u32,
    height: u32,
    budget: u64,
) -> FilterWork {
    let mut walk = Walk::new(budget);
    if let Some(context) = max_filter_bbox(width, height) {
        walk.nodes(tree.root(), context, transform, (width, height));
    }
    walk.finish()
}

/// Measures `resvg::render_node(node, transform, pixmap)` into a `width` by
/// `height` pixmap, as `measure_tree` does. PDF rasterizes isolated nodes.
#[cfg(feature = "pdf")]
pub(super) fn measure_node(
    node: &usvg::Node,
    transform: Transform,
    width: u32,
    height: u32,
    budget: u64,
) -> FilterWork {
    let mut walk = Walk::new(budget);
    if let (Some(bbox), Some(context)) = (
        node.abs_layer_bounding_box(),
        max_filter_bbox(width, height),
    ) {
        walk.node(
            node,
            context,
            transform.pre_translate(-bbox.x(), -bbox.y()),
            (width, height),
        );
    }
    walk.finish()
}

struct Walk {
    work: u64,
    surfaces: u64,
    visits: u64,
    budget: u64,
}

impl Walk {
    const fn new(budget: u64) -> Self {
        Self {
            work: 0,
            surfaces: 0,
            visits: 0,
            budget,
        }
    }

    const fn exhausted(&self) -> bool {
        self.work > self.budget || self.visits > self.budget
    }

    fn finish(self) -> FilterWork {
        FilterWork {
            // A walk cut short by its visits has not seen all the work; report
            // at least the crossing it was cut short for.
            work_units: if self.visits > self.budget {
                self.work.max(self.budget.saturating_add(1))
            } else {
                self.work
            },
            surface_pixels: self.surfaces,
        }
    }

    fn charge(&mut self, surface_pixels: u64, work_units: u64) {
        self.surfaces = self.surfaces.saturating_add(surface_pixels);
        self.work = self.work.saturating_add(work_units);
    }

    /// `render_nodes`: `surface` is the size of the pixmap being painted.
    fn nodes(
        &mut self,
        parent: &usvg::Group,
        context: IntRect,
        transform: Transform,
        surface: (u32, u32),
    ) {
        for node in parent.children() {
            if self.exhausted() {
                return;
            }
            self.node(node, context, transform, surface);
        }
    }

    fn node(
        &mut self,
        node: &usvg::Node,
        context: IntRect,
        transform: Transform,
        surface: (u32, u32),
    ) {
        self.visits = self.visits.saturating_add(1);
        match node {
            usvg::Node::Group(group) => self.group(group, context, transform, surface),
            usvg::Node::Path(path) => {
                if !path.is_visible() {
                    return;
                }
                for paint in [
                    path.fill().map(usvg::Fill::paint),
                    path.stroke().map(usvg::Stroke::paint),
                ]
                .into_iter()
                .flatten()
                {
                    if let usvg::Paint::Pattern(pattern) = paint {
                        self.pattern(pattern, context, transform);
                    }
                }
            }
            usvg::Node::Image(image) => {
                // A nested SVG image paints into a copy of the current pixmap
                // through a fresh `resvg::render`, with its own filter context.
                if let usvg::ImageKind::SVG(tree) = image.kind() {
                    if let Some(context) =
                        max_filter_bbox(surface.0, surface.1).filter(|_| image.is_visible())
                    {
                        self.nodes(tree.root(), context, transform, surface);
                    }
                }
            }
            usvg::Node::Text(text) => self.group(text.flattened(), context, transform, surface),
        }
    }

    /// `render_pattern_pixmap`: the tile is painted at the device scale.
    fn pattern(&mut self, pattern: &usvg::Pattern, context: IntRect, transform: Transform) {
        let (sx, sy) = transform.pre_concat(pattern.transform()).get_scale();
        let rect = pattern.rect();
        let tile = (
            (rect.width() * sx).round() as u32,
            (rect.height() * sy).round() as u32,
        );
        if tile.0 == 0 || tile.1 == 0 {
            return;
        }
        self.nodes(pattern.root(), context, Transform::from_scale(sx, sy), tile);
    }

    /// `render_group`.
    fn group(
        &mut self,
        group: &usvg::Group,
        context: IntRect,
        transform: Transform,
        surface: (u32, u32),
    ) {
        let transform = transform.pre_concat(group.transform());
        if !group.should_isolate() {
            self.nodes(group, context, transform, surface);
            return;
        }
        let Some((context, transform)) = layer_frame(group, context, transform) else {
            return;
        };
        let Some((layer, transform)) = group_layer(group, context, transform) else {
            return;
        };
        let size = (layer.width(), layer.height());
        let pixels = u64::from(size.0).saturating_mul(u64::from(size.1));
        if !group.filters().is_empty() {
            self.charge(pixels, pixels.saturating_mul(LAYER_PIXEL_WORK));
        }
        self.nodes(group, context, transform, size);
        for filter in group.filters() {
            self.filter(filter, transform, size);
        }
        // A mask paints its content into a pixmap of the layer's size; an
        // empty mask ends the chain.
        let mut mask = group.mask();
        while let Some(current) = mask {
            if current.root().children().is_empty() || self.exhausted() {
                break;
            }
            self.nodes(current.root(), context, transform, size);
            mask = current.mask();
        }
    }

    /// `filter::apply`: every primitive's input and result is the size of
    /// the filter region the layer `source` bounds, at most the layer's.
    fn filter(&mut self, filter: &usvg::filter::Filter, transform: Transform, source: (u32, u32)) {
        let Some(region) = filter.rect().transform(transform).map(|r| r.to_int_rect()) else {
            return;
        };
        let Some(source_rect) = IntRect::from_xywh(0, 0, source.0, source.1) else {
            return;
        };
        let Some(region) = fit_to_rect(region, source_rect) else {
            return;
        };
        // A region that does not fill the layer is copied out of it, and the
        // primitives work in the copy's coordinates.
        if region != source_rect {
            let copied = u64::from(region.width()).saturating_mul(u64::from(region.height()));
            self.charge(copied, copied.saturating_mul(REGION_COPY_PIXEL_WORK));
        }
        let transform = transform.post_translate(-region.x() as f32, -region.y() as f32);
        let pixels = u64::from(source.0).saturating_mul(u64::from(source.1));
        for primitive in filter.primitives() {
            if self.exhausted() {
                return;
            }
            let Some(subregion) = primitive
                .rect()
                .transform(transform)
                .map(|r| r.to_int_rect())
            else {
                // resvg abandons the whole filter on an invalid subregion.
                return;
            };
            let per_pixel = primitive_pixel_work(primitive.kind(), transform, source);
            self.charge(pixels, pixels.saturating_mul(per_pixel));
            if let Kind::Image(image) = primitive.kind() {
                let (sx, sy) = transform.get_scale();
                let Some(context) = IntRect::from_xywh(0, 0, region.width(), region.height())
                else {
                    continue;
                };
                self.nodes(
                    image.root(),
                    context,
                    Transform::from_row(
                        sx,
                        0.0,
                        0.0,
                        sy,
                        subregion.x() as f32,
                        subregion.y() as f32,
                    ),
                    (region.width(), region.height()),
                );
            }
        }
    }
}

/// The work units per pixel of one primitive, whose input and result are at
/// most `source` in size, under the filter's device `transform`.
fn primitive_pixel_work(kind: &Kind, transform: Transform, source: (u32, u32)) -> u64 {
    let own = match kind {
        Kind::GaussianBlur(blur) => blur_pixel_work(blur.std_dev_x(), blur.std_dev_y(), transform),
        Kind::DropShadow(shadow) => {
            blur_pixel_work(shadow.std_dev_x(), shadow.std_dev_y(), transform)
                .saturating_add(SIMPLE_PIXEL_WORK)
        }
        Kind::Turbulence(turbulence) => {
            // Four channels, each summing one noise evaluation per octave.
            u64::from(turbulence.num_octaves())
                .saturating_mul(4)
                .saturating_mul(TURBULENCE_OCTAVE_WORK)
        }
        Kind::Morphology(morphology) => {
            // resvg scans a `2 * round(r) + 1` neighbourhood per pixel,
            // clamped to the input, so its cost grows with the radius squared.
            let (sx, sy) = transform.get_scale();
            let (rx, ry) = (
                morphology.radius_x().get() * sx,
                morphology.radius_y().get() * sy,
            );
            if rx > 0.0 && ry > 0.0 {
                let span =
                    |radius: f32| (radius.round() as u32).saturating_mul(2).saturating_add(1);
                let columns = span(rx).min(source.0);
                let rows = span(ry).min(source.1);
                u64::from(columns)
                    .saturating_mul(u64::from(rows))
                    .saturating_mul(KERNEL_CELL_WORK)
            } else {
                0
            }
        }
        Kind::ConvolveMatrix(convolve) => {
            let matrix = convolve.matrix();
            let cells = u64::from(matrix.columns()).saturating_mul(u64::from(matrix.rows()));
            // usvg multiplies the order in `u32`; a product that wrapped
            // yields a kernel resvg would index out of bounds. The sanitizer
            // drops such an `order` first; should one reach a tree anyway,
            // never paint it.
            if u64::try_from(matrix.data().len()).ok() != Some(cells) {
                return u64::MAX;
            }
            cells.saturating_mul(KERNEL_CELL_WORK)
        }
        Kind::DiffuseLighting(_) | Kind::SpecularLighting(_) => LIGHTING_PIXEL_WORK,
        Kind::ComponentTransfer(_) => TRANSFER_PIXEL_WORK,
        Kind::Merge(merge) => u64::try_from(merge.inputs().len())
            .unwrap_or(u64::MAX)
            .saturating_mul(SIMPLE_PIXEL_WORK),
        Kind::ColorMatrix(_)
        | Kind::DisplacementMap(_)
        | Kind::Tile(_)
        | Kind::Blend(_)
        | Kind::Composite(_) => SIMPLE_PIXEL_WORK,
        Kind::Flood(_) | Kind::Offset(_) | Kind::Image(_) => 0,
    };
    PRIMITIVE_PIXEL_WORK.saturating_add(own)
}

/// The blur work per pixel resvg's `resolve_std_dev` selects for the device
/// deviations: none when both are zero, a box blur when either reaches 2,
/// otherwise the recursive filter.
fn blur_pixel_work(
    std_dev_x: usvg::PositiveF32,
    std_dev_y: usvg::PositiveF32,
    transform: Transform,
) -> u64 {
    let (sx, sy) = transform.get_scale();
    let (x, y) = (std_dev_x.get() * sx, std_dev_y.get() * sy);
    if x == 0.0 && y == 0.0 {
        0
    } else if x >= 2.0 || y >= 2.0 {
        BOX_BLUR_PIXEL_WORK
    } else {
        IIR_BLUR_PIXEL_WORK
    }
}

/// `render_group`'s layer frame: a filtered group's layer is painted without
/// the transform's rotation and skew (`rest`), which compositing the layer
/// back applies, so the filter context is carried into that frame. A layer
/// without filters keeps the device frame.
fn layer_frame(
    group: &usvg::Group,
    context: IntRect,
    transform: Transform,
) -> Option<(IntRect, Transform)> {
    let rest = if group.filters().is_empty() {
        Transform::identity()
    } else {
        // resvg takes the platform's `f32::hypot`, which is not correctly
        // rounded; the portable one keeps the charge the same on every
        // target, and the two can only part at a layer edge one ulp from an
        // integer.
        let sx = crate::portable_math::hypotf(transform.sx, transform.ky);
        let sy = crate::portable_math::hypotf(transform.kx, transform.sy);
        Transform::from_row(
            transform.sx / sx,
            transform.ky / sx,
            transform.kx / sy,
            transform.sy / sy,
            0.0,
            0.0,
        )
    };
    let inverse = rest.invert()?;
    let context = context.to_rect().transform(inverse)?.round_out()?;
    Some((context, inverse.pre_concat(transform)))
}

/// `render_layer`'s layer: the group's layer bounds on the device grid,
/// clamped to the filter context, and the transform content is painted into
/// the layer with.
fn group_layer(
    group: &usvg::Group,
    context: IntRect,
    transform: Transform,
) -> Option<(IntRect, Transform)> {
    let bbox = group.layer_bounding_box().transform(transform)?;
    let layer = if group.filters().is_empty() {
        IntRect::from_xywh(
            (bbox.x().floor() as i32).checked_sub(2)?,
            (bbox.y().floor() as i32).checked_sub(2)?,
            (bbox.width().ceil() as u32).checked_add(4)?,
            (bbox.height().ceil() as u32).checked_add(4)?,
        )?
    } else {
        IntRect::from_xywh(
            bbox.x().floor() as i32,
            bbox.y().floor() as i32,
            bbox.width().ceil().max(1.0) as u32,
            bbox.height().ceil().max(1.0) as u32,
        )?
    };
    let layer = fit_to_rect(layer, context)?;
    let mut dx = bbox.x();
    let mut dy = bbox.y();
    dx -= bbox.x() - layer.x() as f32;
    dy -= bbox.y() - layer.y() as f32;
    Some((
        layer,
        Transform::from_translate(-dx, -dy).pre_concat(transform),
    ))
}

/// resvg's filter context for a pixmap: five times its size, centred on it.
fn max_filter_bbox(width: u32, height: u32) -> Option<IntRect> {
    IntRect::from_xywh(
        i32::try_from(width).unwrap_or(i32::MAX).saturating_mul(-2),
        i32::try_from(height).unwrap_or(i32::MAX).saturating_mul(-2),
        width.saturating_mul(5),
        height.saturating_mul(5),
    )
    .or_else(|| IntRect::from_ltrb(i32::MIN / 2, i32::MIN / 2, i32::MAX / 2, i32::MAX / 2))
}

/// resvg's `fit_to_rect`: `rect` intersected with `bounds`.
fn fit_to_rect(rect: IntRect, bounds: IntRect) -> Option<IntRect> {
    IntRect::from_ltrb(
        rect.left().max(bounds.left()),
        rect.top().max(bounds.top()),
        rect.right().min(bounds.right()),
        rect.bottom().min(bounds.bottom()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(svg: &str) -> usvg::Tree {
        usvg::Tree::from_str(svg, &usvg::Options::default()).expect("fixture parses")
    }

    fn measure(svg: &str) -> FilterWork {
        measure_tree(&tree(svg), Transform::identity(), 10, 10, u64::MAX)
    }

    /// A 10x10 rectangle through a filter of `primitives` whose region is
    /// exactly the rectangle.
    fn filtered(primitives: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><filter id="f" x="0" y="0" width="1" height="1">{primitives}</filter><rect width="10" height="10" filter="url(#f)"/></svg>"#
        )
    }

    /// The work of the 100-pixel layer and one primitive costing `per_pixel`.
    const fn one_primitive(per_pixel: u64) -> u64 {
        100 * LAYER_PIXEL_WORK + 100 * (PRIMITIVE_PIXEL_WORK + per_pixel)
    }

    #[test]
    fn an_unfiltered_tree_costs_nothing() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><g opacity="0.5"><rect width="10" height="10"/></g></svg>"#;
        assert_eq!(measure(svg), FilterWork::default());
    }

    #[test]
    fn a_primitive_costs_its_result_over_the_layer() {
        let blur = filtered(r#"<feGaussianBlur stdDeviation="1"/>"#);
        // The layer and the blur's result, 100 pixels each; resvg blurs a
        // device deviation under 2 with its recursive filter.
        assert_eq!(
            measure(&blur),
            FilterWork {
                work_units: one_primitive(IIR_BLUR_PIXEL_WORK),
                surface_pixels: 200,
            }
        );
        assert_eq!(
            measure(&filtered(r#"<feGaussianBlur stdDeviation="3"/>"#)).work_units,
            one_primitive(BOX_BLUR_PIXEL_WORK)
        );
        assert_eq!(
            measure(&filtered(r#"<feGaussianBlur stdDeviation="0"/>"#)).work_units,
            one_primitive(0)
        );
        // At twice the device scale the same filter covers four times the pixels.
        let scaled = measure_tree(
            &tree(&blur),
            Transform::from_scale(2.0, 2.0),
            20,
            20,
            u64::MAX,
        );
        assert_eq!(scaled.surface_pixels, 800);
    }

    #[test]
    fn parameterized_primitives_cost_their_inner_loops() {
        assert_eq!(
            measure(&filtered(r#"<feTurbulence numOctaves="1000"/>"#)).work_units,
            one_primitive(4 * 1000 * TURBULENCE_OCTAVE_WORK)
        );
        // resvg clamps the morphology neighbourhood to the input, and rounds
        // the radius: 1.4 scans a 3x3 neighbourhood.
        assert_eq!(
            measure(&filtered(r#"<feMorphology radius="100000"/>"#)).work_units,
            one_primitive(10 * 10 * KERNEL_CELL_WORK)
        );
        assert_eq!(
            measure(&filtered(r#"<feMorphology radius="1.4"/>"#)).work_units,
            one_primitive(3 * 3 * KERNEL_CELL_WORK)
        );
        assert_eq!(
            measure(&filtered(
                r#"<feConvolveMatrix order="2 3" kernelMatrix="1 1 1 1 1 1"/>"#
            ))
            .work_units,
            one_primitive(6 * KERNEL_CELL_WORK)
        );
    }

    #[test]
    fn the_layer_is_clamped_to_resvgs_filter_context() {
        // A filter region a hundred times the pixmap is cut to five times its
        // size on each side, as resvg cuts it.
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><filter id="f" filterUnits="userSpaceOnUse" x="-500" y="-500" width="1000" height="1000"><feFlood/></filter><rect width="10" height="10" filter="url(#f)"/></svg>"#;
        assert_eq!(measure(svg).surface_pixels, 2 * 50 * 50);
    }

    #[test]
    fn a_filtered_layer_is_laid_out_without_its_rotation() {
        // resvg paints a filtered layer in the group's own frame and rotates
        // the result back, so a square rotated 45 degrees keeps a 10x10 layer
        // and result, not the 15x15 device box around them.
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><filter id="f" x="0" y="0" width="1" height="1"><feFlood/></filter><g transform="rotate(45 5 5)"><rect width="10" height="10" filter="url(#f)"/></g></svg>"#;
        assert_eq!(
            measure(svg),
            FilterWork {
                work_units: one_primitive(0),
                surface_pixels: 200,
            }
        );
    }

    #[test]
    fn a_region_short_of_its_layer_is_copied_out() {
        // The layer covers both filter regions; the second, half as wide, is
        // copied out of it before its flood runs.
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><filter id="a" x="0" y="0" width="1" height="1"><feFlood/></filter><filter id="b" x="0" y="0" width="0.5" height="1"><feFlood/></filter><rect width="10" height="10" filter="url(#a) url(#b)"/></svg>"#;
        assert_eq!(
            measure(svg),
            FilterWork {
                work_units: one_primitive(0)
                    + 50 * REGION_COPY_PIXEL_WORK
                    + 100 * PRIMITIVE_PIXEL_WORK,
                surface_pixels: 100 + 100 + 50 + 100,
            }
        );
    }

    #[test]
    fn the_walk_stops_past_its_budget() {
        let chain = filtered(&r#"<feGaussianBlur stdDeviation="1"/>"#.repeat(1000));
        let work = measure_tree(&tree(&chain), Transform::identity(), 10, 10, 100_000);
        assert!(work.work_units > 100_000);
        assert!(work.surface_pixels < 1000 * 100);
    }
}
