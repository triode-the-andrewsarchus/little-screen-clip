# Native PlantUML renderer

plantuml-core 0.5.4, MIT, Copyright (c) 2026 plantuml-rs contributors.
Ported code derives from PlantUML, copyright (C) 2009-2026 Arnaud Roques,
under its MIT license option. The original license follows in this combined
notice. This does not import a GPL-only Java binary or require a JVM.

Source: https://github.com/bjornharrtell/plantuml-rs
Original crate revision: e4806bd9daa7d9542d219d05788cef60a0846f24
The released crate and the repository's original MIT notice were imported
separately before local modifications. See `vendor/plantuml-core`.

Local changes are limited to the manifest's shared Graphviz 1.2.0 pin, an
additive render-report API retaining warnings, and an additive checked block
splitter that reports incomplete diagrams. Original string-returning and
permissive splitter APIs remain available. The application adapter uses only
bundled resources, bounded embedded images and the existing worker/SVG pipeline.

The bundled C4-PlantUML sources are MIT, copyright Ricardo Niepel, kirchsth and
contributors; their original notice follows. The upstream crate has already
rewritten inter-library includes to bundled C4 resources, as described in its
`stdlib/README.md`. Its original theme files retain individual author/license
headers, including MIT and Apache-2.0 themes. The Apache-2.0 text is included
elsewhere in this combined license output. PlantUML's numerical font metrics
remain part of the upstream port; the application's bundled outline fonts retain
their existing SIL OFL notices.

No third-party editor UI is included. Omaspec (MIT, iced) and the native port's
VS Code preview were evaluated as editor references; the shared egui source/live
preview editor keeps its existing guarded save/history lifecycle.
