//! Text resolution: which text elements are live occurrences, the CJK subset
//! trigger evaluated over them, the official fonts they and the generated
//! labels are laid out in, and one `ResolvedTextElement` per surviving
//! occurrence.

use crate::color;
use crate::normalization::{NormalizedElement, NormalizedElementKind, NormalizedText};
use crate::relationships::{RelationshipKind, ResolvedRelationshipGraph};

use super::super::bound_text::has_reciprocal_bound_text;
use super::super::font_demand::ResolvedFont;
use super::super::text::{
    layout_with_loader, needs_bidi_layout, CjkSubsetTrigger, OfficialFont, ResolvedText,
    TextLayout, EXCALIFONT_FONT_FAMILY,
};
use super::super::try_owned;
use super::{
    label_error, relationship_element_target, FrameMembership, ResolveContext, ResolveSceneError,
    ResolvedTextElement,
};

/// Whether one text element is a live text occurrence, and if so its bound-text
/// container. Empty text and bound text whose container does not bind it back
/// are omitted and contribute nothing, neither glyphs nor font demand.
pub(in crate::render) fn live_text_occurrence(
    text_source: &NormalizedText,
    source_index: usize,
    relationships: &ResolvedRelationshipGraph,
) -> Option<Option<usize>> {
    if text_source.text.is_empty() {
        return None;
    }
    let container_source_index =
        relationship_element_target(source_index, RelationshipKind::Mask, relationships);
    if container_source_index
        .is_some_and(|container| !has_reciprocal_bound_text(container, source_index, relationships))
    {
        return None;
    }
    Some(container_source_index)
}

/// Evaluates the CJK subset trigger once for the scene, over every live
/// Excalifont occurrence's rendered `text`. That text is the format-neutral
/// carrier of the Excalifont font family scalar set: `originalText` only
/// survives normalization for SVG, and a wrapped `text` keeps every
/// non-whitespace scalar of its `originalText`. The outcome is carried on the
/// resolved scene so glyph selection and the SVG font demand share it, even
/// when local recovery later omits the occurrence that fired it.
pub(in crate::render) fn cjk_subset_trigger<'a>(
    sources: impl Iterator<Item = &'a NormalizedElement>,
    relationships: &ResolvedRelationshipGraph,
) -> CjkSubsetTrigger {
    CjkSubsetTrigger::evaluate(sources.filter_map(|source| {
        let NormalizedElementKind::Text(text_source) = &source.kind else {
            return None;
        };
        live_text_occurrence(text_source, source.source.source_index(), relationships)?;
        (text_source.font_family == EXCALIFONT_FONT_FAMILY).then_some(text_source.text.as_str())
    }))
}

/// The index in `official_fonts` of the font that shapes `font_family`, loading
/// it for `text` on first use.
pub(super) fn official_font_index(
    official_fonts: &mut Vec<OfficialFont>,
    font_family: i32,
    text: &str,
    loader: &mut crate::fonts::FontLoader,
) -> Result<usize, ResolveSceneError> {
    if let Some(index) = official_fonts
        .iter()
        .position(|font| font.matches_family(font_family))
    {
        return Ok(index);
    }
    official_fonts.try_reserve(1)?;
    official_fonts
        .push(OfficialFont::load_with_loader(font_family, text, loader).map_err(label_error)?);
    Ok(official_fonts.len() - 1)
}

/// `text` laid out in `official_fonts[font_index]`, then again through the
/// per-scalar fallback chain when that whole-text face misses a scalar or the
/// element needs UAX #9 layout (a right-to-left base direction or a
/// right-to-left run, spec 8.3).
pub(super) fn layout_official_text(
    official_fonts: &mut Vec<OfficialFont>,
    font_index: usize,
    text: &str,
    layout: TextLayout<'_>,
    cjk_subset_trigger: CjkSubsetTrigger,
    loader: &mut crate::fonts::FontLoader,
) -> Result<ResolvedText, ResolveSceneError> {
    let fallback_layout = TextLayout {
        width: layout.width,
        font_size: layout.font_size,
        line_height: layout.line_height,
        text_align: layout.text_align,
        font_family: layout.font_family,
        outline_limit: layout.outline_limit,
    };
    let mut laid_out = official_fonts[font_index]
        .layout(text, layout)
        .map_err(label_error)?;
    laid_out.font_index = font_index;
    if !laid_out.missing_codepoints.is_empty() || needs_bidi_layout(text) {
        laid_out = layout_with_loader(
            font_index,
            official_fonts,
            text,
            fallback_layout,
            cjk_subset_trigger,
            loader,
        )
        .map_err(label_error)?
        .0;
    }
    Ok(laid_out)
}

/// One text element as a live text occurrence, laid out in its official font,
/// or `None` when the occurrence is omitted: empty text, or bound text whose
/// container does not bind it back. A bound occurrence carries its container's
/// source order so it paints right after the container.
pub(super) fn resolve_text(
    source: &NormalizedElement,
    context: &ResolveContext<'_>,
    official_fonts: &mut Vec<OfficialFont>,
    membership: FrameMembership,
    loader: &mut crate::fonts::FontLoader,
) -> Result<Option<ResolvedTextElement>, ResolveSceneError> {
    let NormalizedElementKind::Text(text_source) = &source.kind else {
        unreachable!("only text elements reach text resolution");
    };
    let Some(container_source_index) = live_text_occurrence(
        text_source,
        source.source.source_index(),
        context.relationships,
    ) else {
        return Ok(None);
    };
    let container_source_order = container_source_index.and_then(|container| {
        let relationships = context.relationships;
        relationships
            .node_position(container)
            .and_then(|node| relationships.nodes.get(node))
            .map(|node| node.source_order)
    });
    let font_index = official_font_index(
        official_fonts,
        text_source.font_family,
        &text_source.text,
        loader,
    )?;
    let laid_out = layout_official_text(
        official_fonts,
        font_index,
        &text_source.text,
        TextLayout {
            width: source.width,
            font_size: text_source.font_size,
            line_height: text_source.line_height,
            text_align: &text_source.text_align,
            font_family: text_source.font_family,
            outline_limit: u64::from(
                context
                    .request
                    .resource_limits()
                    .max_geometry_segments_per_element(),
            ),
        },
        context.cjk_subset_trigger,
        loader,
    )?;
    let transform = context.box_transform(source);
    Ok(Some(ResolvedTextElement {
        source_order: source.source_order,
        source: source.source.try_clone()?,
        font_family: text_source.font_family,
        original_text: try_owned(&text_source.original_text)?,
        text: laid_out,
        frame_clips: membership.clips,
        transform,
        color: color::parse_css(&source.stroke_color)
            .ok_or(ResolveSceneError::InvalidColor)?
            .lowered(context.request.color_lowering()),
        opacity: source.opacity / 100.0 * membership.opacity,
        render: true,
        coordinate_failure: None,
        base_element_work_units: 0,
        container_source_order,
        link: source.link.as_deref().map(try_owned).transpose()?,
        width: source.width,
        height: source.height,
    }))
}

/// The official fonts the resolved elements still reference, as the fonts the
/// encoders embed.
pub(super) fn resolve_fonts(
    official_fonts: Vec<OfficialFont>,
) -> Result<Vec<ResolvedFont>, ResolveSceneError> {
    let mut resolved_fonts = Vec::new();
    resolved_fonts.try_reserve_exact(official_fonts.len())?;
    for font in official_fonts {
        let (identity, sfnt) = font.into_parts();
        resolved_fonts.push(ResolvedFont { identity, sfnt });
    }
    Ok(resolved_fonts)
}
