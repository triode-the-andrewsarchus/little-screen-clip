//! Structural validation of a generated subset's GSUB, GPOS and GDEF tables
//! (ADR-0020).
//!
//! Every glyph ID, lookup index, feature index, class value and offset the
//! tables carry must refer to something that exists; format 2 ranges must be
//! ordered; and counts must agree with the arrays they index. The walk covers
//! everything present, including lookups no feature reaches, because it
//! judges the subsetter's bytes rather than shaping. Only bundled,
//! digest-verified fonts reach it, so a lookup type or format the walker
//! cannot model fails closed like any parse error. That includes the 24-bit
//! glyph ID formats and wide-offset lists ISO/IEC 14496-22:2026 adds, which
//! read-fonts parses but no bundled font carries.
//!
//! Typed nesting has a fixed shape and nested lookups are referenced by index
//! rather than followed, so the walk needs no depth limit. Shared offsets can
//! still make it visit the same bytes repeatedly; [`MAX_LAYOUT_WORK`] bounds
//! that.

use write_fonts::read::tables::gdef::{CaretValue, Gdef, LigCaretList, SelectedLigCaretList};
use write_fonts::read::tables::gpos::{
    AnchorTable, CursivePos, DeviceOrVariationIndex, Gpos, MarkArray, MarkBasePos, MarkLigPos,
    MarkMarkPos, PairPos, PositionLookup, SinglePos, ValueRecord,
};
use write_fonts::read::tables::gsub::{
    AlternateSubst, Gsub, LigatureSubst, MultipleSubst, ReverseChainSingleSubst, SingleSubst,
    SubstitutionLookup,
};
use write_fonts::read::tables::layout::{
    ChainedSequenceContext, ClassDef, Condition, CoverageTable, Feature, FeatureList,
    FeatureParams, FeatureVariations, LangSys, Lookup, LookupList, ScriptList, SelectedLookupList,
    SequenceContext, SequenceLookupRecord,
};
use write_fonts::read::tables::variations::ItemVariationStore;
use write_fonts::read::types::{BigEndian, GlyphId16, Tag};
use write_fonts::read::{FontRead, FontRef, MinByteRange, ReadError, ResolveOffset, TableProvider};

/// The most records and array elements one font's layout walk may visit.
/// Offsets may be shared, so a small table can direct the walk over the same
/// subtable many times; the budget bounds that repeated work. It is not a
/// property of the specification.
pub(super) const MAX_LAYOUT_WORK: usize = 1_000_000;

/// The table a layout failure was found in.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(test, derive(serde::Deserialize), serde(rename_all = "kebab-case"))]
pub(super) enum LayoutTable {
    Maxp,
    Gsub,
    Gpos,
    Gdef,
}

/// The check a layout failure broke. This is internal to the renderer: the
/// public failure is always the `Validate` stage.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(test, derive(serde::Deserialize), serde(rename_all = "kebab-case"))]
pub(super) enum LayoutCheck {
    /// A table or the target of a non-null offset does not parse.
    Parse,
    /// A lookup type, subtable format or feature parameter kind the walker
    /// does not model.
    UnknownFormat,
    /// A glyph ID is not below `maxp.numGlyphs`.
    GlyphId,
    /// A format 2 range record ends before it starts.
    RangeOrder,
    /// A ClassDef format 1 array runs past the last glyph.
    ClassRange,
    /// A SingleSubst format 1 delta maps a covered glyph out of range.
    SingleSubstDelta,
    /// A lookup index is not below the lookup count.
    LookupIndex,
    /// A feature index is not below the feature count.
    FeatureIndex,
    /// A class value is not below the class count it indexes.
    ClassValue,
    /// A count disagrees with the array or coverage it describes.
    Count,
    /// A nested lookup record's sequence index is outside its input.
    SequenceIndex,
    /// The walk exceeded [`MAX_LAYOUT_WORK`].
    Budget,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[cfg_attr(test, derive(serde::Deserialize), serde(deny_unknown_fields))]
pub(super) struct LayoutFailure {
    pub(super) table: LayoutTable,
    pub(super) check: LayoutCheck,
}

type Walk<T = ()> = Result<T, LayoutFailure>;

/// Validates the layout tables of `font`, returning the work the walk used.
pub(super) fn validate(font: &FontRef<'_>) -> Walk<usize> {
    walk(font, &mut Walker::new()).map(|walker| MAX_LAYOUT_WORK - walker.budget)
}

/// [`validate`], returning the walker branches the walk took.
#[cfg(test)]
pub(super) fn validate_branches(
    font: &FontRef<'_>,
) -> Walk<std::collections::BTreeSet<&'static str>> {
    let mut walker = Walker::new();
    walk(font, &mut walker)?;
    Ok(walker.branches)
}

fn walk<'w>(font: &FontRef<'_>, walker: &'w mut Walker) -> Walk<&'w mut Walker> {
    let has = |tag: &[u8; 4]| font.data_for_tag(Tag::new(tag)).is_some();
    if !has(b"GSUB") && !has(b"GPOS") && !has(b"GDEF") {
        return Ok(walker);
    }
    walker.glyph_count = u32::from(walker.read(font.maxp())?.num_glyphs());
    if has(b"GSUB") {
        walker.table = LayoutTable::Gsub;
        let gsub = walker.read(font.gsub())?;
        walker.gsub(&gsub)?;
    }
    if has(b"GPOS") {
        walker.table = LayoutTable::Gpos;
        let gpos = walker.read(font.gpos())?;
        walker.gpos(&gpos)?;
    }
    if has(b"GDEF") {
        walker.table = LayoutTable::Gdef;
        let gdef = walker.read(font.gdef())?;
        walker.gdef(&gdef)?;
    }
    Ok(walker)
}

struct Walker {
    glyph_count: u32,
    budget: usize,
    table: LayoutTable,
    /// The current table's lookup and feature counts, which nested records
    /// index.
    lookup_count: u16,
    feature_count: u16,
    /// The walker branches this walk took, for the coverage tests.
    #[cfg(test)]
    branches: std::collections::BTreeSet<&'static str>,
}

impl Walker {
    fn new() -> Self {
        Self {
            glyph_count: 0,
            budget: MAX_LAYOUT_WORK,
            table: LayoutTable::Maxp,
            lookup_count: 0,
            feature_count: 0,
            #[cfg(test)]
            branches: std::collections::BTreeSet::new(),
        }
    }

    fn visit(&mut self, _branch: &'static str) {
        #[cfg(test)]
        self.branches.insert(_branch);
    }

    const fn fail(&self, check: LayoutCheck) -> LayoutFailure {
        LayoutFailure {
            table: self.table,
            check,
        }
    }

    fn ensure(&self, holds: bool, check: LayoutCheck) -> Walk {
        if holds {
            Ok(())
        } else {
            Err(self.fail(check))
        }
    }

    /// A format the walker does not model, which parsed.
    const fn unmodelled<T>(&self) -> Walk<T> {
        Err(self.fail(LayoutCheck::UnknownFormat))
    }

    fn read<T>(&self, result: Result<T, ReadError>) -> Walk<T> {
        result.map_err(|error| {
            self.fail(match error {
                ReadError::InvalidFormat(_) => LayoutCheck::UnknownFormat,
                _ => LayoutCheck::Parse,
            })
        })
    }

    /// Reads a nullable offset's target: absent is fine, unparsable is not.
    fn read_nullable<T>(&self, result: Option<Result<T, ReadError>>) -> Walk<Option<T>> {
        result.map(|result| self.read(result)).transpose()
    }

    /// Checks that every field of `table`, including arrays whose length a
    /// count gives, lies inside its data. read-fonts reads a truncated array
    /// as empty rather than failing, so this is where a count that overruns
    /// the table is caught.
    fn whole<'a>(&self, table: &impl MinByteRange<'a>) -> Walk {
        let range = table.min_byte_range();
        self.ensure(
            table.min_table_bytes().len() == range.len(),
            LayoutCheck::Count,
        )
    }

    fn table<'a, T: MinByteRange<'a>>(&self, result: Result<T, ReadError>) -> Walk<T> {
        let table = self.read(result)?;
        self.whole(&table)?;
        Ok(table)
    }

    fn nullable_table<'a, T: MinByteRange<'a>>(
        &self,
        result: Option<Result<T, ReadError>>,
    ) -> Walk<Option<T>> {
        result.map(|result| self.table(result)).transpose()
    }

    fn work(&mut self, units: usize) -> Walk {
        self.budget = self
            .budget
            .checked_sub(units)
            .ok_or(self.fail(LayoutCheck::Budget))?;
        Ok(())
    }

    fn glyph(&self, glyph: GlyphId16) -> Walk {
        self.ensure(glyph.to_u32() < self.glyph_count, LayoutCheck::GlyphId)
    }

    fn glyphs(&mut self, glyphs: &[BigEndian<GlyphId16>]) -> Walk {
        self.work(glyphs.len())?;
        glyphs.iter().try_for_each(|glyph| self.glyph(glyph.get()))
    }

    fn range(&self, start: GlyphId16, end: GlyphId16) -> Walk {
        self.glyph(start)?;
        self.glyph(end)?;
        self.ensure(start <= end, LayoutCheck::RangeOrder)
    }

    fn lookup_index(&self, index: u16) -> Walk {
        self.ensure(index < self.lookup_count, LayoutCheck::LookupIndex)
    }

    fn feature_index(&self, index: u16) -> Walk {
        self.ensure(index < self.feature_count, LayoutCheck::FeatureIndex)
    }

    /// Validates a coverage table and returns the number of coverage indices
    /// it assigns: the length an array indexed by it must have at least.
    fn coverage(&mut self, coverage: Result<CoverageTable<'_>, ReadError>) -> Walk<usize> {
        self.work(1)?;
        match self.table(coverage)? {
            CoverageTable::Format1(table) => {
                self.visit("coverage-1");
                self.glyphs(table.glyph_array())?;
                Ok(table.glyph_array().len())
            }
            CoverageTable::Format2(table) => {
                self.visit("coverage-2");
                self.work(table.range_records().len())?;
                let mut extent = 0;
                for record in table.range_records() {
                    self.range(record.start_glyph_id(), record.end_glyph_id())?;
                    let span = record.end_glyph_id().to_u16() - record.start_glyph_id().to_u16();
                    extent = extent
                        .max(usize::from(record.start_coverage_index()) + usize::from(span) + 1);
                }
                Ok(extent)
            }
            CoverageTable::Format3(_) | CoverageTable::Format4(_) => self.unmodelled(),
        }
    }

    /// Checks that a coverage-indexed array has an entry for every index.
    fn indexed(&self, coverage_extent: usize, len: usize) -> Walk {
        self.ensure(coverage_extent <= len, LayoutCheck::Count)
    }

    /// Validates a class definition and returns its largest class value.
    fn class_def(&mut self, class_def: Result<ClassDef<'_>, ReadError>) -> Walk<u16> {
        self.work(1)?;
        match self.table(class_def)? {
            ClassDef::Format1(table) => {
                self.visit("class-def-1");
                let values = table.class_value_array();
                self.work(values.len())?;
                self.glyph(table.start_glyph_id())?;
                let end = table.start_glyph_id().to_u32() + u32::from(table.glyph_count());
                self.ensure(end <= self.glyph_count, LayoutCheck::ClassRange)?;
                Ok(values.iter().map(BigEndian::get).max().unwrap_or(0))
            }
            ClassDef::Format2(table) => {
                self.visit("class-def-2");
                let records = table.class_range_records();
                self.work(records.len())?;
                let mut largest = 0;
                for record in records {
                    self.range(record.start_glyph_id(), record.end_glyph_id())?;
                    largest = largest.max(record.class());
                }
                Ok(largest)
            }
            ClassDef::Format3(_) | ClassDef::Format4(_) => self.unmodelled(),
        }
    }

    fn device(&mut self, device: Option<Result<DeviceOrVariationIndex<'_>, ReadError>>) -> Walk {
        self.work(1)?;
        match self.nullable_table(device)? {
            None => {}
            Some(DeviceOrVariationIndex::Device(_)) => self.visit("device"),
            Some(DeviceOrVariationIndex::VariationIndex(_)) => self.visit("variation-index"),
        }
        Ok(())
    }

    fn anchor(&mut self, anchor: Option<Result<AnchorTable<'_>, ReadError>>) -> Walk {
        self.work(1)?;
        match self.nullable_table(anchor)? {
            None => Ok(()),
            Some(AnchorTable::Format1(_)) => {
                self.visit("anchor-1");
                Ok(())
            }
            Some(AnchorTable::Format2(_)) => {
                self.visit("anchor-2");
                Ok(())
            }
            Some(AnchorTable::Format3(table)) => {
                self.visit("anchor-3");
                self.device(table.x_device())?;
                self.device(table.y_device())
            }
        }
    }

    fn value_record(&mut self, record: &ValueRecord<'_>) -> Walk {
        self.work(1)?;
        self.device(record.x_placement_device())?;
        self.device(record.y_placement_device())?;
        self.device(record.x_advance_device())?;
        self.device(record.y_advance_device())
    }

    fn script_list(&mut self, scripts: Result<ScriptList<'_>, ReadError>) -> Walk {
        self.work(1)?;
        let scripts = self.table(scripts)?;
        self.work(scripts.script_records().len())?;
        for record in scripts.script_records() {
            let script = self.table(record.script(scripts.offset_data()))?;
            if let Some(lang_sys) = self.nullable_table(script.default_lang_sys())? {
                self.lang_sys(&lang_sys)?;
            }
            self.work(script.lang_sys_records().len())?;
            for record in script.lang_sys_records() {
                let lang_sys = self.table(record.lang_sys(script.offset_data()))?;
                self.lang_sys(&lang_sys)?;
            }
        }
        Ok(())
    }

    fn lang_sys(&mut self, lang_sys: &LangSys<'_>) -> Walk {
        self.work(1 + lang_sys.feature_indices().len())?;
        if lang_sys.required_feature_index() != 0xffff {
            self.feature_index(lang_sys.required_feature_index())?;
        }
        lang_sys
            .feature_indices()
            .iter()
            .try_for_each(|index| self.feature_index(index.get()))
    }

    fn feature_list(&mut self, features: &FeatureList<'_>) -> Walk {
        self.work(1 + features.feature_records().len())?;
        for record in features.feature_records() {
            let feature = self.table(record.feature(features.offset_data()))?;
            self.feature(&feature)?;
        }
        Ok(())
    }

    fn feature(&mut self, feature: &Feature<'_>) -> Walk {
        self.work(1 + feature.lookup_list_indices().len())?;
        match self.read_nullable(feature.feature_params())? {
            None => {}
            Some(FeatureParams::Size(params)) => {
                self.visit("feature-params-size");
                self.whole(&params)?;
            }
            Some(FeatureParams::StylisticSet(params)) => {
                self.visit("feature-params-stylistic-set");
                self.whole(&params)?;
            }
            Some(FeatureParams::CharacterVariant(params)) => {
                self.visit("feature-params-character-variant");
                self.whole(&params)?;
                self.work(params.character().len())?;
            }
        }
        feature
            .lookup_list_indices()
            .iter()
            .try_for_each(|index| self.lookup_index(index.get()))
    }

    fn feature_variations(
        &mut self,
        features: &FeatureList<'_>,
        variations: Option<Result<FeatureVariations<'_>, ReadError>>,
    ) -> Walk {
        let Some(variations) = self.nullable_table(variations)? else {
            return Ok(());
        };
        self.visit("feature-variations");
        let records = variations.feature_variation_records();
        self.work(1 + records.len())?;
        for record in records {
            if let Some(conditions) =
                self.nullable_table(record.condition_set(variations.offset_data()))?
            {
                self.work(1 + conditions.conditions().len())?;
                for condition in conditions.conditions().iter() {
                    let condition = self.table(condition)?;
                    self.condition(&condition)?;
                }
            }
            if let Some(substitutions) =
                self.nullable_table(record.feature_table_substitution(variations.offset_data()))?
            {
                self.work(1 + substitutions.substitutions().len())?;
                for substitution in substitutions.substitutions() {
                    self.feature_index(substitution.feature_index())?;
                    // read-fonts' `alternate_feature` reads FeatureParams as
                    // tag `NULL`, which rejects any parameters. An alternate
                    // feature's parameters take the form of the feature it
                    // replaces.
                    let tag = features.feature_records()[usize::from(substitution.feature_index())]
                        .feature_tag();
                    let feature = self.table(
                        substitution
                            .alternate_feature_offset()
                            .resolve_with_args(substitutions.offset_data(), tag),
                    )?;
                    self.feature(&feature)?;
                }
            }
        }
        Ok(())
    }

    fn condition(&mut self, condition: &Condition<'_>) -> Walk {
        self.work(1)?;
        match condition {
            Condition::Format1AxisRange(_) => {
                self.visit("condition-1");
                Ok(())
            }
            Condition::Format2VariableValue(_) => {
                self.visit("condition-2");
                Ok(())
            }
            Condition::Format3And(table) => {
                self.visit("condition-3");
                self.conditions(table.conditions().iter())
            }
            Condition::Format4Or(table) => {
                self.visit("condition-4");
                self.conditions(table.conditions().iter())
            }
            Condition::Format5Negate(table) => {
                self.visit("condition-5");
                let condition = self.table(table.condition())?;
                self.condition(&condition)
            }
        }
    }

    fn conditions<'a>(
        &mut self,
        conditions: impl Iterator<Item = Result<Condition<'a>, ReadError>>,
    ) -> Walk {
        for condition in conditions {
            let condition = self.table(condition)?;
            self.condition(&condition)?;
        }
        Ok(())
    }

    /// The lookup header: every subtable is walked by the caller.
    fn lookup<'a, T: FontRead<'a, Args = ()>>(&mut self, lookup: &Lookup<'a, T>) -> Walk {
        self.whole(lookup)?;
        self.work(1 + lookup.subtable_offsets().len())
    }

    /// The lookup list a GSUB or GPOS header selects. A LookupList2, with
    /// 32-bit offsets, is not modelled.
    fn lookup_list<'a, T: FontRead<'a, Args = ()>>(
        &self,
        list: Result<SelectedLookupList<'a, T>, ReadError>,
    ) -> Walk<LookupList<'a, T>> {
        match self.read(list)? {
            SelectedLookupList::Original(list) => {
                self.whole(&list)?;
                Ok(list)
            }
            SelectedLookupList::Extended(_) => self.unmodelled(),
        }
    }

    /// Nested lookup records of a contextual rule whose input is `input`
    /// glyphs long, the first included.
    fn lookup_records(&mut self, records: &[SequenceLookupRecord], input: usize) -> Walk {
        self.work(records.len())?;
        for record in records {
            self.ensure(
                usize::from(record.sequence_index()) < input,
                LayoutCheck::SequenceIndex,
            )?;
            self.lookup_index(record.lookup_list_index())?;
        }
        Ok(())
    }

    fn sequence_context(&mut self, context: Result<SequenceContext<'_>, ReadError>) -> Walk {
        self.work(1)?;
        match self.table(context)? {
            SequenceContext::Format1(table) => {
                self.visit("context-1");
                let extent = self.coverage(table.coverage())?;
                self.indexed(extent, table.seq_rule_sets().len())?;
                self.work(table.seq_rule_sets().len())?;
                for rule_set in table.seq_rule_sets().iter() {
                    let Some(rule_set) = self.nullable_table(rule_set)? else {
                        continue;
                    };
                    self.work(1 + rule_set.seq_rules().len())?;
                    for rule in rule_set.seq_rules().iter() {
                        let rule = self.table(rule)?;
                        self.glyphs(rule.input_sequence())?;
                        self.lookup_records(
                            rule.seq_lookup_records(),
                            usize::from(rule.glyph_count()),
                        )?;
                    }
                }
            }
            SequenceContext::Format2(table) => {
                self.visit("context-2");
                self.coverage(table.coverage())?;
                self.class_def(table.class_def())?;
                self.work(table.class_seq_rule_sets().len())?;
                for rule_set in table.class_seq_rule_sets().iter() {
                    let Some(rule_set) = self.nullable_table(rule_set)? else {
                        continue;
                    };
                    self.work(1 + rule_set.class_seq_rules().len())?;
                    for rule in rule_set.class_seq_rules().iter() {
                        let rule = self.table(rule)?;
                        self.work(rule.input_sequence().len())?;
                        self.lookup_records(
                            rule.seq_lookup_records(),
                            usize::from(rule.glyph_count()),
                        )?;
                    }
                }
            }
            SequenceContext::Format3(table) => {
                self.visit("context-3");
                self.work(table.coverages().len())?;
                for coverage in table.coverages().iter() {
                    self.coverage(coverage)?;
                }
                self.lookup_records(table.seq_lookup_records(), table.coverages().len())?;
            }
            SequenceContext::Format4(_)
            | SequenceContext::Format5(_)
            | SequenceContext::Format6(_) => return self.unmodelled(),
        }
        Ok(())
    }

    fn chained_context(&mut self, context: Result<ChainedSequenceContext<'_>, ReadError>) -> Walk {
        self.work(1)?;
        match self.table(context)? {
            ChainedSequenceContext::Format1(table) => {
                self.visit("chained-context-1");
                let extent = self.coverage(table.coverage())?;
                self.indexed(extent, table.chained_seq_rule_sets().len())?;
                self.work(table.chained_seq_rule_sets().len())?;
                for rule_set in table.chained_seq_rule_sets().iter() {
                    let Some(rule_set) = self.nullable_table(rule_set)? else {
                        continue;
                    };
                    self.work(1 + rule_set.chained_seq_rules().len())?;
                    for rule in rule_set.chained_seq_rules().iter() {
                        let rule = self.table(rule)?;
                        self.glyphs(rule.backtrack_sequence())?;
                        self.glyphs(rule.input_sequence())?;
                        self.glyphs(rule.lookahead_sequence())?;
                        self.lookup_records(
                            rule.seq_lookup_records(),
                            usize::from(rule.input_glyph_count()),
                        )?;
                    }
                }
            }
            ChainedSequenceContext::Format2(table) => {
                self.visit("chained-context-2");
                self.coverage(table.coverage())?;
                self.class_def(table.backtrack_class_def())?;
                self.class_def(table.input_class_def())?;
                self.class_def(table.lookahead_class_def())?;
                self.work(table.chained_class_seq_rule_sets().len())?;
                for rule_set in table.chained_class_seq_rule_sets().iter() {
                    let Some(rule_set) = self.nullable_table(rule_set)? else {
                        continue;
                    };
                    self.work(1 + rule_set.chained_class_seq_rules().len())?;
                    for rule in rule_set.chained_class_seq_rules().iter() {
                        let rule = self.table(rule)?;
                        self.work(
                            rule.backtrack_sequence().len()
                                + rule.input_sequence().len()
                                + rule.lookahead_sequence().len(),
                        )?;
                        self.lookup_records(
                            rule.seq_lookup_records(),
                            usize::from(rule.input_glyph_count()),
                        )?;
                    }
                }
            }
            ChainedSequenceContext::Format3(table) => {
                self.visit("chained-context-3");
                let coverages = [
                    table.backtrack_coverages(),
                    table.input_coverages(),
                    table.lookahead_coverages(),
                ];
                for coverages in &coverages {
                    self.work(coverages.len())?;
                    for coverage in coverages.iter() {
                        self.coverage(coverage)?;
                    }
                }
                self.lookup_records(table.seq_lookup_records(), table.input_coverages().len())?;
            }
            ChainedSequenceContext::Format4(_) | ChainedSequenceContext::Format5(_) => {
                return self.unmodelled();
            }
        }
        Ok(())
    }

    fn gsub(&mut self, gsub: &Gsub<'_>) -> Walk {
        self.work(1)?;
        self.whole(gsub)?;
        let lookups = self.lookup_list(gsub.lookup_list())?;
        let features = self.table(gsub.feature_list())?;
        self.lookup_count = lookups.lookup_count();
        self.feature_count = features.feature_count();
        self.script_list(gsub.script_list())?;
        self.feature_list(&features)?;
        self.feature_variations(&features, gsub.feature_variations())?;
        self.work(1 + lookups.lookups().len())?;
        for lookup in lookups.lookups().iter() {
            match self.read(lookup)? {
                SubstitutionLookup::Single(lookup) => {
                    self.visit("gsub-1");
                    self.lookup(&lookup)?;
                    lookup
                        .subtables()
                        .iter()
                        .try_for_each(|subtable| self.single_subst(subtable))?;
                }
                SubstitutionLookup::Multiple(lookup) => {
                    self.visit("gsub-2");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.multiple_subst(subtable)?;
                    }
                }
                SubstitutionLookup::Alternate(lookup) => {
                    self.visit("gsub-3");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.alternate_subst(subtable)?;
                    }
                }
                SubstitutionLookup::Ligature(lookup) => {
                    self.visit("gsub-4");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.ligature_subst(subtable)?;
                    }
                }
                SubstitutionLookup::Contextual(lookup) => {
                    self.visit("gsub-5");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.sequence_context(subtable)?;
                    }
                }
                SubstitutionLookup::ChainContextual(lookup) => {
                    self.visit("gsub-6");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.chained_context(subtable)?;
                    }
                }
                SubstitutionLookup::Reverse(lookup) => {
                    self.visit("gsub-8");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.reverse_chain_subst(subtable)?;
                    }
                }
                SubstitutionLookup::Extension(lookup) => {
                    self.visit("gsub-7");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.gsub_extension(subtable)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn gsub_extension(
        &mut self,
        extension: Result<write_fonts::read::tables::gsub::ExtensionSubtable<'_>, ReadError>,
    ) -> Walk {
        use write_fonts::read::tables::gsub::ExtensionSubtable;
        self.work(1)?;
        match self.read(extension)? {
            ExtensionSubtable::Single(table) => {
                self.whole(&table)?;
                self.single_subst(table.extension())
            }
            ExtensionSubtable::Multiple(table) => {
                self.whole(&table)?;
                self.multiple_subst(table.extension())
            }
            ExtensionSubtable::Alternate(table) => {
                self.whole(&table)?;
                self.alternate_subst(table.extension())
            }
            ExtensionSubtable::Ligature(table) => {
                self.whole(&table)?;
                self.ligature_subst(table.extension())
            }
            ExtensionSubtable::Contextual(table) => {
                self.whole(&table)?;
                self.sequence_context(table.extension())
            }
            ExtensionSubtable::ChainContextual(table) => {
                self.whole(&table)?;
                self.chained_context(table.extension())
            }
            ExtensionSubtable::Reverse(table) => {
                self.whole(&table)?;
                self.reverse_chain_subst(table.extension())
            }
        }
    }

    fn single_subst(&mut self, subtable: Result<SingleSubst<'_>, ReadError>) -> Walk {
        self.work(1)?;
        match self.table(subtable)? {
            SingleSubst::Format1(table) => {
                self.visit("gsub-1-format-1");
                let coverage = self.table(table.coverage())?;
                self.coverage(Ok(coverage.clone()))?;
                let delta = i64::from(table.delta_glyph_id());
                for glyph in coverage.iter() {
                    self.work(1)?;
                    let substitute = (i64::from(glyph.to_u32()) + delta) & 0xffff;
                    self.ensure(
                        substitute < i64::from(self.glyph_count),
                        LayoutCheck::SingleSubstDelta,
                    )?;
                }
            }
            SingleSubst::Format2(table) => {
                self.visit("gsub-1-format-2");
                let extent = self.coverage(table.coverage())?;
                self.indexed(extent, table.substitute_glyph_ids().len())?;
                self.glyphs(table.substitute_glyph_ids())?;
            }
            SingleSubst::Format3(_) | SingleSubst::Format4(_) => return self.unmodelled(),
        }
        Ok(())
    }

    fn multiple_subst(&mut self, subtable: Result<MultipleSubst<'_>, ReadError>) -> Walk {
        self.work(1)?;
        let MultipleSubst::Format1(table) = self.table(subtable)? else {
            return self.unmodelled();
        };
        let extent = self.coverage(table.coverage())?;
        self.indexed(extent, table.sequences().len())?;
        self.work(table.sequences().len())?;
        for sequence in table.sequences().iter() {
            let sequence = self.table(sequence)?;
            self.glyphs(sequence.substitute_glyph_ids())?;
        }
        Ok(())
    }

    fn alternate_subst(&mut self, subtable: Result<AlternateSubst<'_>, ReadError>) -> Walk {
        self.work(1)?;
        let AlternateSubst::Format1(table) = self.table(subtable)? else {
            return self.unmodelled();
        };
        let extent = self.coverage(table.coverage())?;
        self.indexed(extent, table.alternate_sets().len())?;
        self.work(table.alternate_sets().len())?;
        for set in table.alternate_sets().iter() {
            let set = self.table(set)?;
            self.glyphs(set.alternate_glyph_ids())?;
        }
        Ok(())
    }

    fn ligature_subst(&mut self, subtable: Result<LigatureSubst<'_>, ReadError>) -> Walk {
        self.work(1)?;
        let LigatureSubst::Format1(table) = self.table(subtable)? else {
            return self.unmodelled();
        };
        let extent = self.coverage(table.coverage())?;
        self.indexed(extent, table.ligature_sets().len())?;
        self.work(table.ligature_sets().len())?;
        for set in table.ligature_sets().iter() {
            let set = self.table(set)?;
            self.work(1 + set.ligatures().len())?;
            for ligature in set.ligatures().iter() {
                let ligature = self.table(ligature)?;
                self.ensure(ligature.component_count() >= 1, LayoutCheck::Count)?;
                self.glyph(ligature.ligature_glyph())?;
                self.glyphs(ligature.component_glyph_ids())?;
            }
        }
        Ok(())
    }

    fn reverse_chain_subst(
        &mut self,
        subtable: Result<ReverseChainSingleSubst<'_>, ReadError>,
    ) -> Walk {
        self.work(1)?;
        let ReverseChainSingleSubst::Format1(table) = self.table(subtable)? else {
            return self.unmodelled();
        };
        let extent = self.coverage(table.coverage())?;
        self.indexed(extent, table.substitute_glyph_ids().len())?;
        for coverages in [table.backtrack_coverages(), table.lookahead_coverages()] {
            self.work(coverages.len())?;
            for coverage in coverages.iter() {
                self.coverage(coverage)?;
            }
        }
        self.glyphs(table.substitute_glyph_ids())
    }

    fn gpos(&mut self, gpos: &Gpos<'_>) -> Walk {
        self.work(1)?;
        self.whole(gpos)?;
        let lookups = self.lookup_list(gpos.lookup_list())?;
        let features = self.table(gpos.feature_list())?;
        self.lookup_count = lookups.lookup_count();
        self.feature_count = features.feature_count();
        self.script_list(gpos.script_list())?;
        self.feature_list(&features)?;
        self.feature_variations(&features, gpos.feature_variations())?;
        self.work(1 + lookups.lookups().len())?;
        for lookup in lookups.lookups().iter() {
            match self.read(lookup)? {
                PositionLookup::Single(lookup) => {
                    self.visit("gpos-1");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.single_pos(subtable)?;
                    }
                }
                PositionLookup::Pair(lookup) => {
                    self.visit("gpos-2");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.pair_pos(subtable)?;
                    }
                }
                PositionLookup::Cursive(lookup) => {
                    self.visit("gpos-3");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.cursive_pos(subtable)?;
                    }
                }
                PositionLookup::MarkToBase(lookup) => {
                    self.visit("gpos-4");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.mark_base_pos(subtable)?;
                    }
                }
                PositionLookup::MarkToLig(lookup) => {
                    self.visit("gpos-5");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.mark_lig_pos(subtable)?;
                    }
                }
                PositionLookup::MarkToMark(lookup) => {
                    self.visit("gpos-6");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.mark_mark_pos(subtable)?;
                    }
                }
                PositionLookup::Contextual(lookup) => {
                    self.visit("gpos-7");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.sequence_context(subtable)?;
                    }
                }
                PositionLookup::ChainContextual(lookup) => {
                    self.visit("gpos-8");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.chained_context(subtable)?;
                    }
                }
                PositionLookup::Extension(lookup) => {
                    self.visit("gpos-9");
                    self.lookup(&lookup)?;
                    for subtable in lookup.subtables().iter() {
                        self.gpos_extension(subtable)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn gpos_extension(
        &mut self,
        extension: Result<write_fonts::read::tables::gpos::ExtensionSubtable<'_>, ReadError>,
    ) -> Walk {
        use write_fonts::read::tables::gpos::ExtensionSubtable;
        self.work(1)?;
        match self.read(extension)? {
            ExtensionSubtable::Single(table) => {
                self.whole(&table)?;
                self.single_pos(table.extension())
            }
            ExtensionSubtable::Pair(table) => {
                self.whole(&table)?;
                self.pair_pos(table.extension())
            }
            ExtensionSubtable::Cursive(table) => {
                self.whole(&table)?;
                self.cursive_pos(table.extension())
            }
            ExtensionSubtable::MarkToBase(table) => {
                self.whole(&table)?;
                self.mark_base_pos(table.extension())
            }
            ExtensionSubtable::MarkToLig(table) => {
                self.whole(&table)?;
                self.mark_lig_pos(table.extension())
            }
            ExtensionSubtable::MarkToMark(table) => {
                self.whole(&table)?;
                self.mark_mark_pos(table.extension())
            }
            ExtensionSubtable::Contextual(table) => {
                self.whole(&table)?;
                self.sequence_context(table.extension())
            }
            ExtensionSubtable::ChainContextual(table) => {
                self.whole(&table)?;
                self.chained_context(table.extension())
            }
        }
    }

    fn single_pos(&mut self, subtable: Result<SinglePos<'_>, ReadError>) -> Walk {
        self.work(1)?;
        match self.table(subtable)? {
            SinglePos::Format1(table) => {
                self.visit("gpos-1-format-1");
                self.coverage(table.coverage())?;
                self.value_record(&table.value_record())
            }
            SinglePos::Format2(table) => {
                self.visit("gpos-1-format-2");
                let extent = self.coverage(table.coverage())?;
                let records = table.value_records();
                self.indexed(extent, records.len())?;
                self.work(records.len())?;
                for record in records.iter() {
                    let record = self.read(record)?;
                    self.value_record(&record)?;
                }
                Ok(())
            }
            SinglePos::Format3(_) | SinglePos::Format4(_) => self.unmodelled(),
        }
    }

    fn pair_pos(&mut self, subtable: Result<PairPos<'_>, ReadError>) -> Walk {
        self.work(1)?;
        match self.table(subtable)? {
            PairPos::Format1(table) => {
                self.visit("gpos-2-format-1");
                let extent = self.coverage(table.coverage())?;
                self.indexed(extent, table.pair_sets().len())?;
                self.work(table.pair_sets().len())?;
                for set in table.pair_sets().iter() {
                    let set = self.table(set)?;
                    let records = set.pair_value_records();
                    self.work(records.len())?;
                    for record in records.iter() {
                        let record = self.read(record)?;
                        self.glyph(record.second_glyph())?;
                        self.value_record(record.value_record1())?;
                        self.value_record(record.value_record2())?;
                    }
                }
            }
            PairPos::Format2(table) => {
                self.visit("gpos-2-format-2");
                self.coverage(table.coverage())?;
                let class1 = self.class_def(table.class_def1())?;
                let class2 = self.class_def(table.class_def2())?;
                self.ensure(
                    class1 < table.class1_count() && class2 < table.class2_count(),
                    LayoutCheck::ClassValue,
                )?;
                let rows = table.class1_records();
                self.work(rows.len())?;
                for row in rows.iter() {
                    let row = self.read(row)?;
                    self.work(row.class2_records().len())?;
                    for cell in row.class2_records().iter() {
                        let cell = self.read(cell)?;
                        self.value_record(cell.value_record1())?;
                        self.value_record(cell.value_record2())?;
                    }
                }
            }
            PairPos::Format3(_) | PairPos::Format4(_) => return self.unmodelled(),
        }
        Ok(())
    }

    fn cursive_pos(&mut self, subtable: Result<CursivePos<'_>, ReadError>) -> Walk {
        self.work(1)?;
        let CursivePos::Format1(table) = self.table(subtable)? else {
            return self.unmodelled();
        };
        let extent = self.coverage(table.coverage())?;
        let records = table.entry_exit_record();
        self.indexed(extent, records.len())?;
        self.work(records.len())?;
        for record in records {
            self.anchor(record.entry_anchor(table.offset_data()))?;
            self.anchor(record.exit_anchor(table.offset_data()))?;
        }
        Ok(())
    }

    /// A mark array whose marks `mark_coverage` covers, with classes below
    /// `class_count`.
    fn mark_array(
        &mut self,
        marks: Result<MarkArray<'_>, ReadError>,
        mark_extent: usize,
        class_count: u16,
    ) -> Walk {
        self.work(1)?;
        let marks = self.table(marks)?;
        let records = marks.mark_records();
        self.indexed(mark_extent, records.len())?;
        self.work(records.len())?;
        for record in records {
            self.ensure(record.mark_class() < class_count, LayoutCheck::ClassValue)?;
            self.anchor(Some(record.mark_anchor(marks.offset_data())))?;
        }
        Ok(())
    }

    fn anchors<'a>(
        &mut self,
        anchors: impl Iterator<Item = Option<Result<AnchorTable<'a>, ReadError>>>,
    ) -> Walk {
        for anchor in anchors {
            self.anchor(anchor)?;
        }
        Ok(())
    }

    fn mark_base_pos(&mut self, subtable: Result<MarkBasePos<'_>, ReadError>) -> Walk {
        self.work(1)?;
        let MarkBasePos::Format1(table) = self.table(subtable)? else {
            return self.unmodelled();
        };
        let marks = self.coverage(table.mark_coverage())?;
        let bases = self.coverage(table.base_coverage())?;
        self.mark_array(table.mark_array(), marks, table.mark_class_count())?;
        let base_array = self.table(table.base_array())?;
        let records = base_array.base_records();
        self.indexed(bases, records.len())?;
        self.work(1 + records.len())?;
        for record in records.iter() {
            let record = self.read(record)?;
            self.work(record.base_anchor_offsets().len())?;
            self.anchors(record.base_anchors(base_array.offset_data()).iter())?;
        }
        Ok(())
    }

    fn mark_lig_pos(&mut self, subtable: Result<MarkLigPos<'_>, ReadError>) -> Walk {
        self.work(1)?;
        let MarkLigPos::Format1(table) = self.table(subtable)? else {
            return self.unmodelled();
        };
        let marks = self.coverage(table.mark_coverage())?;
        let ligatures = self.coverage(table.ligature_coverage())?;
        self.mark_array(table.mark_array(), marks, table.mark_class_count())?;
        let ligature_array = self.table(table.ligature_array())?;
        let attaches = ligature_array.ligature_attaches();
        self.indexed(ligatures, attaches.len())?;
        self.work(1 + attaches.len())?;
        for attach in attaches.iter() {
            let attach = self.table(attach)?;
            let components = attach.component_records();
            self.work(1 + components.len())?;
            for component in components.iter() {
                let component = self.read(component)?;
                self.work(component.ligature_anchor_offsets().len())?;
                self.anchors(component.ligature_anchors(attach.offset_data()).iter())?;
            }
        }
        Ok(())
    }

    fn mark_mark_pos(&mut self, subtable: Result<MarkMarkPos<'_>, ReadError>) -> Walk {
        self.work(1)?;
        let MarkMarkPos::Format1(table) = self.table(subtable)? else {
            return self.unmodelled();
        };
        let marks = self.coverage(table.mark1_coverage())?;
        let bases = self.coverage(table.mark2_coverage())?;
        self.mark_array(table.mark1_array(), marks, table.mark_class_count())?;
        let mark2_array = self.table(table.mark2_array())?;
        let records = mark2_array.mark2_records();
        self.indexed(bases, records.len())?;
        self.work(1 + records.len())?;
        for record in records.iter() {
            let record = self.read(record)?;
            self.work(record.mark2_anchor_offsets().len())?;
            self.anchors(record.mark2_anchors(mark2_array.offset_data()).iter())?;
        }
        Ok(())
    }

    fn gdef(&mut self, gdef: &Gdef<'_>) -> Walk {
        self.work(1)?;
        self.whole(gdef)?;
        if let Some(class_def) = gdef.glyph_class_def() {
            self.visit("gdef-glyph-class-def");
            self.class_def(class_def)?;
        }
        if let Some(attach_list) = self.nullable_table(gdef.attach_list())? {
            self.visit("gdef-attach-list");
            let extent = self.coverage(attach_list.coverage())?;
            self.indexed(extent, attach_list.attach_points().len())?;
            self.work(1 + attach_list.attach_points().len())?;
            for points in attach_list.attach_points().iter() {
                let points = self.table(points)?;
                self.work(points.point_indices().len())?;
            }
        }
        if let Some(carets) = self.read_nullable(gdef.lig_caret_list())? {
            self.visit("gdef-lig-caret-list");
            let carets = self.lig_caret_list(carets)?;
            let extent = self.coverage(carets.coverage())?;
            self.indexed(extent, carets.lig_glyphs().len())?;
            self.work(1 + carets.lig_glyphs().len())?;
            for ligature in carets.lig_glyphs().iter() {
                let ligature = self.table(ligature)?;
                self.work(1 + ligature.caret_values().len())?;
                for caret in ligature.caret_values().iter() {
                    match self.table(caret)? {
                        CaretValue::Format1(_) => self.visit("caret-value-1"),
                        CaretValue::Format2(_) => self.visit("caret-value-2"),
                        CaretValue::Format3(caret) => {
                            self.visit("caret-value-3");
                            self.device(Some(caret.device()))?;
                        }
                    }
                }
            }
        }
        if let Some(class_def) = gdef.mark_attach_class_def() {
            self.visit("gdef-mark-attach-class-def");
            self.class_def(class_def)?;
        }
        if let Some(sets) = self.nullable_table(gdef.mark_glyph_sets_def())? {
            self.visit("gdef-mark-glyph-sets");
            self.work(1 + sets.coverages().len())?;
            for coverage in sets.coverages().iter() {
                self.coverage(coverage)?;
            }
        }
        if let Some(store) = self.nullable_table(gdef.item_var_store())? {
            self.visit("gdef-item-variation-store");
            self.item_variation_store(&store)?;
        }
        Ok(())
    }

    /// The ligature caret list a GDEF header selects. A LigCaretList2, with
    /// 24-bit offsets, is not modelled.
    fn lig_caret_list<'a>(&self, carets: SelectedLigCaretList<'a>) -> Walk<LigCaretList<'a>> {
        match carets {
            SelectedLigCaretList::Original(carets) => {
                self.whole(&carets)?;
                Ok(carets)
            }
            SelectedLigCaretList::Extended(_) => self.unmodelled(),
        }
    }

    fn item_variation_store(&mut self, store: &ItemVariationStore<'_>) -> Walk {
        self.work(1)?;
        let regions = self.table(store.variation_region_list())?;
        self.work(1 + regions.variation_regions().len())?;
        for region in regions.variation_regions().iter() {
            let region = self.read(region)?;
            self.work(region.region_axes().len())?;
        }
        self.work(store.item_variation_data_offsets().len())?;
        for data in store.item_variation_data().iter() {
            if let Some(data) = self.nullable_table(data)? {
                self.work(1 + data.region_indexes().len())?;
            }
        }
        Ok(())
    }
}
