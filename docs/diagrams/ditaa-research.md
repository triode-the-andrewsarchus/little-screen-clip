# Ditaa implementation and editor research

Research date: 2026-11-13.

## Renderer comparison

| Candidate | Actual mechanism | Decision |
| --- | --- | --- |
| [Canonical Ditaa 0.11.0](https://github.com/stathissideris/ditaa/tree/v0.11.0) | Java ASCII-grid parser, closed/open boundary analysis, shape tags, color codes, shared-edge separation, point markers, arrows, dashed lines and SVG output. Source headers and project metadata specify LGPL-3.0-or-later. | Selected as a separate local companion. This implements the language rather than a related ASCII dialect. |
| Existing vendored [plantuml-little](https://crates.io/crates/plantuml-little) / `plantuml-core` | Native Rust PlantUML renderer. The vendored tree names `DiagramType::Ditaa`, recognizes the delimiter and embedded-language name, but has no Ditaa factory or rendering module. `render_mode` rejects diagram types without a registered factory. | Cannot supply Ditaa rendering. A type-name enum is not implementation evidence. |
| [mdbook-plantuml](https://github.com/sytsereitsma/mdbook-plantuml) | Rust Markdown preprocessor calling a PlantUML executable/JAR or server. Its documentation explicitly falls back to PNG for PlantUML Ditaa. | A wrapper rather than a native Ditaa parser. The direct canonical API is smaller and avoids PlantUML preprocessing and external source resolution. |
| [ansicht](https://github.com/kober-systems/literate_programming_toolsuite/tree/master/ansicht) | Rust ASCII-art/PlantUML sequence-diagram readers with an internal AST and ASCII/Mermaid/JSON writers. | Different language and output contract; no Ditaa tag/color/boundary renderer. |
| Existing [SvgBob](https://github.com/ivanceras/svgbob) / [GoAT](https://github.com/blampe/goat) | Native ASCII/Unicode diagram engines with different grammars and rendering conventions. | Keep their own explicit formats. They do not implement Ditaa's shape tags, colors and common-edge semantics. |
| Java AOT / a new Rust port | Would require reproducing Java/AWT geometry and font behavior, or proving an AOT toolchain and its dependencies. | No established compatible native library was found in this comparison. An untested port would weaken compatibility and greatly expand the mechanism. |

[PlantUML's Ditaa documentation](https://plantuml.com/ditaa) documents
`@startditaa`/`@endditaa`, common-edge/shadow controls and scale. Its Java
integration only provides PNG; canonical Ditaa itself provides SVG. We call that
SVG API directly and retain the application's existing SVG validation,
font outlining, image blocking, layout and raster budgets.

The canonical source, official release JAR, exact hashes, licenses and notices
are recorded in [the import provenance](../../vendor/ditaa/UPSTREAM.md).
The engine is a separate process and is not linked into the Rust application.

## Source contract and limits

The preview accepts a bare Ditaa canvas or one complete
`@startditaa`/`@endditaa` block. Delimiters never become Java command arguments.
Documented PlantUML Ditaa options are interpreted as bounded data by the bridge;
unsupported options report an error. Arbitrary command-line, HTML conversion,
custom-shape/configuration files, font URLs and output paths are unavailable.

Source is limited to 256 KiB and a 256 × 128 expanded UTF-16 cell plane. Tabs use
canonical eight-column tab stops. The canonical parser operates on Java `char`
cells, so supplementary Unicode characters occupy two cells. Labels are escaped
as XML text, including `]]>`, rather than inserted into an unsafe CDATA section.
Bundled Cousine supplies Java text metrics and the shared SVG composer supplies
the final outlined labels. Missing glyph fallback remains governed by the shared
font pipeline.

The runtime is local Java 11 or newer with headless AWT; no browser engine or
remote service is used. A missing Java runtime or companion reports an actionable
error. JVM heap, metaspace, code cache and processor count are fixed; the existing
`CommandRequest` owns the process group, parent-death behavior, wall timeout, CPU
and address-space limits and bounded output. Source is only stdin data. Ditaa's
loose canvas grammar intentionally accepts ordinary text and incomplete visual
shapes; these are not invented syntax failures.

## Graphical editor comparison

- [ASCIIFlow](https://github.com/lewish/asciiflow) is an ASCII canvas editor with
  text, box/line tools and selection. Its browser application is an interaction
  reference, not a rendering/runtime dependency.
- [Asciio](https://github.com/nkh/P5-App-Asciio) provides GTK3 and terminal editors,
  an explicit grid, undo/redo, grouping, multiple selection and connectors that
  follow moved boxes. Ditaa upstream recommends using `+` for its corner
  characters. It is a useful reference for a native cell editor and separately
  editable shape records; importing an arbitrary ASCII canvas into shape records
  cannot guarantee a lossless round trip.
- [Eclipse Asciidoctor editor](https://github.com/de-jcup/eclipse-asciidoctor-editor)
  supplies dedicated `.ditaa` source editing and live preview. This matches the
  existing source/preview/error workflow, while leaving the source authoritative.
- [Emacs artist-mode](https://www.gnu.org/software/emacs/manual/html_node/emacs/Artist-Mode.html)
  demonstrates character-grid drawing without a browser. Ditaa upstream warns
  that straight lines matter for compatibility.

The practical direct-manipulation direction is one shared native character-grid
editor for Ditaa/SvgBob/GoAT: rectangular selection, box/arrow tools, cell move,
resize and undo applied directly to source. Each renderer retains its own
grammar. Shape-level manipulation inferred from rendered SVG is not implemented
or claimed here.

## Compatibility evidence

The checked-in `pipeline.ditaa` exercises document tags, named fills, connectors
and a dashed boundary. `shapes.ditaa` covers all seven documented tags, storage
curves, colored shared edges, round corners, point markers and dashed arrows.
Unit tests render those fixtures through the real companion and shared composer,
check label injection escaping and require useful errors for incomplete wrappers,
unsupported options, controls and cell budgets. The companion's upstream fixture
probe checks the canonical checked-in text corpus without translating it into
another language.

`python3 vendor/ditaa/probe.py /tmp/clip-ditaa-final-probe.jar` passed 52 exact
canonical geometry comparisons (all 50 upstream text fixtures plus the two local
fixtures), 13 malformed/oversize input checks and three wrapper/XML/UTF-8 special
cases. The admitted dense 256 × 126 cell plane was retired by the native resource
limit after 3.34 seconds with no partial SVG. The source-embedded sidecar
was about 1.1 MiB on the checked JDK, compared with the official 12,621,318-byte
build-input JAR. These checks exercise Java's direct engine API; the centrally
serialized Rust suite additionally verifies shared SVG composition/rasterization.
