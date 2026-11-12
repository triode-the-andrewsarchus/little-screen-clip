# Native diagram previews and editing

Branch: `feat/native-diagram-previews`; baseline: `native-diagram-previews` at `c9cbf22`.

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
| Mermaid | [mermaid](https://crates.io/crates/mermaid) 0.2.0; [flowmaid](https://crates.io/crates/flowmaid) 0.40.0; [mermaid-fixer](https://crates.io/crates/mermaid-fixer) 1.0.2 | Pending comparison / editor research |
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
| Lini | [lini](https://crates.io/crates/lini) 1.1.0; [mdbook-lini](https://crates.io/crates/mdbook-lini) 0.5.5; [hrdf-parser](https://crates.io/crates/hrdf-parser) 0.10.0 | Pending comparison / editor research |
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
