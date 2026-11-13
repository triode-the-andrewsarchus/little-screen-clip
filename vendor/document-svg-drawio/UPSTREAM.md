# Native draw.io renderer source

Pristine selected files from `document-svg` 2.0.2, revision
`e1284f0e5d1ce25328185524a129bc6a79f8b584`.

Upstream: https://github.com/ryusui-hiro/document-svg
Package: https://crates.io/crates/document-svg/2.0.2
License: MIT OR Apache-2.0; original license texts retained.

This import contains the draw.io reader/renderer, page IR, SVG writer and
supporting XML/error/SVG-validation sources. Unrelated Office, PDF, CAD,
CLI, font-loading and language-binding implementations are omitted.
`Cargo.toml.upstream` records the original manifest; it is not a build manifest.
The import commit precedes local adaptations so those remain reviewable.

## Local adaptations

- Focused library manifest uses existing base64/flate2/quick-xml/serde families.
- Memory-only input; no external library paths, file discovery or converters.
- Retain original inline stencil handling and embedded-SVG validation.
- Public bounded SVG report retains page metadata and compatibility warnings.
- Public page IR and SVG writer remain available for future native editing.
- Parsing budgets and a cumulative SVG output budget constrain preview work.
