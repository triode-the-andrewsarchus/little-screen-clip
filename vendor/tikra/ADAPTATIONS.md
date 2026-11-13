# Application preview adaptations

The pristine 1.9.2 crate was committed separately before these changes. Source
identity, archive/file hashes and upstream licenses remain in `PROVENANCE.md`.
Only the following boundaries differ from upstream:

- `engine/pgfplots/table.rs::read_bytes` and `find_file` always reject external
  resources before metadata, file reads, search-path or environment discovery.
  These are the engine's sole filesystem entry points. Inline table parsing is
  unchanged. `graphics.rs::find_image` rejects before extension probing and
  preserves the same explicit refusal for names without extensions.
- `RESOURCE_ACCESS_DENIED` names that diagnostic at the library root. The host
  adapter promotes upstream recovery warnings carrying it to a preview error,
  so unavailable data/images cannot become a plausible partial chart.
- TeX `write` returns that error instead of silently consuming its argument.
  The native engine never implemented file/shell execution. Undefined resource
  commands (`input`, `include`, `openin`, `openout`, `read`, `readline`, `directlua`,
  `special`, `immediate`, `includegraphics`, `pgfimage`) now give an explicit refusal even in preambles. Other unknown
  preamble commands produce warnings instead of being silently dropped. These
  diagnostics supplement the disabled real resolver; they are not a text-based
  security boundary or an implementation of full TeX primitives.
- A default-enabled `raster` feature gates the previously unconditional resvg
  dependency, PNG API and pen-plotter module. `motion` includes it. Default users
  keep those existing APIs; the application's `default-features = false` SVG
  dependency uses the existing shared composer/rasterizer exclusively.
- Mathra is pinned to the application's existing exact 1.12.0 version, with its
  optional chemistry defaults off and Latin Modern enabled for TikZ text/math.
  Font data remains Mathra-owned. The distinct Latin Modern GUST Font License is
  retained verbatim in `third_party/` alongside the upstream font README; this
  font is not labeled OFL.

No command runner, network client, source include resolver, file cache, external
TeX fallback, custom diagram grammar or parallel raster path was added. Upstream
geometry, PGF/TikZ parsing, bounded TeX expansion and SVG glyph outlining remain
intact. The application's existing disposable diagram worker still supplies
process memory/CPU/output/cancellation limits.
