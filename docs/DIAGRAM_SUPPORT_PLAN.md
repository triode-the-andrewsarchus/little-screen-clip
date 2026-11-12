# Native diagram previews and editing

Initial branch: `feat/native-diagram-previews` (merged); continuation: `feat/native-diagram-previews-next`; baseline: `native-diagram-previews` at `c9cbf22`.

## Required outcome

Every item under Markdown in `file_types.md`, from front matter through Symbolator, must render in the large text viewer and the little-screen preview. Formats with standalone extensions also need guarded opening, highlighted editable source, live preview and visible diagnostics. Investigate existing graphical editors and use direct manipulation with source correspondence where practical. No browser engine or silent network rendering. Make reviewable commits as each part lands.

Preserve FileSource identity checks, atomic save/Save As semantics, accepted write completion, preview cancellation and per-host lifecycle. Rendering must not resolve arbitrary network URLs, include files, execute diagram directives or read unguarded resources. Enforce input, complexity, output and raster budgets. Unknown/unsupported syntax must remain inspectable with a diagnostic, never silently treated as fully supported.

## Implementation sequence

- [x] Shared metadata framing/parsing and both Markdown presentations.
- [x] Shared bounded native worker/render contract and diagnostics, reusing command ownership and raster dependencies.
- [ ] Per-element source spans and exact edit correspondence for graphical editing.
- [ ] Native renderer adapters, compared against representative upstream fixtures.
- [ ] Markdown fenced/inline math rendering in both output paths.
- [ ] Standalone format detection (including ambiguous/compound extensions), source + live-preview + diagnostics editor and guarded saves.
- [ ] Format-specific graphical interactions, with exact source edits or explicit unsupported-operation diagnostics; preserve unknown source content.
- [ ] Full validation, visual review at both sizes, examples, license records and accurate checklist updates.

This is a tracking document, not a claim of implemented coverage. The entire scope remains required.

## Research ledger

Initial crates.io search, 2026-11-12. Candidates below require API/source/license/compatibility review before selection; matching a name is not evidence of format compatibility. In particular the `bytefield` crate is a bitfield library, not the Bytefield-SVG language, and `vega-view` embeds a webview and is unsuitable here.

| Requested family | Initial native candidates / alternatives to investigate | Status |
| --- | --- | --- |
| Front matter | [frontmatter](https://crates.io/crates/frontmatter) 0.4.0; [frontmatter-gen](https://crates.io/crates/frontmatter-gen) 0.0.12; [mdbook-frontmatter-fix](https://crates.io/crates/mdbook-frontmatter-fix) 0.4.0 | Pending comparison / editor research |
| LaTeX math | [latex-rust](https://crates.io/crates/latex-rust) 3.0.3; [retex-svg](https://crates.io/crates/retex-svg) 1.9.0; [tex2svg](https://crates.io/crates/tex2svg) 0.1.0 | Pending comparison / editor research |
| Mermaid | Merman 0.10.0; mermaid-rs-renderer 0.3.1; ariel-rs 0.4 | Selected Merman; native source/live previews implemented; parser-backed graphical editing pending |
| PlantUML / C4 | [mdbook-plantuml](https://crates.io/crates/mdbook-plantuml) 2.0.0; [plantuml_encoding](https://crates.io/crates/plantuml_encoding) 2.0.3; [plantuml-little](https://crates.io/crates/plantuml-little) 1.2026.2-4 | Pending comparison / editor research |
| D2 | [graphs-tui](https://crates.io/crates/graphs-tui) 0.4.0; [d2-rs-target](https://crates.io/crates/d2-rs-target) 0.1.7; [d2-rs-scenebuild](https://crates.io/crates/d2-rs-scenebuild) 0.1.7 | Pending comparison / editor research |
| Graphviz DOT | [graphviz-pure](https://crates.io/crates/graphviz-pure) 1.2.0; [graphviz-pure-cli](https://crates.io/crates/graphviz-pure-cli) 1.2.0; [graphviz-pure-core](https://crates.io/crates/graphviz-pure-core) 1.2.0 | Pending comparison / editor research |
| DBML | [dbml-language-server](https://crates.io/crates/dbml-language-server) 0.1.2; [dbml_diff_mysql](https://crates.io/crates/dbml_diff_mysql) 0.1.5; [sea-orm-dbml](https://crates.io/crates/sea-orm-dbml) 0.1.0-beta.2 | Pending comparison / editor research |
| BPMN | [orvanta-api](https://crates.io/crates/orvanta-api) 7.0.0; [bpmn-svg](https://crates.io/crates/bpmn-svg) 0.1.0; [bpmnkit-bpmn](https://crates.io/crates/bpmnkit-bpmn) 0.2.1 | Pending comparison / editor research |
| draw.io | [drawio-exporter](https://crates.io/crates/drawio-exporter) 1.6.0; [flowchart-mcp-server](https://crates.io/crates/flowchart-mcp-server) 0.1.0; [drawio-lc](https://crates.io/crates/drawio-lc) 0.4.0 | Pending comparison / editor research |
| Excalidraw | [excalidraw-image](https://crates.io/crates/excalidraw-image) 0.2.0; [gpui-excalidraw](https://crates.io/crates/gpui-excalidraw) 0.1.9; [excalidraw-render-cli](https://crates.io/crates/excalidraw-render-cli) 0.1.0 | Pending comparison / editor research |
| WaveDrom | [wavedrom](https://crates.io/crates/wavedrom) 0.1.0; [mdbook-wavedrom-rs](https://crates.io/crates/mdbook-wavedrom-rs) 0.1.0; [mdbook-wavedrom](https://crates.io/crates/mdbook-wavedrom) 0.10.0 | Pending comparison / editor research |
| Structurizr | [oak-structurizr](https://crates.io/crates/oak-structurizr) 0.0.11; [dendryform-export](https://crates.io/crates/dendryform-export) 0.1.0; [cadastre](https://crates.io/crates/cadastre) 0.4.0 | Pending comparison / editor research |
| BlockDiag / SeqDiag / ActDiag / NwDiag / PacketDiag / RackDiag | [xio_job_to_blockdiag](https://crates.io/crates/xio_job_to_blockdiag) 0.8.0 | Pending comparison / editor research |
| Bytefield | [bytefield](https://crates.io/crates/bytefield) 0.1.0; [wiry-core](https://crates.io/crates/wiry-core) 0.1.0; [wiry-capture](https://crates.io/crates/wiry-capture) 0.1.0 | Pending comparison / editor research |
| Ditaa | [mdbook-plantuml](https://crates.io/crates/mdbook-plantuml) 2.0.0; [ansicht](https://crates.io/crates/ansicht) 0.1.0; [plantuml-little](https://crates.io/crates/plantuml-little) 1.2026.2-4 | Pending comparison / editor research |
| Erd | [rusterd](https://crates.io/crates/rusterd) 0.3.1; [erd_rs](https://crates.io/crates/erd_rs) 0.1.0; [fastbreak](https://crates.io/crates/fastbreak) 0.5.0 | Pending comparison / editor research |
| Nomnoml | [silica](https://crates.io/crates/silica) 0.0.1; [squaredb](https://crates.io/crates/squaredb) 0.2.0 | Pending comparison / editor research |
| Pikchr | [pikchr](https://crates.io/crates/pikchr) 0.1.4; [pikchr-rs](https://crates.io/crates/pikchr-rs) 0.1.0; [pikchr-sys](https://crates.io/crates/pikchr-sys) 0.1.1 | Selected pikchr-rs; source/live previews implemented; direct manipulation pending |
| Svgbob | [svgbob](https://crates.io/crates/svgbob) 0.7.6; [mdbook-svgbob](https://crates.io/crates/mdbook-svgbob) 0.3.1; [mdbook-svgbob2](https://crates.io/crates/mdbook-svgbob2) 0.3.0 | Selected original SvgBob; source/live previews implemented; cell editor and .bob disambiguation pending |
| GoAT | [aasvg](https://crates.io/crates/aasvg) 1.0.0; [formulaa](https://crates.io/crates/formulaa) 0.1.0 | Pending comparison / editor research |
| Lini | Original lini 1.1.0 compiler; mdbook-lini and lini-wasm integrations of the same compiler; specification reimplementation | Original native compiler selected; highlighted source/live previews implemented, graphical editing pending |
| TikZ / PGF | [mdbook-tikz](https://crates.io/crates/mdbook-tikz) 0.2.8; [tikra](https://crates.io/crates/tikra) 1.9.2; [tikra-cli](https://crates.io/crates/tikra-cli) 1.9.2 | Pending comparison / editor research |
| UMLet | No direct crates.io match; compare upstream implementation and format specification. | Pending comparison / editor research |
| Vega / Vega-Lite | [vega](https://crates.io/crates/vega) 0.0.1; [vega-view](https://crates.io/crates/vega-view) 0.1.0; [encdr](https://crates.io/crates/encdr) 0.9.0 | Pending comparison / editor research |
| WireViz | [wirebug](https://crates.io/crates/wirebug) 0.1.0 | Pending comparison / editor research |
| Symbolator | [tui-piechart](https://crates.io/crates/tui-piechart) 1.4.2; [raptorq](https://crates.io/crates/raptorq) 2.0.1; [qrcode-generator](https://crates.io/crates/qrcode-generator) 6.1.2 | Pending comparison / editor research |

Additional promising Mermaid alternatives: [mermaid-rs-renderer](https://github.com/1jehuang/mermaid-rs-renderer), [merman](https://docs.rs/merman/latest/merman/) and [ariel-rs](https://docs.rs/ariel-rs/latest/ariel_rs/). Compare typed models/source correspondence, compatibility, dependencies and bounded execution, rather than advertised benchmarks alone.

## Design direction

[Bret Victor’s linked-representations examples](https://worrydream.com/MediaForThinkingTheUnthinkable/) suggest that the useful interaction is connecting source, structure and visible result. Keep source authoritative; selection in one view should explain the corresponding objects in the other. Do not force all formats into generic boxes and arrows or replace rich formats with an undocumented subset.

Existing shared Markdown parsing/rendering lives in `little-clip-render::markdown`, with native screen painting in `cards::text`. `clip-studios::text_preview::markdown` consumes the same parsed lines. File-manager text sessions already own guarded edits/saves. Extend these mechanisms rather than creating a second file-save or Markdown implementation. Native renderers may need disposable host workers if their runtime cannot be cooperatively bounded; a compiled worker is distinct from an independently installed tool.

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

## WaveDrom research (not yet implemented)

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
LaTeX parser, so it is not an equivalent standalone math renderer. No math adapter
has been selected or added yet.

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
