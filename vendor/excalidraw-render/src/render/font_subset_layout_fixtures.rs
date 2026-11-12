//! Structurally valid GSUB, GPOS and GDEF tables for the OpenType layout
//! formats no bundled font uses (#210, ADR-0020), built with write-fonts.
//!
//! The bundled catalog has no GSUB type 7 or 8 lookups; no GPOS type 1, 3, 7,
//! 8 or 9 lookups; no context format 1 or 2; no anchor or caret value format 2
//! or 3; no Device or VariationIndex; no FeatureVariations or FeatureParams;
//! and no GDEF AttachList or ItemVarStore. Each fixture here reaches at least
//! one of them, so the layout walker has a branch exercised for every format
//! the specification defines. Every fixture refers only to glyphs 0 to 3.

use write_fonts::tables::gdef::{
    AttachList, AttachPoint, CaretValue, Gdef, LigCaretList, LigGlyph, MarkGlyphSets,
};
use write_fonts::tables::gpos::{
    self, AnchorTable, BaseArray, BaseRecord, Class1Record, Class2Record, ComponentRecord,
    CursivePosFormat1, EntryExitRecord, ExtensionPosFormat1, Gpos, LigatureArray, LigatureAttach,
    Mark2Array, Mark2Record, MarkArray, MarkBasePosFormat1, MarkLigPosFormat1, MarkMarkPosFormat1,
    MarkRecord, PairPos, PairSet, PairValueRecord, PositionChainContext, PositionLookup,
    PositionSequenceContext, SinglePos, ValueRecord,
};
use write_fonts::tables::gsub::{
    self, AlternateSet, AlternateSubstFormat1, ExtensionSubstFormat1, Gsub, Ligature, LigatureSet,
    LigatureSubstFormat1, MultipleSubstFormat1, ReverseChainSingleSubstFormat1, Sequence,
    SingleSubst, SubstitutionChainContext, SubstitutionLookup, SubstitutionSequenceContext,
};
use write_fonts::tables::layout::{
    ChainedClassSequenceRule, ChainedClassSequenceRuleSet, ChainedSequenceContext,
    ChainedSequenceRule, ChainedSequenceRuleSet, CharacterVariantParams, ClassDef,
    ClassRangeRecord, ClassSequenceRule, ClassSequenceRuleSet, Condition, ConditionSet,
    CoverageTable, DeviceOrVariationIndex, Feature, FeatureList, FeatureParams, FeatureRecord,
    FeatureTableSubstitution, FeatureTableSubstitutionRecord, FeatureVariationRecord,
    FeatureVariations, LangSys, LangSysRecord, Lookup, LookupFlag, LookupList, RangeRecord, Script,
    ScriptList, ScriptRecord, SequenceContext, SequenceLookupRecord, SequenceRule, SequenceRuleSet,
    SizeParams, StylisticSetParams, VariationIndex,
};
use write_fonts::tables::variations::{
    ItemVariationData, ItemVariationStore, RegionAxisCoordinates, VariationRegion,
    VariationRegionList,
};
use write_fonts::types::{F2Dot14, GlyphId16, NameId, Tag, Uint24};
use write_fonts::FontBuilder;

/// The fewest glyphs a base font needs for every fixture to be in range.
pub(super) const MIN_GLYPHS: u16 = 4;

/// A font holding only `maxp` with [`MIN_GLYPHS`] glyphs: all the layout
/// walker reads besides the tables a fixture installs.
pub(super) fn maxp_base() -> Vec<u8> {
    let mut builder = FontBuilder::new();
    builder
        .add_table(&write_fonts::tables::maxp::Maxp::new(MIN_GLYPHS))
        .expect("maxp fixture compiles");
    builder.build()
}

/// One fixture: the layout tables it installs, each replacing the base font's
/// table of the same tag.
pub(super) struct LayoutFixture {
    pub(super) name: &'static str,
    pub(super) gsub: Option<Vec<u8>>,
    pub(super) gpos: Option<Vec<u8>>,
    pub(super) gdef: Option<Vec<u8>>,
}

impl LayoutFixture {
    /// `base` with this fixture's tables installed.
    pub(super) fn install(&self, base: &[u8]) -> Vec<u8> {
        let base = write_fonts::read::FontRef::new(base).expect("fixture base is an sfnt");
        let mut builder = FontBuilder::new();
        for (tag, table) in [
            (Tag::new(b"GSUB"), &self.gsub),
            (Tag::new(b"GPOS"), &self.gpos),
            (Tag::new(b"GDEF"), &self.gdef),
        ] {
            if let Some(table) = table {
                builder.add_raw(tag, table.clone());
            }
        }
        builder.copy_missing_tables(base);
        builder.build()
    }
}

pub(super) fn layout_fixtures() -> Vec<LayoutFixture> {
    let mut fixtures = Vec::new();
    for (name, lookup) in gsub_lookups() {
        fixtures.push(LayoutFixture {
            name,
            gsub: Some(dump(&gsub_table(vec![lookup], None, Vec::new()))),
            gpos: None,
            gdef: None,
        });
    }
    for (name, lookup) in gpos_lookups() {
        fixtures.push(LayoutFixture {
            name,
            gsub: None,
            gpos: Some(dump(&gpos_table(vec![lookup]))),
            gdef: Some(dump(&variation_gdef())),
        });
    }
    fixtures.push(LayoutFixture {
        name: "gsub-feature-params",
        gsub: Some(dump(&gsub_table(
            vec![single_lookup()],
            None,
            vec![
                (
                    Tag::new(b"size"),
                    FeatureParams::Size(SizeParams::new(100, 1, 256, 80, 120)),
                ),
                (Tag::new(b"ss01"), stylistic_set()),
                (
                    Tag::new(b"cv01"),
                    FeatureParams::CharacterVariant(CharacterVariantParams::new(
                        NameId::new(256),
                        NameId::new(257),
                        NameId::new(258),
                        1,
                        NameId::new(259),
                        vec![Uint24::new(0x61)],
                    )),
                ),
            ],
        ))),
        gpos: None,
        gdef: None,
    });
    fixtures.push(LayoutFixture {
        name: "gsub-feature-variations",
        gsub: Some(dump(&gsub_table(
            vec![single_lookup(), single_lookup()],
            Some(feature_variations()),
            vec![(Tag::new(b"ss01"), stylistic_set())],
        ))),
        gpos: None,
        gdef: None,
    });
    fixtures.push(LayoutFixture {
        name: "gdef-complete",
        gsub: None,
        gpos: None,
        gdef: Some(dump(&complete_gdef())),
    });
    fixtures
}

fn dump<T: write_fonts::validate::Validate + write_fonts::FontWrite>(table: &T) -> Vec<u8> {
    write_fonts::dump_table(table).expect("layout fixture compiles")
}

fn g(id: u16) -> GlyphId16 {
    GlyphId16::new(id)
}

fn coverage(glyphs: &[u16]) -> CoverageTable {
    CoverageTable::format_1(glyphs.iter().copied().map(g).collect())
}

fn range_coverage(start: u16, end: u16) -> CoverageTable {
    CoverageTable::format_2(vec![RangeRecord::new(g(start), g(end), 0)])
}

fn class_def_1() -> ClassDef {
    ClassDef::format_1(g(1), vec![1, 2])
}

fn class_def_2() -> ClassDef {
    ClassDef::format_2(vec![
        ClassRangeRecord::new(g(1), g(1), 1),
        ClassRangeRecord::new(g(2), g(3), 2),
    ])
}

fn scripts(feature_count: u16) -> ScriptList {
    let features = (0..feature_count).collect::<Vec<_>>();
    let mut required = LangSys::new(features.clone());
    required.required_feature_index = 0;
    ScriptList::new(vec![ScriptRecord::new(
        Tag::new(b"latn"),
        Script::new(
            Some(LangSys::new(features)),
            vec![LangSysRecord::new(Tag::new(b"TRK "), required)],
        ),
    )])
}

fn features(lookup_count: u16, params: Vec<(Tag, FeatureParams)>) -> FeatureList {
    let lookups = (0..lookup_count).collect::<Vec<_>>();
    let mut records = vec![FeatureRecord::new(
        Tag::new(b"liga"),
        Feature::new(None, lookups.clone()),
    )];
    records.extend(
        params
            .into_iter()
            .map(|(tag, params)| FeatureRecord::new(tag, Feature::new(Some(params), vec![0]))),
    );
    FeatureList::new(records)
}

fn gsub_table(
    lookups: Vec<SubstitutionLookup>,
    variations: Option<FeatureVariations>,
    params: Vec<(Tag, FeatureParams)>,
) -> Gsub {
    let lookup_count = u16::try_from(lookups.len()).expect("fixture lookup count fits");
    let feature_list = features(lookup_count, params);
    let feature_count =
        u16::try_from(feature_list.feature_records.len()).expect("fixture feature count fits");
    let mut table = Gsub::new(
        scripts(feature_count),
        feature_list,
        LookupList::new(lookups),
    );
    table.feature_variations = variations.into();
    table
}

fn gpos_table(lookups: Vec<PositionLookup>) -> Gpos {
    let lookup_count = u16::try_from(lookups.len()).expect("fixture lookup count fits");
    Gpos::new(
        scripts(1),
        features(lookup_count, Vec::new()),
        LookupList::new(lookups),
    )
}

fn lookup<T: Default>(subtables: Vec<T>) -> Lookup<T> {
    Lookup::new(LookupFlag::empty(), subtables)
}

fn single_lookup() -> SubstitutionLookup {
    SubstitutionLookup::Single(lookup(vec![SingleSubst::format_1(coverage(&[1]), 1)]))
}

/// A nested lookup record that applies lookup 0 to the first input glyph.
fn nested() -> Vec<SequenceLookupRecord> {
    vec![SequenceLookupRecord::new(0, 0)]
}

fn sequence_contexts() -> [(&'static str, SequenceContext); 3] {
    [
        (
            "format-1",
            SequenceContext::format_1(
                coverage(&[1]),
                vec![Some(SequenceRuleSet::new(vec![SequenceRule::new(
                    vec![g(2)],
                    nested(),
                )]))],
            ),
        ),
        (
            "format-2",
            SequenceContext::format_2(
                coverage(&[1]),
                class_def_2(),
                vec![
                    None,
                    Some(ClassSequenceRuleSet::new(vec![ClassSequenceRule::new(
                        vec![2],
                        nested(),
                    )])),
                ],
            ),
        ),
        (
            "format-3",
            SequenceContext::format_3(vec![coverage(&[1]), range_coverage(2, 3)], nested()),
        ),
    ]
}

fn chained_contexts() -> [(&'static str, ChainedSequenceContext); 3] {
    [
        (
            "format-1",
            ChainedSequenceContext::format_1(
                coverage(&[1]),
                vec![Some(ChainedSequenceRuleSet::new(vec![
                    ChainedSequenceRule::new(vec![g(3)], vec![g(2)], vec![g(3)], nested()),
                ]))],
            ),
        ),
        (
            "format-2",
            ChainedSequenceContext::format_2(
                coverage(&[1]),
                class_def_1(),
                class_def_2(),
                class_def_1(),
                vec![
                    None,
                    Some(ChainedClassSequenceRuleSet::new(vec![
                        ChainedClassSequenceRule::new(vec![1], vec![2], vec![2], nested()),
                    ])),
                ],
            ),
        ),
        (
            "format-3",
            ChainedSequenceContext::format_3(
                vec![coverage(&[3])],
                vec![coverage(&[1]), coverage(&[2])],
                vec![range_coverage(1, 3)],
                nested(),
            ),
        ),
    ]
}

fn gsub_lookups() -> Vec<(&'static str, SubstitutionLookup)> {
    let mut lookups = vec![
        ("gsub-single-format-1", single_lookup()),
        (
            "gsub-single-format-2",
            SubstitutionLookup::Single(lookup(vec![SingleSubst::format_2(
                range_coverage(1, 2),
                vec![g(2), g(3)],
            )])),
        ),
        (
            "gsub-multiple",
            SubstitutionLookup::Multiple(lookup(vec![MultipleSubstFormat1::new(
                coverage(&[1]),
                vec![Sequence::new(vec![g(2), g(3)])],
            )
            .into()])),
        ),
        (
            "gsub-alternate",
            SubstitutionLookup::Alternate(lookup(vec![AlternateSubstFormat1::new(
                coverage(&[1]),
                vec![AlternateSet::new(vec![g(2), g(3)])],
            )
            .into()])),
        ),
        (
            "gsub-ligature",
            SubstitutionLookup::Ligature(lookup(vec![ligature_subst().into()])),
        ),
        (
            "gsub-reverse-chain",
            SubstitutionLookup::Reverse(lookup(vec![ReverseChainSingleSubstFormat1::new(
                coverage(&[1, 2]),
                vec![coverage(&[3])],
                vec![range_coverage(1, 3)],
                vec![g(3), g(3)],
            )
            .into()])),
        ),
        (
            "gsub-extension",
            SubstitutionLookup::Extension(lookup(vec![
                gsub::ExtensionSubtable::Single(ExtensionSubstFormat1::new(
                    1,
                    SingleSubst::format_2(coverage(&[1]), vec![g(2)]),
                )),
                gsub::ExtensionSubtable::Single(ExtensionSubstFormat1::new(
                    1,
                    SingleSubst::format_1(coverage(&[2]), -1),
                )),
            ])),
        ),
        (
            "gsub-extension-ligature",
            SubstitutionLookup::Extension(lookup(vec![gsub::ExtensionSubtable::Ligature(
                ExtensionSubstFormat1::new(4, ligature_subst().into()),
            )])),
        ),
    ];
    for (format, context) in sequence_contexts() {
        lookups.push((
            match format {
                "format-1" => "gsub-context-format-1",
                "format-2" => "gsub-context-format-2",
                _ => "gsub-context-format-3",
            },
            SubstitutionLookup::Contextual(lookup(vec![SubstitutionSequenceContext::from(
                context,
            )])),
        ));
    }
    for (format, context) in chained_contexts() {
        lookups.push((
            match format {
                "format-1" => "gsub-chain-format-1",
                "format-2" => "gsub-chain-format-2",
                _ => "gsub-chain-format-3",
            },
            SubstitutionLookup::ChainContextual(lookup(vec![SubstitutionChainContext::from(
                context,
            )])),
        ));
    }
    lookups
}

fn ligature_subst() -> LigatureSubstFormat1 {
    LigatureSubstFormat1::new(
        coverage(&[1]),
        vec![LigatureSet::new(vec![Ligature::new(g(3), vec![g(2)])])],
    )
}

fn device() -> DeviceOrVariationIndex {
    DeviceOrVariationIndex::device(10, 12, &[1, -1, 0])
}

fn variation_index() -> DeviceOrVariationIndex {
    DeviceOrVariationIndex::VariationIndex(VariationIndex::new(0, 0))
}

fn anchors() -> [AnchorTable; 3] {
    [
        AnchorTable::format_1(10, 20),
        AnchorTable::format_2(10, 20, 1),
        AnchorTable::format_3(10, 20, Some(device()), Some(variation_index())),
    ]
}

fn mark_array() -> MarkArray {
    MarkArray::new(vec![
        MarkRecord::new(0, AnchorTable::format_1(0, 0)),
        MarkRecord::new(1, AnchorTable::format_2(0, 0, 0)),
    ])
}

fn gpos_lookups() -> Vec<(&'static str, PositionLookup)> {
    let [anchor1, anchor2, anchor3] = anchors();
    let mut lookups = vec![
        (
            "gpos-single-format-1",
            PositionLookup::Single(lookup(vec![SinglePos::format_1(
                coverage(&[1, 2]),
                ValueRecord::new()
                    .with_x_advance(10)
                    .with_x_placement_device(device())
                    .with_y_advance_device(variation_index()),
            )])),
        ),
        (
            "gpos-single-format-2",
            PositionLookup::Single(lookup(vec![SinglePos::format_2(
                range_coverage(1, 2),
                vec![
                    ValueRecord::new().with_x_advance(10),
                    ValueRecord::new().with_x_advance(20),
                ],
            )])),
        ),
        (
            "gpos-pair-format-1",
            PositionLookup::Pair(lookup(vec![PairPos::format_1(
                coverage(&[1]),
                vec![PairSet::new(vec![PairValueRecord::new(
                    g(2),
                    ValueRecord::new().with_x_advance(-10),
                    ValueRecord::new().with_x_placement_device(device()),
                )])],
            )])),
        ),
        (
            "gpos-pair-format-2",
            PositionLookup::Pair(lookup(vec![PairPos::format_2(
                coverage(&[1, 2]),
                class_def_1(),
                class_def_2(),
                (0..3)
                    .map(|_| {
                        Class1Record::new(
                            (0..3)
                                .map(|_| {
                                    Class2Record::new(
                                        ValueRecord::new().with_x_advance(-5),
                                        ValueRecord::new(),
                                    )
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            )])),
        ),
        (
            "gpos-cursive",
            PositionLookup::Cursive(lookup(vec![CursivePosFormat1::new(
                coverage(&[1, 2, 3]),
                vec![
                    EntryExitRecord::new(Some(anchor1.clone()), None),
                    EntryExitRecord::new(None, Some(anchor2.clone())),
                    EntryExitRecord::new(Some(anchor3.clone()), Some(anchor1.clone())),
                ],
            )
            .into()])),
        ),
        (
            "gpos-mark-to-base",
            PositionLookup::MarkToBase(lookup(vec![mark_to_base().into()])),
        ),
        (
            "gpos-mark-to-ligature",
            PositionLookup::MarkToLig(lookup(vec![MarkLigPosFormat1::new(
                coverage(&[1, 2]),
                coverage(&[3]),
                mark_array(),
                LigatureArray::new(vec![LigatureAttach::new(vec![
                    ComponentRecord::new(vec![Some(anchor1.clone()), None]),
                    ComponentRecord::new(vec![Some(anchor2.clone()), Some(anchor3.clone())]),
                ])]),
            )
            .into()])),
        ),
        (
            "gpos-mark-to-mark",
            PositionLookup::MarkToMark(lookup(vec![MarkMarkPosFormat1::new(
                coverage(&[1, 2]),
                coverage(&[3]),
                mark_array(),
                Mark2Array::new(vec![Mark2Record::new(vec![Some(anchor3), None])]),
            )
            .into()])),
        ),
        (
            "gpos-extension",
            PositionLookup::Extension(lookup(vec![
                gpos::ExtensionSubtable::MarkToBase(ExtensionPosFormat1::new(
                    4,
                    mark_to_base().into(),
                )),
                gpos::ExtensionSubtable::Single(ExtensionPosFormat1::new(
                    1,
                    SinglePos::format_1(coverage(&[3]), ValueRecord::new().with_x_advance(1)),
                )),
            ])),
        ),
    ];
    for (format, context) in sequence_contexts() {
        lookups.push((
            match format {
                "format-1" => "gpos-context-format-1",
                "format-2" => "gpos-context-format-2",
                _ => "gpos-context-format-3",
            },
            PositionLookup::Contextual(lookup(vec![PositionSequenceContext::from(context)])),
        ));
    }
    for (format, context) in chained_contexts() {
        lookups.push((
            match format {
                "format-1" => "gpos-chain-format-1",
                "format-2" => "gpos-chain-format-2",
                _ => "gpos-chain-format-3",
            },
            PositionLookup::ChainContextual(lookup(vec![PositionChainContext::from(context)])),
        ));
    }
    lookups
}

fn mark_to_base() -> MarkBasePosFormat1 {
    let [anchor1, anchor2, _] = anchors();
    MarkBasePosFormat1::new(
        coverage(&[1, 2]),
        coverage(&[3]),
        mark_array(),
        BaseArray::new(vec![BaseRecord::new(vec![Some(anchor1), Some(anchor2)])]),
    )
}

fn stylistic_set() -> FeatureParams {
    FeatureParams::StylisticSet(StylisticSetParams::new(NameId::new(256)))
}

fn feature_variations() -> FeatureVariations {
    let axis = |min: f32, max: f32| {
        Condition::format_1_axis_range(0, F2Dot14::from_f32(min), F2Dot14::from_f32(max))
    };
    // An alternate feature reads its FeatureParams as the feature it
    // substitutes, so the ss01 substitute carries stylistic-set parameters.
    let alternate = Feature::new(None, vec![1]);
    let alternate_stylistic_set = Feature::new(Some(stylistic_set()), vec![1]);
    FeatureVariations::new(vec![
        FeatureVariationRecord::new(
            Some(ConditionSet::new(vec![
                axis(0.5, 1.0),
                Condition::format_2_variable_value(1, 0),
                Condition::format_3_and(2, vec![axis(0.0, 1.0), axis(0.25, 0.75)]),
                Condition::format_4_or(1, vec![axis(-1.0, 0.0)]),
                Condition::format_5_negate(axis(-0.5, 0.5)),
            ])),
            Some(FeatureTableSubstitution::new(vec![
                FeatureTableSubstitutionRecord::new(0, alternate),
                FeatureTableSubstitutionRecord::new(1, alternate_stylistic_set),
            ])),
        ),
        FeatureVariationRecord::new(None, None),
    ])
}

fn item_variation_store() -> ItemVariationStore {
    let coordinate = |value: f32| F2Dot14::from_f32(value);
    ItemVariationStore::new(
        VariationRegionList::new(
            1,
            vec![VariationRegion::new(vec![RegionAxisCoordinates::new(
                coordinate(0.0),
                coordinate(1.0),
                coordinate(1.0),
            )])],
        ),
        vec![Some(ItemVariationData::new(1, 0, vec![0], vec![5]))],
    )
}

/// The GDEF the GPOS fixtures pair with: the variation store their
/// VariationIndex tables refer to.
fn variation_gdef() -> Gdef {
    let mut gdef = Gdef::new(Some(class_def_2()), None, None, None);
    gdef.item_var_store = Some(item_variation_store()).into();
    gdef
}

fn complete_gdef() -> Gdef {
    let mut gdef = Gdef::new(
        Some(class_def_2()),
        Some(AttachList::new(
            coverage(&[1, 2]),
            vec![AttachPoint::new(vec![0, 1]), AttachPoint::new(vec![2])],
        )),
        Some(LigCaretList::new(
            coverage(&[3]),
            vec![LigGlyph::new(vec![
                CaretValue::format_1(100),
                CaretValue::format_2(1),
                CaretValue::format_3(200, device()),
                CaretValue::format_3(300, variation_index()),
            ])],
        )),
        Some(class_def_1()),
    );
    gdef.mark_glyph_sets_def = Some(MarkGlyphSets::new(vec![
        coverage(&[1]),
        range_coverage(2, 3),
    ]))
    .into();
    gdef.item_var_store = Some(item_variation_store()).into();
    gdef
}
