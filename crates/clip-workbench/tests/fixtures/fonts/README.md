# Font decoder fixtures

`latin.woff` and `latin.woff2` are ASCII subsets of the bundled Noto Sans CJK JP Regular font,
renamed LittleClipFixture. It retains the original copyright and license name
records and is distributed under the accompanying SIL Open Font License.

Generated with fontTools `subset.Subsetter`, keeping U+0020–U+007F and all name
records, replacing name IDs 1, 3, 4, 6 and 16 with LittleClipFixture, then saving
with `TTFont.flavor` set to `"woff"` or `"woff2"`. The source is
`crates/little-clip-render/assets/fonts/NotoSansCJK-JP-Regular.ttf`.
