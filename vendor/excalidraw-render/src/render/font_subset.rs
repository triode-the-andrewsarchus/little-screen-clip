use crate::fonts::{FontSubsetTableAction, FONT_SUBSET_TABLE_RULES_V1};
use crate::resource_accounting::{ResourceAccount, ResourceContext, ResourceDelta, ResourcePhase};
use skera::{subset_font, Plan, SubsetFlags};
use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::sync::Arc;
use write_fonts::from_obj::FromTableRef;
use write_fonts::read::{
    collections::IntSet,
    tables::{
        glyf::CurvePoint,
        name::{Name, NameRecord},
    },
    types::{GlyphId, NameId, Tag},
    FontData, FontRead, FontRef, Offset, TableProvider,
};
use write_fonts::tables::glyf::{Contour, GlyfLocaBuilder, Glyph, SimpleGlyph};
use write_fonts::types::FixedSize;
use write_fonts::FontBuilder;

const SUBSET_FLAGS: SubsetFlags = SubsetFlags::SUBSET_FLAGS_DEFAULT;
const ENCODER_THREADS: Option<NonZeroUsize> = NonZeroUsize::new(1);
const MAX_CACHE_ENTRIES: usize = 230;

#[derive(Debug)]
pub(super) struct Error {
    pub(super) stage: crate::SvgFontSubsetStage,
}

/// The container the source bytes of a subset arrive in. Every official font
/// chunk is a WOFF2 file whose reviewed decode is the sfnt the subsetter
/// plans over; the Noto Emoji fallback (#150) is shipped as the TrueType sfnt
/// itself, so its decode stage is the identity and its decoded length is its
/// manifested length.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SourceContainer {
    Woff2,
    Sfnt,
}

#[derive(Clone, Debug)]
pub(super) struct Derived {
    pub(super) woff2: Arc<[u8]>,
    pub(super) decoded_sfnt_bytes: usize,
    pub(super) subset_sfnt_bytes: usize,
    pub(super) subset_tables: usize,
}

pub(super) struct DerivationMetrics {
    asset_identity: &'static str,
    decoded_sfnt_bytes: usize,
    subset_sfnt_bytes: usize,
    subset_woff2_bytes: usize,
    subset_tables: usize,
}

impl DerivationMetrics {
    pub(super) fn new(asset_identity: &'static str, derived: &Derived) -> Self {
        Self {
            asset_identity,
            decoded_sfnt_bytes: derived.decoded_sfnt_bytes,
            subset_sfnt_bytes: derived.subset_sfnt_bytes,
            subset_woff2_bytes: derived.woff2.len(),
            subset_tables: derived.subset_tables,
        }
    }

    pub(super) const fn asset_identity(&self) -> &'static str {
        self.asset_identity
    }
}

/// The ceiling `F` on the sfnt, and its workspace, that the subset adapter
/// derives from a `decoded`-byte source (spec 13.2): the whole source twice over,
/// which covers a subset keeping every glyph with its widened `loca` and
/// rewritten names, plus 1 MiB of fixed headroom.
pub(super) fn sfnt_ceiling(decoded: u64) -> Option<u64> {
    decoded.checked_mul(2)?.checked_add(1_024 * 1_024)
}

pub(super) fn account_derivation<A: ResourceAccount>(
    account: &mut A,
    metrics: &DerivationMetrics,
) -> Result<(), A::Error> {
    let decoded = u64::try_from(metrics.decoded_sfnt_bytes).unwrap_or(u64::MAX);
    let subset = u64::try_from(metrics.subset_sfnt_bytes).unwrap_or(u64::MAX);
    let encoded = u64::try_from(metrics.subset_woff2_bytes).unwrap_or(u64::MAX);
    let tables = u64::try_from(metrics.subset_tables).unwrap_or(u64::MAX);
    let sfnt_bound = sfnt_ceiling(decoded)
        .ok_or_else(|| account.internal_failure("SVG font subset sfnt envelope overflowed"))?;
    let woff2_bound = decoded
        .checked_mul(2)
        .ok_or_else(|| account.internal_failure("SVG font subset WOFF2 envelope overflowed"))?;
    if subset > sfnt_bound || encoded > woff2_bound || tables > decoded / 16 {
        return Err(account.internal_failure("SVG font dependency exceeded its admitted envelope"));
    }
    for event in [
        "svg_font_woff2_decode",
        "svg_font_subset_plan",
        "svg_font_subset_sfnt",
        "svg_font_woff2_encode",
        "svg_font_validate",
    ] {
        account.transact_resource(
            ResourceContext::new(ResourcePhase::EncodingAndOutcomeAssembly, event, "")
                .with_semantic_identity(metrics.asset_identity),
            ResourceDelta::default(),
        )?;
    }
    Ok(())
}

#[derive(Eq, PartialEq)]
struct CacheKey {
    asset_identity: &'static str,
    effective_alias: String,
    semantic_identity: String,
}

#[derive(Default)]
pub(super) struct Session {
    derived: VecDeque<(CacheKey, Derived)>,
    #[cfg(test)]
    hits: usize,
    #[cfg(test)]
    misses: usize,
}

impl Session {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn derive(
        &mut self,
        asset_identity: &'static str,
        source: &[u8],
        container: SourceContainer,
        scalars: &[u32],
        effective_alias: &str,
        semantic_identity: &str,
    ) -> Result<Derived, Error> {
        let key = CacheKey {
            asset_identity,
            effective_alias: effective_alias.to_owned(),
            semantic_identity: semantic_identity.to_owned(),
        };
        if let Some((_, derived)) = self.derived.iter().find(|(candidate, _)| candidate == &key) {
            #[cfg(test)]
            {
                self.hits = self.hits.saturating_add(1);
            }
            return Ok(derived.clone());
        }
        #[cfg(test)]
        {
            self.misses = self.misses.saturating_add(1);
        }
        let derived = derive(
            source,
            container,
            scalars,
            effective_alias,
            semantic_identity,
        )?;
        self.retain(key, derived.clone());
        Ok(derived)
    }

    fn retain(&mut self, key: CacheKey, derived: Derived) {
        if self.derived.len() >= MAX_CACHE_ENTRIES {
            self.derived.pop_front();
        }
        self.derived.push_back((key, derived));
    }

    #[cfg(test)]
    pub(super) const fn stats(&self) -> (usize, usize) {
        (self.hits, self.misses)
    }
}

pub(super) fn derive(
    source: &[u8],
    container: SourceContainer,
    scalars: &[u32],
    effective_alias: &str,
    semantic_identity: &str,
) -> Result<Derived, Error> {
    let source_sfnt = decode_source(source, container)?;
    let font = FontRef::new(&source_sfnt).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Plan,
    })?;
    validate_source_profile(&font)?;

    let input_glyphs = IntSet::<GlyphId>::empty();
    let mut input_unicodes = scalars.iter().copied().collect::<IntSet<u32>>();
    // HarfBuzz hides default-ignorables using the nominal space glyph. Without
    // that cmap entry it deletes them instead, changing cluster cardinality.
    // Space is a shaping dependency even when absent from the semantic text.
    let face = crate::shaping::Face::from_slice(&source_sfnt, 0).ok_or(Error {
        stage: crate::SvgFontSubsetStage::Plan,
    })?;
    if scalars
        .iter()
        .filter_map(|scalar| char::from_u32(*scalar))
        .any(super::text::is_default_ignorable)
        && face.glyph_index(' ').is_some()
    {
        input_unicodes.insert(u32::from(' '));
    }
    let drop_tables = FONT_SUBSET_TABLE_RULES_V1
        .iter()
        .filter(|rule| rule.action == FontSubsetTableAction::Drop)
        .map(|rule| Tag::new(&rule.tag))
        .collect::<IntSet<Tag>>();
    let layout_scripts = IntSet::<Tag>::all();
    let layout_features = IntSet::<Tag>::all();
    let name_ids = (0_u16..=6)
        .chain([13, 14])
        .map(NameId::new)
        .collect::<IntSet<NameId>>();
    let name_languages = [0x0409_u16].into_iter().collect::<IntSet<u16>>();
    let plan = Plan::new(
        &input_glyphs,
        &input_unicodes,
        &font,
        SUBSET_FLAGS,
        &drop_tables,
        &layout_scripts,
        &layout_features,
        &name_ids,
        &name_languages,
    );
    let subset_sfnt = subset_font(&font, &plan).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    let subset_sfnt = ensure_required_cmap(&subset_sfnt)?;
    let subset_sfnt = rewrite_names(&subset_sfnt, effective_alias, semantic_identity)?;
    let subset_sfnt = normalize_simple_glyph_overlap(&subset_sfnt)?;
    let subset_sfnt = ensure_nonempty_glyf(&subset_sfnt)?;

    let subset_woff2: Arc<[u8]> = encode_woff2(&subset_sfnt)?.into();
    let decoded = wuff::decompress_woff2(&subset_woff2).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Validate,
    })?;
    let decoded_font = FontRef::new(&decoded).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Validate,
    })?;
    validate_checksums(&subset_sfnt)?;
    validate_checksums(&decoded)?;
    validate_canonical_agreement(&subset_sfnt, &decoded)?;
    validate_table_policy(&font, &decoded_font)?;
    validate_layout_closure(&decoded_font)?;
    validate_mappings_and_metrics(&source_sfnt, &decoded, scalars)?;
    validate_names(&source_sfnt, &decoded, effective_alias, semantic_identity)?;
    Ok(Derived {
        woff2: subset_woff2,
        decoded_sfnt_bytes: source_sfnt.len(),
        subset_sfnt_bytes: subset_sfnt.len(),
        subset_tables: decoded_font.table_directory().table_records().len(),
    })
}

/// The encode stage: the pinned `ttf2woff2` at quality 11 with the `glyf`/`loca`
/// transformation on one thread and no metadata or private block (spec
/// 8.3.3). The qualification corpus also encodes the Noto Emoji fallback's
/// complete sfnt with it, so the canonical viewers, which admit only WOFF2,
/// see the source face through the same container the subsets use.
pub(super) fn encode_woff2(sfnt: &[u8]) -> Result<Vec<u8>, Error> {
    ttf2woff2::encode_with_options(
        sfnt,
        ttf2woff2::EncodeOptions {
            quality: ttf2woff2::BrotliQuality::from(11),
            transform_glyf_loca: true,
            threads: ENCODER_THREADS,
        },
    )
    .map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Encode,
    })
}

/// The decode stage: the sfnt the subsetter plans over. A WOFF2 chunk is
/// decoded by the pinned `wuff`; an sfnt source must parse as one whose every
/// table lies inside the bytes, so a malformed or truncated fallback face
/// fails at the same stage a malformed chunk does, and is otherwise the
/// decode of itself.
fn decode_source(source: &[u8], container: SourceContainer) -> Result<Vec<u8>, Error> {
    let decode_error = || Error {
        stage: crate::SvgFontSubsetStage::Decode,
    };
    match container {
        SourceContainer::Woff2 => wuff::decompress_woff2(source).map_err(|_| decode_error()),
        SourceContainer::Sfnt => {
            let font = FontRef::new(source).map_err(|_| decode_error())?;
            for record in font.table_directory().table_records() {
                let end = usize::try_from(record.offset())
                    .ok()
                    .zip(usize::try_from(record.length()).ok())
                    .and_then(|(offset, length)| offset.checked_add(length))
                    .ok_or_else(decode_error)?;
                if end > source.len() {
                    return Err(decode_error());
                }
            }
            let mut sfnt = Vec::new();
            sfnt.try_reserve_exact(source.len())
                .map_err(|_| decode_error())?;
            sfnt.extend_from_slice(source);
            Ok(sfnt)
        }
    }
}

fn ensure_required_cmap(sfnt: &[u8]) -> Result<Vec<u8>, Error> {
    let font = FontRef::new(sfnt).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    if font.data_for_tag(Tag::new(b"cmap")).is_some() {
        return Ok(sfnt.to_vec());
    }
    let mut builder = FontBuilder::new();
    builder.add_raw(Tag::new(b"cmap"), vec![0, 0, 0, 0]);
    builder.copy_missing_tables(font);
    Ok(builder.build())
}

fn ensure_nonempty_glyf(sfnt: &[u8]) -> Result<Vec<u8>, Error> {
    let font = FontRef::new(sfnt).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    let glyf = font.data_for_tag(Tag::new(b"glyf")).ok_or(Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    if !glyf.as_bytes().is_empty() {
        return Ok(sfnt.to_vec());
    }
    // Chromium OTS rejects a zero-length glyf table even when every retained
    // glyph is empty (for example, a subset containing only U+0020). Encode an
    // outline-free simple .notdef glyph so WOFF2 glyf transformation retains a
    // structurally nonempty table without changing any selected visible glyph.
    let notdef = Glyph::Simple(SimpleGlyph {
        contours: vec![Contour::from(vec![CurvePoint::on_curve(0, 0)])],
        ..SimpleGlyph::default()
    });
    let glyph_count = font
        .maxp()
        .map_err(|_| Error {
            stage: crate::SvgFontSubsetStage::Sfnt,
        })?
        .num_glyphs();
    let mut glyf_loca = GlyfLocaBuilder::new();
    for glyph_id in 0..glyph_count {
        let glyph = if glyph_id == 0 {
            notdef.clone()
        } else {
            Glyph::Empty
        };
        glyf_loca.add_glyph(&glyph).map_err(|_| Error {
            stage: crate::SvgFontSubsetStage::Sfnt,
        })?;
    }
    let (glyf, loca, _) = glyf_loca.build();
    let mut builder = FontBuilder::new();
    builder.add_table(&glyf).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    builder.add_table(&loca).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    builder.copy_missing_tables(font);
    Ok(builder.build())
}

/// The program PDF embeds for a font (ADR-0021): the complete source program
/// with its names rewritten exactly as a derived subset's are, so the
/// Modified Version Krilla subsets while embedding carries the effective
/// alias and keeps the source's legal records (ADR-0001). Glyph IDs are
/// untouched, so the renderer's shaped glyphs address it directly.
#[cfg(feature = "pdf")]
pub(super) fn rename_program(sfnt: &[u8], alias: &str, identity: &str) -> Result<Vec<u8>, Error> {
    let renamed = rewrite_names(sfnt, alias, identity)?;
    validate_names(sfnt, &renamed, alias, identity)?;
    Ok(renamed)
}

fn rewrite_names(sfnt: &[u8], alias: &str, identity: &str) -> Result<Vec<u8>, Error> {
    let font = FontRef::new(sfnt).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    let source_name = font.data_for_tag(Tag::new(b"name")).ok_or(Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    let prefix = identity.get(..32).ok_or(Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    let postscript_alias = alias.replace(' ', "-");
    let rewritten = [
        (1_u16, alias.to_owned()),
        (2, "Regular".to_owned()),
        (3, format!("excalidraw-render;{alias};{identity}")),
        (4, format!("{alias} Regular {prefix}")),
        (6, format!("{postscript_alias}-Regular-{prefix}")),
    ];
    let mut records = Vec::<(u16, Vec<u8>)>::new();
    for name_id in [0_u16, 1, 2, 3, 4, 5, 6, 13, 14] {
        let bytes = if let Some((_, value)) = rewritten.iter().find(|(id, _)| *id == name_id) {
            utf16be(value)
        } else if let Some(bytes) = english_name(source_name.as_bytes(), name_id) {
            bytes.to_vec()
        } else {
            continue;
        };
        records.push((name_id, bytes));
    }
    // write-fonts 0.53's NameRecord stores strings, not raw bytes. Its reader
    // conversion replaces malformed UTF-16 and drops odd trailing bytes; using
    // it here would change preserved legal records. Keep bounded raw writing
    // through FontBuilder (evaluation: docs/research/fontations-name-tables.md).
    let count = u16::try_from(records.len()).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    let string_offset = 6_usize
        .checked_add(records.len().checked_mul(12).ok_or(Error {
            stage: crate::SvgFontSubsetStage::Sfnt,
        })?)
        .ok_or(Error {
            stage: crate::SvgFontSubsetStage::Sfnt,
        })?;
    let mut table = Vec::new();
    let strings_len = records
        .iter()
        .try_fold(0_usize, |total, (_, bytes)| total.checked_add(bytes.len()))
        .ok_or(Error {
            stage: crate::SvgFontSubsetStage::Sfnt,
        })?;
    table
        .try_reserve_exact(string_offset.checked_add(strings_len).ok_or(Error {
            stage: crate::SvgFontSubsetStage::Sfnt,
        })?)
        .map_err(|_| Error {
            stage: crate::SvgFontSubsetStage::Sfnt,
        })?;
    table.extend_from_slice(&0_u16.to_be_bytes());
    table.extend_from_slice(&count.to_be_bytes());
    table.extend_from_slice(
        &u16::try_from(string_offset)
            .map_err(|_| Error {
                stage: crate::SvgFontSubsetStage::Sfnt,
            })?
            .to_be_bytes(),
    );
    let mut offset = 0_usize;
    for (name_id, bytes) in &records {
        table.extend_from_slice(&3_u16.to_be_bytes());
        table.extend_from_slice(&1_u16.to_be_bytes());
        table.extend_from_slice(&0x0409_u16.to_be_bytes());
        table.extend_from_slice(&name_id.to_be_bytes());
        table.extend_from_slice(
            &u16::try_from(bytes.len())
                .map_err(|_| Error {
                    stage: crate::SvgFontSubsetStage::Sfnt,
                })?
                .to_be_bytes(),
        );
        table.extend_from_slice(
            &u16::try_from(offset)
                .map_err(|_| Error {
                    stage: crate::SvgFontSubsetStage::Sfnt,
                })?
                .to_be_bytes(),
        );
        offset = offset.checked_add(bytes.len()).ok_or(Error {
            stage: crate::SvgFontSubsetStage::Sfnt,
        })?;
    }
    for (_, bytes) in records {
        table.extend_from_slice(&bytes);
    }
    let mut builder = FontBuilder::new();
    builder.add_raw(Tag::new(b"name"), table);
    builder.copy_missing_tables(font);
    let rewritten = builder.build();
    FontRef::new(&rewritten).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    Ok(rewritten)
}

fn english_name(table: &[u8], name_id: u16) -> Option<&[u8]> {
    let data = FontData::new(table);
    let name = Name::read(data).ok()?;
    // Read one typed record at a time: Name::name_record() returns an empty
    // slice when *any* declared record is truncated. Our first-match policy
    // still accepts an earlier complete record in that case. The range is
    // bounded by the u16 count and Fontations' fixed-size record layout.
    for offset in name
        .name_record_byte_range()
        .step_by(NameRecord::RAW_BYTE_LEN)
    {
        let record = data.read_ref_at::<NameRecord>(offset).ok()?;
        if record.platform_id() != 3
            || record.language_id() != 0x0409
            || record.name_id() != NameId::new(name_id)
        {
            continue;
        }
        // Encoding ID deliberately does not participate in selection. Preserve
        // the bytes, including malformed UTF-16, rather than NameString::chars.
        // Slice against the original table: string_data() defaults an invalid
        // storage offset to empty data, conflating it with a valid empty name.
        let start = usize::from(name.storage_offset())
            .checked_add(record.string_offset().non_null().unwrap_or(0))?;
        return table.get(start..start.checked_add(usize::from(record.length()))?);
    }
    None
}

fn utf16be(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect::<Vec<_>>()
}

fn validate_names(source: &[u8], sfnt: &[u8], alias: &str, identity: &str) -> Result<(), Error> {
    let source = FontRef::new(source).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Validate,
    })?;
    let source_table = source.data_for_tag(Tag::new(b"name")).ok_or(Error {
        stage: crate::SvgFontSubsetStage::Validate,
    })?;
    let font = FontRef::new(sfnt).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Validate,
    })?;
    let table = font.data_for_tag(Tag::new(b"name")).ok_or(Error {
        stage: crate::SvgFontSubsetStage::Validate,
    })?;
    for (name_id, expected) in [
        (1_u16, alias.to_owned()),
        (2, "Regular".to_owned()),
        (3, format!("excalidraw-render;{alias};{identity}")),
        (4, format!("{alias} Regular {}", &identity[..32])),
        (
            6,
            format!("{}-Regular-{}", alias.replace(' ', "-"), &identity[..32]),
        ),
    ] {
        if english_name(table.as_bytes(), name_id) != Some(utf16be(&expected).as_slice()) {
            return Err(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            });
        }
    }
    for name_id in [0_u16, 5, 13, 14] {
        if let Some(expected) = english_name(source_table.as_bytes(), name_id) {
            if english_name(table.as_bytes(), name_id) != Some(expected) {
                return Err(Error {
                    stage: crate::SvgFontSubsetStage::Validate,
                });
            }
        }
    }
    Ok(())
}

fn validate_mappings_and_metrics(
    source: &[u8],
    subset: &[u8],
    scalars: &[u32],
) -> Result<(), Error> {
    let source = crate::shaping::Face::from_slice(source, 0).ok_or(Error {
        stage: crate::SvgFontSubsetStage::Validate,
    })?;
    let subset = crate::shaping::Face::from_slice(subset, 0).ok_or(Error {
        stage: crate::SvgFontSubsetStage::Validate,
    })?;
    if source.units_per_em() != subset.units_per_em()
        || source.ascender() != subset.ascender()
        || source.descender() != subset.descender()
        || source.line_gap() != subset.line_gap()
    {
        return Err(Error {
            stage: crate::SvgFontSubsetStage::Validate,
        });
    }
    let mut retained_text = String::new();
    retained_text
        .try_reserve_exact(
            scalars
                .len()
                .checked_mul(char::MAX.len_utf8())
                .ok_or_else(validate_error)?,
        )
        .map_err(|_| validate_error())?;
    for scalar in scalars {
        let Some(character) = char::from_u32(*scalar) else {
            return Err(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            });
        };
        if let Some(source_glyph) = source.glyph_index(character) {
            let subset_glyph = subset.glyph_index(character).ok_or(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            })?;
            if source.glyph_hor_advance(source_glyph) != subset.glyph_hor_advance(subset_glyph) {
                return Err(Error {
                    stage: crate::SvgFontSubsetStage::Validate,
                });
            }
            let mut source_outline = Outline::default();
            let mut subset_outline = Outline::default();
            let source_bounds = source.outline_glyph(source_glyph, &mut source_outline);
            let subset_bounds = subset.outline_glyph(subset_glyph, &mut subset_outline);
            if source_bounds != subset_bounds || source_outline != subset_outline {
                return Err(Error {
                    stage: crate::SvgFontSubsetStage::Validate,
                });
            }
            retained_text.push(character);
        }
    }
    for character in retained_text.chars() {
        let mut repeated = String::new();
        repeated
            .try_reserve(character.len_utf8() * 3)
            .map_err(|_| Error {
                stage: crate::SvgFontSubsetStage::Validate,
            })?;
        repeated.extend([character, character, character]);
        validate_shaping_probe(
            &source,
            &subset,
            &repeated,
            Some(crate::shaping::Direction::LeftToRight),
        )?;
        validate_shaping_probe(
            &source,
            &subset,
            &repeated,
            Some(crate::shaping::Direction::RightToLeft),
        )?;
    }
    for probe in ["fi", "fl", "ffi", "office", "لا"] {
        if probe
            .chars()
            .all(|character| retained_text.contains(character))
        {
            validate_shaping_probe(&source, &subset, probe, None)?;
        }
    }
    let mut retained = Vec::new();
    retained
        .try_reserve_exact(retained_text.chars().count())
        .map_err(|_| validate_error())?;
    retained.extend(retained_text.chars());
    for chunk in retained.chunks(32).filter(|chunk| chunk.len() > 1) {
        let probe = fallible_probe(chunk)?;
        validate_shaping_probe(&source, &subset, &probe, None)?;
    }
    for width in 2..=4 {
        for window in retained.windows(width) {
            let probe = fallible_probe(window)?;
            validate_shaping_probe(&source, &subset, &probe, None)?;
        }
    }
    Ok(())
}

fn validate_layout_closure(font: &FontRef<'_>) -> Result<(), Error> {
    super::font_subset_layout::validate(font)
        .map(drop)
        .map_err(|_| validate_error())
}

fn fallible_probe(characters: &[char]) -> Result<String, Error> {
    let bytes = characters
        .iter()
        .try_fold(0_usize, |total, character| {
            total.checked_add(character.len_utf8())
        })
        .ok_or_else(validate_error)?;
    let mut probe = String::new();
    probe
        .try_reserve_exact(bytes)
        .map_err(|_| validate_error())?;
    probe.extend(characters);
    Ok(probe)
}

/// The glyphs the qualification corpus treats as layout-referenced
/// (ADR-0020): those in a Coverage table of a GSUB or GPOS lookup subtable,
/// whole ranges included. GDEF and class definitions do not count. `None`
/// when a layout table does not parse, covers a glyph past `numGlyphs`, or
/// uses a 24-bit glyph ID subtable format the walker does not model either.
#[cfg(test)]
pub(super) fn layout_glyph_ids(font: &FontRef<'_>) -> Option<Vec<bool>> {
    use write_fonts::read::tables::{gpos::PositionLookup, gsub::SubstitutionLookup};
    let mut glyphs = vec![false; usize::from(font.maxp().ok()?.num_glyphs())];
    let mut coverages = Vec::new();
    if font.data_for_tag(Tag::new(b"GSUB")).is_some() {
        for lookup in font.gsub().ok()?.lookup_list().ok()?.lookups().iter() {
            match lookup.ok()? {
                SubstitutionLookup::Extension(lookup) => {
                    for extension in lookup.subtables().iter() {
                        gsub_extension_coverages(&extension.ok()?, &mut coverages)?;
                    }
                }
                lookup => gsub_lookup_coverages(&lookup, &mut coverages)?,
            }
        }
    }
    if font.data_for_tag(Tag::new(b"GPOS")).is_some() {
        for lookup in font.gpos().ok()?.lookup_list().ok()?.lookups().iter() {
            match lookup.ok()? {
                PositionLookup::Extension(lookup) => {
                    for extension in lookup.subtables().iter() {
                        gpos_extension_coverages(&extension.ok()?, &mut coverages)?;
                    }
                }
                lookup => gpos_lookup_coverages(&lookup, &mut coverages)?,
            }
        }
    }
    for coverage in coverages {
        for glyph in coverage.iter() {
            *glyphs.get_mut(usize::try_from(glyph.to_u32()).ok()?)? = true;
        }
    }
    Some(glyphs)
}

#[cfg(test)]
type Coverages<'a> = Vec<write_fonts::read::tables::layout::CoverageTable<'a>>;

#[cfg(test)]
fn gsub_lookup_coverages<'a>(
    lookup: &write_fonts::read::tables::gsub::SubstitutionLookup<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gsub::SubstitutionLookup;
    fn each<'a, T>(
        subtables: impl Iterator<Item = Result<T, write_fonts::read::ReadError>>,
        coverages: &mut Coverages<'a>,
        push: impl Fn(&T, &mut Coverages<'a>) -> Option<()>,
    ) -> Option<()> {
        subtables
            .map(|subtable| push(&subtable.ok()?, coverages))
            .collect()
    }
    match lookup {
        SubstitutionLookup::Single(lookup) => {
            each(lookup.subtables().iter(), coverages, single_subst_coverages)
        }
        SubstitutionLookup::Multiple(lookup) => each(
            lookup.subtables().iter(),
            coverages,
            multiple_subst_coverages,
        ),
        SubstitutionLookup::Alternate(lookup) => each(
            lookup.subtables().iter(),
            coverages,
            alternate_subst_coverages,
        ),
        SubstitutionLookup::Ligature(lookup) => each(
            lookup.subtables().iter(),
            coverages,
            ligature_subst_coverages,
        ),
        SubstitutionLookup::Contextual(lookup) => each(
            lookup.subtables().iter(),
            coverages,
            sequence_context_coverages,
        ),
        SubstitutionLookup::ChainContextual(lookup) => each(
            lookup.subtables().iter(),
            coverages,
            chained_context_coverages,
        ),
        SubstitutionLookup::Reverse(lookup) => each(
            lookup.subtables().iter(),
            coverages,
            reverse_chain_subst_coverages,
        ),
        SubstitutionLookup::Extension(_) => None,
    }
}

#[cfg(test)]
fn gsub_extension_coverages<'a>(
    extension: &write_fonts::read::tables::gsub::ExtensionSubtable<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gsub::ExtensionSubtable;
    match extension {
        ExtensionSubtable::Single(table) => {
            single_subst_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::Multiple(table) => {
            multiple_subst_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::Alternate(table) => {
            alternate_subst_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::Ligature(table) => {
            ligature_subst_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::Contextual(table) => {
            sequence_context_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::ChainContextual(table) => {
            chained_context_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::Reverse(table) => {
            reverse_chain_subst_coverages(&table.extension().ok()?, coverages)
        }
    }
}

#[cfg(test)]
fn single_subst_coverages<'a>(
    subtable: &write_fonts::read::tables::gsub::SingleSubst<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gsub::SingleSubst;
    coverages.push(match subtable {
        SingleSubst::Format1(table) => table.coverage().ok()?,
        SingleSubst::Format2(table) => table.coverage().ok()?,
        SingleSubst::Format3(_) | SingleSubst::Format4(_) => return None,
    });
    Some(())
}

#[cfg(test)]
fn multiple_subst_coverages<'a>(
    subtable: &write_fonts::read::tables::gsub::MultipleSubst<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gsub::MultipleSubst;
    let MultipleSubst::Format1(table) = subtable else {
        return None;
    };
    coverages.push(table.coverage().ok()?);
    Some(())
}

#[cfg(test)]
fn alternate_subst_coverages<'a>(
    subtable: &write_fonts::read::tables::gsub::AlternateSubst<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gsub::AlternateSubst;
    let AlternateSubst::Format1(table) = subtable else {
        return None;
    };
    coverages.push(table.coverage().ok()?);
    Some(())
}

#[cfg(test)]
fn ligature_subst_coverages<'a>(
    subtable: &write_fonts::read::tables::gsub::LigatureSubst<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gsub::LigatureSubst;
    let LigatureSubst::Format1(table) = subtable else {
        return None;
    };
    coverages.push(table.coverage().ok()?);
    Some(())
}

#[cfg(test)]
fn reverse_chain_subst_coverages<'a>(
    subtable: &write_fonts::read::tables::gsub::ReverseChainSingleSubst<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gsub::ReverseChainSingleSubst;
    let ReverseChainSingleSubst::Format1(table) = subtable else {
        return None;
    };
    coverages.push(table.coverage().ok()?);
    for coverage in table
        .backtrack_coverages()
        .iter()
        .chain(table.lookahead_coverages().iter())
    {
        coverages.push(coverage.ok()?);
    }
    Some(())
}

#[cfg(test)]
fn gpos_lookup_coverages<'a>(
    lookup: &write_fonts::read::tables::gpos::PositionLookup<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gpos::PositionLookup;
    fn each<'a, T>(
        subtables: impl Iterator<Item = Result<T, write_fonts::read::ReadError>>,
        coverages: &mut Coverages<'a>,
        push: impl Fn(&T, &mut Coverages<'a>) -> Option<()>,
    ) -> Option<()> {
        subtables
            .map(|subtable| push(&subtable.ok()?, coverages))
            .collect()
    }
    match lookup {
        PositionLookup::Single(lookup) => {
            each(lookup.subtables().iter(), coverages, single_pos_coverages)
        }
        PositionLookup::Pair(lookup) => {
            each(lookup.subtables().iter(), coverages, pair_pos_coverages)
        }
        PositionLookup::Cursive(lookup) => {
            each(lookup.subtables().iter(), coverages, cursive_pos_coverages)
        }
        PositionLookup::MarkToBase(lookup) => each(
            lookup.subtables().iter(),
            coverages,
            mark_base_pos_coverages,
        ),
        PositionLookup::MarkToLig(lookup) => {
            each(lookup.subtables().iter(), coverages, mark_lig_pos_coverages)
        }
        PositionLookup::MarkToMark(lookup) => each(
            lookup.subtables().iter(),
            coverages,
            mark_mark_pos_coverages,
        ),
        PositionLookup::Contextual(lookup) => each(
            lookup.subtables().iter(),
            coverages,
            sequence_context_coverages,
        ),
        PositionLookup::ChainContextual(lookup) => each(
            lookup.subtables().iter(),
            coverages,
            chained_context_coverages,
        ),
        PositionLookup::Extension(_) => None,
    }
}

#[cfg(test)]
fn gpos_extension_coverages<'a>(
    extension: &write_fonts::read::tables::gpos::ExtensionSubtable<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gpos::ExtensionSubtable;
    match extension {
        ExtensionSubtable::Single(table) => {
            single_pos_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::Pair(table) => pair_pos_coverages(&table.extension().ok()?, coverages),
        ExtensionSubtable::Cursive(table) => {
            cursive_pos_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::MarkToBase(table) => {
            mark_base_pos_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::MarkToLig(table) => {
            mark_lig_pos_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::MarkToMark(table) => {
            mark_mark_pos_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::Contextual(table) => {
            sequence_context_coverages(&table.extension().ok()?, coverages)
        }
        ExtensionSubtable::ChainContextual(table) => {
            chained_context_coverages(&table.extension().ok()?, coverages)
        }
    }
}

#[cfg(test)]
fn single_pos_coverages<'a>(
    subtable: &write_fonts::read::tables::gpos::SinglePos<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gpos::SinglePos;
    coverages.push(match subtable {
        SinglePos::Format1(table) => table.coverage().ok()?,
        SinglePos::Format2(table) => table.coverage().ok()?,
        SinglePos::Format3(_) | SinglePos::Format4(_) => return None,
    });
    Some(())
}

#[cfg(test)]
fn pair_pos_coverages<'a>(
    subtable: &write_fonts::read::tables::gpos::PairPos<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gpos::PairPos;
    coverages.push(match subtable {
        PairPos::Format1(table) => table.coverage().ok()?,
        PairPos::Format2(table) => table.coverage().ok()?,
        PairPos::Format3(_) | PairPos::Format4(_) => return None,
    });
    Some(())
}

#[cfg(test)]
fn cursive_pos_coverages<'a>(
    subtable: &write_fonts::read::tables::gpos::CursivePos<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gpos::CursivePos;
    let CursivePos::Format1(table) = subtable else {
        return None;
    };
    coverages.push(table.coverage().ok()?);
    Some(())
}

#[cfg(test)]
fn mark_base_pos_coverages<'a>(
    subtable: &write_fonts::read::tables::gpos::MarkBasePos<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gpos::MarkBasePos;
    let MarkBasePos::Format1(table) = subtable else {
        return None;
    };
    coverages.push(table.mark_coverage().ok()?);
    coverages.push(table.base_coverage().ok()?);
    Some(())
}

#[cfg(test)]
fn mark_lig_pos_coverages<'a>(
    subtable: &write_fonts::read::tables::gpos::MarkLigPos<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gpos::MarkLigPos;
    let MarkLigPos::Format1(table) = subtable else {
        return None;
    };
    coverages.push(table.mark_coverage().ok()?);
    coverages.push(table.ligature_coverage().ok()?);
    Some(())
}

#[cfg(test)]
fn mark_mark_pos_coverages<'a>(
    subtable: &write_fonts::read::tables::gpos::MarkMarkPos<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::gpos::MarkMarkPos;
    let MarkMarkPos::Format1(table) = subtable else {
        return None;
    };
    coverages.push(table.mark1_coverage().ok()?);
    coverages.push(table.mark2_coverage().ok()?);
    Some(())
}

#[cfg(test)]
fn sequence_context_coverages<'a>(
    context: &write_fonts::read::tables::layout::SequenceContext<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::layout::SequenceContext;
    match context {
        SequenceContext::Format1(table) => coverages.push(table.coverage().ok()?),
        SequenceContext::Format2(table) => coverages.push(table.coverage().ok()?),
        SequenceContext::Format3(table) => {
            for coverage in table.coverages().iter() {
                coverages.push(coverage.ok()?);
            }
        }
        SequenceContext::Format4(_) | SequenceContext::Format5(_) | SequenceContext::Format6(_) => {
            return None
        }
    }
    Some(())
}

#[cfg(test)]
fn chained_context_coverages<'a>(
    context: &write_fonts::read::tables::layout::ChainedSequenceContext<'a>,
    coverages: &mut Coverages<'a>,
) -> Option<()> {
    use write_fonts::read::tables::layout::ChainedSequenceContext;
    match context {
        ChainedSequenceContext::Format1(table) => coverages.push(table.coverage().ok()?),
        ChainedSequenceContext::Format2(table) => coverages.push(table.coverage().ok()?),
        ChainedSequenceContext::Format3(table) => {
            for coverage in table
                .backtrack_coverages()
                .iter()
                .chain(table.input_coverages().iter())
                .chain(table.lookahead_coverages().iter())
            {
                coverages.push(coverage.ok()?);
            }
        }
        ChainedSequenceContext::Format4(_) | ChainedSequenceContext::Format5(_) => return None,
    }
    Some(())
}

const fn validate_error() -> Error {
    Error {
        stage: crate::SvgFontSubsetStage::Validate,
    }
}

fn validate_shaping_probe(
    source: &crate::shaping::Face,
    subset: &crate::shaping::Face,
    text: &str,
    direction: Option<crate::shaping::Direction>,
) -> Result<(), Error> {
    let shape = |face: &crate::shaping::Face| crate::shaping::shape(face, &[], text, direction);
    let source_shape = shape(source);
    let subset_shape = shape(subset);
    if source_shape.glyph_infos().len() != subset_shape.glyph_infos().len()
        || source_shape
            .glyph_infos()
            .iter()
            .zip(subset_shape.glyph_infos())
            .any(|(source, subset)| source.cluster != subset.cluster)
        || source_shape
            .glyph_positions()
            .iter()
            .zip(subset_shape.glyph_positions())
            .any(|(source, subset)| {
                source.x_advance != subset.x_advance
                    || source.y_advance != subset.y_advance
                    || source.x_offset != subset.x_offset
                    || source.y_offset != subset.y_offset
            })
    {
        return Err(Error {
            stage: crate::SvgFontSubsetStage::Validate,
        });
    }
    for (source_info, subset_info) in source_shape
        .glyph_infos()
        .iter()
        .zip(subset_shape.glyph_infos())
    {
        let source_id = u16::try_from(source_info.glyph_id).map_err(|_| Error {
            stage: crate::SvgFontSubsetStage::Validate,
        })?;
        let subset_id = u16::try_from(subset_info.glyph_id).map_err(|_| Error {
            stage: crate::SvgFontSubsetStage::Validate,
        })?;
        let mut source_outline = Outline::default();
        let mut subset_outline = Outline::default();
        let source_bounds =
            source.outline_glyph(crate::shaping::GlyphId(source_id), &mut source_outline);
        let subset_bounds =
            subset.outline_glyph(crate::shaping::GlyphId(subset_id), &mut subset_outline);
        if source_bounds != subset_bounds || source_outline != subset_outline {
            return Err(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            });
        }
    }
    Ok(())
}

fn validate_source_profile(font: &FontRef<'_>) -> Result<(), Error> {
    for record in font.table_directory().table_records() {
        if table_rule(record.tag()).is_none() {
            return Err(Error {
                stage: crate::SvgFontSubsetStage::Plan,
            });
        }
    }
    for rule in FONT_SUBSET_TABLE_RULES_V1 {
        if rule.required && font.data_for_tag(Tag::new(&rule.tag)).is_none() {
            return Err(Error {
                stage: crate::SvgFontSubsetStage::Plan,
            });
        }
    }
    let glyph_count = usize::from(
        font.maxp()
            .map_err(|_| Error {
                stage: crate::SvgFontSubsetStage::Plan,
            })?
            .num_glyphs(),
    );
    let loca = font.loca(None).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Plan,
    })?;
    let glyf = font.glyf().map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Plan,
    })?;
    if loca.len() != glyph_count || !loca.all_offsets_are_ascending() {
        return Err(Error {
            stage: crate::SvgFontSubsetStage::Plan,
        });
    }
    for glyph_id in 0..glyph_count {
        let glyph_id = u32::try_from(glyph_id).map_err(|_| Error {
            stage: crate::SvgFontSubsetStage::Plan,
        })?;
        let glyph = loca
            .get_glyf(GlyphId::new(glyph_id), &glyf)
            .map_err(|_| Error {
                stage: crate::SvgFontSubsetStage::Plan,
            })?;
        if let Some(write_fonts::read::tables::glyf::Glyph::Composite(composite)) = glyph {
            for component in composite.components() {
                if usize::try_from(component.glyph.to_u32()).map_or(true, |id| id >= glyph_count)
                    || loca
                        .get_glyf(GlyphId::new(component.glyph.to_u32()), &glyf)
                        .map_err(|_| Error {
                            stage: crate::SvgFontSubsetStage::Plan,
                        })?
                        .is_none()
                {
                    return Err(Error {
                        stage: crate::SvgFontSubsetStage::Plan,
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_table_policy(source: &FontRef<'_>, subset: &FontRef<'_>) -> Result<(), Error> {
    for record in subset.table_directory().table_records() {
        if table_rule(record.tag()).is_none() {
            return Err(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            });
        }
    }
    for rule in FONT_SUBSET_TABLE_RULES_V1 {
        let tag = Tag::new(&rule.tag);
        let source_data = source.data_for_tag(tag);
        let subset_data = subset.data_for_tag(tag);
        match rule.action {
            FontSubsetTableAction::Drop if subset_data.is_some() => {
                return Err(Error {
                    stage: crate::SvgFontSubsetStage::Validate,
                });
            }
            FontSubsetTableAction::PassThrough
                if subset_data.map(|data| data.as_bytes())
                    != source_data.map(|data| data.as_bytes()) =>
            {
                return Err(Error {
                    stage: crate::SvgFontSubsetStage::Validate,
                });
            }
            _ => {}
        }
        if rule.required && subset_data.is_none() {
            return Err(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            });
        }
    }
    Ok(())
}

fn table_rule(tag: Tag) -> Option<&'static crate::fonts::FontSubsetTableRule> {
    FONT_SUBSET_TABLE_RULES_V1
        .iter()
        .find(|rule| Tag::new(&rule.tag) == tag)
}

fn canonicalize_sfnt(sfnt: &[u8]) -> Result<Vec<u8>, Error> {
    let font = FontRef::new(sfnt).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Validate,
    })?;
    let mut builder = FontBuilder::new();
    builder.copy_missing_tables(font);
    Ok(builder.build())
}

fn validate_canonical_agreement(generated: &[u8], decoded: &[u8]) -> Result<(), Error> {
    let generated = canonicalize_sfnt(generated)?;
    let decoded = canonicalize_sfnt(decoded)?;
    let generated = FontRef::new(&generated).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Validate,
    })?;
    let decoded = FontRef::new(&decoded).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Validate,
    })?;
    let generated_tags = generated
        .table_directory()
        .table_records()
        .iter()
        .map(|record| record.tag())
        .collect::<Vec<_>>();
    let decoded_tags = decoded
        .table_directory()
        .table_records()
        .iter()
        .map(|record| record.tag())
        .collect::<Vec<_>>();
    if generated_tags != decoded_tags {
        return Err(Error {
            stage: crate::SvgFontSubsetStage::Validate,
        });
    }
    if canonical_glyf_loca(&generated)? != canonical_glyf_loca(&decoded)? {
        return Err(Error {
            stage: crate::SvgFontSubsetStage::Validate,
        });
    }
    for tag in generated_tags {
        if tag == Tag::new(b"glyf") || tag == Tag::new(b"loca") {
            continue;
        }
        let generated_data = generated.data_for_tag(tag).ok_or(Error {
            stage: crate::SvgFontSubsetStage::Validate,
        })?;
        let decoded_data = decoded.data_for_tag(tag).ok_or(Error {
            stage: crate::SvgFontSubsetStage::Validate,
        })?;
        if tag == Tag::new(b"head") {
            let mut generated_head = generated_data.as_bytes().to_vec();
            let mut decoded_head = decoded_data.as_bytes().to_vec();
            generated_head
                .get_mut(8..12)
                .ok_or(Error {
                    stage: crate::SvgFontSubsetStage::Validate,
                })?
                .fill(0);
            decoded_head
                .get_mut(8..12)
                .ok_or(Error {
                    stage: crate::SvgFontSubsetStage::Validate,
                })?
                .fill(0);
            if generated_head != decoded_head {
                return Err(Error {
                    stage: crate::SvgFontSubsetStage::Validate,
                });
            }
        } else if generated_data.as_bytes() != decoded_data.as_bytes() {
            return Err(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            });
        }
    }
    Ok(())
}

fn canonical_glyf_loca(font: &FontRef<'_>) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let (glyf, loca) = rebuild_glyf_loca(font, false, crate::SvgFontSubsetStage::Validate)?;
    Ok((
        write_fonts::dump_table(&glyf).map_err(|_| Error {
            stage: crate::SvgFontSubsetStage::Validate,
        })?,
        write_fonts::dump_table(&loca).map_err(|_| Error {
            stage: crate::SvgFontSubsetStage::Validate,
        })?,
    ))
}

fn normalize_simple_glyph_overlap(sfnt: &[u8]) -> Result<Vec<u8>, Error> {
    let font = FontRef::new(sfnt).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    let (glyf, loca) = rebuild_glyf_loca(&font, true, crate::SvgFontSubsetStage::Sfnt)?;
    if i16::from(loca.format() as u8)
        != font
            .head()
            .map_err(|_| Error {
                stage: crate::SvgFontSubsetStage::Sfnt,
            })?
            .index_to_loc_format()
    {
        return Err(Error {
            stage: crate::SvgFontSubsetStage::Sfnt,
        });
    }
    let mut builder = FontBuilder::new();
    builder.add_table(&glyf).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    builder.add_table(&loca).map_err(|_| Error {
        stage: crate::SvgFontSubsetStage::Sfnt,
    })?;
    builder.copy_missing_tables(font);
    Ok(builder.build())
}

fn rebuild_glyf_loca(
    font: &FontRef<'_>,
    clear_simple_overlap: bool,
    stage: crate::SvgFontSubsetStage,
) -> Result<
    (
        write_fonts::tables::glyf::Glyf,
        write_fonts::tables::loca::Loca,
    ),
    Error,
> {
    let glyf = font.glyf().map_err(|_| Error { stage })?;
    let loca = font.loca(None).map_err(|_| Error { stage })?;
    let glyph_count = font.maxp().map_err(|_| Error { stage })?.num_glyphs();
    let mut builder = GlyfLocaBuilder::new();
    for glyph_id in 0..glyph_count {
        let glyph_ref = loca
            .get_glyf(GlyphId::new(u32::from(glyph_id)), &glyf)
            .map_err(|_| Error { stage })?;
        if let Some(write_fonts::read::tables::glyf::Glyph::Composite(composite)) = &glyph_ref {
            for component in composite.components() {
                if component.glyph.to_u32() >= u32::from(glyph_count)
                    || loca
                        .get_glyf(GlyphId::new(component.glyph.to_u32()), &glyf)
                        .map_err(|_| Error { stage })?
                        .is_none()
                {
                    return Err(Error { stage });
                }
            }
        }
        let mut glyph = glyph_ref
            .map(|glyph| Glyph::from_table_ref(&glyph))
            .unwrap_or(Glyph::Empty);
        if clear_simple_overlap {
            if let Glyph::Simple(simple) = &mut glyph {
                simple.overlaps = false;
            }
        }
        builder.add_glyph(&glyph).map_err(|_| Error { stage })?;
    }
    let (glyf, loca, _) = builder.build();
    Ok((glyf, loca))
}

#[derive(Default, PartialEq)]
struct Outline(Vec<OutlineCommand>);

#[derive(PartialEq)]
enum OutlineCommand {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Close,
}

impl crate::shaping::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.push(OutlineCommand::Move(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.0.push(OutlineCommand::Line(x, y));
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.push(OutlineCommand::Quad(x1, y1, x, y));
    }

    fn close(&mut self) {
        self.0.push(OutlineCommand::Close);
    }
}

pub(super) fn validate_checksums(sfnt: &[u8]) -> Result<(), Error> {
    if !sfnt.len().is_multiple_of(4)
        || sfnt.as_chunks::<4>().0.iter().fold(0_u32, |sum, word| {
            sum.wrapping_add(u32::from_be_bytes(*word))
        }) != 0xb1b0_afba
    {
        return Err(Error {
            stage: crate::SvgFontSubsetStage::Validate,
        });
    }
    let count = usize::from(u16::from_be_bytes(
        sfnt.get(4..6)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            })?,
    ));
    for index in 0..count {
        let start = 12_usize
            .checked_add(index.checked_mul(16).ok_or(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            })?)
            .ok_or(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            })?;
        let record = sfnt.get(start..start + 16).ok_or(Error {
            stage: crate::SvgFontSubsetStage::Validate,
        })?;
        let is_head = record.get(0..4) == Some(b"head");
        let expected = u32::from_be_bytes(record[4..8].try_into().map_err(|_| Error {
            stage: crate::SvgFontSubsetStage::Validate,
        })?);
        let offset = usize::try_from(u32::from_be_bytes(record[8..12].try_into().map_err(
            |_| Error {
                stage: crate::SvgFontSubsetStage::Validate,
            },
        )?))
        .map_err(|_| Error {
            stage: crate::SvgFontSubsetStage::Validate,
        })?;
        let length = usize::try_from(u32::from_be_bytes(record[12..16].try_into().map_err(
            |_| Error {
                stage: crate::SvgFontSubsetStage::Validate,
            },
        )?))
        .map_err(|_| Error {
            stage: crate::SvgFontSubsetStage::Validate,
        })?;
        let data = sfnt
            .get(
                offset..offset.checked_add(length).ok_or(Error {
                    stage: crate::SvgFontSubsetStage::Validate,
                })?,
            )
            .ok_or(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            })?;
        if table_checksum(data, is_head) != expected {
            return Err(Error {
                stage: crate::SvgFontSubsetStage::Validate,
            });
        }
    }
    Ok(())
}

fn table_checksum(data: &[u8], zero_adjustment: bool) -> u32 {
    let mut sum = 0_u32;
    for (index, chunk) in data.chunks(4).enumerate() {
        let mut word = [0_u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        if zero_adjustment && index == 2 {
            word = [0; 4];
        }
        sum = sum.wrapping_add(u32::from_be_bytes(word));
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name_fixture(records: &[([u16; 4], &[u8])]) -> Vec<u8> {
        let mut table = Vec::new();
        for value in [0, records.len() as u16, (6 + records.len() * 12) as u16] {
            table.extend_from_slice(&value.to_be_bytes());
        }
        let mut offset = 0_u16;
        for (keys, bytes) in records {
            for value in keys.iter().copied().chain([bytes.len() as u16, offset]) {
                table.extend_from_slice(&value.to_be_bytes());
            }
            offset += bytes.len() as u16;
        }
        for (_, bytes) in records {
            table.extend_from_slice(bytes);
        }
        table
    }

    fn font_with_names(table: Vec<u8>) -> Vec<u8> {
        let mut builder = FontBuilder::new();
        builder.add_raw(Tag::new(b"name"), table);
        builder.build()
    }

    fn validation_names(legal: &[u8]) -> Vec<u8> {
        let names = [
            (0, legal.to_vec()),
            (1, utf16be("Test")),
            (2, utf16be("Regular")),
            (
                3,
                utf16be("excalidraw-render;Test;0123456789abcdef0123456789abcdef"),
            ),
            (4, utf16be("Test Regular 0123456789abcdef0123456789abcdef")),
            (6, utf16be("Test-Regular-0123456789abcdef0123456789abcdef")),
        ];
        name_fixture(
            &names
                .iter()
                .map(|(id, bytes)| ([3, 1, 0x0409, *id], bytes.as_slice()))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn name_validation_preserves_a_complete_match_before_truncated_records() {
        let mut source = name_fixture(&[([3, 1, 0x0409, 0], &[0xd8, 0x00])]);
        // The declared second record is absent; the first legal record is complete.
        source[2..4].copy_from_slice(&2_u16.to_be_bytes());
        let source = font_with_names(source);
        let changed = font_with_names(validation_names(&[0xff, 0xfd]));
        assert_eq!(
            validate_names(
                &source,
                &changed,
                "Test",
                "0123456789abcdef0123456789abcdef"
            )
            .expect_err("a complete legal record must not be silently omitted")
            .stage,
            crate::SvgFontSubsetStage::Validate
        );
        let preserved = font_with_names(validation_names(&[0xd8, 0x00]));
        validate_names(
            &source,
            &preserved,
            "Test",
            "0123456789abcdef0123456789abcdef",
        )
        .expect("raw unpaired surrogate remains intact");
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn name_validation_keeps_first_english_windows_bytes_without_decoding() {
        for encoding in [0, 1, 2, 10, u16::MAX] {
            for legal in [&[0xd8, 0x00][..], &[0xdc, 0x00], &[0x00, 0x41, 0xff], &[]] {
                let source = font_with_names(name_fixture(&[
                    ([0, 1, 0x0409, 0], &[0x00, 0x55]),
                    ([3, 1, 0x0411, 0], &[0x00, 0x4a]),
                    ([3, encoding, 0x0409, 0], legal),
                    ([3, 1, 0x0409, 0], &[0x00, 0x42]),
                ]));
                let preserved = font_with_names(validation_names(legal));
                validate_names(
                    &source,
                    &preserved,
                    "Test",
                    "0123456789abcdef0123456789abcdef",
                )
                .expect("first matching record is preserved irrespective of encoding");
                let changed = font_with_names(validation_names(&[0x00, 0x42]));
                assert_eq!(
                    validate_names(
                        &source,
                        &changed,
                        "Test",
                        "0123456789abcdef0123456789abcdef"
                    )
                    .expect_err("a later duplicate cannot replace the first match")
                    .stage,
                    crate::SvgFontSubsetStage::Validate
                );
            }
        }
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn name_validation_does_not_skip_an_unreadable_first_match() {
        let mut out_of_bounds = name_fixture(&[
            ([3, 1, 0x0409, 0], &[0x00, 0x41]),
            ([3, 1, 0x0409, 0], &[0x00, 0x42]),
        ]);
        out_of_bounds[16..18].copy_from_slice(&u16::MAX.to_be_bytes());
        let mut invalid_empty_storage = name_fixture(&[([3, 1, 0x0409, 0], &[])]);
        invalid_empty_storage[4..6].copy_from_slice(&u16::MAX.to_be_bytes());
        for table in [vec![], vec![0; 5], out_of_bounds, invalid_empty_storage] {
            let source = font_with_names(table);
            let subset = font_with_names(validation_names(&[0x00, 0x43]));
            validate_names(&source, &subset, "Test", "0123456789abcdef0123456789abcdef")
                .expect("unreadable source record is absent, not a later or empty record");
        }
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn derived_names_preserve_raw_legal_records_and_selected_ids() {
        let asset =
            crate::fonts::inspection::load_asset("fonts/Liberation/LiberationSans-Regular.woff2")
                .unwrap()
                .unwrap();
        let sfnt = wuff::decompress_woff2(asset.bytes()).unwrap();
        let mut builder = FontBuilder::new();
        builder.add_raw(
            Tag::new(b"name"),
            name_fixture(&[
                ([3, 1, 0x0409, 0], &[0xd8, 0x00]),
                ([3, 1, 0x0409, 5], &[0x00, 0x56]),
                ([3, 1, 0x0409, 7], &[0x00, 0x58]),
                ([3, 1, 0x0409, 13], &[0x00, 0x41, 0xff]),
                ([3, 1, 0x0409, 14], &[0xdc, 0x00]),
            ]),
        );
        builder.copy_missing_tables(FontRef::new(&sfnt).unwrap());
        let derived = derive(
            &builder.build(),
            SourceContainer::Sfnt,
            &[u32::from('A')],
            "ExcalidrawRender-9",
            &"0".repeat(64),
        )
        .expect("raw legal bytes survive the complete derivation");
        let sfnt = wuff::decompress_woff2(&derived.woff2).unwrap();
        let font = FontRef::new(&sfnt).unwrap();
        let name = font.name().unwrap();
        assert_eq!(
            name.name_record()
                .iter()
                .map(|record| record.name_id().to_u16())
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5, 6, 13, 14]
        );
        for record in name.name_record() {
            assert_eq!(
                (
                    record.platform_id(),
                    record.encoding_id(),
                    record.language_id()
                ),
                (3, 1, 0x0409)
            );
        }
        for (id, expected) in [
            (0, &[0xd8, 0x00][..]),
            (5, &[0x00, 0x56]),
            (13, &[0x00, 0x41, 0xff]),
            (14, &[0xdc, 0x00]),
        ] {
            let record = name
                .name_record()
                .iter()
                .find(|record| record.name_id() == NameId::new(id))
                .unwrap();
            let start = record.string_offset().non_null().unwrap_or(0);
            assert_eq!(
                &name.string_data().as_bytes()[start..start + usize::from(record.length())],
                expected
            );
        }
    }

    #[test]
    #[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
    fn derived_names_reject_unrepresentable_lengths_and_offsets_at_sfnt_stage() {
        let asset =
            crate::fonts::inspection::load_asset("fonts/Liberation/LiberationSans-Regular.woff2")
                .unwrap()
                .unwrap();
        // Individual UTF-16 byte length, supplementary-plane length, then
        // cumulative storage offset overflow while individual strings fit u16.
        for alias in [
            "A".repeat(32768),
            "\u{1f600}".repeat(16384),
            "A".repeat(20000),
        ] {
            assert_eq!(
                derive(
                    asset.bytes(),
                    SourceContainer::Woff2,
                    &[u32::from('A')],
                    &alias,
                    &"0".repeat(64)
                )
                .expect_err("name storage fields must fit u16")
                .stage,
                crate::SvgFontSubsetStage::Sfnt
            );
        }
    }

    #[test]
    fn manifested_zero_subset_flags_disable_every_skera_option() {
        for flag in [
            SubsetFlags::SUBSET_FLAGS_NO_HINTING,
            SubsetFlags::SUBSET_FLAGS_RETAIN_GIDS,
            SubsetFlags::SUBSET_FLAGS_DESUBROUTINIZE,
            SubsetFlags::SUBSET_FLAGS_NAME_LEGACY,
            SubsetFlags::SUBSET_FLAGS_SET_OVERLAPS_FLAG,
            SubsetFlags::SUBSET_FLAGS_PASSTHROUGH_UNRECOGNIZED,
            SubsetFlags::SUBSET_FLAGS_NOTDEF_OUTLINE,
            SubsetFlags::SUBSET_FLAGS_GLYPH_NAMES,
            SubsetFlags::SUBSET_FLAGS_NO_PRUNE_UNICODE_RANGES,
            SubsetFlags::SUBSET_FLAGS_NO_LAYOUT_CLOSURE,
            SubsetFlags::SUBSET_FLAGS_OPTIMIZE_IUP_DELTAS,
            SubsetFlags::SUBSET_FLAGS_NO_BIDI_CLOSURE,
        ] {
            assert!(!SUBSET_FLAGS.contains(flag));
        }
        assert_eq!(ENCODER_THREADS, NonZeroUsize::new(1));
    }

    #[test]
    fn subset_cache_evicts_in_insertion_order() {
        let mut session = Session::new();
        for index in 0..=MAX_CACHE_ENTRIES {
            session.retain(
                CacheKey {
                    asset_identity: "test-font",
                    effective_alias: "Test".to_owned(),
                    semantic_identity: format!("{index:03}"),
                },
                Derived {
                    woff2: Arc::from([u8::try_from(index % 256).unwrap_or_default()]),
                    decoded_sfnt_bytes: 0,
                    subset_sfnt_bytes: 0,
                    subset_tables: 0,
                },
            );
        }

        assert_eq!(session.derived.len(), MAX_CACHE_ENTRIES);
        assert_eq!(session.derived.front().unwrap().0.semantic_identity, "001");
        assert_eq!(session.derived.back().unwrap().0.semantic_identity, "230");
    }

    #[test]
    fn default_ignorables_keep_the_space_dependency_and_hidden_clusters() {
        let asset =
            crate::fonts::inspection::load_asset("fonts/Liberation/LiberationSans-Regular.woff2")
                .unwrap()
                .unwrap();
        let source_sfnt = wuff::decompress_woff2(asset.bytes()).unwrap();
        let source = crate::shaping::Face::from_slice(&source_sfnt, 0).unwrap();
        for text in ["\u{ad}", "A\u{ad}B", "A\u{200d}B", "AB"] {
            let scalars: Vec<_> = text.chars().map(u32::from).collect();
            let derived = derive(
                asset.bytes(),
                SourceContainer::Woff2,
                &scalars,
                "ExcalidrawRender-9",
                &"0".repeat(64),
            )
            .unwrap();
            let sfnt = wuff::decompress_woff2(&derived.woff2).unwrap();
            let subset = crate::shaping::Face::from_slice(&sfnt, 0).unwrap();
            assert_eq!(subset.glyph_index(' ').is_some(), text != "AB");
            for direction in [
                crate::shaping::Direction::LeftToRight,
                crate::shaping::Direction::RightToLeft,
            ] {
                validate_shaping_probe(&source, &subset, text, Some(direction)).unwrap();
            }
            let shaped = crate::shaping::shape(&subset, &[], text, None);
            assert_eq!(shaped.glyph_infos().len(), text.chars().count());
            if let Some(offset) = text.find(super::super::text::is_default_ignorable) {
                // A joiner can merge into its preceding grapheme cluster;
                // locate its hidden glyph by the retained space mapping.
                let space = subset.glyph_index(' ').unwrap();
                let index = shaped
                    .glyph_infos()
                    .iter()
                    .position(|info| info.glyph_id == u32::from(space.0))
                    .unwrap();
                assert!(shaped.glyph_infos()[index].cluster <= offset as u32);
                assert_eq!(shaped.glyph_positions()[index].x_advance, 0);
                let id = crate::shaping::GlyphId(shaped.glyph_infos()[index].glyph_id as u16);
                assert!(subset.outline_glyph(id, &mut Outline::default()).is_none());
            }
        }
    }

    #[test]
    fn whitespace_only_subsets_keep_a_browser_admissible_glyf_table() {
        let record = crate::fonts::revision_1()
            .assets
            .iter()
            .find(|record| record.family == "Liberation")
            .expect("Liberation source is manifested");
        let asset = crate::fonts::inspection::load_asset(record.upstream_path)
            .expect("Liberation source verifies")
            .expect("Liberation source exists");
        let derived = derive(
            asset.bytes(),
            SourceContainer::Woff2,
            &[u32::from(' ')],
            "ExcalidrawRender-9",
            &"0".repeat(64),
        )
        .expect("whitespace-only subset derives");
        let sfnt = wuff::decompress_woff2(&derived.woff2).expect("subset decodes");
        let font = FontRef::new(&sfnt).expect("subset sfnt parses");

        assert!(
            font.data_for_tag(Tag::new(b"glyf"))
                .is_some_and(|table| !table.as_bytes().is_empty()),
            "Chromium OTS rejects zero-length glyf tables"
        );
    }

    #[test]
    fn malformed_woff2_sources_return_typed_failures_without_panicking() {
        let record = &crate::fonts::revision_1().assets[0];
        let asset = crate::fonts::inspection::load_asset(record.upstream_path)
            .expect("qualified source verifies")
            .expect("qualified source exists");
        let mut bad_signature = asset.bytes().to_vec();
        bad_signature[0] ^= 0xff;
        for malformed in [
            &[][..],
            &asset.bytes()[..4],
            &asset.bytes()[..asset.bytes().len() / 2],
            bad_signature.as_slice(),
        ] {
            let result = std::panic::catch_unwind(|| {
                derive(
                    malformed,
                    SourceContainer::Woff2,
                    &[u32::from('A')],
                    "Test",
                    &"0".repeat(64),
                )
            });
            let failure = result.expect("malformed source handling does not panic");
            let error = match failure {
                Ok(_) => panic!("malformed WOFF2 is rejected"),
                Err(error) => error,
            };
            assert_eq!(error.stage, crate::SvgFontSubsetStage::Decode);
        }
    }

    /// The Noto Emoji fallback is an sfnt, not a WOFF2 chunk (#150): its
    /// decode stage is the identity, the frozen table policy admits every
    /// table it carries, and the derived subset keeps the source's outlines,
    /// advances and metrics for the requested scalars under the canonical
    /// family name, since Noto Emoji reserves no font name.
    #[test]
    fn noto_emoji_sfnt_source_derives_a_validated_woff2_subset() {
        let asset =
            crate::fonts::inspection::load_noto_emoji_sfnt().expect("Noto Emoji fallback verifies");
        let scalars = [0x1f600, 0x1f389];
        let identity = "1".repeat(64);
        let derived = derive(
            asset.bytes(),
            SourceContainer::Sfnt,
            &scalars,
            "Noto Emoji",
            &identity,
        )
        .expect("Noto Emoji subset derives");

        assert_eq!(derived.decoded_sfnt_bytes, asset.bytes().len());
        assert!(derived.woff2.len() < asset.bytes().len() / 64);
        let sfnt = wuff::decompress_woff2(&derived.woff2).expect("subset decodes");
        let subset = crate::shaping::Face::from_slice(&sfnt, 0).expect("subset parses");
        let source = crate::shaping::Face::from_slice(asset.bytes(), 0).expect("source parses");
        for scalar in scalars {
            let character = char::from_u32(scalar).expect("scalar is a character");
            let source_glyph = source
                .glyph_index(character)
                .expect("source maps the emoji");
            let subset_glyph = subset
                .glyph_index(character)
                .expect("subset maps the emoji");
            assert_eq!(
                source.glyph_hor_advance(source_glyph),
                subset.glyph_hor_advance(subset_glyph)
            );
        }
        let font = FontRef::new(&sfnt).expect("subset sfnt parses");
        let table = font
            .data_for_tag(Tag::new(b"name"))
            .expect("subset has names");
        assert_eq!(
            english_name(table.as_bytes(), 1),
            Some(utf16be("Noto Emoji").as_slice())
        );
        assert_eq!(
            english_name(table.as_bytes(), 0),
            Some(utf16be("Copyright 2013 Google LLC").as_slice())
        );
        assert!(font.data_for_tag(Tag::new(b"FFTM")).is_none());
    }

    /// A truncated or non-sfnt fallback source fails at the decode stage like
    /// a malformed WOFF2 chunk does, without panicking.
    #[test]
    fn malformed_sfnt_sources_return_typed_decode_failures() {
        let asset =
            crate::fonts::inspection::load_noto_emoji_sfnt().expect("Noto Emoji fallback verifies");
        for malformed in [&[][..], &asset.bytes()[..4], &asset.bytes()[..4096]] {
            let error = derive(
                malformed,
                SourceContainer::Sfnt,
                &[0x1f600],
                "Noto Emoji",
                &"1".repeat(64),
            )
            .expect_err("malformed sfnt is rejected");
            assert_eq!(error.stage, crate::SvgFontSubsetStage::Decode);
        }
    }

    #[test]
    fn malformed_generated_sfnt_validation_is_bounded_and_typed() {
        for malformed in [&[][..], &[0, 1, 0, 0][..], &[0; 12][..], &[0; 64][..]] {
            assert_eq!(
                validate_checksums(malformed)
                    .expect_err("malformed sfnt is rejected")
                    .stage,
                crate::SvgFontSubsetStage::Validate
            );
        }
    }

    #[test]
    fn malformed_layout_closure_returns_a_typed_validate_failure() {
        let record = crate::fonts::revision_1()
            .assets
            .iter()
            .find(|record| record.family == "Liberation")
            .expect("Liberation source is manifested");
        let asset = crate::fonts::inspection::load_asset(record.upstream_path)
            .expect("Liberation source verifies")
            .expect("Liberation source exists");
        let derived = derive(
            asset.bytes(),
            SourceContainer::Woff2,
            &[u32::from('A')],
            "ExcalidrawRender-9",
            &"0".repeat(64),
        )
        .expect("fixture subset derives");
        let sfnt = wuff::decompress_woff2(&derived.woff2).expect("fixture subset decodes");

        let outside_maxp = [
            0x00, 0x01, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
            0xff, 0xff, 0x00, 0x01, 0x00, 0x01,
        ];
        let unresolved_offset = [
            0x00, 0x01, 0x00, 0x00, 0x7f, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        for malformed_gdef in [&outside_maxp[..], &unresolved_offset[..]] {
            let font = FontRef::new(&sfnt).expect("fixture sfnt parses");
            let mut builder = FontBuilder::new();
            builder.add_raw(Tag::new(b"GDEF"), malformed_gdef.to_vec());
            builder.copy_missing_tables(font);
            let malformed = builder.build();
            let malformed = FontRef::new(&malformed).expect("outer malformed sfnt parses");

            assert_eq!(
                validate_layout_closure(&malformed)
                    .expect_err("broken layout closure is rejected")
                    .stage,
                crate::SvgFontSubsetStage::Validate
            );
        }
    }
}
