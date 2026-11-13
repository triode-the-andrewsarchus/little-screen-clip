//! Tests of the typed layout walker (`font_subset_layout.rs`, #210).

use super::font_subset_layout::{
    validate, validate_branches, LayoutCheck, LayoutFailure, LayoutTable, MAX_LAYOUT_WORK,
};
use super::font_subset_layout_fixtures::{layout_fixtures, maxp_base, LayoutFixture};
use super::{font_subset, svg_fonts};
use std::collections::BTreeSet;
use write_fonts::read::{types::Tag, FontRef};
use write_fonts::tables::gpos::{
    Class1Record, Class2Record, Gpos, PairPos, PositionLookup, ValueRecord,
};
use write_fonts::tables::gsub::{Gsub, SingleSubst, SubstitutionLookup};
use write_fonts::tables::layout::{
    ClassDef, CoverageTable, Feature, FeatureList, FeatureRecord, LangSys, Lookup, LookupFlag,
    LookupList, RangeRecord, Script, ScriptList, ScriptRecord, SequenceContext,
    SequenceLookupRecord, SequenceRule, SequenceRuleSet,
};
use write_fonts::types::GlyphId16;

/// Every branch the walker records: each lookup type and subtable format of
/// the OpenType layout specification.
const BRANCHES: [&str; 56] = [
    "anchor-1",
    "anchor-2",
    "anchor-3",
    "caret-value-1",
    "caret-value-2",
    "caret-value-3",
    "chained-context-1",
    "chained-context-2",
    "chained-context-3",
    "class-def-1",
    "class-def-2",
    "condition-1",
    "condition-2",
    "condition-3",
    "condition-4",
    "condition-5",
    "context-1",
    "context-2",
    "context-3",
    "coverage-1",
    "coverage-2",
    "device",
    "feature-params-character-variant",
    "feature-params-size",
    "feature-params-stylistic-set",
    "feature-variations",
    "gdef-attach-list",
    "gdef-glyph-class-def",
    "gdef-item-variation-store",
    "gdef-lig-caret-list",
    "gdef-mark-attach-class-def",
    "gdef-mark-glyph-sets",
    "gpos-1",
    "gpos-1-format-1",
    "gpos-1-format-2",
    "gpos-2",
    "gpos-2-format-1",
    "gpos-2-format-2",
    "gpos-3",
    "gpos-4",
    "gpos-5",
    "gpos-6",
    "gpos-7",
    "gpos-8",
    "gpos-9",
    "gsub-1",
    "gsub-1-format-1",
    "gsub-1-format-2",
    "gsub-2",
    "gsub-3",
    "gsub-4",
    "gsub-5",
    "gsub-6",
    "gsub-7",
    "gsub-8",
    "variation-index",
];

fn fixture_font(fixture: &LayoutFixture) -> Vec<u8> {
    fixture.install(&maxp_base())
}

fn raw_font(tag: &[u8; 4], table: Vec<u8>) -> Vec<u8> {
    LayoutFixture {
        name: "raw",
        gsub: (tag == b"GSUB").then(|| table.clone()),
        gpos: (tag == b"GPOS").then(|| table.clone()),
        gdef: (tag == b"GDEF").then_some(table),
    }
    .install(&maxp_base())
}

fn check(sfnt: &[u8]) -> Result<usize, LayoutFailure> {
    validate(&FontRef::new(sfnt).expect("test font is an sfnt"))
}

fn g(id: u16) -> GlyphId16 {
    GlyphId16::new(id)
}

/// A GSUB with one `liga` feature over all of `lookups` under a default
/// language system that also requires `required`.
fn gsub(lookups: Vec<SubstitutionLookup>, feature_lookups: Vec<u16>, required: u16) -> Vec<u8> {
    let mut lang_sys = LangSys::new(vec![0]);
    lang_sys.required_feature_index = required;
    let table = Gsub::new(
        ScriptList::new(vec![ScriptRecord::new(
            Tag::new(b"latn"),
            Script::new(Some(lang_sys), Vec::new()),
        )]),
        FeatureList::new(vec![FeatureRecord::new(
            Tag::new(b"liga"),
            Feature::new(None, feature_lookups),
        )]),
        LookupList::new(lookups),
    );
    raw_font(
        b"GSUB",
        write_fonts::dump_table(&table).expect("GSUB fixture compiles"),
    )
}

fn single(subtable: SingleSubst) -> SubstitutionLookup {
    SubstitutionLookup::Single(Lookup::new(LookupFlag::empty(), vec![subtable]))
}

#[test]
fn layout_fixtures_pass_and_cover_every_walker_branch() {
    let mut branches = BTreeSet::new();
    for fixture in layout_fixtures() {
        let sfnt = fixture_font(&fixture);
        let taken = validate_branches(&FontRef::new(&sfnt).expect("fixture is an sfnt"))
            .unwrap_or_else(|failure| panic!("fixture {} fails: {failure:?}", fixture.name));
        branches.extend(taken);
    }
    assert_eq!(branches, BRANCHES.into_iter().collect());
}

#[test]
fn layout_failures_carry_the_table_and_check() {
    let failure = |table, check| Err(LayoutFailure { table, check });
    let gsub_failure = |check| failure(LayoutTable::Gsub, check);

    assert_eq!(
        check(&gsub(
            vec![single(SingleSubst::format_2(
                CoverageTable::format_1(vec![g(1), g(9)]),
                vec![g(2), g(3)],
            ))],
            vec![0],
            0xffff,
        )),
        gsub_failure(LayoutCheck::GlyphId)
    );
    assert_eq!(
        check(&gsub(
            vec![single(SingleSubst::format_2(
                CoverageTable::format_2(vec![RangeRecord::new(g(3), g(1), 0)]),
                vec![g(2), g(3), g(1)],
            ))],
            vec![0],
            0xffff,
        )),
        gsub_failure(LayoutCheck::RangeOrder)
    );
    assert_eq!(
        check(&gsub(
            vec![single(SingleSubst::format_2(
                CoverageTable::format_1(vec![g(1), g(2)]),
                vec![g(2)],
            ))],
            vec![0],
            0xffff,
        )),
        gsub_failure(LayoutCheck::Count)
    );
    assert_eq!(
        check(&gsub(
            vec![single(SingleSubst::format_1(
                CoverageTable::format_1(vec![g(3)]),
                5,
            ))],
            vec![0],
            0xffff,
        )),
        gsub_failure(LayoutCheck::SingleSubstDelta)
    );
    let ok = || {
        single(SingleSubst::format_1(
            CoverageTable::format_1(vec![g(1)]),
            1,
        ))
    };
    assert!(check(&gsub(vec![ok()], vec![0], 0)).is_ok());
    assert_eq!(
        check(&gsub(vec![ok()], vec![1], 0xffff)),
        gsub_failure(LayoutCheck::LookupIndex)
    );
    assert_eq!(
        check(&gsub(vec![ok()], vec![0], 1)),
        gsub_failure(LayoutCheck::FeatureIndex)
    );
    let context = |sequence_index, lookup_index| {
        SubstitutionLookup::Contextual(Lookup::new(
            LookupFlag::empty(),
            vec![SequenceContext::format_1(
                CoverageTable::format_1(vec![g(1)]),
                vec![Some(SequenceRuleSet::new(vec![SequenceRule::new(
                    vec![g(2)],
                    vec![SequenceLookupRecord::new(sequence_index, lookup_index)],
                )]))],
            )
            .into()],
        ))
    };
    assert!(check(&gsub(vec![context(1, 0)], vec![0], 0xffff)).is_ok());
    assert_eq!(
        check(&gsub(vec![context(2, 0)], vec![0], 0xffff)),
        gsub_failure(LayoutCheck::SequenceIndex)
    );
    assert_eq!(
        check(&gsub(vec![context(0, 1)], vec![0], 0xffff)),
        gsub_failure(LayoutCheck::LookupIndex)
    );

    // write-fonts derives class1Count from ClassDef1, so the invalid count is
    // patched into a valid table: ClassDef1 assigns class 1, and one class 1
    // record fewer than it needs remains.
    let pair = Gpos::new(
        ScriptList::new(Vec::new()),
        FeatureList::new(Vec::new()),
        LookupList::new(vec![PositionLookup::Pair(Lookup::new(
            LookupFlag::empty(),
            vec![PairPos::format_2(
                CoverageTable::format_1(vec![g(1)]),
                ClassDef::format_1(g(1), vec![1]),
                ClassDef::format_1(g(1), vec![0]),
                (0..2)
                    .map(|_| {
                        Class1Record::new(vec![Class2Record::new(
                            ValueRecord::new().with_x_advance(1),
                            ValueRecord::new(),
                        )])
                    })
                    .collect(),
            )],
        ))]),
    );
    let mut pair = write_fonts::dump_table(&pair).expect("GPOS fixture compiles");
    assert!(check(&raw_font(b"GPOS", pair.clone())).is_ok());
    let class1_count = {
        use write_fonts::read::tables::gpos::{Gpos, PairPos, PositionSubtables};
        use write_fonts::read::FontRead;
        let gpos = Gpos::read(write_fonts::read::FontData::new(&pair)).expect("GPOS parses");
        let lookup = gpos
            .lookup_list()
            .and_then(|list| list.lookups().get(0))
            .and_then(|lookup| lookup.subtables())
            .expect("the lookup parses");
        let PositionSubtables::Pair(subtables) = lookup else {
            unreachable!("the lookup is a pair lookup")
        };
        let Ok(PairPos::Format2(subtable)) = subtables.get(0) else {
            unreachable!("the subtable is a class pair subtable")
        };
        subtable.offset_data().as_bytes().as_ptr() as usize - pair.as_ptr() as usize
            + subtable.class1_count_byte_range().start
    };
    assert_eq!(pair[class1_count..class1_count + 2], [0, 2]);
    pair[class1_count + 1] = 1;
    assert_eq!(
        check(&raw_font(b"GPOS", pair)),
        failure(LayoutTable::Gpos, LayoutCheck::ClassValue)
    );

    // A GSUB whose only lookup has type 9, which the specification does not
    // define.
    let unknown_lookup = vec![
        0x00, 0x01, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x0c, 0x00, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x01, 0x00, 0x04, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00,
    ];
    assert_eq!(
        check(&raw_font(b"GSUB", unknown_lookup)),
        gsub_failure(LayoutCheck::UnknownFormat)
    );
    // A GDEF 1.0 whose glyph ClassDef has format 5, which the specification
    // does not define.
    let unknown_class_format = vec![
        0x00, 0x01, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00,
        0x00,
    ];
    assert_eq!(
        check(&raw_font(b"GDEF", unknown_class_format)),
        failure(LayoutTable::Gdef, LayoutCheck::UnknownFormat)
    );
    // A GDEF 1.0 whose glyph ClassDef is a well-formed format 3, the 24-bit
    // glyph ID form ISO/IEC 14496-22:2026 adds, classifying glyph 1 as 1.
    // read-fonts parses it; the walker does not model it.
    let wide_class_format = vec![
        0x00, 0x01, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x01,
    ];
    assert_eq!(
        check(&raw_font(b"GDEF", wide_class_format)),
        failure(LayoutTable::Gdef, LayoutCheck::UnknownFormat)
    );
    // A GDEF 1.0 whose glyph ClassDef format 1 starts at glyph 2 and
    // classifies three glyphs of four.
    let class_range = vec![
        0x00, 0x01, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x02, 0x00, 0x03, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01,
    ];
    assert_eq!(
        check(&raw_font(b"GDEF", class_range)),
        failure(LayoutTable::Gdef, LayoutCheck::ClassRange)
    );
    // A GDEF 1.0 whose glyph ClassDef format 1 counts two class values but
    // ends after the count. read-fonts reads the truncated array as empty,
    // which the retired traversal validator accepted (#210).
    let truncated = vec![
        0x00, 0x01, 0x00, 0x00, 0x00, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x01, 0x00, 0x02,
    ];
    assert_eq!(
        check(&raw_font(b"GDEF", truncated)),
        failure(LayoutTable::Gdef, LayoutCheck::Count)
    );
    // A GDEF 1.0 whose glyph ClassDef offset points past the table.
    let unresolved = vec![
        0x00, 0x01, 0x00, 0x00, 0x7f, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    assert_eq!(
        check(&raw_font(b"GDEF", unresolved)),
        failure(LayoutTable::Gdef, LayoutCheck::Parse)
    );
}

#[test]
fn shared_offsets_exhaust_the_layout_work_budget() {
    // A GDEF 1.2 whose 40 mark glyph sets share one 30,000-glyph coverage:
    // 60 KB of table directing 1.2M visits.
    const SETS: u32 = 40;
    const GLYPHS: u16 = 30_000;
    let mut gdef = vec![
        0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0e, 0x00,
        0x01,
    ];
    gdef.extend(u16::try_from(SETS).expect("set count fits").to_be_bytes());
    for _ in 0..SETS {
        gdef.extend((4 + 4 * SETS).to_be_bytes());
    }
    gdef.extend([0x00, 0x01]);
    gdef.extend(GLYPHS.to_be_bytes());
    gdef.extend(std::iter::repeat_n(0, usize::from(GLYPHS) * 2));
    let sfnt = raw_font(b"GDEF", gdef);
    assert_eq!(
        check(&sfnt),
        Err(LayoutFailure {
            table: LayoutTable::Gdef,
            check: LayoutCheck::Budget,
        })
    );
}

/// Every bundled source: the 230 official chunks, then the Noto Emoji
/// fallback, each as the sfnt the subsetter plans over.
fn bundled_sources() -> Vec<(svg_fonts::SubsetSource, Vec<u8>)> {
    crate::fonts::revision_1()
        .assets
        .iter()
        .map(svg_fonts::SubsetSource::chunk)
        .chain([svg_fonts::SubsetSource::noto_emoji()])
        .map(|source| {
            let bytes = source
                .load()
                .expect("bundled source verifies")
                .expect("bundled source exists")
                .bytes()
                .to_vec();
            let sfnt = match source.container() {
                font_subset::SourceContainer::Woff2 => {
                    wuff::decompress_woff2(&bytes).expect("bundled source decodes")
                }
                font_subset::SourceContainer::Sfnt => bytes,
            };
            (source, sfnt)
        })
        .collect()
}

#[test]
fn the_largest_bundled_layout_stays_under_the_work_budget() {
    let (identity, work) = bundled_sources()
        .into_iter()
        .map(|(source, sfnt)| {
            let work = check(&sfnt)
                .unwrap_or_else(|failure| panic!("{} fails: {failure:?}", source.identity()));
            (source.identity(), work)
        })
        .max_by_key(|(_, work)| *work)
        .expect("fonts are bundled");
    println!("largest bundled layout walk: {identity}, {work} of {MAX_LAYOUT_WORK}");
    assert!(
        work > 0 && work <= MAX_LAYOUT_WORK / 4,
        "{identity} uses {work}"
    );
}
