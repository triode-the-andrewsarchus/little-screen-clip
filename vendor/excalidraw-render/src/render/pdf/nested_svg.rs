//! Nested SVG in PDF: the planning analysis that finds every filter occurrence
//! in a sanitized `usvg` tree and turns the ones the asset's effect census
//! attributes into `PdfRasterClosure`s — the smallest page regions the raster
//! fallback must paint — and the choice of the tree each resource is drawn
//! from, which `svg_draw` draws where each element shows it (ADR-0021).

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::portable_math::sin_cos;
use crate::{FiniteValue, LogicalBounds, NestedPath, NestedPathSegment, NestedSegmentKind};

use super::super::asset::svg::EffectCause;
use super::super::resolve::{ResolvedImageAsset, ResolvedImageProjection, ResolvedTransform};
use super::super::{FormatEncodeError, SvgTreeCache};
use super::raster::{Matrix, RasterPlacement};
use super::{
    pdf_size_error, visible_rect, PdfElement, PdfElementKind, PdfRasterClosure, PdfSvgResource,
};

#[derive(Debug)]
struct FilterOccurrence {
    filter_id: String,
    usage_id: String,
    marker: String,
    bounds: (f64, f64, f64, f64),
    input_bounds: (f64, f64, f64, f64),
    parent_transform: Matrix,
    parent: Option<usize>,
    /// A repeating texture is clipped to its own tile viewport, not to the
    /// location of the base tile within the consumer's scene crop.
    texture_consumer: Option<(f64, f64, f64, f64)>,
}

pub(super) fn collect_raster_closures(
    elements: &[PdfElement],
    resources: &[PdfSvgResource],
    final_scale: f64,
    logical_width: f64,
    logical_height: f64,
) -> Result<Vec<PdfRasterClosure>, FormatEncodeError> {
    let mut closures: Vec<PdfRasterClosure> = Vec::new();
    for element in elements {
        let PdfElement {
            frame_clips,
            transform,
            width,
            height,
            kind:
                PdfElementKind::Image {
                    source_order,
                    source,
                    asset,
                    source_rect,
                    projection,
                    scale,
                    ..
                },
            ..
        } = element
        else {
            continue;
        };
        if element.opacity == 0.0 {
            continue;
        }
        let Some(prepared) = resources
            .iter()
            .find(|resource| resource.source_index == source.source_index())
            .and_then(|resource| resource.prepared.as_ref())
        else {
            continue;
        };
        let tree = &prepared.tree;
        let mut occurrences = Vec::new();
        collect_filter_occurrences(
            tree.root(),
            None,
            None,
            Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]),
            None,
            &mut occurrences,
        );
        let mut occurrence_closures: Vec<Option<usize>> = Vec::new();
        occurrence_closures.resize(occurrences.len(), None);
        let mut matched_occurrences = BTreeMap::<String, usize>::new();
        for (occurrence_index, occurrence) in occurrences.iter().enumerate() {
            let exact_usage = asset.svg_effects.iter().find(|effect| {
                !occurrence.usage_id.is_empty()
                    && effect.usage_id == prepared.original_usage(&occurrence.usage_id)
            });
            let candidates = asset
                .svg_effects
                .iter()
                .filter(|effect| {
                    effect.marker == occurrence.marker || effect.filter_id == occurrence.filter_id
                })
                .collect::<Vec<_>>();
            let key = if occurrence.marker.is_empty() {
                occurrence.filter_id.clone()
            } else {
                occurrence.marker.clone()
            };
            let matched = matched_occurrences.entry(key).or_default();
            let effect = exact_usage.or_else(|| {
                (!candidates.is_empty()).then(|| candidates[*matched % candidates.len()])
            });
            *matched = matched.saturating_add(1);
            let Some(effect) = effect else {
                return Err(FormatEncodeError::Encoding(
                    "PDF effect lost its source provenance".to_owned(),
                ));
            };
            if let Some(parent_closure) = occurrence
                .parent
                .and_then(|parent| occurrence_closures.get(parent).copied().flatten())
            {
                let closure = closures
                    .get_mut(parent_closure)
                    .ok_or_else(pdf_size_error)?;
                closure
                    .causes
                    .extend(effect.causes.iter().map(effect_cause_path));
                closure.unisolatable |= effect.unisolatable;
                occurrence_closures[occurrence_index] = Some(parent_closure);
                continue;
            }
            let (x, y, right, bottom) = occurrence.texture_consumer.unwrap_or(occurrence.bounds);
            let normalized_x = f64::from(asset.width) / f64::from(tree.size().width());
            let normalized_y = f64::from(asset.height) / f64::from(tree.size().height());
            let (x, y, right, bottom) = (
                x * normalized_x,
                y * normalized_y,
                right * normalized_x,
                bottom * normalized_y,
            );
            let viewport_right = f64::from(asset.width);
            let viewport_bottom = f64::from(asset.height);
            let x = x.max(0.0).max(source_rect.x);
            let y = y.max(0.0).max(source_rect.y);
            let right = right
                .min(viewport_right)
                .min(source_rect.x + source_rect.width);
            let bottom = bottom
                .min(viewport_bottom)
                .min(source_rect.y + source_rect.height);
            if right <= x || bottom <= y {
                continue;
            }
            let (cx, cy, cw, ch) = projection.content_rect();
            let sx = f64::from(scale[0]);
            let sy = f64::from(scale[1]);
            let asset_to_local = Matrix([
                sx * cw / f64::from(tree.size().width()),
                0.0,
                0.0,
                sy * ch / f64::from(tree.size().height()),
                if sx < 0.0 { *width - cx } else { cx },
                if sy < 0.0 { *height - cy } else { cy },
            ]);
            let local_to_scene = scene_matrix(*transform);
            let local = map_asset_bounds(
                (x, y, right, bottom),
                projection,
                *width,
                *height,
                asset,
                *scale,
            );
            let (x, y, right, bottom) = transform_bounds(local, *transform);
            let mut x = x.max(0.0);
            let mut y = y.max(0.0);
            let mut right = right.min(logical_width);
            let mut bottom = bottom.min(logical_height);
            for clip in frame_clips {
                // Frame exports use straight rectangular clips, which contain
                // no cubic segments. Both clip shapes retain the frame bounds.
                let clip_bounds = (0.0, 0.0, clip.width, clip.height);
                let clip_bounds = transform_bounds(clip_bounds, clip.transform);
                x = x.max(clip_bounds.0);
                y = y.max(clip_bounds.1);
                right = right.min(clip_bounds.2);
                bottom = bottom.min(clip_bounds.3);
            }
            if right <= x || bottom <= y {
                continue;
            }
            // Visibility follows the consumer, but a repeating dependency must
            // retain a complete tile. A crop can expose a later repetition even
            // when the base tile lies wholly outside the crop or the page.
            if occurrence.texture_consumer.is_some() {
                (x, y, right, bottom) = local_to_scene
                    .concat(asset_to_local)
                    .bounds(occurrence.bounds);
            }
            let pixel_x = (x * final_scale).floor();
            let pixel_y = (y * final_scale).floor();
            let pixel_right = (right * final_scale).ceil();
            let pixel_bottom = (bottom * final_scale).ceil();
            let pixel_width = (pixel_right - pixel_x).max(1.0) as u64;
            let pixel_height = (pixel_bottom - pixel_y).max(1.0) as u64;
            let logical_bounds = LogicalBounds {
                x: FiniteValue::new(pixel_x / final_scale).ok_or_else(pdf_size_error)?,
                y: FiniteValue::new(pixel_y / final_scale).ok_or_else(pdf_size_error)?,
                width: FiniteValue::new(pixel_width as f64 / final_scale)
                    .ok_or_else(pdf_size_error)?,
                height: FiniteValue::new(pixel_height as f64 / final_scale)
                    .ok_or_else(pdf_size_error)?,
            };
            let device = Matrix([final_scale, 0.0, 0.0, final_scale, 0.0, 0.0])
                .concat(local_to_scene)
                .concat(asset_to_local);
            let input = device.bounds(occurrence.input_bounds);
            let input_x = input.0.floor();
            let input_y = input.1.floor();
            let input_width = (input.2.ceil() - input_x).max(1.0) as u64;
            let input_height = (input.3.ceil() - input_y).max(1.0) as u64;
            let causes = effect
                .causes
                .iter()
                .map(effect_cause_path)
                .collect::<Vec<_>>();
            closures.push(PdfRasterClosure {
                source_index: source.source_index(),
                raster: RasterPlacement {
                    node_id: occurrence.usage_id.clone(),
                    parent_to_device: device.concat(occurrence.parent_transform),
                    output_x: pixel_x,
                    output_y: pixel_y,
                    input_x,
                    input_y,
                    input_width,
                    input_height,
                },
                element_id: source.id().to_owned(),
                json_pointer: asset.svg_fallback_pointer.clone().unwrap_or_default(),
                source_order: *source_order,
                source_position: effect.node_ordinal,
                logical_bounds,
                pixel_width,
                pixel_height,
                causes,
                unisolatable: effect.unisolatable,
            });
            occurrence_closures[occurrence_index] = Some(closures.len() - 1);
        }
    }
    Ok(closures)
}

fn collect_filter_occurrences(
    group: &usvg::Group,
    parent: Option<usize>,
    ancestor_clip: Option<(f64, f64, f64, f64)>,
    space: Matrix,
    texture_consumer: Option<(f64, f64, f64, f64)>,
    occurrences: &mut Vec<FilterOccurrence>,
) -> Option<usize> {
    let Ok(inverse) = Matrix::from_skia(group.transform()).inverse() else {
        return parent;
    };
    let mut ancestor_clip = ancestor_clip;
    if let Some(clip) = group.clip_path() {
        let Some(bounds) = nested_clip_bounds(clip) else {
            return parent;
        };
        let bounds = space
            .concat(Matrix::from_skia(group.abs_transform()))
            .bounds(bounds);
        ancestor_clip = Some(ancestor_clip.map_or(bounds, |a| {
            (
                a.0.max(bounds.0),
                a.1.max(bounds.1),
                a.2.min(bounds.2),
                a.3.min(bounds.3),
            )
        }));
    }
    let mut child_parent = parent;
    if !group.filters().is_empty()
        && group.has_children()
        && group.bounding_box().width() > 0.0
        && group.bounding_box().height() > 0.0
    {
        let rect = group.abs_layer_bounding_box();
        let input_bounds = space.bounds((
            f64::from(rect.x()),
            f64::from(rect.y()),
            f64::from(rect.right()),
            f64::from(rect.bottom()),
        ));
        let mut bounds = input_bounds;
        if let Some(clip_bounds) = ancestor_clip {
            bounds.0 = bounds.0.max(clip_bounds.0);
            bounds.1 = bounds.1.max(clip_bounds.1);
            bounds.2 = bounds.2.min(clip_bounds.2);
            bounds.3 = bounds.3.min(clip_bounds.3);
            if bounds.2 <= bounds.0 || bounds.3 <= bounds.1 {
                return parent;
            }
        }
        for filter in group.filters() {
            occurrences.push(FilterOccurrence {
                filter_id: filter.id().to_owned(),
                usage_id: group.id().to_owned(),
                marker: filter
                    .primitives()
                    .first()
                    .map(|primitive| primitive.result().to_owned())
                    .unwrap_or_default(),
                bounds,
                input_bounds,
                parent_transform: space
                    .concat(Matrix::from_skia(group.abs_transform()))
                    .concat(inverse),
                parent: child_parent,
                texture_consumer,
            });
            child_parent = Some(occurrences.len() - 1);
        }
    }
    for child in group.children() {
        if let usvg::Node::Group(child) = child {
            collect_filter_occurrences(
                child,
                child_parent,
                ancestor_clip,
                space,
                texture_consumer,
                occurrences,
            );
        }
        if let usvg::Node::Path(path) = child {
            for paint in path
                .fill()
                .map(|p| p.paint())
                .into_iter()
                .chain(path.stroke().map(|p| p.paint()))
            {
                if let usvg::Paint::Pattern(pattern) = paint {
                    let b = path.abs_stroke_bounding_box();
                    let mut consumer = space.bounds((
                        f64::from(b.x()),
                        f64::from(b.y()),
                        f64::from(b.right()),
                        f64::from(b.bottom()),
                    ));
                    if let Some(clip) = ancestor_clip {
                        consumer = (
                            consumer.0.max(clip.0),
                            consumer.1.max(clip.1),
                            consumer.2.min(clip.2),
                            consumer.3.min(clip.3),
                        );
                    }
                    if consumer.2 <= consumer.0 || consumer.3 <= consumer.1 {
                        continue;
                    }
                    let tile_space = space
                        .concat(Matrix::from_skia(path.abs_transform()))
                        .concat(Matrix::from_skia(pattern.transform()))
                        .concat(Matrix::translate(
                            f64::from(pattern.rect().x()),
                            f64::from(pattern.rect().y()),
                        ));
                    let tile = pattern.rect();
                    let tile_bounds = tile_space.bounds((
                        0.0,
                        0.0,
                        f64::from(tile.width()),
                        f64::from(tile.height()),
                    ));
                    collect_filter_occurrences(
                        pattern.root(),
                        child_parent,
                        Some(tile_bounds),
                        tile_space,
                        Some(texture_consumer.unwrap_or(consumer)),
                        occurrences,
                    );
                }
            }
        }
    }
    // Resource transforms are relative to this consumer, not to the SVG root.
    let resource_space = space.concat(Matrix::from_skia(group.abs_transform()));
    let mut mask = group.mask();
    while let Some(current) = mask {
        collect_filter_occurrences(
            current.root(),
            child_parent,
            ancestor_clip,
            resource_space,
            texture_consumer,
            occurrences,
        );
        mask = current.mask();
    }
    for filter in group.filters() {
        for primitive in filter.primitives() {
            if let usvg::filter::Kind::Image(image) = primitive.kind() {
                collect_filter_occurrences(
                    image.root(),
                    child_parent,
                    ancestor_clip,
                    resource_space,
                    texture_consumer,
                    occurrences,
                );
            }
        }
    }
    child_parent
}

fn nested_clip_bounds(clip: &usvg::ClipPath) -> Option<(f64, f64, f64, f64)> {
    // usvg does not populate Group's cached layer bounds for clip subroots.
    // Measure their actual paths, including straight rectangular viewport clips.
    fn measure(group: &usvg::Group, transform: Matrix) -> Option<(f64, f64, f64, f64)> {
        let transform = transform.concat(Matrix::from_skia(group.transform()));
        let mut bounds: Option<(f64, f64, f64, f64)> = None;
        for child in group.children() {
            let next = match child {
                usvg::Node::Path(path) => path.data().compute_tight_bounds().map(|r| {
                    transform.bounds((
                        f64::from(r.x()),
                        f64::from(r.y()),
                        f64::from(r.right()),
                        f64::from(r.bottom()),
                    ))
                }),
                usvg::Node::Group(group) => measure(group, transform),
                _ => None,
            };
            if let Some(n) = next {
                bounds = Some(bounds.map_or(n, |b| {
                    (b.0.min(n.0), b.1.min(n.1), b.2.max(n.2), b.3.max(n.3))
                }));
            }
        }
        bounds
    }
    let mut bounds = measure(clip.root(), Matrix::from_skia(clip.transform()))?;
    if let Some(parent) = clip.clip_path() {
        let parent = nested_clip_bounds(parent)?;
        bounds.0 = bounds.0.max(parent.0);
        bounds.1 = bounds.1.max(parent.1);
        bounds.2 = bounds.2.min(parent.2);
        bounds.3 = bounds.3.min(parent.3);
        if bounds.2 <= bounds.0 || bounds.3 <= bounds.1 {
            return None;
        }
    }
    Some(bounds)
}

/// Maps asset-pixel bounds into the element's space the way the content
/// matrix draws the asset: through the projection's content rectangle,
/// mirrored about the element's unrounded center when flipped, and bounded by
/// the visible rectangle.
fn map_asset_bounds(
    bounds: (f64, f64, f64, f64),
    projection: &ResolvedImageProjection,
    width: f64,
    height: f64,
    asset: &ResolvedImageAsset,
    scale: [i8; 2],
) -> (f64, f64, f64, f64) {
    let (content_x, content_y, content_width, content_height) = projection.content_rect();
    let map_x = |value: f64| {
        let value = content_x + value / f64::from(asset.width) * content_width;
        if scale[0] < 0 {
            width - value
        } else {
            value
        }
    };
    let map_y = |value: f64| {
        let value = content_y + value / f64::from(asset.height) * content_height;
        if scale[1] < 0 {
            height - value
        } else {
            value
        }
    };
    let (x1, x2) = (map_x(bounds.0), map_x(bounds.2));
    let (y1, y2) = (map_y(bounds.1), map_y(bounds.3));
    let (visible_x, visible_y, visible_width, visible_height) =
        visible_rect(projection, width, height, scale);
    (
        x1.min(x2).max(visible_x),
        y1.min(y2).max(visible_y),
        x1.max(x2).min(visible_x + visible_width),
        y1.max(y2).min(visible_y + visible_height),
    )
}

fn transform_bounds(
    bounds: (f64, f64, f64, f64),
    transform: ResolvedTransform,
) -> (f64, f64, f64, f64) {
    scene_matrix(transform).bounds(bounds)
}
fn scene_matrix(transform: ResolvedTransform) -> Matrix {
    let (sin, cos) = sin_cos(transform.angle_radians);
    let offset_x = transform.translation_x + transform.center_x - cos * transform.center_x
        + sin * transform.center_y;
    let offset_y = transform.translation_y + transform.center_y
        - sin * transform.center_x
        - cos * transform.center_y;
    Matrix([cos, sin, -sin, cos, offset_x, offset_y])
}

fn effect_cause_path(cause: &EffectCause) -> NestedPath {
    let mut segments = Vec::new();
    for (node, attribute) in &cause.slots {
        segments.push(NestedPathSegment {
            kind: NestedSegmentKind::Node,
            ordinal: *node,
            name: None,
        });
        segments.push(NestedPathSegment {
            kind: NestedSegmentKind::Attribute,
            ordinal: *attribute,
            name: Some("href".to_owned()),
        });
    }
    segments.push(NestedPathSegment {
        kind: NestedSegmentKind::Node,
        ordinal: cause.node_ordinal,
        name: None,
    });
    if let Some((rule, declaration)) = cause.css {
        segments.push(NestedPathSegment {
            kind: NestedSegmentKind::CssRule,
            ordinal: rule,
            name: None,
        });
        segments.push(NestedPathSegment {
            kind: NestedSegmentKind::CssDeclaration,
            ordinal: declaration,
            name: Some("filter".to_owned()),
        });
    }
    if let Some((ordinal, name)) = &cause.attribute {
        segments.push(NestedPathSegment {
            kind: if cause.effect_input {
                NestedSegmentKind::EffectInput
            } else {
                NestedSegmentKind::Attribute
            },
            ordinal: *ordinal,
            name: Some(name.clone()),
        });
    }
    NestedPath { segments }
}

/// The tree each nested-SVG resource is drawn from, in resource order: the
/// sanitized tree itself, or, when the resource has raster closures, the tree
/// `PreparedSvg::lower` makes with each closure replaced by its pixels, so
/// Krilla receives no filter to rasterize on its own (ADR-0009).
pub(super) fn svg_draw_trees(
    resources: &[PdfSvgResource],
    svg_trees: &SvgTreeCache,
    closures: &[PdfRasterClosure],
) -> Result<Vec<Arc<usvg::Tree>>, FormatEncodeError> {
    let mut trees = Vec::new();
    trees
        .try_reserve_exact(resources.len())
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "format_resource",
            subject: "pdf_svg_trees",
        })?;
    for resource in resources {
        let original = svg_trees.get(&resource.asset.identity).ok_or_else(|| {
            FormatEncodeError::Encoding("PDF SVG resource lost its sanitized tree".to_owned())
        })?;
        let tree = match resource.prepared.as_ref() {
            Some(prepared) => {
                let selected: Vec<_> = closures
                    .iter()
                    .filter(|closure| closure.source_index == resource.source_index)
                    .collect();
                Arc::new(prepared.lower(&selected)?)
            }
            None => Arc::clone(original),
        };
        trees.push(tree);
    }
    Ok(trees)
}
