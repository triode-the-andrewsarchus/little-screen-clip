use super::font_subset;
use crate::fonts::{Container, Face, FamilyId, FontNotice};
use crate::resource_accounting::{
    ResourceAccount, ResourceContext, ResourceDelta, ResourcePhase, ResourceRecordKind,
};
use sha2::{Digest, Sha256};
use std::collections::{hash_map::DefaultHasher, HashSet, TryReserveError};
use std::hash::BuildHasherDefault;
use std::sync::Arc;

pub(super) struct Demand {
    pub(super) families: Vec<FamilyDemand>,
}

pub(super) struct FamilyDemand {
    pub(super) family: &'static str,
    pub(super) scalars: Vec<u32>,
    scalar_membership: HashSet<u32, BuildHasherDefault<DefaultHasher>>,
}

impl FamilyDemand {
    pub(super) fn new(family: &'static str) -> Self {
        Self {
            family,
            scalars: Vec::new(),
            scalar_membership: HashSet::default(),
        }
    }

    pub(super) fn from_scalars(
        family: &'static str,
        scalars: &[u32],
    ) -> Result<Self, TryReserveError> {
        let mut demand = Self::new(family);
        for scalar in scalars {
            if !demand.scalar_membership.contains(scalar) {
                demand.scalars.try_reserve(1)?;
                demand.scalar_membership.try_reserve(1)?;
                demand.scalar_membership.insert(*scalar);
                demand.scalars.push(*scalar);
            }
        }
        Ok(demand)
    }

    pub(super) fn add_text(&mut self, text: &str) -> Result<(), TryReserveError> {
        for scalar in text.chars().map(u32::from) {
            if !self.scalar_membership.contains(&scalar) {
                self.scalars.try_reserve(1)?;
                self.scalar_membership.try_reserve(1)?;
                self.scalar_membership.insert(scalar);
                self.scalars.push(scalar);
            }
        }
        Ok(())
    }
}

/// One source direct SVG may subset for a demanded family (spec 8.3.1): an
/// official Unicode-range WOFF2 chunk of a resolved family, or the complete
/// Noto Emoji fallback face, which is its own one-face family declaring every
/// Unicode scalar and is selected exactly when an emoji run demanded it
/// (#150). Both go through the one qualified subsetter; the fallback differs
/// only in arriving as an sfnt rather than a WOFF2 container.
#[derive(Clone, Copy, Debug)]
pub(super) struct SubsetSource {
    face: &'static Face,
}

/// The most sources one family can have: Xiaolai's chunk count bounds it, and
/// the catalog holds no more chunks than this in total.
const MAX_FAMILY_SOURCES: usize = 230;

impl SubsetSource {
    #[cfg(test)]
    pub(super) fn chunk(record: &'static crate::fonts::inspection::AssetRecord) -> Self {
        Self {
            face: crate::fonts::find_face(record.upstream_path).expect("catalog chunk has a face"),
        }
    }

    #[cfg(test)]
    pub(super) fn noto_emoji() -> Self {
        Self {
            face: &FamilyId::NotoEmoji.faces()[0],
        }
    }

    /// The catalog identity the subset notice, the semantic identity and the
    /// resource trace name the source by.
    pub(super) const fn identity(self) -> &'static str {
        self.face.identity()
    }

    pub(super) const fn complete_sha256(self) -> &'static str {
        self.face.source_sha256()
    }

    /// The manifested source length: the WOFF2 container of a chunk, the
    /// sfnt itself for the fallback.
    const fn complete_length(self) -> usize {
        self.face.source_length()
    }

    /// The decoded sfnt length the dependency envelope is sized from: a
    /// chunk's reviewed decode, the fallback's own length.
    fn decoded_length(self) -> Option<usize> {
        Some(self.face.decoded_length())
    }

    pub(super) const fn container(self) -> font_subset::SourceContainer {
        match self.face.container() {
            Container::Woff2 => font_subset::SourceContainer::Woff2,
            Container::Sfnt => font_subset::SourceContainer::Sfnt,
        }
    }

    /// The media type the source's own bytes carry when a test embeds them
    /// complete in place of a subset.
    #[cfg(test)]
    const fn complete_mime(self) -> &'static str {
        match self.face.container() {
            Container::Woff2 => "font/woff2",
            Container::Sfnt => "font/ttf",
        }
    }

    /// The declared Unicode ranges selection intersects the family scalar set
    /// with; `None` only when the catalog lacks a chunk's declaration.
    pub(super) fn unicode_ranges(self) -> Option<&'static [(u32, u32)]> {
        Some(self.face.declared_ranges())
    }

    /// Whether the family scalar set intersects the declared ranges, which is
    /// what selects the source.
    pub(super) fn selected_by(self, scalars: &[u32]) -> Option<bool> {
        let ranges = self.unicode_ranges()?;
        Some(scalars.iter().any(|scalar| {
            ranges
                .iter()
                .any(|(start, end)| start <= scalar && scalar <= end)
        }))
    }

    /// Loads the source on demand, preserving unavailable and corrupt asset errors.
    #[cfg(test)]
    pub(super) fn load(
        self,
    ) -> Result<
        Option<crate::fonts::inspection::VerifiedAsset>,
        crate::fonts::inspection::AssetIntegrityError,
    > {
        crate::fonts::load_source(self.face.id()).map(Some)
    }
}

/// Fills `slots` with every source of the resolved `family`, in pinned face
/// order, and returns how many there are; `None` when the family has more
/// sources than the slots hold, which the catalog bound above rules out.
fn family_sources(family: &'static str, slots: &mut [Option<SubsetSource>]) -> Option<usize> {
    let family = FamilyId::from_name(family)?;
    for (index, face) in family.faces().iter().enumerate() {
        *slots.get_mut(index)? = Some(SubsetSource { face });
    }
    Some(family.faces().len())
}

#[derive(Clone, Copy)]
pub(super) enum Payload {
    Subset,
    #[cfg(test)]
    CompleteChunks,
}

pub(super) struct Session {
    subset: font_subset::Session,
    fonts: crate::fonts::FontLoader,
}

impl Session {
    pub(super) fn new() -> Self {
        Self {
            subset: font_subset::Session::new(),
            fonts: crate::fonts::FontLoader::default(),
        }
    }

    #[cfg(test)]
    pub(super) const fn stats(&self) -> (usize, usize) {
        self.subset.stats()
    }
}

#[derive(Debug)]
pub(super) struct Bundle {
    resources: Vec<Resource>,
    mandatory_notices: Option<super::notices::MandatoryNotices>,
}

struct Resource {
    family: &'static str,
    mime: &'static str,
    bytes: Arc<[u8]>,
    metrics: Option<font_subset::DerivationMetrics>,
}

/// `DerivationMetrics` lives in the digest-pinned `font_subset.rs` and cannot
/// grow a derive until its next re-qualification, so a resource prints the
/// identity of the chunk it was derived from in the metrics' place and the
/// length of its bytes rather than the bytes.
impl std::fmt::Debug for Resource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Resource")
            .field("family", &self.family)
            .field("mime", &self.mime)
            .field("bytes", &self.bytes.len())
            .field(
                "derived_from",
                &self
                    .metrics
                    .as_ref()
                    .map(font_subset::DerivationMetrics::asset_identity),
            )
            .finish()
    }
}

pub(super) struct BuildError {
    pub(super) stage: crate::SvgFontSubsetStage,
    pub(super) family: &'static str,
    pub(super) asset_identity: &'static str,
}

impl From<BuildError> for super::FormatEncodeError {
    fn from(error: BuildError) -> Self {
        Self::SvgFontSubset {
            stage: error.stage,
            resolved_family: error.family,
            asset_identity: error.asset_identity,
        }
    }
}

pub(super) struct SubsetNotice {
    /// The family notice the subset is a Modified Version under; the artifact
    /// carries its attribution and license body beside the subset notice.
    pub(super) provenance: &'static FontNotice,
    pub(super) resource_identity: String,
    pub(super) resource_text: String,
}

impl Bundle {
    pub(super) fn take_mandatory_notices(&mut self) -> Option<super::notices::MandatoryNotices> {
        self.mandatory_notices.take()
    }

    pub(super) fn account_resource_transcript<A: ResourceAccount>(
        &self,
        account: &mut A,
    ) -> Result<(), A::Error> {
        for metrics in self
            .resources
            .iter()
            .filter_map(|resource| resource.metrics.as_ref())
        {
            font_subset::account_derivation(account, metrics)?;
            account.transact_resource(
                ResourceContext::new(
                    ResourcePhase::EncodingAndOutcomeAssembly,
                    "svg_font_css_data_url",
                    "",
                )
                .with_semantic_identity(metrics.asset_identity()),
                ResourceDelta::default(),
            )?;
        }
        Ok(())
    }

    /// The `<style class="style-fonts">` block declaring every embedded face,
    /// for the document's `<defs>`; nothing when no face is embedded.
    pub(super) fn write_style<W: super::svg::SvgSink + ?Sized>(
        &self,
        output: &mut W,
        mut write_base64: impl FnMut(&mut W, &[u8]) -> Result<(), super::FormatEncodeError>,
    ) -> Result<(), super::FormatEncodeError> {
        if self.resources.is_empty() {
            return Ok(());
        }
        output.push_str("<style class=\"style-fonts\">");
        for resource in &self.resources {
            output.push_str("@font-face{font-family:'");
            output.push_str(resource.family);
            output.push_str("';src:url(data:");
            output.push_str(resource.mime);
            output.push_str(";base64,");
            write_base64(output, resource.bytes.as_ref())?;
            output.push_str(")}");
        }
        output.push_str("</style>");
        Ok(())
    }

    pub(super) fn effective_family(family: &'static str) -> &'static str {
        match family {
            "Virgil" => "ExcalidrawRender-1",
            "Cascadia Code" => "ExcalidrawRender-3",
            "Lilita One" => "ExcalidrawRender-7",
            "Liberation Sans" => "ExcalidrawRender-9",
            _ => family,
        }
    }

    /// The `font-family` list SVG text carries for a resolved family (#150,
    /// #151). The effective family comes first, so a browser and the parity
    /// harness resolve the face the document embeds exactly as before; the
    /// rest is for a consumer that ignores `@font-face` and would otherwise
    /// fall to its default font for an alias it has never heard of: the
    /// canonical family name where the alias differs, Helvetica for the
    /// Liberation Sans that substitutes it (Section 8.3), the reference
    /// export's own fallback chain for the family (`getFontFamilyFallbacks`:
    /// Xiaolai for Excalifont, then Segoe UI Emoji for every family), and a
    /// generic family of the same class. Names are spelled bare, as the
    /// reference spells `Segoe UI Emoji`.
    pub(super) fn font_family_list(family: &'static str) -> &'static str {
        match family {
            "Virgil" => "ExcalidrawRender-1, Virgil, Segoe UI Emoji, sans-serif",
            "Cascadia Code" => "ExcalidrawRender-3, Cascadia Code, Segoe UI Emoji, monospace",
            "Lilita One" => "ExcalidrawRender-7, Lilita One, Segoe UI Emoji, sans-serif",
            "Liberation Sans" => {
                "ExcalidrawRender-9, Liberation Sans, Helvetica, Segoe UI Emoji, sans-serif"
            }
            "Excalifont" => "Excalifont, Xiaolai, Segoe UI Emoji, sans-serif",
            "Xiaolai" => "Xiaolai, Segoe UI Emoji, sans-serif",
            "Nunito" => "Nunito, Segoe UI Emoji, sans-serif",
            "Comic Shanns" => "Comic Shanns, Segoe UI Emoji, monospace",
            "Noto Emoji" => "Noto Emoji, Segoe UI Emoji, sans-serif",
            _ => family,
        }
    }
}

#[cfg(test)]
mod family_list_tests {
    use super::Bundle;

    /// Every bundled family the writer can name, resolved families and the
    /// two fallback faces alike.
    const FAMILIES: [&str; 9] = [
        "Virgil",
        "Liberation Sans",
        "Cascadia Code",
        "Excalifont",
        "Nunito",
        "Lilita One",
        "Comic Shanns",
        "Xiaolai",
        "Noto Emoji",
    ];

    /// The list starts with the effective family the `@font-face` declares,
    /// so a viewer that honors embedded fonts resolves the same face as the
    /// single-family attribute did, and ends with the reference's Segoe UI
    /// Emoji fallback followed by a generic family.
    #[test]
    fn every_family_list_leads_with_its_effective_family_and_ends_generic() {
        for family in FAMILIES {
            let list = Bundle::font_family_list(family);
            let mut entries = list.split(", ");
            assert_eq!(entries.next(), Some(Bundle::effective_family(family)));
            assert!(
                list.split(", ").any(|entry| entry == family),
                "{list} names the canonical family"
            );
            let tail: Vec<&str> = entries.collect();
            let [.., emoji, generic] = tail.as_slice() else {
                panic!("{list} has a fallback chain");
            };
            assert_eq!(*emoji, "Segoe UI Emoji");
            assert!(matches!(*generic, "sans-serif" | "monospace"), "{list}");
            assert!(
                !list.contains('\'') && !list.contains('"'),
                "{list} is spelled bare"
            );
        }
    }

    /// The reference's `getFontFamilyFallbacks` puts Xiaolai before Segoe UI
    /// Emoji for Excalifont alone; the monospace faces degrade to `monospace`.
    #[test]
    fn family_lists_mirror_the_reference_fallback_order() {
        assert_eq!(
            Bundle::font_family_list("Excalifont"),
            "Excalifont, Xiaolai, Segoe UI Emoji, sans-serif"
        );
        assert_eq!(
            Bundle::font_family_list("Liberation Sans"),
            "ExcalidrawRender-9, Liberation Sans, Helvetica, Segoe UI Emoji, sans-serif"
        );
        for family in FAMILIES {
            let list = Bundle::font_family_list(family);
            assert_eq!(
                list.contains("Xiaolai") && family != "Xiaolai",
                family == "Excalifont",
                "{list}"
            );
            assert_eq!(
                list.ends_with("monospace"),
                matches!(family, "Cascadia Code" | "Comic Shanns"),
                "{list}"
            );
        }
    }
}

#[derive(Clone, Copy)]
struct WorkingRecord {
    context: ResourceContext<'static>,
    bytes: u64,
}

pub(super) struct SelectionAccounting {
    retained: [Option<WorkingRecord>; MAX_SELECTION_RECORDS],
    retained_len: usize,
}

/// One scalar-set record per demanded family (eight resolved families,
/// Xiaolai and Noto Emoji), then a considered and a selected record per
/// source.
const MAX_SELECTION_RECORDS: usize = 10 + (MAX_FAMILY_SOURCES + 1) * 2;

impl SelectionAccounting {
    pub(super) fn release<A: ResourceAccount>(mut self, account: &mut A) -> Result<(), A::Error> {
        while self.retained_len != 0 {
            self.retained_len -= 1;
            let record = self.retained[self.retained_len]
                .take()
                .ok_or_else(|| account.internal_failure("SVG font selection record is missing"))?;
            account.release_resource(record.context, record.bytes)?;
        }
        Ok(())
    }

    fn retain<A: ResourceAccount>(
        &mut self,
        account: &mut A,
        record: WorkingRecord,
    ) -> Result<(), A::Error> {
        let slot = self
            .retained
            .get_mut(self.retained_len)
            .ok_or_else(|| account.internal_failure("SVG font selection record bound exceeded"))?;
        *slot = Some(record);
        self.retained_len += 1;
        Ok(())
    }
}

pub(super) struct DependencyEnvelope(WorkingRecord);

impl DependencyEnvelope {
    pub(super) fn release<A: ResourceAccount>(self, account: &mut A) -> Result<(), A::Error> {
        account.release_resource(self.0.context, self.0.bytes)
    }
}

pub(super) fn account_demand<A: ResourceAccount>(
    account: &mut A,
    estimates: impl Iterator<Item = (&'static str, usize)>,
) -> Result<SelectionAccounting, A::Error> {
    let mut accounting = SelectionAccounting {
        retained: [None; MAX_SELECTION_RECORDS],
        retained_len: 0,
    };

    let result = (|| -> Result<(), A::Error> {
        for (family, scalar_count) in estimates.filter(|(_, count)| *count != 0) {
            let scalar_count = u64::try_from(scalar_count)
                .map_err(|_| account.internal_failure("SVG font scalar count overflowed"))?;
            let scalar_work = checked_product(
                account,
                scalar_count,
                36,
                "SVG font scalar-set work overflowed",
            )?;
            let scalar_working = checked_product(
                account,
                scalar_count,
                64,
                "SVG font scalar-set working bytes overflowed",
            )?;
            let context = ResourceContext::new(
                ResourcePhase::FormatPlanning,
                "svg_font_family_scalar_set",
                "",
            )
            .with_semantic_identity(family)
            .with_record(ResourceRecordKind::BuiltInGlyphOrRangeState);
            account.transact_resource(
                context,
                ResourceDelta {
                    working_bytes: scalar_working,
                    total_work_units: scalar_work,
                    ..ResourceDelta::default()
                },
            )?;
            accounting.retain(
                account,
                WorkingRecord {
                    context,
                    bytes: scalar_working,
                },
            )?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        accounting.release(account)?;
        return Err(error);
    }
    Ok(accounting)
}

pub(super) fn account_selection<A: ResourceAccount>(
    account: &mut A,
    demand: &Demand,
    mut accounting: SelectionAccounting,
) -> Result<SelectionAccounting, A::Error> {
    let result = (|| -> Result<(), A::Error> {
        for family in &demand.families {
            let scalar_count = u64::try_from(family.scalars.len())
                .map_err(|_| account.internal_failure("SVG font scalar count overflowed"))?;
            let mut sources = [None; MAX_FAMILY_SOURCES];
            let count = family_sources(family.family, &mut sources)
                .ok_or_else(|| account.internal_failure("SVG font family source bound exceeded"))?;
            for source in sources[..count].iter().flatten() {
                let ranges = source.unicode_ranges().ok_or_else(|| {
                    account.internal_failure("official font range declaration is missing")
                })?;
                let range_count = u64::try_from(ranges.len())
                    .map_err(|_| account.internal_failure("SVG font range count overflowed"))?;
                let pairs = checked_product(
                    account,
                    scalar_count,
                    range_count,
                    "SVG font scalar-range pair count overflowed",
                )?;
                let pair_work =
                    checked_product(account, pairs, 32, "SVG font scalar-range work overflowed")?;
                let considered_context = ResourceContext::new(
                    ResourcePhase::FormatPlanning,
                    "svg_font_chunk_considered",
                    "",
                )
                .with_semantic_identity(source.identity())
                .with_record(ResourceRecordKind::BuiltInGlyphOrRangeState);
                account.transact_resource(
                    considered_context,
                    ResourceDelta {
                        working_bytes: 64,
                        total_work_units: pair_work,
                        ..ResourceDelta::default()
                    },
                )?;
                accounting.retain(
                    account,
                    WorkingRecord {
                        context: considered_context,
                        bytes: 64,
                    },
                )?;
                if source.selected_by(&family.scalars) == Some(true) {
                    let context = ResourceContext::new(
                        ResourcePhase::FormatPlanning,
                        "svg_font_chunk_selected",
                        "",
                    )
                    .with_semantic_identity(source.identity())
                    .with_record(ResourceRecordKind::BuiltInSelectionState);
                    account.transact_resource(
                        context,
                        ResourceDelta {
                            working_bytes: 256,
                            total_work_units: 8,
                            ..ResourceDelta::default()
                        },
                    )?;
                    accounting.retain(
                        account,
                        WorkingRecord {
                            context,
                            bytes: 256,
                        },
                    )?;
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        accounting.release(account)?;
        return Err(error);
    }
    Ok(accounting)
}

pub(super) fn account_dependency_envelope<A: ResourceAccount>(
    account: &mut A,
    demand: &Demand,
) -> Result<Option<DependencyEnvelope>, A::Error> {
    const GLYPH_BOUND: u64 = u16::MAX as u64;
    const ENCODER_WORKSPACE: u64 = 8 * 1_024 * 1_024;

    let mut retained_bound = 0_u64;
    let mut active_bound = 0_u64;
    let mut work_bound = 0_u64;
    for family in &demand.families {
        let scalar_count = u64::try_from(family.scalars.len())
            .map_err(|_| account.internal_failure("SVG font scalar count overflowed"))?;
        let mut sources = [None; MAX_FAMILY_SOURCES];
        let count = family_sources(family.family, &mut sources)
            .ok_or_else(|| account.internal_failure("SVG font family source bound exceeded"))?;
        for source in sources[..count].iter().flatten() {
            match source.selected_by(&family.scalars) {
                Some(true) => {}
                Some(false) => continue,
                None => {
                    return Err(
                        account.internal_failure("official font range declaration is missing")
                    )
                }
            }
            let source_length = u64::try_from(source.complete_length())
                .map_err(|_| account.internal_failure("SVG font source size overflowed"))?;
            let decoded = u64::try_from(source.decoded_length().ok_or_else(|| {
                account.internal_failure("official font decoded bound is missing")
            })?)
            .map_err(|_| account.internal_failure("SVG font decoded size overflowed"))?;
            let table_bound = decoded / 16;
            let sfnt_bound = font_subset::sfnt_ceiling(decoded).ok_or_else(|| {
                account.internal_failure("SVG font subset sfnt envelope overflowed")
            })?;
            let woff2_bound = checked_product(
                account,
                decoded,
                2,
                "SVG font subset WOFF2 envelope overflowed",
            )?;
            let base64_groups = checked_sum(
                account,
                &[woff2_bound, 2],
                "SVG font base64 rounding overflowed",
            )? / 3;
            let base64_bound = checked_product(
                account,
                base64_groups,
                4,
                "SVG font base64 envelope overflowed",
            )?;
            let table_work = checked_product(
                account,
                table_bound,
                8,
                "SVG font table work envelope overflowed",
            )?;
            let scalar_work = checked_product(
                account,
                scalar_count,
                32,
                "SVG font scalar work envelope overflowed",
            )?;
            let glyph_work = checked_product(
                account,
                GLYPH_BOUND,
                32,
                "SVG font glyph work envelope overflowed",
            )?;
            let chunk_work = checked_sum(
                account,
                &[
                    source_length,
                    decoded,
                    table_work,
                    scalar_work,
                    glyph_work,
                    sfnt_bound,
                    table_work,
                    sfnt_bound,
                    woff2_bound,
                    woff2_bound,
                    sfnt_bound,
                    glyph_work,
                    woff2_bound,
                    base64_bound,
                ],
                "SVG font dependency work envelope overflowed",
            )?;
            work_bound = checked_sum(
                account,
                &[work_bound, chunk_work],
                "SVG font aggregate work envelope overflowed",
            )?;
            let plan_records = checked_sum(
                account,
                &[scalar_count, GLYPH_BOUND],
                "SVG font subset plan record envelope overflowed",
            )?;
            let plan_bound = checked_product(
                account,
                plan_records,
                64,
                "SVG font subset plan workspace envelope overflowed",
            )?;
            let decoded_blob = checked_derived_blob_bytes(account, decoded)?;
            let sfnt_blob = checked_derived_blob_bytes(account, sfnt_bound)?;
            let chunk_active = checked_sum(
                account,
                &[decoded_blob, sfnt_blob, ENCODER_WORKSPACE, plan_bound],
                "SVG font active dependency envelope overflowed",
            )?;
            active_bound = active_bound.max(chunk_active);
            let woff2_blob = checked_derived_blob_bytes(account, woff2_bound)?;
            retained_bound = checked_sum(
                account,
                &[retained_bound, woff2_blob],
                "SVG font retained dependency envelope overflowed",
            )?;
        }
    }
    let bytes = checked_sum(
        account,
        &[active_bound, retained_bound],
        "SVG font live dependency envelope overflowed",
    )?;
    let context = ResourceContext::new(
        ResourcePhase::EncodingAndOutcomeAssembly,
        "svg_font_dependency_envelope",
        "",
    )
    .with_record(ResourceRecordKind::SvgFontSubsetWorkspace);
    if bytes == 0 {
        return Ok(None);
    }
    account.transact_resource(
        context,
        ResourceDelta {
            working_bytes: bytes,
            total_work_units: work_bound,
            ..ResourceDelta::default()
        },
    )?;
    Ok(Some(DependencyEnvelope(WorkingRecord { context, bytes })))
}

fn checked_product<A: ResourceAccount>(
    account: &mut A,
    left: u64,
    right: u64,
    subject: &'static str,
) -> Result<u64, A::Error> {
    left.checked_mul(right)
        .ok_or_else(|| account.internal_failure(subject))
}

fn checked_sum<A: ResourceAccount>(
    account: &mut A,
    values: &[u64],
    subject: &'static str,
) -> Result<u64, A::Error> {
    values.iter().try_fold(0_u64, |total, value| {
        total
            .checked_add(*value)
            .ok_or_else(|| account.internal_failure(subject))
    })
}

fn checked_derived_blob_bytes<A: ResourceAccount>(
    account: &mut A,
    bytes: u64,
) -> Result<u64, A::Error> {
    let rounded = checked_sum(account, &[bytes, 63], "SVG font blob alignment overflowed")? / 64;
    let aligned = checked_product(account, rounded, 64, "SVG font blob size overflowed")?;
    checked_sum(account, &[256, aligned], "SVG font blob record overflowed")
}

pub(super) fn build(
    demand: Demand,
    payload: Payload,
    session: &mut Session,
) -> Result<Bundle, super::FormatEncodeError> {
    let mut resources = Vec::new();
    let mut notices = super::notices::FontNoticeSet::default();
    for family in demand.families {
        if family.scalars.is_empty() {
            continue;
        }
        let mut sources = [None; MAX_FAMILY_SOURCES];
        let count = family_sources(family.family, &mut sources).ok_or(BuildError {
            stage: crate::SvgFontSubsetStage::Plan,
            family: family.family,
            asset_identity: "family-sources",
        })?;
        for source in sources[..count].iter().flatten().copied() {
            let selected = source.selected_by(&family.scalars).ok_or(BuildError {
                stage: crate::SvgFontSubsetStage::Plan,
                family: family.family,
                asset_identity: source.identity(),
            })?;
            if !selected {
                continue;
            }
            let decode_error = || BuildError {
                stage: crate::SvgFontSubsetStage::Decode,
                family: family.family,
                asset_identity: source.identity(),
            };
            let asset = session.fonts
                .load_source(source.face.id())
                .map_err(|error| {
                    if matches!(&error, crate::fonts::FontError::Integrity(error) if error.is_unavailable()) {
                        return super::FormatEncodeError::FontUnavailable(
                            super::FontUnavailable::face(source.face),
                        );
                    }
                    if matches!(error, crate::fonts::FontError::Memory) {
                        return super::FormatEncodeError::MemoryUnavailable {
                            subject_kind: "svg_font_demand", subject: "font_source_cache",
                        };
                    }
                    super::FormatEncodeError::from(decode_error())
                })?;
            let effective_alias = Bundle::effective_family(family.family);
            match payload {
                Payload::Subset => {
                    let semantic_identity = source_semantic_identity(source, &family.scalars)
                        .ok_or(BuildError {
                            stage: crate::SvgFontSubsetStage::Plan,
                            family: family.family,
                            asset_identity: source.identity(),
                        })?;
                    let derived = session
                        .subset
                        .derive(
                            source.identity(),
                            &asset,
                            source.container(),
                            &family.scalars,
                            effective_alias,
                            &semantic_identity,
                        )
                        .map_err(|error| BuildError {
                            stage: error.stage,
                            family: family.family,
                            asset_identity: source.identity(),
                        })?;
                    let notice = subset_notice(
                        source,
                        family.family,
                        effective_alias,
                        &semantic_identity,
                        &derived,
                    )?;
                    let notice_error = || BuildError {
                        stage: crate::SvgFontSubsetStage::Encode,
                        family: family.family,
                        asset_identity: source.identity(),
                    };
                    notices
                        .add_family(notice.provenance)
                        .map_err(|_| notice_error())?;
                    notices.add_subset(notice.resource_identity, notice.resource_text);
                    resources.push(Resource {
                        family: effective_alias,
                        mime: "font/woff2",
                        metrics: Some(font_subset::DerivationMetrics::new(
                            source.identity(),
                            &derived,
                        )),
                        bytes: derived.woff2,
                    });
                }
                #[cfg(test)]
                Payload::CompleteChunks => {
                    notices
                        .add_family(family_notice(source, family.family)?)
                        .map_err(|_| BuildError {
                            stage: crate::SvgFontSubsetStage::Encode,
                            family: family.family,
                            asset_identity: source.identity(),
                        })?;
                    let mut bytes = Vec::new();
                    bytes
                        .try_reserve_exact(asset.len())
                        .map_err(|_| BuildError {
                            stage: crate::SvgFontSubsetStage::Encode,
                            family: family.family,
                            asset_identity: source.identity(),
                        })?;
                    bytes.extend_from_slice(&asset);
                    resources.push(Resource {
                        family: effective_alias,
                        mime: source.complete_mime(),
                        bytes: bytes.into(),
                        metrics: None,
                    });
                }
            }
        }
    }
    let mandatory_notices = if notices.is_empty() {
        None
    } else {
        Some(notices.into_manifest().map_err(|_| BuildError {
            stage: crate::SvgFontSubsetStage::Encode,
            family: "",
            asset_identity: "mandatory-notices",
        })?)
    };
    Ok(Bundle {
        resources,
        mandatory_notices,
    })
}

pub(super) fn subset_notice(
    source: SubsetSource,
    family: &'static str,
    effective_alias: &str,
    semantic_identity: &str,
    derived: &font_subset::Derived,
) -> Result<SubsetNotice, BuildError> {
    let provenance = family_notice(source, family)?;
    let output_digest = encode_hex_32(&Sha256::digest(derived.woff2.as_ref()).into());
    let profile_digest = encode_hex_32(&crate::fonts::font_subset_profile_digest_v1());
    Ok(SubsetNotice {
        provenance,
        resource_identity: format!("svg-font-subset@sha256:{semantic_identity}"),
        resource_text: format!(
            "Derived font subset of {}. Source SHA-256: {}. {} Modified Version under {}. Transformation: Unicode and required layout/glyph closure subset; names rewritten for compatibility revision 1. Effective alias: {}. Subset profile SHA-256: {}. Semantic subset identity: {}. Output WOFF2: {} bytes, SHA-256 {}.",
            source.identity(),
            source.complete_sha256(),
            provenance.attribution,
            provenance.license_identity,
            effective_alias,
            profile_digest,
            semantic_identity,
            derived.woff2.len(),
            output_digest,
        ),
    })
}

/// The notice the catalog states for the source's family; a source without
/// one cannot be validated for subsetting.
fn family_notice(
    source: SubsetSource,
    family: &'static str,
) -> Result<&'static FontNotice, BuildError> {
    source.face.family().notice().ok_or(BuildError {
        stage: crate::SvgFontSubsetStage::Validate,
        family,
        asset_identity: source.identity(),
    })
}

/// The semantic subset identity of one official chunk's subset (spec 8.3.2),
/// as the qualification corpus and the fuzz regressions name it.
#[cfg(test)]
pub(super) fn semantic_identity(
    record: &'static crate::fonts::inspection::AssetRecord,
    scalars: &[u32],
) -> Option<String> {
    source_semantic_identity(SubsetSource::chunk(record), scalars)
}

/// The semantic subset identity of any source's subset: the spec 8.3.2
/// preimage over the source's catalog identity and complete-bytes digest,
/// which for the Noto Emoji fallback are its manifested path and sfnt digest.
pub(super) fn source_semantic_identity(source: SubsetSource, scalars: &[u32]) -> Option<String> {
    let profile_digest = crate::fonts::font_subset_profile_digest_v1();
    let source_digest = decode_hex_32(source.complete_sha256())?;
    let identity = source.identity();
    let mut hasher = Sha256::new();
    hasher.update(b"excalidraw-render:svg-font-subset:v1");
    hasher.update([0]);
    hasher.update(1_u64.to_le_bytes());
    hasher.update(profile_digest);
    hasher.update(u64::try_from(identity.len()).ok()?.to_le_bytes());
    hasher.update(identity.as_bytes());
    hasher.update(source_digest);
    hasher.update(u64::try_from(scalars.len()).ok()?.to_le_bytes());
    for scalar in scalars {
        hasher.update(scalar.to_le_bytes());
    }
    Some(encode_hex_32(&hasher.finalize().into()))
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut output = [0_u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        let offset = index.checked_mul(2)?;
        *byte = u8::from_str_radix(value.get(offset..offset.checked_add(2)?)?, 16).ok()?;
    }
    Some(output)
}

pub(super) fn encode_hex_32(value: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in value {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(all(test, not(feature = "cjk")))]
mod optional_font_tests {
    use super::*;

    #[test]
    fn semantic_source_demand_detects_unavailable_fonts_without_glyph_layout() {
        let demand = |scalars: &[u32]| Demand {
            families: vec![FamilyDemand::from_scalars("Xiaolai", scalars).unwrap()],
        };
        let mut session = Session::new();
        assert!(build(demand(&[0x10ffff]), Payload::Subset, &mut session).is_ok());
        let error = match build(demand(&[u32::from('中')]), Payload::Subset, &mut session) {
            Ok(_) => panic!("a selected unavailable source must fail"),
            Err(error) => error,
        };
        let super::super::FormatEncodeError::FontUnavailable(font) = error else {
            panic!("expected unavailable font, got {error:?}");
        };
        assert_eq!(font.resolved_family, "Xiaolai");
        assert!(crate::fonts::revision_1()
            .assets
            .iter()
            .any(|record| record.identity == font.asset_identity && !record.is_available()));
    }
}
