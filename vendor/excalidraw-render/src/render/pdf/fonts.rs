//! PDF fonts (ADR-0021): every face the page shows text in is embedded as the
//! Modified Version `font_subset::rename_program` makes of its reviewed
//! program — the effective alias in its names, its legal records kept — and
//! Krilla subsets that program to the glyphs drawn. Each shaped run is drawn
//! as one text object at the positions the renderer shaped, each glyph with
//! the text of its own cluster, so Krilla's `ToUnicode` maps what the
//! renderer mapped and nothing is reshaped or repositioned.

use std::ops::Range;

use krilla::geom::Point;
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId};
use sha2::{Digest, Sha256};

use super::super::font_demand::ResolvedFont;
use super::super::font_subset;
use super::super::notices::{FontNoticeSet, MandatoryNotices};
use super::super::svg_fonts::{encode_hex_32, Bundle};
use super::super::text::{ResolvedGlyph, LIBERATION_SANS_IDENTITY};
use super::super::FormatEncodeError;

/// The embedded programs: the Liberation Sans face placeholder labels and
/// frame names use, and one per official font, in font-index order.
#[derive(Debug)]
pub(super) struct PdfFonts {
    pub(super) fallback: Option<PdfFont>,
    pub(super) official: Vec<PdfFont>,
}

impl PdfFonts {
    /// Prepares the program of the Liberation Sans fallback, when labels need
    /// it, and of every official font the text references.
    pub(super) fn prepare(
        fallback_sfnt: Option<&[u8]>,
        official: &[ResolvedFont],
    ) -> Result<Self, FormatEncodeError> {
        let fallback = fallback_sfnt
            .map(|sfnt| prepare_font(LIBERATION_SANS_IDENTITY, sfnt))
            .transpose()?;
        let mut prepared = Vec::new();
        prepared.try_reserve_exact(official.len()).map_err(|_| {
            FormatEncodeError::MemoryUnavailable {
                subject_kind: "format_plan",
                subject: "pdf_font_programs",
            }
        })?;
        for font in official {
            prepared.push(prepare_font(font.identity, &font.sfnt)?);
        }
        Ok(Self {
            fallback,
            official: prepared,
        })
    }

    /// The artifact's mandatory notices: each embedded family's attribution
    /// and license body, and each embedded program's Modified Version notice.
    pub(super) fn notices(
        &self,
        official: &[ResolvedFont],
    ) -> Result<MandatoryNotices, FormatEncodeError> {
        let invalid =
            || FormatEncodeError::Encoding("PDF font notice is not catalogued".to_owned());
        let memory = |_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "metadata",
            subject: "pdf_mandatory_notices",
        };
        let mut notices = FontNoticeSet::default();
        if self.fallback.is_some() {
            notices
                .add_family(
                    crate::fonts::font_notice(LIBERATION_SANS_IDENTITY).ok_or_else(invalid)?,
                )
                .map_err(memory)?;
        }
        for font in official {
            notices
                .add_family(crate::fonts::font_notice(font.identity).ok_or_else(invalid)?)
                .map_err(memory)?;
        }
        for program in self.fallback.iter().chain(&self.official) {
            notices.add_subset(program.notice_identity.clone(), program.notice_text.clone());
        }
        notices.into_manifest().map_err(memory)
    }
}

/// One embedded program: the Krilla font drawn with, and the Modified Version
/// notice the artifact carries for it.
#[derive(Debug)]
pub(super) struct PdfFont {
    pub(super) font: Font,
    pub(super) notice_identity: String,
    pub(super) notice_text: String,
}

/// Prepares the program PDF embeds for the manifested `identity`, whose
/// admitted source program is `sfnt`.
pub(super) fn prepare_font(identity: &str, sfnt: &[u8]) -> Result<PdfFont, FormatEncodeError> {
    let family = crate::fonts::font_family(identity).ok_or_else(|| {
        FormatEncodeError::Encoding(format!("PDF font {identity} has no catalog family"))
    })?;
    let notice = crate::fonts::font_notice(identity).ok_or_else(|| {
        FormatEncodeError::Encoding(format!("PDF font {identity} has no catalog notice"))
    })?;
    let alias = Bundle::effective_family(family.display_name());
    let source_digest = encode_hex_32(&Sha256::digest(sfnt).into());
    let program_identity = program_identity(identity, &source_digest);
    let program = font_subset::rename_program(sfnt, alias, &program_identity).map_err(|_| {
        FormatEncodeError::Encoding(format!("PDF font {identity} could not be renamed"))
    })?;
    let program_digest = encode_hex_32(&Sha256::digest(&program).into());
    let notice_text = format!(
        "Derived font program of {identity}. Source program SHA-256: {source_digest}. {} \
         Modified Version under {}. Transformation: names rewritten for compatibility \
         revision 1; glyphs outside the artifact's text removed by the PDF writer when \
         embedding. Effective alias: {alias}. Renamed program: {} bytes, SHA-256 \
         {program_digest}.",
        notice.attribution,
        notice.license_identity,
        program.len(),
    );
    let font = Font::new(program.into(), 0).ok_or_else(|| {
        FormatEncodeError::Encoding(format!("PDF font {identity} is not a usable program"))
    })?;
    Ok(PdfFont {
        font,
        notice_identity: format!("pdf-font-program@sha256:{program_identity}"),
        notice_text,
    })
}

/// The program's identity: a digest over the manifested identity and the
/// admitted program's bytes, domain-separated from the SVG subset identity.
fn program_identity(identity: &str, source_digest: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"excalidraw-render:pdf-font-program:v1");
    hasher.update([0]);
    hasher.update(
        u64::try_from(identity.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    hasher.update(identity.as_bytes());
    hasher.update(source_digest.as_bytes());
    encode_hex_32(&hasher.finalize().into())
}

/// One glyph of a run as Krilla draws it: its cluster's text, and the step
/// to the next glyph's shaped origin and its baseline's rise, both per unit
/// of font size as Krilla asks for them.
struct RunGlyph {
    id: GlyphId,
    text: Range<usize>,
    advance: f32,
    rise: f32,
}

impl krilla::text::Glyph for RunGlyph {
    fn glyph_id(&self) -> GlyphId {
        self.id
    }
    fn text_range(&self) -> Range<usize> {
        self.text.clone()
    }
    fn x_advance(&self, size: f32) -> f32 {
        self.advance * size
    }
    fn x_offset(&self, _size: f32) -> f32 {
        0.0
    }
    fn y_offset(&self, size: f32) -> f32 {
        self.rise * size
    }
    fn y_advance(&self, _size: f32) -> f32 {
        0.0
    }
    fn location(&self) -> Option<krilla::surface::Location> {
        None
    }
}

/// Draws one shaped run of `glyphs` in `font` at `font_size` with the
/// surface's current fill: one text object whose glyphs sit at the origins
/// the renderer shaped, each carrying its own cluster's text for Krilla's
/// `ToUnicode`. Each glyph advances to the next one's origin, and a glyph
/// off the first glyph's baseline rises by the difference.
pub(super) fn draw_glyphs(
    surface: &mut Surface<'_>,
    font: &Font,
    font_size: f64,
    glyphs: &[ResolvedGlyph],
) -> Result<(), FormatEncodeError> {
    let Some(first) = glyphs.first() else {
        return Ok(());
    };
    if !(font_size.is_finite() && font_size > 0.0) {
        return Err(FormatEncodeError::Encoding(
            "PDF text font size must be positive".to_owned(),
        ));
    }
    let size = finite_f32(font_size, "PDF text font size")?;
    let mut text = String::new();
    let mut run = Vec::new();
    run.try_reserve_exact(glyphs.len())
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "format_workspace",
            subject: "pdf_text_run",
        })?;
    // The glyphs of one cluster share its text. Krilla reads each glyph's
    // range for its content, compares neighbouring ranges for equality (equal
    // ranges mark one cluster for `ActualText`), and orders ranges to detect a
    // reversed run. Giving every glyph its own copy kept neighbours unequal and
    // ranges ascending, but cost the cluster's length per glyph. Two copies,
    // alternated, keep each content, neighbours unequal and no run reversed,
    // so Krilla sees the same run in space linear in the text.
    let mut shared: Option<(&ResolvedGlyph, [Range<usize>; 2], usize)> = None;
    for (index, glyph) in glyphs.iter().enumerate() {
        let range = match &mut shared {
            Some((previous, copies, next))
                if !glyph.unicode.is_empty()
                    && std::sync::Arc::ptr_eq(&previous.unicode, &glyph.unicode) =>
            {
                *previous = glyph;
                let range = copies[*next].clone();
                *next = 1 - *next;
                range
            }
            _ => {
                let start = text.len();
                text.push_str(&String::from_utf16_lossy(&glyph.unicode));
                let own = start..text.len();
                shared = None;
                if glyphs.get(index + 1).is_some_and(|next| {
                    !glyph.unicode.is_empty()
                        && std::sync::Arc::ptr_eq(&next.unicode, &glyph.unicode)
                }) {
                    let copy = text.len();
                    text.push_str(&String::from_utf16_lossy(&glyph.unicode));
                    shared = Some((glyph, [own.clone(), copy..text.len()], 1));
                }
                own
            }
        };
        let step = match glyphs.get(index + 1) {
            Some(next) => (next.x - glyph.x) / font_size,
            None => f64::from(glyph.advance_1000) / 1000.0,
        };
        run.push(RunGlyph {
            id: GlyphId::new(u32::from(glyph.id)),
            text: range,
            advance: finite_f32(step, "PDF text glyph advance")?,
            rise: finite_f32((first.y - glyph.y) / font_size, "PDF text glyph rise")?,
        });
    }
    surface.draw_glyphs(
        Point::from_xy(
            finite_f32(first.x, "PDF text glyph x")?,
            finite_f32(first.y, "PDF text glyph y")?,
        ),
        &run,
        font.clone(),
        &text,
        size,
        false,
    );
    Ok(())
}

pub(super) fn finite_f32(value: f64, subject: &str) -> Result<f32, FormatEncodeError> {
    let narrowed = value as f32;
    if !narrowed.is_finite() {
        return Err(FormatEncodeError::Encoding(format!(
            "{subject} must be a finite PDF number"
        )));
    }
    Ok(narrowed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_named_for_its_effective_family_and_carries_its_notice() {
        let identity = super::super::super::text::LIBERATION_SANS_IDENTITY;
        let face = crate::fonts::font_family(identity).expect("Liberation Sans is catalogued");
        assert_eq!(
            Bundle::effective_family(face.display_name()),
            "ExcalidrawRender-9"
        );
        let prepared = program_identity(identity, &"0".repeat(64));
        assert_eq!(prepared.len(), 64);
        assert_ne!(prepared, program_identity(identity, &"1".repeat(64)));
    }
}
