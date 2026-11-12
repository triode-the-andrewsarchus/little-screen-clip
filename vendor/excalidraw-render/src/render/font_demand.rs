use crate::resource_accounting::ResourceContext;
use crate::{RenderError, RenderRequest};

use super::resolve::{ResolveSceneError, ResolvedElement, ResolvedScene, ResolvedTextElement};
use super::{svg_fonts, text, FormatEncodeError, RenderResourceAccount, RenderSession};

/// One official face the PDF embeds complete: its catalog identity, which
/// names its family notice and its reviewed program's subset tag, its PDF
/// name, and the sfnt program itself.
#[derive(Debug)]
pub(super) struct ResolvedFont {
    pub(super) identity: &'static str,
    #[cfg_attr(not(feature = "pdf"), allow(dead_code))]
    pub(super) sfnt: std::sync::Arc<[u8]>,
}

pub(super) fn svg_font_text_order(
    scene: &ResolvedScene,
) -> Result<Vec<&ResolvedTextElement>, FormatEncodeError> {
    let mut ordinary = Vec::new();
    ordinary.try_reserve(scene.elements.len()).map_err(|_| {
        FormatEncodeError::MemoryUnavailable {
            subject_kind: "svg_font_demand",
            subject: "ordinary_text_order",
        }
    })?;
    ordinary.extend(scene.elements.iter().filter_map(|element| match element {
        ResolvedElement::Text(text) => Some(text),
        _ => None,
    }));
    ordinary.sort_by_key(|text| text.source_order);
    Ok(ordinary)
}

pub(super) fn svg_font_demand(
    scene: &ResolvedScene,
    text_order: &[&ResolvedTextElement],
) -> Result<svg_fonts::Demand, FormatEncodeError> {
    let mut families = Vec::<svg_fonts::FamilyDemand>::new();
    visit_svg_font_text(scene, text_order, |family, text| {
        add_svg_font_text(&mut families, family, text)
    })?;
    // The CJK subset trigger was evaluated once for the scene, so Xiaolai is
    // declared exactly when glyph selection could have shaped a run with it.
    if scene.cjk_subset_trigger.selects_xiaolai() {
        if let Some(excalifont) = families.iter().find(|family| family.family == "Excalifont") {
            let xiaolai = svg_fonts::FamilyDemand::from_scalars("Xiaolai", &excalifont.scalars)
                .map_err(|_| FormatEncodeError::MemoryUnavailable {
                    subject_kind: "svg_font_demand",
                    subject: "xiaolai_scalar_set",
                })?;
            families
                .try_reserve(1)
                .map_err(|_| FormatEncodeError::MemoryUnavailable {
                    subject_kind: "svg_font_demand",
                    subject: "xiaolai_family",
                })?;
            families.insert(0, xiaolai);
        }
    }
    // The Noto Emoji fallback is demanded last, after every resolved family,
    // by exactly the scalars glyph face selection shaped with it (#150): the
    // runs already record which face each scalar took, so the subset covers
    // what the emoji `<tspan>`s show and nothing a family face mapped.
    visit_noto_emoji_runs(scene, text_order, |text| {
        add_svg_font_text(&mut families, "Noto Emoji", text)
    })?;
    Ok(svg_fonts::Demand { families })
}

/// The number of families one SVG demand can name: the eight resolved
/// families, Xiaolai when the CJK subset trigger selects it, and Noto Emoji
/// when an emoji run selected it.
pub(super) const MAX_DEMAND_FAMILIES: usize = 10;

pub(super) fn svg_font_demand_estimates(
    scene: &ResolvedScene,
    text_order: &[&ResolvedTextElement],
) -> Result<[(&'static str, usize); MAX_DEMAND_FAMILIES], FormatEncodeError> {
    const UNICODE_WORDS: usize = 0x11_0000_usize.div_ceil(u64::BITS as usize);
    let mut family_order = [None; 8];
    let mut family_count = 0_usize;
    visit_svg_font_text(scene, text_order, |family, text| {
        if text.is_empty() {
            return Ok(());
        }
        if !family_order[..family_count].contains(&Some(family)) {
            let slot =
                family_order
                    .get_mut(family_count)
                    .ok_or(FormatEncodeError::MemoryUnavailable {
                        subject_kind: "svg_font_demand",
                        subject: "family_count",
                    })?;
            *slot = Some(family);
            family_count += 1;
        }
        Ok(())
    })?;
    let xiaolai_selected = scene.cjk_subset_trigger.selects_xiaolai()
        && family_order[..family_count].contains(&Some("Excalifont"));

    let mut estimates = [("", 0_usize); MAX_DEMAND_FAMILIES];
    let mut seen = [0_u64; UNICODE_WORDS];
    // Slot 0 is reserved for Xiaolai when the CJK subset trigger selects it; at
    // most eight families follow, and Noto Emoji takes the slot after them.
    let slots = estimates.iter_mut().skip(usize::from(xiaolai_selected));
    for (slot, family) in slots.zip(family_order.into_iter().flatten()) {
        seen.fill(0);
        let mut unique = 0_usize;
        visit_svg_font_text(scene, text_order, |candidate, text| {
            if candidate != family {
                return Ok(());
            }
            count_unique_scalars(&mut seen, &mut unique, text)
        })?;
        *slot = (family, unique);
    }
    if xiaolai_selected {
        let excalifont = estimates
            .iter()
            .find(|(family, _)| *family == "Excalifont")
            .map(|(_, count)| *count)
            .unwrap_or_default();
        estimates[0] = ("Xiaolai", excalifont);
    }
    seen.fill(0);
    let mut emoji_unique = 0_usize;
    visit_noto_emoji_runs(scene, text_order, |text| {
        count_unique_scalars(&mut seen, &mut emoji_unique, text)
    })?;
    if emoji_unique != 0 {
        let slot = estimates
            .iter_mut()
            .find(|(family, _)| family.is_empty())
            .ok_or(FormatEncodeError::MemoryUnavailable {
                subject_kind: "svg_font_demand",
                subject: "family_count",
            })?;
        *slot = ("Noto Emoji", emoji_unique);
    }
    Ok(estimates)
}

fn count_unique_scalars(
    seen: &mut [u64],
    unique: &mut usize,
    text: &str,
) -> Result<(), FormatEncodeError> {
    for scalar in text.chars().map(u32::from) {
        let scalar = usize::try_from(scalar).unwrap_or(usize::MAX);
        let word = scalar / u64::BITS as usize;
        let bit = 1_u64 << (scalar % u64::BITS as usize);
        let entry = seen
            .get_mut(word)
            .ok_or(FormatEncodeError::MemoryUnavailable {
                subject_kind: "svg_font_demand",
                subject: "scalar_range",
            })?;
        if *entry & bit == 0 {
            *entry |= bit;
            *unique = unique
                .checked_add(1)
                .ok_or(FormatEncodeError::MemoryUnavailable {
                    subject_kind: "svg_font_demand",
                    subject: "family_scalar_count",
                })?;
        }
    }
    Ok(())
}

/// Visits the display text of every glyph run the Noto Emoji fallback shaped,
/// in the same order `visit_svg_font_text` visits the occurrences that own
/// them: ordinary text in source order, then generated embeddable labels in
/// paint order. Frame names and placeholder labels never reach the fallback.
fn visit_noto_emoji_runs(
    scene: &ResolvedScene,
    text_order: &[&ResolvedTextElement],
    mut visit: impl FnMut(&str) -> Result<(), FormatEncodeError>,
) -> Result<(), FormatEncodeError> {
    let mut visit_label = |label: &text::ResolvedText| -> Result<(), FormatEncodeError> {
        for line in &label.lines {
            for run in &line.runs {
                if run.font_family == "Noto Emoji" {
                    visit(&run.display_text)?;
                }
            }
        }
        Ok(())
    };
    for text in text_order {
        visit_label(&text.text)?;
    }
    for element in &scene.elements {
        if let ResolvedElement::Outline(outline) = element {
            if let Some(label) = &outline.generated_label {
                visit_label(label)?;
            }
        }
    }
    Ok(())
}

fn visit_svg_font_text(
    scene: &ResolvedScene,
    text_order: &[&ResolvedTextElement],
    mut visit: impl FnMut(&'static str, &str) -> Result<(), FormatEncodeError>,
) -> Result<(), FormatEncodeError> {
    for text in text_order {
        visit(resolved_family(text.font_family), &text.original_text)?;
        if !text.text.missing_codepoints.is_empty() {
            visit("Liberation Sans", "?")?;
        }
    }
    for element in &scene.elements {
        match element {
            ResolvedElement::Outline(outline) => {
                if let Some(label) = &outline.generated_label {
                    for line in &label.lines {
                        visit("Liberation Sans", &line.display_text)?;
                    }
                }
            }
            ResolvedElement::Frame(frame) => {
                if let Some(name) = &frame.name {
                    visit("Liberation Sans", &name.label.display_text)?;
                }
            }
            ResolvedElement::Fallback(fallback) => {
                if let Some(label) = &fallback.label {
                    visit("Liberation Sans", &label.display_text)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn build_svg_font_bundle(
    demand: svg_fonts::Demand,
    payload: svg_fonts::Payload,
    subset_session: &mut svg_fonts::Session,
) -> Result<svg_fonts::Bundle, FormatEncodeError> {
    svg_fonts::build(demand, payload, subset_session)
}

pub(super) fn release_svg_font_encoding_records(
    session: &mut RenderSession,
    request: &RenderRequest,
    workspace: Option<(ResourceContext<'static>, u64)>,
    envelope: Option<svg_fonts::DependencyEnvelope>,
) -> Result<(), RenderError> {
    if let Some((context, bytes)) = workspace {
        session.release_working(request, context, bytes)?;
    }
    if let Some(envelope) = envelope {
        envelope.release(&mut RenderResourceAccount::new(session, request))?;
    }
    Ok(())
}

fn add_svg_font_text(
    families: &mut Vec<svg_fonts::FamilyDemand>,
    family: &'static str,
    text: &str,
) -> Result<(), FormatEncodeError> {
    if text.is_empty() {
        return Ok(());
    }
    let index = if let Some(index) = families.iter().position(|demand| demand.family == family) {
        index
    } else {
        families
            .try_reserve(1)
            .map_err(|_| FormatEncodeError::MemoryUnavailable {
                subject_kind: "svg_font_demand",
                subject: "family_set",
            })?;
        families.push(svg_fonts::FamilyDemand::new(family));
        families.len() - 1
    };
    families[index]
        .add_text(text)
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "svg_font_demand",
            subject: "family_scalar_set",
        })
}

pub(super) const fn resolved_family(font_family: i32) -> &'static str {
    match font_family {
        1 => "Virgil",
        2 | 9 => "Liberation Sans",
        3 => "Cascadia Code",
        5 => "Excalifont",
        6 => "Nunito",
        7 => "Lilita One",
        8 => "Comic Shanns",
        _ => "Excalifont",
    }
}

pub(super) fn prune_unused_official_fonts(
    elements: &mut [ResolvedElement],
    fonts: &mut Vec<text::OfficialFont>,
) -> Result<(), ResolveSceneError> {
    let mut used = Vec::new();
    for element in elements.iter() {
        match element {
            ResolvedElement::Text(text) => text.text.retain_font_indices(&mut used),
            ResolvedElement::Outline(outline) => {
                if let Some(label) = &outline.generated_label {
                    label.retain_font_indices(&mut used);
                }
            }
            _ => {}
        }
    }
    used.sort_unstable();
    let mut mapping = Vec::new();
    mapping.try_reserve_exact(fonts.len())?;
    mapping.resize(fonts.len(), None);
    let mut retained = Vec::new();
    retained.try_reserve_exact(used.len())?;
    for (old_index, font) in fonts.drain(..).enumerate() {
        if used.binary_search(&old_index).is_ok() {
            mapping[old_index] = Some(retained.len());
            retained.push(font);
        }
    }
    *fonts = retained;
    for element in elements {
        let text = match element {
            ResolvedElement::Text(text) => Some(&mut text.text),
            ResolvedElement::Outline(outline) => outline.generated_label.as_mut(),
            _ => None,
        };
        if let Some(text) = text {
            text.remap_font_indices(&mapping)
                .map_err(super::resolve::label_error)?;
        }
    }
    Ok(())
}
