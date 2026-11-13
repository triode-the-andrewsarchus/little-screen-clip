//! Image resolution: one `ResolvedImage` drawn through its projection, crop
//! and corner clip when the census resolved the asset, or one
//! `ResolvedImageFallback` for the normalized status it recorded instead.

use std::sync::Arc;

use crate::normalization::{NormalizedElement, NormalizedElementKind};

use super::{
    DarkModeImageFilter, FrameMembership, ResolveContext, ResolveSceneError, ResolvedElement,
    ResolvedImage, ResolvedImageCornerClip, ResolvedImageFallback, ResolvedImageInputs,
    ResolvedImageProjection, ResolvedImageSourceRect,
};

/// One image element: the asset drawn through its projection, crop and corner
/// clip when the census resolved one, the compatibility placeholder for its
/// normalized status when the census recorded that instead, and nothing when
/// it has neither.
pub(super) fn resolve_image(
    source: NormalizedElement,
    images: &ResolvedImageInputs,
    context: &ResolveContext<'_>,
    membership: FrameMembership,
) -> Result<Option<ResolvedElement>, ResolveSceneError> {
    let NormalizedElementKind::Image(image) = &source.kind else {
        unreachable!("only image elements reach image resolution");
    };
    if let Some(asset) = images.assets.get(&source.source.source_index()) {
        let source_rect = ResolvedImageSourceRect::for_image(asset, image.crop);
        let transform = context.box_transform(&source);
        return Ok(Some(ResolvedElement::Image(ResolvedImage {
            source_order: source.source_order,
            source: source.source,
            dark_mode_filter: DarkModeImageFilter::for_image(context.request.color_mode(), asset),
            asset: Arc::clone(asset),
            frame_clips: membership.clips,
            corner_clip: ResolvedImageCornerClip::for_image(
                source.width,
                source.height,
                source.roundness,
            )?,
            transform,
            opacity: source.opacity / 100.0 * membership.opacity,
            source_rect,
            projection: ResolvedImageProjection::for_image(source.width, source.height, image.crop),
            scale: image.scale,
            link: source.link,
            width: source.width,
            height: source.height,
        })));
    }
    let Some((_, status)) = images
        .fallbacks
        .iter()
        .find(|(source_index, _)| *source_index == source.source.source_index())
    else {
        return Ok(None);
    };
    let transform = context.box_transform(&source);
    Ok(Some(ResolvedElement::ImageFallback(
        ResolvedImageFallback {
            source_order: source.source_order,
            source: source.source,
            status: *status,
            frame_clips: membership.clips,
            transform,
            opacity: source.opacity / 100.0 * membership.opacity,
            width: source.width,
            height: source.height,
        },
    )))
}
