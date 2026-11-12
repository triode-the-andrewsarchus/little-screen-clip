use std::collections::{HashMap, TryReserveError};
use std::sync::Arc;

use crate::fonts::{Face, FaceId, FamilyId};
use crate::shaping::{GlyphId, OutlineBuilder};

use crate::font_metadata::FamilyMetrics;
use crate::fonts::{FontError, FontLoader};

pub(super) const LIBERATION_SANS_IDENTITY: &str = concat!(
    "fonts/Liberation/LiberationSans-Regular.woff2@sha256:",
    "006a2b28cbbeeaec937d1b367d0949f5f48ef4b2e7b4e1d8dc7a799d2d639be8"
);
pub(super) const FALLBACK_FONT_SIZE: f64 = 14.0;

/// Excalidraw's `FRAME_STYLE`, as far as a frame's visible name goes. The
/// export synthesizes the name as a Helvetica text element
/// (`addFrameLabelsAsTextElements`): `nameFontSize` 14 at `nameLineHeight`
/// 1.25 gives a 17.5 px line box, placed `nameOffsetY` 3 px above the frame's
/// top edge and truncated with `...` to the frame's width.
pub(super) const FRAME_NAME_FONT_FAMILY: i32 = 2;
pub(super) const FRAME_NAME_FONT_SIZE: f64 = 14.0;
pub(super) const FRAME_NAME_LINE_HEIGHT: f64 = 1.25;
pub(super) const FRAME_NAME_OFFSET_Y: f64 = 3.0;
pub(super) const FRAME_NAME_BOX_HEIGHT: f64 = FRAME_NAME_FONT_SIZE * FRAME_NAME_LINE_HEIGHT;

/// A frame's visible name laid out as the reference's synthesized text element:
/// left-aligned in a `FRAME_NAME_BOX_HEIGHT` line box, on the baseline
/// Helvetica's metadata metrics give that box, shaped with the Liberation Sans
/// substitute.
#[derive(Debug)]
pub(super) struct FrameName {
    pub(super) label: ResolvedLabel,
    /// The synthesized text element's width: the measured advance of the name,
    /// or the frame's width once the name had to be truncated to fit it.
    pub(super) width: f64,
}

pub(super) struct FallbackFont {
    woff2: Vec<u8>,
    sfnt: Arc<[u8]>,
}

/// One official face glyph selection loaded for a target: a family chunk's
/// reviewed decode, or the complete Noto Emoji fallback. The sfnt is what
/// shaping and PDF embedding read; direct SVG derives its own subset from the
/// sealed source bytes (`svg_fonts`), so no format needs the container here.
pub(super) struct OfficialFont {
    source_family: i32,
    family: FamilyId,
    identity: &'static str,
    sfnt: Arc<[u8]>,
}

#[derive(Debug)]
pub(super) struct ResolvedText {
    pub(super) font_index: usize,
    pub(super) font_family: &'static str,
    pub(super) font_size: f64,
    pub(super) text_align: ResolvedTextAlign,
    pub(super) lines: Vec<ResolvedLabel>,
    pub(super) scalar_count: u64,
    pub(super) line_break_steps: u64,
    pub(super) missing_codepoints: Vec<u32>,
    pub(super) noto_emoji_substituted: bool,
    pub(super) fallback_probes: u64,
    fallback_resources: u64,
    pub(super) prepaid_glyphs: u64,
    pub(super) prepaid_line_break_steps: u64,
    /// Outline segments counted but not kept: every one, once the text passed
    /// its [`TextLayout::outline_limit`]. Such text exceeds the per-element
    /// geometry limit, so accounting omits it before any encoder reads a path.
    pub(super) discarded_outline_segments: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResolvedTextAlign {
    Left,
    Center,
    Right,
}

impl ResolvedTextAlign {
    fn from_normalized(value: &str) -> Self {
        match value {
            "center" => Self::Center,
            "right" => Self::Right,
            _ => Self::Left,
        }
    }
}

pub(super) struct TextLayout<'a> {
    pub(super) width: f64,
    pub(super) font_size: f64,
    pub(super) line_height: f64,
    pub(super) text_align: &'a str,
    /// The source family whose metadata metrics position the text. It is the
    /// element's own family, not the loaded font's: Helvetica and Liberation
    /// Sans share one font and must not share a baseline.
    pub(super) font_family: i32,
    /// Outline operations kept before the text is known to exceed the
    /// per-element geometry limit; the rest are only counted, so a text the
    /// limit rejects never holds all of its glyph outlines at once.
    pub(super) outline_limit: u64,
}

impl TextLayout<'_> {
    /// The first baseline for this layout, from the positioning family's
    /// metadata metrics. Every family a text element can resolve to is in the
    /// table, so a miss is the same invalid font `OfficialFont::load` rejects.
    fn first_baseline(&self) -> Result<f64, LabelError> {
        let metrics =
            FamilyMetrics::for_source_family(self.font_family).ok_or(LabelError::InvalidFont)?;
        Ok(metrics.first_baseline(self.font_size, self.font_size * self.line_height))
    }
}

/// The outline operations one text element may keep, and how many it produced.
struct OutlineBudget {
    limit: u64,
    counted: u64,
}

impl OutlineBudget {
    fn new(limit: u64) -> Self {
        Self { limit, counted: 0 }
    }

    fn unlimited() -> Self {
        Self::new(u64::MAX)
    }

    fn exceeded(&self) -> bool {
        self.counted > self.limit
    }

    /// The reservation for `glyphs` more glyphs' outlines, within the limit.
    fn reservation(&self, glyphs: usize) -> usize {
        let remaining = self.limit.saturating_sub(self.counted);
        glyphs
            .saturating_mul(32)
            .min(usize::try_from(remaining).unwrap_or(usize::MAX))
    }

    /// Drops `lines`' outlines once the limit is exceeded and returns the
    /// segments that were counted but are no longer kept.
    fn settle(&self, lines: &mut [ResolvedLabel]) -> u64 {
        if !self.exceeded() {
            return 0;
        }
        for line in lines {
            line.path.operations = Vec::new();
            for glyph in &mut line.glyphs {
                glyph.outline = 0..0;
            }
        }
        self.counted
    }
}

/// `FONT_FAMILY.Excalifont`: the only source family whose fallback chain the
/// CJK subset trigger extends with Xiaolai.
pub(super) const EXCALIFONT_FONT_FAMILY: i32 = 5;

/// The CJK subset trigger's outcome for one resolved scene.
///
/// Xiaolai is selected by this trigger alone: when the scene's Excalifont font
/// family scalar set intersects the frozen CJK scalar table (spec §8.3.1),
/// Xiaolai follows Excalifont's own chunks for every Excalifont occurrence, in
/// glyph selection and in the SVG font demand alike. Whether a Xiaolai chunk
/// happens to carry a glyph never selects it. Evaluated once per scene so the
/// PNG and PDF glyphs and the SVG `@font-face` set cannot disagree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct CjkSubsetTrigger {
    xiaolai_selected: bool,
}

impl CjkSubsetTrigger {
    /// Evaluates the trigger over the scene's Excalifont occurrences' text.
    pub(super) fn evaluate<'a>(excalifont_texts: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            xiaolai_selected: excalifont_texts
                .into_iter()
                .flat_map(str::chars)
                .any(|scalar| crate::fonts::is_cjk_scalar_v1(u32::from(scalar))),
        }
    }

    pub(super) fn selects_xiaolai(self) -> bool {
        self.xiaolai_selected
    }
}

#[derive(Debug)]
pub(super) struct ResolvedLabel {
    pub(super) text: String,
    pub(super) display_text: String,
    pub(super) baseline: f64,
    pub(super) path: LabelPath,
    pub(super) glyph_count: u64,
    pub(super) missing_codepoints: Vec<u32>,
    pub(super) glyphs: Vec<ResolvedGlyph>,
    pub(super) runs: Vec<ResolvedTextRun>,
}

impl ResolvedLabel {
    /// The UTF-16 code units of cluster text this label's glyphs repeat: each
    /// glyph of a cluster shaped to several glyphs carries the cluster's whole
    /// text, which PDF hands Krilla to read and compare once per glyph.
    #[cfg(feature = "pdf")]
    pub(super) fn repeated_cluster_text(&self) -> u64 {
        let shares = |left: &ResolvedGlyph, right: &ResolvedGlyph| {
            !left.unicode.is_empty() && Arc::ptr_eq(&left.unicode, &right.unicode)
        };
        self.glyphs
            .iter()
            .enumerate()
            .filter(|(index, glyph)| {
                index
                    .checked_sub(1)
                    .and_then(|previous| self.glyphs.get(previous))
                    .is_some_and(|previous| shares(previous, glyph))
                    || self
                        .glyphs
                        .get(index + 1)
                        .is_some_and(|next| shares(glyph, next))
            })
            .map(|(_, glyph)| u64::try_from(glyph.unicode.len()).unwrap_or(u64::MAX))
            .fold(0, u64::saturating_add)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ResolvedTextRun {
    pub(super) font_index: usize,
    pub(super) font_family: &'static str,
    pub(super) display_text: String,
    pub(super) glyph_start: usize,
    pub(super) glyph_end: usize,
    pub(super) rtl: bool,
    /// For each scalar of `display_text` in logical order, the index into the
    /// line's glyphs of the glyph that shows it. The glyphs themselves sit in
    /// visual order; SVG assigns the nth `x` and `y` to the nth character of
    /// the run's content, which is logical, so it positions through this.
    pub(super) logical_glyphs: Vec<usize>,
}

#[derive(Debug, PartialEq)]
pub(super) struct ResolvedGlyph {
    pub(super) id: u16,
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) advance_1000: u16,
    /// The UTF-16 text of this glyph's whole cluster, shared by every glyph
    /// of the cluster.
    pub(super) unicode: Arc<Vec<u16>>,
    /// The byte offset in the shaped display text where this glyph's cluster
    /// starts.
    pub(super) cluster: usize,
    /// This glyph's contours in its label's shared outline. Empty glyphs keep
    /// an empty range. PNG needs the boundary to position each cached glyph
    /// independently on the canvas's device-subpixel grid.
    pub(super) outline: std::ops::Range<usize>,
}

#[derive(Debug, PartialEq)]
pub(super) struct LabelPath {
    pub(super) operations: Vec<LabelPathOperation>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum LabelPathOperation {
    Move {
        x: f64,
        y: f64,
    },
    Line {
        x: f64,
        y: f64,
    },
    Quadratic {
        control_x: f64,
        control_y: f64,
        x: f64,
        y: f64,
    },
    Close,
}

#[derive(Debug)]
pub(super) enum LabelError {
    Memory,
    InvalidFont,
    FontUnavailable(super::FontUnavailable),
}

impl From<TryReserveError> for LabelError {
    fn from(_: TryReserveError) -> Self {
        Self::Memory
    }
}

fn font_error(id: FaceId, error: FontError) -> LabelError {
    match error {
        FontError::Memory => LabelError::Memory,
        FontError::Integrity(error) if error.is_unavailable() => {
            LabelError::FontUnavailable(super::FontUnavailable::face(id.face()))
        }
        _ => LabelError::InvalidFont,
    }
}

impl FallbackFont {
    #[cfg(test)]
    pub(super) fn load() -> Result<Self, LabelError> {
        Self::load_with_loader(&mut FontLoader::default())
    }

    pub(super) fn load_with_loader(loader: &mut FontLoader) -> Result<Self, LabelError> {
        let id = FamilyId::Liberation.faces()[0].id();
        let source = loader
            .load_source(id)
            .map_err(|error| font_error(id, error))?;
        let mut woff2 = Vec::new();
        woff2.try_reserve_exact(source.len())?;
        woff2.extend_from_slice(&source);
        let sfnt = loader
            .load_sfnt(id)
            .map_err(|error| font_error(id, error))?;
        Ok(Self { woff2, sfnt })
    }

    pub(super) fn resolve(
        &self,
        full_text: &str,
        width: f64,
        height: f64,
    ) -> Result<Option<ResolvedLabel>, LabelError> {
        if height < FALLBACK_FONT_SIZE {
            return Ok(None);
        }
        let full = self.shape(full_text, width, height)?;
        if full.is_some() {
            return Ok(full);
        }
        self.shape("\u{2026}", width, height)
    }

    pub(super) fn into_parts(self) -> (Vec<u8>, Arc<[u8]>) {
        (self.woff2, self.sfnt)
    }

    /// Lays out a frame's name the way `addFrameLabelsAsTextElements` and
    /// `truncateText` do: measured at 14 px, and when wider than the frame,
    /// cut to the longest prefix that fits with `...` appended. When even one
    /// character with the dots does not fit, the reference keeps the whole
    /// name at the frame's width, and so does this.
    ///
    /// The reference measures each candidate afresh, so it also sees the kern
    /// pair between the last kept character and the first dot; this measures
    /// prefixes of the one shaped name and adds the dots' own advance, which
    /// can differ from that by one kern pair (a fraction of a pixel) exactly
    /// at the cut. It also cuts between scalar values where the reference cuts
    /// between UTF-16 code units, so a cut never splits a surrogate pair.
    pub(super) fn resolve_frame_name(
        &self,
        name: &str,
        frame_width: f64,
    ) -> Result<FrameName, LabelError> {
        let face =
            crate::shaping::Face::from_shared(&self.sfnt, 0).ok_or(LabelError::InvalidFont)?;
        let scale = FRAME_NAME_FONT_SIZE / f64::from(face.units_per_em());
        let baseline = FamilyMetrics::for_source_family(FRAME_NAME_FONT_FAMILY)
            .ok_or(LabelError::InvalidFont)?
            .first_baseline(FRAME_NAME_FONT_SIZE, FRAME_NAME_BOX_HEIGHT);
        let (display_text, missing_codepoints) = fallback_display_text(&face, name)?;
        let glyphs = shape_text(&face, &display_text, ShapeDirection::Guessed);
        // `measureText` measures a blank line as one space.
        let measured = if display_text.is_empty() {
            shaped_advance(&face, " ", scale)
        } else {
            shaped_advance_units(&glyphs) as f64 * scale
        };
        if measured <= frame_width {
            let label = build_label(
                &face,
                &glyphs,
                name,
                display_text,
                missing_codepoints,
                0.0,
                baseline,
                scale,
            )?;
            return Ok(FrameName {
                label,
                width: measured,
            });
        }
        let dots = shaped_advance(&face, "...", scale);
        let prefix_widths = shaped_prefix_widths(&glyphs, &display_text, scale)?;
        // `truncateText` tries every prefix from the whole name down to its
        // first character; `prefix_widths[i]` is the width of the first `i`
        // characters, so index 0 (the empty prefix) is never a candidate.
        let cut = (1..prefix_widths.len())
            .rev()
            .find(|&count| prefix_widths[count] + dots <= frame_width);
        let text = match cut {
            Some(count) => {
                let end = char_boundary(name, count)?;
                let mut text = String::new();
                text.try_reserve_exact(end.saturating_add(3))?;
                text.push_str(&name[..end]);
                text.push_str("...");
                text
            }
            None => try_owned(name)?,
        };
        // The cut text has its own replaced scalars: the ones the cut removed
        // are not reported.
        let (display_text, missing_codepoints) = fallback_display_text(&face, &text)?;
        let glyphs = shape_text(&face, &display_text, ShapeDirection::Guessed);
        let label = build_label(
            &face,
            &glyphs,
            &text,
            display_text,
            missing_codepoints,
            0.0,
            baseline,
            scale,
        )?;
        Ok(FrameName {
            label,
            width: frame_width,
        })
    }

    fn shape(
        &self,
        text: &str,
        width: f64,
        height: f64,
    ) -> Result<Option<ResolvedLabel>, LabelError> {
        let face =
            crate::shaping::Face::from_shared(&self.sfnt, 0).ok_or(LabelError::InvalidFont)?;
        let (display_text, missing_codepoints) = fallback_display_text(&face, text)?;
        let glyphs = shape_text(&face, &display_text, ShapeDirection::Guessed);
        let units_per_em = f64::from(face.units_per_em());
        let scale = FALLBACK_FONT_SIZE / units_per_em;
        let advance = shaped_advance_units(&glyphs) as f64 * scale;
        if advance > width {
            return Ok(None);
        }
        let origin_x = (width - advance) / 2.0;
        let baseline = height / 2.0 + f64::from(face.ascender() + face.descender()) * scale / 2.0;
        build_label(
            &face,
            &glyphs,
            text,
            display_text,
            missing_codepoints,
            origin_x,
            baseline,
            scale,
        )
        .map(Some)
    }
}

/// The text `face` can show: every character it has a glyph for, and `?` for
/// each it has not, with the replaced scalars in source order. A
/// default-ignorable code point is kept as it is and never replaced or
/// reported (spec 8.3, #167): the shaper hides it, as the browser does. `text`
/// is one line's content; a line terminator never reaches this.
fn fallback_display_text(
    face: &crate::shaping::Face,
    text: &str,
) -> Result<(String, Vec<u32>), LabelError> {
    let mut display_text = String::new();
    display_text.try_reserve_exact(text.len())?;
    let mut missing_codepoints = Vec::new();
    for character in text.chars() {
        if is_default_ignorable(character) || face.glyph_index(character).is_some() {
            display_text.push(character);
        } else {
            display_text.push('?');
            missing_codepoints.try_reserve(1)?;
            missing_codepoints.push(u32::from(character));
        }
    }
    Ok((display_text, missing_codepoints))
}

/// Whether `character` is a default-ignorable code point as the pinned shaper
/// (`harfrust 0.14.0`, HarfBuzz's `is_default_ignorable`) classifies it: the
/// bidi controls U+200E, U+200F, U+202A–U+202E and U+2066–U+2069 with the
/// rest of U+2060–U+206F, the joiners U+200B–U+200D, the soft hyphen U+00AD,
/// the combining grapheme joiner U+034F, the Arabic letter mark U+061C, the
/// Khmer and Mongolian format controls U+17B4–U+17B5 and U+180B–U+180F, the
/// variation selectors U+FE00–U+FE0F, U+FEFF, U+FFF0–U+FFF8, the musical
/// format controls U+1D173–U+1D17A and the tag and variation selector plane
/// U+E0000–U+E0FFF. This is HarfBuzz's modified Default_Ignorable_Code_Point,
/// which omits the Hangul fillers U+115F, U+1160, U+3164 and U+FFA0 the
/// Unicode property includes; the reference renders these through HarfBuzz,
/// which draws nothing for any of them whether or not the face has a glyph.
///
/// Such a scalar is never a missing codepoint (spec 8.3): it pays no font
/// fallback probe, is shaped in the run its neighbours use with the zero
/// advance the shaper gives it, draws nothing and reports nothing.
pub(super) fn is_default_ignorable(character: char) -> bool {
    let scalar = u32::from(character);
    match scalar >> 16 {
        0 => match scalar >> 8 {
            0x00 => scalar == 0x00ad,
            0x03 => scalar == 0x034f,
            0x06 => scalar == 0x061c,
            0x17 => (0x17b4..=0x17b5).contains(&scalar),
            0x18 => (0x180b..=0x180f).contains(&scalar),
            0x20 => {
                (0x200b..=0x200f).contains(&scalar)
                    || (0x202a..=0x202e).contains(&scalar)
                    || (0x2060..=0x206f).contains(&scalar)
            }
            0xfe => (0xfe00..=0xfe0f).contains(&scalar) || scalar == 0xfeff,
            0xff => (0xfff0..=0xfff8).contains(&scalar),
            _ => false,
        },
        0x01 => (0x1d173..=0x1d17a).contains(&scalar),
        0x0e => (0xe0000..=0xe0fff).contains(&scalar),
        _ => false,
    }
}

fn shaped_advance_units(glyphs: &crate::shaping::Buffer) -> i64 {
    glyphs
        .glyph_positions()
        .iter()
        .fold(0_i64, |advance, position| {
            advance.saturating_add(i64::from(position.x_advance))
        })
}

/// The advance of `text` shaped with `face`, in logical pixels at `scale`.
fn shaped_advance(face: &crate::shaping::Face, text: &str, scale: f64) -> f64 {
    shaped_advance_units(&shape_text(face, text, ShapeDirection::Guessed)) as f64 * scale
}

/// The advance of every character-count prefix of `display_text`, from the
/// empty prefix to the whole text, in logical pixels. A glyph belongs to the
/// prefix that contains the character its cluster starts at, so the widths
/// keep the kerning the shaper applied inside each prefix.
fn shaped_prefix_widths(
    glyphs: &crate::shaping::Buffer,
    display_text: &str,
    scale: f64,
) -> Result<Vec<f64>, LabelError> {
    let mut advances = Vec::new();
    advances.try_reserve_exact(glyphs.len())?;
    for (info, position) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
        let cluster = usize::try_from(info.cluster).map_err(|_| LabelError::InvalidFont)?;
        advances.push((cluster, i64::from(position.x_advance)));
    }
    advances.sort_unstable_by_key(|(cluster, _)| *cluster);
    let mut widths = Vec::new();
    widths.try_reserve_exact(display_text.chars().count().saturating_add(1))?;
    let mut next = advances.iter().peekable();
    let mut units = 0_i64;
    for boundary in display_text
        .char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(display_text.len()))
    {
        while let Some((_, advance)) = next.next_if(|(cluster, _)| *cluster < boundary) {
            units = units.saturating_add(*advance);
        }
        widths.push(units as f64 * scale);
    }
    Ok(widths)
}

/// The byte offset where the first `count` characters of `text` end.
fn char_boundary(text: &str, count: usize) -> Result<usize, LabelError> {
    text.char_indices()
        .map(|(offset, _)| offset)
        .chain(std::iter::once(text.len()))
        .nth(count)
        .ok_or(LabelError::InvalidFont)
}

/// Builds the label for one shaped line of fallback-face text with its pen
/// origin at `origin_x` on `baseline`.
#[allow(clippy::too_many_arguments)]
fn build_label(
    face: &crate::shaping::Face,
    glyphs: &crate::shaping::Buffer,
    text: &str,
    display_text: String,
    missing_codepoints: Vec<u32>,
    origin_x: f64,
    baseline: f64,
    scale: f64,
) -> Result<ResolvedLabel, LabelError> {
    let mut operations = Vec::new();
    let estimated_operations = glyphs.glyph_infos().len().saturating_mul(32);
    operations.try_reserve(estimated_operations)?;
    let mut budget = OutlineBudget::unlimited();
    let mut pen_x = 0_i64;
    let mut pen_y = 0_i64;
    let mut resolved_glyphs = Vec::new();
    resolved_glyphs.try_reserve_exact(glyphs.glyph_infos().len())?;
    let cluster_starts = ClusterStarts::of(glyphs)?;
    let mut previous_unicode = None;
    for (info, position) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
        let outline_start = operations.len();
        let mut builder = GlyphOutlineBuilder {
            operations: &mut operations,
            budget: &mut budget,
            failed: false,
            origin_x,
            baseline,
            glyph_x: (pen_x + i64::from(position.x_offset)) as f64,
            glyph_y: (pen_y + i64::from(position.y_offset)) as f64,
            scale,
        };
        let glyph_id = u16::try_from(info.glyph_id).map_err(|_| LabelError::InvalidFont)?;
        let cluster = usize::try_from(info.cluster).map_err(|_| LabelError::InvalidFont)?;
        let cluster_text = cluster_starts.text(&display_text, cluster)?;
        if glyph_id == 0 {
            return Err(LabelError::InvalidFont);
        }
        let unicode = cluster_unicode(&mut previous_unicode, cluster, cluster_text)?;
        let advance = face
            .glyph_hor_advance(GlyphId(glyph_id))
            .unwrap_or_default();
        let units_per_em = u64::from(face.units_per_em());
        let advance_1000 =
            u16::try_from((u64::from(advance) * 1_000 + units_per_em / 2) / units_per_em)
                .map_err(|_| LabelError::InvalidFont)?;
        if face
            .outline_glyph(GlyphId(glyph_id), &mut builder)
            .is_none()
            && info.glyph_id != 3
        {
            return Err(LabelError::InvalidFont);
        }
        if builder.failed {
            return Err(LabelError::Memory);
        }
        resolved_glyphs.push(ResolvedGlyph {
            id: glyph_id,
            x: origin_x + (pen_x + i64::from(position.x_offset)) as f64 * scale,
            y: baseline - (pen_y + i64::from(position.y_offset)) as f64 * scale,
            advance_1000,
            unicode,
            cluster,
            outline: outline_start..operations.len(),
        });
        pen_x = pen_x.saturating_add(i64::from(position.x_advance));
        pen_y = pen_y.saturating_add(i64::from(position.y_advance));
    }
    let mut owned_text = String::new();
    owned_text.try_reserve_exact(text.len())?;
    owned_text.push_str(text);
    Ok(ResolvedLabel {
        text: owned_text,
        display_text,
        baseline,
        path: LabelPath { operations },
        glyph_count: u64::try_from(glyphs.glyph_infos().len()).unwrap_or(u64::MAX),
        missing_codepoints,
        glyphs: resolved_glyphs,
        runs: Vec::new(),
    })
}

impl OfficialFont {
    #[cfg(test)]
    pub(super) fn load(font_family: i32, text: &str) -> Result<Self, LabelError> {
        Self::load_with_loader(font_family, text, &mut FontLoader::default())
    }

    pub(super) fn load_with_loader(
        font_family: i32,
        text: &str,
        loader: &mut FontLoader,
    ) -> Result<Self, LabelError> {
        let family_id = match font_family {
            1 => FamilyId::Virgil,
            2 | 9 => FamilyId::Liberation,
            3 => FamilyId::Cascadia,
            EXCALIFONT_FONT_FAMILY => FamilyId::Excalifont,
            6 => FamilyId::Nunito,
            7 => FamilyId::Lilita,
            8 => FamilyId::ComicShanns,
            _ => return Err(LabelError::InvalidFont),
        };
        let mut chunks = OfficialFontChunks::load(family_id, loader);
        // The shaped scalars: every scalar but the line terminators and the
        // default-ignorable code points, which no face needs to declare or
        // map since the shaper hides them in whichever run they fall (#167).
        let shaped = || {
            text.chars().filter(|character| {
                !is_line_terminator(*character) && !is_default_ignorable(*character)
            })
        };
        // The face for the whole text is the first official font chunk, in
        // pinned face order, whose declared Unicode ranges cover every shaped
        // scalar; failing that, the family's basic-Latin chunk shapes what it
        // can and leaves the rest to per-scalar fallback. A chunk's cmap never
        // selects it for scalars outside its declared ranges.
        let mut selected = chunks.first_declaring(shaped)?;
        if selected.is_none() {
            selected = chunks.first_declaring(|| ['?', 'A'].into_iter())?;
        }
        let (face, sfnt) = chunks.take(selected.ok_or(LabelError::InvalidFont)?)?;
        Ok(Self::from_face(font_family, face, sfnt))
    }

    fn from_face(source_family: i32, face: &'static Face, sfnt: Arc<[u8]>) -> Self {
        Self {
            source_family,
            family: face.family(),
            identity: face.identity(),
            sfnt,
        }
    }

    pub(super) fn layout(
        &self,
        text: &str,
        layout: TextLayout<'_>,
    ) -> Result<ResolvedText, LabelError> {
        let face =
            crate::shaping::Face::from_shared(&self.sfnt, 0).ok_or(LabelError::InvalidFont)?;
        let text_align = ResolvedTextAlign::from_normalized(layout.text_align);
        let mut missing_codepoints = Vec::new();
        let mut stored_lines = Vec::new();
        for line in source_lines(text) {
            let (display, line_missing) = fallback_display_text(&face, line.content)?;
            missing_codepoints.try_reserve(line_missing.len())?;
            missing_codepoints.extend(line_missing);
            stored_lines.try_reserve(1)?;
            stored_lines.push(StoredLine {
                source: line.source()?,
                display,
            });
        }
        let line_break_steps = u64::try_from(stored_lines.len()).unwrap_or(u64::MAX);
        let line_step = layout.font_size * layout.line_height;
        let first_baseline = layout.first_baseline()?;
        let mut lines = Vec::new();
        lines.try_reserve_exact(stored_lines.len())?;
        let mut budget = OutlineBudget::new(layout.outline_limit);
        for (index, line) in stored_lines.into_iter().enumerate() {
            let baseline = first_baseline + index as f64 * line_step;
            lines.push(
                shape_official_line(
                    &face,
                    &line.source,
                    &line.display,
                    layout.width,
                    layout.font_size,
                    baseline,
                    text_align,
                    ShapeDirection::Guessed,
                    &mut budget,
                )?
                .label,
            );
        }
        let discarded_outline_segments = budget.settle(&mut lines);
        Ok(ResolvedText {
            font_index: 0,
            font_family: self.family.display_name(),
            font_size: layout.font_size,
            text_align,
            lines,
            scalar_count: u64::try_from(text.chars().count()).unwrap_or(u64::MAX),
            line_break_steps,
            missing_codepoints,
            noto_emoji_substituted: false,
            fallback_probes: 0,
            fallback_resources: 0,
            prepaid_glyphs: prospective_glyph_count(text),
            prepaid_line_break_steps: line_count(text),
            discarded_outline_segments,
        })
    }

    pub(super) fn wrap_embeddable_label(
        &self,
        text: &str,
        font_size: f64,
        max_width: f64,
    ) -> Result<String, LabelError> {
        if max_width < 0.0 || !max_width.is_finite() {
            return try_owned(text);
        }
        let face =
            crate::shaping::Face::from_shared(&self.sfnt, 0).ok_or(LabelError::InvalidFont)?;
        let scale = font_size / f64::from(face.units_per_em());
        let mut wrapped = String::new();
        wrapped.try_reserve_exact(text.len().saturating_add(text.chars().count()))?;
        for (line_index, line) in text.split('\n').enumerate() {
            if line_index != 0 {
                wrapped.push('\n');
            }
            let width = shape_text(&face, line, ShapeDirection::Guessed)
                .glyph_positions()
                .iter()
                .fold(0_i64, |sum, position| {
                    sum.saturating_add(i64::from(position.x_advance))
                }) as f64
                * scale;
            if width <= max_width {
                wrapped.push_str(line);
                continue;
            }
            let mut line_width = 0.0;
            let mut has_character = false;
            for character in line.chars() {
                let advance = face
                    .glyph_index(character)
                    .and_then(|glyph| face.glyph_hor_advance(glyph))
                    .map_or(0.0, |advance| f64::from(advance) * scale);
                if has_character && line_width + advance > max_width {
                    wrapped.push('\n');
                    line_width = 0.0;
                }
                wrapped.push(character);
                line_width += advance;
                has_character = true;
            }
        }
        Ok(wrapped)
    }

    pub(super) fn into_parts(self) -> (&'static str, Arc<[u8]>) {
        (self.identity, self.sfnt)
    }

    fn supports(&self, character: char) -> bool {
        crate::shaping::Face::from_shared(&self.sfnt, 0)
            .and_then(|face| face.glyph_index(character))
            .is_some()
    }

    fn advance(
        &self,
        text: &str,
        font_size: f64,
        direction: ShapeDirection,
    ) -> Result<f64, LabelError> {
        let face =
            crate::shaping::Face::from_shared(&self.sfnt, 0).ok_or(LabelError::InvalidFont)?;
        let units = shape_text(&face, text, direction)
            .glyph_positions()
            .iter()
            .fold(0_i64, |sum, position| {
                sum.saturating_add(i64::from(position.x_advance))
            });
        Ok(units as f64 * font_size / f64::from(face.units_per_em()))
    }

    fn load_noto_emoji(
        character: char,
        loader: &mut FontLoader,
    ) -> Result<Option<Self>, LabelError> {
        let id = FamilyId::NotoEmoji.faces()[0].id();
        let sfnt = loader
            .load_sfnt(id)
            .map_err(|error| font_error(id, error))?;
        let face = crate::shaping::Face::from_shared(&sfnt, 0).ok_or(LabelError::InvalidFont)?;
        if face.glyph_index(character).is_none() {
            return Ok(None);
        }
        Ok(Some(Self::from_face(1_000, id.face(), sfnt)))
    }

    pub(super) fn matches_family(&self, font_family: i32) -> bool {
        self.source_family == font_family
            || matches!((self.source_family, font_family), (2, 9) | (9, 2))
    }
}

#[cfg(test)]
pub(super) fn layout_with_fallbacks(
    primary_index: usize,
    fonts: &mut Vec<OfficialFont>,
    text: &str,
    layout: TextLayout<'_>,
    trigger: CjkSubsetTrigger,
) -> Result<(ResolvedText, bool), LabelError> {
    layout_with_loader(
        primary_index,
        fonts,
        text,
        layout,
        trigger,
        &mut FontLoader::default(),
    )
}

pub(super) fn layout_with_loader(
    primary_index: usize,
    fonts: &mut Vec<OfficialFont>,
    text: &str,
    layout: TextLayout<'_>,
    trigger: CjkSubsetTrigger,
    loader: &mut FontLoader,
) -> Result<(ResolvedText, bool), LabelError> {
    let text_align = ResolvedTextAlign::from_normalized(layout.text_align);
    let line_step = layout.font_size * layout.line_height;
    let first_baseline = layout.first_baseline()?;
    // The base direction is the element's, detected once over the stored
    // `text` (spec 8.3, #166): the reference sets the canvas `dir` and every
    // line's SVG `direction` from `isRTL(element.text)`, so every source line
    // and every UAX #9 paragraph within one takes that level rather than
    // detecting its own from its first strong character.
    let base_level = if element_is_rtl(text) {
        unicode_bidi::Level::rtl()
    } else {
        unicode_bidi::Level::ltr()
    };
    let mut lines = Vec::new();
    let mut missing_codepoints = Vec::new();
    let mut used_noto = false;
    let mut fallback_probes = 0_u64;
    let mut resolved_characters = ResolvedCharacters::new();
    let mut budget = OutlineBudget::new(layout.outline_limit);

    for (line_index, line) in source_lines(text).enumerate() {
        let source_content = line.content;
        let baseline = first_baseline + line_index as f64 * line_step;
        let mut selections = Vec::<(usize, bool, usize, char)>::new();
        let mut line_missing = Vec::<(usize, u32)>::new();
        selections.try_reserve_exact(source_content.chars().count())?;
        let bidi = unicode_bidi::BidiInfo::new(source_content, Some(base_level));
        // Spec 8.3 keeps the reference's line rule, so a UAX #9 paragraph
        // separator (U+2029, U+0085, U+001C..U+001E) stays inside the source
        // line while unicode-bidi splits the line into paragraphs at it (P1).
        // Every paragraph is content: each is laid out in order, at the
        // element's base level, and the separator ends the paragraph it
        // belongs to. The paragraphs partition the line, so every scalar is
        // in some visual run; an empty line has none.
        let mut bidi_run = 0_usize;
        for paragraph in &bidi.paragraphs {
            let (levels, ranges) = bidi.visual_runs(paragraph, paragraph.range.clone());
            for range in ranges {
                let rtl = levels.get(range.start).is_some_and(|level| level.is_rtl());
                let run_text = source_content
                    .get(range.clone())
                    .ok_or(LabelError::InvalidFont)?;
                for (offset, character) in run_text.char_indices() {
                    // A default-ignorable code point is shaped, hidden, by the
                    // face of the run it falls in (#167): it consults no face
                    // and pays no probe. Its face is settled below, once its
                    // neighbours' are known.
                    if is_default_ignorable(character) {
                        selections.push((bidi_run, rtl, usize::MAX, character));
                        continue;
                    }
                    let (font_index, display, noto, missing) = resolve_character_cached(
                        primary_index,
                        fonts,
                        character,
                        trigger,
                        &mut resolved_characters,
                        &mut fallback_probes,
                        loader,
                    )?;
                    selections.push((bidi_run, rtl, font_index, display));
                    used_noto |= noto;
                    if missing {
                        line_missing.try_reserve(1)?;
                        line_missing
                            .push((range.start.saturating_add(offset), u32::from(character)));
                    }
                }
                bidi_run = bidi_run.saturating_add(1);
            }
        }
        line_missing.sort_unstable_by_key(|(offset, _)| *offset);
        missing_codepoints.try_reserve(line_missing.len())?;
        missing_codepoints.extend(line_missing.into_iter().map(|(_, codepoint)| codepoint));
        settle_default_ignorable_faces(&mut selections, primary_index);

        let mut grouped = Vec::<(usize, bool, usize, String)>::new();
        for (bidi_run, rtl, font_index, character) in selections {
            if let Some((last_bidi_run, _, last_index, run_text)) = grouped.last_mut() {
                if *last_bidi_run == bidi_run && *last_index == font_index {
                    run_text.push(character);
                    continue;
                }
            }
            grouped.try_reserve(1)?;
            let mut run_text = String::new();
            run_text.try_reserve(character.len_utf8())?;
            run_text.push(character);
            grouped.push((bidi_run, rtl, font_index, run_text));
        }
        // A visual run's face runs were grouped in logical order. Every glyph
        // run is placed left to right in visual order (spec 8.3), and a
        // right-to-left visual run reads its faces right to left: the first
        // in logical order is the rightmost, so its face runs are reversed.
        let mut start = 0;
        while let Some((bidi_run, rtl, ..)) = grouped.get(start) {
            let (bidi_run, rtl) = (*bidi_run, *rtl);
            let mut end = start.saturating_add(1);
            while grouped.get(end).is_some_and(|(next, ..)| *next == bidi_run) {
                end = end.saturating_add(1);
            }
            if rtl {
                grouped
                    .get_mut(start..end)
                    .ok_or(LabelError::InvalidFont)?
                    .reverse();
            }
            start = end;
        }

        let mut total_advance = 0.0;
        for (_, rtl, font_index, run_text) in &grouped {
            total_advance += fonts
                .get(*font_index)
                .ok_or(LabelError::InvalidFont)?
                .advance(
                    run_text,
                    layout.font_size,
                    ShapeDirection::Run { rtl: *rtl },
                )?;
        }
        let line_origin = match text_align {
            ResolvedTextAlign::Left => 0.0,
            ResolvedTextAlign::Center => (layout.width - total_advance) / 2.0,
            ResolvedTextAlign::Right => layout.width - total_advance,
        };
        let mut pen_x = line_origin;
        let mut operations = Vec::new();
        let mut glyphs = Vec::new();
        let mut runs = Vec::new();
        let mut display_text = String::new();
        for (_, rtl, font_index, run_text) in grouped {
            let font = fonts.get(font_index).ok_or(LabelError::InvalidFont)?;
            let face =
                crate::shaping::Face::from_shared(&font.sfnt, 0).ok_or(LabelError::InvalidFont)?;
            let direction = ShapeDirection::Run { rtl };
            let ShapedLine {
                label: mut run,
                reverses_cluster_glyphs,
            } = shape_official_line(
                &face,
                &run_text,
                &run_text,
                layout.width,
                layout.font_size,
                baseline,
                ResolvedTextAlign::Left,
                direction,
                &mut budget,
            )?;
            shift_label_x(&mut run, pen_x);
            let glyph_start = glyphs.len();
            let logical_glyphs = logical_glyph_indexes(
                &run.glyphs,
                &run_text,
                reverses_cluster_glyphs,
                glyph_start,
            )?;
            let outline_start = operations.len();
            for glyph in &mut run.glyphs {
                glyph.outline.start += outline_start;
                glyph.outline.end += outline_start;
            }
            operations.try_reserve(run.path.operations.len())?;
            operations.extend(run.path.operations);
            glyphs.try_reserve(run.glyphs.len())?;
            glyphs.extend(run.glyphs);
            let glyph_end = glyphs.len();
            display_text.try_reserve(run_text.len())?;
            display_text.push_str(&run_text);
            runs.try_reserve(1)?;
            runs.push(ResolvedTextRun {
                font_index,
                font_family: font.family.display_name(),
                display_text: run_text.clone(),
                glyph_start,
                glyph_end,
                rtl,
                logical_glyphs,
            });
            pen_x += font.advance(&run_text, layout.font_size, direction)?;
        }
        lines.try_reserve(1)?;
        lines.push(ResolvedLabel {
            text: line.source()?,
            display_text,
            baseline,
            path: LabelPath { operations },
            glyph_count: u64::try_from(glyphs.len()).unwrap_or(u64::MAX),
            missing_codepoints: Vec::new(),
            glyphs,
            runs,
        });
    }
    let discarded_outline_segments = budget.settle(&mut lines);
    let line_break_steps = u64::try_from(lines.len()).unwrap_or(u64::MAX);
    let mut fallback_indexes = Vec::new();
    for run in lines.iter().flat_map(|line| &line.runs) {
        if run.font_index != primary_index && !fallback_indexes.contains(&run.font_index) {
            fallback_indexes.push(run.font_index);
        }
    }
    Ok((
        ResolvedText {
            font_index: primary_index,
            font_family: fonts
                .get(primary_index)
                .ok_or(LabelError::InvalidFont)?
                .family
                .display_name(),
            font_size: layout.font_size,
            text_align,
            lines,
            scalar_count: u64::try_from(text.chars().count()).unwrap_or(u64::MAX),
            line_break_steps,
            missing_codepoints,
            noto_emoji_substituted: used_noto,
            fallback_probes,
            fallback_resources: u64::try_from(fallback_indexes.len()).unwrap_or(u64::MAX),
            prepaid_glyphs: prospective_glyph_count(text),
            prepaid_line_break_steps: line_count(text),
            discarded_outline_segments,
        },
        used_noto,
    ))
}

/// Gives every default-ignorable code point of one line's `selections`
/// (`(bidi run, rtl, face index, display)`, the face still `usize::MAX`) the
/// face of the nearest scalar of its own bidi visual run: the preceding one in
/// logical order, else the following one, else the whole-text face when the
/// run holds nothing else (a lone bidi control is its own UAX #9 run). So a
/// joiner between two emoji stays in their Noto Emoji run, and a mark inside
/// a Cyrillic run in the Cyrillic chunk's, and no run is split for a scalar
/// that draws nothing.
fn settle_default_ignorable_faces(selections: &mut [(usize, bool, usize, char)], primary: usize) {
    for index in 0..selections.len() {
        let (bidi_run, _, face, _) = selections[index];
        if face != usize::MAX {
            continue;
        }
        let neighbour = |candidates: &mut dyn Iterator<Item = usize>| {
            candidates
                .map(|candidate| selections[candidate])
                .take_while(|(run, ..)| *run == bidi_run)
                .map(|(_, _, face, _)| face)
                .find(|face| *face != usize::MAX)
        };
        let settled = neighbour(&mut (0..index).rev())
            .or_else(|| neighbour(&mut (index + 1..selections.len())))
            .unwrap_or(primary);
        selections[index].2 = settled;
    }
}

/// Whether `text` needs the UAX #9 layout of `layout_with_fallbacks` rather
/// than the whole-line shaping of `OfficialFont::layout`: the element's base
/// direction is right-to-left, or some scalar resolves to a right-to-left
/// level under it. A left-to-right element whose every line resolves to one
/// left-to-right run is unaffected by the base level and shapes whole.
pub(super) fn needs_bidi_layout(text: &str) -> bool {
    element_is_rtl(text) || unicode_bidi::BidiInfo::new(text, None).has_rtl()
}

/// The reference's `isRTL(element.text)`, which sets the canvas `dir`
/// attribute and every line's SVG `direction` for the whole element (#166):
///
/// ```js
/// const LTR = "A-Za-z\xC0-\xD6\xD8-\xF6\xF8-\u02B8\u0300-\u0590\u0800-\u1FFF\u2C00-\uFB1C\uFDFE-\uFE6F\uFEFD-\uFFFF";
/// const RTL = "\u0591-\u07FF\uFB1D-\uFDFD\uFE70-\uFEFC";
/// const isRTL = (text) => new RegExp(`^[^${LTR}]*[${RTL}]`).test(text);
/// ```
///
/// The text is right-to-left when, reading from the start and skipping every
/// unit in neither class (digits, spaces, punctuation, line terminators), the
/// first unit in either class is in the right-to-left one. The regex has no
/// `u` flag, so it reads UTF-16 code units: a surrogate falls inside the
/// `\u2C00-\uFB1C` range and every supplementary scalar therefore counts as
/// left-to-right, as do the combining marks of `\u0300-\u0590`. This is not
/// UAX #9 P2/P3 and is reproduced as the reference spells it.
pub(super) fn element_is_rtl(text: &str) -> bool {
    const LTR: &[(u16, u16)] = &[
        (0x41, 0x5a),
        (0x61, 0x7a),
        (0xc0, 0xd6),
        (0xd8, 0xf6),
        (0xf8, 0x2b8),
        (0x300, 0x590),
        (0x800, 0x1fff),
        (0x2c00, 0xfb1c),
        (0xfdfe, 0xfe6f),
        (0xfefd, 0xffff),
    ];
    const RTL: &[(u16, u16)] = &[(0x591, 0x7ff), (0xfb1d, 0xfdfd), (0xfe70, 0xfefc)];
    let within = |ranges: &[(u16, u16)], unit: u16| {
        ranges
            .iter()
            .any(|(start, end)| (*start..=*end).contains(&unit))
    };
    for unit in text.encode_utf16() {
        if within(LTR, unit) {
            return false;
        }
        if within(RTL, unit) {
            return true;
        }
    }
    false
}

fn resolve_character_font(
    primary_index: usize,
    fonts: &mut Vec<OfficialFont>,
    character: char,
    trigger: CjkSubsetTrigger,
    fallback_probes: &mut u64,
    loader: &mut FontLoader,
) -> Result<(usize, char, bool, bool), LabelError> {
    let primary = fonts.get(primary_index).ok_or(LabelError::InvalidFont)?;
    if primary.supports(character) {
        return Ok((primary_index, character, false, false));
    }
    let source_family = primary.source_family;
    let family = primary.family;
    if let Some(index) = select_catalog_font(
        fonts,
        source_family,
        family,
        character,
        fallback_probes,
        loader,
    )? {
        return Ok((index, character, false, false));
    }
    if source_family == EXCALIFONT_FONT_FAMILY && trigger.selects_xiaolai() {
        if let Some(index) = select_catalog_font(
            fonts,
            100,
            FamilyId::Xiaolai,
            character,
            fallback_probes,
            loader,
        )? {
            return Ok((index, character, false, false));
        }
    }
    // The Noto Emoji consultation is one font fallback probe per scalar
    // occurrence (spec 8.3.1), whether the complete face is selected now or
    // was selected by an earlier scalar: a face that already serves the
    // target is asked once and never decoded again, so what a scalar pays
    // never depends on which scalars came before it.
    *fallback_probes = fallback_probes.saturating_add(1);
    if let Some(index) = fonts
        .iter()
        .position(|font| font.family == FamilyId::NotoEmoji)
    {
        if fonts
            .get(index)
            .is_some_and(|font| font.supports(character))
        {
            return Ok((index, character, true, false));
        }
    } else if let Some(font) = OfficialFont::load_noto_emoji(character, loader)? {
        fonts.try_reserve(1)?;
        fonts.push(font);
        return Ok((fonts.len() - 1, character, true, false));
    }
    let replacement =
        select_catalog_font(fonts, 9, FamilyId::Liberation, '?', fallback_probes, loader)?
            .ok_or(LabelError::InvalidFont)?;
    Ok((replacement, '?', false, true))
}

/// One element's settled scalar resolutions, keyed by scalar: the face index,
/// the displayed scalar, whether Noto Emoji served it, whether it is missing,
/// and the fallback probes its first resolution paid. The map is only ever
/// looked up, never iterated, so its order cannot reach the output; the keyed
/// lookup keeps a text of many distinct scalars linear rather than quadratic.
type ResolvedCharacters = HashMap<char, (usize, char, bool, bool, u64)>;

fn resolve_character_cached(
    primary_index: usize,
    fonts: &mut Vec<OfficialFont>,
    character: char,
    trigger: CjkSubsetTrigger,
    cache: &mut ResolvedCharacters,
    fallback_probes: &mut u64,
    loader: &mut FontLoader,
) -> Result<(usize, char, bool, bool), LabelError> {
    if let Some((font_index, display, noto, missing, probes)) = cache.get(&character) {
        *fallback_probes = fallback_probes.saturating_add(*probes);
        return Ok((*font_index, *display, *noto, *missing));
    }
    let probes_before = *fallback_probes;
    let resolved = resolve_character_font(
        primary_index,
        fonts,
        character,
        trigger,
        fallback_probes,
        loader,
    )?;
    let probes = fallback_probes.saturating_sub(probes_before);
    cache.try_reserve(1)?;
    cache.insert(
        character,
        (resolved.0, resolved.1, resolved.2, resolved.3, probes),
    );
    Ok(resolved)
}

fn select_catalog_font(
    fonts: &mut Vec<OfficialFont>,
    source_family: i32,
    family: FamilyId,
    character: char,
    fallback_probes: &mut u64,
    loader: &mut FontLoader,
) -> Result<Option<usize>, LabelError> {
    let mut chunks = OfficialFontChunks::load(family, loader);
    // Only the official font chunks whose declared Unicode range covers the
    // scalar are probed, in pinned face order; a chunk's cmap never selects it
    // for a scalar outside its declared ranges. A chunk already loaded for the
    // scene is reused rather than decoded again.
    for index in 0..chunks.len() {
        if !chunks.declares(index, character) {
            continue;
        }
        *fallback_probes = fallback_probes.saturating_add(1);
        let identity = chunks.face(index)?.identity();
        if let Some(loaded) = fonts.iter().position(|font| font.identity == identity) {
            if fonts
                .get(loaded)
                .is_some_and(|font| font.supports(character))
            {
                return Ok(Some(loaded));
            }
            continue;
        }
        if !sfnt_maps(&chunks.decode(index)?, character) {
            continue;
        }
        let (face, sfnt) = chunks.take(index)?;
        fonts.try_reserve(1)?;
        fonts.push(OfficialFont::from_face(source_family, face, sfnt));
        return Ok(Some(fonts.len() - 1));
    }
    Ok(None)
}

/// Whether one decoded face maps a scalar to a glyph.
fn sfnt_maps(sfnt: &Arc<[u8]>, character: char) -> bool {
    crate::shaping::Face::from_shared(sfnt, 0)
        .and_then(|face| face.glyph_index(character))
        .is_some()
}

/// Selection over one family's immutable declarations. Loading is delegated
/// to the target's physical cache, independent of semantic fallback probes.
struct OfficialFontChunks<'a> {
    chunks: &'static [Face],
    loader: &'a mut FontLoader,
}

impl<'a> OfficialFontChunks<'a> {
    fn load(family: FamilyId, loader: &'a mut FontLoader) -> Self {
        Self {
            chunks: family.faces(),
            loader,
        }
    }

    /// Whether the chunk at `index` declares a scalar in its Unicode ranges.
    fn declares(&self, index: usize, character: char) -> bool {
        self.chunks
            .get(index)
            .is_some_and(|chunk| chunk.declares(character))
    }

    fn len(&self) -> usize {
        self.chunks.len()
    }

    /// The first chunk, in pinned face order, whose declared ranges cover every
    /// scalar `scalars` yields and whose cmap maps them all.
    fn first_declaring<I: Iterator<Item = char>>(
        &mut self,
        scalars: impl Fn() -> I,
    ) -> Result<Option<usize>, LabelError> {
        for index in 0..self.chunks.len() {
            let chunk = self.chunks.get(index).ok_or(LabelError::InvalidFont)?;
            if !scalars().all(|character| chunk.declares(character)) {
                continue;
            }
            let sfnt = self.decode(index)?;
            if scalars().all(|character| sfnt_maps(&sfnt, character)) {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    fn face(&self, index: usize) -> Result<&'static Face, LabelError> {
        self.chunks.get(index).ok_or(LabelError::InvalidFont)
    }

    fn decode(&mut self, index: usize) -> Result<Arc<[u8]>, LabelError> {
        let id = self.chunks.get(index).ok_or(LabelError::InvalidFont)?.id();
        self.loader
            .load_sfnt(id)
            .map_err(|error| font_error(id, error))
    }

    /// The record and decoded bytes of one chunk, decoding it if needed.
    fn take(&mut self, index: usize) -> Result<(&'static Face, Arc<[u8]>), LabelError> {
        let sfnt = self.decode(index)?;
        Ok((self.face(index)?, sfnt))
    }
}

/// For each scalar of `display_text` in logical order, the index of the glyph
/// in `glyphs` that shows it, offset by `first_index` so the result indexes
/// the line's glyph list the run's glyphs are appended to.
///
/// The shaper groups scalars and glyphs into clusters keyed by the byte offset
/// of the cluster's first scalar. Within a cluster, the i-th scalar is shown
/// by the i-th glyph in logical order, and scalars beyond the cluster's glyph
/// count share its last glyph; a scalar that starts no cluster belongs to the
/// last cluster starting before it. A one-glyph-per-scalar run therefore maps
/// each scalar to its own glyph, and a mark keeps the position the shaper gave
/// it over its base. `glyphs` are in the order the line places them: buffer
/// order, which for a right-to-left run is visual order. Clusters are found by
/// their keys whatever their order; `reversed` says whether each cluster's
/// glyphs are reversed from logical order as well, which the shaper decides
/// ([`crate::shaping::reverses_cluster_glyphs`]).
fn logical_glyph_indexes(
    glyphs: &[ResolvedGlyph],
    display_text: &str,
    reversed: bool,
    first_index: usize,
) -> Result<Vec<usize>, LabelError> {
    // `(cluster, logical position)` per glyph; sorting by that total key
    // groups each cluster's glyphs in logical order.
    let mut ordered = Vec::<(usize, usize, usize)>::new();
    ordered.try_reserve_exact(glyphs.len())?;
    for (index, glyph) in glyphs.iter().enumerate() {
        let logical = if reversed {
            glyphs.len().saturating_sub(1).saturating_sub(index)
        } else {
            index
        };
        ordered.push((glyph.cluster, logical, index));
    }
    ordered.sort_unstable_by_key(|(cluster, logical, _)| (*cluster, *logical));

    let mut indexes = Vec::new();
    indexes.try_reserve_exact(display_text.chars().count())?;
    // The glyphs of the cluster the current scalar belongs to, as a range of
    // `ordered`, and how many of its scalars have been mapped.
    let mut cluster = 0..0;
    let mut mapped = 0_usize;
    for (offset, _) in display_text.char_indices() {
        while let Some((start, ..)) = ordered.get(cluster.end) {
            if *start > offset {
                break;
            }
            let mut end = cluster.end.saturating_add(1);
            while ordered.get(end).is_some_and(|(next, ..)| next == start) {
                end = end.saturating_add(1);
            }
            cluster = cluster.end..end;
            mapped = 0;
        }
        let (.., index) = ordered
            .get(
                cluster
                    .start
                    .saturating_add(mapped.min(cluster.len().saturating_sub(1))),
            )
            .filter(|_| !cluster.is_empty())
            .ok_or(LabelError::InvalidFont)?;
        indexes.push(index.saturating_add(first_index));
        mapped = mapped.saturating_add(1);
    }
    Ok(indexes)
}

fn shift_label_x(label: &mut ResolvedLabel, delta: f64) {
    for operation in &mut label.path.operations {
        match operation {
            LabelPathOperation::Move { x, .. } | LabelPathOperation::Line { x, .. } => *x += delta,
            LabelPathOperation::Quadratic { control_x, x, .. } => {
                *control_x += delta;
                *x += delta;
            }
            LabelPathOperation::Close => {}
        }
    }
    for glyph in &mut label.glyphs {
        glyph.x += delta;
    }
}

struct StoredLine {
    source: String,
    display: String,
}

/// One line of a text element's `text`, split the way the reference renders
/// it. `drawElementOnCanvas` (`renderElement.ts`) and `renderElementToSvg`
/// (`staticSvgScene.ts`) both lay out
/// `element.text.replace(/\r\n?/g, "\n").split("\n")`, so `\r\n`, `\r`, and
/// `\n` each end a line and the text always has at least one. Neither rewrites
/// `text`, so the source bytes are kept, terminator included: the per-line
/// source text (`data-source-text`, `ActualText`) still concatenates to the
/// stored `text`.
struct SourceLine<'a> {
    /// The line without its terminator: what is shaped.
    content: &'a str,
    /// `"\r\n"`, `"\r"`, `"\n"`, or `""` on the last line.
    terminator: &'a str,
}

impl SourceLine<'_> {
    /// The line's source bytes, terminator included.
    fn source(&self) -> Result<String, LabelError> {
        let mut source = String::new();
        source.try_reserve_exact(self.content.len().saturating_add(self.terminator.len()))?;
        source.push_str(self.content);
        source.push_str(self.terminator);
        Ok(source)
    }
}

/// The `SourceLine`s of one text, in order; `None` once the last line, the one
/// with no terminator, has been yielded.
struct SourceLines<'a> {
    rest: Option<&'a str>,
}

impl<'a> Iterator for SourceLines<'a> {
    type Item = SourceLine<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.rest?;
        // Terminators are ASCII, so every index here is a char boundary.
        let Some(index) = rest.find(is_line_terminator) else {
            self.rest = None;
            return Some(SourceLine {
                content: rest,
                terminator: "",
            });
        };
        let terminator_len = if rest[index..].starts_with("\r\n") {
            2
        } else {
            1
        };
        let (line, remaining) = rest.split_at(index + terminator_len);
        self.rest = Some(remaining);
        Some(SourceLine {
            content: &line[..index],
            terminator: &line[index..],
        })
    }
}

/// The lines of `text` by the reference's line-ending rule; see `SourceLine`.
fn source_lines(text: &str) -> SourceLines<'_> {
    SourceLines { rest: Some(text) }
}

/// Whether `character` ends a line in the reference's text rendering.
fn is_line_terminator(character: char) -> bool {
    matches!(character, '\n' | '\r')
}

/// How many lines `text` lays out to: one more than its line terminators,
/// counting `\r\n` once. The ledger prepays one line step per line.
pub(super) fn line_count(text: &str) -> u64 {
    u64::try_from(source_lines(text).count()).unwrap_or(u64::MAX)
}

/// The scalars of `text` that reach the shaper: everything but its line
/// terminators. The ledger prepays one glyph per such scalar.
pub(super) fn prospective_glyph_count(text: &str) -> u64 {
    u64::try_from(
        text.chars()
            .filter(|character| !is_line_terminator(*character))
            .count(),
    )
    .unwrap_or(u64::MAX)
}

impl ResolvedText {
    pub(super) fn glyph_count(&self) -> u64 {
        self.lines
            .iter()
            .fold(0_u64, |total, line| total.saturating_add(line.glyph_count))
    }

    /// The outline operations this text holds, without the discarded ones.
    pub(super) fn kept_outline_segments(&self) -> u64 {
        self.lines.iter().fold(0_u64, |total, line| {
            total.saturating_add(u64::try_from(line.path.operations.len()).unwrap_or(u64::MAX))
        })
    }

    pub(super) fn outline_segments(&self) -> u64 {
        self.lines
            .iter()
            .fold(self.discarded_outline_segments, |total, line| {
                total.saturating_add(u64::try_from(line.path.operations.len()).unwrap_or(u64::MAX))
            })
    }

    pub(super) fn run_count(&self) -> u64 {
        self.lines.iter().fold(0_u64, |total, line| {
            total.saturating_add(if line.runs.is_empty() {
                1
            } else {
                u64::try_from(line.runs.len()).unwrap_or(u64::MAX)
            })
        })
    }

    pub(super) fn fallback_resource_count(&self) -> u64 {
        self.fallback_resources
    }

    pub(super) fn retain_font_indices(&self, indexes: &mut Vec<usize>) {
        if self.lines.iter().all(|line| line.runs.is_empty()) {
            if !indexes.contains(&self.font_index) {
                indexes.push(self.font_index);
            }
            return;
        }
        for run in self.lines.iter().flat_map(|line| &line.runs) {
            if !indexes.contains(&run.font_index) {
                indexes.push(run.font_index);
            }
        }
    }

    pub(super) fn remap_font_indices(
        &mut self,
        mapping: &[Option<usize>],
    ) -> Result<(), LabelError> {
        let remapped_primary = mapping.get(self.font_index).copied().flatten();
        for run in self.lines.iter_mut().flat_map(|line| &mut line.runs) {
            run.font_index = mapping
                .get(run.font_index)
                .copied()
                .flatten()
                .ok_or(LabelError::InvalidFont)?;
        }
        self.font_index = remapped_primary
            .or_else(|| {
                self.lines
                    .iter()
                    .flat_map(|line| &line.runs)
                    .next()
                    .map(|run| run.font_index)
            })
            .ok_or(LabelError::InvalidFont)?;
        Ok(())
    }
}

/// One shaped line, and whether the shaper returned each of its clusters'
/// glyphs in reverse logical order ([`crate::shaping::reverses_cluster_glyphs`]).
struct ShapedLine {
    label: ResolvedLabel,
    reverses_cluster_glyphs: bool,
}

#[allow(clippy::too_many_arguments)]
fn shape_official_line(
    face: &crate::shaping::Face,
    source_text: &str,
    display_text: &str,
    width: f64,
    font_size: f64,
    baseline: f64,
    text_align: ResolvedTextAlign,
    direction: ShapeDirection,
    budget: &mut OutlineBudget,
) -> Result<ShapedLine, LabelError> {
    let glyphs = shape_text(face, display_text, direction);
    let scale = font_size / f64::from(face.units_per_em());
    let advance = glyphs
        .glyph_positions()
        .iter()
        .fold(0_i64, |sum, position| {
            sum.saturating_add(i64::from(position.x_advance))
        }) as f64
        * scale;
    let origin_x = match text_align {
        ResolvedTextAlign::Center => (width - advance) / 2.0,
        ResolvedTextAlign::Right => width - advance,
        ResolvedTextAlign::Left => 0.0,
    };
    let mut operations = Vec::new();
    operations.try_reserve(budget.reservation(glyphs.glyph_infos().len()))?;
    let mut missing_codepoints = Vec::new();
    let mut resolved_glyphs = Vec::new();
    resolved_glyphs.try_reserve_exact(glyphs.glyph_infos().len())?;
    let mut pen_x = 0_i64;
    let mut pen_y = 0_i64;
    let cluster_starts = ClusterStarts::of(&glyphs)?;
    let mut previous_unicode = None;
    for (info, position) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
        let glyph_id = u16::try_from(info.glyph_id).map_err(|_| LabelError::InvalidFont)?;
        let cluster = usize::try_from(info.cluster).map_err(|_| LabelError::InvalidFont)?;
        let cluster_text = cluster_starts.text(display_text, cluster)?;
        if glyph_id == 0 {
            if let Some(codepoint) = cluster_text.chars().next().map(u32::from) {
                missing_codepoints.try_reserve(1)?;
                missing_codepoints.push(codepoint);
            }
        }
        let unicode = cluster_unicode(&mut previous_unicode, cluster, cluster_text)?;
        let advance = face.glyph_hor_advance(GlyphId(glyph_id));
        let advance_units = advance.unwrap_or_default();
        let units_per_em = u64::from(face.units_per_em());
        let advance_1000 =
            u16::try_from((u64::from(advance_units) * 1_000 + units_per_em / 2) / units_per_em)
                .map_err(|_| LabelError::InvalidFont)?;
        let outline_start = operations.len();
        let mut builder = GlyphOutlineBuilder {
            operations: &mut operations,
            budget,
            failed: false,
            origin_x,
            baseline,
            glyph_x: (pen_x + i64::from(position.x_offset)) as f64,
            glyph_y: (pen_y + i64::from(position.y_offset)) as f64,
            scale,
        };
        // A glyph the font defines but leaves without an outline draws
        // nothing and keeps its advance. That is ordinary font content, not
        // corruption: Cascadia Code spells its `calt` ligatures as empty `LIG`
        // spacers followed by one glyph that carries the joined outline. Only
        // a glyph the face has no metrics for at all is a sealed-font failure.
        let outlined = face
            .outline_glyph(GlyphId(glyph_id), &mut builder)
            .is_some();
        if builder.failed {
            return Err(LabelError::Memory);
        }
        if !outlined && advance.is_none() {
            return Err(LabelError::InvalidFont);
        }
        resolved_glyphs.push(ResolvedGlyph {
            id: glyph_id,
            x: origin_x + (pen_x + i64::from(position.x_offset)) as f64 * scale,
            y: baseline - (pen_y + i64::from(position.y_offset)) as f64 * scale,
            advance_1000,
            unicode,
            cluster,
            outline: outline_start..operations.len(),
        });
        pen_x = pen_x.saturating_add(i64::from(position.x_advance));
        pen_y = pen_y.saturating_add(i64::from(position.y_advance));
    }
    let mut owned_text = String::new();
    owned_text.try_reserve_exact(source_text.len())?;
    owned_text.push_str(source_text);
    let mut owned_display_text = String::new();
    owned_display_text.try_reserve_exact(display_text.len())?;
    owned_display_text.push_str(display_text);
    Ok(ShapedLine {
        label: ResolvedLabel {
            text: owned_text,
            display_text: owned_display_text,
            baseline,
            path: LabelPath { operations },
            glyph_count: u64::try_from(glyphs.glyph_infos().len()).unwrap_or(u64::MAX),
            missing_codepoints,
            glyphs: resolved_glyphs,
            runs: Vec::new(),
        },
        reverses_cluster_glyphs: crate::shaping::reverses_cluster_glyphs(&glyphs),
    })
}

fn shape_text(
    face: &crate::shaping::Face,
    text: &str,
    direction: ShapeDirection,
) -> crate::shaping::Buffer {
    let direction = match direction {
        ShapeDirection::Run { rtl: true } => Some(crate::shaping::Direction::RightToLeft),
        ShapeDirection::Run { rtl: false } => Some(crate::shaping::Direction::LeftToRight),
        ShapeDirection::Guessed => None,
    };
    let features = [
        crate::shaping::Feature::new(crate::shaping::Tag::new(b"liga"), 0, ..),
        crate::shaping::Feature::new(crate::shaping::Tag::new(b"clig"), 0, ..),
    ];
    crate::shaping::shape(face, &features, text, direction)
}

/// The direction text is shaped in.
#[derive(Clone, Copy)]
enum ShapeDirection {
    /// The shaper's guess from the text's script: left to right unless the
    /// text carries a right-to-left script. Text shaped this way has had no
    /// UAX #9 resolution: the primary path's whole line, whose element needs
    /// no bidi layout or it would take `layout_with_fallbacks`; the
    /// embeddable label wrapper's width measurements; and the frame name and
    /// placeholder labels `FallbackFont` shapes whole after `?`-replacement.
    Guessed,
    /// The resolved bidi level of one visual run (spec 8.3). A right-to-left
    /// run's glyphs come back in visual order, leftmost first, with mirrored
    /// characters showing their mirror glyph when the face has one (UAX #9
    /// L4); guessing from the run's display text would shape replacement
    /// glyphs and neutrals left to right instead.
    Run { rtl: bool },
}

fn try_owned(value: &str) -> Result<String, LabelError> {
    let mut owned = String::new();
    owned.try_reserve_exact(value.len())?;
    owned.push_str(value);
    Ok(owned)
}

/// The UTF-16 text of the cluster at `cluster`, shared with the previous
/// glyph's when that glyph is of the same cluster: a cluster of many glyphs
/// would otherwise hold its whole text once per glyph.
fn cluster_unicode(
    previous: &mut Option<(usize, Arc<Vec<u16>>)>,
    cluster: usize,
    cluster_text: &str,
) -> Result<Arc<Vec<u16>>, LabelError> {
    if let Some((previous_cluster, unicode)) = previous {
        if *previous_cluster == cluster {
            return Ok(Arc::clone(unicode));
        }
    }
    let mut unicode = Vec::new();
    unicode.try_reserve_exact(cluster_text.encode_utf16().count())?;
    unicode.extend(cluster_text.encode_utf16());
    let unicode = Arc::new(unicode);
    *previous = Some((cluster, Arc::clone(&unicode)));
    Ok(unicode)
}

/// The distinct cluster starts of one shaped run, ascending whatever the run's
/// visual order, so each glyph finds where its cluster's text ends without
/// scanning every other glyph.
struct ClusterStarts(Vec<usize>);

impl ClusterStarts {
    fn of(glyphs: &crate::shaping::Buffer) -> Result<Self, LabelError> {
        Self::new(glyphs.glyph_infos().iter().map(|info| info.cluster))
    }

    fn new(clusters: impl ExactSizeIterator<Item = u32>) -> Result<Self, LabelError> {
        let mut starts = Vec::new();
        starts.try_reserve_exact(clusters.len())?;
        starts.extend(clusters.filter_map(|cluster| usize::try_from(cluster).ok()));
        starts.sort_unstable();
        starts.dedup();
        Ok(Self(starts))
    }

    /// The text of the cluster starting at `cluster`: up to the nearest
    /// greater cluster start, else to the end of `text`.
    fn text<'a>(&self, text: &'a str, cluster: usize) -> Result<&'a str, LabelError> {
        let next = self.0.partition_point(|start| *start <= cluster);
        let cluster_end = self.0.get(next).copied().unwrap_or(text.len());
        text.get(cluster..cluster_end)
            .ok_or(LabelError::InvalidFont)
    }
}

/// The font notices a PDF carries for the complete programs it embeds (spec
/// 12, #151): one attribution notice per font family whose chunk or fallback
/// face is embedded, plus the Liberation Sans family's when a generated label
/// used the fallback font, and each license body once.
pub(super) fn mandatory_notices_json(
    liberation: bool,
    official: &[super::font_demand::ResolvedFont],
) -> Result<super::notices::MandatoryNotices, LabelError> {
    let mut notices = super::notices::FontNoticeSet::default();
    let notice_error = |error| match error {
        super::notices::NoticeError::Memory => LabelError::Memory,
        #[cfg(feature = "png")]
        super::notices::NoticeError::InvalidProvenance => LabelError::InvalidFont,
    };
    if liberation {
        let notice =
            crate::catalog::font_notice(LIBERATION_SANS_IDENTITY).ok_or(LabelError::InvalidFont)?;
        notices.add_family(notice).map_err(notice_error)?;
    }
    for font in official {
        let notice = crate::catalog::font_notice(font.identity).ok_or(LabelError::InvalidFont)?;
        notices.add_family(notice).map_err(notice_error)?;
    }
    notices.into_manifest().map_err(notice_error)
}

#[cfg(all(test, not(feature = "cjk")))]
mod optional_font_tests {
    use super::*;

    #[test]
    fn unavailable_chunks_fail_only_when_declared_coverage_is_demanded() {
        let mut loader = FontLoader::default();
        let mut chunks = OfficialFontChunks::load(FamilyId::Xiaolai, &mut loader);
        assert!(chunks.len() > 0);
        assert!(chunks
            .first_declaring(|| "\u{10ffff}".chars())
            .expect("uncovered scalars do not load a chunk")
            .is_none());
        let error = chunks.first_declaring(|| "中".chars()).unwrap_err();
        let LabelError::FontUnavailable(font) = error else {
            panic!("expected unavailable font, got {error:?}");
        };
        assert_eq!(font.resolved_family, "Xiaolai");
        let record = crate::fonts::revision_1()
            .assets
            .iter()
            .find(|record| record.identity == font.asset_identity)
            .expect("error names the catalog chunk");
        assert!(!record.is_available());
    }
}

struct GlyphOutlineBuilder<'a> {
    operations: &'a mut Vec<LabelPathOperation>,
    budget: &'a mut OutlineBudget,
    failed: bool,
    origin_x: f64,
    baseline: f64,
    glyph_x: f64,
    glyph_y: f64,
    scale: f64,
}

impl GlyphOutlineBuilder<'_> {
    fn point(&self, x: f32, y: f32) -> (f64, f64) {
        (
            self.origin_x + (self.glyph_x + f64::from(x)) * self.scale,
            self.baseline - (self.glyph_y + f64::from(y)) * self.scale,
        )
    }

    fn push(&mut self, operation: LabelPathOperation) {
        if self.failed {
            return;
        }
        self.budget.counted = self.budget.counted.saturating_add(1);
        if self.budget.exceeded() {
            return;
        }
        if self.operations.try_reserve(1).is_err() {
            self.failed = true;
        } else {
            self.operations.push(operation);
        }
    }
}

impl OutlineBuilder for GlyphOutlineBuilder<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.push(LabelPathOperation::Move { x, y });
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.push(LabelPathOperation::Line { x, y });
    }

    fn quad_to(&mut self, control_x: f32, control_y: f32, x: f32, y: f32) {
        let (control_x, control_y) = self.point(control_x, control_y);
        let (x, y) = self.point(x, y);
        self.push(LabelPathOperation::Quadratic {
            control_x,
            control_y,
            x,
            y,
        });
    }

    fn close(&mut self) {
        self.push(LabelPathOperation::Close);
    }
}
#[cfg(test)]
mod cluster_tests {
    use super::*;

    /// The text of the cluster at `cluster` as it was found before the starts
    /// were indexed: the nearest greater start among all glyphs.
    fn scanned(clusters: &[u32], text: &str, cluster: usize) -> Option<String> {
        let end = clusters
            .iter()
            .filter_map(|candidate| usize::try_from(*candidate).ok())
            .filter(|candidate| *candidate > cluster)
            .min()
            .unwrap_or(text.len());
        text.get(cluster..end).map(str::to_owned)
    }

    #[test]
    fn indexed_cluster_text_matches_the_nearest_greater_cluster_start() {
        let text = "a\u{301}b\u{10ffff}c d\u{5d0}\u{5d1}";
        let mut state = 0x2545_f491_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for _ in 0..2_000 {
            // Clusters in any order, repeated, past the text or off a
            // character boundary, as logical, visual or merged runs give them.
            let clusters = (0..next() % 12)
                .map(|_| next() % (u32::try_from(text.len()).expect("short") + 3))
                .collect::<Vec<_>>();
            let starts = ClusterStarts::new(clusters.iter().copied()).expect("starts index");
            for &cluster in &clusters {
                let cluster = usize::try_from(cluster).expect("cluster fits");
                assert_eq!(
                    starts.text(text, cluster).ok().map(str::to_owned),
                    scanned(&clusters, text, cluster),
                    "{clusters:?} at {cluster}"
                );
            }
        }
    }

    #[test]
    fn cluster_text_of_a_long_run_is_not_quadratic() {
        // A scan per glyph took seconds here; an indexed lookup takes
        // milliseconds. Right-to-left runs list their clusters descending.
        let text = "\u{200b}".repeat(400_000);
        let clusters = (0..text.len())
            .step_by('\u{200b}'.len_utf8())
            .rev()
            .map(|cluster| u32::try_from(cluster).expect("cluster fits"))
            .collect::<Vec<_>>();
        let started = std::time::Instant::now();
        let starts = ClusterStarts::new(clusters.iter().copied()).expect("starts index");
        for &cluster in &clusters {
            let cluster = usize::try_from(cluster).expect("cluster fits");
            assert_eq!(
                starts.text(&text, cluster).expect("cluster text"),
                "\u{200b}"
            );
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
