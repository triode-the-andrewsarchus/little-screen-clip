# Local Ditaa companion adaptations

The pristine snapshot in `upstream/` and official standalone JAR are unchanged.
The companion overlays only these separately tracked source files:

- `bridge/ClipDitaaPreview.java` is the local stdin/stdout entry point. It calls
  canonical `TextGrid`, `Diagram` and `SVGRenderer`, accepts a bare canvas or one
  complete Ditaa block, applies a fixed whitelist of bounded rendering options,
  rejects malformed UTF-8/XML controls and bounds expanded cells/output. It never
  invokes the upstream CLI, HTML/config parsers, custom shapes or file/URL input.
- `bridge/org/stathissideris/ascii2image/graphics/FontMeasurer.java` is the upstream
  0.11.0 file with its `Dialog` font name replaced by loading the existing bundled
  `Cousine-Bold.ttf` resource and deriving each measured font from that face.
  This avoids host font discovery for diagram text metrics. Its original LGPL
  header remains. No geometry/parser algorithm is changed.
- `bridge/org/stathissideris/ascii2image/graphics/SVGBuilder.java` is the upstream
  0.11.0 file with a provenance/license header and the text serialization changed
  from unescaped CDATA to XML-escaped text (`&`, `<`, `>`). This correctly handles
  label strings containing `]]>` without permitting label text to add SVG nodes.
  No shape construction, path ordering, shadows, colors or shared-edge behavior
  is replaced.

Exact source changes can be inspected with:

```sh
diff -u vendor/ditaa/upstream/src/java/org/stathissideris/ascii2image/graphics/FontMeasurer.java vendor/ditaa/bridge/org/stathissideris/ascii2image/graphics/FontMeasurer.java
diff -u vendor/ditaa/upstream/src/java/org/stathissideris/ascii2image/graphics/SVGBuilder.java vendor/ditaa/bridge/org/stathissideris/ascii2image/graphics/SVGBuilder.java
```

`build.py` uses a local JDK 11+ to compile these three files against the pinned
official standalone JAR. It selects canonical `org/stathissideris/` classes and
resources plus `META-INF/` and `license/` notices/metadata into a new JAR, overlays
compiled classes, and adds the existing metric font and its license. It uses
no network, external dependency resolver or downloaded compiler. Cargo packaging
copies the resulting immutable, content-hashed companion alongside the host.
Java 11+ with headless AWT is required at runtime.

The runtime selection omits unused Clojure/Batik/Jericho/Commons dependency
packages. They support upstream's CLI/HTML/configuration/custom-image modes,
which the fixed bridge never invokes; canonical Ditaa's parser, shape analysis
and SVG construction are retained. The differential fixture probe compares the
selected companion's geometry against the complete pristine JAR. Every sidecar
also contains the complete pristine engine source, local adaptation sources,
build/probe scripts and retained notices under `clip-ditaa/source/`.

## Replacing or rebuilding the companion

```sh
python3 vendor/ditaa/build.py /absolute/output/clip-ditaa.jar
printf '+-----+\n|hello|\n+-----+\n' | java -Djava.awt.headless=true -jar /absolute/output/clip-ditaa.jar
```

`LITTLE_CLIP_JAVAC` selects a local build compiler. The Rust invocation uses a
fixed `java` runtime command with explicit resource reservations and no authored
arguments; the JAR can be rebuilt/replaced independently from the Rust binary.
`upstream/project.clj` and `lein uberjar` describe the canonical engine rebuild
from the complete retained source. Its old dependency versions require a suitable
Leiningen/JDK environment and locally available or externally fetched dependency
artifacts; the application build does not perform that dependency fetch.

## Dependency records and distribution

The pristine official standalone build input includes Clojure 1.9.0, Commons CLI 1.4,
Jericho HTML 3.4 and the Batik 1.9 / XML dependency closure, as recorded by
`upstream/project.clj` and retained `META-INF/maven/**/pom.properties` entries.
Its license/notice entries remain in every rebuilt companion and are copied
verbatim under `licenses/` for inspection. Ditaa source headers/project metadata
say LGPL-3.0-or-later; the upstream README's older GPL wording is retained.
`licenses/GPL-3.0.txt` also supplies the GNU GPL version 3 incorporated by the
LGPL supplement, copied verbatim from https://www.gnu.org/licenses/gpl-3.0.txt
on 2026-11-13; it is additional to the pristine JAR's extracted notices.

Complete Ditaa/adaptation sources, build scripts, provenance and notices are
embedded in the sidecar. The pristine build-input JAR is retained separately in
the repository and is not copied wholesale to the runtime companion. If
distributing that pristine full JAR, its pinned dependency coordinates, POMs and
notices identify additional dependencies; this source snapshot does not contain
every transitive dependency's full source. Distributors must assess and satisfy
the applicable source/notice obligations of the artifacts they distribute. The
Rust application does not statically link this Java code. The bundled metric
font's OFL notice is included in the sidecar; canonical font assets/licenses
remain under the render crate's `assets/fonts/graphviz` directory.
