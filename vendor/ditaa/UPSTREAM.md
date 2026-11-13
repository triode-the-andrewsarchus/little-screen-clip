# Pristine Ditaa 0.11.0 import

- Project: https://github.com/stathissideris/ditaa
- Tag: `v0.11.0`
- Commit: `99ba96c3b20b1adb6463095f2501ab0ec5195cac`
- Source: https://codeload.github.com/stathissideris/ditaa/tar.gz/refs/tags/v0.11.0
- Source archive SHA-256: `0cd69b13febd285d90fcdc220e81158aae0cba54c6d6bdfd38d97e708c611ebd`
- Official release: https://github.com/stathissideris/ditaa/releases/tag/v0.11.0
- Binary: https://github.com/stathissideris/ditaa/releases/download/v0.11.0/ditaa-0.11.0-standalone.jar
- Binary SHA-256: `9418aa63ff6d89c5d2318396f59836e120e75bea7a5930c4d34aa10fe7a196a9`
- Retrieved: 2026-11-13.

`upstream/` is the complete source archive with only its top-level archive directory removed. Its contents are unchanged. The release jar is unchanged. `licenses/` holds verbatim copies of all license and notice entries in that jar, preserving their paths; those entries also remain inside the jar.

Ditaa source headers and `project.clj` declare LGPL-3.0-or-later. The upstream README still says GPL and is retained as published. The standalone jar includes its upstream dependencies (Clojure, Apache Commons CLI, Jericho, Batik and XML libraries); their notices are retained in the jar and copied in `licenses/`. `upstream/LICENSE` is the canonical Ditaa license. No Java source is linked into the Rust application. A separately launched data-only companion uses the Ditaa library API; its local bridge and packaging instructions are separate from the pristine upstream source.
