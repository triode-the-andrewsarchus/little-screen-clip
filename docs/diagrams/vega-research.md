# Vega and Vega-Lite renderer review

Reviewed 2026-11-13. The implementation uses canonical Vega 6.4.0 and Vega-Lite
6.5.0 in the application's existing bounded QuickJS interpreter, with no browser,
DOM, filesystem, network, canvas, Node installation or second script engine.
Vega-Lite compiles the actual JSON grammar to Vega; Vega executes that dataflow
and exports SVG through its public `View.toSVG()` API. The shared SVG composer
then outlines labels with bundled metric-compatible fonts and applies the common
image and dimension policies.

## Candidate comparison

| Candidate | What it actually implements | Decision |
| --- | --- | --- |
| [Vega](https://github.com/vega/vega) and [Vega-Lite](https://github.com/vega/vega-lite) | Canonical grammar compiler, expression parser, transforms, scales, axes, legends and scenegraph/SVG renderer. JavaScript libraries, usable headlessly. | Selected in the existing interpreter. This preserves authored semantics instead of translating a small chart subset. |
| [vl-convert-rs](https://github.com/vega/vl-convert) | A genuine Rust conversion library embedding the canonical JS compiler/renderer in V8; it also owns Canvas 2D/font integration. It is not a separate pure-Rust Vega parser. | Strong compatibility alternative, but duplicates the already-approved script runtime and brings a substantially larger runtime/dependency boundary. Keep as a future option if accurate canvas layout becomes necessary. |
| [VegaFusion](https://github.com/vega/vegafusion) | Native Rust analysis, query planning and supported transform execution, with Python/JS integration for scaling Vega. It does not replace Vega's entire mark/layout/SVG renderer. | Valuable for large datasets; not a standalone renderer for this task. Adding its compute stack would not remove the canonical renderer. |
| [Charton](https://github.com/wangjiawen2013/charton) | A genuine native Rust grammar-of-graphics library, with SVG, raster and GPU backends. Its documented input is its own Rust dataset/mark/encoding API. | Considered honestly as a native renderer. Altair-inspired API is not evidence of accepting Vega/Vega-Lite JSON. A translation layer would introduce a second partial grammar and lose compatibility. |
| [Plotters](https://github.com/plotters-rs/plotters) | Native Rust plotting and SVG/raster drawing using its own chart-building API. | Useful drawing primitives, not a Vega schema parser/compiler. Reimplementing the two grammars on it is not a practical compatibility improvement. |
| [vega_lite_4](https://docs.rs/vega_lite_4/latest/vega_lite_4/) | Generated schema structures and JSON builders for Vega-Lite v4. Its documentation calls it a thin wrapper. | Data-model/export support, not a native parser/layout/SVG renderer. It would also pin an older grammar. |
| [vega 0.0.1](https://docs.rs/vega/0.0.1/vega/) | The published crate contains generated schema structures; `lib.rs` exports `schema` and serde dependencies. | Schema bindings, not a renderer. The crate name is not compatibility evidence. |
| [vega-view](https://docs.rs/vega-view/latest/vega_view/) | Nushell chart viewer using Wry and a platform webview. | Conflicts with the no-browser requirement. |
| [encdr](https://docs.rs/encdr/latest/encdr/) | Hardware/device/screen interface library. | Name-search false positive; unrelated to Vega grammar rendering. |

These comparisons use primary project documentation and the published crate
source. No candidate was called a native schema renderer solely because its
package name or API resembles Vega. No global tools or packages were installed.

## Graphical editing comparison

The [canonical Vega Editor](https://github.com/vega/editor) is a browser-based
source/live-preview environment, with examples and debugging tools. It is the
compatibility reference for authored source, not an embeddable native widget.
[Lyra](https://github.com/vega/lyra) supports graphical mark placement, property
bindings and data pipelines; its own README distinguishes incomplete Lyra 2
work from its stable branches. [Voyager](https://github.com/vega/voyager) provides
field-to-channel specification and chart recommendations, with a browser
React/Redux interface. None is an existing egui/Rust editor that can be imported
without bringing a browser engine.

The practical native editor is the application's existing source/live-preview
and error workflow. A future optional inspector can edit a simple Vega-Lite
`mark`, width/height and encoding field/type properties while retaining the full
JSON document. It must not claim arbitrary layered/faceted/conditional charts
are reversible to a small visual editor. No pointer interaction, selection
binding or visual mark editor is claimed by this adapter.

## Boundaries and fidelity

- The custom loader is supplied in the `View` constructor, before initial data
  requests can begin, as required by the [canonical View API](https://vega.github.io/vega/docs/api/view/).
  It denies all URI loading and sanitization: file, HTTP, relative, inline data
  URLs, computed image URLs and hyperlinks. Inline JSON data and named inline
  datasets remain available. A denied load reports an error rather than a
  plausible empty chart.
- Canonical expression parsing is retained. Authored source is parsed as JSON
  and passed as data; it is never appended to executable script source.
- The shared interpreter supplies its memory, deadline, stack and output bounds;
  the shared composer supplies SVG structural, viewport, font and image bounds.
  There is no timer, canvas or DOM host. Timer-dependent animated transforms
  cannot run; a force layout needs explicit static configuration. Canvas-only
  transforms such as word clouds and label raster collision placement can fail
  with a visible diagnostic.
- Headless Vega estimates text widths without a canvas. Final label outlines use
  the existing bundled fonts, so layout is not advertised as pixel-identical to
  browser canvas metrics. Each result carries this limitation and the inactive
  pointer/tooltips/input-binding limitation as warnings. Preserve viewport
  composition retains chart padding rather than tightly cropping plot content.
- QuickJS lacks browser `structuredClone`, which Lite 6.5 calls. The local prelude
  preserves JSON values, undefined, non-finite numbers, dates, regular
  expressions, arrays/holes, maps, sets, typed arrays/buffers, cycles and shared
  references. Functions and symbols are rejected. Browser transferable objects,
  DOM objects, shared buffers and Error-specific cloning semantics are outside
  this data-only compiler use and are not claimed.

## Provenance and validation

Pristine npm archives were pinned, their SHA-512 integrity checked, and imported
files SHA-256 recorded separately in `vendor/vega-js/PROVENANCE.md` and
`vendor/vega-lite-js/PROVENANCE.md`. Both are BSD-3-Clause; each distinct copyright
text is retained. The conservative dependency license closure is recorded in
`vendor/vega-js/licenses/inventory.json`, with exact package/archive/license-file
hashes. It retains CLI/type dependency notices even when Rollup omits that code.
The Vega-Lite UMD sourcemap lists its own 181 source files and its only UMD
external is Vega. Bundles remain byte-for-byte pristine.

Review fixtures:

- `docs/examples/diagrams/native-vega.vega.json`: inline records, formula
  transform, band/linear scales, axes, title and rectangles.
- `docs/examples/diagrams/native-vega-lite.vl.json`: aggregation, grouped bars,
  field offset, legend, rounded marks and authored colors.

Empty-host JavaScript probes exercise both fixtures, layered line/point marks,
facets, rejected URL data/images and invalid/runtime expressions. Rust unit tests
cover those behaviors, native rasterization and clone-value preservation. In the
centrally serialized build window,
`cargo test -p little-clip-render -j4 --lib diagram::vega::tests` passed all five
tests (0.52 seconds runtime). The two canonical fixture SVGs were also rendered
with the installed `rsvg-convert` and visually reviewed. Host registry/editor
integration and native light/dark acceptance belong to the primary integration
pass; this adapter-level result does not claim those checks.
