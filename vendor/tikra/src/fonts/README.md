# Embedded fonts

Modified versions of the four styles of TeX Gyre Termes, cut down to what tikra reads: the Times that
labels in `\fontfamily{ptm}` are drawn in. They are not the original fonts and are not meant to be
installed or used outside tikra.

| File | Original | License |
|---|---|---|
| `texgyretermes-{regular,bold,italic,bolditalic}-subset.otf` | TeX Gyre Termes 2.004, [GUST](https://www.gust.org.pl/projects/e-foundry/tex-gyre/termes) (in TeX Live) | GUST Font License (`GUST-FONT-LICENSE-TeXGyreTermes.txt`) |

Changes, made with fontTools from the originals:

```console
$ fonttools subset texgyretermes-$style.otf --unicodes="U+0020-007E,U+00A0-00FF,U+2013,U+2014,U+2018,U+2019,U+201C,U+201D,U+FB01,U+FB02" \
    --layout-features='' --no-hinting --name-IDs=0,13,14 --drop-tables+=GSUB,GDEF,GPOS \
    --output-file=texgyretermes-$style-subset.otf
```

Basic Latin and Latin-1, dashes, quotes and the fi and fl ligatures, with their outlines and advances
as in the original; hinting, substitutions, positioning (kerning: tikra kerns as TeX's `ptm` does, in
`times.rs`) and glyph names removed; the `name` table reduced to copyright and license.
