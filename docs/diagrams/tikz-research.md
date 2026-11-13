# TikZ, PGF and pgfplots renderer review

Reviewed 2026-11-13. The selected renderer is the native Rust
[Tikra 1.9.2](https://docs.rs/tikra/1.9.2/tikra/) interpreter and SVG driver. It
accepts actual TikZ/PGF source, including a `tikzpicture`, a `pgfpicture`, inline
`\tikz`, an `axis`, loose drawing statements or a supported standalone document.
The adapter does not introduce its own diagram grammar or translate a few chart
commands into a language called TikZ.

## Candidate comparison

| Candidate | Actual implementation and compatibility | Decision |
| --- | --- | --- |
| [Tikra](https://github.com/matheusmota-dev-br/tikra) | Native Rust TeX token/macro execution, TikZ keys/paths/nodes, PGF system primitives, pgfplots transforms/layout and outlined SVG. Math/text use the application's existing exact Mathra 1.12.0. | Selected, with its real file resolver disabled and its unused raster backend made optional. No TeX installation, browser, JS runtime, command runner or new raster pipeline is needed. |
| [texrs](https://docs.rs/texrs/latest/texrs/tikz/) | Genuine native Rust TeX/TikZ implementation, drawing PDF operators. Its documented TikZ coverage includes paths, four node shapes, transforms, arrows, decorations and loops; it explicitly excludes patterns, matrices, graphs, pics and other node shapes. | A real native alternative, not a wrapper. Narrower documented TikZ coverage and PDF-oriented output make it a worse fit than Tikra's PGF/pgfplots/SVG path here. |
| [Tectonic](https://github.com/tectonic-typesetting/tectonic) | Rust API around a full XeTeX-derived engine and TeX Live resources. Can execute canonical PGF macros with the supplied packages. | Strong full-TeX compatibility alternative, but requires a much larger engine/resource/IO boundary. `--untrusted` controls unsafe capabilities such as shell escape; it is not a complete filesystem sandbox. No runtime fallback is added. |
| Installed pdfTeX + [canonical PGF](https://github.com/pgf-tikz/pgf) + dvisvgm/PDF-to-SVG | Actual TeX macro implementation and backend drivers; installed locally as TeX Live 2026. | Reference candidate for fixture comparison. Running arbitrary TeX would need an isolated filesystem/network boundary in addition to disabled shell escape and process limits. Native rendering makes that helper unnecessary for the selected supported features. |
| [mdbook-tikz 0.2.8](https://github.com/tensorigami/mdbook-tikz) | Rust mdBook preprocessor wrapping Tectonic/pdfLaTeX and pdf2svg. Published source writes temporary TeX, invokes commands and caches SVG; it is not a native TikZ parser/renderer. | Useful mdBook integration reference, not the rendering engine or resource boundary to embed. |
| tikra-cli | Command-line front end to the same native Tikra library. | Use the library in the existing owned diagram worker instead of creating another process type. |

The canonical PGF project describes PGF as a TeX macro graphics package and TikZ
as its syntax layer. Native Tikra is a compatibility implementation, not a claim
that the original TeX macros are being executed. Upstream publishes manual-figure
conformance comparisons; those are upstream results, not a substitute for this
application's representative tests or a claim of perfect TeX compatibility.

A relevant [primary security advisory](https://github.com/docling-project/docling/security/advisories/GHSA-x3q2-h9hx-4r4j)
demonstrates why shell-escape restrictions alone do not protect TeX file access:
file primitives still operate, and textual prechecks can be bypassed by macros.
The selected native boundary removes actual resolver behavior rather than
trying to recognize every possible dangerous source spelling.

## Resource and execution boundary

Source audit found one shared filesystem implementation in upstream:
`engine/pgfplots/table.rs::{find_file,read_bytes}`, used by external tables and
images. Upstream searches `TIKRA_INPUT_DIR` or the current directory, including
metadata probes. The vendored preview version replaces both entry points with
unconditional refusal before discovery or access. Image extension probing is
also refused, preserving the same diagnostic for names without extensions.
After adaptation, `rg` finds no filesystem, environment-discovery, process or
network calls in Tikra's source. Inline tables, literal coordinates, functions,
macros and built-in native libraries still execute normally.

The adapter promotes resource-refusal warnings from upstream recovery to an
error. A requested external data/image file therefore cannot disappear into a
plausible incomplete plot. Undefined `input`/`include`/file primitives and native
`write` now provide explicit refusals, including in preambles; these diagnostic
checks supplement the removed resolver. Other unknown preamble commands warn
instead of disappearing silently. All ordinary upstream unsupported/recovered
statement warnings are retained for the existing preview UI.

Native bounds remain in place: 100,000 statements, two million expansions per
statement and twenty million per figure, one million tokens, scope/loop depth
64, key execution limit 50,000 and bounded range/plot/grid generation. These
complement the existing disposable worker's source, memory, CPU, output,
cancellation and SVG limits. Conversion never runs on the GUI thread. Tests of
untrusted source use the native library; TeX reference compilation, if used, is
restricted to the small authored fixtures and is not an application feature.

## Rendering and font ownership

Tikra generates SVG with outlined text/math glyphs. The existing `compose_svg`
function remains the single owner of SVG normalization, IDs, viewports, fonts
and blocked images; the existing rasterizer produces previews. Viewport
preservation retains the author's standalone border. The optional PNG,
pen-plotter and motion APIs keep upstream-compatible default availability, but
the application's dependency disables those defaults. No resvg 0.45 pipeline is
activated for the application.

TikZ labels enable Mathra's optional Latin Modern Math font while keeping
chemistry defaults disabled and preserving the existing exact Mathra version.
Latin Modern Math uses the **GUST Font License**, distinct from the STIX subset's
OFL. The upstream font license and font README are retained verbatim under
`vendor/tikra/third_party/`; bytes remain owned by the Mathra dependency.

## Native editor comparison

[TikZiT](https://tikzit.github.io/) is an actual native Qt graphical editor for
graphs and string diagrams. Its own documentation explicitly describes its
native format as a subset of PGF/TikZ. It offers node dragging, labels, edge
bends and source jumps; it cannot be treated as a universal round-trip editor
for arbitrary macro-generated diagrams. [TikzEdt](https://github.com/mkantem/tikzedt)
combines WYSIWYG and text editing in a separate native application.
[Q/KtikZ](https://github.com/Piraty/qtikz) supplies native source editing with an
automatic TeX-backed preview. These are interaction references, not drop-in egui
components. No editor code was imported.

The existing highlighted source/live-preview/error studio is the immediate
native workflow. Tikra already exposes named points, origins and source spans
through `Options::source`/`Output`, useful for later source-linked selection and
literal-coordinate handles. Macro/foreach generated geometry maps back to its
invocation, so moving arbitrary rendered nodes cannot be advertised as a safe
reversible source edit. A general graphical editor remains explicit future work.

## Provenance and review fixtures

The pristine 1.9.2 crate and upstream MIT/Apache licenses were committed before
local changes. `vendor/tikra/PROVENANCE.md` records the verified crates.io archive
SHA-256, upstream commit and all imported file hashes. Local modifications are
listed in `vendor/tikra/ADAPTATIONS.md`; geometry and the native grammar remain
upstream-owned.

- `native-tikz.tikz`: macros, scoped transforms, foreach, authored colors, nodes,
  named anchors, arrows, a Bezier path and mathematical labels.
- `native-pgf.pgf`: genuine lower-level PGF path, paint, point and text primitives.
- `native-pgfplots.pgfplots`: function sampling, inline table observations,
  marks, ticks, grids, axis labels and legends.

All five owned unit tests passed with
`cargo test -p little-clip-render -j4 --lib diagram::tikz::tests` in the serialized
build window (0.16 seconds runtime). Coverage includes actual node-contained
images, macro-computed external table paths, unsupported diagnostics, bounded
nonterminating macro expansion, isolated definitions and the shared native
composer/rasterizer.

All three authored fixtures also compiled with installed pdfLaTeX using
`-no-shell-escape -interaction=nonstopmode -halt-on-error`. Reference PDF-to-SVG
used installed `pdftocairo`; installed dvisvgm could not use the system's newer
Ghostscript, which is a reference-tool limitation and does not affect native
rendering. Reference PDFs and native SVGs were rasterized at matched 120 dpi and
visually compared. TikZ and lower-level PGF show essentially matching geometry,
text, math, colors and paths; pgfplots shows matching data, curves, grids, ticks
and labels with visible axis-padding/legend-layout differences. This is
representative evidence, not full manual conformance. Review artifacts remain in
`/tmp/tikz-reference-review/`.

Registry/editor integration and native light/dark host acceptance remain with
the primary integration pass; this adapter-level result does not claim them.
