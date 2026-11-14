# reMarkable qualification corpus

All retained inputs are unmodified. `manifest.json` records each immutable source
URL/revision, original path, byte size and SHA-256. Each source group includes
its original MIT license notice. These are document fixtures, not implementation
imports; no upstream code has been copied into application source.

- `rmscene/`: Rick Lupton and contributors' real v6 parser fixtures: ordered
  strokes, layers, paragraph styles, newer colors/shader, and scene dimensions.
  Expected semantics can be checked against that project's parser tests.
- `rmc/`: Rick Lupton and contributors' real v6 rendering fixtures: erasers,
  extended pages, mixed text/ink, multiline text, pens, sizes and checkboxes.
  These inputs alone are not independent pixel reference images.
- `legacy-generated/`: Stewart McSporran (Scratchydisk) and contributors'
  synthetic v5 simple/multilayer parser fixtures. They are intentionally labeled
  generated, not real tablet captures or full legacy-format qualification.
- `archives/Test.rmdoc`: go-go-golems/remarquee contributors' actual two-page
  notebook ZIP export. Its `.content` has modern `cPages` metadata, ordered live
  page identifiers and a dotted-paper template name; the template artwork itself
  is not embedded. Missing template assets must not silently become a claim of
  faithful paper rendering. Device/firmware details are unknown.
- `tablet-thumbnails/`: hwangso595 and eink-sync contributors' two paired real
  `.rm` inputs and native 384 × 512 tablet thumbnails, from revision
  `33a686ba8ab7dec3fc22d036460b183a0301b4ed`. Upstream identifies their origin in
  `extraction/test_renderer_quality.py`. Heavy pencil (`p2`) and mixed pressure
  (`p3`) provide independent visible-ink evidence. Device, firmware, template and
  capture dates are unknown. Their reference PNGs must not be regenerated from
  the implementation under test.

Do not use Scratchydisk's `sample-page.rm` / `sample-tablet-export.png` as a
full-image oracle: the Rust renderer's upstream conformance investigation found
that they contain different page revisions. No such pair is retained here.

The corpus is a qualification input, not a support declaration. Rendering,
container ordering/backgrounds, bounded workers and all three presentation
routes still require implementation and verification.

`legacy-real/` adds Lyr and remarkable-lines contributors' two actual v5 test
pages (math and Advent of Code), with their original package MIT declaration
and pinned source hashes. Unlike the generated legacy cases, these exercise
substantial real handwriting. They are not independent image references.
