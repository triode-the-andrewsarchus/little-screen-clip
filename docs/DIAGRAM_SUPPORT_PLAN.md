# Native diagram previews and editing

Branch: `feat/native-diagram-previews`; baseline: `native-diagram-previews` at `c9cbf22`.

## Required outcome

Every item under Markdown in `file_types.md`, from front matter through Symbolator, must render in the large text viewer and the little-screen preview. Formats with standalone extensions also need guarded opening, highlighted editable source, live preview and visible diagnostics. Investigate existing graphical editors and use direct manipulation with source correspondence where practical. No browser engine or silent network rendering. Make reviewable commits as each part lands.

Preserve FileSource identity checks, atomic save/Save As semantics, accepted write completion, preview cancellation and per-host lifecycle. Rendering must not resolve arbitrary network URLs, include files, execute diagram directives or read unguarded resources. Enforce input, complexity, output and raster budgets. Unknown/unsupported syntax must remain inspectable with a diagnostic, never silently treated as fully supported.

## Implementation sequence

- [x] Shared metadata framing/parsing and both Markdown presentations.
- [ ] Shared bounded diagram source/render contract, source spans and diagnostics; reuse existing raster and worker ownership.
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
| Pikchr | [pikchr](https://crates.io/crates/pikchr) 0.1.4; [pikchr-rs](https://crates.io/crates/pikchr-rs) 0.1.0; [pikchr-sys](https://crates.io/crates/pikchr-sys) 0.1.1 | Pending comparison / editor research |
| Svgbob | [svgbob](https://crates.io/crates/svgbob) 0.7.6; [mdbook-svgbob](https://crates.io/crates/mdbook-svgbob) 0.3.1; [mdbook-svgbob2](https://crates.io/crates/mdbook-svgbob2) 0.3.0 | Pending comparison / editor research |
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

Compared [gray_matter](https://docs.rs/gray_matter/latest/gray_matter/) (multiple engines, owned custom Pod), [frontmatter-gen](https://github.com/sebastienrousseau/frontmatter-gen) (broader generation/SSG facilities), and [frontmatter-lossless](https://github.com/osdlc/osdlc) (YAML framing only). The shared core reader needs original byte offsets, TOML/JSON delimiters, bare JSON objects, bounded diagnostics and bounded YAML expansion. Its small framing layer reuses existing Serde JSON/TOML; [serde-saphyr](https://docs.rs/serde-saphyr/2.1.1/serde_saphyr/) supplies YAML with explicit depth/event/node/alias/scalar budgets and includes disabled. Deserialize-only features avoid serializer/comment facilities. License: MIT OR Apache-2.0; five newly locked packages, no executable dependency.

The large viewer exposes expandable metadata fields; screen cards show format and a compact field summary. Source remains unchanged. Framing failures leave the body visible and closed malformed metadata reports its error. Parsing is capped at 64 KiB; presentation bounds field count and text length.

Validation: five core metadata tests; six studio Markdown/source tests and seven render Markdown tests pass. Targeted strict Clippy for core/render/studios passes. Tests cover CRLF/BOM/Unicode byte offsets, nested JSON strings, malformed/unclosed/oversized framing, partial delimiters, YAML depth/aliases/includes and screen-prefix parsing after metadata longer than 4 KiB. Full visual review and workspace validation remain in the final checklist.

## Build compatibility

The added native parser dependency graph exposes an ambiguous `ImageBuffer::as_mut()` inference in the existing SAM mask blender. The buffer is explicitly typed as `&mut [u8]`; pixel behavior is unchanged. All three actual-host diagram dispatch tests compile and pass with that annotation.
