# Native diagram renderer notices

These notices cover the Merman, Lini and Graphviz rendering families, not the complete workspace dependency inventory.

Merman 0.10.0 and its MIT/Apache-2.0 components are used under the MIT license
(reproduced in `merman-MIT.txt`). Upstream: https://github.com/Latias94/merman.

The native ELK implementation, `merman-elk-layered` 0.10.0, is a separate,
unmodified EPL-2.0 dependency. Its license is reproduced in
`merman-elk-EPL-2.0.txt`. Source corresponding to the locked release is available at:

- https://crates.io/api/v1/crates/merman-elk-layered/0.10.0/download
- https://github.com/Latias94/merman/tree/e0e75bcab2fa8e39dd7a09465b7f2e035eda82ef/crates/merman-elk-layered

The published crate includes original source references beside the translated
algorithms and the EPL license. Its recorded derivation baselines are:

- Eclipse ELK: https://github.com/eclipse-elk/elk/tree/62d5909f96fad541bc101ad52dabaece6b7eab7e
- elkjs: https://github.com/kieler/elkjs/tree/a8304cf79fde75bc2ab1a89d28320f53f8637436
- Mermaid adapter: https://github.com/mermaid-js/mermaid/blob/41646dfd43ac83f001b03c70605feb036afae46d/packages/mermaid-layout-elk/src/render.ts

These notices and both license texts are embedded in every diagram-capable host.
Run the executable with `--diagram-licenses` to print them without starting its UI,
daemon runtime or display hardware. Retain this notice, license texts and source
availability information when redistributing the executable.

## Lini

Lini 1.1.0 is used as an unmodified MIT dependency. Its license is reproduced in
`lini-MIT.txt`. Source: https://github.com/monfa-red/lini and
https://crates.io/api/v1/crates/lini/1.1.0/download.

Default features include Google Sans / Google Sans Code font subsets under the
SIL Open Font License 1.1 and Phosphor duotone icon geometry under MIT. Their
upstream notices are reproduced verbatim in `lini-google-sans-OFL.txt`,
`lini-google-sans-code-OFL.txt` and `lini-phosphor-MIT.txt`. Fonts are not modified
by this application. Static SVG conversion outlines labels using these bundled
subsets, without needing system font installation.

## Graphviz

The unmodified graphviz-pure 1.2.0 family is a native port of Graphviz 16.1.0,
licensed under EPL-2.0, with MIT-licensed CORE-MATH and Expat ports. Full upstream
notices and license texts accompany this file. Corresponding source:

- https://crates.io/api/v1/crates/graphviz-pure/1.2.0/download
- https://github.com/graphviz-pure/graphviz-pure/tree/234fc35f116bceca2c34579879a6e6893112ccf8

Cargo.lock pins the component crates (core, geom, layout, parser, render) to
1.2.0. Each is available through the same crates.io release-download endpoint,
substituting its package name. Source provenance and original notices remain in
those packages. This application does not modify the port or compile its CLI,
PDF/raster package, parallel execution feature or math-tracing feature.

Tinos, Arimo and Cousine font assets are copied unchanged from
`graphviz-pure-raster` 1.2.0 under SIL OFL 1.1. Their full notices are retained
under `assets/fonts/graphviz/OFL-*.txt` and printed by `--diagram-licenses`.
These fonts supply the corresponding Times, Helvetica and Courier metric
families. The existing Noto fonts remain fallbacks for other glyphs.
