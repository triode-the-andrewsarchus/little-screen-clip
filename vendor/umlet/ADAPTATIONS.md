# Local UMLet SVG companion

UMLet 15.1 source code and the official ZIP remain unchanged. The upstream
`.gitattributes` declares `text=auto`; Git may normalize original CRLF text when
checking out the pristine import. UPSTREAM.md preserves the exact archive hashes.
All local Java code stays in this separate GPL-3.0 companion; none is copied into
the Rust application.

The bridge calls canonical builtin factories and the SVG export API directly.
It never invokes the desktop entrypoint, InputHandler's source-driven reflection
or Java compiler, ConfigHandler, HTML widgets, file pickers or file export paths.
XML parsing rejects DTDs/external entities and applies strict known fields,
512-element/eight-group-level limits, bounded coordinates and UTF-8/source caps.
Modern IDs are checked against the canonical enum. Known legacy builtin types
use the explicit constructor table in LegacyFactory; custom types/code fail.
No source string enters reflection or a class loader.

Confined adaptations:

- `bridge/ClipUmletPreview.java` supplies the fixed stdin/XML -> SVG protocol and
  buffered artifact publication. Authored `_umletcode=` and `custom_code` fail
  before any canonical element is constructed.
- `bridge/LegacyFactory.java` maps the predefined legacy builtin classes directly
  to constructors, excluding ErrorOccurred/custom elements.
- `bridge/com/baselet/diagram/draw/swing/javascriptparser/JavascriptParserSwing.java`
  replaces Rhino execution with an unconditional explicit error. The runtime
  selection omits Rhino, Java compiler/bytecode libraries and source class-loader
  implementation classes. The bounded canonical JavaCC `customelement=` drawing
  DSL remains available; it is not a general-purpose code evaluator.
- `bridge/com/baselet/diagram/FontHandler.java` is the canonical file with font
  creation redirected to bundled Arimo/Tinos/Cousine resources. Generic family
  settings retain their meaning without system-font discovery for text metrics.
- `bridge/com/baselet/element/NewGridElement.java` is the canonical file with
  `drawError` replaced by throwing the property error. Invalid markup reports an
  error rather than succeeding with an embedded error-placeholder drawing.
- `bridge/com/baselet/diagram/io/OutputHandler.java` retains the canonical SVG
  export/painting routines and only the SVG branch of `createToStream`. Other
  output formats are unavailable; their class references would otherwise force
  unused PDF/EPS libraries to load while verifying the SVG entrypoint.
  Its generated SVG DOM is serialized with a secure local JDK transformer,
  avoiding Batik's fixed public SVG DOCTYPE. Authored DTDs remain rejected and
  the shared SVG parser keeps its DTD prohibition.

Exact adapted-source differences can be inspected against the matching paths
under `upstream/umlet-swing/src/main/java` and
`upstream/umlet-elements/src/main/java`.

The builtin element/text/draw source audit found no image/HTML/include/file/URL
payload loader. Link metadata does not activate a browser while rendering.
Custom Java and Rhino are rejected and unavailable; no user configuration is
loaded. The fixed bridge accepts no command arguments. All canonical source
processing additionally runs within the shared Rust Java runner's fixed heap,
metaspace, stack, code-cache, processor, CPU, address-space, wall and output limits.

## Rebuild and replacement

```sh
python3 vendor/umlet/build.py /absolute/output/clip-umlet.jar
java -Djava.awt.headless=true -jar /absolute/output/clip-umlet.jar < diagram.uxf > diagram.svg
```

Building requires local JDK 11+; `LITTLE_CLIP_JAVAC` selects that compiler. Runtime
requires Java 11+ with headless AWT. The build is offline and extracts only the
explicit SVG runtime library selection in build.py. It retains every selected
artifact's META-INF metadata/notices under a distinct dependency path, avoiding
fat-JAR license/notice overwrites. The complete source snapshot, adaptations,
build instructions, provenance and notices are embedded under
`clip-umlet/source/`; metric-font OFL notices are retained too. Cargo publishes
the resulting JAR as an immutable hashed sidecar beside the host application.

The canonical legacy sequence/activity elements use jlibeps 0.1 for an internal
memory-only dummy prepaint to measure layout. Its tiny binary is retained from
the official ZIP, with complete original sources/license/build.xml retained in
`jlibeps-pristine/` and embedded in the companion. This preserves canonical
geometry without exposing EPS export or authored filenames.

The official build-input ZIP also contains unused PDF, editor, compiler, Rhino,
mail and logging libraries. Those are not distributed in the runtime companion.
If distributing that full pristine ZIP, assess those additional artifacts'
respective source/notice requirements. Selected SVG dependencies are Apache
Batik/Commons IO, SLF4J and jlibeps (GPL-2.0-or-later) with retained notices/source;
no broad licensing guarantee is inferred from the application process boundary. The canonical
source rebuild uses the retained Maven reactor (`mvn -pl umlet-swing -am package`)
and its pinned dependency/plugin coordinates, which may require externally
provided Maven artifacts; application builds do not run Maven or fetch them.
