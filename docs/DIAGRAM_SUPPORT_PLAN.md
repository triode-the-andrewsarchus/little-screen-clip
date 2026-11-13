# Native diagram previews and editing

Initial branch: `feat/native-diagram-previews` (merged); continuations: `feat/native-diagram-previews-next` (merged), `feat/native-math-previews`; baseline: `native-diagram-previews` at `c9cbf22`.

## Required outcome

Every item under Markdown in `file_types.md`, from front matter through Symbolator, must render in the large text viewer and the little-screen preview. Formats with standalone extensions also need guarded opening, highlighted editable source, live preview and visible diagnostics. Investigate existing graphical editors and use direct manipulation with source correspondence where practical. No browser engine or silent network rendering. Make reviewable commits as each part lands.

Preserve FileSource identity checks, atomic save/Save As semantics, accepted write completion, preview cancellation and per-host lifecycle. Rendering must not resolve arbitrary network URLs, include files, execute diagram directives or read unguarded resources. Enforce input, complexity, output and raster budgets. Unknown/unsupported syntax must remain inspectable with a diagnostic, never silently treated as fully supported.

File-manager list presentation is also a completion requirement of this active
goal: rendered standalone diagram strips with enlarged rendered hover popouts,
compact Hugo-oriented front-matter fields beside the text minimap, and rendered
Markdown hover at the hovered source position. These are not deferred polish or
optional follow-up. The detailed acceptance checklist is under
[Desktop review requirements](#desktop-review-requirements-vectors-and-browser-strips).
The goal must remain incomplete while the pending strip and hover items remain.

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
| PlantUML / C4 | plantuml-core 0.5.4; puml 0.2.6; plantuml-little 1.2026.2-4; Omaspec | Native plantuml-core selected with shared Graphviz and bundled-only resolver; representative family, C4 sprite, host and native visual checks pass; extended compatibility qualification pending |
| D2 | d2-rs 0.1.7 pipeline; d2-render 0.1.0; graphs-tui 0.4.0; official Go D2 | Native compiler/layout and scene renderer integrated; flat-board Markdown/standalone previews and highlighted guarded saves implemented; graphical editing/board selection pending |
| Graphviz DOT | graphviz-pure 1.2.0; layout-rs 0.1.3; graphviz-rust 0.9.9 | Native Graphviz port selected; source/live preview and bounded multi-graph rendering implemented; graphical manipulation pending |
| DBML | dbml-rs 1.0.1, dbml-language-server, dbml_diff_mysql, sea-orm-dbml | Native parser/Graphviz adapter and shared source/list routes implemented; newer syntax and graphical editing pending |
| BPMN | BPMN Kit 0.2.1 native model/layout/SVG; bpmn-svg 0.1.0; bpmn-js and BPMNKit graphical editors | Native authored-DI and automatic-layout previews; highlighted source editor; direct manipulation optional follow-up |
| draw.io | [drawio-exporter](https://crates.io/crates/drawio-exporter) 1.6.0; [flowchart-mcp-server](https://crates.io/crates/flowchart-mcp-server) 0.1.0; [drawio-lc](https://crates.io/crates/drawio-lc) 0.4.0 | Pending comparison / editor research |
| Excalidraw | excalidraw-render 0.1.0; excalidraw-document 0.2.0; gpui-excalidraw 0.1.9; excalidraw-image 0.2.0 | Native SVG renderer and preserving graphical transactions integrated; initial drawing/selection/movement canvas works; text editing and resize handles pending |
| WaveDrom | Canonical 3.8.0; native wavedrom 0.1.0 and current Rust main; WavePaint graphical reference | Canonical engine embedded without a browser; timing/register/logic source previews integrated; direct timing painting and lane naming integrated; advanced operations stay in source |
| Structurizr | oak-structurizr; dendryform-export; Cadastre; tree-sitter; StructurizrX; structurizr-rs | Native candidates probed; selected memory-only libraries prepared; host integration pending (qualification below) |
| BlockDiag / SeqDiag / ActDiag / NwDiag / PacketDiag / RackDiag | [xio_job_to_blockdiag](https://crates.io/crates/xio_job_to_blockdiag) 0.8.0 | Pending comparison / editor research |
| Bytefield | Canonical Bytefield-SVG 1.11/1.12; ProtoViz; Framegram; unrelated native bitfield crates | Pin canonical 1.11 after differential helper-function regression; bounded hostless rendering, shared Markdown/clipboard/source/list paths implemented |
| Ditaa | [mdbook-plantuml](https://crates.io/crates/mdbook-plantuml) 2.0.0; [ansicht](https://crates.io/crates/ansicht) 0.1.0; [plantuml-little](https://crates.io/crates/plantuml-little) 1.2026.2-4 | Pending comparison / editor research |
| Erd | rusterd 0.3.1; erd_rs 0.1; erd-rs; erd-go; original Haskell grammar | Native canonical bracket/entity parser, styled Graphviz rendering and shared Markdown/source/list routes implemented; graphical editing pending |
| Nomnoml | Canonical Nomnoml 1.7.0 with Graphre 0.1.3; crates.io hits silica/squaredb inspected and unrelated | Hostless canonical rendering, standalone/Markdown/clipboard routes and source/live editor implemented; graphical editing pending |
| Pikchr | [pikchr](https://crates.io/crates/pikchr) 0.1.4; [pikchr-rs](https://crates.io/crates/pikchr-rs) 0.1.0; [pikchr-sys](https://crates.io/crates/pikchr-sys) 0.1.1 | Selected pikchr-rs; source/live previews implemented; direct manipulation pending |
| Svgbob | [svgbob](https://crates.io/crates/svgbob) 0.7.6; [mdbook-svgbob](https://crates.io/crates/mdbook-svgbob) 0.3.1; [mdbook-svgbob2](https://crates.io/crates/mdbook-svgbob2) 0.3.0 | Selected original SvgBob; source/live previews implemented; cell editor and .bob disambiguation pending |
| GoAT | aasvg 1.0.0, SvgBob, upstream GoAT 0.5.1; differential upstream fixtures | Pinned upstream static companion, ASCII/Unicode rendering and shared source/strip routes implemented; cell editing pending |
| Lini | Original lini 1.1.0 compiler; mdbook-lini and lini-wasm integrations of the same compiler; specification reimplementation | Original native compiler selected; highlighted source/live previews implemented, graphical editing pending |
| TikZ / PGF | [mdbook-tikz](https://crates.io/crates/mdbook-tikz) 0.2.8; [tikra](https://crates.io/crates/tikra) 1.9.2; [tikra-cli](https://crates.io/crates/tikra-cli) 1.9.2 | Pending comparison / editor research |
| UMLet | No direct crates.io match; compare upstream implementation and format specification. | Pending comparison / editor research |
| Vega / Vega-Lite | [vega](https://crates.io/crates/vega) 0.0.1; [vega-view](https://crates.io/crates/vega-view) 0.1.0; [encdr](https://crates.io/crates/encdr) 0.9.0 | Pending comparison / editor research |
| WireViz | [wirebug](https://crates.io/crates/wirebug) 0.1.0; oneyaml/yaml-edit source-preserving parsers; existing native Graphviz tables/ports | Native harness model, initial rendered previews, shared routes and source/live editor implemented. Template/palette compatibility and graphical harness transactions remain pending. |
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

The initial source review is now supplemented by an executed, pinned comparison
in [`scripts/diagram_research/wireviz_yaml`](../scripts/diagram_research/wireviz_yaml/README.md).
Both candidates preserve unrelated bytes when replacing an existing pin-label
literal in block and flow lists, including comments, BOM/CRLF, quoted numeric
pin IDs and designators containing `/`, `.` and `~`. Both reject the malformed
and excessively nested fixtures. `oneyaml` additionally rejects semantic
duplicate keys and oversized alias expansion, refuses writes through aliases
and merged keys, and restores the exact source after a failed multi-step patch.
`yaml-edit` accepts the duplicate-key fixture, so it would need another semantic
validation layer. Select `oneyaml` for the next harness-model prototype, with
tighter application budgets; this does not yet establish full WireViz support.

One important counterexample remains an application responsibility: changing
an anchor definition is allowed and changes other alias users. Concrete-source
preservation alone cannot authorize a graphical transaction. Compare the parsed
result against the intended semantic delta and refuse changes to unrelated
connectors, wires or connections. A pin-label rename also needs connection
reference handling; changing a label literal alone is not a valid circuit edit.

The executed compatibility fixture preserves quoted `"01"` as a string and
resolves unquoted `01`, `NO` and `ON` according to the explicitly selected YAML
1.1 preset. Use consistent scalar rules for reads and edits rather than silently
switching to YAML 1.2 identities. Unknown custom tags remain tagged values and
must not be treated as ordinary component mappings. Multiple documents are
refused. A graphical edit must either preserve unrelated source and semantics
or refuse visibly. No library may write files itself; the existing source
editor owns all saves and accepted completions. Parser/edit probes do not
replace connection-expansion, renderer, UI or guarded-save acceptance tests.

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

### Native harness model checkpoint

`little-clip-core::diagram::wireviz` now normalizes connector pins, cable
conductors, shields and connection links without file I/O or a rendering
dependency. Declaration order and typed pin identities are preserved. Each
normalized link retains its source connection set, step and lane, or its
connector-loop pair. Renderers and direct editors will consume this same model.

The implemented connection semantics include parallel paths, ascending and
descending numeric ranges, exact pin IDs before unique labels, cable wire
numbers/labels/unique colors, single-ended shields, explicit single-pin
component lists, connector loops, per-pin mating arrows, whole-connector mating
arrows and unconnected components. Explicit cable colors repeat or trim to the
wire count. Invalid lane counts, ambiguous references, invalid path alternation,
duplicate identities and unsupported tagged YAML fail with diagnostics.
Unreferenced definitions retain their unused status and produce a warning.

Limits are explicit: 256 KiB source, depth 64, 32,768 alias-expanded nodes,
1 MiB alias-expanded bytes, 256 components, 8,192 ordinary terminals, 512 pins
or wires per component, 1,024 connection sets, 256 steps per set and 8,192
normalized links. Component and terminal limits are checked during assembly.
Images are never opened and Graphviz tweaks are never executed; their omission
is reported. The renderer/routing checkpoint below builds on this model;
neither checkpoint claims full WireViz compatibility or graphical editing.

Template/autogenerated instances and automatic `color_code` palettes remain
explicit errors pending implementation. These are still required compatibility
work, not a permanent reduced dialect. The first guarded graphical transactions
are described below; connection creation/removal still needs source planning.
Tests cover normal
and refusal cases, YAML 1.1 identities, aliases/merges, BOM/CRLF, and expansion
budgets. The YAML dependencies and their unmodified license are recorded in
`crates/little-clip-core/third_party/wireviz-notices.md`.

Validation for this checkpoint: the complete core test suite and doctests pass
(377 passed, 3 ignored), as do core all-targets Clippy with warnings denied,
workspace formatting and whitespace checks. Architecture comparison introduces
no high-complexity functions or dependency cycles. Its clone warnings are the
required unmodified MIT license and existing unchanged format-registry branches
in the file receiving the new module declaration; neither justifies changing
upstream license text or unrelated format routing.

### Native rendering and routes

The existing compiled Graphviz engine lays out native connector pin tables,
cable conductor tables, shields and resolved links. No WireViz Python runtime,
server, browser engine or new layout dependency is introduced. Names, labels and
notes use the existing XML escaper and are always literal text. Node/port IDs
come from bounded indices. Standard wire color abbreviations and explicit RGB
bands share a core parser; unknown colors are errors. Parallel border bands
keep white wires visible on the paper background. Rendering additionally caps
visible terminals at 1,024, links at 4,096 and generated DOT at 1 MiB before
entering the existing isolated renderer and its SVG/output/process budgets.

The shared format registry routes `wireviz` Markdown fences, `.wireviz`,
`.wire.yaml` and `.wire.yml`. Generic `.yaml`/`.yml` and clipboard inference
require a complete valid harness of at most 32 KiB with a referenced component;
ordinary YAML, incomplete documents and empty definitions retain text behavior.
Explicit files/fences retain diagnostics for unsupported syntax. A complete
harness beginning with a YAML comment stays source text rather than being
misclassified as Markdown from its `#` line. Rendering failures still fall back
to readable source through the existing preview mechanisms.

Standalone files use the existing highlighted source/live-preview/diagnostics
editor and guarded saves. Browser strips fit the entire harness height rather
than clipping away every electrical link below the titles; hover uses the
enlarged original. Markdown viewers, Markdown hover and the little-screen share
the same engine. `harness.wireviz` and `harness.md` are review fixtures under
`docs/examples/diagrams` and in the user's requested review directory.

Presentation flags, automatic color-code palettes and generated instances still
need compatibility work. The graphical harness editor remains an explicit
priority; the source editor is a baseline, not its replacement.

Validation: the complete core/render suites and doctests pass (472 passed,
4 ignored), the harness clipboard/editor highlighting test passes, and both
desktop hosts pass the real executable worker-dispatch fixtures. The private
native UI scenario `scripts/diagram_wireviz_acceptance.py` passes in light and
dark themes, checking explicit and recognized-YAML strips/popouts, the source
editor, embedded Markdown, real rendered-header pixels and unchanged source
hashes. Screenshots were also inspected for source highlighting, conductor
colors and actual wiring; matching source text alone is not accepted as a
rendering check. Both desktop debug binaries build without warnings.
All-targets Clippy with warnings denied passes for core, render, studios and
both desktop hosts; workspace formatting and whitespace checks also pass.

Architecture comparison leaves maximum complexity and cycle count unchanged
and introduces no high-complexity renderer functions. The existing clipboard
classifier gains one necessary condition (CCN 16 to 17) to distinguish complete
commented diagrams from Markdown. Reported clones touch existing registry and
editor/test code; the new renderer reuses Graphviz, XML escaping, fonts, raster
budgets, process ownership, preview caches and guarded source saving.

### Source-preserving harness transactions

Core's `wireviz::edits::Editable` captures an immutable source revision and its
normalized circuit. Its first operations change pin labels and individual wire
colors, returning text to the existing host undo/save owner. Parsing and editing
belong on bounded workers. A stale captured source is refused; a future UI must
also compare the source revision again before publishing a background result.
These are backend transactions, not a completed graphical editor.

Planning uses the same YAML parser, scalar rules, endpoint resolvers, numeric
range interpretation and model limits as previews. It checks whether every
reference to the affected component still resolves to the same physical pin or
conductor after the proposed edit. Only invalidated or newly ambiguous
references are rewritten to stable IDs, including connector loops. Unchanged
symbolic labels and numeric ranges retain their authored spelling. Duplicate
display labels or wire colors can therefore be introduced without disconnecting
the circuit or accidentally redirecting an existing reference.

Short repeating color lists are materialized before changing one conductor;
other wires keep their previous colors. Missing or short pin-label/color lists
are extended as needed without rebuilding the document. Existing comments,
BOM/CRLF, unrelated quoted values, opaque fields and block scalars are preserved.
New literals use the parser's required quoting, including boolean-like labels
and quoted numeric pin IDs.

After an atomic YAML patch, fresh parsing verifies both the complete expected
YAML value and the independently expected normalized circuit. This refuses
anchor-definition edits that would change another component or even an opaque
metadata field, as well as writes through aliases and inherited merge fields.
No mutation reaches the input snapshot or a file on failure. Limits include
4,096 operations, 512 KiB of patch paths/values, 1 KiB edited labels and the
existing 256 KiB final source limit. Color strings are now validated by the
shared bounded color parser during model assembly, before repeating them across
conductors; a large arbitrary color string cannot multiply into model memory.

Tests exercise literal preservation, rewritten paths/loops, repeated palettes,
missing lists, clearing colors, newly ambiguous labels/colors, alias/merge
refusal, opaque anchor users, stale snapshots and operation/byte/source limits.
The graphical inspector, pin/conductor gestures and connect/disconnect edits
remain required work.

Validation: all core/render tests and doctests pass (485 passed, 4 ignored),
including 12 harness transaction tests. Core/render all-targets Clippy denies
warnings; workspace formatting and whitespace checks pass. No high-complexity
functions or clone groups are introduced. Maximum complexity and cycle count
remain unchanged. The final architecture report's paired new/resolved cycles
contain identical pre-existing Python members in a different order; none of
those Python files changed. No new rendering backend, file writer or undo owner
is added for these transactions.

### Initial native harness inspector

- [x] Click rendered pin, conductor and shield rows using worker-prepared SVG
  geometry, and highlight the actual connected terminals and inspect their path.
- [x] Apply pin labels and individual wire colors through the source-preserving
  transactions above, including swatches, custom colors and clearing a color.
- [x] Share the source editor's undo, guarded saves and accepted-operation close
  lifecycle. Quick Save/Undo commands retain their order while edits are pending;
  failed or superseded edits cannot save an older source.
- [x] Render retained vectors with pointer-centered zoom, middle-button pan and
  Fit. The optional source split shares the same source and history.
- [ ] Add connect/disconnect gestures and further compatibility, including
  templates and generated color palettes. This remains initial support rather
  than complete compatibility; the format checklist stays open.

Generated terminal IDs are presentation metadata only: no external links are
introduced. The renderer supplies named geometry and vector artwork together;
old geometry cannot authorize clicks against a newer source. Both model parsing
and edits run on existing bounded jobs. The inspector has no additional file
writer or undo stack, and whole-connector mating does not invent pin connectivity.

The whiteboard and harness views now share camera transforms and vector/fallback
painting. Worker/UI artwork metadata and source-editor initialization are also
centralized. Architecture measurements show unchanged maximum complexity and
cycle count, lower duplication percentage and two additional small clone groups.
Those groups are the format-specific model-job admission/setup (different result
models and lifecycles); introducing a generic editor job framework for them would
add more machinery than it removes. Other flagged clones are existing test
fixtures/polling, while the reported Python cycle has the same members reordered.

Validation: 37 targeted studio/preview/vector/renderer tests pass, including
shared save/undo ordering and failed/stale edit delivery. Both desktop debug
binaries build; strict all-targets Clippy for the affected crates, workspace
formatting and whitespace checks pass. Native light/dark acceptance exercises
strips, enlarged hover, pin selection, pin-label edits with shield-reference
repair, wire-color selection, guarded saves, exact-source undo, 1600% vector zoom,
Fit and Markdown embedding. Acceptance fixtures end byte-for-byte unchanged.
The existing native whiteboard acceptance also passes: filled artwork and bound
arrows follow a held drag across the original export boundary, with guarded
save, exact-source undo and 1600% zoom preserved.

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

### Integrated Nomnoml rendering

- [x] Route `nomnoml`/`noml` fences and `.nomnoml`/`.noml` files through the shared
  renderer, source editor, live preview and diagnostics.
- [x] Render browser strips and enlarged hover through the existing row cache;
  fit component overviews vertically so paper margins do not hide the diagram.
- [x] Recognize distinctive bracket-to-bracket relations in clipboard text while
  leaving bare labels, checkboxes, Markdown links and directives alone as text.
- [x] Add source highlighting and Markdown/little-screen routing fixtures.
- [ ] Explore source-linked direct editing after the priority graphical editors.

The application calls the unmodified Nomnoml 1.7.0 and Graphre 0.1.3 releases
already imported separately. No new dependency, browser, installed Node runtime
or process integration is needed. Nomnoml and WaveDrom share one small interpreter
boundary: a fresh runtime per call, 128 MiB heap, 512 KiB stack, a three-second
interrupt deadline and a 2 MiB generated-SVG limit. Only trusted bundled scripts
are evaluated; source arrives as a string argument without host bindings, module
loading, filesystem, network or DOM access. Existing disposable workers retain
their outer process/resource limits, cancellation and native SVG/raster path.

The adapter parses before layout, visibly refuses imports and empty figures,
and bounds nested components, relations, text and non-finite/excessive layout
values. It supports upstream classifiers, compartments, nested graphs, relation
styles and custom styles. Native SVG composition outlines labels with the shared
bundled fonts and blocks image resources. Upstream server layout uses estimated
character widths; custom-font and mixed-script inputs disclose this limitation
rather than claiming exact browser font measurement. This is also how the
upstream command-line SVG renderer lays out text without a canvas.

`pipeline.nomnoml` and `nomnoml.md` are authored review fixtures, also copied to
`/home/vega/Documents/type_testing/md_extensions/`. Bundled license output now
includes both original MIT notices. The pristine vendor bundles remain unchanged.

Validation: 45 core diagram tests, 53 renderer/diagram tests and 16 syntax tests
pass. Real worker protocol and license tests pass in both desktop applications,
alongside the standalone viewer-routing test. Strict all-targets Clippy on the
affected crates and both desktop builds pass. Private native light/dark
acceptance verifies normal-height overview strips, painted node artwork in hover
and source/Markdown views, invalid-source diagnostics and exact source retention.
Architecture measurements show no additional clone groups, unchanged maximum
complexity/cycle count and removal of the old WaveDrom wrapper hotspot. Reported
clones are pre-existing extension tables and viewer tests in touched files; the
paired Python cycle reports contain the same unchanged members reordered.

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

The user's review identified the following requirements. They are part of the
active goal, alongside the remaining format coverage, and must be completed before
that goal is marked complete. The user reaffirmed this scope on 2026-11-13:

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
- [x] Render standalone diagram strips in the file-manager list, with enlarged
  rendered hover previews. WaveDrom should prioritize the actual waveforms over
  decorative heading text. Preserve useful scale, clip vertically when needed,
  and reuse existing cache admission, revision checks and hidden-row retirement.
  Verify timing diagrams and representative other supported families in actual
  list rows and hover popouts, including oversized diagrams and failed rendering.
- [x] Give Markdown front matter a compact metadata area beside a smaller text
  minimap. Prioritize supplied Hugo fields (`title`, `date`, `draft`, `tags`,
  categories and related dates/weight) without increasing row height. Do not
  infer publication state or project timezone from those fields.
- [x] Replace Markdown's syntax-only hover excerpt with a rendered view at the
  hovered source position, including equations and diagrams. Preserve useful
  source-position mapping and bounded work for large documents. Verify hovering
  different positions in a long Markdown file reaches the corresponding rendered
  content, rather than always showing the document's beginning; retain an
  inspectable fallback when a fragment cannot render.

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

### Shared runtime extraction

`clip-ui::vectors` now owns the validated callback protocol. Each egui host has
an independent registration and UI-thread renderer; callbacks verify the
painter's actual context identity before touching GPU resources. The first
failure remains visible and stops later native callbacks. Explicit destruction
checks the owning context and removes only that host's registration. A missed
exit hook or panic never attempts GL deletion without a current-context
guarantee; its last-resort fallback leaves reclamation to native context teardown.

The test application uses this shared implementation, replacing its private
renderer registry and callback code. Light/dark desktop scenarios pass, as do
two lifecycle tests covering stale owners, independent contexts, sticky errors
and rejected callbacks. Strict all-target UI Clippy and UI doctests pass.
FemtoVG is now a shared UI dependency, still with text/image loaders disabled.
The applications' host hooks and the actual retained SVG scene remain the next
integration steps; no production drawing canvas has switched renderers yet.

This backend intentionally refuses an unexpected host-owned offscreen FBO:
FemtoVG's external-framebuffer adapter takes ownership, so handing it egui's
borrowed target would risk deleting that target. Current native egui glow uses
the window framebuffer. A future offscreen host needs an explicitly owned
composition surface rather than treating a borrowed FBO as disposable.

Architecture comparison reports no new clone groups or hotspot, with maximum
complexity and cycle count unchanged and slightly lower duplication. Its sole
reported cycle change is the same untouched Python cycle with reordered members.

### Desktop host lifecycle

File manager and workbench now install their own shared vector runtime from
eframe's creation context and release it in `App::on_exit`, before eframe
destroys its painter/context. Workbench fixtures retain a headless `None`
owner. Initialization and cleanup failures are logged; an unavailable renderer
cannot silently acquire a different context. Application navigation and studio
state remain independently owned by their existing hosts.

Both desktop binaries build, all-target checks and strict Clippy pass, and
formatting passes. The private file-manager drawing scenario passes creation,
guarded save, exact source undo and clean close. The private workbench scenario
passes fixed/centered sizing, singleton focus and Escape dismissal with exit
status zero. An initial gallery run used the popup-close assertion even though
Escape intentionally returns that viewer to the palette; the ordinary popup
run verifies actual application exit instead. Neither application reports a
native-vector initialization or cleanup failure in these runs.

The cumulative architecture comparison keeps maximum complexity and cycle
count unchanged. Its one new clone is the explicit exit-hook adapter in the two
independent `eframe::App` implementations. These short host-owned safety
boundaries deliberately repeat the same cleanup contract; extracting them
would add another optional-owner/logging wrapper or bring eframe's windowing
dependency into the shared widget API. Existing fixture clone windows and
reordered untouched Python cycles do not represent new application mechanisms.

### Retained drawing artwork

The drawing canvas now uses FemtoVG for retained paths, outlined handwriting,
ordinary fills/strokes, gradients, embedded images, clip paths and group opacity.
Zoom tessellates at the current camera scale, and moving an object translates
its actual artwork and bound text before the gesture is released. The camera
reports its zoom percentage. Source mutation remains one accepted operation per
gesture, with the same guarded saves and text undo history. A committed movement
retains its visual translation until the replacement scene arrives, independently
of selection feedback using the newly committed model.

The existing renderer still owns canonical SVG normalization, native glyph
outlining, image admission and bounded source/structure limits. Investigation
found that usvg 0.49, already used by the native drawing dependency, owns a
thread-safe tree. Retained preparation uses that tree instead of inventing a
second geometry interchange format. The existing usvg 0.37 normalization/raster
path remains unchanged. Both parser-version adapters call the same image decoder
and budget; retained preparation keeps its decoded pixels for UI-thread upload.
Neither adapter opens external image paths or discovers fonts.

Retained preparation rejects unoutlined text, unavailable images, excessive
source/depth and expanded geometry, including a small `<use>` document whose
expanded tree exceeds the node budget. It retains all eight source object
identities and the embedded raster in the real drawing fixture. GPU path/clip
caches stay in the context owner. They hold only weak scene ownership and release
expired image resources during current-context painting. Clip geometry is
resolved on its worker and converted once into cached native paths, rather than
being rewalked and rebuilt for every pointer frame.

Masks, filters, non-normal blend modes, repeating gradients, patterns and nested
clip unions still select the existing raster preview with a visible notice.
They are not silently omitted. This is an initial native drawing pass, not
completion of the broader editor parity requirements above.

Validation at this checkpoint:

- Renderer library: 83 tests pass; one existing native-compute GPU test is ignored.
- Shared UI library: all 34 tests pass, including explicit unsupported-effect
  fallback and independent renderer lifetime checks.
- Strict all-target Clippy passes for render, UI and studios; formatting passes.
- Native light/dark file-manager scenarios verify filled movement while the
  pointer remains held, the other object staying stationary, no file mutation
  before release, guarded save, exact source/asset undo, and 1600% zoom.
  Screenshots show crisp outlined glyphs at that zoom. The private runner uses
  software OpenGL, so this does not establish parity across hardware drivers.

The private pointer helper has a bounded held-drag protocol, released by input,
EOF or timeout. The drawing scenarios share fixture/bootstrap/opening code.
Architecture comparison leaves maximum complexity and dependency-cycle count
unchanged. Scheduling no longer also contains the derived-artifact preparation
pipeline, and clip compilation replaces duplicated validation/render traversal.
The remaining SVG clone is only the seven-line typed signature of the group and
child traversal methods. The existing private C input test's main function adds
three branches for the held-drag command; its wait/release protocol is separate.
Reordered members of untouched Python cycles do not introduce new cycles.

Follow-up from the user's first native review, to be fixed in separate commits:

- [x] Remove export-viewport clipping from the native interactive canvas. Dragging past
  the old drawing bounds must remain visible, while authored frame/image clips
  remain intentional.
- [x] Update supported bound arrows during pointer movement, including endpoint
  and label movement, instead of waiting for the source operation to finish.
  Rotated/elbowed path stretching and unsupported mask effects remain explicit
  limitations of the current editor, not claims of complete editor parity.

During this work, a serialized cleanup removed 323 superseded hashed debug/test
executables. It retained the newest two per target, top-level application and
example hardlinks, running executable identities, and every dependency library.
Reported repository usage fell from about 279 GiB to 127 GiB; free filesystem
space rose to about 127 GiB at cleanup completion. The discarded executable
variants are not all regenerated by an ordinary next build. The removal manifest
is `/tmp/little-clip-debug-executable-cleanup-20261113.json`. No source or personal
build settings were changed by that cleanup.

### Review fix: export bounds are not editor bounds

Retained preparation now makes only `compose_svg`'s direct export-plane SVGs
overflow-visible. Their sizes/transforms still establish initial placement, but
they cannot clip live movement to the last export bounds. Deeper authored SVG
crops and explicit clip paths are preserved. Byte-range edits affect only the
retained preparation copy; canonical export SVG, source JSON and saves are unchanged.

The held-drag regression moves the filled rectangle 200 pixels left, past its
old export boundary while staying inside the editor widget. Before the fix,
its visible left edge stopped at x=335 instead of reaching x=145. The same test
now passes in light and dark, with the other object stationary, followed by
guarded save, exact source undo and 1600% zoom. Three retained-preparation tests
pass, including preservation of both an authored nested-SVG crop and an explicit
frame clip. Strict render all-target Clippy and both desktop builds pass. The
baseline's eight studio drawing lifecycle/input tests also passed after its commit.
Architecture comparison adds no clone group or hotspot and retains the cycle
count; reported cycle replacements only reorder untouched Python members.

### Review fix: live bound connections

The core movement plan now supplies both source transactions and live previews:
rigid object/label offsets and changed connection origins/points. This keeps
bound endpoints, grouped movement and label centers consistent with the saved
result without mutating source/history during pointer movement.

A small local native-renderer adapter reuses its existing normalization, seeded
Rough geometry, arrowhead construction and SVG paint writer. It prepares styling
once per source revision, then regenerates only changed connection geometry on
the existing bounded worker pool. One request is in flight; newer pointer
positions coalesce. Results are tied to their immutable model identity; hidden,
cancelled and replaced work retires normally. Source/element, geometry, work and
aggregate output budgets apply, with cooperative cancellation between arrows.
The native scene replaces the identified geometry group inside its existing
paint order and clipping. It does not stretch arrowhead pixels or parse SVG on
the UI thread. The old model/geometry remains owned through publication until
the canonical replacement arrives. Existing unsupported rotated/elbowed routing
still fails atomically with its existing diagnostic; mask/filter fallback is
unchanged and remains part of broader native editor parity work.

Validation: nine core movement tests pass; eight existing drawing/editor tests
and the new connection cancellation/placement test pass. A native geometry test
compares three arrowhead/stroke/roughness combinations against committed exports
(with sub-millipixel tolerance for the normalizer's floating-point round trip).
The held-drag visual assertion fails on the previous binary and passes in light
and dark themes with the fix, alongside guarded save, exact-source/asset undo,
stationary-object checks, movement beyond old export bounds and 1600% zoom.
Both desktop binaries build; strict all-target Clippy for core/render/UI/studios
passes. Software OpenGL acceptance is not a hardware-driver parity claim.

Architecture review: maximum complexity and cycle count remain unchanged. The
writer adds one feature-gated source-identity call; ID encoding is shared with
the existing source-group writer. The remaining short clones are common task
result handling and independent coordinate admission at the model/renderer
boundaries; introducing a new cross-domain abstraction for them would add more
coupling. Pre-existing upstream documentation/backend clones and reordered
members of unchanged Python cycles are not new implementations.

### Viewer state size check

- [x] Keep the optional per-session diagram preview behind one owned box.
  Adding preview state had pushed the containing text viewer over Clippy's enum
  size threshold. This keeps cache growth inside its existing session owner,
  without changing viewer dispatch, navigation or save ownership. The 25 shared
  text-editor tests pass; strict all-target Clippy for both desktop hosts passes.

### Initial rendered diagram strips ready for review

Standalone formats recognized by the shared diagram registry now render in the
file-manager list. The visible text-row worker reads a bounded complete source
from its existing guarded file handle, retaining the ordinary text summary as a
fallback. The same row cache owns the rendered pixels and supplies an enlarged
hover popout; no second per-format cache or UI-thread decoder was introduced.
Source revisions, context changes, hidden-row retirement and cancellation retain
their existing owners. Source identity is checked before and after rendering.

Short diagrams fit their row; tall diagrams preserve a bounded readable slice,
and the larger popout remains vertically clipped. Timing strips use a smaller
scale to expose more lanes. WaveDrom title text is omitted only from the passive
preview copy; lane names, groups, time ticks and editable source remain intact.
Invalid renders retain the syntax minimap, show an alert marker, and disclose the
rendering failure alongside the source excerpt on hover. Ordinary JSON/XML and
Markdown keep their existing conservative classification. Diagram clicks do not
invent source-line correspondence from image coordinates.

Validation: three diagram-strip tests cover classification/fallback/source
budgets, title suppression with time ticks retained, and stale-source rejection.
All 12 text-strip tests pass; the 25 shared text-editor tests pass for the compact
preview-owner change. Both desktop binaries build and strict all-target Clippy
passes. Private light/dark scenarios verify rendered timing/flowchart rows,
normal row height, enlarged hover content, invalid-source diagnostics and exact
source-byte preservation. Raster previews are sampled smoothly while syntax
minimaps retain their existing nearest sampling.

Architecture review moved diagram admission/diagnostics into the diagram strip
module, leaving text loading below the hotspot threshold. Row and strip hover
now share one dispatch point, ready for rendered Markdown hover. No new hotspot
or clone group was introduced, maximum complexity/cycle count is unchanged, and
measured duplication decreased slightly. Reported clones in the already-present
text editor/row declarations are unchanged; reordered members of existing Python
cycles are not new cycles.

Review the existing files in `/home/vega/Documents/type_testing/md_extensions/`.
Rendered Markdown hover at the selected source position remains pending and is
still required before the active goal can be completed.

### Markdown hover foundation: native source correspondence

- [x] Retain UTF-8 source ranges from the existing native Markdown parser.
  The plain and located APIs share the same emitter; this does not add a second
  Markdown grammar or change the rendered elements. Front matter includes its
  framing, code lines/fences retain their actual byte ranges, and multiline
  equations cover all consumed physical lines. An equation's trailing paragraph
  may share that range. BOM and CRLF positions remain relative to the original
  source, including explicit incomplete-equation ranges.
- [x] Use those ranges to prepare bounded hover excerpts around the selected
  source position, preserving complete diagram/math blocks and retiring hidden
  preview jobs. The completed hover integration below supplies the presentation;
  the parser foundation alone was not a completion claim.

Validation: the 15 render Markdown tests and nine shared studio Markdown tests
pass. New cases verify exact Unicode/front-matter/fence/CRLF/multiline-math
ranges and preservation of the existing element sequence. Strict all-target
Clippy for render, studios and UI passes. Ordinary parsing collects directly
through the shared emitter, without allocating the additional located model.

The framing state owns an unfinished display equation's source start alongside
its accumulated text. This removes a parallel per-line start/reset condition
from the body parser; adding correspondence does not add another grammar branch.
The architecture check's reordered Python-cycle members are unchanged cycles.

### Rendered Markdown hover at the selected source position

The text strip now requests a guarded, worker-prepared Markdown excerpt for the
hovered line. It uses the original native element ranges and the shared studio
Markdown presentation, including equation/diagram preview ownership. Fenced
blocks stay complete; incomplete fences/equations, unavailable sources and
budget failures retain the highlighted source excerpt with an explicit error.
Loading shows a compact pending state rather than flashing syntax-highlighted
content as if it were the completed rendered view.

The selector keeps a nearby heading but excludes preceding large blocks or
metadata tables that would hide the selected content. Diagram fences keep their
complete source and their native renderer limits. Ordinary excerpts bound bytes
and span count. The input limit is 2 MiB, matching the existing full text viewer;
larger files disclose that limit and retain their selected source excerpt,
rather than silently showing a prefix as though it were the requested position.
Indentation uses one shared bounded rule for ordinary and mathematical lists.

The existing one-hover cache admits the preparation job and retires replaced,
hidden, navigation and context-invalidated requests. The host owns only the
active excerpt's native view and drops its math/diagram jobs with it. Reads use
the captured ordinary FileSource identity, guarded open and checks before result
publication; no source parsing or file reading was added to the UI callback.

Validation: 13 shared studio Markdown tests, 15 render Markdown tests, three
file-manager Markdown tests and three matching workbench tests pass. Coverage
includes source-position selection, complete diagrams/equations, front-matter
body visibility, incomplete/oversized/cancelled input and replaced source
identity. Strict all-target Clippy for both desktop hosts and studios passes.
Private light/dark scenarios verify the actual rendered first/last sections,
complete diagram popouts, incomplete-fence fallback and unchanged file bytes.
The acceptance helper can reject loading/failure states; loading no longer
contains the source text that initially allowed a false positive. Screenshots
were inspected to confirm styled output rather than trusting matching words.

Architecture review reports no new maximum-complexity or cycle increase and a
small duplication decrease. Text-row loading adds one explicit eligibility
condition (CCN 14 to 16) to keep ambiguous diagram files out of Markdown
presentation; this is necessary classification, not duplicated parsing. The
remainder reuses the existing hover cache, parser, native renderer and preview
lifecycles. Existing declaration clones and reordered Python-cycle members are
unchanged.

## DBML parser selection and compatibility probe

Compared published [`dbml-rs` 1.0.1](https://github.com/apskhem/dbml-rs),
[`dbml-language-server` 0.1.2](https://github.com/kamal-hamza/dbml-language-server)
and [`dbml-mermaid`](https://github.com/notpeter/dbml-mermaid) at
`63bd9d43b12d10a93f2c88bb88d4bda72fac4630`. The first is MIT/Apache-2.0,
preserves source ranges and validates references; the LSP package pulls in
server/runtime dependencies, while the dependency-free Mermaid converter silently
skips non-table/ref blocks. Select the native parser with the existing native
Graphviz engine rather than writing another grammar or installing a runtime.

The official [DBML syntax](https://dbml.dbdiagram.io/docs/) now includes partials,
records and optional relationship operators not accepted by this parser release.
Those remain explicit compatibility work: never strip them or silently render a
partial schema. Full-source parsing must fail visibly for unsupported syntax.

Reviewed [DBML Editor](https://github.com/alswl/dbml-editor) (MIT, AntV X6 + Monaco
+ upstream JS parser) and [erd-editor](https://github.com/dineug/erd-editor) as
interaction alternatives. They are browser applications rather than embeddable
egui editors. Table/column selection and source-linked relationship editing are
useful future interactions; highlighted source/live preview remains the initial
baseline. No browser UI code has been copied.

- [x] Import pristine parser source separately before modifications.
- [x] Fix an upstream analyzer panic on valid bare string types with defaults;
  bound checks use optional length arguments and count Unicode characters.
  The focused parser regression and four native renderer probes pass.
- [x] Finish file/Markdown/clipboard routing and native strip/editor validation.
- [ ] Expand modern DBML syntax compatibility and investigate direct editing.

The vendored integration record gives the upstream revision, published checksum
and exact patch. Original licenses remain intact. Parser expressions/defaults are
text; no SQL or database access is performed.

### Initial native DBML integration

`.dbml` files, `dbml` fences and distinctive table-declaration clipboard snippets
use the shared format registry, guarded highlighted source editor, bounded live
preview, Markdown/little-screen rendering and file-manager strip/hover caches.
Generic enum declarations and SQL are not claimed as DBML snippets. Failed
inference remains highlighted text. Review fixtures `schema.dbml` and `dbml.md`
are also available under `/home/vega/Documents/type_testing/md_extensions/`.

The adapter shows schema-qualified tables, columns/types, PK/UQ/NN/AUTO flags,
defaults, column/table notes, indexes (including composite primary keys) and enum
values. Aliases resolve to their real tables. All four relationship directions
and composite column pairs use actual field ports; separate incoming/outgoing
outer-cell ports prevent arrows from crossing the field's type/flag text. A
retained-SVG geometry regression verifies that edges stay between table borders.
Symbols describe maximum one/many cardinality, not minimum participation.

Source parsing is complete, not a filter that drops unsupported blocks. Partials,
records and newer optional relationship syntax still report errors and remain
required compatibility work. Project/group/free-note context and unpainted
presentation settings/ref metadata produce visible limitations. No database,
expression or SQL execution is provided. Render limits are 128 tables/enums,
2,048 fields/enum values, 2,048 column edges and 1 MiB of generated DOT inside the
existing source, process, SVG and raster budgets. The original native Graphviz
HTML-label escaper is now shared with harness diagrams.

Validation includes 46 core diagram tests, 59 renderer/diagram tests, six focused
DBML tests, the upstream analyzer regression, actual helper/notice tests for both
hosts, browser strip/source-retirement tests and clipboard fallback/highlighting.
Both desktop binaries build without warnings and strict affected-crate all-targets
Clippy and workspace formatting pass. One schema acceptance scenario now covers
both Nomnoml and DBML, rather than adding a duplicate native UI script.

Architecture measurements include the newly vendored upstream parser: its three
existing high-complexity functions and ten internal clone groups account for most
of the reported regression. The DBML adapter itself has no duplicate groups and
maximum complexity 12. The renderer's exhaustive format dispatch gains one case
(CCN 15 to 16), which is necessary registry growth, not another rendering mechanism.
Other flagged viewer clones already existed in the touched test module. Keep the
upstream parser intact beyond the recorded fix instead of restructuring third-party
code to improve our metrics. Current DBML format checkboxes remain open for the
explicit newer-syntax and richer visualization work above.

Native schema acceptance passes in both light and dark themes for DBML and
Nomnoml, including source-retaining navigation and painted-artwork checks in the
normal strip, enlarged popout, source/live preview and embedded Markdown views.
Microsoft LINQ-to-SQL `.dbml` XML is explicitly excluded from diagram inference
and keeps XML highlighting; malformed XML drafts are excluded too. The shared
extension/source dispatch is now a direct per-extension match, retaining the
existing Mermaid/DOT/BPMN/JSON/YAML classification tests.

### Clipboard Markdown framing correction

- [x] Require DBML/Nomnoml snippet candidates to begin as their source language,
  rather than finding a table or relation somewhere inside a Markdown document.
  Leading source comments, compact Nomnoml directives and DBML project blocks
  remain recognized. Markdown headings, link/prose introductions, front matter
  and fenced blocks keep their document route and exact clipboard bytes.

A regression reproduced the original fenced-Nomnoml misclassification before the
fix. Core classification tests now verify both languages and the resulting
Markdown payload, while the little-screen route test verifies prepared artwork
for copied whole Markdown documents as well as standalone sources/files. This
changes only best-effort clipboard inference; explicit files and fence languages
retain their existing parser/rendering behavior.

Validation: all 47 core diagram tests, the expanded little-screen route test,
strict core/render all-targets Clippy, workspace formatting and final desktop
builds pass. Architecture clone-group and maximum-complexity counts are unchanged;
the report flags the existing language/extension lookup tables and reordered
members of untouched Python cycles. No new cache, parser runtime or renderer is
introduced for this classification fix.

## GoAT selection and integration

Compared the native Rust [aasvg 1.0.0](https://github.com/bearcove/aasvg-rs), its
current Dodeca source, existing SvgBob 0.7.6 and upstream
[GoAT 0.5.1](https://github.com/blampe/goat). `formulaa` is a TUI math editor rather
than an ASCII diagram parser. aasvg is a related Markdeep port; common ancestry
is not evidence of equivalent geometry. A standalone Rust probe and pinned Go
CLI comparison covered hello-world, overlaps, line decorations, hollow circles
and the complicated diagram. The line-decoration case differed materially:
Rust emitted 11 arrow polygons and six text elements, versus GoAT's 15 arrows and
no text. Other cases differed too, including preservation of leading indentation.

Select the actual upstream ASCII/UTF-8 libraries through a compiled, data-only
companion. Vendored sources/licenses were committed pristine before integration;
the small application adapter lives separately. Builds now require Go 1.25+;
end users need only the shipped static companion, not Go or an installed GoAT.
The build is offline and CGO-free, with pinned local module replacements and no
toolchain downloads. The build/deployment/provenance details are recorded in
[`scripts/goat-renderer/README.md`](../scripts/goat-renderer/README.md).

- [x] Share guarded file/Markdown/clipboard rendering, source editor, live preview,
  diagnostics, list strips and enlarged hover without a second preview cache.
- [x] Use the upstream ASCII grammar and Unicode box frontend; box-drawing glyphs
  select the latter. Preserve authored whitespace; report tabs and budget failures
  instead of silently changing grid alignment.
- [x] Bound source, rows/columns, generated SVG, process lifetime and transport.
  Go gets a soft GC target in addition to the existing hard child limits. No
  filenames, external stylesheets, imports or arbitrary CLI arguments are exposed.
- [x] Outline fonts and block images through the shared native SVG composer.
- [x] Verify offline helper tests, original marker semantics, real host workers,
  license availability and relocated sibling-companion discovery.
- [ ] Explore source-linked ASCII grid editing after the priority graphical editors.

Reviewed [MonoSketch](https://github.com/tuanchauict/MonoSketch) (Apache-2.0) and
[ASCIIFlow](https://github.com/lewish/asciiflow) as graphical editing references.
Their cell-grid rectangles, line/arrow tools and text selection are relevant, but
their browser applications are not native egui components. No browser UI or engine
is embedded. The baseline remains highlighted source with immediate rendering and
errors; a future cell editor must preserve the shared history/save lifecycle.

`pipeline.goat`, `unicode.goat` and `goat.md` are authored review fixtures, also
copied to `/home/vega/Documents/type_testing/md_extensions/`. Closed rounded or
Unicode frames provide conservative clipboard hints; shared square-box syntax
retains its existing SvgBob hint. Fenced Markdown is never inferred as raw GoAT.

Architecture measurement includes the imported upstream GoAT/CSS packages and
their existing duplication. The application adds one compiled renderer adapter
and its build/transport glue, reusing CommandRequest, native SVG composition and
all source/cache owners. The additional exhaustive renderer dispatch case is
necessary format-catalog growth. Original third-party license duplication is
retained for correct attribution rather than changed to influence metrics.

Validation: offline Go helper tests passed, as did the 48 core diagram tests,
61 render diagram tests and both desktop hosts' worker integration tests. The
relocated-companion test passes. Both desktop binaries build successfully; strict
Clippy across core, render, studios and both hosts' all-targets passes without
warnings. The native dark-theme acceptance run passed all 22 observations for
Nomnoml, DBML and GoAT strips, hover, source/errors and embedded Markdown.
Final architecture comparison retains maximum CCN 195 and reduces duplication
by 0.06 percentage points. The imported upstream sources account for the new
algorithm hotspots/clone groups; the shared Go adapter test XML counter removes
the newly introduced local test duplication. Existing catalog/lookup tables and
untouched viewer clones remain, along with original license texts.

The final light-theme run also passed all 22 observations. Its first attempt
failed on OCR of the initial folder names despite a correct rendered list; the
harness now checks the initial folder count, then still verifies each family's
actual filenames, artwork, popout and editor after directly addressing its folder.

## Bytefield selection and integration

Compared the requested [Bytefield-SVG](https://github.com/Deep-Symmetry/bytefield-svg)
Clojure DSL with the Rust `bytefield` crate (unrelated bitfield storage), native
[ProtoViz](https://github.com/danielstuart14/protoviz) and Python
[Framegram](https://github.com/czarnota/framegram). The latter two draw binary
structures from different input models, not Bytefield programs. Existing
WaveDrom register rendering likewise does not implement this Clojure language.
Reimplementing Bytefield would require a Lisp evaluator, helper functions,
attribute inheritance, nested SVG content, gaps and layout semantics. Instead,
reuse the existing bounded hostless QuickJS adapter for the original compiled
upstream bundle. No Node process, browser engine or installed runtime is added.

Evaluated upstream 1.12.0 and 1.11.0 under both Node (reference only) and QuickJS.
The released 1.12.0 bundle fails `(defn field [s n] (draw-box (str/upper-case s)
{:span n})) (field "Type" 2)` under Node too. Pin 1.11.0, where that helper and
all seven shipped protocol/sample fixtures render. The original sources and
bundles were committed independently before application changes. No upstream
algorithm changes were made. Source/package hashes, original EPL-2.0 license,
EPL-1.0 runtime notices and Apache-2.0 Closure attribution are retained in
`bytefield-notices.md` and the combined `--diagram-licenses` output.

- [x] Shared `bytefield` Markdown fences, both large/small previews, best-effort
  clipboard hints, `.bytefield` standalone source/live preview/errors, rendered
  list strips and enlarged popouts.
- [x] Recognize the upstream `.edn` extension by actual drawing list heads;
  generic EDN/Clojure data and quoted examples retain ordinary text behavior.
  Ordinary Clojure program suffixes keep their code viewer association.
  `.bytefield` is the explicit application association, not a claim that upstream
  standardizes a separate filename suffix.
- [x] Reuse the same guarded source, history/save, asynchronous rendering and
  preview cache owners. No new file reader or save mechanism.
- [x] Keep a fresh interpreter per call, 128 MiB heap, three-second deadline,
  2 MiB SVG transport and existing outer helper limits. SCI 1.11 also uses its
  termination-safe preset and 10,000 realization cap. A known-size scoped native
  thread gives the Clojure evaluator a bounded 4 MiB interpreter stack; the
  diagram-only engines retain 512 KiB. This prevents fixture failures caused by
  differing Rust caller-thread stacks, without unsafe stack manipulation.
- [x] Preserve line/column SCI errors as readable text instead of opaque QuickJS
  object addresses. Use complete namespaced SVG output, bundled metric fonts,
  outlined text and image blocking. Invalid XML labels produce an actionable
  diagnostic: upstream expects XML escaping for literal `<` and `&`.
- [ ] Source-linked field resizing/selection where a literal source call exists;
  arbitrary Clojure helper output must not be rewritten as if it were a literal.

Editor research: upstream and its Asciidoctor wrapper are source-to-SVG tools,
not native graphical components. ProtoViz provides a useful configurable field
model but uses its own syntax. Framegram's nested bit-field rows and
[BinTV](https://github.com/tomerfry/binteractiview)'s synchronized structure/byte
highlighting suggest future linked field selection; BinTV interprets Construct,
not Bytefield, and is not a component to embed. No editor code was copied.
For this non-priority editor, source/highlighting/live preview/errors is the
baseline; priority native drawing work remains WireViz, WaveDrom, Mermaid and
Excalidraw.

Review fixtures `packet.bytefield`, `packet.edn` and `bytefield.md` are in the
user's `type_testing/md_extensions/` folder. The repository keeps the authored
standalone source and embedded Markdown under `docs/examples/diagrams/`.

Architecture comparison against `before-next-diagram-family` reduces duplication
by 0.04 percentage points and average CCN by 0.01. Maximum CCN increases from
195 to 274 inside the unmodified minified upstream ClojureScript bundle (`ca`),
not application Rust. The dispatch match adds its required format case. Original
runtime license texts and their duplication are retained. The 12-line shared
shape of independent GoAT/Bytefield failure-case tests verifies the same public
error contract with different inputs; replacing those explicit assertions with
a trivial generic wrapper would obscure rather than simplify them. Interpreter
thread ownership and evaluation remain separate cohesive methods; no new Rust
production hotspot, cache, global runtime or file-save owner is introduced.

Bytefield validation: 49 core diagram tests, 63 render diagram tests, ten card
preview tests, three file-strip tests and the native-extension viewer test pass.
Both real-host worker suites (including licenses and relocation) pass. Strict
all-targets Clippy across core/render/studios/both desktop hosts passes with
`-D warnings`; both desktop binaries build successfully. Final private native
runs pass in light and dark themes, covering actual field-color pixels in the
strip, popout, source/live preview and Markdown, error display and unchanged
source hashes. The harness now scrolls to the source affordance after checking
artwork because a legitimate tall diagram can place that control below the
viewport. Earlier attempts exposed a singular/plural harness check and an
executable replacement during overlapping development builds; the final runs
use the completed build without overlapping compilation.

## Erd parser diagnostics

- [x] Count classic CR-only line endings as well as LF and CRLF in parse errors.
  A targeted test verifies line 2, column 10 for the same malformed statement
  under all three encodings. Authored bytes and token offsets remain unchanged.

## Erd research and native implementation

The requested Erd syntax is the original [bracket/entity language](https://github.com/BurntSushi/erd),
not the newer Rust `rusterd` DSL. Compared rusterd 0.3.1 (different `entity/rel`
language), erd_rs 0.1.0 (Rust-source-to-Mermaid tool), `fastbreak` (formal methods
specification language), [erd-rs](https://github.com/davechallis/erd-rs),
[erd-go](https://github.com/kaishuu0123/erd-go) and original Erd. The native erd-rs
parser prototype parsed basic entities but rejected hyphenated settings,
fractional field font sizes and Unicode identifiers; it also applied global
entity background over a local override. Its renderer leaves much formatting
unfinished. The prototype used its unmodified parser with the already-built,
API-compatible nom 7 dependency; the source itself requests nom 6. The failures
follow its grammar/model, not a different interpretation of the requested format.
No candidate code was assumed compatible just because it draws ER diagrams.

Choose a small native implementation of the original grammar and semantic model,
retaining exact authored identifier/option token spans. It adds no dependency or
installed tool and reuses the existing Graphviz layout, HTML escaping, bundled
fonts, SVG composition and guarded preview/source lifecycle. Original grammar,
semantics, presentation reference, license and fixtures at
`d18075d205583458b477cdcb7fe8bb41ec092cf9` were imported unmodified in a separate
commit under `vendor/erd-reference`. The reference is Unlicense; no Haskell
runtime or external configuration-file lookup is used.

- [x] Parse quoted/bare/Unicode identifiers, ordered entities/attributes,
  independent primary/foreign keys, relationships before declarations and all
  four participation cardinalities. Retain minimums in explicit UML labels.
- [x] Parse all twelve original settings, multiline maps, comments/trailing
  commas, global title/header/entity/relationship scopes and local precedence.
  Unknown settings or invalid/excessive numeric values return locations.
- [x] Bound source at 256 KiB, entities at 128, attributes at 4096, relationships
  at 2048, names at 4096 bytes and setting values at 8192 bytes. No recursive
  parser. Validate every relationship and refuse ambiguous duplicate entities.
- [x] Preserve underlined primary keys, italic foreign keys, labels, table/cell
  formatting, fonts and explicit global/local precedence in native DOT output.
- [x] Standalone `.er/.erd`, `erd` Markdown fences and conservative clipboard
  hints share format identity. Bracket headings alone are too ambiguous for a
  clipboard claim; a complete cardinality relationship supplies the hint.
- [x] Keep Eclipse ERMaster XML and incomplete XML drafts out of the Erd renderer;
  use ordinary XML highlighting and previews instead.
- [x] Complete real-host, warning-free and native visual validation.

Editor comparison: original Erd and erd-go are source/DOT tools, and the
[VS Code Erd extension](https://github.com/kaishuu0123/vscode-erd) is likewise a
source-linked preview rather than a graphical editor. The [t2erd](https://github.com/dosaki/t2erd)
JavaScript renderer has a different styling/layout dialect and GPL-3.0 license;
its code is not imported. The existing DBML table preview offers shared native
presentation patterns, while source-preserving schema editing can use this
parser's token spans. For this non-priority editor, source/highlighting/live
preview/errors is the initial baseline. Direct table/field/relationship editing
remains future work after the priority editors; no destructive round-trip
serializer is substituted for the authored source.

Review fixtures: `people.er`, the same source in `people.erd`, and `erd.md` under
`type_testing/md_extensions/`. Repository examples retain the authored `.er`
source and embedded Markdown. The original `simple.er` and `nfldb.er` fixtures
are used by parser and renderer tests; the latter contains seven entities and
thirteen relationships.

Erd validation: 54 shared core diagram tests plus the targeted CR/LF diagnostic
test pass; 15 syntax tests, 66 render diagram tests, ten card-preview tests,
three file-strip tests, the native-extension viewer test and both desktop host
worker suites pass. Strict all-targets Clippy across core/render/studios/both
hosts and the final file-manager fit adjustment passes with `-D warnings`;
both desktop binaries build successfully. Native light/dark runs each pass
nine observations with actual table-color pixels and source hashes preserved.
The first visual run exposed title-only cropping; Erd now shares the existing
fit-height treatment for DBML/Nomnoml/WireViz component overviews.

Architecture comparison against `before-erd` leaves maximum CCN 274 unchanged
and reduces duplication by 0.02 percentage points. The required catalog case
adds one branch to the renderer dispatch. The two added clone groups are in
retained upstream reference fixtures; existing lookup tables and untouched
viewer/test clones are also flagged because their files changed. No new Rust
parser/renderer hotspot, dependency cycle, cache, runtime dependency, generic
factory or file owner is introduced. The native parser/renderer growth is the
actual requested language/model/presentation, not a second diagram pipeline.

## PlantUML and C4 native implementation

Compared [plantuml-core 0.5.4](https://github.com/bjornharrtell/plantuml-rs),
[puml 0.2.6](https://github.com/alliecatowo/puml) and
[plantuml-little 1.2026.2-4](https://github.com/Actrium/plantuml-little), including
actual crate sources, license notices and preprocessor/resource APIs. All three
have native Rust implementations; PlantUML encoding/Markdown wrapper crates do
not themselves supply a renderer. plantuml-little's default native backend adds
graphviz-anywhere's prebuilt native library; its reference-test mode uses a
Node/wasm backend. puml has a rich normalized scene and language-service model,
but its default local include path reads files; disabling its URL feature alone
does not prevent those reads. Its source-correspondence API remains a useful
future reference.

Choose plantuml-core: no filesystem/network/command I/O in its production core,
a replaceable resolver with a bundled-only implementation, bundled C4 macros
and themes, and the same pure-Rust Graphviz family already used here. The
upstream reports a broad reference corpus; our selection additionally exercises
sequence, class, state, activity, mindmap, JSON, Salt and C4 inputs rather than
relying on those claims. Its MIT notice explicitly selects upstream PlantUML's
MIT option; C4's original MIT notice and theme authors/licenses are retained.
No GPL-only Java artifact, browser, Node process or JVM is introduced.

The pristine 0.5.4 crate (repository revision
`e4806bd9daa7d9542d219d05788cef60a0846f24`) was imported before modifications.
Small local changes were separately committed: share Graphviz 1.2.0 instead of
adding its pinned 1.1.0 copy; expose render warnings through an additive report
API; add checked block splitting without changing the original permissive API.
The shared-version check compiles, and Cargo.lock contains only Graphviz 1.2.0.
This also lets the adapter apply the existing no-file-loading Graphviz policy
before any layout. Original includes between C4 files were already rewritten to
bundled resources by the upstream crate; that provenance is recorded there.

- [x] Share `plantuml`, `puml` and `c4plantuml` fences, explicit closed-block
  clipboard hints, `.puml/.plantuml/.pu/.iuml` files, source/live preview/errors,
  strips and enlarged hover through the existing owners.
- [x] Support bare UML fence bodies through preview-only wrappers. Explicit
  unfinished blocks remain errors; source and save bytes are never rewritten.
- [x] Compose all complete diagram blocks (up to sixteen), refuse unfinished or
  superseded blocks, and retain warning banners and diagnostics.
- [x] Use only bundled stdlib/themes. External file/URL includes and missing
  bundled libraries produce errors, with no environment or filesystem resolver.
- [x] Keep C4's bounded embedded raster sprites in both outlined SVG and final
  PNG. The final raster stage previously used the default blocked-image policy;
  a comparison against deliberately blocked output now verifies real sprite
  pixels survive. Linked images remain excluded by the shared image resolver.
- [x] Retain 256 KiB source, 2 MiB SVG, structural/raster and disposable-process
  memory/CPU/wall-clock limits, including for programmable preprocessing.
- [x] Finish real-host, warnings-as-errors and native light/dark validation.
- [ ] Extend compatibility qualification beyond the representative family corpus;
  unsupported upstream diagram kinds/syntax remain visible errors, not fabricated
  fallback diagrams. Bundled stdlib coverage currently supplies C4, not every
  third-party PlantUML macro package; external includes are deliberately refused.

Editor research: [Omaspec](https://github.com/enhapu-afk/omaspec) is a native MIT
application with linked source/diagram editing, layout hints stored as comments,
model simulation and diagram comparisons. It uses iced/scrive, so its widgets
cannot be dropped into this egui host. Those source-preserving interactions are
useful design references, especially for future Mermaid editing; no editor code
was imported. plantuml-rs also has a VS Code source/preview extension. The
initial standalone baseline here remains the shared highlighted source, live
native preview and errors, preserving existing history and guarded saves.

Authored review files `sequence.puml`, `architecture.puml` and `plantuml.md` are
in `type_testing/md_extensions/` and `docs/examples/diagrams/`.

PlantUML validation: 56 shared core diagram tests, 15 syntax tests, 71 render
diagram tests, ten card previews, three strip tests, native-extension routing,
and both real-host worker suites pass. The file-manager worker suite additionally
checks programmable preprocessor rejection: the empty infinite `!while` is
reported by the native engine as `Infinite loop? (line 2)` before the outer
resource limits are needed. No artifact is published. Strict all-targets Clippy
across core/render/studios/both hosts and final builds pass without warnings.
Both native themes pass sequence and C4 strips, hover, source/errors and Markdown
checks while preserving fixture hashes. The C4 acceptance predicate uses its
larger person-background fill; the smaller system node has fewer than 500 solid
color pixels in a 66-pixel strip, despite correct rendering.

The architecture comparison includes the original port: approximately 97,000
measured LOC and its pre-existing clone groups, increasing global duplication
by 0.79 percentage points while leaving maximum CCN 274 unchanged. Imported
parser/layout families explain the new upstream hotspots. The local adapter
reuses the process, resolver, font, SVG and cache mechanisms; the two required
dispatch branches add format selection and embedded-raster policy. No additional
application cache, save owner, browser/runtime process or second Graphviz copy
is introduced. Existing lookup-table and touched-file clones remain; unchanged
acceptance helpers' graph centrality shifts are not source-code regressions.

## Draw.io native renderer selection and integration

Compared the native [`document-svg` 2.0.2](https://github.com/ryusui-hiro/document-svg)
reader/renderer with [`diagram-ir` 0.1.0](https://github.com/DreamLab-AI/diagram-ir),
[`drawio-exporter` 1.6.0](https://github.com/rlespinasse/drawio-exporter),
`drawio-lc` 0.4.0 and
[`flowchart-mcp` 0.1.0](https://github.com/zavora-ai/flowchart-mcp).
Diagram-ir extracts nodes/edges but does not render; the exporter tools invoke
the draw.io desktop application. Flowchart authoring is not a general mxGraph
reader. Document-svg supplies the most useful native geometry implementation
among these candidates: compressed containers, groups, ports, connector routes,
HTML-like labels, inline stencils, gradients and a substantial shape vocabulary.

Its full package also includes unrelated Office/PDF/CAD/font/CLI dependencies.
A focused MIT/Apache-2.0 library retains the draw.io sources, page IR, SVG writer,
XML helpers and embedded-SVG validator instead. The pristine selected source is
committed separately from adaptations. The local library accepts memory only;
external stencil file discovery and the unrelated converters are removed.
The host's existing disposable worker, guarded reader, cancellation, SVG/font
composition, image policy and caches retain their responsibilities. The public
page IR retains source IDs and semantic metadata for future direct editing.

The diagrams.net graphical editor itself is browser-based. Its interaction
patterns remain useful reference material, but embedding its application would
violate the no-browser requirement. This integration provides highlighted XML,
live native preview and errors in the existing shared editor. Direct editing is
still future work; previewing does not rewrite the compressed source.

- [x] Import pristine upstream source separately from local modifications.
- [x] Adapt memory-only rendering with bounded source/inflate/XML/page/SVG work.
- [x] Route `drawio` / `diagramsnet` fences, explicit `.drawio`, compound
  `.drawio.xml` / `.dio.xml`, and distinctive generic XML/clipboard roots.
  `.dio` requires a matching mxfile/mxGraphModel root so unrelated digital-I/O
  data stays ordinary text. Generic XML requires the actual root, not a nested
  or quoted example, and does not inherit another namespace.
- [x] Connect shared standalone source/live preview, Markdown, list strips and
  enlarged hover through the existing mechanisms.
- [x] Complete actual desktop-worker and native light/dark acceptance checks.

Unsupported named libraries/shapes and external images produce compatibility
warnings. Inline shapes are retained; no stencil collection is downloaded.
Embedded raster images use the existing bounded PNG/JPEG/WebP policy; unsupported
embedded image types remain diagnosed. Compound SVG/PNG files currently remain
ordinary images, rather than silently claiming editable diagram content.
Extended shape-fidelity qualification remains separate from this initial
integration, particularly proprietary/cloud shape libraries and rich HTML text.

Architecture comparison against `before-drawio` reports approximately 12,791
additional lines, dominated by the upstream renderer, with unchanged maximum
complexity and slightly lower total duplication (7.4% to 7.3%). Its upstream
shape dispatch and SVG writer account for the new hotspots/clone groups; keeping
the maintained geometry avoids recreating that substantial implementation.
The host adds one renderer dispatch arm; XML signature checking now shares the
existing root/namespace mechanism instead of introducing a second parser loop.
The isolated crate's bounded writer cannot depend on desktop IPC helpers; it
protects its public API independently. Existing viewer declaration clones and
reordered members of unchanged Python cycles are not newly duplicated logic.

Validation: four focused library tests cover plain/raw/URI-compressed models,
multiple pages, malformed containers, source/inflate/event/page/output limits
and external image warnings. The 57 core diagram tests, 17 selected syntax
tests, 74 selected render/diagram tests, standalone viewer routing, and both
desktop-worker protocol suites pass. Explicit little-screen routing includes
clipboard, `.drawio`, content-checked `.dio` and generic XML sources. The shared
source-editor highlighting test now covers both XML and YAML without duplicating
its assertion mechanism. Strict all-target Clippy and desktop builds pass.

Native light/dark acceptance passes rendered strips, enlarged hover, invalid
source diagnostics, source/live preview, embedded Markdown, source affordances
and unchanged source hashes. Screenshot inspection caught an initially missing
editor grammar-name mapping; the final XML colors are verified by both the
shared source-editor test and screenshots. Examples `pipeline.drawio` and
`drawio.md` are in `docs/examples/diagrams/` and the user's review folder.

## Structurizr library qualification and native preparation

Rechecked the earlier registry candidates: `oak-structurizr` is a syntax parser,
`dendryform-export` is a Structurizr DSL exporter. Cadastre remains a secondary
registry candidate; it was not selected over the two compiled native libraries. Also investigated the native tree-sitter/LSP project
[`tree-sitter-structurizr`](https://github.com/sinon/tree-sitter-structurizr),
which supplies editor-oriented syntax rather than computed views/rendering.
A search result for `junix/diagram-ast-parser` was unavailable at its repository
URL, so its claims were not used as implementation evidence.

Two source-level native candidates were actually compiled and probed:

- [`StructurizrX`](https://github.com/pomali/structurizrx), Apache-2.0,
  revision `6dae3d8b0870cc7f4ee6faffd8eb18d7b122e3e4`: useful identifier/source
  locations, clear documentation and attractive static SVGs. Four common static
  views render. However, the probe parsed an additional dynamic view and silently
  left it out of the four-page output. An unknown view keyword was also accepted,
  and the tested left-to-right container view still laid out vertically.
- [`structurizr-rs`](https://github.com/Helms-AI/structurizr-rs), MIT as declared
  in its workspace manifest, revision
  `59a5c4a93bfcb11e501d81a06cd4492c01f1f890`: native models, DSL, styles,
  layout, navigation/edit-history types and SVG drawing. The same four static
  views plus the dynamic view produce five SVGs, unknown identifiers/keywords
  fail, and requested layout direction is honored. This broader renderer is the
  selected foundation. The pristine selected source is committed before patches;
  preserved metadata records the upstream license declaration and authors.

The official [Structurizr feature documentation](https://docs.structurizr.com/features)
explains that its graphical editor adjusts manual layout; the architecture model
is authored as code. Both inspected Rust frontends are web viewers, rather than
an egui editor that can be embedded here. The selected native model/layout APIs
are useful for later direct interaction, while the existing source/live-preview
studio remains the initial integration path. No browser/server/Java runtime is
needed or imported.

The selected parser accepted an unfinished outer workspace in the probe. Local
preparation now uses its existing lexer to check complete braces, excessive
nesting and trailing content before recursive descent. Strings and comments do
not participate in brace depth. Memory inputs have 256 KiB / 16,384-token /
64-level limits. External workspace extension/includes and scripts/plugins fail
instead of being silently omitted. Documentation paths remain inert metadata.
All filesystem access, include resolution, theme downloads/caches and execution
features are removed from the selected libraries, together with HTTP and unused
Pest dependencies. Native JSON serialization remains; compatibility with the
canonical Structurizr JSON schema has not been established and is not claimed.

Validation of this preparation: 16 core-model tests, 28 DSL unit tests,
six directive tests, three new preview-boundary tests, 88 native renderer tests
and one core rustdoc test pass. Coverage includes static/dynamic/deployment,
filtered/custom/image renderers, lexer-aware brace handling, malformed/trailing
input and source/token/depth limits. The three libraries also check against the
application workspace without compiler warnings. Only one small SHA-1 dependency
is added for the upstream stable UUID implementation; HTTP dependencies are not
retained. Both candidate probes share the existing target directory rather than
creating another build cache.

- [x] Research and compare syntax-only/export-only and native render candidates.
- [x] Compile both native candidates and compare actual view/error behavior.
- [x] Import pristine selected libraries separately from local adaptation.
- [x] Adapt and test a bounded, memory-only parser/rendering foundation.
- [ ] Integrate native Markdown/little-screen/file/clipboard routes and source UI.
- [ ] Validate rendered strips, hover, editor, errors and Markdown in both themes.

Architecture review of this import/adaptation reports roughly 15,942 added
lines, all in the selected upstream libraries and their preview-boundary tests.
Maximum complexity is unchanged; total duplication changes by 0.02 percentage
points. The 138 additional clone groups and parser/renderer hotspots are in the
upstream implementation, not a second application preview/cache/layout system.
The reported reordered Python cycle is unchanged.

Further qualification found correctness work that must precede host integration:

- [x] Replace transitive graph closure with actual containment-based implied
  relationships, enabled by default and respecting explicit endpoint pairs.
  The [official rule](https://docs.structurizr.com/dsl/implied-relationships)
  lifts endpoints through their containing architecture elements; unrelated
  A→B→C edges must not invent an A→C relationship.
- [x] Retain explicitly selected disconnected elements and preserve empty
  selections; do not confuse wildcard scope with global graph connectivity.
  Check context/container/component scope against the
  [official include rules](https://docs.structurizr.com/dsl/language#include).
- [x] Qualify static component/filtered selections and diagnose unsupported
  selector expressions instead of silently omitting them.
- [x] Qualify dynamic interactions, model relationship binding, replies and
  parallel/explicit ordering; preserve annotations and diagnose invalid scopes.
- [x] Preserve custom views and canonical custom elements through model lookup,
  ordered selection and the shared static renderer.
- [ ] Correct native static relationship routing: left-to-right orthogonal edges
  currently use top/bottom attachment points, which can hide arrowheads under nodes.
- [ ] Qualify deployment view behavior and additional DSL forms.

The format checkbox remains open while these and host integration are pending.

### Structurizr implied-relationship model correction

The parser now derives parent endpoint combinations, with implied relationships
enabled by default. It never infers direct links from graph reachability.
Explicit endpoint pairs are retained, inferred pairs are deduplicated, and
derived links retain their original description/technology/tags. `false` retains
only authored model edges. Unsupported Java strategy names report an error.
A shared core containment query owns the ancestor traversal for later view
scoping work; no second DSL parser was introduced.

Three focused regressions cover default/true/false, component/container/system
ancestor combinations, no transitive dependency shortcuts, explicit pair
precedence and unsupported strategy diagnostics. All model/DSL/renderer tests
still pass (144 unit/integration tests plus the core rustdoc test). Earlier upstream expectations
of transitive closure were corrected rather than preserved as the specification.
Architecture review finds no excessive complexity in the added containment or
derivation logic; the import's maximum complexity and clone counts are unchanged.

Renderer-side selection and edge projection are still pending: they must consume
the resolved model faithfully, including when implied relationships are disabled.
This is not yet exposed as a supported file or Markdown fence.

### Structurizr resolved view selections

Static views now resolve selected element and relationship IDs in the parser.
The four C4 renderers share one existing native layout/paint pipeline and honor
those IDs directly, including an empty selection. The renderer no longer drops
disconnected explicit members, re-infers model edges, or substitutes the whole
model when exclusions remove every member. The scoped system remains present in
a context view, matching the official protected-scope behavior.

Wildcard membership uses the view's documented seed elements and direct
neighbours, rather than unrelated connected pairs anywhere in the graph. Local
containers/components survive even if isolated; component views can render their
selected people, external systems and sibling containers. Invalid scopes,
unknown/unsupported selectors and ancestor/descendant mixtures produce errors.
The AST retains include/exclude statements and their term order, supporting
multiple terms, re-inclusion and wildcard exclusion without parallel selection
lists. Common title/description/properties/layout/default metadata is retained.

Filtered views refine their base view's selection, so tags cannot admit unrelated
model members or edges. They reuse the same pipeline for all four static base
view types, including components; relationship tags can hide edges while keeping
their nodes. Model containment/metadata enumeration remains owned by core.

Eight new end-to-end parser/SVG regressions check actual painted object IDs, not
only successful SVG creation: scoped wildcards, disconnected members, duplicates,
ordered operations, quoted identifiers, empty views, protected context scope,
disabled implied links, filtered membership/edge tags and metadata propagation.
All 152 unit/integration tests plus one core rustdoc test pass. The application
workspace checks the three libraries without warnings, and strict all-target
Clippy for the shared renderer passes. Additional grammar/view compatibility
remains pending, including bare metadata property keys and extended views.

Architecture comparison against `before-structurizr-selection` removes 37 clone
groups (3177 to 3140), lowers duplication by 0.07 percentage points and removes
122 measured lines despite the new regression suite. The three previously
complex static-render entrypoints fall below the hotspot threshold; the new
selection functions stay below it. Shared property conversion also removes
repeated construction branches from the workspace builder. Raw classification
still reports changed-file clone warnings: the six core declaration groups are
byte-identical to the baseline, and remaining parser/renderer field/keyword
patterns were inspected. They do not justify replacing typed view adapters with
a larger generic framework. Unchanged Python cycles are not new dependencies.

This is a backend correction, committed separately from the implied-relationship
fix. Host routing, graphical UI and native acceptance remain pending; the
Structurizr format checkbox stays open.


### Structurizr dynamic interaction correction

Dynamic views now resolve each interaction against an existing model relationship,
with technology disambiguation and description preference. Unknown scopes,
endpoints, incompatible abstraction levels and missing relationships fail rather
than disappearing or becoming placeholder nodes. Replies bind to an earlier
request's model declaration, retaining its technology; self-calls draw a loop.
Model relationship styles apply to the actual selected declaration. The renderer
also rejects inconsistent runtime bindings instead of painting misleading links.

Authored dotted orders are retained. Parallel groups follow the documented
sequence behavior (the standard nested example is `1, 2, 2, 3`), without invented
hierarchical numbers. Overflowing numeric tokens report a lexer error. Static and
dynamic views share annotation parsing/conversion; dynamic include/exclude terms
fail because this view selects interactions. Forward-declaration selection uses
a bounded iterator rather than allocating a temporary candidate vector.

Five end-to-end regressions cover parallel and explicit orders, scoped endpoints,
technology selection, replies, self-calls, annotations, malformed scopes/relations,
integer overflow and corrupted runtime bindings. All 157 unit/integration tests
and one core rustdoc test pass. Strict all-target Clippy for the application's
shared renderer passes. A rendered native SVG was rasterized and inspected to
confirm interaction order, dashed reply and visible self-call geometry.

Architecture comparison against `before-structurizr-dynamic` reduces clone groups
by three and duplication by 0.01 percentage points. Maximum complexity and cycle
count are unchanged; common annotation parsing removes the dynamic-body hotspot.
The initially flagged relationship-selection complexity was simplified into its
independent forward-selection rule; no new complexity hotspot remains. Raw
classification still flags clones in touched upstream parser/lexer/renderer
files. Both lexer fragments and all 13 renderer fragments exist byte-for-byte in
the baseline; parser declaration patterns remain, while repeated annotation
construction was removed. No additional host worker, cache or layout framework
was introduced.

This backend fix is separate from static selection corrections. Deployment and
custom views, remaining DSL compatibility, host routing and native UI acceptance
are still pending; the full format checkbox remains open.


### Structurizr custom view preservation

The canonical `element` model declaration now works alongside the imported
`customElement` alias. Optional metadata, descriptions, inline/body tags and
properties reach the common model; custom identities participate in core lookup.
Custom views are no longer discarded during workspace construction. They reuse
the existing ordered selection and static layout/drawing pipeline, including
empty/disconnected selections, resolved relationship selection, annotations,
styles and auto layout. Only custom elements are permitted; C4/custom abstraction
mixes report errors. The obsolete second element-details traversal and separate
custom layout implementation were removed.

Four end-to-end parser/SVG regressions check canonical and alias syntax, optional
metadata, actual painted IDs/relationship paths, style/description propagation,
exclusion/re-inclusion, empty views, invalid members and corrupt runtime views.
The existing upstream test was corrected to use actual custom elements rather
than people/software systems. All 161 unit/integration tests and one core rustdoc
test pass. Strict all-target Clippy for the shared application renderer passes.
Native SVG inspection confirms disconnected membership, descriptions and tags;
it also exposed an existing shared orthogonal-routing defect, tracked above for
a separate correction. This is backend preparation, not a host support claim.

Architecture comparison against `before-structurizr-custom` removes two clone
groups and lowers duplication by 0.01 percentage points. Maximum complexity and
cycle count are unchanged. The existing workspace builder gains the required
custom-view loop/fallible selection (+2 CCN); the dedicated custom layout and
duplicate model traversal are removed rather than adding another framework.
Raw clone warnings concern touched files: all six core, 28 parser and 13 renderer
baseline fragments remain byte-identical. The new tests account for the small
net measured line increase. Remaining grammar, deployment, routing and host UI
acceptance stay open.
