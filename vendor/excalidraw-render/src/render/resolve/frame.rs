//! Frame resolution: the frame names the reference synthesizes before it
//! measures the scene, the semantic frame sources their children's clips and
//! opacity resolve from, and one `ResolvedFrame` per emitted frame with its
//! name and outline.

use crate::color::{CanonicalColor, ColorLowering};
use crate::normalization::{NormalizedElement, NormalizedElementKind};
use crate::ColorMode;

use super::super::geometry::GeneratedGeometry;
use super::super::text::{FallbackFont, FrameName, FRAME_NAME_BOX_HEIGHT, FRAME_NAME_OFFSET_Y};
use super::{
    label_error, FrameMembership, ResolveContext, ResolveSceneError, ResolvedFrame,
    ResolvedFrameName, ResolvedFrameOutline, SemanticFrameSource,
};

/// `FRAME_STYLE.nameColorLightTheme` and `nameColorDarkTheme`: the frame name
/// is a text element whose stroke color the export picks by theme, and the
/// dark one then passes through the dark-mode filter like every other color.
const FRAME_NAME_COLOR_LIGHT: CanonicalColor = CanonicalColor::from_rgb8(0x99, 0x99, 0x99);
const FRAME_NAME_COLOR_DARK: CanonicalColor = CanonicalColor::from_rgb8(0x7a, 0x7a, 0x7a);

/// The visible name of every frame, shaped before the scene is measured: the
/// reference synthesizes each name as a text element
/// (`addFrameLabelsAsTextElements`), so the names take part in the bounds.
/// Keyed by the frame's source index; a frame with no name shows `Frame` or
/// `AI Frame`.
pub(super) fn resolve_frame_names(
    outlines: &[(NormalizedElement, Option<GeneratedGeometry>)],
    fallback_font: Option<&FallbackFont>,
) -> Result<Vec<(usize, FrameName)>, ResolveSceneError> {
    let mut frame_names = Vec::new();
    for (source, _) in outlines {
        let (NormalizedElementKind::Frame(frame) | NormalizedElementKind::MagicFrame(frame)) =
            &source.kind
        else {
            continue;
        };
        let title = frame.name.as_deref().unwrap_or({
            if matches!(source.kind, NormalizedElementKind::MagicFrame(_)) {
                "AI Frame"
            } else {
                "Frame"
            }
        });
        let font = fallback_font.ok_or(ResolveSceneError::InvalidFallbackFont)?;
        let name = font
            .resolve_frame_name(title, source.width)
            .map_err(label_error)?;
        frame_names.try_reserve(1)?;
        frame_names.push((source.source.source_index(), name));
    }
    Ok(frame_names)
}

/// Every frame in the scene as the source its children's clips and opacity
/// resolve from.
pub(super) fn semantic_frame_sources(
    outlines: &[(NormalizedElement, Option<GeneratedGeometry>)],
) -> Result<Vec<SemanticFrameSource>, ResolveSceneError> {
    let mut semantic_frames = Vec::new();
    semantic_frames.try_reserve(outlines.len())?;
    for (source, _) in outlines {
        if matches!(
            source.kind,
            NormalizedElementKind::Frame(_) | NormalizedElementKind::MagicFrame(_)
        ) {
            semantic_frames.push(SemanticFrameSource {
                source_index: source.source.source_index(),
                x: source.x,
                y: source.y,
                width: source.width,
                height: source.height,
                angle: source.angle,
                opacity: source.opacity / 100.0,
            });
        }
    }
    Ok(semantic_frames)
}

/// `FRAME_STYLE.nameColorLightTheme` or `nameColorDarkTheme` for the theme,
/// then lowered like every other color the reference paints under its
/// dark-mode filter. SVG keeps the dark-theme color unlowered under its root
/// filter, as the reference SVG does.
fn frame_name_color(theme: ColorMode, lowering: ColorLowering) -> CanonicalColor {
    match theme {
        ColorMode::Light => FRAME_NAME_COLOR_LIGHT,
        ColorMode::Dark => FRAME_NAME_COLOR_DARK,
    }
    .lowered(lowering)
}

/// One frame as a scene export emits it — with the synthesized name taken out
/// of `frame_names` and the outline the effective frame rendering asks for —
/// or `None` when frame elements are not emitted. Its clips and opacity are
/// those of the frame it is itself a child of.
pub(super) fn resolve_frame(
    source: NormalizedElement,
    context: &ResolveContext<'_>,
    membership: FrameMembership,
    frame_names: &mut Vec<(usize, FrameName)>,
) -> Result<Option<ResolvedFrame>, ResolveSceneError> {
    if !(context.frame_rendering.enabled && context.frame_rendering.emit_frame_elements) {
        return Ok(None);
    }
    let name = frame_names
        .iter()
        .position(|(index, _)| *index == source.source.source_index())
        .map(|position| frame_names.swap_remove(position).1)
        .map(|name| ResolvedFrameName {
            transform: context.transform(
                source.x,
                source.y - FRAME_NAME_OFFSET_Y - FRAME_NAME_BOX_HEIGHT,
                0.0,
                name.width / 2.0,
                FRAME_NAME_BOX_HEIGHT / 2.0,
            ),
            color: frame_name_color(
                context.request.color_mode(),
                context.request.color_lowering(),
            ),
            label: name.label,
        });
    let outline = context
        .frame_rendering
        .outline
        .then(|| {
            ResolvedFrameOutline::new(
                source.width,
                source.height,
                matches!(source.kind, NormalizedElementKind::MagicFrame(_)),
                context.request.color_mode(),
                context.request.color_lowering(),
            )
        })
        .transpose()?;
    let transform = context.box_transform(&source);
    Ok(Some(ResolvedFrame {
        source_order: source.source_order,
        source: source.source,
        name,
        outline,
        frame_clips: membership.clips,
        transform,
        opacity: source.opacity / 100.0 * membership.opacity,
        link: source.link,
        width: source.width,
        height: source.height,
    }))
}
