# Phase 8 completion — personal Home and organization

Active objective: complete the full Phase 8 scope, including the user's expanded
Home direction. The earlier static-pin delivery is a foundation, not completion.
Work stays on `feat/file-manager-phase8`; personal Cargo/runtime configuration
changes remain outside commits. Baseline: `before-phase8-completion`.

## Experience

Home is a personal, zoomable desk. Users draw, write, arrange and link their own
landmarks; hand-drawn artwork can become a shortcut. Live file/project cards sit
among those drawings rather than forcing a dashboard grid. Browse and Arrange
modes distinguish opening/checking things from drawing/moving things. No floating
property windows, browser engine, or rasterized-at-one-zoom drawing layer.

Reuse the existing native drawing studio, vector renderer and canvas camera.
Keep artwork, local presentation and target references distinct. Support pan,
zoom, fit, drawing/text/shapes, selection, resize, undo/redo and persistent layout.
Named sections frame areas of the desk. Provide recoverable defaults, keyboard
access and a readable appearance in both themes. Existing pins migrate losslessly.

Projects stay in their own ordinary folders. Home contains references and live
views; it never becomes the owner of project files. A Projects area can reflect
registered projects as they are added/removed, with task excerpts backed by each
project's own TODO document. Checking a task edits that source document through
identity-checked atomic replacement; failures remain visible, external edits are
not overwritten, and unchecked/checked state is not duplicated in Home JSON.
This specific registry-backed area does not introduce the Phase 10 query engine.

## Delivery ledger

- [x] C1. Expose the existing native drawing surface for host-managed live overlays
      and clickable artwork without copying its renderer/editor.
- [x] C2. Home canvas: migration, free arrangement, artwork shortcuts, named
      sections, live card placement, view state, undo/redo and deliberate editing.
- [x] C3. Versioned `.fmproject`, safe create/register workflow, no-overwrite
      README/TODO defaults and local global project registry.
- [ ] C4. Bounded live project/TODO views, source-backed checkboxes, refresh/error
      handling and project-area membership from the registry.
- [ ] C5. Session red/yellow/green marks with editable meanings; distinct labeled
      underlines, persistence and clearing/recovery.
- [ ] C6. Configurable A/B/... sets, membership/filtering and repeated subdivision;
      independent from session state and visibility.
- [ ] C7. `.hide`/`.show` syntax, precedence/inheritance, selective dotfile reveal,
      labeled solid backgrounds and an obvious recovery/ignore-rules control.
- [ ] C8. Home browser history/startup behavior; defined/tested rename/move/restart
      handling for pins, marks, sets and project identities.
- [ ] C9. Realistic review fixtures, native interaction/visual verification in both
      themes, affected tests plus workspace gate, strict Clippy, formatting,
      architecture review and logical commits as each delivery becomes usable.

## Storage and mutation contracts

Local Home, organization and registry state use separate versioned models over
the shared guarded `local_state` transport/worker. Preserve unknown/newer input
and report failures; migrate old Home pins without touching source content.
Cooperating windows merge semantic changes, never replace another window's newer
state with a stale snapshot. Layout/drawing writes must retain their completion
owner and defer close. Bound canvas size/elements/points, source documents,
visible project reads, worker admission and retained previews.

Project manifests are portable project-owned metadata with stable UUID identity,
a readable title and relative README/TODO references confined to the project.
The local registry maps that identity to observed folder references; project
content and tasks remain there. Do not infer original identity from an old path,
follow unvalidated relationship targets or overwrite existing files on creation.
Browser-managed moves/renames update local references only after confirmed
success; external moves need explicit re-registration or report unavailable.

Visibility rules are project/folder-owned, not Home state. Specify exact matching,
negation/precedence, ancestor scope, bounds and errors before enabling them.
Rule recovery must always be accessible even if all normal items are hidden.
Session/sets must remain separate models and visual signals from these rules.

## Completion audit

Each ledger item needs committed implementation plus requirement-specific tests
or native evidence. C2/C4 need observed live interactions, not static screenshots
alone. Verify external TODO edits, write failures, stale identities, restart,
rename/move, malformed/newer persistence, no-overwrite creation and missing
mounts/targets. The completed goal is the whole scope above, not only Home polish.

## Evidence and ongoing work

C1 exposes `clip_studios::drawing_canvas::Canvas`, reusing the existing native
editor/renderer/camera. Browse clicks return artwork IDs; host-owned live widgets
reserve their world bounds so underlying artwork cannot capture their clicks.
Overlay coordinates use the same camera through zoom, pan and Fit; live bounds
participate in Fit without fake drawing elements. Hosts own source history/saves,
and can poll accepted edits while the canvas is hidden. No new renderer, browser
engine, GL context or preview cache was introduced.

- `cargo test -p clip-studios --lib -j4 -- --test-threads=1`: 845 passed,
  zero failed, 12 ignored; includes existing native editor regressions and new
  artwork activation/overlay-coordinate/hit-exclusion tests.
  Log: `/tmp/phase8-canvas-studios-tests.log`.
- `cargo clippy -p clip-studios --all-targets -j4 -- -D warnings`: passed.
  Log: `/tmp/phase8-canvas-clippy.log`.
- The API has Rust documentation for interaction and host source/save ownership.

C2 now has committed board migration, stale-layout write protection and an
interactive Home presentation. The evidence below records those deliveries.

C2 storage foundation: Home v2 adds bounded native artwork, movable card/section
records and artwork-to-pin bindings. Legacy v1 pins migrate without changing pin
IDs or target paths. Layout writes compare the board revision under the shared
writer lock; a stale window gets an explicit failure and the latest file remains
byte-for-byte unchanged. Pin removal retires its cards/bindings without touching
the referenced content. Deleting artwork retires dangling shape bindings.

The local-state transaction callback can now reject a semantic change before
installation. Each schema supplies its byte budget: folder preferences retain
1 MiB; Home v2 allows 2 MiB so near-limit valid v1 pins have room for their migrated
layout. Artwork itself is limited to 384 KiB, 1,024 live cards and 2,048 bindings;
core drawing validation retains its existing element/geometry budgets. Invalid
geometry, unknown future schemas and oversized input remain explicit failures.

- `cargo test -p clip-file-manager --lib -j4 -- --test-threads=1`: 971 passed,
  zero failed, 6 ignored, including migration near the old byte limit, stale
  layout rejection and existing folder-preference safety tests.
  Log: `/tmp/phase8-board-model-tests.log`.
- `cargo clippy -p clip-file-manager --all-targets -j4 -- -D warnings`: passed.
  Log: `/tmp/phase8-board-clippy.log`. Owned formatting and diff checks pass.
- Architecture comparison reports no new excessive-complexity function after
  separating card geometry/content validation from board identity/reference
  validation. The aggregate result still flags one additional short test clone;
  old controller/renderer matches are also repeated in the diff-aware report.
  The new fixture deliberately asserts hit/mapping behavior independently of
  the original drawing tests. There is no second renderer or persistence path.

The interactive presentation, persisted camera and leave/close behavior are
delivered below. Live project task content remains C3/C4 work.

### C1 follow-up: same-frame pan alignment

- [x] Fixed native artwork lagging live overlays during primary-button pan.
  A rendered-frame regression first demonstrated a 40×30-pixel mismatch.
  Artwork projection is now computed after pan/Fit input, matching feedback
  and live-widget projection in that same frame. Middle-button Fit also updates
  before painting. The drawing regression suite passes: 17 tests, zero failures.
  Before-fix evidence: `/tmp/phase8-pan-regression-before.log`;
  current suite: `/tmp/phase8-desk-drawing-tests.log`.

### C2 interactive desk delivery

The static pin list is replaced by the shared native canvas and host-owned live
cards. Browse opens guarded file/folder references or linked artwork. Arrange
supports native pencil/text/shapes, movable/resizable cards, named tinted sections,
editable labels/title and 24-step layout/source undo/redo. Sections leave their
interiors available for drawing. Card and artwork selection are mutually exclusive.
The host polls accepted drawing edits even while hidden, debounces completed
layout edits and retains newer drafts across earlier save acknowledgments.
Reference removal prunes history so undo cannot resurrect an orphan target.
Rejected writes retain the draft with explicit retry/discard controls.

- Native dark-theme acceptance passed 28 observations, including resize,
  move/undo/redo, section-body drawing, artwork binding, restart, clicking real
  painted ink to open its file, studio return, folder opening and non-destructive
  unpin. Report: `/tmp/phase8-desk-selection-dark-02/report.json`.
- Light-theme acceptance passed the same 28-observation workflow with the final
  mutual-selection correction: `/tmp/phase8-desk-selection-light/report.json`.
  Both rendered desks were visually inspected. OCR needs
  `TESSDATA_PREFIX=/tmp/clip-video-ocr/tessdata` in this environment.
- Targeted Home tests: 20 passed (`/tmp/phase8-home-canvas-tests.log`).
- Strict combined file-manager/studios Clippy passed
  (`/tmp/phase8-desk-clippy.log`); owned formatting and diff checks pass.
- Architecture comparison has no new excessive-complexity functions. Its
  aggregate result still flags three additional clone groups; the reported
  matches include independent rendered test fixtures, existing controller/job
  transport scaffolding, and symmetric undo/redo state transitions. The native
  renderer, camera and guarded persistence transport remain shared. Overall
  duplication percentage decreased by 0.02 points; maximum complexity is unchanged.

At this delivery point camera persistence and draft-close handling were pending;
the follow-up deliveries below resolve them. Live projects/tasks and the other
organization/history work still remain before Phase 8 is complete.

### C2 persisted camera

Home stores finite world-center/zoom state, restores it independently of window
size and keeps it out of layout undo/redo. Older boards without camera state
still Fit normally. Invalid camera data is rejected rather than silently applied.
The existing shared camera owns mapping and validation; Home does not introduce
a second transform. Home tests pass (21), shared camera tests pass (4), and strict
combined Clippy passes. Logs: `/tmp/phase8-view-{tests,camera-tests,clippy}.log`.
Native acceptance now projects observed ink through the saved camera and asserts
that restart leaves that camera unchanged. Draft-close behavior is delivered
in the following fix.

### C2 leave/close draft ownership

Leaving Home or toggling Browse refuses to discard unapplied drawing text.
An inline notice offers Keep editing and Discard unapplied text; the existing
Apply text control submits it through the drawing transaction owner. Application
close checks the same condition and returns to Home when a decision is needed.
Unchanged selected text does not block leaving. Discard never cancels an accepted
edit. Title and card-label fields are explicitly committed before closing, so a
still-focused input does not rely on a later focus-loss frame to become durable.

- Home tests: 22 passed (`/tmp/phase8-drafts-home-tests.log`).
- Drawing tests: 18 passed (`/tmp/phase8-drafts-studios-tests.log`).
- Close persistence tests: 2 passed (`/tmp/phase8-drafts-close-tests.log`).
- Strict combined Clippy and workspace formatting passed
  (`/tmp/phase8-drafts-{clippy,format}.log`).
- Architecture comparison still has no new excessive-complexity function and
  the same three short additional clone groups documented above; shared camera,
  source transaction and persistence owners remain in use.

- Final native workflow passed all 33 observations in both appearances:
  `/tmp/phase8-desk-drafts-dark-03/report.json` and
  `/tmp/phase8-desk-drafts-light/report.json`. The test performs primary-drag pan
  and wheel zoom, checks the saved transform survives restart, verifies actual
  painted artwork remains clickable under that transform, and verifies Ctrl+Q
  leaves an unapplied text draft alive until an explicit discard. Restored desks
  and the inline close notice were visually inspected. Earlier native failures
  exposed OCR label boxes crossing input bounds or matching explanatory prose;
  the driver now restricts those targets to their observed control interiors.

C2 is complete. C3–C9 remain open: this is an editable personal desk with ordinary
file/folder references, not yet the project/TODO or organization delivery.

### C3 project manifests and registry

Home Add now creates a project in the currently browsed ordinary folder or
registers an existing `.fmproject`. Creation has an inline title/path form;
existing README/TODO documents are preserved and only missing defaults are
created. A Projects menu exposes the global local registry, folder opening,
non-destructive unregister and refresh. Copied IDs require an explicit inline
location choice tied to the previously observed folder. The versioned manifest,
relative-reference confinement, bounds, partial-failure behavior and local-state
contract are documented in `projects.md`.

Creation and registry changes run off the UI thread with unbounded accepted
write ownership and participate in the application close gate. The existing
pinned-directory create/write routines and shared local-state transport are
reused. Unknown/malformed metadata, symlinks and unsafe document parents are
rejected without replacing the original input.

- Focused Home tests: 28 passed (`/tmp/phase8-project-tests.log`).
- Full file-manager tests: 987 passed, zero failed, 6 ignored
  (`/tmp/phase8-project-all-tests.log`). New tests verify existing CRLF documents,
  no-overwrite creation, symlink/escape rejection, future manifest/registry
  preservation, registry merge, explicit relocation, restart and accepted-write
  ownership through unregister.
- Strict file-manager Clippy passed (`/tmp/phase8-project-clippy.log`).
- Native dark workflow passed 19 observations:
  `/tmp/phase8-project-dark-04/report.json`. Creation, restart, unregister,
  re-registration, copied-ID conflict, explicit relocation and folder opening
  are checked against the real files and registry. Light-theme forms were
  visually inspected; its full workflow is being rerun with tighter OCR crops
  for bordered/colored buttons. Earlier failures were OCR target/label issues,
  retained in the corresponding `/tmp/phase8-project-*` reports.
- Architecture comparison has no new excessive-complexity function after
  separating project controls from ordinary Home pin controls. The aggregate
  baseline still reports the three previously documented short clone groups;
  project work adds none. Duplication percentage is down 0.03 points overall.

C3 is complete. Live project content/checkboxes are still C4, rather than being
claimed from the project menu or manifest foundation.

### Home shortcut follow-up

- [x] Ctrl+Shift+Z now reaches redo instead of being consumed by plain undo.
  egui logical matching allows extra Shift, so the more specific chord must be
  consumed first. A regression demonstrated the wrong title before the fix and
  passes afterward. Logs: `/tmp/phase8-redo-before.log` and
  `/tmp/phase8-redo-after.log`. The live project native workflow exposed this
  while checking that layout undo leaves independently saved TODO content alone.
