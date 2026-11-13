# Diagram goal completion audit

Read-only audit on 2026-11-13, at `8e57bd34` plus concurrent working-tree changes.
Only this audit file is owned by this task. No Cargo, application build, GUI run
or production edit was performed for the audit. Historical test/GUI claims below
are evidence recorded in the plan/research notes, not newly executed checks.
The primary agent owns final rebuild, native light/dark acceptance and checklist
updates. Its reported pending Mermaid effect/hit-geometry correction and WireViz
presentation changes are present in source but still need final qualification.

## Main result

Every requested Markdown diagram family from Mermaid through Symbolator has a
shared language identity and an actual renderer dispatch. Front matter and
inline/display math have their own shared implementations. There is no remaining
entirely unimplemented Markdown family in the inspected registry. The new engines
must still pass the coordinating agent's final real-host/GUI pass before their
unchecked coverage rows are closed.

The strongest remaining implementation gap against an explicit user requirement
is **Excalidraw text creation/editing and resize handles/transactions**. The current
canvas does real vector drawing, selection, movement, delete and basic paint,
but text and resize remain absent at both transaction and UI layers. The active
plan calls them essential; a source editor alone does not satisfy that whiteboard
requirement. The compact icon/contextual toolbar requirement also needs a truthful
review: the current canvas toolbar is wrapped text tools and simple paint controls.

WireViz already has useful source-preserving direct manipulation. Creating and
removing connections is absent, and the plan explicitly calls those gestures
remaining required work. This is an editor acceptance gap, **not evidence that
WireViz lacks a renderer or its minimum source/live/error editor**. Template
instances remain rejected; that is an honest compatibility boundary rather than
a reason to demand every upstream feature before acknowledging basic support.

## Shared routes actually present

| Requirement | Source evidence | Audit conclusion |
| --- | --- | --- |
| All requested diagram fences | `little-clip-core/src/diagram.rs::Format::from_language`; `little-clip-render/src/diagram.rs::render_inline` | All catalog families are named and dispatched, including six distinct BlockDiag grammars, TikZ/PGF, Vega and Vega-Lite, UMLet and Symbolator. |
| Large Markdown | `clip-studios/src/text_preview/markdown.rs::render_blocks` asks the same registry/Preview owner; fallback retains styled source. | Common route covers the newly registered families; final representative native rendering is still the primary agent's job. |
| Little-screen Markdown and standalone text | `little-clip-render/src/cards/text.rs` resolves fences with `from_language` and source extensions with `from_source_extension`; completed fences retain bounded whole source. | No per-family painter missing from this path. Explicit/compound hints must remain registered; current new-family registrations are present. |
| Highlighted source, live preview and errors | `clip-studios/src/text_editor.rs::{TextSession::load,Editor::source_ui,Editor::diagram_ui}`; `diagram_preview.rs::status_ui` | All recognized standalone formats use the existing source/live/error owner. Syntax lookup uses `Format::name()`, and bundled syntax definitions cover the registry names. Failed drafts retain a clearly labelled last-valid preview. |
| Guarded source lifecycle | Same TextSession/Editor and FileSource save/Save As, undo and accepted-write owners | No new family-specific writer is needed. Source editing for difficult graphical languages satisfies the clarified minimum. |
| Standalone strips and enlarged hover | `clip-file-manager/src/file_manager/diagram_strip.rs::{Document::read,Document::render,show,hover}` | Shared worker/cache/identity route covers explicit candidates and admitted ambiguous extensions, including `.diag` and `.tex`. Existing text fallback and diagnostics remain. |
| Compact front matter | `file_manager/text_strip/metadata.rs`; plan's compact metadata checkpoint | Implemented and previously reviewed light/dark; not a remaining implementation gap. |
| Rendered Markdown hover at hovered position | `file_manager/markdown_hover.rs`; `text_preview/markdown/excerpt.rs`; plan's completed hover checkpoint | Implemented, including native source ranges, complete fences/equations and bounded diagnostic fallback. Do not reopen it merely because an older checkpoint says pending. |
| No browser engine | Native Rust renderers, shared hostless QuickJS Program, bounded Python/canonical Java companions and static GoAT helper | No browser/webview route is introduced by the inspected adapters. Java/Python/Go companions are explicit native process/build prerequisites; they are not browser rendering. Resource refusal and budgets still apply. |

Relevant code paths are relative to `crates/` in this table. The shared registry
is the completion mechanism: adding a separate Markdown viewer, cache, file
reader or generic graphical-editor framework would not close a missing route.

## Remaining user-visible acceptance items

1. **Final native validation and rebuild.** Newly registered families, deployment
   and modern DBML need the real application worker/editor/Markdown/strip paths
   qualified against the final linked binaries. The current paused GUI pass,
   Mermaid effect-hit fix and latest WireViz patch must be included. Validate
   actual artwork/geometry and diagnostics, not only OCR/source words. Historical
   worker tests and fixture files alone do not close this requirement.
2. **Whiteboard text and resizing.** Core `diagram/drawing.rs::TOOLS` contains
   rectangle, ellipse, diamond, arrow, line and freehand tools. `Change` contains
   Move/Paint/Delete/Create, with no text or resize operation. The studio
   `text_editor_drawing/canvas.rs::Gesture` is Move/Draw/Marquee only. Existing text
   may render and move; that is not text editing. Selection outlines are not
   resize handles. This is concrete missing functionality, not perfect-format
   compatibility.
3. **WireViz connection authoring, as promised by the active plan.** The inspector
   selects real pin/conductor geometry, traces the circuit and edits labels/colors.
   The edit model and UI do not create/delete links. Implement bounded,
   source-planned connect/disconnect with the same revision/undo/save owners if
   closing the plan's recorded requirement; do not substitute a second circuit
   model or mutate Graphviz geometry alone. This does not block checking the
   narrower source/live renderer baseline as implemented.
4. **Finish accurate documentation/checklists.** Several unchecked format rows
   and old “pending” notes now contradict implemented code. Several checked rows
   still describe an editor as entirely pending when initial editing exists.
   Reconcile them after acceptance, retaining specific compatibility qualifiers.
5. **Resolve misleading/unfinished standalone associations.** `.bob`, `.bf` and
   `.sym` are not the same as the implemented explicit alternatives. The table
   below states what actually opens. If retaining these suffixes as promised
   associations, they need safe content qualification and host/strip admission;
   otherwise correct the catalog rather than checking nonexistent routes.

The first two items have direct required-outcome evidence. The WireViz connection
item is additionally called required by the plan's source-preserving-harness
checkpoint. Full flowchart/whiteboard/WireViz upstream editor parity is not implied.

## Current standalone catalog checkboxes

“Baseline present” means real routing/renderer/editor code exists; it does not
claim the pending final native pass has already passed.

| Current `file_types.md` row | Actual code status | Honest checkbox action after native acceptance |
| --- | --- | --- |
| Unchecked `.uxf` | `.uxf`/`.umlet` candidates, `umlet`/`uxf` fences; canonical data-only companion; XML grammar. | Can mark the bounded UMLet baseline supported; retain refusal of executable custom Java/JavaScript and unsupported custom elements. |
| Unchecked `.structurizr`, `.dsl` | Explicit `.structurizr`; content-qualified `.dsl`; native C4/dynamic/filtered/custom and now nested deployment rendering. | Baseline present. Deployment is no longer missing. Image views, deployment groups and richer DSL/style parity remain qualified limitations. |
| Unchecked `.d2` | Native D2 source/live/error route and normal standalone association exist. | Mark the existing bounded flat-board baseline; board/layer/scenario support and graphical editing are separate limits. Binary Diablo II data is not D2 source. |
| Unchecked `.diag` and six family suffixes | Explicit family suffixes plus content-qualified `.diag`; distinct canonical parsers and shared source/live/strip paths. | Baseline present for all six; generic diagnostic logs must remain ordinary text. |
| Unchecked `.bytefield`, `.bf`; later checked `.bytefield` row | `.bytefield` is explicit; upstream `.edn` is content-qualified. `.bf` is **not** Bytefield and is mapped to Brainfuck syntax. | Consolidate duplicated Bytefield rows around real `.bytefield`/`.edn` support. Do not check `.bf` as implemented or redirect ordinary Brainfuck. |
| Unchecked WireViz compounds | `.wireviz`, `.wire.yaml`, `.wire.yml`; conservative generic YAML inference; highlighted source/live/errors and graphical pin/color inspector. | Renderer baseline present. Keep connect/disconnect and template instantiation separately disclosed. Palette/presentation improvements are in the concurrent patch and need final tests/review. |
| Unchecked `.sym` for Symbolator | No `.sym` route. Actual explicit inputs are `.symbolator` and `.symbolator.v/.vlog/.vhd/.vhdl`; ordinary HDL stays ordinary code. | Correct the catalog to the actual HDL documentation format. A compiler symbol file must not become Symbolator merely by suffix. Add `.sym` only if a real Symbolator content convention is deliberately supported. |
| Unchecked `.ditaa` | Explicit source/live/error and strip routes plus canonical companion. | Baseline present; runtime/resource and bounded grammar/options limits remain explicit. |
| Unchecked combined `.bob`, `.svgbob` (nested `.svgbob` checked) | `.svgbob` works. Fence alias `bob` works. **No** standalone `.bob` recognition exists in `from_path` or `from_source_extension`. | Keep `.bob` separate/unchecked until content classification is implemented or explicitly outside claimed associations. The existing `.svgbob` baseline is supported. |
| Unchecked `.vg.json`, `.vega` | Also supports `.vega.json`; canonical Vega through shared QuickJS; source grammar and live editor. | Baseline present. Inline bounded data only; external resources and browser interactions are not supported. |
| Unchecked `.vl.json`, `.vegalite` | Also `.vega-lite.json`/`.vegalite.json`; canonical compiler plus Vega renderer. | Same qualified baseline; do not hold it for arbitrary browser interactive charts. |
| Unchecked `.tikz`, `.pgf` | Native Tikra source/live/error paths; `.pgfplots` and content-qualified `.tex` also exist. `.pgf` content classifier requires TeX text. | Baseline present with explicit unsupported TeX/library/resource diagnostics. See the binary PGF admission issue below. |
| Checked `.excalidraw` row says graphical canvas pending | A native source-preserving vector canvas actually exists. | Update wording to initial canvas with text/resize still missing; do not claim those essentials are complete. |
| Checked Mermaid row says graphical editing pending | Parser-backed flowchart canvas now supports selection, label editing, shaped new nodes, connections and constrained delete; other families retain source/live. | Update wording after the pending effect/hit fix is qualified in the actual host. |
| Checked `.dbml` row says newer syntax pending | Partials, ordered fields, records and explicit optional endpoints landed in `8e57bd34`; source/live/raster checks passed. | Replace obsolete blanket qualifier with actual remaining modules/checks/date/custom-type limits. |

There is one concrete ambiguity mismatch beyond checkboxes: file-manager
`viewers.rs::kind` prioritizes **extension-only** `Format::from_path`, which returns
TikZ for every `.pgf`. The more careful `from_source_path` rejects binary PGF only
later in TextSession/strip classification. Consequently a binary Progressive
Graphics File is still dispatched to the text studio, not a content-qualified
ordinary image route. This does not prevent real TeX `.pgf` rendering, and adding
a PGF raster codec is outside this diagram goal; nevertheless, do not describe
all host PGF ambiguity handling as finished. Handle/reject that admission
honestly without treating binary image bytes as TeX.

## Unchecked Markdown rows versus implementation

The unchecked DBML, Structurizr, BlockDiag family, Ditaa, TikZ/PGF, UMLet,
Vega/Vega-Lite and Symbolator rows now represent **final qualification/checklist
work**, not absent family engines. WireViz likewise has a renderer and baseline
editor, with the specific editor/compatibility gaps above. Its palette and
presentation qualifier must be updated after the concurrent implementation is
validated. The parent “Inline diagram block rendering” checkbox should remain
open until the final family/host acceptance and this documentation reconciliation
are complete; there is no need for a new generic inline-diagram implementation.

## Limitations that do not automatically block basic support

- Graphical source correspondence is required for the operations offered, not
  a universal per-element span system for every rendering language. Excalidraw
  identity/transactions, WaveDrom concrete JSON5 literals, WireViz YAML patches
  and Mermaid parser facts already own their corresponding edits. D2 source
  editing was explicitly accepted as sufficient. Source-only editors for the
  other difficult families are not missing minimum editors.
- WaveDrom lane/cell painting and renaming are implemented and byte-preserving.
  Group/lane creation, advanced arcs/data/period controls and register/logic
  graphical manipulation remain source operations; they do not invalidate timing
  rendering or the existing useful painter.
- Native retained vector artwork and supported bound connections do follow
  Excalidraw gestures, including beyond original export bounds. Earlier unchecked
  vector checklist items are partly superseded by later validated checkpoints.
  Mask/filter/blend fallback is explicitly disclosed; perfect effect parity is
  separate from the missing text/resize essentials.
- Structurizr deployment now renders selected placement IDs and actual nested
  containment/links. Image views and deployment-group constraints still fail
  explicitly. An unsupported view error is a real scope limit, not proof that
  C4/deployment rendering is absent.
- Modern DBML preserves source and has native partial/record/optional-ref support.
  Module imports, table checks, index sort-order syntax, full SQL coercion and
  inferred nullable minimum cardinality remain qualified limits.
- WireViz template creation is a finite but unimplemented normalization feature.
  Rejected template documents must retain their diagnostic/source. This is a
  visible compatibility gap and should remain stated, without silently upgrading
  a renderer baseline claim to full canonical compatibility.
- External images/includes/data URLs, arbitrary code/custom plugins, unrestricted
  file/network resources and browser event interaction may remain refused by
  design. Safe refusal is not missing “perfect format” support. Packaged companion
  discovery, source/license notices and actual-host entrypoint validation remain
  necessary regardless of that refusal.

The goal should remain active for the explicit UI/acceptance/documentation work
above. It should not remain open solely to chase every upstream grammar feature,
all graphical operations in every family, unsupported resource loading or exact
pixel parity with browser editors.


## Subsequent integration resolution

This audit is historical at its stated snapshot. The identified required gaps
are now implemented and qualified: native text/resize, harness connect/disconnect,
ambiguous `.bob`/`.bf` admission, corrected Symbolator catalog association, and
both-theme real UI checks. The main support plan's completion qualification and
architecture review record final evidence and bounded compatibility limits.
Binary PGF remains outside raster decoding; its content is never treated as a
valid TikZ source without a TeX signature. Optional full-format/editor parity
items stay explicitly tracked rather than being claimed complete.
