# Native diagram previews and editing

Initial branch: `feat/native-diagram-previews` (merged); continuations: `feat/native-diagram-previews-next` (merged), `feat/native-math-previews`; baseline: `native-diagram-previews` at `c9cbf22`.

## Required outcome

Every item under Markdown in `file_types.md`, from front matter through Symbolator, must render in the large text viewer and the little-screen preview. Formats with standalone extensions also need guarded opening, highlighted editable source, live preview and visible diagnostics. Investigate existing graphical editors and use direct manipulation with source correspondence where practical. No browser engine or silent network rendering. Make reviewable commits as each part lands.

Preserve FileSource identity checks, atomic save/Save As semantics, accepted write completion, preview cancellation and per-host lifecycle. Rendering must not resolve arbitrary network URLs, include files, execute diagram directives or read unguarded resources. Enforce input, complexity, output and raster budgets. Unknown/unsupported syntax must remain inspectable with a diagnostic, never silently treated as fully supported.

## Implementation sequence

- [x] Shared metadata framing/parsing and both Markdown presentations.
- [x] Shared bounded native worker/render contract and diagnostics, reusing command ownership and raster dependencies.
- [ ] Per-element source spans and exact edit correspondence for graphical editing.
- [ ] Native renderer adapters, compared against representative upstream fixtures.
- [x] Markdown inline/display math rendering in both output paths.
- [ ] Standalone format detection (including ambiguous/compound extensions), source + live-preview + diagnostics editor and guarded saves.
- [ ] Format-specific graphical interactions, with exact source edits or explicit unsupported-operation diagnostics; preserve unknown source content.
- [ ] Full validation, visual review at both sizes, examples, license records and accurate checklist updates.

This is a tracking document, not a claim of implemented coverage. The entire scope remains required.

## Research ledger

Initial crates.io search, 2026-11-12. Candidates below require API/source/license/compatibility review before selection; matching a name is not evidence of format compatibility. In particular the `bytefield` crate is a bitfield library, not the Bytefield-SVG language, and `vega-view` embeds a webview and is unsuitable here.

| Requested family | Initial native candidates / alternatives to investigate | Status |
| --- | --- | --- |
| Front matter | [frontmatter](https://crates.io/crates/frontmatter) 0.4.0; [frontmatter-gen](https://crates.io/crates/frontmatter-gen) 0.0.12; [mdbook-frontmatter-fix](https://crates.io/crates/mdbook-frontmatter-fix) 0.4.0 | Pending comparison / editor research |
| LaTeX math | Mathra 1.12.0; latex-rust 2.1.1 / 3.0.3; retex-svg 1.9.0; KaTeX; MathLive editor reference | Native Mathra selected; inline/display previews implemented in both viewers; structured graphical editing pending |
| Mermaid | Merman 0.10.0; mermaid-rs-renderer 0.3.1; ariel-rs 0.4 | Selected Merman; native source/live previews implemented; parser-backed graphical editing pending |
| PlantUML / C4 | [mdbook-plantuml](https://crates.io/crates/mdbook-plantuml) 2.0.0; [plantuml_encoding](https://crates.io/crates/plantuml_encoding) 2.0.3; [plantuml-little](https://crates.io/crates/plantuml-little) 1.2026.2-4 | Pending comparison / editor research |
| D2 | d2-rs 0.1.7 pipeline; d2-render 0.1.0; graphs-tui 0.4.0; official Go D2 | Native compiler/layout and scene renderer integrated; flat-board Markdown/standalone previews and highlighted guarded saves implemented; graphical editing/board selection pending |
| Graphviz DOT | graphviz-pure 1.2.0; layout-rs 0.1.3; graphviz-rust 0.9.9 | Native Graphviz port selected; source/live preview and bounded multi-graph rendering implemented; graphical manipulation pending |
| DBML | [dbml-language-server](https://crates.io/crates/dbml-language-server) 0.1.2; [dbml_diff_mysql](https://crates.io/crates/dbml_diff_mysql) 0.1.5; [sea-orm-dbml](https://crates.io/crates/sea-orm-dbml) 0.1.0-beta.2 | Pending comparison / editor research |
| BPMN | BPMN Kit 0.2.1 native model/layout/SVG; bpmn-svg 0.1.0; bpmn-js and BPMNKit graphical editors | Native authored-DI and automatic-layout previews; highlighted source editor; direct manipulation optional follow-up |
| draw.io | [drawio-exporter](https://crates.io/crates/drawio-exporter) 1.6.0; [flowchart-mcp-server](https://crates.io/crates/flowchart-mcp-server) 0.1.0; [drawio-lc](https://crates.io/crates/drawio-lc) 0.4.0 | Pending comparison / editor research |
| Excalidraw | excalidraw-render 0.1.0; excalidraw-document 0.2.0; gpui-excalidraw 0.1.9; excalidraw-image 0.2.0 | Native SVG renderer and preserving graphical transactions integrated; initial drawing/selection/movement canvas works; text editing and resize handles pending |
| WaveDrom | Canonical 3.8.0; native wavedrom 0.1.0 and current Rust main; WavePaint graphical reference | Canonical engine embedded without a browser; timing/register/logic source previews integrated; direct timing painting and lane naming integrated; advanced operations stay in source |
| Structurizr | [oak-structurizr](https://crates.io/crates/oak-structurizr) 0.0.11; [dendryform-export](https://crates.io/crates/dendryform-export) 0.1.0; [cadastre](https://crates.io/crates/cadastre) 0.4.0 | Pending comparison / editor research |
| BlockDiag / SeqDiag / ActDiag / NwDiag / PacketDiag / RackDiag | [xio_job_to_blockdiag](https://crates.io/crates/xio_job_to_blockdiag) 0.8.0 | Pending comparison / editor research |
| Bytefield | [bytefield](https://crates.io/crates/bytefield) 0.1.0; [wiry-core](https://crates.io/crates/wiry-core) 0.1.0; [wiry-capture](https://crates.io/crates/wiry-capture) 0.1.0 | Pending comparison / editor research |
| Ditaa | [mdbook-plantuml](https://crates.io/crates/mdbook-plantuml) 2.0.0; [ansicht](https://crates.io/crates/ansicht) 0.1.0; [plantuml-little](https://crates.io/crates/plantuml-little) 1.2026.2-4 | Pending comparison / editor research |
| Erd | [rusterd](https://crates.io/crates/rusterd) 0.3.1; [erd_rs](https://crates.io/crates/erd_rs) 0.1.0; [fastbreak](https://crates.io/crates/fastbreak) 0.5.0 | Pending comparison / editor research |
| Nomnoml | Canonical Nomnoml 1.7.0 with Graphre 0.1.3; crates.io hits silica/squaredb inspected and unrelated | Hostless SVG prototype works; pristine MIT engines imported separately; renderer/editor integration pending |
| Pikchr | [pikchr](https://crates.io/crates/pikchr) 0.1.4; [pikchr-rs](https://crates.io/crates/pikchr-rs) 0.1.0; [pikchr-sys](https://crates.io/crates/pikchr-sys) 0.1.1 | Selected pikchr-rs; source/live previews implemented; direct manipulation pending |
| Svgbob | [svgbob](https://crates.io/crates/svgbob) 0.7.6; [mdbook-svgbob](https://crates.io/crates/mdbook-svgbob) 0.3.1; [mdbook-svgbob2](https://crates.io/crates/mdbook-svgbob2) 0.3.0 | Selected original SvgBob; source/live previews implemented; cell editor and .bob disambiguation pending |
| GoAT | [aasvg](https://crates.io/crates/aasvg) 1.0.0; [formulaa](https://crates.io/crates/formulaa) 0.1.0 | Pending comparison / editor research |
| Lini | Original lini 1.1.0 compiler; mdbook-lini and lini-wasm integrations of the same compiler; specification reimplementation | Original native compiler selected; highlighted source/live previews implemented, graphical editing pending |
| TikZ / PGF | [mdbook-tikz](https://crates.io/crates/mdbook-tikz) 0.2.8; [tikra](https://crates.io/crates/tikra) 1.9.2; [tikra-cli](https://crates.io/crates/tikra-cli) 1.9.2 | Pending comparison / editor research |
| UMLet | No direct crates.io match; compare upstream implementation and format specification. | Pending comparison / editor research |
| Vega / Vega-Lite | [vega](https://crates.io/crates/vega) 0.0.1; [vega-view](https://crates.io/crates/vega-view) 0.1.0; [encdr](https://crates.io/crates/encdr) 0.9.0 | Pending comparison / editor research |
| WireViz | [wirebug](https://crates.io/crates/wirebug) 0.1.0 | Pending comparison / editor research |
| Symbolator | No verified native candidate yet; compare canonical Symbolator with HDL parser libraries | Pending compatibility, license and editor research; unrelated graphics crates are not format implementations |

Additional promising Mermaid alternatives: [mermaid-rs-renderer](https://github.com/1jehuang/mermaid-rs-renderer), [merman](https://docs.rs/merman/latest/merman/) and [ariel-rs](https://docs.rs/ariel-rs/latest/ariel_rs/). Compare typed models/source correspondence, compatibility, dependencies and bounded execution, rather than advertised benchmarks alone.

## Design direction

[Bret Victor’s linked-representations examples](https://worrydream.com/MediaForThinkingTheUnthinkable/) suggest that the useful interaction is connecting source, structure and visible result. Keep source authoritative; selection in one view should explain the corresponding objects in the other. Do not force all formats into generic boxes and arrows or replace rich formats with an undocumented subset.

Existing shared Markdown parsing/rendering lives in `little-clip-render::markdown`, with native screen painting in `cards::text`. `clip-studios::text_preview::markdown` consumes the same parsed lines. File-manager text sessions already own guarded edits/saves. Extend these mechanisms rather than creating a second file-save or Markdown implementation. Native renderers may need disposable host workers if their runtime cannot be cooperatively bounded; a compiled worker is distinct from an independently installed tool.

## Updated editing and clipboard priorities

The graphical-editor priorities are **WireViz, WaveDrom, Mermaid and Excalidraw**.
Excalidraw needs actual canvas editing, not just a document preview. For the other
formats, retain source/highlighting/live preview/errors as the baseline and add
useful direct manipulation when practical; difficult graphical editors can be
improved later. D2 source editing is sufficient for now. Keep the complete format
coverage checklist; these priorities do not remove any requested renderer.

- [x] Add best-effort recognition and editor opening for text-only clipboard
  items in the implemented families; extend signatures as new adapters land.
- [x] Reuse shared format identities, parser/renderer workers and editor sessions;
  do not create another clipboard payload or save implementation.
- [x] Use bounded, conservative content signatures, not merely punctuation or
  ambiguous words. Preserve the exact clipboard source.
- [x] Render complete recognized snippets; incomplete/unrenderable candidates
  fall back to text, with syntax highlighting when recognition permits it.
- [x] Verify ordinary prose/code, partial input, parser failure, source limits,
  repeated clipboard changes, and opening an unsaved snippet in its editor.

## Front matter decision and validation

Compared [gray_matter](https://docs.rs/gray_matter/latest/gray_matter/) (multiple engines, owned custom Pod), [frontmatter-gen](https://github.com/sebastienrousseau/frontmatter-gen) (broader generation/SSG facilities), and [frontmatter-lossless](https://github.com/osdlc/osdlc) (YAML framing only). The shared core reader needs original byte offsets, TOML/JSON delimiters, bare JSON objects, bounded diagnostics and bounded YAML expansion. Its small framing layer reuses existing Serde JSON/TOML; [serde-saphyr](https://docs.rs/serde-saphyr/2.1.1/serde_saphyr/) supplies YAML with explicit depth/event/node/alias/scalar budgets and includes disabled. Deserialize-only features avoid serializer/comment facilities. License: MIT OR Apache-2.0; four newly locked packages, no executable dependency.

The large viewer exposes expandable metadata fields; screen cards show format and a compact field summary. Source remains unchanged. Framing failures leave the body visible and closed malformed metadata reports its error. Parsing is capped at 64 KiB; presentation bounds field count and text length.

Validation: five core metadata tests; six studio Markdown/source tests and seven render Markdown tests pass. Targeted strict Clippy for core/render/studios passes. Tests cover CRLF/BOM/Unicode byte offsets, nested JSON strings, malformed/unclosed/oversized framing, partial delimiters, YAML depth/aliases/includes and screen-prefix parsing after metadata longer than 4 KiB. Full visual review and workspace validation remain in the final checklist.

## Build compatibility

The added native parser dependency graph exposes an ambiguous `ImageBuffer::as_mut()` inference in the existing SAM mask blender. The buffer is explicitly typed as `&mut [u8]`; pixel behavior is unchanged. All three actual-host diagram dispatch tests compile and pass with that annotation.

## First native diagram adapters

Pikchr and SvgBob now share a compiled rendering worker inside the file manager, workbench, daemon and display diagnostic executable. It dispatches before UI/runtime/hardware startup. JSON stdin contains only syntax and source; output contains SVG, a PNG and bounded dimensions. No input/output path is accepted. Core CommandRequest owns the process group, parent-death handling, cancellation, eight-second wall timeout, six CPU seconds, 2 GiB address space and bounded stdout. SVG image resolvers are disabled; fonts come from the existing bundled fallback. The source cap is 256 KiB; SvgBob additionally bounds the cell plane to 256 × 128; vector structure/dimensions and raster allocation are bounded.

The large Markdown viewer prepares only visible diagrams on cancellable/debounced jobs. Hidden work is cancelled and GPU images are released while retaining layout geometry. Standalone `.pikchr` / `.svgbob` sessions use the existing guarded editor/save path with highlighted source, live preview and diagnostics. Small-screen cards render fenced diagrams and standalone source, reuse up to eight prepared images and retry transient failures after ten seconds. A visible fenced diagram may read beyond the normal text prefix to obtain its complete source within the diagram budget. Copied `.md` files now use rendered Markdown instead of accidentally falling through to highlighted raw text.

### Pikchr comparison and editor direction

Compared [pikchr-rs 0.1.0](https://github.com/gzivdo/pikchr-rs) (MIT; safe Rust grammar/geometry port, upstream corpus-parity tests) with [pikchr 0.1.4](https://docs.rs/pikchr/0.1.4/pikchr/) (bundled upstream C, mature token budget, FFI). Source inspection found the latter’s destructor tests for a null pointer before freeing, leaving a successful allocation unfreed. The Rust port is selected, pinned and contained in the bounded worker because macro depth alone does not bound expanded work. This avoids adding application unsafe code or an installed renderer. Normal source errors preserve the engine diagnostic.

The official [Pikchr editor](https://pikchr.org/home/help/www/pikchrshow) is a source/preview application rather than a native bidirectional editor. Its client-side rendering and immediate diagnostics are useful interaction references. Numeric scrubbing and object/source correspondence are still pending; no editor implementation was copied.

### SvgBob comparison and editor direction

Compared [SvgBob 0.7.6](https://github.com/ivanceras/svgbob) (Apache-2.0, exact target language) and [aasvg-rs](https://github.com/bearcove/aasvg-rs) (BSD-2-Clause, related Markdeep/GoAT dialect). The latter is not a substitute for SvgBob syntax, so the original native renderer is selected. Its static Sauron SVG writer pins futures 0.3.30, incompatible with the current workspace. `vendor/sauron-core` retains upstream 0.61.9 library source/license, with only the dependency constraint and one explicit return lifetime changed. DOM/browser features remain disabled; this is a static SVG node writer, not a browser engine.

[svgbob-gui](https://github.com/qqn2/svgbob-gui) (MIT, with separately licensed dependencies) and [AsciiGrid](https://github.com/mbarkhau/asciigrid) demonstrate grid editing, rectangular selections, box/arrow tools and movement/resizing while keeping ASCII authoritative. A native cell editor is a better fit than translating their browser UI frameworks. Graphical grid editing and ambiguous `.bob` content classification remain pending; only unambiguous `.svgbob` is routed automatically so far.

### Current evidence and remaining work

Native vectors and raster output for both adapters pass tests and were visually inspected with the [example files](examples/diagrams/README.md). Three worker-dispatch suites pass across all four actual host executables, including the display diagnostic, under the production address-space/CPU limits. Preview cancellation, loaded source syntaxes, guarded diagram saves, complete diagram fences and actual small-screen `.md` / standalone routing have targeted tests. Workspace strict Clippy passes. These results do not claim graphical editing or completion of the remaining format families.

The architecture comparison flags additional complexity: native protocol/parser/raster validation and preview ownership add necessary domain behavior, and the source-vendored Sauron writer accounts for most of the roughly 9,800 newly measured lines (including upstream DOM code that is not enabled). No app dependency cycle was added; reported Python cycle changes only reorder members of untouched scripts. Diagram presentation adds branches to existing text painting; final review must keep these bounded as more adapters arrive rather than duplicating format-specific painting.

First-adapter validation: `cargo test -p little-clip-render -p clip-studios -p clip-file-manager -j4 --lib -- --test-threads=1` passes 1,688 tests (19 ignored); the subsequent viewer-route test and metadata doctest pass. Actual worker integration tests pass across all four executables under the same address-space/CPU limits. `cargo fmt --all --check`, `git diff --check` and final `cargo clippy --workspace --all-targets -j4 -- -D warnings` pass. User-local `.cargo/config.toml` remains excluded. Approximately 125 GiB remains available; no additional target directory or cleanup sweep was used.

## Mermaid decision and compatibility

Compared [merman 0.10.0](https://docs.rs/merman/0.10.0/merman/),
[mermaid-rs-renderer 0.3.1](https://github.com/1jehuang/mermaid-rs-renderer),
and [ariel-rs 0.4](https://docs.rs/ariel-rs/0.4.0/ariel_rs/). The latter two
provide lighter native SVG paths. Merman is selected for its broad diagram
families, explicit operation controls and native-safe SVG pipeline, and available
parser-backed source/editor facts for subsequent direct editing. The editor
feature is not compiled until used. Default features retain its native ELK and
Cytoscape layout engines; no JS runtime or browser engine is used.

The render request uses constrained resources, a five-second operation deadline
inside the existing bounded worker, and SVG-only labels rather than HTML
`foreignObject`. Our existing raster stage resolves text against bundled fonts;
it does not claim the exported SVG embeds its fonts. Percentage-sized root SVGs
now derive their viewport from the validated `viewBox`, preserving aspect ratio.
System clock/timezone adapters preserve local Gantt "today" markers rather than
using a fixed test epoch. Visible previews and the small-screen cache refresh
Mermaid results after one minute. Failed refreshes are paced too.

`.mermaid` selects the native source editor. `.mmd` selects it only for a strong
Mermaid header, including after metadata and initialization/comments. Ordinary
MultiMarkdown remains Markdown. Once chosen, the session retains its syntax
while editing an incomplete header. Standalone source saves still use FileSource;
no second save mechanism was introduced.

Merman's MIT/Apache-2.0 components use the MIT option. The translated ELK
implementation is a separate, unmodified EPL-2.0 dependency. Exact license texts,
release/source links and upstream derivation points are retained under
[`little-clip-render/third_party`](../crates/little-clip-render/third_party/diagram-notices.md)
and embedded in every diagram-capable executable (`--diagram-licenses`).

Merman requires pulldown-cmark >=0.13.6; rdocx 0.16.4 pins exactly 0.13.4. Cargo's
attempted mixed 0.16.0/0.16.4 resolution does not compile. Updating the direct
rdocx requirement to 0.17 aligns its companion crates and exact pulldown version.
Eleven core document tests pass with that alignment; no document API call sites
needed changes. This dependency update is part of the renderer integration.

For editor design, compared [Archyne](https://github.com/archyne/archyne)
(MIT; source-preserving Mermaid canvas), the
[Mermaid Gantt editor](https://github.com/yyamamot/mermaid-gantt-editor)
(MIT; task/date/dependency edits), and the
[official live editor](https://github.com/mermaid-js/mermaid-live-editor)
(MIT; source plus SVG). Object/source selection and semantic edits are better
follow-ups than pretending automatic-layout formats persist arbitrary positions.
No editor code was copied. Graphical interaction remains pending.

Architecture review still reports the previously recorded vendor growth and
necessary parser/protocol/preview lifecycle complexity. The new raster branches
validate responsive viewports before allocation; periodic preview branches keep
calendar-dependent diagrams fresh. They reuse the existing renderer, preview
owner and cache rather than adding format-specific UI implementations. Test
harness duplication across hosts verifies independently linked executables;
source-sharing through crate-crossing `#[path]` remains prohibited.

## WaveDrom research (renderer and basic signal painter integrated)

Compared the native [wavedrom 0.1.0](https://github.com/coastalwhite/wavedrom-rs)
renderer with the [canonical JS engine](https://github.com/wavedrom/wavedrom)
and mdbook wrappers. The native crate renders timing signals, grouping, edges,
headers and markers without a runtime dependency. Its published API and actual
schema need comparison with current WaveJSON, particularly register/logic
figures; wrappers and related bitfield libraries are not substitutes for missing
syntax. The `embed_font` feature brings a Helvetica asset: prefer existing
bundled font handling unless that asset's license is verified.

Compared the source/live official editor and [WavePaint](https://github.com/lodigic/WavePaint)
(MIT application; third-party components retain their licenses). WavePaint's
clickable signal cells, bus data, edge markers and undo model are useful native
editor directions. Its browser/CDN preview cannot be integrated directly under
the no-browser requirement. No code was ported.

Mermaid validation: targeted tests cover four diagram families, native raster
output, responsive viewport sizing, `.mmd` classification and retained source
syntax, guarded saves and both small-screen routing variants. The actual-host
integration suites pass for file manager, workbench, daemon and display diagnostic
under production limits; file-manager tests also exercise malformed Mermaid and
embedded license/source notices. The example PNG and a dark-theme variant were
visually inspected. Workspace Clippy with `--all-targets -- -D warnings` passes.
`cargo test -p little-clip-render -p little-clip-core -p clip-studios -p clip-file-manager -j4 --lib -- --test-threads=1` passes 2,009 tests (22 ignored).
`cargo fmt --all --check` and `git diff --check` pass. Approximately 118 GiB
remains free, with no separate target tree or cleanup sweep. User-local Cargo
build settings remain uncommitted.

Further WaveDrom source inspection found compatibility limitations in the native
0.1.0 crate: `config.skin` is parsed but ignored, malformed edges are discarded,
and fractional periods are rounded upward. Register and logic figures are not
represented in its WaveJson model. Do not silently label this a fully compatible
adapter. Compare a corrected native port with the canonical engine in a small
embedded JS runtime (without DOM/browser), then verify current WaveJSON fixtures
before choosing.

Math follow-up: mathra 1.12.0 is a native AST/layout/SVG candidate with explicit
strict errors, a STIX Two Math asset (OFL-1.1), and no source-driven file loader.
Its base SVG API can use our existing rasterizer without its optional PNG stack.
Compare latex-rust 2.1.1 (MSRV 1.76); its 3.0.x releases require Rust 1.99, beyond
the installed 1.98.1. retex-svg 1.9.0 is a PDF-to-SVG component, not itself a
LaTeX parser, so it is not an equivalent standalone math renderer. The subsequent
selection and integration status are recorded below.

## Lini integration

Compared the original [Lini 1.1.0 compiler](https://github.com/monfa-red/lini)
with [mdbook-lini](https://github.com/monfa-red/mdbook-lini), its WASM-based
integrations, and implementing the language from its specification. The wrappers
all use the same compiler; no independent compatible native renderer was found.
The original Rust library provides the full syntax/layout implementation rather
than a new approximate subset. `hrdf-parser` from the initial search is unrelated
and was discarded. Library-only calls are used; no CLI, local HTTP service,
JavaScript runtime or browser engine is invoked.

The worker selects `static_mode: true` (baked CSS values and outlined labels) and
`no_fs: true`, with no supplied assets, base directory or asset root. Local image
paths are refused before lookup; HTTP/data image references remain inert because
the common SVG rasterizer disables image resolution. Their omission now produces
a visible warning rather than a silently incomplete image. Support for bounded
embedded images remains future work, relevant to the richer scene formats too.
The existing source/vector/raster/process limits apply unchanged.

The shared worker result retains bounded non-fatal warnings alongside pixels.
The large preview displays them below the image; the small-screen cache keeps
warnings with its thumbnail and reserves one compact status line. Lini warning
locations use source line/column, including near-miss auto-created objects.
At most fifteen detailed Lini warnings plus an overflow summary are returned;
one raster limitation warning may be added. Each is at most 1,024 characters.
Existing engines produce empty warning lists when there are no limitations.

`.lini` uses the shared guarded source editor and a bundled source grammar;
`lini` fences render in both Markdown paths. Save/Save As, unsaved drafts,
cancellation, last-valid large previews and offscreen texture release reuse their
existing owners. The [pipeline example](examples/diagrams/pipeline.lini) uses
three colored nodes; a bar chart was also checked through the actual worker.

The existing Lini playground and serve editor use highlighted source, a live SVG
result and diagnostics in resizable panes. The Rust crate exposes warnings and
suggested source edits but not its parsed/layout object tree as a public API.
Source-aware number/label manipulation is a useful later addition; recreating its
whole private tree or embedding its HTML editor is not part of this adapter.
No upstream editor code was copied. Graphical manipulation remains pending.

Lini is MIT; its default font subsets are OFL-1.1 and Phosphor icon geometry is
MIT. Full notices are embedded by `--diagram-licenses`, including the copyright
records verified from bundled font name tables. Our code does not alter the
fonts. License/source records are in
[the shared notice file](../crates/little-clip-render/third_party/diagram-notices.md).
Only the Lini package is newly locked; its dependencies already existed.

Architecture comparison against `native-diagrams-after-mermaid` reports roughly
221 new code lines, five functions and one Rust file. New branches carry bounded
warnings through the existing renderer/cache/preview owners, not parallel Lini
UI or save implementations. Reported host-test clone growth reflects the same
fixture/protocol exercised in independently linked executables. Reported Python
cycles reorder the members of untouched script cycles; no application cycle was
added. No extra abstraction or source vendoring was introduced for the compiler.

Validation: targeted renderer/source/save/routing tests and actual-host
integration suites pass. Native flowchart and bar-chart rasters were visually
inspected. Workspace strict Clippy passes. `cargo test -p little-clip-render -p clip-studios -p clip-file-manager -j4 --lib -- --test-threads=1` passes 1,696 tests (19 ignored).
The final file-manager integration run also verifies Lini/font notices. Formatting
and diff whitespace checks pass. Approximately 116 GiB remains free.

Source errors, no-filesystem asset handling, warning retention and
overflow summaries have targeted regression coverage.

## Graphviz next-step research

Downloaded and inspected `graphviz-pure` 1.2.0 (EPL-2.0), `layout-rs` 0.1.3
(MIT), and `graphviz-rust` 0.9.9 (its actual license file is MIT, although its
registry license metadata is non-standard). The first provides a native port of
the eight Graphviz engines and explicit diagnostics; the second has a public
DOT AST and its own layout; the third defaults to spawning installed Graphviz
and is not equivalent to a compiled renderer. No adapter has been selected yet.
Review file/resource loaders, layout limits, source correspondence and fidelity
before selecting. The high-level `render_dot` renders only the first graph:
do not silently discard additional graphs in a source file or fence.

Compared [Graphviz Visual Editor](https://github.com/magjac/graphviz-visual-editor)
and [d3-graphviz](https://github.com/magjac/d3-graphviz) as interaction references:
node/edge insertion, selection linked to source, style changes and pan/zoom are
more useful than embedding their browser frameworks. Their current license files
still need checking before considering any port; no editor code has been copied.

## Graphviz integration and safeguards

Selected [graphviz-pure 1.2.0](https://github.com/graphviz-pure/graphviz-pure)
over [layout-rs 0.1.3](https://crates.io/crates/layout-rs), which has its own
layout/shape coverage, and [graphviz-rust 0.9.9](https://crates.io/crates/graphviz-rust),
whose default execution feature launches an installed `dot`. The native port
preserves the DOT language and eight Graphviz layout engines without C/FFI,
installed executables or a browser. It adds six locked component packages, not
its CLI, parallel mode, math tracing or alternate raster/PDF stack.

The helper sets the library's file-loading denial switch directly before any
layout, without changing environment variables. Image, HTML IMG and shapefile
paths are refused before lookup. The SVG-inline output avoids document DTDs and
stylesheet processing; its omitted namespace declarations are materialized for
standalone parsing. Typed native-parser preflight bounds the stream to sixteen
graphs before layout and warns about external stylesheets. The facade reparses
for normal layout/render diagnostics; temporary preflight models are dropped.
BOM normalization affects rendering only, leaving saved source intact. Linked
image/shape and stylesheet resources remain an explicit preview limitation.

Every graph is included; invalid syntax in later graphs rejects the request
rather than returning an apparently complete first graph. The existing SVG parser
now exposes its validated tree/font-materialization stage for reuse. Each graph
is outlined and serialized with an isolated ID prefix by usvg before vertical
composition, so gradients, clipping and label references cannot cross graphs.
Combined vector size, geometry, raster and process limits remain enforced. This
reuses the established renderer and avoids another XML/SVG rewriting engine.

Tinos, Arimo and Cousine assets (4.4 MiB, OFL-1.1) come unchanged from the port's
raster package; byte hashes and license texts accompany the canonical assets.
Core `font_assets` owns their bytes. A separate Graphviz font database adds
standard Times/Helvetica/Courier family aliases and retains existing Noto
glyph fallbacks, without system-font discovery. Visual review caught a Helvetica
to serif fallback; the aliases and a font-resolution test fix it. This does not
claim pixel identity with platform Pango/Cairo renderers or perfect matching of
arbitrary user font families.

The EPL-2.0 implementation boundary remains the unmodified dependency packages.
Original Graphviz/CORE-MATH/Expat notices, corresponding-source release links,
and font licenses are retained and printed by `--diagram-licenses`. No editor
code was copied. Graphviz Visual Editor's actual license is MIT; d3-graphviz is
BSD-3-Clause. Their source-linked selection, insertion/deletion, styles and
pan/zoom are useful native interaction references. Public renderer handles are
opaque; parser models and JSON drawing output are available, while exact source
span/undo correspondence still needs implementation. Graphical editing remains
pending rather than being simulated by lossy source regeneration.

`graphviz` / `dot` fences and standalone `.gv` / `.graphviz` use the shared native
paths. Ambiguous `.dot` uses bounded header classification (comments, BOM,
optional strict, quoted/concatenated names), excluding ordinary prose and binary
Word-template routing. Source highlighting stays active during invalid drafts;
existing guarded FileSource saves and Save As rules remain. Commit `e81f030`
separately fixes content-detected diagram extensions defaulting to `edited.txt`,
including the earlier `.mmd` case, with a regression assertion.

Warning transport now has one shared count/character budget instead of an
adapter-specific copy. The bounded warning-overflow test remains; resource
limitations stay visible in both presentation sizes. No new UI/save owner or
application dependency cycle was introduced. Architecture comparison reports
small necessary growth for the adapter, format classification and test coverage.
The SVG parser's validation complexity is shared with composition rather than
duplicated. Worker fixtures and protocol/resource assertions are now owned by
`diagram::test_support`, enabled only through explicit host dev-dependencies.
Each host retains its own actual-executable test, with one fixture list for
future adapters. This removes five of the six newly reported clone groups.
The remaining net clone group is license text; required notices are retained.
Manifest declarations and unchanged viewer logic are also reported by the tool,
and unchanged Python-cycle member ordering is not a new application cycle.
The final comparison adds 423 code lines and two Rust files over the Lini
baseline, including the independent Save As fix and shared test helper.

Validation: all eight engines, multi-graph gradient IDs, late syntax errors,
file/stylesheet denial, stream limits, BOMs, source grammars, guarded saves and
big/small dispatch have targeted tests. The affected core/render/studio/file-
manager suites pass 2,017 tests (22 ignored); the subsequent font-alias regression
and final targeted suite pass. Actual-host tests pass for file manager,
workbench, daemon and display diagnostic under production memory/CPU limits.
Flowchart, multi-graph and HTML-table PNGs were visually inspected; the corrected
Helvetica rendering was checked again. Final workspace strict Clippy passes.

Disk maintenance: after checking Cargo/compiler idleness, removed 137 older
cached executables, preserving root-linked binaries, running executable inodes,
recent variants and library dependencies. Available space rose from roughly
111 to 136 GiB (logical removed file sizes were 89.3 GiB; filesystem savings are
smaller). No extra target tree or dependency-cache sweep was used. User-local
Cargo build settings remain uncommitted.


## Native math backend (Markdown presentation pending)

Compared [Mathra 1.12.0](https://github.com/matheusmota-dev-br/mathra),
[latex-rust 2.1.1](https://crates.io/crates/latex-rust/2.1.1) and
KaTeX's JavaScript/HTML approach. Mathra is selected: its compiled AST/layout/SVG
pipeline exposes baseline and em measurements, strict diagnostics, accessible
SVG metadata and outlined glyphs. It can also share the math engine with later
TikZ support. The older latex-rust release fits our compiler and has an exact
rational layout model, but its optional egui backend targets a different egui
version; current releases require a newer compiler. KaTeX would require an
additional runtime and HTML/MathML layout. retex-svg is not a math parser.

The unmodified Mathra dependency disables optional chemistry, alternate fonts
and PNG features. The existing bounded native helper, SVG validation/rasterizer,
source/resource limits, cancellation and real-host fixtures are reused. Equations
carry validated em width/height/baseline metrics and transparent pixels; requested
text colors and explicit LaTeX colors coexist. SVG uses a physical outer viewport
so font-unit coordinates do not require relaxing shared dimension limits. Source
commands do not select filesystem resources. MIT and STIX font OFL notices are
embedded in every diagram-capable executable.

[MathLive](https://github.com/arnog/mathlive) is a useful MIT-licensed structured
math editor reference (selection of expressions, keyboard navigation, insertion
palette, source synchronization). It is a DOM component, not a native widget we
can import. No editor code is copied. Native direct manipulation and source
correspondence remain follow-up work.

This commit supplies the renderer backend only: Markdown dollar-delimiter
parsing, inline baseline-aware layout, display blocks and theme-aware preview
cache keys still need integration in **both** output surfaces before checking off
math support. General `.tex` documents are not classified as equations; the math
engine does not implement whole-document TeX.

Backend validation: 18 renderer/diagram tests pass, including four math tests
for baseline/empty/phantom layouts, transparency, theme/explicit colors, malformed
commands and invalid worker metrics. Real-host dispatch tests pass for file
manager, workbench, daemon and test_screen; the license query includes Mathra
and STIX notices. Strict Clippy passes for core/render across all targets. Actual
host PNGs for an infinite sum and a matrix were visually inspected. Architecture
comparison adds no high-complexity application function or cycle: the two new
clone groups are the required verbatim MIT/OFL license texts; reported Python
cycle changes contain the same pre-existing members in another order.


## Markdown math presentation

Inline `$...$` and display `$$...$$` now use the native backend in the large text
viewer and little-screen renderer. The existing CommonMark dependency handles
inline escapes, code exclusion, emphasis and links. A separate bounded display
framing pass preserves formulas whose lines look like Markdown lists, rules or
fences; unfinished blocks show an explicit diagnostic and never render a partial
formula. The compact preview may extend its ordinary prefix through a real
closing delimiter, up to the same 256 KiB source budget as other native previews.

Both surfaces use equation em dimensions and a shared prose/equation baseline,
reserving ascent and descent for fractions and matrices. Standalone display
rows are centered at natural text size, fitting available bounds rather than
stretching every equation to the panel width. Explicit LaTeX colors remain
intact; transparent rasters and foreground-aware cache/source identities adapt
to the surrounding theme. Lists and quotes retain their native presentation.

The desktop path reuses the existing preview task/debounce/cancellation owner.
Only a visible equation can admit or poll its render task, after wrapping has
located its actual rectangle; hidden previews release textures and keep their
geometry. Errors are visible in the document, and equation hover exposes bounded
source text. The small renderer shares its existing thumbnail cache, now bounded
by both 32 entries and 48 MiB of retained pixel data to accommodate dense rows of
small formulas. No second decoder, browser, TeX installation or external service
is added.

Validation: the four affected library suites pass with 2,029 tests and 22 ignored
when serialized. The initial parallel file-manager run hit shared admission
contention; its full serialized rerun passed. All four executable dispatch checks
and embedded-license checks pass, strict workspace/all-target Clippy passes, and
formatting/whitespace checks pass. Additional checks cover exact document-route
painting, theme cache identity, inline emphasis/code/links, multiline display
framing, unclosed/budget-limited source, natural-size/centered desktop image
bounds and clearance before the following paragraph. A final Markdown-focused
rerun verifies the consolidated heading/rule parser. Actual native equation PNGs
and a font-loaded little-screen document render were inspected visually.

Architecture comparison against `native-math-backend` reduces clone groups by
one, keeps maximum/average complexity unchanged, and reduces `parse_body` from
CCN 17 to 15 and the original preview `show` from 21 to 11. Reported remaining
clones are unchanged portions of touched files; reported Python cycles contain
the same members in a different order. No new application cycle is introduced.

See `docs/examples/diagrams/math.md`. General TeX documents and structured math
editing are still separate work, as are exact source correspondence and the
remaining diagram families. Compact overflow badges still count physical source
lines; precise block/source accounting should accompany source correspondence.
The overall goal remains incomplete.


## Native D2 engine

Compared [d2-rs 0.1.7](https://github.com/nelsonjchen/d2-rs),
[d2-render 0.1.0](https://crates.io/crates/d2-render/0.1.0), the official Go D2
library/CLI, and graphs-tui. d2-render's default native implementation is a
smaller subset (its full backend invokes the CLI); graphs-tui is AGPL-3.0 and
is not incorporated. The original Go engine would require a Go runtime/FFI
boundary or an external tool. d2-rs supplies the full native parser, IR, font
measurement, Dagre/ELK/TALA layouts, target model and resolved scene. This avoids
inventing another partial D2 parser or duplicating its source-editing machinery.

The published wasm facade already contains the complete source-to-layout
pipeline. Its small local MPL patch exposes that same implementation with
ordinary Rust errors, without constructing JavaScript exceptions. It compiles
as an rlib: no JavaScript, WASM runtime, DOM or browser runs in the application.
A second manifest patch accepts compatible binding versions so existing locked
packages stay unchanged. User-requested history separation is preserved:
`4239986` imports unchanged upstream sources/notices (131 package files verified
byte-for-byte); `81f28ea` contains our API/manifest modifications and wiring.
Their PATCHES.md files record changes and corresponding published source.

The application adapter uses an explicit unavailable-image resolver and bounded,
bundled-only font resolution. It does not call the CLI PNG exporter, whose
default resource resolver can open files. Source imports have no host opener.
D2's typed scene handles text, Markdown and TeX labels natively; feeding their
SVG foreignObject labels to an ordinary SVG rasterizer would drop content.
SVG structure and dimensions still pass the existing shared safety checks;
scene construction, glyphs, paths, fonts, raster pixels and auxiliary work have
finite budgets inside the existing owned/cancellable helper. Images show the
engine's placeholder with a warning rather than resolving their source URLs.

Renderer validation: 23 diagram tests pass, including all three D2 engines,
class/SQL shapes, sequence/grid layouts, Markdown/TeX labels, sketch mode,
theme overrides, valid-but-denied local D2 imports, a valid-but-denied red PNG,
inert remote image references, syntax/configuration failures and a large diagram
fitted to the raster bounds. Root flat-board previews are implemented;
isometric output reports a diagnostic. Board selection and interactive editing
remain separate work. Standalone highlighting/save and executable-host checks
are being verified before claiming the integration complete.

Editor research: [Daedalus](https://github.com/Sotilrac/daedalus) is MPL-2.0,
with useful grid snapping, endpoint placement, pan/zoom and reversible layout
comparison. Its React/Tauri UI stores layout in a separate sidecar rather than
editing D2 source, so importing it would not meet this application's native UI
and source-correspondence goals. [D2 Canvas](https://github.com/ArisMontclair/d2-canvas)
illustrates a node palette, two-node connection gesture and source/canvas sync;
its README's open-source claim is not a verified license grant, and no code is
copied. D2 Studio is a design reference, not an imported editor. The selected
native d2-rs-editor library already provides source edits, references and
reconnect/move/rename operations without an LSP transport; prefer those APIs for
future direct manipulation rather than translating a web editor's parser.

Whole-tree architecture measurements include the unchanged upstream source
imports: 118 added measured files, about 35.7k lines, and upstream test/renderer
clone/complexity warnings. Do not remove upstream sources/tests just to hide
these metrics. A separate measurement of the application adapter reports 256
code lines, eight functions, maximum CCN 15 and no clones. The structural tool
cannot analyze a single-file target; the whole-tree report supplies structural
context. The full feature and editing goal remains active.


### D2 editor and host integration

D2 fences and `.d2` sources now share the existing native preview owner in the
large viewer and little-screen renderer. Standalone sources load the bundled D2
syntax definition and retain the existing guarded editor/atomic save/unused
Save As behavior; the session's live source remains authoritative. Added sample
`docs/examples/diagrams/pipeline.d2` also appears as a Markdown fence in the
example README. No new source-save or resource-loading mechanism is introduced.

Final targeted checks: 32 integration tests pass (file-manager save/draft cases,
shared syntax definitions and ownership, core format classification, renderer
routing and native engine tests). Real dispatch and license queries pass across
all four executable hosts; an initial missing upstream-notice printout was
caught and fixed in `4ac3611`. Strict workspace/all-target Clippy and formatting
checks pass. Actual file-manager worker PNGs for the colored pipeline,
Markdown/TeX labels and a sequence diagram were visually inspected. Free disk
space remains about 128 GiB; existing locked package versions were retained.

The library still emits layout compatibility notes despite stronger README
parity claims. The application preserves the user-relevant limitation as
“layout may differ from the reference renderer,” while omitting a purely
informational sequence-adapter identity. It makes no byte-for-byte fidelity
claim. Imported images remain unavailable placeholders with a warning; source
imports report an error. Isometric output, selecting child boards, exact source
correspondence and native graphical operations remain open work. These are not
silently treated as completed by this integration.

## BPMN decision and implementation

Compared [bpmn-svg](https://crates.io/crates/bpmn-svg/0.1.0), which explicitly
ignores authored BPMN-DI, with [BPMN Kit](https://github.com/bpmnkit/bpmnkit)'s
native 0.2.1 model, layout and SVG crates. BPMN Kit preserves authored coordinates,
labels, pools/lanes and element geometry; it is the stronger foundation for later
hit-testing. SVG rendering reuses the application's bundled fonts, rasterizer,
worker protocol and limits; no optional font bundle, extra rasterizer, browser
engine or BPMN execution runtime is enabled. The MIT license is retained verbatim
in a separate commit before application integration.

The [BPMNKit editor](https://bpmnkit.com) demonstrates palette/property/canvas
interaction but uses browser UI. [bpmn-js](https://github.com/bpmn-io/bpmn-js) is
another mature graphical reference; its bpmn.io license has a visible watermark
requirement and must not be treated as plain MIT. No browser editor code is
copied. Native direct manipulation is optional after the newly prioritized editors.

Standalone `.bpmn`/`.bpmn2` and Markdown `bpmn`/`bpmn2` fences use the same native
engine. Generic `.xml` is recognized only by a bounded, namespace-resolved BPMN
`definitions` root, so ordinary XML is not claimed. Authored diagram planes are
all rendered (up to sixteen), with isolated SVG reference IDs. Missing shape
layouts use automatic layout and disclose that choice; missing displayed geometry
is reported. Automatic layout currently uses the first collaboration. The upstream
parser requires a definitions ID; a missing root ID is represented as empty only
in its private parser input, preserving the original editable source. Namespace and finite-geometry validation runs
before semantic parsing, DTDs are rejected, and vendor extension trees stay inert.
Color attributes are resolved by namespace rather than assumed prefix names.
Edits/save use the original XML text, not a normalizing model serialization.


Validation: 40 focused core/render/studio/file-manager tests pass, including seven
BPMN cases, XML classification, Markdown/screen routing, highlighting, draft edits
and guarded saves. All four independently linked host worker checks and license
output pass under the production resource limits. Workspace/all-target strict
Clippy and formatting pass. A rendered authored-layout example was inspected;
this caught cropped external labels. Shared multi-SVG composition now fits actual
outlined glyph bounds before stacking, with the existing viewport budget, fixing
that clipping for BPMN and retaining Graphviz coverage.

Architecture comparison against `native-diagrams-after-d2` reports 12 fewer clone
groups and unchanged maximum complexity. The initial oversized preflight and
render orchestration were separated into geometry validation and parser adaptation
responsibilities; no new function exceeds the CCN threshold. The tool still flags
unchanged handler/test bodies in files that gained only registry/fixture entries,
and repeated MIT license text. Those are not new application mechanisms; the
verbatim license is intentionally retained. Existing widget/callback bodies were
not rewritten to improve the score. Graphviz and BPMN share SVG composition
rather than duplicating outlining, reference isolation and viewport limits.


## Clipboard recognition checkpoint

The little-screen text path now recognizes distinctive headers for the seven
implemented non-math families: Mermaid, DOT, BPMN, Pikchr, Lini, D2 and SvgBob.
The shared core hint is capped at 256 KiB and never executes a parser or changes
the source. It deliberately leaves ambiguous edge-only D2 and bare equations as
text. These are hints: incomplete input, worker/decoder failure and invalid source
fall back to the existing highlighted text path. Actual file extensions still
select their source format and explicit diagnostics. Content-derived diagram-looking clipboard
titles cannot force native rendering; existing programming-language title hints
remain in use so code is not mistaken for a table. Copied Markdown containing
fenced diagrams uses the existing Markdown route too.

Both raster and desktop syntax inventories now use the same builder and bundled
diagram grammars, avoiding a second incomplete highlighter list. Source painting
is separate from presentation classification; that reduces the old dispatcher's
CCN from 24 to 15 while retaining its existing table/unit/network/time/model routes.
Architecture comparison has one fewer clone group overall; remaining flagged
clones are unchanged bodies in touched files. No new high-complexity function is
reported. Clipboard editor routing and the remaining format families are still
open; recognition must expand alongside each new adapter.

Validation for this checkpoint: all 71 render-library tests pass (one existing
ignored), plus four core diagram tests and fourteen core syntax tests. Strict
Clippy for core/render/studios passes. Native diagram engines were unchanged;
existing native-worker fixtures remain the parser/render compatibility checks.


The guarded text/diagram source session now lives in `clip-studios::text_editor`;
the file manager reexports that owner. Its source editor, preview, draft/save
state, atomic FileSource writes and four tests moved together. Filename-address
validation and its byte/path test moved to core beside the shared address decoder;
file-manager naming reuses that same function. Job polling uses core's existing
nonblocking delivery helper with no response deadline for accepted writes.
This preparatory commit does not yet route clipboard text to the editor.
Validation: all four relocated editor tests and the core filename test pass;
file-manager all-target checks and workspace/all-target strict Clippy pass.


## Opening and delivering clipboard sources

Recognized clipboard diagrams now open the shared source editor on the existing
background text-load path. Ordinary/oversized/unrecognized text retains the
normal text viewer, and raw diagrams take precedence over delimiter-based table
inference. The editable source retains the original BOM and line endings; no
surrogate file is created. The existing source/live-preview/error presentation is
used for incomplete candidates too, keeping their text editable when rendering
fails. File-manager navigation still belongs to its own crate.

Clipboard Save opens Save As with an existing output directory and unused filename;
first creation uses core's held-directory atomic no-overwrite writer. Subsequent
writes retain FileSource identity checks. Save As decodes reversible directory and
filename addresses, and saved identity/path come from the installed handle. Copy
source uses core's clipboard transport on an owned background job. It publishes the
captured text, not later keystrokes. Successful clipboard delivery advances that
snapshot/preview; copying a file-backed draft does not mark the disk file saved.
Failures retain drafts and report an error, including completion failures received
while a close was pending. Accepted writes/copies retain unbounded delivery owners
and block close/replacement until completion. Dirty sessions offer save/discard/
keep-editing; Escape dismisses nested dialogs before leaving the editor.

Host lifecycle fixtures use the studio's explicit `test-support` copy gate rather
than touching the real system clipboard. Tests cover byte retention, first-save
policy, existing destinations, source identity, queued/cancelled text loads,
changed clipboard snapshots, owned delivery, newer edits during copying, copy
failure, file-draft cleanliness, nested Escape and window-close cancellation.
Graphical editing priorities and the complete unimplemented-format list remain
unchanged. This checkpoint completes the clipboard mechanism for the seven
current non-math native families, not the broader diagram goal.


Final clipboard checks: ten shared editor tests and all 187 workbench unit tests
pass (three existing opt-in tests ignored); workspace/all-target strict Clippy
passes. Earlier clipboard render and syntax checks remain green. The completed
diff architecture comparison has no new high-complexity function and retains the
source-painter simplification. Its one additional clone group is test/guard scaffolding,
not another source renderer, save implementation or clipboard transport. Shared
headless-editor fixtures removed the larger duplicated test setup. The additional
whole-tree measurement request did not return and was ended; the diff comparison
completed normally and supplies the before/after evidence. No result from the
unfinished request is treated as successful measurement.

The final file-manager viewer pass also succeeds (24 tests), as do all four
actual-host worker checks, license output and formatting. No new parser library,
web engine, clipboard payload type or unguarded save path was introduced.

## WaveDrom renderer and source integration

Selected the canonical WaveDrom 3.8.0 engine over the published native 0.1.0
crate and its current Rust main branch. Both native versions round fractional
periods; neither covers the canonical logic-assignment family, and the published
crate ignores requested skins. The canonical `renderAny` API runs without a DOM.
It is embedded in compiled QuickJS (`rquickjs` 0.14), with no host filesystem,
network, dynamic loader or browser bindings. Trusted bundled code alone is
evaluated; JSON5 source becomes a data argument after bounded Rust parsing.
No installed JavaScript runtime is required.

Pristine npm source and dependency notices were committed before integration.
The npm archive SHA-512 and upstream git revision are recorded in the bundled
WaveDrom provenance notice. Existing OFL Arimo/Tinos/Cousine metrics are shared
with Graphviz; no upstream Helvetica asset is imported. SVG is outlined and
isolated through the existing composition/raster path.

Signal, register and logic figures, all six bundled skins and non-clock
fractional periods render through the shared disposable worker. Source size,
tokens, depth, cells and finite numeric inputs are bounded before layout.
QuickJS has a 128 MiB heap, bounded stack and three-second interrupt; the existing
outer worker limits remain in force. External skin requests fail explicitly.

Compared `json5`, `json-five`, `json5format` and `jsonc-parser`: the existing
JSON5 semantic parser and `json-five` lexer preserve actual JSON5 syntax while
allowing pre-deserialization bounds. Generic JSON5 receives its own shared
syntax grammar; WaveJSON aliases receive field highlighting. `.json`/`.json5`
classification and clipboard inference require distinctive complete structures.
Explicit `.wavejson`/`.wavedrom` aliases keep invalid drafts in the editor.
Markdown uses `wavedrom` or `wavejson` fences. Saving, copying, worker ownership
and failed-inference fallback reuse the shared editor and preview mechanisms.

The architecture comparison against `native-diagrams-after-clipboard` reports
extra clones/complexity in the retained pristine upstream sources and bundles.
Those imports are necessary provenance, not newly duplicated application logic.
An application-side clipboard-title branch was separated into its existing
language-hint responsibility to keep dispatch complexity from increasing.
Graphical timing editing is still the next deliverable; these source/live
previews do not claim to be a graphical editor.

## Direct timing editing

WaveDrom timing sources gain a signal-cell painter alongside the authoritative
source and canonical rendered preview. Choose low/high/hold/unknown/high-impedance,
clock or bus state, then click or drag across a lane. Cell width is adjustable;
lane names can be edited. The canvas explicitly shows source cells rather than
claiming they are uniformly timed: fractional periods, phases, headings and all
other timing semantics remain visible in the canonical rendering below. Extended
subcycle lanes remain source-editable rather than being approximated by the painter.

A bounded background task builds a concrete JSON5 syntax model. It must reproduce
the exact source before graphical editing is admitted. A gesture changes one
string literal, preserving grouping, comments, unknown fields, CRLF/BOM, quoting
and source around the edited literal. Duplicate edited fields disable direct
edits with an explicit diagnostic. Stale models retire immediately on typing;
no old geometry can overwrite a newer draft. Parsing and rendering stay off the
UI thread. One pointer gesture commits one replacement on release, including
release outside the lane. Graphical and source edits share the existing TextEdit
undo state, and all saves/copies/close guards retain the existing editor owners.

Painted runs use WaveJSON holds rather than introducing repeated-edge glitches.
Untouched trailing holds keep their old meaning, and repainting an already matching
state is a no-op.

The first direct editor covers waveform cells and existing string lane names.
Adding lanes/groups, editing bus labels/markers/arcs, period/phase controls and
register/logic manipulation remain source operations and possible later improvements.
It is not a full graphical WaveJSON authoring environment.

Timing validation includes byte-preserving concrete-syntax checks, inline comments,
ambiguous edited fields, pointer drags with press-origin interpolation and release
outside the lane, continuous-state holds, per-session undo isolation and keyboard
undo/redo. The private desktop acceptance exercises opening a real `.json5` file,
clicking a signal cell, exact-source guarded saving, canvas undo, a second save and
returning to its folder. `scripts/diagram_timing_acceptance.py` retains screenshots
and checks the saved bytes; it uses only the existing isolated compositor runner.

The dark signal skin gets matching paper in the application's adapter: the upstream
template otherwise hardcodes white behind white strokes. A pixel test checks dark
coverage in addition to SVG structure. Vendor source remains unchanged.

Final timing checkpoint validation: `cargo test -j4 --workspace --lib --test
diagram_worker -- --test-threads=1` passes 2832 tests (33 ignored), including the
actual-host dispatch checks. Workspace `cargo clippy -j4 --workspace --all-targets
-- -D warnings`, formatting and diff checks pass. Private light and dark native
window runs both pass painting, byte-exact saves, keyboard undo and folder return;
the SPI example and dark-skin PNGs were visually inspected. About 110 GiB remains
free; no separate target directory or cleanup sweep was used. User-local Cargo
settings remain uncommitted. The full run also exposed an insertion-order-sensitive
front-matter schema hash; its separate fix canonicalizes only structured front
matter, preserving ordered prose behavior and the former default schema ordering.

## Excalidraw native canvas research

Compared published source for four independent roles/implementations:

- [excalidraw-render 0.1.0](https://github.com/sagikazarmark/excalidraw-render)
  is a genuine Rust renderer targeting Excalidraw 0.18.1, with Rough/freehand
  paths, bundled fonts, image handling, compatibility diagnostics and explicit
  semantic resource limits. Select its library SVG path, with optional PNG/PDF
  backends disabled, inside the existing disposable diagram worker. Its rendering
  snapshot is not an authoring model.
- [excalidraw-document 0.2.0](https://github.com/sagikazarmark/excaliplot)
  supplies a typed preserving scene/library model and transactional authoring.
  Duplicate keys are rejected; unknown fields, resources, tombstones, exact
  numbers and malformed entries are retained. It preserves JSON semantics,
  not original whitespace/key order/number spelling. Use this as the prospective
  scene-editing owner rather than inventing another lossy serde scene struct.
  Its Rust 1.97 minimum fits the installed 1.98.1 toolchain. The published README
  contains older repository/example paths; provenance uses the package metadata
  and its recorded VCS revision instead.
- [gpui-excalidraw 0.1.9](https://github.com/LittleGuest/gpui-excalidraw)
  is an MIT native editor with selection, shape creation, freehand, move/resize,
  rotation, snapping, bindings, undo and property panels. Its GPUI Kit dependency
  is unconditional and the library exposes its UI together with its core. It
  is a useful interaction/geometry reference; importing another GUI framework
  is unnecessary. Any source extraction/port would require a separate pristine
  import before modifications. No such code has been copied.
- [excalidraw-image 0.2.0](https://github.com/rickardp/excalidraw-image)
  is self-contained but its actual engine uses `deno_core`/V8, a JavaScript bundle,
  polyfills, async event-loop pumping and optional cache-helper subprocesses.
  This is not the same implementation tradeoff as a Rust renderer. Prefer the
  native candidate above instead of adding that runtime.

Pristine renderer/font license bundles and the document model's root MIT/Apache
licenses are retained before integration. Renderer source revision:
`2f4c461ea40ba6850e143f70f310a25a6a50f94d`; document revision:
`29392d10d9c504ec22d67f37a4dd545a63d5b371`. Font catalog version is
`0.1.0+excalidraw.0.18.1`, not a download named just `0.1.0`.

Integration must preserve actual fonts and embedded images. Native SVG carries
WOFF2 subset faces and sometimes effective aliases; the existing compositor
does not honor CSS font-face loading and intentionally disables image resolution.
Do not silently substitute the generic font database or discard scene images.
The public bundled font catalog exposes verified source/predecoded programs,
reviewed decode sizes/hashes and family names, allowing a worker-only font database
without system discovery. The existing WOFF decoder is currently file-manager
private; the renderer must not depend on that application. Trusted bundled-font
decode can call the same native decoder directly. Embedded raster admission
needs explicit size/pixel/aggregate limits; paths, network assets and executable
SVG content remain unavailable. Retain compatibility warnings.

The canvas should reuse the shared editor's guarded delivery and undo ownership,
and use worker-prepared typed scene snapshots with revision checks. Direct
manipulation needs immediate native feedback during gestures, followed by one
transactional source edit on release. Preserve unknown data; document any source
reformatting explicitly. Existing image markup/transform code is a reuse
candidate for geometry and handles, not a second file-save implementation.

The dependency spike passes `cargo check -j4 --workspace --all-targets` with
`excalidraw-render` and its font catalog pinned to 0.1.0, SVG-only rendering
features and bundled CJK faces, plus `excalidraw-document` 0.2.0. WOFF decoding
uses the same 0.2.10 library version as the existing font viewer. No GUI-framework
dependency is added. This verifies build compatibility, not scene rendering or
canvas functionality; those remain to be integrated and tested.

## Native drawing rendering foundations

The shared SVG pipeline now has an explicit embedded-raster policy. Existing
formats keep their original image blocking. The opt-in path accepts only embedded
PNG/JPEG/WebP data, with 2 MiB encoded bounds, 2048-pixel sides, an aggregate
eight-megapixel decode budget per SVG and bounded decoder allocations. Accepted
images become bounded PNGs. Paths, URLs and nested SVG payloads stay blocked;
malformed, unsupported and oversized references retain visible omission warnings.
Tests verify colored pixels survive composition and final rasterization, verify
that the former blocking policy still produces white paper, and reject actual
ordinary file paths, network references, nested SVG, malformed data URLs and
exhausted budgets. SVG viewBox admission is shared for subsequent viewport mapping.

The native renderer's computed origin was private. Its pristine published source
was imported separately before a 31-line extension exposes `SceneViewport` on
`RenderOutcome`, carrying the existing source-space origin and logical size through
the pipeline. This will anchor actual canvas coordinates without duplicating
text/rotation/frame bounds rules. The original SVG output stays unchanged.
The dependency is excluded from workspace membership because the release package
does not ship upstream qualification fixtures; application-owned integration
tests use its real public API.

Three native tests pass: translation and rotation correspondence; empty-scene
padding separate from output scaling; and the authored `drawing.excalidraw` scene
with shapes, handwriting, arrows, freehand and an embedded raster. That scene
produces no native compatibility diagnostics. Shared diagram regression tests
pass (38 renderer tests plus affected host/core checks), as does strict workspace
Clippy. Architecture comparison reports no new application function above the
complexity threshold. Source import growth is the already selected native
dependency, retained for a small reviewable extension, not new application logic.

This checkpoint supplied the rendering/coordinate foundation. The next section
records source routing and composition; the interactive canvas remains pending.


## Native drawing source integration

Excalidraw now uses the shared bounded helper, static SVG/PNG pipeline and existing
source-session save ownership. Explicit `.excalidraw` and `.excalidraw.json` files
retain their identity even for invalid drafts; ordinary JSON and clipboard text
require the distinctive complete scene envelope. Ambiguous detection keeps its
small input ceiling. Explicit drawing sources allow 2 MiB for embedded assets,
while other diagram and clipboard budgets remain unchanged. Shared extension-hint
handling keeps desktop and little-screen compound-name behavior consistent.
Save As proposes `.excalidraw`, including for scenes originally inferred from JSON.

A second SVG text engine must not reshape the drawing's handwriting: usvg 0.37's
missing-glyph fallback searches database insertion order and can choose a different
family for CJK or mixed-font text. Instead, the local renderer's opt-in
`svg-text-outlines` feature writes its existing, positioned native contours for
text, frame names and generated labels. Default upstream text output remains
unchanged when that feature is off. Font subset/notice accounting is retained;
notices survive composition as escaped inert metadata. Placeholder diagnostic
labels use the shared metric-compatible font database. The adapter needs no
second font catalog loader, alias table or font decoding dependency.

The source-space viewport also survives the helper protocol, with finite-size
validation and preservation of the native authored viewport. It will supply
canvas coordinates without fitting the view again after every edit. Native geometry, text, raster, output and helper-process budgets all remain
explicit; retained compatibility diagnostics appear beside the preview.

The focused adapter tests pass for embedded pixels/static notices, translated
source correspondence, protocol geometry rejection and source-size limits.
All four real-host dispatch checks, syntax/source save checks and mixed-script
outline tests passed in the serial workspace run. Visual inspection of the real
file-manager helper output confirmed handwriting, shapes, freehand and pixels.
Full-workspace verification also exposed a shared JSON-number/SOG compatibility
issue; the separate correction is recorded below.
The graphical canvas is still pending. It must use fresh worker-prepared scene
snapshots, one source-history transaction per gesture, and the existing guarded
save/close flow. Semantic JSON preservation includes unknown records, resources,
tombstones and exact numbers; graphical edits may reformat JSON. The native
model's `translate_connected` moves the entire connected composition, so it is
not a substitute for moving one box while updating its attached arrow endpoints.

Architecture review prompted a focused text-emission extraction from the vendor's
large writer and shared extension-hint handling in core. Maximum CCN stays at the
existing 195; there is no new application hotspot or dependency cycle. The tool
still flags the extracted pre-existing text branch (CCN 16) and a clone-group
increase in vendor/test code; those require review rather than unrelated changes
to pristine renderer algorithms or shared test infrastructure.

### Clipped-image viewport correction

- [x] Preserve the native drawing viewport through SVG composition. The shared
  content-fitting mode uses usvg's geometric bbox, which includes invisible
  portions of clipped images. In the authored fixture, an image symbol expanded
  the 500×295 viewport to 844×454 despite those extra pixels being clipped.
  Native drawings now select authored bounds; other generators retain their
  existing content fitting. This also removes unnecessary viewport reconstruction
  in the adapter. The regression assertion and real bounded helper both report
  source origin (-10, 30), size 500×295 and a correctly fitted 1000×590 preview.
  The rendered artifact was visually inspected with no omission warnings.

Architecture comparison for this fix adds no function or duplication regression;
the sole reported cycle delta is the same untouched Python benchmark cycle with
its members listed in a different order.

### Exact JSON numbers and existing SOG decoding

- [x] Restore SOG decoding with serde_json's unified arbitrary-precision feature.
  The preserving drawing document model enables exact JSON numbers. Spark's
  `serde(untagged)` metadata root buffered fractional fields into an intermediate
  form that its typed f32 schemas could not read. Its original Rust workspace and
  MIT license were committed byte-for-byte first. The local patch parses the
  original V2 and V1 schemas directly in the same priority order, retaining all
  decoding math and archive guards. Existing tests cover both versions, all SH
  bands, fractional metadata, color, scale, orientation and malformed archives.
- [x] Keep the selected-feature dependency build warning-free. SPZ's gzip constant
  is guarded by that feature; format detection assigns completion in each branch
  instead of overwriting an unused initial value. No warning suppression or
  additional decoder feature is introduced.

The complete serial workspace run passes **2,951 tests, with 36 ignored and zero
failures**, including all four real diagram-helper hosts, the viewport regression,
mixed-script native outlines and restored SOG decoding. Strict workspace Clippy
(`--all-targets -- -D warnings`) and workspace formatting checks pass with no
warnings. The first parallel run saturated two existing process-global test
budgets; the serial run avoids that contention. The native drawing artifact was
also inspected visually through the actual bounded file-manager helper.

The Spark source import retains an already-used dependency for a small patch;
it does not add a second scene decoder. Architecture comparison reports the
imported upstream clone groups as repository growth, with no new application
hotspot and no increase in maximum CCN. Source provenance and the exact patch
scope are recorded in `vendor/spark/README.integration.md`. The interactive
whiteboard canvas and the other priority graphical editors remain outstanding.


## Preserving canvas edit model

The shared core drawing model now prepares bounded immutable scene snapshots with
native document storage. Gesture transactions create ordinary shapes/paths, move
compositions, change one paint property, or retain deletions as tombstones.
Groups, bound labels and frame children follow a selected composition. Moving a
box updates its attached arrow endpoint without dragging the other connected box;
arrow labels follow the changed center once. Rigid arrow movement preserves local
points. Stretching rotated/elbowed routes is explicitly refused until their
routing rules are integrated, rather than breaking their bindings.

Unknown properties, exact numbers, malformed neighboring records, unused files
and tombstones remain in the native document. Unsupported or locked compositions
reject atomically. Semantic no-ops retain the original source bytes; actual edits
may reformat JSON. Property-specific paint edits retain mixed group fills.
Element revisions advance only on actual changes. The authored example now uses
real label bindings and still renders with no compatibility diagnostics.

Eight model tests pass for connected movement, mixed group styles, arrow labels,
tombstones/resources, no-op and failed-gesture behavior, native path creation,
budgets and rotated coordinate correspondence. Canvas UI work is in progress;
this model checkpoint does not claim completed graphical editor support.


## First native whiteboard canvas

A first egui canvas is now wired into the existing source session. It supports
basic shape/arrow/line/freehand creation, click/Shift/marquee selection, moving
compositions, stroke/fill/width controls, deletion, pan, cursor-anchored zoom and
fit. The canonical image still comes from the native renderer; gesture feedback
uses source-space geometry. Cached pixels are explicitly stale after source
changes and cannot authorize hit testing against a newer draft.

Model preparation and semantic source edits run through the existing bounded
jobs. Accepted gestures keep their session owner across tab changes. Save, Save
As and shared-history commands issued during a gesture are replayed in order
after its source revision arrives; failed or superseded edits never save an older
source. The source editor's TextEdit undoer owns graphical and textual revisions.
Keyboard routing distinguishes text-field focus from canvas/button focus, because
egui's wants_keyboard_input also includes non-text widgets.

Targeted tests pass for stale geometry, captured pointer release outside the
canvas, camera precision, focus-aware history, both Save/Undo orderings, and
failure/supersession. The private desktop acceptance script
`diagram_drawing_acceptance.py` passed a real draw/save/undo/save sequence in the
light theme, retaining assets and restoring the exact original source bytes.
The first two acceptance attempts needed OCR targeting adjustments; the next
caught the canvas-focus undo issue, which is now covered by a regression test.
Text creation/editing, resize handles and further interaction polish remain
outstanding; this is an initial reviewable canvas, not completed editor coverage.

### GPUI reference follow-up

The published gpui-excalidraw 0.1.9 sources (MIT, revision
2641e1b9c00b51e5e8329094a7d88cd1223ed630) separate a framework-independent core
from GPUI widgets. `core/binding.rs`, `collision.rs`, `transform.rs` and the UI's
resize/text-commit interaction are useful references for the next controls.
Its internal linear/freehand points are absolute, unlike our native relative
points. Its core text-width helper estimates character advances; actual painting
uses GPUI shaping. Those shortcuts are not a replacement for canonical font
measurement. Its typed scene and binding schema also require review before
adapting algorithms to the preserving 0.18.1 document profile. No GPUI source or
assets have been copied into this implementation. Any future extraction must
retain the original MIT license and be committed pristine before adaptation.

The follow-up source review informs these implementation requirements:

- Text editing should commit one source transaction, preserve container bindings
  and unknown properties, and share the existing save/close/undo delivery rules.
  GPUI's `begin_text_edit` / `commit_text_edit` interaction is a useful reference;
  its character-width approximation is unsuitable for mixed-font labels. Our
  native renderer already computes shaped line advances in
  `vendor/excalidraw-render/src/render/text.rs`; extend that bounded worker result
  if authoring requires measurements rather than adding a second font layout.
- Resize should anchor the opposite handle and support an aspect-ratio modifier.
  Calculate the completed transaction from the captured starting geometry, so
  intermediate frames do not accumulate rounding or generate undo revisions.
  Crossing the anchor, rotated shapes, relative paths, bound labels and attached
  arrows need explicit tests. The GPUI transform helper takes absolute path
  points and applies absolute dimensions before checking their sign; it cannot
  be used directly for our relative-point geometry or negative-scale handling.
- New arrow binding should select the visible topmost eligible shape and use a
  screen-distance threshold converted through the camera scale. GPUI's
  `find_bindable` uses scene iteration order and a fixed world-space threshold;
  copying that behavior would make overlap selection and zoom inconsistent.
  Its normalized `fixedPoint` / `mode` bindings also differ from the selected
  document profile's `focus` / `gap` representation.

These are adaptation decisions, not completed features. Primary source:
[GPUI editor at the inspected revision](https://github.com/LittleGuest/gpui-excalidraw/tree/2641e1b9c00b51e5e8329094a7d88cd1223ed630/src).
A fresh crates.io search still found that editor alongside the native document
and rendering crates, rather than an existing egui editor to integrate.

### Post-canvas desktop regression checkpoint

The current debug file-manager binary passes the private light- and dark-theme whiteboard
draw/save/undo/save test, including exact original-byte restoration and retained
embedded assets (`/tmp/diagram-whiteboard-ui-{light,dark}-final-1/report.json`). The
existing WaveDrom timing-painter acceptance also passes in both appearances after
the shared editor changes
(`/tmp/diagram-timing-ui-{light,dark}-post-canvas-1/report.json`). These exercise actual
pointer and keyboard input on an isolated desktop; they do not prove the pending
text/resize functionality or full format coverage. No Cargo job or source change
was needed for this checkpoint while the user's scratch-worktree build ran.

## WireViz source-preserving editor research

Published sources inspected on 2026-11-12, before adding a dependency:

| Candidate | Source findings | Fit |
| --- | --- | --- |
| [yaml-edit 0.3.6](https://github.com/jelmer/yaml-edit) | Apache-2.0; Rowan syntax tree, byte ranges, direct mapping/sequence edits, anchor-aware mutations; parser nesting ceiling 256; Rust 1.70 | Strong direct-edit candidate. Recovered trees must not authorize semantic edits without validation. |
| [yaml-edit-rp 0.3.1](https://github.com/roberts-pumpurs/yaml-edit) | Apache-2.0; source-preserving fork, also Rowan 0.16 | Compare concrete failing fixtures before preferring the fork to current upstream; the package description alone does not establish which fixes remain missing upstream. |
| [yaml-serde-edit 0.1.1](https://github.com/bldr-project/yaml-serde-edit) | MIT/Apache-2.0; depends on older yaml-edit 0.2 and serde_norway; value-diff layer may rebuild unsupported edits | Rebuilding can discard comments and anchors away from the requested edit. Avoid automatic fallback for graphical authoring; its disk-writing layer also duplicates FileSource. |
| [oneyaml 1.2.0](https://github.com/recursive-ai-oss/oneyaml-rust) | MIT/Apache-2.0; one semantic/syntax interpretation, node spans, retained text, configurable resource limits, atomic patches; writes through aliases or merged keys are refused | Promising first prototype for explicit transactions. New implementation with limited production history; verify refusal, rollback and preservation using actual WireViz structures before selecting it. |
| [yamled 0.0.13](https://github.com/victor-software-house/yamled) | MIT; published Rust requirement 1.99.1 | Does not fit the installed 1.98.1 toolchain. Do not raise the project toolchain for this candidate. |

These findings are source review, not executed compatibility tests. Compare
oneyaml and yaml-edit with comments, quoted numeric pin identities, flow and
block sequences, BOM/CRLF, aliases/merge keys, malformed input, duplicate keys,
oversized expansion and failed multi-step edit rollback. A graphical edit must
either preserve unrelated source or refuse visibly. No library may write files
itself; the existing source editor owns all saves and accepted completions.

Wirebug's public `Design`/`Instance`/`Connector`/`Wire` IR and harness SVG renderer
are potential reuse points, but its package unconditionally includes server,
LSP, PDF and a second raster stack. Its `.wb` parser is not a WireViz YAML parser.
Compare a minimal licensed renderer extraction with generating bounded DOT
tables/ports through our existing native Graphviz adapter. Do not add the whole
server stack merely to obtain harness drawing, or translate GPL WireViz Python
code into the application. Any extraction still needs a pristine import commit
before local modifications.

The [WireViz syntax specification](https://github.com/wireviz/WireViz/blob/master/docs/syntax.md)
also makes clear that `connections` is more than a list of endpoint pairs:
parallel pin/wire lists, ascending and descending ranges, named/unnamed template
instances, shields, connector loops and mating arrows affect the real circuit.
The adapter must model those semantics, enforce expansion budgets and report
unsupported operations. Local images and Graphviz tweaks must not escape the
guarded resource policy. Generic `.yml`/`.yaml` files require a strong structural
signature before opening a harness editor; a filename alone is insufficient.

## Nomnoml renderer selection checkpoint

The published `silica` 0.0.1 is a bare-metal OS library, and `squaredb` 0.2.0 is
an HTTP database whose README merely mentions Nomnoml. Neither is a renderer.
Compared these search results with the canonical MIT Nomnoml 1.7.0 and its MIT
Graphre 0.1.3 layout dependency. The canonical SVG entrypoint works without a
document, filesystem or module loader, making reuse of our bounded QuickJS
adapter preferable to an independent partial language implementation. This is
an embedded interpreter, not a browser or an installed Node dependency.

A development-only Node VM prototype loaded the trusted release bundles into an
empty context, then passed diagram text as a function argument. It produced a
135×328 SVG, rejected an unclosed node with a line/column diagnostic, and escaped
markup labels. Node was only the research harness. Crucially, direct `renderSvg`
silently ignores `#import`; the application adapter must refuse imports visibly.
The server text measurer also uses character-width estimates when no canvas is
provided, so mixed-script layout requires explicit validation/diagnostics.

A second development probe linked the already-built rquickjs-sys 0.14.0 QuickJS
archive directly. With 128 MiB heap, 512 KiB stack and an interrupt deadline, it
produced byte-identical SVG. This verifies interpreter compatibility without
another Cargo build; application-worker and raster validation still remain.

The pristine release bundles/licenses were committed before integration changes.
Their hashes, archive URLs and intended restricted entrypoint are recorded in
[`vendor/nomnoml/README.integration.md`](../vendor/nomnoml/README.integration.md).
The official web editor provides source and live preview; its DOM UI is not an
embeddable native graphical editor. Use the existing shared
source editor as the baseline and investigate source-linked direct operations
after the higher-priority graphical editors. Format routing, syntax highlighting,
Markdown/standalone rendering, clipboard recognition and actual bounded-worker
validation are still pending; the import is not a format-support completion.

After reviewing the code sizes and ongoing compatibility costs, the user accepted
retaining the upstream JavaScript engines instead of maintaining Rust ports.
Keep their versions pinned, original sources/licenses separate from adapters,
and the interpreter hostless and bounded. Canonical WaveDrom is already active;
Nomnoml/Graphre integration is still pending. The native canvas and file-manager
preview corrections take priority over adding that adapter.

## Canvas interaction corrections

- [x] Select ellipses and diamonds by their actual rotated geometry rather than
  their bounding-box corners. The regression test covers interior points,
  edge tolerance and empty corners after a 90-degree rotation; it passes in the
  studio test target. Native paths retain their existing segment-distance hits.
- [x] Keep Delete and Escape responsive while canonical drawing pixels refresh.
  Pointer hits still require fresh geometry; selection commands use the current
  model. Escape also handles egui's automatic focus loss and clears captured
  gesture feedback without discarding accepted edits. All eight drawing-related
  studio tests pass, including actual input with stale preview pixels.

## Front-matter date correction

- [x] Represent unquoted TOML dates/times as ISO strings in the shared metadata
  model, including nested tables and arrays. The former Serde conversion exposed
  TOML's private datetime carrier object instead. All six core metadata tests
  pass, retaining existing source offsets, malformed-input and resource-limit
  coverage; the file-manager's cross-format page-summary test passes too.

## Desktop review requirements: vectors and browser strips

The user's review identified two gaps in the current implementation. These are
required follow-up work, alongside the remaining format coverage:

- [ ] Replace the editor's raster-image canvas with direct antialiased vector
  drawing. Curves, handwritten strokes and outlined glyphs must remain crisp
  through zoom. Bitmap assets remain bitmap assets.
- [ ] Draw the actual styled artwork during gestures, rather than moving only
  selection outlines until a worker finishes. Retain object identities, paint
  order, bindings, opacity, clipping and image crops; keep parsing/preparation
  bounded and off the UI thread. Source/history commits remain one per gesture.
- [ ] Use the supplied editor screenshots as interaction references: compact
  icon tools and contextual style/selection controls. Text and resize remain
  essential; the reference's AI tools, Mermaid conversion and web embeds are
  outside the requested scope.
- [ ] Render standalone diagram strips in the file-manager list, with enlarged
  rendered hover previews. WaveDrom should prioritize the actual waveforms over
  decorative heading text. Preserve useful scale, clip vertically when needed,
  and reuse existing cache admission, revision checks and hidden-row retirement.
- [x] Give Markdown front matter a compact metadata area beside a smaller text
  minimap. Prioritize supplied Hugo fields (`title`, `date`, `draft`, `tags`,
  categories and related dates/weight) without increasing row height. Do not
  infer publication state or project timezone from those fields.
- [ ] Replace Markdown's syntax-only hover excerpt with a rendered view at the
  hovered source position, including equations and diagrams. Preserve useful
  source-position mapping and bounded work for large documents.

The [Hugo front-matter reference](https://gohugo.io/content-management/front-matter/)
confirms the reserved page fields, taxonomy arrays and unquoted TOML datetimes.
Its build/cascade configuration is not evaluated by a passive file preview.

Vector backend candidates inspected: `lyon_tessellation` 1.0.23 provides robust
path fills/strokes but leaves antialiasing, clipping and group compositing to its
host; Grafia 0.7.0 targets egui 0.37 and is not directly compatible with this
workspace's egui 0.27; FemtoVG 0.28.0 provides native antialiased GPU paths,
transforms and compositing and fits the installed Rust version. Both applications
already use eframe's OpenGL backend and expose its function loader at creation.
Evaluate retaining the native renderer's vector paths/glyphs with per-element
identity, and a host-owned GPU lifetime, rather than repeatedly increasing a
cached PNG's resolution. No backend has been integrated at this checkpoint.

### Compact page metadata checkpoint

Page summaries now reuse the text-strip read worker and cache. They prioritize
title/date/draft/tags, then categories, publication/modification dates and weight;
unfamiliar metadata falls back to bounded field summaries. Empty metadata does
not consume strip space. Narrow rows retain the full minimap; wider rows reserve
a bounded metadata area without changing their height. Click and hover source
positions use the remaining minimap rectangle. No publication status is inferred.

The scanner retains a bounded metadata prefix while language detection keeps its
original 8 KiB window. Source identity checks and cancellation remain with the
existing two-pass reader; there is no additional file reader or preview cache.

Validation: all 12 file-manager text-strip tests and six core metadata tests pass.
The native browser acceptance script passes in light and dark appearance, showing
YAML, typed TOML and JSON page fields with and without the Shelf dock and checking
that passive previews leave all source bytes unchanged. Final reports are under
`/tmp/diagram-browser-frontmatter-{light,dark}-3/`. The debug file-manager build,
workspace formatting and strict all-target Clippy for core, studios and file
manager pass. Five additional review fixtures exercise Hugo fields and Markdown
hover positions. Rendered diagram strips and rendered Markdown hover remain pending.

Architecture comparison adds the metadata presenter and desktop test while
keeping maximum complexity, clone-group count and dependency-cycle count
unchanged; duplication falls slightly. The reported new/resolved Python cycle
is the same untouched cycle with its members reordered, and the touched preview
clone windows retain existing raster/cache handling. No new hotspot is reported.

## Vector object correspondence checkpoint

The native renderer's opt-in `svg-source-groups` feature now retains each
object's identity around its existing vector content. It reuses the renderer's
source correspondence rather than inferring objects from paths. Hex-encoded
UTF-8 identities survive the shared SVG namespace isolation, including markup
characters, spaces and emoji. The real example retains eight independently
identified shape, arrow, text, freehand and image objects. Frame names stay with
their frame, and existing clipping, viewport and asset accounting remain intact.

All four native drawing-renderer integration tests pass, and strict all-target
Clippy for render and file manager passes. The first new fixture omitted its
timestamp and correctly produced a repair diagnostic; it now supplies that
timestamp rather than suppressing the diagnostic. The small writer patch adds
one necessary fallible output operation to the existing serializer; maximum
complexity and clone-group counts remain unchanged. No new application hotspot
is introduced. The renderer's upstream output is unchanged when the feature is off.

This provides correspondence for retained vector drawing; the current canvas
still paints a PNG until the GPU backend is integrated. FemtoVG's context-sharing
prototype is the next gate. The user also identified future VCV Rack previews as
a reuse opportunity for that renderer. Keep its host/context lifetime reusable,
and validate coexistence with egui before enabling it in the studios.

## Shared vector context validation

The development-only `clip-ui` example `vector_context_probe` now exercises
FemtoVG 0.28.0 inside eframe 0.27's existing current OpenGL context. It uses
eframe's function loader; the two glow versions are function tables for that
same context, not separate rendering surfaces. GPU objects remain on the
creating UI thread and are destroyed in eframe's current-context exit hook.
Callbacks capture only thread-safe input and metrics, with no unsafe Send/Sync
implementations. The few GL calls document their current-context requirements.

The private desktop acceptance scenario covers arbitrary rounded clipping,
an even-odd hole, stroked curves, 1600% zoom, pointer panning, repeated frames,
egui controls painted after vectors, window shrinking/restoration, and renderer
destruction. Light and dark runs pass with zero reported GL errors or layer
allocation failures. Screenshots confirm crisp enlarged curves and intact
surrounding controls. This runner uses software OpenGL; these results do not
claim testing every hardware driver or multi-window context configuration.

The prototype explicitly scales logical coordinates by egui's pixels per point,
uses an offscreen layer with its own stencil attachment, restores egui's
framebuffer after painting, and disables stencil testing before returning to
egui. Initial runs caught a harness window-ID mismatch and ambiguous OCR button
matching; dedicated IDs and a distinct control label resolve those. Private
compositor resize crossfades are disabled for unambiguous screenshots. The
example continuously repaints to exercise interleaving during asynchronous
native resizes; this is a development stress test, not a production repaint policy.

Validation commands:

```sh
cargo build -j4 -p clip-ui --example vector_context_probe
cargo clippy -j4 -p clip-ui --example vector_context_probe -- -D warnings
cargo fmt --all --check
TESSDATA_PREFIX=/home/vega/.local/share/tessdata python3 scripts/vector_context_acceptance.py --appearance light --output-dir /tmp/vector-context-light-final
TESSDATA_PREFIX=/home/vega/.local/share/tessdata python3 scripts/vector_context_acceptance.py --appearance dark --output-dir /tmp/vector-context-dark-final
```

Architecture comparison leaves maximum complexity and dependency cycles
unchanged. Its two new clone groups are verbatim MIT license paragraphs, which
must remain intact; it reports no duplicated application logic in the prototype.

FemtoVG remains a development dependency until the retained scene and host
lifetime are connected to studios. The production owner must distinguish
contexts, handle unavailable GL explicitly, and preserve the callback's actual
framebuffer target. The current example owns one native window, so it does not
establish a multi-context registry. SVG preparation, per-object live transforms,
glyph/image fidelity and the actual studio integration remain pending. The
editor still paints its existing raster preview at this checkpoint.
