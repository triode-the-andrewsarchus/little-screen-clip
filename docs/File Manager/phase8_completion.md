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
- [x] C4. Bounded live project/TODO views, source-backed checkboxes, refresh/error
      handling and project-area membership from the registry.
- [x] C5. Session red/yellow/green marks with editable meanings; distinct labeled
      underlines, persistence and clearing/recovery.
- [x] C6. Configurable A/B/... sets, membership/filtering and repeated subdivision;
      independent from session state and visibility.
- [x] C7. `.hide`/`.show` syntax, precedence/inheritance, selective dotfile reveal,
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
  are checked against the real files and registry. Light workflow also passed all 19 observations:
  `/tmp/phase8-project-light-05/report.json`. Both themes were visually inspected.
  Earlier failures were OCR target/label issues,
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

### C4 live project/TODO areas

Add → Live Projects area creates a movable/resizable/renameable desk area whose
membership follows the registry. It shows visible project rows, four source-backed
Markdown task checkboxes per row, refresh and an ordinary guarded Open TODO
handoff. Document content and checked state remain solely in each project folder;
layout undo/redo only changes the desk. Browse enables checkboxes; Arrange keeps
them inert. Live-widget bounds own wheel interaction instead of also zooming the
canvas through a scrolling task area.

CommonMark task-list parsing reuses pulldown-cmark (already used by document
rendering), including byte offsets, nested/inline formatting and fenced-code
exclusion. A toggle changes one source byte via FileSource atomic replacement,
retaining project/root/document parents and rechecking manifest/source identity.
Extensionless TODO works through the same manifest path. New sources, replaced
parents, missing payloads, non-UTF-8/oversized documents and changed project IDs
fail explicitly instead of overwriting/recreating content.

Task reads use a separate two-worker queue, at most two pending reads and 24
retained snapshots. Only visible rows request work; hidden reads retire and
accepted writes survive retirement with unbounded delivery ownership. Visible
areas refresh every two seconds, keeping prior content during refresh. Errors
keep stale checkboxes disabled. Save failures remain visible in Home recovery
even if the live area was removed, with explicit Reload task previews.

- Focused Home suite: 36 passed (`/tmp/phase8-project-live-tests.log`), including
  one-byte CRLF/UTF-8 edits, nested tasks/code exclusion, external conflicts,
  changed project IDs, extensionless TODO, parent replacement, bounded retention,
  hidden-view accepted writes and error visibility without any live area.
- Final full file-manager suite: 995 passed, zero failed, 6 ignored
  (`/tmp/phase8-project-live-final-all-tests.log`).
- Drawing suite: 18 passed (`/tmp/phase8-project-live-drawing-tests.log`).
- Strict combined file-manager/studios Clippy and workspace formatting passed:
  `/tmp/phase8-project-live-{clippy,format}.log`.
- Native final workflows passed all 20 observations in both themes:
  `/tmp/phase8-project-tasks-final-dark-02/report.json` and
  `/tmp/phase8-project-tasks-final-light/report.json`. They click actual checkboxes,
  verify exact source bytes, undo/redo the layout without reverting tasks, force
  a real write-lock failure and recover, refresh external edits, remove/recreate
  TODO, open it in the studio and unregister without deleting files. Live areas,
  disabled/error states and both appearances were visually inspected. Prior OCR
  failures are retained in earlier reports; narrow contrast crops identify
  bordered controls and the red failure text without changing production styles.
- Architecture comparison has no new excessive-complexity function. The fourth
  short additional clone group is in Cargo dependency declarations after adding
  the existing CommonMark dependency directly to file manager, not a duplicate
  parser or runtime mechanism. Average/max complexity stay unchanged and overall
  duplication percentage is down 0.03 points from the original baseline.

C4 is complete. C5–C9 remain open; the full Phase 8 goal is not complete.

### Native restart input harness follow-up

- [x] Prime the private virtual keyboard after restarting a client. A small
  close/restart probe showed the second identical Ctrl+Q chord never reached the
  restarted application's raw input, while the focused application was idle and
  had no pending writes. A modifier-only keymap round trip restores delivery
  without typing text, cancelling a load or firing a command. Failing probe:
  `/tmp/phase8-close-probe/report.json`; passing probe:
  `/tmp/phase8-close-probe-modifier/report.json`. Temporary close/input tracing
  was removed from the application. This changes test setup, not production
  keyboard or application close behavior.


### C5 session marks

The existing view toolbar now contains Session controls. Red/Yellow/Green marks
use labeled underlines without increasing row height; List, Columns, ancestor
columns and Photos share one immutable index and renderer. Name and color
meanings are editable, with explicit save/discard and draft-safe application
close. Session disable retains assignments. Clear retains one recovery set;
Restore and Forget validate the observed recovery revision against concurrent
windows. Assignments describe the captured metadata revision, while unavailable
or changed references remain stored and dormant. The complete contract is in
`organization.md`.

The existing local-state worker handles semantic transactions, cooperating-window
merges and pinned atomic replacement. Different definition fields merge;
conflicting edits to the same label fail. Accepted writes retain completion
ownership and block close. Unknown/malformed files are preserved. No source
content is read or written to assign a mark.

- Full file-manager tests: 1,000 passed, zero failed, 6 ignored
  (`/tmp/phase8-session-all-tests.log`). Tests cover lossless path storage,
  revision matching, disable/clear/recovery, stale recovery rejection, concurrent
  assignment/definition merging, failed same-field edits, restart, close ownership
  and malformed/future-version preservation.
- Strict file-manager Clippy, debug build and workspace formatting passed:
  `/tmp/phase8-session-{clippy,build,format}.log`.
- Native acceptance passes 32 observations in each appearance:
  `/tmp/phase8-session-complete-dark/report.json` and
  `/tmp/phase8-session-complete-light/report.json`. Independent pixel checks
  confirm all three painted underlines, including Columns ancestors. Real saved
  state is checked through clear/restore, custom meanings and restart. An invalid
  label survives a close request from a saved text studio, returns to a visible
  browser error, and can be explicitly discarded without replacing saved labels.
  Fixtures are checked byte-for-byte unchanged. Both themes and error presentation
  were visually inspected.
- Architecture comparison retains average/max complexity and reduces overall
  duplication by 0.04 percentage points from the completion baseline. The
  aggregate report flags eight additional short clone groups across C1–C5.
  C5's reported matches are shared job-owner setup/result handling, six lines of
  ordinary absolute-reference validation and independent model test setup.
  These do not duplicate the persistence transport or annotation renderer; a
  generic controller framework would add more indirection than these small
  domain-specific sequences. No C5 function introduces excessive complexity.

C5 is complete. C6–C9 remain open; Phase 8 is not complete.

### C6 sorting sets — implementation checkpoint, acceptance pending

Configurable A/B groups now have stable IDs, editable names/colors and nested
subdivisions. Parent filters include all descendant assignments. Unassigned and
All files filters remain available; folders stay visible for navigation.
Classification does not modify file content. Membership badges are separate from
session underlines, and session clear/disable does not change sets. Ordinary
folder previews and keyboard selection share the file filter. Bounded pickers
support the complete 256-group budget. Inline group drafts must be saved or
discarded before closing and are not lost when assigning files.

Organization v1 migrates to v2 on an accepted change, retaining session data and
adding initial A/B groups. The existing guarded worker merges assignments and
rejects stale group edits; deletion checks current assignments/children before
removing an empty group. Bounds, path-reference semantics and hierarchy behavior
are documented in `organization.md`.

- Targeted organization tests: 11 passed
  (`/tmp/phase8-sets-targeted-tests.log`). Covers hierarchy and ancestor filters,
  unassigned files, missing parents, cycles/depth limits, names, stale edits,
  lossless non-UTF-8 memberships, v1 migration, concurrent assignments, restart,
  draft ownership and browser selection reconciliation.
- Final full file-manager suite: 1,006 passed, zero failed, 6 ignored
  (`/tmp/phase8-sets-all-tests.log`). Strict file-manager Clippy and debug build
  passed (`/tmp/phase8-sets-{clippy,build}.log`). Native acceptance is still being
  finalized; C6 remains unchecked until both appearances have been inspected.
- The first native attempts exposed OCR failing on highlighted/one-letter filter
  labels and choosing an identically named parent filter instead of the
  assignment submenu. The driver now uses the observed native submenu geometry,
  with saved membership independently asserted after every assignment. A narrow
  popup OCR region preserves readable evidence. These earlier reports remain
  under `/tmp/phase8-sets-*` rather than being treated as passing evidence.
- Architecture comparison has no new excessive-complexity function. Whole-goal
  duplication is down 0.05 percentage points with unchanged average/maximum
  complexity. Aggregate short clone groups are up ten across C1–C6; C6 continues
  to use the same worker, state transaction, path validator and row presentation
  rather than adding another persistence or inventory system.

C6 is in progress. C7–C9 remain open. Phase 8 is not complete.


### C6 native acceptance completed

Both final native workflows passed all 24 observations:
`/tmp/phase8-sets-dark-06/report.json` and
`/tmp/phase8-sets-light-06/report.json`. They assign A/B memberships, filter to A,
create a child Keep group, move the selected file into that child, verify the
parent still includes it, inspect Unassigned, return to All and restart with
stored memberships intact. Source bytes remain unchanged. Badge placement and
both appearances were visually inspected, including size/modified columns.
Earlier input/OCR failures remain in the preceding reports and are not counted
as successful evidence. The final build includes bounded shared pickers and
ancestor-column badges. Full suite: 1,006 passed, 6 ignored; strict Clippy/build
passed as recorded above.

C6 is complete. C7–C9 remain open. Phase 8 is not complete.

### C7 visibility rules — implementation checkpoint, native acceptance pending

The versioned rule contract is documented in `visibility_rules.md`: TOML version
1, bounded globs/negation, deeper-directory precedence, `.show` priority at equal
depth, inherited directory opinions, selective dotfiles and mount-scoped ancestry.
The guarded loader pins directory parents, verifies directory object/mount identity,
rejects symlink/nonregular rule sources, checks same-handle source revisions and
never rewrites unknown/malformed input. A separate bounded read worker owns root
collection. Existing folder-inventory workers use the same loader for local
preview overrides; source errors remain attached to the retained inventory.

Rules: on/off, source opening, reload and one-click reveal-all recovery use the
existing toolbar. Root rows use solid tinted backgrounds and explicit source
labels. Session underlines and set badges remain separate. Ancestor columns keep
active-route entries available. Collection failure clears previous hide opinions
and exposes normal visibility with an explicit error. Details remain temporary
viewing state, independent from file permissions and source content.

- Full file-manager tests: 1,013 passed, zero failed, 6 ignored
  (`/tmp/phase8-rules-all-tests.log`). New cases cover real inherited sources,
  selective dotfiles, negation/precedence/anchoring/directory matches, malformed
  and future schemas, source aliases/types/limits, replaced directory identities,
  non-UTF-8 Unix names, an ordinary `/dev/shm` mount beneath a protected parent,
  and agreement between folder cards and keyboard children during recovery.
- Strict Clippy and debug build passed (`/tmp/phase8-rules-{clippy,build}.log`).
  The earlier larger archive-view enum was avoided by retaining each preview's
  rule success/failure together in one shared immutable result.
- Native checks are still running in both appearances. The first captures
  demonstrated working selective dotfiles and child-folder overrides, and exposed
  a cramped Rules menu. Its minimum width is now explicit; those initial failed
  reports are retained rather than counted as acceptance.
- Architecture comparison has no new excessive-complexity function after
  separating bounded source collection from held-parent traversal. Overall
  average/maximum complexity is unchanged and duplication is down 0.05 points;
  the aggregate report still flags eleven additional short clone groups across
  C1–C7, including existing job scaffolding and independently asserted fixtures.

C7 remains unchecked pending final native results and review. C8/C9 are still
required before Phase 8 is complete.

C7 acceptance follow-up: recovery captures visibly contained the rule files and
ordinary dotfiles, but sparse OCR omitted leading dots. Native assertions now use
unambiguous basename fragments for those rows, while parser/preview tests retain
exact path checks. Independent pixel counts also verify the actual solid Shown
and ignored-Hidden backgrounds, rather than inferring them from labels alone.
Final reports are still pending under `/tmp/phase8-rules-*-03/`.

C8 implementation entry point: `FileManager::follow_rename` already receives
confirmed naming results, completed move pairs (including partial batch reports)
and studio rename handoffs. It currently updates browser history/selection and
pending navigation only. Home pins, project registry and organization persistence
still require accepted, queued semantic retargeting through their existing owners;
a read-style refresh must not replace those writes or drop events while busy.
Copy results must not retarget references. Home also remains a separate open flag,
so virtual history/startup integration still needs implementation and tests.


### C7 native acceptance completed

Both final native workflows passed all ten observations:
`/tmp/phase8-rules-dark-04/report.json` and
`/tmp/phase8-rules-light-04/report.json`. They verify selective dotfiles, normal
hidden-file exclusion, labeled solid row colors using independent pixel counts,
reveal-all recovery including the rule files, restart with policy restored,
future-version preservation and visible fail-open error, source repair/refresh,
and child-folder local overrides. Source bytes remain unchanged after repair.
Both themes, the error presentation and child-folder previews were visually
inspected. Filename OCR uses basename fragments where leading dots are omitted;
unit tests check exact paths and source bytes. Prior failed captures remain as
recorded and do not count toward acceptance. Formatting also passed
(`/tmp/phase8-rules-format.log`).

C7 is complete. C8/C9 remain required before Phase 8 is complete.

### C8 Home history/startup delivery

Home now has an explicit virtual history kind with a retained real folder context.
No fabricated path enters filesystem navigation. Folder and Home surfaces share
Back/Forward controls. Keyboard history, Escape and Back to folder preserve folder
selection/scroll and forward visits. Reopening the current Home is idempotent;
opening a shortcut creates a normal visit even for the same contextual folder.
No-argument startup opens the personal Home; an explicit directory argument opens
that directory. The desk remains one persistent canvas rather than a separate
copy per visit. The contract is in `home_navigation.md`.

A Home visit can retain context A while the cache belongs to later folder B.
Returning now keeps Home visible until the guarded A read is accepted. Failed
navigation leaves Home visible with its error instead of exposing B under A's
address. Accepted desk writes retain their owners and unapplied text still blocks
leaving. The existing UI regression now waits for that accepted folder read before
checking resumed keyboard selection.

- Full file-manager tests: 1,017 passed, zero failed, 6 ignored
  (`/tmp/phase8-home-history-tests.log`). Four new regressions cover virtual
  visits, selection/scroll/forward history, startup context, same-folder visits,
  and a missing return folder with an unrelated cache, including the pending
  transition before failure.
- Strict Clippy, build and workspace formatting passed:
  `/tmp/phase8-home-history-{clippy,build,format}.log`.
- Native acceptance passed all 12 observations in each appearance:
  `/tmp/phase8-home-history-dark-03/report.json` and
  `/tmp/phase8-home-history-light-03/report.json`. Back/Forward, Escape, the
  Back to folder button, no-argument restart and real-home context navigation
  are observed in a private environment. Both appearances were visually reviewed.
  Narrow header OCR avoids bordered-control omissions; Home is identified by
  its actual empty-desk content and navigation controls. Earlier failed captures
  remain available rather than being counted as successful evidence.
- Shared history-button presentation is reused by both surfaces, removing new
  header complexity. No separate navigation controller, canvas or decoder cache
  was introduced.

The history/startup part of C8 is delivered. Confirmed rename/move retargeting of
Home pins, organization and project references remains outstanding, as does C9.
Phase 8 is not complete.

### C8 exact file-path relocation fix

- [x] Preserve exact destination bytes when retargeting a file. The shared
  `destination.join(empty_suffix)` added a trailing slash, which Path equality
  concealed but ordinary file opening rejected. The regression checks raw path
  bytes, opens the target and checks a similarly prefixed sibling stays unchanged.
  It failed before the fix (`/tmp/phase8-relocation-path-before.log`) and passes
  afterward (`/tmp/phase8-relocation-path-after.log`). This fix is committed
  separately from the reference-update feature.
