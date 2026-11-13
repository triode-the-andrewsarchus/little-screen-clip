# UMLet / UXF rendering and editor research

Research date: 2026-11-13.

## Actual renderer options

| Candidate | Mechanism and license | Assessment |
| --- | --- | --- |
| [UMLet desktop 15.1](https://github.com/umlet/umlet/tree/v15.1) | GPL-3.0 Java element engine with positioned UXF XML, shape-specific property DSLs, relation decorations, text markup, hierarchy/plot/sequence elements and SVG export. | Selected as a separate local companion; no code is linked into the Rust application. |
| [UMLetino / VS Code UMLet](https://github.com/umlet/umlet/wiki/Information-for-Developers) | GWT browser frontend sharing `umlet-elements` with the desktop engine, with platform-specific drawing adapters. | A genuine implementation of the same format, but not a separate native renderer. Its selection/drag/property workflow is useful editor inspiration. No browser is embedded. |
| [Kroki UMLet](https://github.com/yuzutech/kroki/tree/main/tools/umlet) | A command-line adapter to the canonical Java engine; its current build uses GraalVM AOT, AWT/Fontconfig and explicit reflection/resource configuration. | Confirms headless canonical SVG rendering is practical. Its unrestricted custom JavaScript path is unsuitable for this preview contract; we do not adopt its CLI or custom-code behavior. |
| [Asciidoctor Diagram UMLet](https://docs.asciidoctor.org/diagram-extension/latest/diagram_types/umlet/) | A document extension invoking the UMLet conversion tool. | A useful wrapper, not an independent UXF renderer or native element implementation. |
| crates.io `umlet` / `uxf` search | No compatible UMLet diagram renderer found. Packages for unrelated generic UXF data formats do not implement UMLet elements. | Not interchangeable with UMLet's positioned diagram XML. |
| Independent native XML rendering | Positions and bounds are easy to parse, but each element embeds its own nontrivial property language. | A rectangle/label importer would lose relation multiplicities/qualifiers, all-in-one sequence/activity syntax, specialized shapes and markup. It is not presented as complete UMLet support. |

The [official FAQ](https://www.umlet.com/faq.htm) describes simple XML storage,
per-element markup, `bg=`/`fg=`, relation `lt=`, role/multiplicity/qualifier
properties and implicitly sticky endpoints. It does not make the element DSLs
trivial: [the canonical common palette](https://github.com/umlet/umlet/blob/v15.1/umlet-res/src/main/resources/palettes/UML%20Common%20Elements.uxf)
and the sequence, activity, plot and custom-drawing palettes are compatibility
examples for those languages.

The selected source/distribution revisions, hashes and notices are recorded in
[the import provenance](../../vendor/umlet/UPSTREAM.md). A separately launched
local Java companion uses the canonical element engine and SVG exporter. The
shared Rust Java runner supplies the existing bounded process lifecycle, and the
existing SVG composer validates, outlines and rasterizes the returned geometry.

## Execution boundaries

UXF can contain executable custom elements. The canonical desktop XML loader
can invoke a Java compiler for `custom_code`, load legacy types by name, and run
unrestricted Rhino JavaScript after the `_umletcode=` property. Merely disabling
the desktop custom-elements preference does not establish a data-only renderer.
Those execution paths must be rejected before construction and absent or disabled
in the separate companion. The bounded `customelement=` drawing DSL is a
different mechanism from arbitrary Java/Rhino code.

The chosen path uses no user configuration/home loading, authored input/output
paths, CLI mode, browser, HTML renderer or external include resolver. The source
audit found no image/file/URL loader in the builtin element packages or text/draw
adapters. Links remain presentation metadata rather than activated navigation.
XML must reject DTDs/external entities, executable elements and unsupported IDs;
malformed element properties must produce diagnostics instead of a deceptively
successful error-placeholder drawing. Coordinates, element counts, source/output
bytes, JVM reservations and process CPU/wall/address-space usage remain bounded.

## Graphical editing direction

The desktop [UMLet FAQ](https://www.umlet.com/faq.htm) documents palette insertion,
dragging/resizing, Ctrl-selection, rectangle lasso and direct property-text
editing. [UMLetino](https://www.umletino.com/) and the [VS Code extension](https://marketplace.visualstudio.com/items?itemName=TheUMLetTeam.umlet)
provide the same source-linked property workflow on other platforms. Their
GPL implementation is an interaction reference, not source copied into MIT Rust
widgets.

UXF is particularly suitable for later native editing of positions/sizes,
relation bend points and property text: preserve the XML source and modify the
relevant numeric/text ranges, while asking the canonical renderer to interpret
element semantics. The editor must account for `zoom_level`: stored element
coordinates use the saved zoom, while relation `additional_attributes` points
use default ten-grid units. A palette at zoom eight can have a relation width of
152 and endpoint coordinate 170, whose painted coordinate is 136.

Relation attachment is inferred from endpoint contact with element borders;
there is no persistent node-ID connection to reconstruct. A move/resize tool
must move the inferred attached endpoints together and retain untouched element
markup. Graphical editing and source-preserving geometry transactions remain
separate work from canonical rendering; arbitrary custom-code execution remains
unsupported.

## Qualification evidence

The fixed companion passed 19 exact canonical SVG geometry comparisons: all 18
non-executable upstream palettes and the local class/relation fixture. The
reference uses the pristine desktop parser/exporter with identical portable font
metrics. This covers relations, plots, bounded custom drawings, hierarchy,
sequence/activity and the legacy all-in-one elements. The upstream Advanced
Drawings palette is rejected for arbitrary JavaScript, with no partial artifact.

Thirteen malformed/forbidden/oversize cases passed, including DTD/local entities,
custom Java, custom JavaScript, unsupported IDs, invalid properties, coordinates,
element/source caps and excessive XML depth. A 512-element admitted plane
completed within the fixed native limits in 1.63 seconds. The internal canonical
jlibeps prepaint remains available solely for layout measurement; EPS export and
authored output paths remain inaccessible.
