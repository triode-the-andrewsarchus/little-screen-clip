# Embedded fonts

These are modified versions of three fonts, cut down to what mathra reads. They are not the original
fonts and are not meant to be installed or used outside mathra.

| File | Original | License |
|---|---|---|
| `STIXTwoMath-Regular-subset.otf` | STIX Two Math 2.12, [stipub/stixfonts](https://github.com/stipub/stixfonts) | SIL Open Font License 1.1 (`OFL-STIXTwoMath.txt`) |
| `latinmodern-math-subset.otf` | Latin Modern Math 1.959, [GUST](https://www.gust.org.pl/projects/e-foundry/lm-math) | GUST Font License (`GUST-FONT-LICENSE-LatinModernMath.txt`) |
| `NotoEmoji-Regular-subset.ttf` (the `emoji` feature) | Noto Emoji 3.002, the monochrome one, by Google, [google/fonts](https://github.com/google/fonts/tree/b979dba422e445492b0eb9951ac52ee0b4d648c3/ofl/notoemoji) | SIL Open Font License 1.1 (`OFL-NotoEmoji.txt`) |

Changes, made by [`tools/fonts/subset.sh`](../../../tools/fonts/subset.sh) from the originals kept in
`tools/fonts`: hinting removed; the GPOS, GDEF and DSIG tables removed, and of GSUB all but the `ssty`
feature (the forms for scripts), with the glyphs reached only through what was removed; glyph names removed; the `name` table reduced to copyright and license;
CFF subroutines expanded. Every character the original maps keeps its glyph, glyph ID, outline,
metrics and MATH data, which `tests/fonts.rs` checks against the originals.

Noto Emoji (not Noto Color Emoji, a different font) comes from google/fonts, the repository of the Google Fonts collection: `https://raw.githubusercontent.com/google/fonts/b979dba422e445492b0eb9951ac52ee0b4d648c3/ofl/notoemoji/NotoEmoji%5Bwght%5D.ttf`, commit `b979dba` ("Update Noto Emoji to the 3.002 release"), 1,982,596 bytes, sha256 `de6c18832938afc99caf132b39d6a30a19bac7f2e812e28db2535b4608d27551`, kept as `tools/fonts/NotoEmoji[wght].ttf`; its license is `OFL.txt` from the same folder (sha256 `500bb1ccf43df7bbb522112f9133a52b16e1c35e809632f5d8609b179152de5b`). It is a variable font (weight 300 to 700) and no single-weight file is published there, so `subset.sh` fixes it at its default weight, regular (400), with fonttools' instancer, then keeps the glyphs the cmap reaches with no GSUB (mathra does not shape text: ZWJ sequences, flags and skin tones go), vertical metrics, hinting or glyph names, and the `name` table cut to copyright and license; its glyph IDs change. The result is 773,660 bytes, sha256 `e4df2b2136b49fefcc2e730aa879c4008aba850908ae340985b38896f50122ad`, and the script writes the same bytes each time. The OFL allows modified versions; the original declares no Reserved Font Name, and the file is named `*-subset` so it is not taken for the original.
