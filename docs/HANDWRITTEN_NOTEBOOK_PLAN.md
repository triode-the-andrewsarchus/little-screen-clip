# Handwritten notebook previews

Active goal: add read-only Supernote `.note`, Xournal++ `.xopp` / legacy Xournal
`.xoj`, and reMarkable notebook previews, in that priority order, on the branch
`feat/handwritten-notebook-previews`. Work is committed in bounded steps.

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
- [ ] Connect list strips and selected-page handoff.
- [ ] Extend the shared paged viewer in read-only mode, disabling PDF-only tools.

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
