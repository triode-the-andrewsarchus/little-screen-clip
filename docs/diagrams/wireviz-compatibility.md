# WireViz native compatibility audit

Audited 2026-11-13. The initial audit used read-only code/source research.
The subsequent bounded implementation/validation is recorded below; no new
dependencies were introduced.
Canonical source is pinned to
[`5f961acd4c1ddfa509b0eb02ef4bfed7a510b7c7`](https://github.com/wireviz/WireViz/tree/5f961acd4c1ddfa509b0eb02ef4bfed7a510b7c7),
dated 2026-10-25. Repository implementation/acceptance claims below come from
inspecting the existing source and tests, rather than executing a new suite.

## Recommendation

Extend the existing normalized model, Graphviz renderer and guarded editor.
Component autogeneration is a finite data operation; it needs no template
language evaluator. HTML report templates are a separate output feature.
First close the palette/inference/presentation gaps, then add instance provenance
before admitting generated components to graphical transactions.

## Audit baseline and visible gaps

| Area | Existing native behavior | Bounded next change |
| --- | --- | --- |
| Circuit | Typed pin identities, ranges, lane expansion, cable wires/shields, loops, pin/whole-connector mating and unconnected referenced components are normalized. | Reuse this circuit and its limits; do not build a second interpretation for the canvas. |
| Palettes | Explicit `colors` repeat/trim; every `color_code` currently errors, even alongside an explicit list. | Implement seven finite schemes and canonical explicit-color precedence. Unknown selected schemes remain errors. |
| Connector inference | Pin IDs/labels infer counts; a `pincolors`-only definition currently fails. | Include color-list length in count inference while preserving explicit-count validation. |
| Simple connectors | `simple` controls the single-pin rule but the renderer still paints the full name/count/pin table. | Paint the compact form and apply its visibility defaults. Keep physical pin identity for hit testing. |
| Visibility | Name/count/wire-number flags and `hide_disconnected_pins` are not represented in the model. All ordinary rows remain visible. | Add typed presentation flags; hidden rows must not remove electrical terminals or renumber visible ports. Loops count as populated pins. |
| Bundle appearance | `bundle` changes a title string; its box is otherwise ordinary and every wire is numbered. | Use the intended dashed enclosure and default hidden wire numbers, with explicit override support. |
| Colors and text | Wire bands are real colors; component/global backgrounds, color-name formatting and requested fonts are ignored. | Honor bounded color properties and known text presentation. Keep portable fonts; diagnose unsupported requests. |
| Component information | `type`/`notes` render; gauge/length, subtype, product data and additional-component rows do not. | Preserve all source now. Add literal metadata rows deliberately; do not claim BOM/report completeness. |
| Source editor/canvas | Actual terminal geometry, path highlighting, label/color changes, swatches, source split, zoom/pan/Fit and host undo/guarded saves exist. | Keep these owners. Connection creation/removal and generated-instance edit scopes remain additional work. |
| External/custom output | Images and Graphviz tweaks are omitted with warnings; notes/labels stay escaped literal text. | Retain the resource boundary. HTML/CSS templates or arbitrary DOT are not needed for the proposed phase. |

Local evidence: [model](../../crates/little-clip-core/src/diagram/wireviz.rs),
[attributes](../../crates/little-clip-core/src/diagram/wireviz/attributes.rs),
[connections](../../crates/little-clip-core/src/diagram/wireviz/connections.rs),
[renderer](../../crates/little-clip-render/src/diagram/wireviz.rs),
[transactions](../../crates/little-clip-core/src/diagram/wireviz/edits.rs),
[canvas](../../crates/clip-studios/src/text_editor_wireviz/canvas.rs).
Many presentation properties are currently silently absent because the model
does not read them; successful parsing alone does not establish visual parity.

## Canonical details that matter

The [syntax document](https://github.com/wireviz/WireViz/blob/5f961acd4c1ddfa509b0eb02ef4bfed7a510b7c7/docs/syntax.md)
separates reusable component definitions, graph presentation and HTML reports.
It documents `template_separator`, named/anonymous instantiation, visibility
controls, component/global backgrounds and report metadata.

The [data classes](https://github.com/wireviz/WireViz/blob/5f961acd4c1ddfa509b0eb02ef4bfed7a510b7c7/src/wireviz/DataClasses.py)
establish precedence/defaults: explicit nonempty colors win over `color_code`;
otherwise a selected palette repeats/trims to `wirecount`. Palette-only input
does not infer a conductor count. Simple connectors hide name/count by default;
bundles hide wire numbers; generated designators are normally hidden.

The [color catalog](https://github.com/wireviz/WireViz/blob/5f961acd4c1ddfa509b0eb02ef4bfed7a510b7c7/src/wireviz/wv_colors.py)
contains finite `DIN` (60), `IEC` (10), `BW` (2), `TEL` (50), `TELALT` (50),
`T568A` (8) and `T568B` (8) sequences. TELALT includes first-pair-group solid
conductors; it is not just TEL with every striped pair reversed. Treat these as
format data verified against standards/fixtures, rather than copying GPL Python
implementation into the Rust application.

The [canonical graph builder](https://github.com/wireviz/WireViz/blob/5f961acd4c1ddfa509b0eb02ef4bfed7a510b7c7/src/wireviz/Harness.py)
uses presentation flags independently from connectivity. Pin hiding preserves
loop/connected terminals; a simple connector has no ordinary pin table, and a
bundle has a dashed outline. Additional-component/product/gauge/length content
also affects visible rows. Those omissions should remain explicit until added.

## Template semantics and source origins

The [canonical resolver](https://github.com/wireviz/WireViz/blob/5f961acd4c1ddfa509b0eb02ef4bfed7a510b7c7/src/wireviz/wireviz.py)
binds instances in connection order. `Y.Y1` creates/reuses named instance Y1 from
definition Y; a later bare Y1 reuses that binding. `Z.` creates anonymous
instances per expanded lane when a bare entry is broadcast. A mapping selecting
multiple pins on one named instance creates one component, not a component per
pin. Internal anonymous names are not stable author-facing identities.

Current normalization eagerly gives every definition its own component name.
Current `Plan.item` then treats that name as a literal YAML mapping key. Merely
cloning a connector named Y1 would make an edit incorrectly target
`connectors.Y1`, even when only `connectors.Y` exists. Keep definitions separate
from evaluated instances and retain origin records for definition key,
instantiation/reference sites, set/step/lane and anonymity. Apply component,
terminal, link and expanded-string budgets during generation, not afterward.

The default separator introduces another compatibility hazard: current native
exact-name lookup permits literal dots in designators, while canonical splitting
uses the decoded string. YAML quoting does not escape the separator. Diagnose
ambiguous literal names or require an explicit different separator; never
silently reconnect an existing native harness by changing token precedence.

For the first template-rendering phase, generated components should be visibly
read-only in the inspector with a source/template explanation. Template-wide
editing and instance-local materialization are different transactions. A later
instance-local edit must create a deliberate explicit definition and rewrite all
affected creation/reuse sites atomically, while checking the complete expected
YAML value and circuit. Never persist internal anonymous counters as identities.

## Preservation requirements

- Retain comments, BOM/CRLF, quoting, block/flow forms, numeric-vs-string pin IDs,
  unknown fields, anchors/merges and unrelated bytes. Failed edits publish no
  source and never write a file.
- Continue immutable revision checks before planning and at worker completion,
  host undo ownership, guarded saves and accepted-write completion behavior.
- A palette-derived color is a resolved value, not an authored `colors` item.
  An individual color edit must materialize the full resolved list, preserving
  every other conductor and its connection references. Removing/changing
  `color_code` is an explicit semantic delta, not incidental cleanup.
- Do not edit a shared anchor/template through a selected occurrence. Preserve
  the existing alias/merge refusal and independent whole-document semantic guard.
- Visibility and palette selection change presentation; they must not change
  physical terminal IDs, normalized links, loops or clickable-source identity.

## Approaches and bounded delivery

[Wirebug](https://github.com/LHelge/wirebug) has a native harness renderer and
MIT/Apache-2.0 package licensing, but consumes its own `.wb` DSL and includes
server/LSP/PDF/raster dependencies. It does not supply WireViz YAML/templates.
[WireViz-Web](https://github.com/wireviz/wireviz-web) is a Flask wrapper around the
canonical GPL engine, not an independent native parser. Neither justifies
replacing the working local model/Graphviz mechanism for these finite gaps.

The independently maintained [WireViz Online editor](https://www.wireviz.app/docs/overview/)
is useful interaction research: pin-to-wire gestures, component/property panels,
shared YAML undo and a separate canonical preview. It is a browser application,
not a runtime dependency or authority for this renderer's compatibility.

Recommended delivery order:

1. Add count inference, finite color schemes and typed presentation defaults to
   the current model; reuse `colors::bands` and existing Graphviz port IDs.
2. Add geometry/color assertions for every palette boundary/repetition, explicit
   override, hidden-but-connected/looped pins, simple connectors and bundles.
   Verify source bytes remain unchanged by previews and unrelated edits.
3. Introduce origin-aware instance normalization separately. Test shared named
   instances, anonymous per-lane generation, separator/collision failures and
   post-generation budgets before enabling any instance edit gesture.
4. Add connect/disconnect as source-planned transactions using link provenance;
   leave HTML report templates, BOM export, linked artwork and arbitrary tweaks
   to explicit later work.

No general template execution, browser, new persistence owner or rendering
backend is required for steps 1–3.

## Bounded implementation checkpoint

The native model now infers connector counts from pin colors, resolves all seven
finite palettes, and preserves canonical explicit-color precedence. All 60 DIN
positions were independently checked against [LAPP's published color chart](https://t3.lappcdn.com/fileadmin/DAM/Lapp_USA/Technical_Documentation/Cables/Color_Chart_8_9.pdf);
all scheme sequences/order were compared with the pinned canonical format data.

The renderer now honors name/count/wire-number visibility, compact simple
connectors, hidden disconnected pins with connected/looped terminals retained,
dashed bundles, and safe component/global backgrounds. It preserves the existing
readable native styling when no background is authored. Physical pin indices and
semantic terminal groups remain stable when rows are hidden; compact connectors
retain their terminal hit geometry.

Palette-derived single-wire edits materialize the resolved colors through the
existing transaction. The original `color_code`, comments and unrelated source
remain intact; explicit colors then take precedence. Tests verify unchanged
electrical links. Fonts, color-name text formatting and additional-component/BOM
metadata have visible omission diagnostics rather than claims of full visual
compatibility. Template instances remain explicit errors pending origin-aware
normalization and editing.

Validation: 29 core WireViz model/edit tests and four renderer tests passed.
Strict library/test Clippy passed for core and render. A review fixture is
[harness-presentation.wireviz](../examples/diagrams/harness-presentation.wireviz).
No application binaries were rebuilt in this checkpoint; the earlier GUI
acceptance screenshots exercised frozen binaries from before these changes.


## Source-preserving wiring transactions

The native model and studio now implement bounded wiring transactions. The frozen
GUI binaries used in the current acceptance run do not include this patch.
Connect creates a new connection set
using typed pin IDs, wire numbers or the shield selector, leaving existing sets
untouched. A reversed duplicate direct connection is a no-op. Disconnect targets
one immutable link index and its exact set/step/lane or loop-pair origin, retaining
other duplicate occurrences.

For a parallel connection set, only the selected lane is removed from existing
selector items. A numeric range is trimmed/split at its own scalar item. New
prefix/suffix paths preserve every surviving electrical relationship and include
singletons so components remain visible. For a single-lane set, the suffix moves
as actual oneyaml source nodes with its authored spelling and comments. Existing
connection paths and document subtrees are never reserialized.

Before returning source, the transaction proves the complete expected YAML value,
comment multiset, every non-link harness field and the multiset of electrical
links/annotations. Link origins may change after lane decomposition; endpoint
identities and link multiplicities may not. Shared-anchor side effects, inherited
or aliased writable collections, lost comments, unsupported source moves and
budget failures refuse the edit without publishing source.

Native controls use primary-button terminal-to-terminal drag and explicit
Disconnect buttons beside direct incident links in the inspector. Selection
continues to address stable terminal IDs. Outside/incompatible release, Escape,
middle-button pan, suspension and changed source/model/frame cancel the drag.
The existing worker, host undo, guarded save and close lifecycle own publication;
a duplicate connect does not create an undo update. Edge bounding rectangles do
not select connections.

Mating annotations remain visibly distinct and source-edit-only. Removing an
electrical link from a set containing whole-connector mating is also refused
until a transaction can prove preservation of its component-level meaning.
Template instance expansion remains a separate follow-up.

The review fixture is [harness-wiring.wireviz](../examples/diagrams/harness-wiring.wireviz).
Validation: 44 core WireViz tests, nine studio WireViz tests and five renderer
WireViz tests passed. Strict library Clippy passed for core, render and studios.
The new renderer test applies a parallel disconnect plus a shield connection and
verifies native SVG terminal hit groups. Studio tests exercise primary-button
press/release, outside/incompatible cancellation and changed model/source. No
application binary rebuild or native GUI acceptance was performed for this patch.


### Native wiring qualification

Both rebuilt native scenarios pass with the same final script:
`/tmp/clip-wireviz-wiring-dark-20261113-run5/report.json` and
`/tmp/clip-wireviz-wiring-light-20261113-run2/report.json`. Each captures16
observations plus saved-source/proof artifacts. Actual held connection pixels
appear before release without premature writes; Save changes link count8→9,
Disconnect9→8, and a middle parallel-lane cut8→7. Duplicate links, loops, other
lanes, opaque values and singleton visibility remain exact. Incompatible,
Escape and stale-held cancellation pass; every Undo/Save restores exact bytes.
Earlier pending-GUI notes are superseded; mating and template limits remain.
