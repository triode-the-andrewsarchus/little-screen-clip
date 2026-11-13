# BlockDiag family: local renderer selection

Research checked 2026-11-13. The six languages are BlockDiag, SeqDiag, ActDiag,
NwDiag, PacketDiag and RackDiag. These are distinct real grammars/layouts, even
though they share a drawing substrate. Converting their text to DOT with a small
handwritten parser would lose sequence, lane, network, bit-field and rack semantics.

## Candidates and decision

| Candidate | Actual implementation | Fit |
| --- | --- | --- |
| [Original BlockDiag](https://github.com/blockdiag/blockdiag), [SeqDiag](https://github.com/blockdiag/seqdiag), [ActDiag](https://github.com/blockdiag/actdiag), [NwDiag](https://github.com/blockdiag/nwdiag) | Python parsers using funcparserlib, dedicated layouts, native SVG writer; NwDiag includes PacketDiag/RackDiag | Canonical language implementations; reuse the actual parser and drawing code |
| [YuzuTech BlockDiag](https://github.com/yuzutech/blockdiag) | Maintained fork of the common parser/layout/drawing substrate; used by Kroki | Selected common library, pinned to v3.4.3, with external-image denial support |
| [Kroki](https://github.com/yuzutech/kroki) | Gateway invoking the actual BlockDiag family libraries; native binary distributions bundle Python implementations | Valid reference and integration evidence, but a network server/container is unnecessary for local previews |
| [Sphinx extension](https://github.com/blockdiag/sphinxcontrib-blockdiag) | Documentation integration around BlockDiag | Does not replace or improve the parser/layout and adds Sphinx dependencies |
| [funcparserlib](https://github.com/vlasovskikh/funcparserlib) | The actual combinator parser dependency used by the canonical grammars | Selected pinned dependency, not a competing renderer |

Searches for native Rust and hostless JavaScript implementations did not identify a
credible complete implementation of all six languages. The selected libraries
are Python, not Rust ports. A small isolated Python companion is justified here
because it preserves the real grammar/layout with a much smaller maintenance
surface than six new renderers. It uses neither a browser engine nor a hosted
conversion service. Python and Pillow are runtime dependencies; the language,
parser and color libraries are vendored and embedded in the application.

Pinned sources and archive digests are in
[`vendor/blockdiag-family/PROVENANCE.json`](../../vendor/blockdiag-family/PROVENANCE.json).
All imported files remain pristine, with upstream license headers and license
files. BlockDiag/SeqDiag/ActDiag/NwDiag are Apache-2.0; the parser code also retains
its original MIT notices. funcparserlib is MIT and webcolors is BSD-3-Clause.

## Canonical editors

The [official interactive shell](https://github.com/blockdiag/blockdiag_interactive_shell)
is a Flask/Google App Engine text editor with live output and URL-based source
storage. It is not a reusable drag-and-drop scene editor. The
[Kroki editor](https://github.com/yuzutech/kroki.io) and
[Krokidile](https://github.com/sommerfeld-io-archive/krokidile) likewise pair text
with a rendered preview. They depend on server rendering and browser UI.
Embedding those editors would add a browser/runtime without supplying a native
round-trip graphical model. Source editing with immediate preview/errors is the
faithful practical editor for these grammars. Free positioning would fight their
automatic layout; a future property panel should edit authored tokens through the
canonical grammar rather than convert to another drawing format.

## Local execution boundary

The adapter embeds a fixed table of pinned trusted Python modules and a fixed
bootstrap. Diagram source and language selection are separately serialized data;
source is never evaluated as Python. `python3 -I` prevents user-site and Python
path injection. Modules are imported from the embedded table, with no runtime
package download/install, working-directory dependency or user configuration.
Only built-in SVG and node shapes are registered; entry-point discovery and
third-party plugins are disabled. Every grammar is parsed before building, and
AST validation rejects authored resource and plugin attributes, including class
attributes and defaults. Upstream external-image denial plus a failing image
resolver protect the drawing path. Font measurement uses bundled face bytes.
A Python audit hook denies filesystem, directory, network and child-process access
after trusted setup. The existing `CommandRequest` bounds time, memory, output and
child lifetime. The existing SVG composer owns outline conversion, image policy
and final viewport/raster budgets.

## Compatibility

Canonical nodes, groups, edges, sequence messages/fragments, activity lanes,
network memberships, packet bit ranges and rack units retain their own upstream
layout. Supported styling follows the pinned library, including named/hex colors,
labels, node shapes and line styles. Plugin extensions, authored image resources,
background images, external font paths and runtime configuration are intentionally
rejected with an actionable error before drawing. Hyperlink/resource attributes
are disabled in local previews. Invalid syntax and unsupported attributes return
errors rather than partial diagrams. Bundled fonts cover ordinary text; glyph
coverage and upstream automatic layout remain subject to their normal limits.

## Validation evidence

Isolated Python bridge probes successfully parsed and rendered the six checked-in
fixtures, with no stderr warnings. Per-language probes rejected external images,
background URLs, font paths, plugin attributes and plugin declarations; class
resource attributes were also rejected in the four grammars that support classes.
PacketDiag and RackDiag reject class declarations at the grammar level. Visual
inspection confirmed canonical packet bit spans and compact rack-unit placement.
Owned adapter measurements report zero duplicated lines/clone groups and maximum
cyclomatic complexity 13; pristine upstream sources account for imported grammar
and layout complexity. `cargo test -p little-clip-render -j4 --lib diagram::blockdiag::tests` passed
all three tests, covering all six layouts through outline conversion and
rasterization, 34 resource denials and syntax/source-budget errors. Application
acceptance validation is run by the coordinating agent.
