//! The font boundary (#202): one read-fonts parse serves HarfRust's shaping
//! and the renderer's metrics, character map and outlines. Those keep the
//! rules of ttf-parser 0.25, which the renderer measured with until HarfRust,
//! write-fonts and skera shared a read-fonts line: the same integer metrics,
//! the first Unicode subtable that maps a scalar, advances only for glyph IDs
//! the font has, and ttf-parser's outline pen stream ([`glyf`]).

mod glyf;

use std::sync::Arc;

use write_fonts::read::tables::cmap::PlatformId;
use write_fonts::read::tables::os2::{Os2, SelectionFlags};
use write_fonts::read::TableProvider;

pub(crate) use glyf::{OutlineBuilder, Rect};
pub(crate) use harfrust::{Buffer, Direction, Feature, Tag};

/// A glyph ID in a face with at most 65,535 glyphs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct GlyphId(pub(crate) u16);

pub(crate) struct Face {
    // HarfRust's font owns its bytes and caches its layout data on first shape.
    font: harfrust::Font,
}

impl Face {
    /// Parses face `index` of an SFNT, keeping a copy of `data`;
    /// [`Face::from_shared`] shares it instead.
    pub(crate) fn from_slice(data: &[u8], index: u32) -> Option<Self> {
        Self::new(data.to_vec().into(), index)
    }

    /// Parses face `index` of a shared SFNT without copying it.
    pub(crate) fn from_shared(data: &Arc<[u8]>, index: u32) -> Option<Self> {
        let shared: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(Arc::clone(data));
        Self::new(shared.into(), index)
    }

    /// A face must carry a `head` with 16 to 16,384 units per em and a known
    /// `loca` format, an `hhea`, and a `maxp` with at least one glyph.
    fn new(blob: harfrust::font::Blob, index: u32) -> Option<Self> {
        let font = harfrust::Font::new(blob, index)?;
        let tables = font.tables();
        let head = tables.head().ok()?;
        if !(16..=16384).contains(&head.units_per_em())
            || !matches!(head.index_to_loc_format(), 0 | 1)
        {
            return None;
        }
        tables.hhea().ok()?;
        let maxp = tables.maxp().ok()?;
        let version = u32::from_be_bytes(maxp.version().to_be_bytes());
        if !matches!(version, 0x0000_5000 | 0x0001_0000) || maxp.num_glyphs() == 0 {
            return None;
        }
        Some(Self { font })
    }

    pub(crate) fn tables(&self) -> &harfrust::font::Tables {
        self.font.tables()
    }

    pub(crate) fn units_per_em(&self) -> u16 {
        self.font.units_per_em()
    }

    #[cfg(test)]
    pub(crate) fn number_of_glyphs(&self) -> u16 {
        self.tables().maxp().map_or(0, |maxp| maxp.num_glyphs())
    }

    fn typographic(&self) -> Option<Os2<'_>> {
        self.tables().os2().ok().filter(|os2| {
            os2.version() >= 4
                && os2
                    .fs_selection()
                    .contains(SelectionFlags::USE_TYPO_METRICS)
        })
    }

    /// `hhea`'s ascender, or OS/2's when `USE_TYPO_METRICS` is set or `hhea`
    /// has none.
    pub(crate) fn ascender(&self) -> i16 {
        if let Some(os2) = self.typographic() {
            return os2.s_typo_ascender();
        }
        let value = self.hhea_line().0;
        if value != 0 {
            return value;
        }
        match self.tables().os2() {
            Ok(os2) if os2.s_typo_ascender() != 0 => os2.s_typo_ascender(),
            Ok(os2) => os2.us_win_ascent() as i16,
            Err(_) => value,
        }
    }

    /// `hhea`'s descender, or OS/2's when `USE_TYPO_METRICS` is set or `hhea`
    /// has none.
    pub(crate) fn descender(&self) -> i16 {
        if let Some(os2) = self.typographic() {
            return os2.s_typo_descender();
        }
        let value = self.hhea_line().1;
        if value != 0 {
            return value;
        }
        match self.tables().os2() {
            Ok(os2) if os2.s_typo_descender() != 0 => os2.s_typo_descender(),
            Ok(os2) => (os2.us_win_descent() as i16).wrapping_neg(),
            Err(_) => value,
        }
    }

    /// `hhea`'s line gap, or OS/2's when `USE_TYPO_METRICS` is set or `hhea`
    /// lacks an ascender or descender.
    pub(crate) fn line_gap(&self) -> i16 {
        if let Some(os2) = self.typographic() {
            return os2.s_typo_line_gap();
        }
        let (ascender, descender, value) = self.hhea_line();
        if ascender != 0 && descender != 0 {
            return value;
        }
        match self.tables().os2() {
            Ok(os2) if os2.s_typo_ascender() != 0 || os2.s_typo_descender() != 0 => {
                os2.s_typo_line_gap()
            }
            Ok(_) => 0,
            Err(_) => value,
        }
    }

    fn hhea_line(&self) -> (i16, i16, i16) {
        // The constructor checked that `hhea` parses.
        self.tables().hhea().map_or((0, 0, 0), |hhea| {
            (
                hhea.ascender().to_i16(),
                hhea.descender().to_i16(),
                hhea.line_gap().to_i16(),
            )
        })
    }

    /// The glyph that the first Unicode subtable mapping `c` maps it to. Unicode
    /// subtables are the Unicode platform's, Windows BMP, and Windows full
    /// repertoire in format 12 or 13. A subtable does not map `c` to a glyph
    /// ID past `u16`, nor to a format 0 subtable's zero.
    pub(crate) fn glyph_index(&self, c: char) -> Option<GlyphId> {
        let cmap = self.tables().cmap().ok()?;
        let data = cmap.offset_data();
        for record in cmap.encoding_records() {
            let Ok(subtable) = record.subtable(data) else {
                continue;
            };
            let unicode = match record.platform_id() {
                PlatformId::Unicode => true,
                PlatformId::Windows => {
                    record.encoding_id() == 1
                        || (record.encoding_id() == 10 && matches!(subtable.format(), 12 | 13))
                }
                _ => false,
            };
            if !unicode {
                continue;
            }
            let glyph = subtable
                .map_codepoint(c)
                .filter(|glyph| subtable.format() != 0 || glyph.to_u32() != 0)
                .and_then(|glyph| u16::try_from(glyph.to_u32()).ok());
            if let Some(glyph) = glyph {
                return Some(GlyphId(glyph));
            }
        }
        None
    }

    /// `hmtx`'s advance for a glyph the font has, and `None` past its glyphs.
    pub(crate) fn glyph_hor_advance(&self, glyph: GlyphId) -> Option<u16> {
        let tables = self.tables();
        let metrics = tables.hhea().ok()?.number_of_h_metrics();
        if metrics == 0 {
            return None;
        }
        let glyphs = tables.maxp().ok()?.num_glyphs();
        if glyph.0 >= metrics.max(glyphs) {
            return None;
        }
        let long = tables.hmtx().ok()?.h_metrics();
        if long.len() < usize::from(metrics) {
            return None;
        }
        long.get(usize::from(glyph.0))
            .or(long.last())
            .map(|metric| metric.advance())
    }

    #[cfg(test)]
    pub(crate) fn glyph_hor_side_bearing(&self, glyph: GlyphId) -> Option<i16> {
        let tables = self.tables();
        let metrics = tables.hhea().ok()?.number_of_h_metrics();
        let hmtx = tables.hmtx().ok()?;
        let long = hmtx.h_metrics();
        if metrics == 0 || long.len() < usize::from(metrics) {
            return None;
        }
        match long.get(usize::from(glyph.0)) {
            Some(metric) => Some(metric.side_bearing()),
            None => hmtx
                .left_side_bearings()
                .get(usize::from(glyph.0) - long.len())
                .map(|bearing| bearing.get()),
        }
    }

    /// Outlines `glyph` in font units into `builder` and returns its bounds.
    /// `None` when the face has no `glyf` outline for it.
    pub(crate) fn outline_glyph(
        &self,
        glyph: GlyphId,
        builder: &mut dyn OutlineBuilder,
    ) -> Option<Rect> {
        let tables = self.tables();
        let glyf = tables.glyf().ok()?;
        let loca = tables.loca(None).ok()?;
        glyf::outline(&loca, &glyf, glyph.0, builder)
    }
}

/// Shapes `text` in font units (no scale) with `features`, in `direction`
/// when given. The script, and the direction when none is given, are guessed
/// from the text.
pub(crate) fn shape(
    face: &Face,
    features: &[Feature],
    text: &str,
    direction: Option<Direction>,
) -> Buffer {
    let mut buffer = Buffer::new();
    buffer.push_str(text);
    if let Some(direction) = direction {
        buffer.set_direction(direction);
    }
    buffer.guess_segment_properties();
    harfrust::shape(
        &harfrust::ShaperFont::new(&face.font),
        &mut buffer,
        harfrust::ShapeOptions::new().features(features),
    )
    // A fresh buffer with guessed properties has text and a direction.
    .expect("a guessed text buffer shapes");
    buffer
}

/// Whether a shaped right-to-left `buffer` holds each cluster's glyphs in
/// reverse logical order, as well as the clusters themselves.
///
/// A right-to-left buffer always comes back in visual order. HarfRust 0.15,
/// as HarfBuzz, shapes it backward and reverses it whole, so a cluster's
/// glyphs come back reversed too, only when its script is natively right to
/// left or has no single native direction. With no script (only Common and
/// Inherited scalars, as a run of punctuation and marks) or a natively
/// left-to-right one, it reverses the grapheme order, shapes left to right,
/// and keeps each cluster's glyphs in logical order. HarfRust keeps its
/// script table private (`Direction::from_script`); [`BACKWARD_SCRIPTS`]
/// mirrors it.
///
/// A left-to-right buffer is taken to keep logical order. It does unless it
/// holds letters of a natively right-to-left script, which only an explicit
/// bidi override sets left to right; that case is not modelled.
pub(crate) fn reverses_cluster_glyphs(buffer: &Buffer) -> bool {
    buffer.direction() == Direction::RightToLeft && buffer.script().is_some_and(backward_script)
}

/// Whether HarfRust 0.15 shapes `script` backward when asked for right to
/// left.
fn backward_script(script: harfrust::Script) -> bool {
    BACKWARD_SCRIPTS.contains(&script)
}

/// HarfRust 0.15's natively right-to-left scripts, then the four whose native
/// direction it leaves open (HarfBuzz issue 1000): those it shapes backward
/// when asked for right to left.
pub(crate) const BACKWARD_SCRIPTS: [harfrust::Script; 40] = {
    use harfrust::Script;
    [
        Script::ARABIC,
        Script::HEBREW,
        Script::SYRIAC,
        Script::THAANA,
        Script::CYPRIOT,
        Script::KHAROSHTHI,
        Script::PHOENICIAN,
        Script::NKO,
        Script::LYDIAN,
        Script::AVESTAN,
        Script::IMPERIAL_ARAMAIC,
        Script::INSCRIPTIONAL_PAHLAVI,
        Script::INSCRIPTIONAL_PARTHIAN,
        Script::OLD_SOUTH_ARABIAN,
        Script::OLD_TURKIC,
        Script::SAMARITAN,
        Script::MANDAIC,
        Script::MEROITIC_CURSIVE,
        Script::MEROITIC_HIEROGLYPHS,
        Script::MANICHAEAN,
        Script::MENDE_KIKAKUI,
        Script::NABATAEAN,
        Script::OLD_NORTH_ARABIAN,
        Script::PALMYRENE,
        Script::PSALTER_PAHLAVI,
        Script::HATRAN,
        Script::ADLAM,
        Script::HANIFI_ROHINGYA,
        Script::OLD_SOGDIAN,
        Script::SOGDIAN,
        Script::ELYMAIC,
        Script::CHORASMIAN,
        Script::YEZIDI,
        Script::OLD_UYGHUR,
        Script::GARAY,
        Script::SIDETIC,
        Script::OLD_HUNGARIAN,
        Script::OLD_ITALIC,
        Script::RUNIC,
        Script::TIFINAGH,
    ]
};

/// [`shape`] with no features and the buffer's script set to `script` rather
/// than guessed.
#[cfg(test)]
pub(crate) fn shape_in_script(
    face: &Face,
    text: &str,
    direction: Direction,
    script: Option<harfrust::Script>,
) -> Buffer {
    let mut buffer = Buffer::new();
    buffer.push_str(text);
    buffer.set_direction(direction);
    buffer.set_script(script);
    harfrust::shape(
        &harfrust::ShaperFont::new(&face.font),
        &mut buffer,
        harfrust::ShapeOptions::new(),
    )
    .expect("a buffer with a direction shapes");
    buffer
}
