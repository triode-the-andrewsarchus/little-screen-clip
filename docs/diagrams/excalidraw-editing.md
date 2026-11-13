# Native drawing text and resizing

The Text tool creates standalone text at the clicked canvas position. The
docked Text section edits existing standalone text and reciprocal rectangular
labels; Add label creates both binding directions on a selected empty rectangle.
Apply uses the existing accepted-edit worker, shared undo/save ordering and
source identity. Unchanged text is a semantic no-op.

Text dimensions use the renderer's bundled font shaping, fallback and bidi runs,
with real glyph advances and line height. No character-count estimate, new font
stack or scene-authoring rewrite is used. Unknown neighbours, nested future
binding fields and exact numeric values remain preserved.

Corner handles resize admitted unrotated shapes, images, standalone text and
unbound nondegenerate paths/freehand. Live previews transform actual retained
artwork; image crop/flip/assets and path pressures remain intact. Rectangular
labels retain alignment, and simple bound arrows use the existing endpoint and
label updater. Export-origin offsets are accounted for in the live affine.

Rotated/group/frame, elbow/fixed-segment, wrapped-label and unsupported binding
cases refuse with source-edit guidance. Labels must fit; full text reflow and
perfect editor parity are not claimed. Effect fallback hides resize handles
rather than pretending to transform unsupported raster artwork.

Focused core, renderer and studio tests and strict library Clippy pass. They
cover nonzero export origins, no-op preservation, bindings and nonzero-origin
freehand paths. The native text/resize scenario now passes actual light/dark GUI qualification
against rebuilt hosts.

## Validation follow-ups

- [x] Preserve entered text when label fit, font measurement or the edit deadline
  rejects Apply. Successful/no-op acceptance closes only its matching draft;
  newer drafts, selections and source revisions remain protected. Forty-eight
  editor tests and strict studio library Clippy pass, including rejected-draft,
  timeout, cancellation and no-op lifecycle cases.
- [x] Qualify text/resize and rejected-label recovery in both rebuilt themes.
  `/tmp/diagram-text-resize-dark-20261113-4/report.json` and
  `/tmp/diagram-text-resize-light-20261113-3/report.json` pass32 observations each,
  with actual held artwork, source/metadata preservation and exact Undo/Save.

## Fixed controls follow-up

The tools occupy one centered icon row with keyboard shortcuts: H pans, V
selects, R/D/O draw shapes, A/L draw connections, P draws freehand and T creates
text. Stroke/background swatches, line widths, sloppiness and opacity live in a
fixed left sidebar. Text drafts use that same sidebar, retaining the existing
Apply/rejection/undo semantics; no detached inspector window is created.

Zoom, actual size and Fit controls sit below the canvas. The host keeps its
existing Save and Undo/Redo owners. Properties scroll inside their own bounds
when the host is short; the artwork never moves behind the sidebar.

The fixed-control follow-up passes the real light/dark text/resize scenarios in
`/tmp/drawing-sidebar-dark-20261113-final2/report.json` and
`/tmp/drawing-sidebar-light-20261113-final2/report.json` (31 observations each).
These cover exact draft input, rejected-label recovery, held artwork and exact
Undo/Save restoration. Help/errors share one bounded status scroller below the
zoom controls, so extra messages do not grow over the surrounding host content.
