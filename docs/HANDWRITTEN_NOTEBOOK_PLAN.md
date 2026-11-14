# Handwritten notebook previews

Active goal: add read-only Supernote `.note`, Xournal++ `.xopp` / legacy Xournal
`.xoj`, and reMarkable notebook previews, in that priority order, on the branch
`feat/handwritten-notebook-previews`, merged at the requested checkpoint.
Supernote/Xournal integration was merged at `be539806`; remaining reMarkable
work continues on `feat/remarkable-notebook-previews`, committed in bounded steps.

Completion requires all three presentation routes for every family: rendered
file-manager list strips (with real page selection), a rendered little-screen
preview, and a large paged viewer with thumbnails, page navigation and zoom.
Parsing a file or displaying an embedded cover alone does not complete a family.
No editing, device connection, cloud sync or notebook conversion workflow is
required. Ordinary file-manager navigation and source identity guards remain.

## Library investigation

Checked public upstream source/manifests and registry metadata on 2026-11-13.
These are implementation candidates, not claims of supported formats.

### Supernote — first priority

- [supernote_pdf](https://github.com/RohanGautam/supernote_pdf), registry 0.1.1:
  native Rust, manifest explicitly declares MIT. It is a binary-only, single-file
  converter, not a drop-in parser library. It loads/renders whole notebooks for
  PDF output, and its limited device/layer handling needs comparison with real
  current files before reuse. A missing repository LICENSE file is distinct
  from its explicit manifest license declaration.
- [snlib](https://github.com/Walnut356/snlib): independent Rust container and
  `TOTALPATH` investigation. The inspected manifest/repository provides no
  license grant. Do not import its code without a license. It also leaves bitmap
  decoding as a TODO, so it is not a complete preview renderer.
- [supernote-tool / supernotelib](https://github.com/jya-dev/supernote-tool),
  inspected commit `c1019f1c290ed5b586a9518978dbb78240b882d3`: Apache-2.0,
  parser/renderer covers original ASA, X and X2, including the 20260016 signature,
  grayscale variants, orientations, visibility and templates. This is the first
  reference for fidelity comparisons. Preserve a pristine source import before
  adaptations or any attributed native port.
- [supernote-typescript](https://github.com/philips/supernote-typescript):
  Apache-2.0; modern raster/vector paths, recognition metadata and useful real
  fixtures/reference exports. Its image-js/PDF integration is not a Rust drop-in.
  Native `TOTALPATH` data must be distinguished from bitmap tracing; do not claim
  traced bitmaps are original vector strokes.
- [sntool](https://github.com/jdlugosz963/sntool): MIT Go implementation, small
  layered parser/decoder. Worth comparing for ASA/RLE details, but using it would
  add a companion build/runtime boundary instead of a compiled Rust library.

Working direction: use the licensed, mature implementation as a reference and
oracle; investigate a bounded native page reader/decoder rather than invoking a
whole-notebook PDF converter for every visible row. Decide from real-fixture
comparisons. Preserve encoded raster ink faithfully where that is the source's
canonical representation; vector availability must not compromise correctness.

### Xournal / Xournal++ — second priority

Crates.io searches for `xournal` and `xopp` found no dedicated parser/renderer.
The [canonical project](https://github.com/xournalpp/xournalpp) documents gzipped
XML, legacy compatibility and external PDF backgrounds. Review its current
loader/serializer and rendering semantics, not just example XML. The existing
flate2/quick-xml/image/font/PDF mechanisms may make a focused native reader more
appropriate than linking the entire C++ editor. Important cases are pressure
widths, pen/highlighter/eraser behavior, layers, text, embedded images, ruling,
orientation and guarded PDF backgrounds. Missing backgrounds need a visible
explanation; existing backgrounds must actually be rendered.

### reMarkable — third priority

- [remarkable-rm](https://github.com/citizen-stig/remarkable-tablet-cli-rs),
  registry 0.1.1: Rust v6 parser and raster renderer; dual MIT/Apache-2.0. Inspect
  actual parser, CRDT ordering, color/text/eraser coverage and error paths before
  selecting it. Its own description still labels rendering work in progress.
- [remarkable_lines](https://github.com/Lyr-7D1h/remarkable-lines), registry 0.1.3:
  Rust parser advertising versions 3–6; explicitly cautions about incomplete
  testing. Compare format coverage and licenses with the renderer candidate.
- [remarkable-rm TypeScript](https://github.com/Scratchydisk/remarkable-rm):
  v5/v6 SVG, pen simulation and newer color fields; useful alternative and
  comparison corpus. Evaluate the license before any code reuse.
- Also examine [rmscene](https://github.com/ricklupton/rmscene),
  [rmc](https://github.com/ricklupton/rmc), and
  [rmconvert](https://github.com/YakBarber/rmconvert), plus practical `.rmdoc`
  exports and `.rmn` archives. Raw `.rm` pages alone must not be mistaken for an
  entire notebook. RealMedia also uses `.rm`; dispatch requires a matching header.

## Architecture and validation

Reuse candidates were queried and the baseline `before-handwritten-notebooks`
was established before implementation. Current useful owners:

- Core `FileSource` / `ReadCheck`, guarded ordinary-file reads, CommandRequest,
  existing cancellable task and staging mechanisms.
- File-manager `pdf_strip::Pages` / shared preview delivery and page-at-open
  preservation; shared studio PDF thumbnail/detail caches and page viewport.
- Little-screen file ingestion and first-page image payloads; existing bounded
  SVG/raster painting and bundled fonts.

Prefer extending page-source dispatch and read-only presentation in those owners
to a second page cache/viewer or converting every notebook page to temporary PDF.
Do not accidentally expose PDF delete/rotate/OCR-write controls or shortcuts on
notebooks. Parsing and decoding stay on bounded workers. Preview dimensions,
page counts, decompression, metadata blocks and retained pixel/source memory
must be bounded independently of notebook length. Preserve source revisions and
selected page through asynchronous loading. Companion files and archive members
must obey ordinary-file guards and never follow arbitrary archive paths or
external resource URLs.

Acceptance checklist (each family must satisfy every applicable row):

- [ ] Header-qualified dispatch, including malformed/unsupported inputs and
  extension collisions; no generic XML/JSON or RealMedia misclassification.
- [ ] Real source fixtures and independent exported/reference images establish
  visible ink, correct page order, dimensions, layers, orientation and templates.
- [ ] Supernote current X/X2 and older variants are qualified or have precise,
  evidence-based compatibility diagnostics.
- [ ] Xournal++ and legacy Xournal pages, images/text/pressure/ruling and ordinary
  PDF backgrounds render; incomplete inputs do not become blank-success pages.
- [ ] reMarkable raw page versions and practical multi-page containers preserve
  ordering and ordinary backgrounds; current pens, highlights, text and erasers
  have meaningful reference comparisons.
- [ ] List strips render bounded visible first/last page groups; clicked pages
  remain selected when the large viewer finishes loading.
- [ ] Little-screen ingestion produces actual notebook page artwork.
- [ ] Large viewer provides thumbnails, page navigation and useful zoom without
  notebook editing or a browser engine.
- [ ] Source replacement, cancellation and protected inputs retain existing
  guard/lifecycle behavior; malformed data cannot cause unbounded allocation.
- [ ] Native GUI checks in both themes, appropriate backend/integration tests,
  strict Clippy, formatting and architecture comparison complete.
- [ ] Accurate catalog/compatibility documentation and review examples complete.

Personal `.cargo/config.toml` and `config.toml` edits are outside the commits.
Check disk space before large builds; serialize Cargo and cleanup. Reuse the
existing target directory. Use a frozen executable for GUI checks when workers
relaunch `current_exe`, and keep pristine imports separate from adaptations.


## Initial Supernote implementation checkpoint

- [x] Create the feature branch and establish the architecture baseline.
- [x] Compare native Rust projects with maintained Python/TypeScript/Go options;
  distinguish a CLI manifest's MIT declaration from an unlicensed parser.
- [x] Commit pristine supernotelib reference source separately from local code.
- [x] Retain three unmodified licensed real X/X2 fixtures with hashes, including
  ten-page, two-page and current N5/20260016 notebooks.
- [x] Add bounded native container inspection: header qualification, supported
  versions, footer-authoritative page order, duplicate scalar rejection, per-page
  orientation/dimensions and checked metadata offsets/budgets. No page bitmap is
  decoded by metadata inspection. Three targeted tests and strict core Clippy pass.
- [x] Implement bounded selected-page raster/layer decoding. Seven targeted tests
  pass, including exact native RGB comparison against eight independent
  reference pages covering current ink, older pens, landscape orientation and
  a custom PNG background. Strict core Clippy and leaf formatting checks pass.
  The reference images also reproduce byte-for-byte. ASA decoding remains
  unqualified without a real fixture; missing layer visibility is an explicit
  compatibility error.

Architecture comparison against `before-handwritten-notebooks` flags the
pristine upstream Python reference import (clone groups and decoder complexity),
plus unrelated cross-file symbol matches. The application notebook module
measures zero duplication/cycles and maximum cyclomatic complexity 15. Retain
the reference unchanged so independent decoder comparisons remain reproducible.

This checkpoint is not preview support: all list/little-screen/large-viewer
acceptance items above remain required. The goal remains active.

## Preview integration checkpoint

- [x] Continue after the merge on `feat/notebook-preview-integration`.
- [x] Add a disposable native notebook worker using inherited ordinary-file
  descriptors, parent/held source revalidation, cancellation, bounded output and
  CPU/address-space/time limits. Wire dispatch into studio hosts, daemon and CLI.
  A real subprocess acceptance harness verifies header-based identification,
  ten-page metadata, late-page visible ink, bounded dimensions, rejected pages,
  pre-cancellation and protected inputs. Strict core Clippy and checks of
  studios/daemon/CLI pass.
- [x] Connect little-screen artwork ingestion and IPC rehydration. The real
  subprocess harness checks visible premultiplied page thumbnails, original-file
  association, page-count labels, exact PNG regeneration after serialization
  omits payload bytes, and ordinary-text fallback for a misleading `.note`.
  PDF, Office and notebook artwork share first-page item construction.
- [x] Connect list strips and selected-page handoff. Real native acceptance
  checks in dark and light themes verify all ten page thumbnails, enlarged
  hover, page-10 handoff, visible large-page ink, landscape and custom PNG
  template artwork, read-only shortcuts, return navigation and unchanged files.
- [x] Extend the shared paged viewer in read-only mode, disabling PDF-only
  editing, OCR, text indexing, layout extraction and application handoff controls.
  Reuse the existing page cache, zoom and thumbnail navigation; selected-page
  and metadata workers carry cancellation through to native notebook children.
  All 63 paged-viewer tests pass, including direct mutation admission guards and
  shortcut/action visibility checks in both themes.

These are integration checkpoints, not completion of the full format goal.

Integration architecture review: the global comparison reports clone flags on
modified files, including unchanged payload construction and the required early
worker dispatch in independent executables. Clone groups decrease by two and
maximum complexity is unchanged. The reported CLI complexity is the existing
async body renamed from `main` to `run_cli`; an unrelated Python cycle changes
only member order. No additional application framework or raster cache is added.

- [x] Fix list-strip opening positions being overwritten by common row actions.
  Route the hovered document position through `ListActions.open_position`
  instead of writing browser state before `finish_listing` replaces it. Native
  evidence in `/tmp/notebook-gui-dark-04/04-selected-page.png` confirms that
  double-clicking the tenth thumbnail opens Page 10 of 10 through asynchronous
  notebook loading. The fix also preserves existing PDF/text strip positions.

The paged header now keeps PDF actions together, reducing accidental branching
in the shared renderer. The remaining `load` dispatch complexity increase
(18 to 22) is the notebook qualification/compatibility branch; future notebook
formats share that branch. Header reads reuse core's guarded opening and source
revalidation in both the browser loader and metadata previews. No second page
cache or notebook editing subsystem is introduced.

Native acceptance artifacts: `/tmp/notebook-gui-dark-06/report.json` and
`/tmp/notebook-gui-light-02/report.json`. The runner checks actual page paper
and ink in addition to UI labels, so loading spinners cannot satisfy the large
preview assertions. Run `scripts/notebook_preview_acceptance.py` with the
built file-manager binary and separate output directories for each appearance.
Final targeted checks: seven native Supernote tests, the real notebook worker/
clipboard harness, 63 paged-viewer tests, nine existing item regression tests,
strict Clippy for core/studios/file-manager/workbench, and a warning-free native
file-manager build. Full-workspace validation remains part of the final goal
audit after Xournal and reMarkable are integrated.

Supernote presentation now works in all three requested routes for qualified
X/X2 fixtures. Original ASA sources still need a real licensed fixture to
qualify their existing decoder; unknown versions and missing layer visibility
report explicit compatibility failures. Xournal and reMarkable remain required.

## Xournal investigation and corpus checkpoint

- [x] Establish `before-xournal-preview` at `fae43fde`. Registry queries for
  `xopp` still return no crates; `xournal` returns only unrelated utilities.
- [x] Compare the canonical C++ loader/renderer with Rnote's Rust importer
  (`crates/rnote-engine/src/fileformats/xoppformat.rs`, revision
  `83a76367dbd3d6c461c10553c45231799a25ea4b`). Rnote's parser is coupled to its
  engine traits/model, decompresses without an expansion bound, and does not
  handle the upstream ZIP test containers. Importing the complete engine would
  bring much more than a document reader. Use a focused bounded reader with
  existing flate2/quick-xml XML preflight and the already-used native resvg
  renderer, rather than importing a GTK editor or copying either codebase.
- [x] Retain unmodified licensed fixtures: original Xournal 0.4.5, modern pressure/
  eraser/highlighter strokes, named layers, background styles, transformed text
  and images, a relative external PDF, and ZIP-contained stroke/PDF documents.
  Their companion paths, hashes and source revision are recorded alongside them.
- [x] Obtain the official Xournal++ 1.3.8 x86_64 AppImage for development-only
  reference exports. Its extracted CLI renders PNG without a display; it is not
  shipped or invoked by the application. Initial canonical stroke export succeeds.
- [x] Implement bounded gzipped XML/plain XML/ZIP container reads and page index.
  Three targeted tests qualify original 0.4.5, modern gzip and packaged inputs,
  authored page fragments/order, malformed roots/versions/dimensions/entities,
  nesting, cancellation, truncated gzip, expansion and ZIP member path/size
  budgets. Strict core Clippy passes. Rendering and UI dispatch remain pending.
- [x] Render actual geometry, text/images and guarded backgrounds; compare with
  canonical exports before enabling the three presentation routes.

References inspected directly: [upstream XML loader](https://github.com/xournalpp/xournalpp/blob/4d6810b3760c7b3f90c6a3271d6a5623cfb5481f/src/core/control/xojfile/XmlParser.cpp),
[stroke renderer](https://github.com/xournalpp/xournalpp/blob/4d6810b3760c7b3f90c6a3271d6a5623cfb5481f/src/core/view/StrokeView.cpp),
[Rnote importer](https://github.com/flxzt/rnote/blob/83a76367dbd3d6c461c10553c45231799a25ea4b/crates/rnote-engine/src/fileformats/xoppformat.rs).

- [x] Export ten independent reference images using the official 2026-11-11
  nightly CLI. Stable 1.3.8 rejects current transformed text/image layouts, so
  those comparisons use the nightly instead; binary hash and diagnostics are
  recorded with the corpus. No runtime editor dependency is added.

- [x] Extend the oracle corpus to sixteen page exports, adding music staves,
  dotted/isometric paper, an actual ZIP image attachment and embedded rendered
  LaTeX PDFs. Fonts are selected through a private deterministic Fontconfig
  configuration using the same bundled metric families as the portable native
  reader; no user font settings or upstream rendering algorithms are changed.
- [x] Implement selected-page native SVG generation/rasterization with pressure
  contours, caps/dashes/fills, multiply-blended highlighters, ordered layers,
  transformed text and raster images, wrapping/alignment, ruling, local PDF/
  image backgrounds and ZIP attachments. Embedded LaTeX uses its stored PDF;
  no TeX program or notebook-supplied command executes. Background files and
  staged embedded PDFs retain source fingerprints through rendering.
- [x] Compare all sixteen independent exports with bounded pixel-difference and
  foreground-coverage assertions. Raster/photo/PDF cases are especially close;
  text layout remains an approximation of Pango and does not claim pixel-perfect
  font shaping/justification. Five backend tests pass, including malformed
  geometry and missing/remote-resource rejection. The scoped renderer has no
  clone groups/cycles and maximum function complexity 14 after separating
  paper configuration, resource resolution, stroke style and text layout.
- [x] Connect Xournal to the shared worker dispatch, all three preview routes,
  clipboard rehydration and native UI acceptance before calling it supported.


## Xournal integration validation

The shared notebook worker now qualifies Xournal XML roots inside bounded plain,
gzip and ZIP containers. The existing notebook routes supply list-page strips,
hover/selected-page handoff, a read-only paged studio, and little-screen image
payloads with clipboard rehydration. Unrelated XML and gzip content are rejected;
protected and missing backgrounds produce explicit errors. Header qualification
also covers indented plain XML.

Validation: twelve notebook backend tests, the real worker/clipboard harness,
and strict core all-target Clippy pass. Native isolated desktop acceptance covers
actual rendered artwork, eleven-page strips and last-page opening, legacy ink,
ZIP images/PDFs, embedded LaTeX, missing-background diagnostics and unchanged
source hashes. The acceptance script reuses the existing private compositor
harness and page observer; it does not access the user's desktop or clipboard.

Architecture comparison retains maximum workspace complexity and slightly reduces
duplication overall. Reported cycles/chokepoints are in untouched Python/vendor
files. The small duplicate matches are existing SVG dimension vocabulary against
an independent vendored renderer and explicit notebook acceptance gestures/setup;
the application shares its XML root reader and existing notebook presentation
rather than introducing another cache, viewer or renderer dispatch.

This is a merge checkpoint for Supernote and Xournal. reMarkable support and the
final whole-goal audit remain pending.

## reMarkable investigation checkpoint

Integration was merged at `be539806`; remaining work continues on
`feat/remarkable-notebook-previews`, with baseline `before-remarkable-previews`.

- [x] Compare actual implementation source, not only registry descriptions.
  `remarkable-rm` 0.1.1's published page parser matches upstream revision
  `544968f002f230ff55c5d438e843aab5a5e9c552`. It supports v6 only, despite a
  broader introductory sentence: the actual header reader rejects other versions.
  It provides scene-item topological ordering, nested groups, root-text layout
  with anchored ink, layers/visibility, glyph highlights, current color/pen
  fields, page-size metadata and a native tiny-skia renderer. The renderer
  approximates tablet pen simulation and skips eraser records, so erased-content
  semantics require real-fixture checks, not an assumption of perfect fidelity.
  Its dual MIT/Apache license and independent tablet-thumbnail comparisons make
  it the first candidate to qualify as a normal crate dependency.
- [x] Compare alternatives: `remarkable_lines` 0.1.3 (MIT), revision
  `5b43636f12d1509f462d45ff41687e13321c34b2`, parses v3–v6 but supplies no renderer
  and warns that versions are lightly tested. `rmconvert`, revision
  `24c679f8029dbd7a8e1fe7a0d41cfa2b8fa72b57`, assumes a fixed 163-byte front
  matter and recognizes only a handful of block flags; it is less suitable than
  the dedicated v6 parser. `rmscene` and `rmc` are useful MIT Python references,
  but rmc documents multiline text/ink-anchor problems. Scratchydisk's MIT
  TypeScript renderer also provides legacy synthetic fixtures. C++
  `librm_lines` is GPL-3.0 and would introduce another native build/FFI boundary;
  do not import it as the application's first choice while a practical Rust
  parser/renderer exists.
- [x] Retain a small independent licensed corpus with immutable hashes, including
  real v6 text/ink/layer/color/eraser pages, a two-page `.rmdoc`, generated legacy
  cases and two paired actual tablet thumbnails. Preserve fixtures separately
  from implementation changes. Total original inputs are under 1 MB.
- [ ] Qualify the Rust renderer against those inputs and immutable thumbnails;
  reject malformed/oversized geometry before raster allocation. Resolve gaps in
  legacy versions, archive page order, templates/PDF backgrounds and deletion
  semantics explicitly before calling the family supported.
- [ ] Integrate guarded worker and all three presentation routes, including
  RealMedia `.rm` collision tests and multi-page selected-page handoff.

### Native v6 page checkpoint

- [x] Add `remarkable-rm = 0.1.1` as a normal registry dependency, without
  vendoring or rewriting its implementation. Its newer resvg dependency adds a
  second renderer version; preserve existing 0.37 callers instead of coupling
  this feature to a workspace-wide SVG upgrade. Only five packages are added.
- [x] Bound raw pages to 16 MiB, 65,536 blocks, one million points, one MiB of
  text, finite coordinate ranges and bounded source/output dimensions. The
  backend is documented for disposable workers; synchronous upstream scene
  assembly/rasterization still needs the existing process timeout/memory limit.
  No file dispatch is enabled at this checkpoint.
- [x] Qualify all seventeen retained real raw v6 fixtures, including text, layers,
  newer colors and eraser pages. Empty deletion envelopes are common and do
  not themselves mean unsupported visible content. Preflight preserves that
  case but rejects deletion intervals overlapping retained live IDs and
  unsupported text/tombstone payloads, rather than displaying erased ink.
- [x] Compare actual native tablet thumbnails at 384 × 512: heavy pencil mean
  grayscale error 11.146 (limit 20), ink recall .972, precision 1.000; mixed
  pressure error 3.154 (limit 10), recall .979, precision 1.000. These are direct
  grayscale MAEs, not upstream's blended block score. A blank output control
  fails ink matching. This establishes approximate pen appearance/geometry,
  not exact tablet texture or full complex-page fidelity.
- [x] Run sixteen combined notebook tests and strict all-target core Clippy.
  Scoped native adapter metrics have no clone groups and maximum complexity 15.
  Whole-repository comparison reports only reordered cycle descriptions in
  untouched Python/vendor files; maximum workspace complexity is unchanged.
- [ ] Complete legacy pages, notebook containers/backgrounds, worker dispatch,
  list/little-screen/large-view acceptance and the final goal audit.

Developer image artifacts for this checkpoint: `/tmp/remarkable-native-review/`.
The corpus remains immutable; runtime output must never replace its references.

### Notebook ZIP index checkpoint

- [x] Index an actual `.rmdoc` through its unique document `.metadata` and
  `.content`, preserving the explicit live page array rather than sorting ZIP
  names or trusting `pageCount` over modern `cPages`. Keep per-page template and
  source-PDF redirection, inserted pages, legacy numeric fallback and deletion
  flags. Reject ambiguous roots, non-document metadata, unsafe/duplicate IDs,
  malformed arrays/register values and source/member/page budgets.
- [x] Reuse Xournal's bounded ZIP member reader in a shared notebook container
  module. Both formats keep entries in memory instead of extracting by name;
  cancellation, expanded-size checks and path validation have one owner. The
  existing Xournal expansion/traversal/truncation tests remain in place.
- [x] Check modern/legacy metadata policy against remarquee's MIT `pkg/rmdoc`
  implementation at `0694a8163ca1ca5f446ff610a961a80491451f84`. The separate
  `remarkable-metadata` Rust crate was also inspected: its narrow public content
  projection omits PDF redirection, so it does not replace the required page
  plan. This adapter reads only bounded JSON and does not import an inventory,
  device connection or CLI subsystem.
- [x] Run nineteen combined notebook tests and strict all-target core Clippy.
  The real archive's two raw annotation members also pass v6 qualification.
  Archive indexing still does not claim rendering of absent template artwork,
  PDF backgrounds or legacy raw ink; those remain explicit next steps.


### Legacy page adapter checkpoint

- [x] Use the normal `remarkable_lines = 0.1.3` MIT registry crate for v3–v5
  decoding. It has no dependencies. Adapt its stroke model into the selected
  native renderer instead of copying a binary parser or adding a second
  rasterizer. Preserve layer/point order and colors, normalize the top-left
  legacy origin to the renderer's top-center origin, and round packed pressure,
  direction, speed and width units once. Reject invalid float geometry and
  unsupported selection/erase-all tools explicitly.
- [x] Qualify the two actual v5 handwriting pages and both generated v5 cases;
  the Advent of Code page retains its independently specified 139 strokes.
  Assert exact coordinates and normalized point units from the generated source.
  v3/v4 are covered by explicitly synthetic older-header/layout probes; real
  v3/v4 captures remain unavailable, so do not describe them as tablet-qualified.
  Truncated legacy sources and v2 have explicit errors.
- [x] Visually inspect a real math-page raster; real legacy files render at strip
  resolution, while tiny generated strokes are checked at native resolution.
  This is meaningful visible-ink/geometry coverage, not an independent tablet
  pixel-fidelity claim for legacy pens. The two independent v6 tablet comparisons
  remain unchanged and pass.
- [x] Twenty combined notebook backend tests and strict all-target core Clippy
  pass. Separate modern-page register policy from collection traversal to keep
  archive parsing comprehensible; final architecture comparison reports no new
  notebook complexity/duplication/cycle findings. Unchanged Python cycle-order
  reporting still labels the global report a regression.
- [ ] Render complete archive pages with background policy, add practical RMN
  handling, connect the existing worker and all three UI routes, and verify the
  final goal. No new file-manager dispatch is enabled at this checkpoint.

### Shared background preparation

- [x] Reuse the existing Xournal native SVG/font owner and retained temporary PDF
  holder as notebook-level helpers for the next archive renderer. No second
  font database, external SVG path resolver or temporary PDF lifecycle is added.
- [x] Add an explicitly square-bounded detail PDF render path up to 4096 pixels.
  Compact thumbnails keep their 1024 ceiling; notebook background detail no
  longer silently inherits that thumbnail ceiling. A real PDF regression test
  verifies a requested 2048-pixel background stays at 2048, and the canonical
  Xournal/Supernote comparisons still pass.

### Complete archive-page backend checkpoint

- [x] Index plain RMN TAR exports through the same authored document plan as
  RMDOC ZIPs. Keep bounded entry tables and seek only retained payload offsets;
  reject traversal, duplicate canonical names, TAR links/special entries,
  truncation and source/member/count limits. Never extract by archive paths.
- [x] Resolve nested RMT templates through their actual `template.json` identity,
  including an archive filename that differs from `fileName`. Decode only the
  selected SVG/PNG artwork, with bounded XML/image budgets, bundled fonts and
  no external resource resolver. SVG image/foreign/script payloads and external
  hrefs fail explicitly; ordinary vector templates and PNG artwork are supported.
- [x] Apply legacy `.pagedata`, preserve modern template references, composite
  selected annotation ink over template/PDF backgrounds, honor source-PDF page
  redirection and render intentionally empty inserted pages. Present corrupt ink
  and missing source PDFs remain errors. Missing factory paper artwork instead
  produces a structured metadata warning while retaining usable real ink.
- [x] Retain explicitly derived RMN/RMT and PDF-redirection fixtures with our own
  paper/PDF artwork and the already licensed real v5/v6 ink. These exercise the
  documented archive layouts without pretending to be untouched tablet exports.
  See RCU's [RMN/RMT appendices](https://www.davisr.me/projects/rcu/manual.pdf),
  pages 53–56 (printed numbering). No RCU implementation is imported.
- [x] Twenty-six combined notebook backend tests and strict all-target core
  Clippy pass, including independent tablet comparisons, known background colors,
  source-page order, actual ink, empty insertion, missing PDF/corrupt ink errors,
  TAR limits and resource rejection. Scoped archive modules have no clone groups,
  dependency cycles or complexity findings (maximum function complexity 14).
  Whole-repository findings are unchanged Python ordering noise and an existing
  PDF edit fallback clone outside these changes; its source is unchanged.
- [x] Consolidate identical decoder cancellation checks under the notebook owner,
  retaining Acquire observation. Shared page metadata now owns optional warnings;
  existing worker JSON omits empty warnings for compatibility.
- [ ] Wire the new backend through guarded worker selection, preserve RealMedia
  routing, display warnings and qualify list/little-screen/large-view interaction.
