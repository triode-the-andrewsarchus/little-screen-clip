# Native Mermaid flowchart editing

Flowcharts now use an interactive vector canvas backed by the existing Merman
parser and SVG renderer. Select rendered nodes or edges, edit ordinary labels,
add box/rounded/decision nodes, and connect a source node to another node.
Layout remains source-driven and automatic; the editor does not invent stored
coordinates for Mermaid.

Edits patch parser-proven original byte ranges, then reparse and verify the
semantic change before publication. Comments, styles, front matter, unknown
syntax and exact undo bytes remain owned by the shared text session. The same
accepted-edit ordering governs Save, Undo and closing. Canvas selection uses
actual semantic IDs in outlined SVG geometry, not guessed ordinal IDs.

Deletion is available only when reparse proves that it preserves other content.
Connected nodes, compound declarations, ambiguous parallel relationships,
indexed-style retargeting and rich/shared labels remain source-only. Other
Mermaid families retain their highlighted source/live preview workflow.

Core diagram tests (66), studio editor tests (33), preview revision tests (4),
syntax registration, both application workers and strict all-target Clippy pass.
The private native acceptance script exercises selection, edits, connections,
exact Undo/Save and unsupported-family source editing; actual light/dark GUI
execution remains pending at this checkpoint.

## Native review follow-up

- [x] Retain semantic hit geometry when canonical masks/filters require raster
  painting. The first actual styled-flowchart review exposed missing selection;
  geometry now comes from the existing bounded outlined tree independently of
  native paint support. Canonical effects remain visible. Three shared SVG tests
  and four studio preview tests pass, including masked hit-bound regressions.
- [x] Repeat the actual light/dark editor scenario after rebuilding this fix.
  Both native reports pass: `/tmp/little-clip-mermaid-dark-20261113-07/report.json`
  and `/tmp/little-clip-mermaid-light-20261113-06/report.json`, including real
  node/edge interaction and exact Undo/Save.
- [x] Keep an explicit Fit request pending while previous-source geometry is
  still displayed. The existing camera flag is consumed only by a fresh frame;
  a stale-to-fresh egui regression passes in the shared editor suite.
- [x] Repeat one-shot Fit in both rebuilt native themes: dark `final01` and
  light `final02` under `/tmp/little-clip-mermaid-*-20261113-*/report.json` pass
  all18 scenario screenshots without retrying Fit.

The freshness condition raises the existing canvas CCN from 15 to 16. It is the
required stale/current-source distinction, without a new state owner or
metric-only helper.
