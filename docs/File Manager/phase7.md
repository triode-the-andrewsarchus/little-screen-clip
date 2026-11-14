# Phase 7 — views, container previews and meaningful sorting

Status: implemented on `feat/file-manager-phase7`; ready for user review.
The branch started at `d0298ebd`.
Architecture baseline: `before-phase7-views`. Automatic view selection is
explicitly excluded by the user; existing automatic behavior is preserved.

## Review changes — current requirements

The user's review supersedes the older disclosure-button and separate-preview
checkpoints below. Current implementation and acceptance cover:

- [x] Remove folder/archive +/− controls. Use a full square folder entry target,
  matching other preview icons, and navigate into folders normally from List.
- [x] Reserve strip space for every direct child first. Use only spare space for
  in-line subfolder contents, progressively smaller, at most three preview layers.
  Keep reads/cache/entries bounded and expanded-search/selection provenance valid.
- [x] Keep the complete navigation path reachable in Columns without retaining
  unlimited full-width columns. Reuse the shared List renderer in the rightmost
  column; remove the bespoke adjacent preview pane.
- [x] Give archives a distinct zipped-folder icon and navigate decompressed
  contents through ordinary List/Columns presentation. Keep archive origin and
  lifecycle explicit, with a persistent bottom-bar compressed-archive warning.

### Review verification

- [x] Keep filenames legible in narrow List/Columns rows: optional line/page
  counts yield their horizontal reservation when the name column is narrow.
  Native column inspection and shared List rendering cover this.

- [x] Fix mixed-size SVG icon softness. egui 0.27 caches textures without the
  SVG size hint; icon URIs now distinguish raster sizes. Regression test loads
  16px, 40px and 64px versions in one context and verifies their actual sizes.
  `cargo test -p clip-ui raster_size_tests` passes.
- Folder strips are fixed-height with 64/44/32px inline layers. Direct-child
  capacity is reserved first; complete nested groups must fit spare capacity.
  Targeted container tests pass (36); complete file-manager tests passed
  939 with six ignored before the final review integration.
- Columns now uses a compact full ancestor rail, an optional nearby sibling
  column and the shared rich List. The old separate preview/pane formatter is
  removed. Native review caught narrow-name/count crowding; the count now yields
  space when the name column is narrow.
- Archive browsing uses managed temporary workspaces and the existing validated
  ZIP/TAR/compressed-TAR readers. Bounds: 4,096 members, 32 path levels, 512 MiB
  encoded source, 64 MiB per snapshot and 256 MiB retained history allowance.
  The allowance includes conservative workspace, member and directory overhead.
  Links/special members, unsafe/duplicate paths, prefix collisions, truncation,
  failed checksums and incomplete scans fail without publishing a partial folder.
  The original archive stays unchanged. Browser mutations into snapshots are
  disabled; outgoing drags are Copy-only; text editing requires Save As.
- [x] Native dark/light and narrow/wide review passes. Evidence:
  `/tmp/phase7-container-navigation-dark-04/report.json` and
  `/tmp/phase7-container-navigation-light-01/report.json`. The current driver
  replaces the superseded disclosure tests in
  `scripts/file_manager_phase7_acceptance.py`; it checks square activation,
  member navigation/reveal, archive parent return, complete path access and
  ordinary List content in Columns.
- [x] Workspace tests: 3,283 passed, zero failed, 36 ignored across 47 suites;
  no compiler warnings (`/tmp/phase7-review-workspace-tests.log`). Final affected
  file-manager tests: 942 passed, six ignored
  (`/tmp/phase7-review-fm-final-tests.log`). Shared studios: 841 passed, 12 ignored;
  shared UI: 39 passed (`/tmp/phase7-review-final-tests.log`).
- [x] Strict workspace/all-target Clippy passes
  (`/tmp/phase7-review-workspace-clippy.log`), with final file-manager all-target
  recheck (`/tmp/phase7-review-clippy.log`). Final native build has no warnings
  (`/tmp/phase7-review-build.log`). Changed Rust sources pass `rustfmt --check`;
  the diff passes whitespace checks. Vendor formatting is preserved.
- [x] Architecture reviewed against `before-phase7-container-review-redesign`:
  duplication 7.31% → 7.30%, clone groups unchanged at 4,280, maximum complexity
  unchanged at 274, dependency-cycle count unchanged at 143. The tool still
  labels the comparison a regression for one additional repaint condition and
  one additional archive-context binding in the existing host dispatcher. These
  are required lifecycle/context paths; no new hotspot remains. Shared payload
  copying, shared List rendering and shared file-control exclusion remove
  duplicate mechanisms rather than moving them between files.

## Post-review fixes

- [x] Make Escape return to the folder consistently for every file-backed viewer.
  The host previously handled Escape only on loading/error screens; the newer
  read-only viewers and text edit mode had no common return binding. The host
  now routes unconsumed Escape through the same guarded close as Back to folder.
  Popups, rename/export/save dialogs, filters, tool gestures, fullscreen video
  and unsaved-edit confirmation retain first refusal. Local handlers consume
  the key they handle, so cancelling a nested action cannot also close its owner.
  An idle drawing canvas lets Escape reach the host after cancelling selection.
  Accepted writes/exports remain protected and drafts still require confirmation.
- [x] Verify real host frames across archive, notebook, comic, EPUB, database,
  font, geography, toolpath, multisample, calendar, mail and text-edit viewers.
  Regression tests also cover rename, popups, dirty-image confirmation, file
  Save As and idle/selected drawing canvases. Affected suites: 946 file-manager
  tests and 843 studio tests pass (six and 12 ignored respectively). The final
  drawing change is rechecked by all five canvas tests. Strict all-target Clippy
  and the native build pass without warnings. Logs:
  `/tmp/consistent-escape-all-tests.log`, `/tmp/consistent-escape-drawing-tests.log`,
  `/tmp/consistent-escape-clippy.log` and `/tmp/consistent-escape-build.log`.
- [x] Native keyboard delivery returns calendar, toolpath and notebook viewers
  to their existing folders, without clicking Back to folder. Evidence:
  `/tmp/consistent-escape-native-dark-01/report.json`; reproduction:
  `scripts/file_manager_escape_acceptance.py`.
- [x] Architecture reviewed against `before-consistent-viewer-escape`.
  No renderer, dependency or close lifecycle is duplicated. Global average/max
  complexity and duplication percentage remain unchanged. The tool flags short
  repeated input-binding blocks and the drawing/wiring cancellation conditions;
  these preserve each local editor's first-Escape behavior while the host owns
  folder return. The drawing guard excludes derived ghost/retained-pixel and
  dismissed-draft identity state rather than treating these as active gestures.

## Media-view simplification review

- [x] Keep waveform range focus through mouse release. The release is still an
  audio gesture, so it must not fall through to file-row selection and steal
  Space/Ctrl+C focus. A real pointer-frame regression verifies press, drag and
  release ownership and range focus (`/tmp/list-range-focus-tests.log`). Native
  private-sink PCM checks verify click playback, Escape stop and Space range
  audition (`/tmp/list-media-native-dark-02/report.json`).
- [ ] Remove redundant Music/Videos screens and default selection, retaining
  their waveform/timeline functionality in List and migrating old view choices.

## Required delivery and acceptance

- [x] Audit every Phase 4 scope/gate against current owners, tests and native UI;
  fix actual gaps and replace its stale capability/architecture descriptions.
- [x] Preview small folders directly in List, with distinct containment styling,
  names, useful existing file previews and square single-click folder entry.
  Direct children take priority; nested groups only use spare inline capacity.
- [x] Preview archive contents in the same interaction model. Retain archive
  provenance. Passive strips do not extract; explicit archive entry creates a
  validated temporary snapshot with a persistent bottom-bar warning.
- [x] Configurable automatic previews of small folders and space-driven nesting,
  bounded depth/reads/entries/cache, cancellation and replacement invalidation.
- [x] Search `here` includes displayed ordinary descendants through the
  existing scope model, with accurate provenance, coverage and file actions.
- [x] Columns retains the complete path in a compact rail, nearby siblings and
  a shared rich List in the final column. Navigation/history and mixed files
  retain their ordinary browser owners; no separate preview pane remains.
- [x] Persist per-folder view overrides with explicit parent inheritance and a
  way to clear overrides. Browsing creates no sidecars in user folders.
- [x] Sort/group by broad content family (all images together). Each group gets
  appropriate metadata headers and sortable fields. Audio artist/album/genre,
  useful image/video/document metadata, deterministic missing values and stable
  selected-file identity while background metadata arrives.
- [x] Retain Phase 6 image hash/reference sorting. Warm/cool sorting is optional
  exploration in the roadmap, not a prerequisite for the requested delivery.
- [x] Native review in both appearances and narrow/wide windows; targeted safety,
  lifecycle, navigation and ordering tests; full workspace tests with bounded
  test concurrency, strict Clippy, formatting and architecture review.

## Implementation boundaries

Extend browser location/selection and existing bounded preview owners instead of
creating per-child studios. Visible expansion requests are bounded and retired
when hidden. Ordinary folders use pinned directory observations, skip automatic
symlink traversal and preserve incomplete reads. Archive inventories are read-only
and do not authorize extraction. Explicit archive navigation uses a bounded,
validated private snapshot and the same ordinary readers; it never installs
links or special members and never modifies the original archive.
Persistent preferences belong to local application state, using lossless paths;
portable project/visibility/session schemas remain Phase 8.

## Completed Phase 4 audit

The original audit input, preserved in `phase4_initial_checkpoint.md`, described
the initial checkpoint rather than current behavior:
shared studios now live in `clip-studios`; PDF/MIDI/model/notebook and other list
strips exist; embedded video works; studio title/path controls have changed.
These are stale documentation findings, not evidence that those features are
missing. Current owners include `viewers.rs`, `viewers/chrome.rs`,
`viewers/rename.rs`, `ui/list.rs`, `previews.rs`, shared studio source/save owners
and core `FileSource`/ordinary-directory pins. The completed audit in `phase4.md` ties each original scope item to current tests
and observed UI; both-theme current native audits pass.

## Initial container-preview checkpoint

- [x] Add small-folder cards with contained-file names/shared thumbnails,
  explicit collapse/expand and direct double-click opening of files/folders.
- [x] Add native ZIP/plain-TAR member inventories with full member-path hover,
  without extraction or treating members as ordinary host files.
- [x] Pin ordinary folders/files, reject replaced sources before/after inventory,
  preserve truncation, avoid automatic symlink traversal, and reuse core's
  allocation-bounded ZIP central-directory reader. TAR verifies member ranges.
- [x] Limit caches to 32 visible containers, two actual executing inventory reads
  on a separate existing BackgroundJobs pool, 32 displayed children, 4,096
  archive members and 512 MiB archive input. Retire work on collapse, hidden rows,
  navigation, source-context changes, view switches and studio entry.
- [x] Six focused tests and the full file-manager library pass (878 passed,
  six ignored); strict library/test Clippy and warning-free native build pass.
- [x] Native dark/light interaction reports pass at
  `/tmp/phase7-containers-dark-03/report.json` and
  `/tmp/phase7-containers-light-01/report.json`, including a real thumbnail,
  collapse/re-expand, nested folder navigation, history return and child studio
  opening. Tracked driver: `scripts/file_manager_phase7_acceptance.py`.

This is an initial slice, not completion of the container requirements above:
small-folder threshold defaults to eight and is configurable; nested inline levels,
child selection/search integration, archive member artwork/opening and broader
compressed archive families remain to implement. Search-result inventories are
not expanded yet, preserving published-result source guards until descendant
observations are integrated. No phase completion is claimed by these tests.

Architecture review against `before-phase7-views` has no new complexity threshold
violations, unchanged maximum complexity and duplication percentage, and no new
clones in the container modules. The aggregate tool still reports REGRESSION for
existing clones in touched large files/manifests and reordered untouched Python
cycles. The initial increase of ListPreviews::row above 15 was removed by moving
contained-row composition into its own rendering responsibility.

### Configurable inline previews

- [x] Add a List toolbar menu to enable/disable automatic small-container previews
  and set their item threshold (1–32). Explicit +/− remains available when the
  automatic default is disabled. No automatic media-view selection is changed.
- [x] Add backwards-compatible `[file_manager]` defaults:
  `inline_container_previews = true`, `small_container_items = 8`. Runtime menu
  changes are session-local; manual config edits set startup defaults. The later
  per-folder preference owner will provide remembered folder overrides.
- [x] Seven container tests and fourteen selected core configuration tests pass;
  strict file-manager library/test Clippy passes. Disabling automatic previews
  starts no inventory reads; explicit expansion and clamped limits are tested.

### Explicit expansion choices

- [x] Preserve explicit +/− choices when automatic-preview settings change or a
  child changes its folder's revision. Changing an automatic default must not
  silently replace an explicit user choice. Automatically collapsed reads are
  cancelled immediately; explicit expanded containers remain available.
- [x] Eight container tests and strict file-manager library/test Clippy pass;
  the rebuilt desktop binary is warning-free. New cases verify cancellation of
  automatic reads and preservation of explicit choices across settings/revisions.

### Native options review

- [x] Keep preview options readable: give the popup a stable width before laying
  out its checkbox/slider, with shorter labels. The initial automatic-width
  layout wrapped its first checkbox into an unnecessarily narrow column.
- [x] Extended native dark/light reports pass at
  `/tmp/phase7-options-dark-02/report.json` and
  `/tmp/phase7-options-light-01/report.json`. They verify automatic-off hides
  child previews, explicit expansion works while off, automatic-on restores
  archive previews, plus the original collapse/navigation/history/studio cases.

### Next integration constraints

The initial cards are a preview slice, not the completed expanded location model.
The remaining work must make ordinary children selectable, searchable in `here`,
and safely actionable; nested folder expansion and archive-member artwork/opening
must retain provenance. Do not flatten member names into host paths. In
particular, `edits::begin_naming` currently chooses a non-search rename parent
from the current folder: child selection must change that to the actual selected
entry's parent and retain execution-time ordinary-directory/source checks.

Keep explicit expansion choices distinct from cancellable cached observations;
scrolling, view switches and history must not accidentally redefine the user's
choice. Preserve the visible path/relative position when expansion heights or
metadata group headers change. Search must retain its existing guarded roots,
exclusions, scope bounds, incomplete coverage and stale-generation behavior when
expanded descendants are added.

Metadata type groups need one broad-family identity (e.g. Images, not JPG). Group
headers must reserve matching row cells, support artist/album/genre and useful
image/video/document fields, and put unknown/pending values consistently last in
both directions. Expensive metadata belongs on bounded workers, not in comparison
functions or per-frame probes. Completing a metadata sort must preserve selected
file identity and its visible anchor, rather than continually reshuffling the
active interaction as each individual result arrives.

Remembered folder preferences belong in local application state with explicit
nearest-parent inheritance and a clear override/reset action. Browsing must not
create sidecars in content folders. Preference writes must retain an accepted
write owner and integrate with close deferral; do not turn them into cancellable
preview jobs. Miller columns should reuse existing directory/source/navigation
and preview owners, keeping a single adjacent active preview and conventional
List available, rather than embedding a studio per column.

Final container checkpoint: full file-manager library passes 880 tests with six
ignored; strict library/test Clippy passes. Leaf Rust formatting, Python syntax
checks and `git diff --check` pass. Personal config edits remain outside commits.

### Remembered folder views

- [x] Add View → Folder view actions to remember the current view, explicitly
  inherit it in descendants, clear an override and reload saved preferences.
  Temporary manual choices remain authoritative for the current visit/history;
  a late save acknowledgement cannot undo a newer manual view choice.
- [x] Store bounded, versioned, lossless-path preferences in
  `$XDG_STATE_HOME/little-screen-clip/folder-views.json` (or the ordinary HOME
  fallback). Browsing alone creates nothing in state or content folders. Choices
  are path-based; renaming/moving a folder does not migrate its preference.
- [x] Merge cooperating-window writes under a nonblocking advisory lock, pin
  output directories and preserve accepted writes through publication/close.
  Corrupt, future-version, symlinked or protected state is never overwritten.
- [x] Keep the controls in the existing View menu. Truncate long inherited-origin
  labels with the full origin available on hover; move photo selection guidance
  into its button tooltip so the Photos toolbar remains usable.
- [x] Eight preference tests pass; full file-manager library passes 889 tests
  with six ignored (`/tmp/phase7-preferences-library-tests.log`). Strict library/
  test Clippy passed before the final label/layout change; final checks continue
  with the following slices. Native dark/light restart/inheritance/override/reset
  checks pass in `/tmp/phase7-preferences-dark-08/report.json` and
  `/tmp/phase7-preferences-light-02/report.json`. These verify exact saved state
  and unchanged content files. Earlier closes use Ctrl+Q; the final fresh client
  uses compositor close because GDB showed wtype delivering Ctrl+Escape instead
  of its requested first Ctrl+Q. No application shortcut behavior was changed.

Architecture review retains the baseline maximum complexity and slightly lowers
whole-tree duplication. Separating the general/photo/masonry toolbar duties
reduces `media_view_toolbar` complexity from 19 to 4. Preference tests share their
browser setup and acknowledgement polling. The aggregate comparison still flags
pre-existing clones in touched broad files and unchanged Python cycles; it is not
reported as an unqualified clean whole-tree architecture result.

### Empty ZIP inventories

- [x] Accept ordinary empty ZIPs in the shared allocation-bounded memory/file
  readers. Empty ZIPs begin with the end-of-directory signature, rather than a
  local member header. Both forms retain directory, size and count validation.
- [x] Shared reader tests verify empty memory/file archives, every truncated
  directory prefix and inconsistent member counts; both ZIP tests pass
  (`/tmp/phase7-empty-zip-tests.log`).

### Expansion intent across cache retirement

- [x] Keep explicit expand/collapse choices separately from visible inventories.
  Scrolling offscreen, opening a studio, switching views and folder/history
  navigation retire reads and thumbnails without resetting the user's choice.
  Choices follow paths for this session; at 4,096 chosen paths, new overrides are
  disabled with an explanation rather than silently evicting earlier choices.
- [x] Nine container tests pass, including cache retirement, folder/view reset,
  automatic-setting changes and cancellation of retired expanded reads. The
  extended native dark check passes at
  `/tmp/phase7-choices-dark-01/report.json`, verifying a collapsed folder stays
  collapsed after navigating away and back, and still re-expands/opens children.
  This does not complete nested inline expansion or child selection/search.

### Honest archive coverage

- [x] Count deliberately omitted ZIP/TAR links and special members and keep the
  inventory incomplete. A link-only archive now shows “No previewable entries”
  and its omission count instead of “Empty”. Ordinary directory omissions are
  disclosed too. Preview cards never activate archive links or special entries.
- [x] Refuse duplicate normalized member paths rather than silently hiding the
  ambiguity. Empty ZIP inventory, link/special-member coverage and duplicate TAR
  tests bring the container suite to twelve passing tests.
- [x] Native dark/light reports pass in
  `/tmp/phase7-archive-coverage-dark-02/report.json` and
  `/tmp/phase7-archive-coverage-light-01/report.json`, with exact source hashes
  unchanged and no extraction. Tracked runner:
  `scripts/file_manager_archive_inventory_acceptance.py`.
- [x] Strict file-manager library/test Clippy passes
  (`/tmp/phase7-archive-coverage-clippy.log`); the current native binary builds
  without warnings (`/tmp/phase7-archive-coverage-build.log`). Card composition
  has its own rendering responsibility, avoiding the new complexity threshold
  violation found during architecture review. The latest comparison introduces
  no excessive-CCN function; whole-tree maximum/duplication remain unchanged or
  lower, with the previously documented broad-file clone/cycle flags retained.

The expansion-choice native light check also passes at
`/tmp/phase7-choices-light-01/report.json`. These completed slices leave the main
Phase 7 requirements open: nested selectable/searchable contents, richer archive
member previews/opening and compressed formats, metadata groups/sorts and Miller
columns, followed by the full milestone acceptance audit.

### Contained-entry naming preparation

- [x] Capture every rename's actual source parent, including ordinary contained
  files, rather than using the browser root unless a search guard exists. The
  execution-time same-parent check and retained-directory mutation path remain
  authoritative; previously an ordinary child rename would fail that check.
- [x] Six naming tests pass (`/tmp/phase7-child-rename-tests.log`). The new test
  begins a contained-file rename from its ancestor's browser location, executes
  the captured operation, and verifies exact bytes remain in the child folder
  while a same-named root sibling is untouched. This prepares child actions;
  it does not yet expose/select children in the browser location model.

Final verification for this checkpoint: the current full file-manager library
passes 893 tests with six ignored (`/tmp/phase7-checkpoint-library-tests.log`),
and strict library/test Clippy passes (`/tmp/phase7-child-rename-clippy.log`).
Personal `.cargo/config.toml` and `config.toml` edits remain outside commits.

### Ordinary contained-file selection and actions

- [x] Make ordinary contained cards use the browser's existing row selection,
  context menu, drag, opening and file-action owners. Single-click selects;
  Ctrl/Shift selection and keyboard movement use the same displayed-entry model.
  Child folders are real navigation/drop targets. Archive members remain separate
  read-only member references, never invented host entries.
- [x] Retain up to 32 folder/archive observations (at most 1,024 displayed child
  records) independently from visible thumbnail payloads. Hidden reads are still
  cancelled. Admission evicts old unselected snapshots and protects parents of
  selected children; exhausted admission is disclosed in the row. Studio/view
  entry retires reads without discarding retained observations. Navigation to a
  different location clears them; same-folder refresh validates root revisions.
- [x] Keep an existing child selection while its refreshed/history observation
  is unknown. Pending children cannot authorize a mutation. A completed snapshot
  removes selections it proves absent; collapsing a folder removes its children
  from the displayed selection model. Delete confirmations retain child entry
  identities through the existing guarded mutation pipeline, and rename uses
  the source-parent fix from the preceding checkpoint.
- [x] Sixteen container tests pass, including child keyboard/range selection,
  offscreen retention, protected cache admission, pending/absent observations
  and archive-member separation. Full file-manager library passes 897 tests,
  with six ignored (`/tmp/phase7-child-selection-library-tests.log`); targeted
  final fixture checks pass (`/tmp/phase7-child-selection-targeted-tests.log`).
- [x] Native dark/light reports pass at
  `/tmp/phase7-child-selection-dark-03/report.json` and
  `/tmp/phase7-child-selection-light-04/report.json`. They verify the exact child
  clipboard URI, F2 rename inside its real parent, absence of an incorrectly
  relocated root file, history return and opening the renamed file's exact text.
  The observer waits for the renamed card, not the completion notice, before
  clicking through the asynchronous refresh. Small card labels have dedicated
  OCR crops; screenshots retain the full native view.

Architecture comparison against `before-phase7-expanded-selection` introduces
no excessive-CCN function or additional clone group after sharing the new test
fixture. Maximum complexity stays unchanged and duplication slightly decreases.
The aggregate still flags pre-existing clones in touched application/gallery/test
files and reordered unchanged Python cycles; it is not a clean aggregate result.
Nested inline levels, expanded-scope search and preservation of the visible
scroll anchor during layout changes remain required work, alongside archive
member previews/opening, metadata groups/sorts and Miller columns.


### Bounded nested inline sections

- [x] Add separate disclosure controls to actual folder/archive cards. Selecting,
  dragging and opening their names retain the normal browser actions; disclosure
  expands an indented subsection without navigating away. Nested headers reuse
  the host's row renderer and columns through an explicit rendering callback.
- [x] Automatic expansion remains one level. Further expansion is explicit, up
  to four container levels including the root. Each root preview lays out at most
  32 sections and discloses truncation; global observations remain capped at 32,
  with 32 child records each and two actual executing inventory reads. Only
  visible subsection rows admit reads, and frame retirement cancels hidden work.
- [x] Use the same bounded section walk for height, displayed entries and keyboard
  reveal offsets. Hidden subsections reserve no space or expose descendants while
  hidden files are disabled. A collapsed subsection removes descendants from the
  active selection model. Selected descendants protect every retained ancestor.
- [x] Preserve selection through unknown ancestor coverage during refresh/history
  loading, while requiring observed entries for mutation. Completed observations
  prove absence; unchanged nested observations survive a same-folder refresh.
  Revision invalidation has one node-owned transition that preserves explicit
  expansion intent while cancelling obsolete work and clearing stale observations.
- [x] Twenty-one container tests are covered by the full file-manager library:
  902 passed, six ignored (`/tmp/phase7-nested-library-tests.log`). Native reports
  pass in `/tmp/phase7-nested-dark-02/report.json` and
  `/tmp/phase7-nested-light-01/report.json`, verifying nested expansion in place,
  the nested child's exact clipboard URI, collapse, and the previous ordinary
  child rename/history/open workflow. Screenshots confirm the indented hierarchy;
  an isolated label crop handles a dark-theme OCR omission without weakening the
  clipboard/path assertions.

Architecture review: new functions remain below the excessive-complexity gate;
scoped Lizard measures maximum CCN 13. Shared revision invalidation and test setup
remove actual repeated implementation. Aggregate comparison still reports
REGRESSION: its new source clone is a seven-line rendering parameter declaration,
plus a six-line refresh/assertion sequence in tests; pre-existing touched-file
clones and reordered unchanged Python cycles remain. These are not duplicate
business rules, and no wrapper was added merely to hide their tokens. File-target
Valknut measurement was unavailable, so dependency findings use the full-tree
comparison rather than claiming a successful scoped structural check.

The folder-preview and configurable-expansion delivery items are now checked off.
Expanded-scope search and scroll-anchor preservation still require integration;
archive member artwork/opening and broader compressed inventories, metadata
family headers/sorts and Miller columns remain open. This checkpoint does not
mark Phase 7 complete.

Final nested checkpoint checks: strict file-manager library/test Clippy passes
(`/tmp/phase7-nested-clippy.log`), the current binary builds without warnings
(`/tmp/phase7-nested-build.log`), leaf Rust formatting and Python syntax checks
pass, and `git diff --check` is clean. Personal configuration remains unstaged.

### Expanded-folder search scopes

- [x] Include expanded ordinary folders in `here` name, regex, metadata,
  content and semantic searches. Capture at most 33 directories including the
  current folder, through four levels; retain the captured scope across query
  edits and refreshes. Closed folders, directory aliases and archive members do
  not become descendant scopes. No automatic index enrollment is introduced.
- [x] Revalidate captured directory identities and mount/session observations in
  disposable readers, including first probes of pending observations. Apply exact
  parent constraints before lexical/vector result limits and recheck coverage
  before publication. Preserve exclusions, hidden-file policy and partial results.
- [x] Share worker dispatch across submission, refresh and directory watching so
  ordinary watch updates cannot replace expanded results. Hidden-file changes
  rerun the accepted query. Preserve bounded preview observations on studio/search
  transitions. File actions verify every captured ancestor, even if a leaf and its
  immediate parent retained their identities after an ancestor replacement.
- [x] Full library tests pass: file manager 904 passed / 6 ignored; search 298
  passed / 4 ignored (`/tmp/phase7-expanded-search-final-tests.log`). Strict
  library/test Clippy, application build, search rustdoc/doctest, leaf formatting,
  Python compilation and diff whitespace checks pass.
- [x] Native dark/light acceptance passes at
  `/tmp/phase7-expanded-search-{dark,light}-03/report.json`: name/content search,
  unenrolled semantic filename fallback, real child clipboard paths and rejection
  of an ancestor replacement while leaf identity/timestamps remain unchanged.

Architecture comparison against `before-phase7-expanded-search` adds no excessive
complexity function and leaves maximum complexity unchanged. Aggregate clone
flags include existing SQLite error handling and typed test-fixture setup;
reviewed scoped production duplication is pre-existing. Shared scope preparation,
query dispatch and action guards avoid independent search-policy implementations.
Image reference selection from expanded cards still needs the separate image-sort
compatibility audit above; temporary answer columns are not claimed for inline
cards by this checkpoint.

### Expanded-card image reference compatibility

- [x] Fix similarity-sort reference selection for ordinary images selected inside
  expanded folders. Feed the captured root/child entry iterators into the existing
  image-order owner for both admission and ongoing reference validation, without
  copying a whole directory or adding another hash cache/decoder.
- [x] Five image-order tests pass, including a real nested-folder reference and
  invalidation when its captured revision changes. Existing unsigned hash order,
  Hamming-distance order and missing/error placement remain covered. Strict
  file-manager library/test Clippy and diff whitespace checks pass
  (`/tmp/phase7-expanded-reference-{tests,clippy}.log`).

Architecture comparison against `before-phase7-expanded-image-reference` leaves
production complexity, file count and clone count unchanged. Its clone flag is
existing visible-photo filtering in status/tick; reported Python dependency-cycle
changes only reorder the same unchanged members. The main image-sort acceptance
item remains open for final integrated milestone validation.

### Broad-family List sections

- [x] Type sorting now orders broad content families, with natural filenames
  across formats inside each family. JPEG/PNG and WAV/MP3 do not form separate
  extension subgroups. Multisample instruments join Audio; structured documents
  and archives retain their family instead of falling into generic text/binary
  groups merely because their serialization is textual or binary.
- [x] Add named section headers to the same variable-height List virtualization,
  preserving actual row selection, drag targets and nested-child keyboard reveal.
  Headers have distinct widget identities and reuse existing sort controls;
  ordering, answer-column state and row geometry share one active-sort predicate.
- [x] Test both family directions, natural intra-family ordering, exact added
  header heights, normal-view restoration and selected-file identity. Full
  file-manager library: 906 passed / 6 ignored
  (`/tmp/phase7-family-all-tests.log`). Strict library/test Clippy, application
  build, leaf formatting, Python compilation and diff whitespace checks pass.
- [x] Native dark/light reports pass at
  `/tmp/phase7-family-{dark,light}-03/report.json`: existing name/size/date sorts,
  type-navigation focus, Audio/Images headers, reversed family order and clicking
  a section's own Size header. Synthetic invalid media confirms grouping does not
  depend on decoder success. Earlier runs with the expanded Shelf correctly
  failed visibility checks; final runs explicitly collapse it to expose both
  sections together.

The architecture comparison against `before-phase7-family-groups` adds no
excessive-complexity function or clone group. Reviewed aggregate flags touch
pre-existing test fragments and unchanged Python cycle/chokepoint observations;
scoped duplication does not touch the new family test. File-kind handling and
format grouping remain separate, and row layout shares the same family boundaries
as sorting.

This checkpoint supplies the family sections, not the completed metadata feature.
Family-specific fields, bounded metadata acquisition and metadata sorting remain
required under the unchecked main milestone item. Existing music previews already
parse artist/album; their waveform/cover work must not be repeated just to sort
unseen files. Reuse their tag parsing for lightweight summaries, add genre, and
retain those small revision-checked summaries independently of visible waveform
textures. Keep missing/pending values last in either direction and selected-file
identity stable as values arrive.

### Audio family metadata columns and sorting

- [x] Give the Audio section Artist, Album, Genre and Duration headers and values.
  Clicking a metadata header sorts inside that family; clicking again reverses
  it. Family ordering and conventional sorts remain available. Missing tags,
  unsupported members, pending values and failures remain last in both directions;
  equal values preserve the existing natural filename order. Numeric durations
  sort numerically, not by their display strings. These are session secondary
  sorts; no new command syntax or persisted search schema is introduced.
- [x] Reuse the music tag parser, adding genre and passing cancellation into its
  bounded media probe. Metadata summaries never request waveform, cover or PCM
  decoding. Keep at most 8,192 small revision-keyed summaries and two pending
  reads per browser on a shared two-worker/four-queue executor. Queue saturation
  retries admission; retirement cancels reads and resets scheduling so returning
  to the view can retry unfinished files. The section reports progress and the
  summary limit; additional files remain accessible and explicitly unanalyzed.
- [x] Check ordinary source revisions around reads and retain the existing search
  PreviewContext checks. Reconcile summaries against current candidates and retire
  them on path/source-context changes. Known summaries survive ordinary preview
  retirement; they do not retain audio payloads or textures. Candidate scanning
  happens on listing reconciliation, rather than scanning the folder every frame.
  Completed tags paint immediately; active metadata sorts coalesce updates over
  200 ms. Ordinary metadata completion does not rebuild an unsorted listing.
- [x] Preserve selected-file identity and the top visible file/within-row offset
  during background reordering. Share that List anchor mechanism with asynchronous
  container-height updates. Explicit header sorts use the browser's existing sort
  completion behavior. Audio headings omit the duplicate tag/title summary when
  those fields have their own columns; other media views retain their details.
- [x] Six focused tests cover typed/case-folded values, stable ordering and missing
  values in both directions, source revision/context invalidation, scroll/selection
  retention, summary capacity and retirement, and actual genre/album/artist/duration
  extraction from a tagged WAV with no preview artifacts. Full file-manager library
  passes 912 tests / 6 ignored (`/tmp/phase7-audio-metadata-all-tests.log`). Strict
  library/test Clippy, application build, leaf formatting, Python compilation and
  diff whitespace checks pass.
- [x] Native dark/light reports pass at
  `/tmp/phase7-audio-metadata-{dark,light}-03/report.json`: each audio metadata field,
  both artist/duration directions, absent artist placement, real tagged WAVs and
  mixed Audio/Images headers. Screenshots were visually reviewed in both themes.
  The driver explicitly selects List when the existing automatic behavior initially
  chooses Music; earlier short fixture names were replaced after a dark OCR
  mismatch despite correct rendered ordering.

Architecture comparison against `before-phase7-audio-metadata` adds no excessive
complexity function or production clone. Its one added clone is seven lines of
necessary typed command setup/assertions shared by two native-media tests; no
extra abstraction is introduced solely to hide that fixture setup. Remaining cycle
flags reorder unchanged Python members. The cache reuses the shared cancellable
Task delivery and source guards; explicit sorts share browser sort completion.

The main metadata acceptance item stays open: useful image/video/document fields,
related ordering and final mixed/narrow-window validation remain required. The
new scroll anchoring is an initial shared mechanism, not final proof for every
nested-expansion and layout-change scenario.

### Image, video and document family metadata

- [x] Add Images width/height/megapixels, Video duration/width/height/codec and
  Documents title/author/pages, using the same secondary-sort owner as Audio.
  Compare exact numeric values independently of rounded display strings. Photo
  dimensions follow observed orientation. Reflowed EPUBs do not invent page
  counts; unsupported or unavailable properties stay unknown and sort last.
  Short audio/video durations display milliseconds instead of rounding to zero.
- [x] Reuse the shared photo header cache, selected non-cover video stream reader,
  PDF info reader and notebook info helper. Separate video header facts from the
  playback pixel budget, preserving the existing decoder admission limit. A large
  video can have useful header facts even when its preview cannot be decoded.
- [x] Add cancellable photo/PDF header entrypoints while preserving existing
  callers. Add seekable EPUB package metadata through the existing bounded ZIP/XML
  parser; it reads no chapter/artwork payloads and retains no book container.
  Full Book parsing shares tag extraction with the header path. These helpers
  retain ordinary-source checks and do not change persisted inventory/cache/search
  representations. Header tags alone do not establish complete book readability.
- [x] Share the existing bounded summary executor, revision/source-context checks,
  candidate budget and coalesced sorting across families. Count progress per
  family. Handwriting hints use the notebook page inspector, while Jupyter cells
  are not mislabeled as handwritten pages. Document tag readers currently cover
  PDF, EPUB and the supported handwriting containers; other formats retain unknown
  properties and their normal browser/preview access. Office files are not rendered
  merely to obtain sort keys.
- [x] Add a metadata-header context menu to restore that family's natural filename
  order without clearing another family's choice. Centralize file/metadata sort
  headers into one fixed-rectangle control, with clipped labels, SVG direction
  arrows, distinct identities and no layout growth in narrow cells. Align ordinary
  Size/Modified values with the shared header padding; the existing mixed-media
  alignment test caught and now guards the padding regression. Suppress duplicate
  photo dimensions and video descriptions when those facts have their own columns;
  video audio-track descriptions remain available.
- [x] Full core library: 440 passed / 3 ignored. Full file-manager library: 917
  passed / 6 ignored. Ten focused metadata tests cover typed fields, independent
  family counts/reset, source lifetimes, exact pixel arithmetic, header-only images,
  real selected-stream/EPUB facts, capacity and selection/scroll continuity. Core
  tests include package-only EPUB tags with absent chapters, PDF count/cancellation
  and cancellation before returning cached photo headers. The narrow-header test
  checks actual interaction/layout bounds for long labels in 28/60/160-pixel cells.
- [x] Strict core/file-manager library/test Clippy and the application build pass
  without warnings. Core rustdoc with `-D warnings` and 12 doctests pass; rendered
  pages for the new APIs were inspected. Leaf formatting, Python compilation and
  diff whitespace checks pass. Logs:
  `/tmp/phase7-family-metadata-{core-all-tests,all-tests,clippy,build,rustdoc,doctests}.log`.
- [x] Native dark/light family reports pass at
  `/tmp/phase7-family-metadata-{dark,light}-04/report.json`: oriented JPEG dimensions,
  exact pixel and video property order, PDF/EPUB title/author/count order, unknown
  reflowed page counts, family reset and independent sort retention. Audio
  regressions pass at `/tmp/phase7-family-audio-{dark,light}-01/report.json`; general
  name/size/date/type and section-header regressions pass at
  `/tmp/phase7-family-sorting-{dark,light}-01/report.json`. Both-theme screenshots
  were visually reviewed.

Native scenarios explicitly select List after filters because existing automatic
media selection can switch to Videos. The document scenario uses an extension
regex: the existing `:document` selector still means supported extracted-text
formats, rather than every member of the broader browser Documents section.
Those Phase 6 selector semantics were preserved. Native ordering checks inspect
filename crops in the actual observed rows, avoiding full-window OCR losses of
filename punctuation and active-header arrows; checks select the interiors of
observed labels rather than rounded boundary pixels.

Architecture comparison against `before-phase7-family-metadata` introduces no
excessive-complexity function and reduces aggregate duplication by 0.01 percentage
points. Reviewed clone flags are existing music/video view setup, PDF mutation
fallbacks and typed native-fixture command setup, not new duplicated header/read
policies. The shared header control removes separate rendering implementations;
format-specific loaders delegate to their existing owners. The metadata delivery
item is now checked; final whole-milestone narrow/wide, lifecycle, compatibility
and workspace validation remain under the separate unchecked acceptance gate.

### Bounded compressed TAR inventories

- [x] Preview gzip, bzip2, xz and zstd TARs through native compiled decoders,
  including common short aliases. Ordinary compressed non-TAR files are not
  guessed as containers. Reuse the existing pinned ordinary source, revision,
  TAR member-path, duplicate-name and special/link omission policies. No member
  is extracted or treated as a host path.
- [x] Bound encoded sources to 512 MiB and decoded reads to 64 MiB. Xz memory
  and zstd window limits are 64 MiB. Check cancellation and a cooperative
  five-second deadline at read boundaries; this does not interrupt a blocked
  underlying filesystem read. Existing two-worker ownership remains unchanged.
- [x] Stop after 33 observed members and display at most 32 cards. Stop before
  skipping a declared oversized payload. Retain partial header observations with
  a visible preview-limit notice, explicitly marking remaining payloads unverified.
  Complete small inventories require decoder EOF, including checksum validation;
  concatenated gzip/TAR streams are inspected rather than silently omitted.
- [x] Six codec tests cover real compressed data, lossless non-UTF8 source paths,
  concatenation, bad/truncated trailers, unsafe/duplicate member names, decoded
  and card limits, exact-budget EOF, cancellation and deadline checks. Full
  file-manager library tests: 923 passed / 6 ignored. Strict library/test Clippy
  and application build pass. Logs: `/tmp/phase7-compressed-{all-tests,clippy,build}.log`.
- [x] Native dark/light reports pass at
  `/tmp/phase7-compressed-{dark,light}-03/report.json`. Each codec's own rendered
  row contains its decoded member; expanding a genuine 128 MiB member exposes
  the partial-limit notice. Source hashes are unchanged and no extracted paths
  exist. Both-theme screenshots were visually reviewed.

Architecture comparison against `before-phase7-compressed-archives` reports
unchanged rounded duplication and maximum complexity, with one additional clone
cluster. Scoped review identifies test limit-fixture setup, dependency manifest
lists and existing rendering signatures/fixtures; there is no duplicated
production decoding or member-path policy. Reported Python cycles concern
unchanged upstream/worker functions with reordered members. This increment adds
inventory coverage only: the archive milestone remains open for member artwork,
activation and safe navigation. Final integrated Phase 7 acceptance is also open.

### Initial Miller columns and adjacent preview

- [x] Add a Columns view alongside List, sharing the browser's current directory,
  sorted/filtered visible entries, selection, row menus, file actions, history
  and view preferences. Ordinary folder clicks navigate; modified clicks retain
  multi-selection. Left returns to the parent and reveals the departed folder;
  Right enters a selected folder. Existing Up/Down, Enter and Alt+Back/Forward
  keep their browser roles.
- [x] Reuse the sidebar's directory observations and bounded directory jobs for
  nearby parent columns, without a second listing cache or recursive sibling
  enumeration. Show one parent at ordinary widths and two when space permits;
  horizontally scroll smaller layouts. Virtualize each vertical listing and
  reveal offscreen route/keyboard selections. Clip painting and hit targets to
  the actual column/dock boundaries. Parent-column drops retain the parent as
  destination instead of inheriting the current folder's drop target.
- [x] Render one chosen file beside the current column through the shared preview
  owner. Text uses existing syntax-colored excerpts, images use existing decoded
  textures, and other available summaries use their existing details. Unknown
  formats remain visible/openable. Retire other preview requests on selection or
  view changes. The adjacent pane is a preview, not a second studio; its Open
  action uses the ordinary shared viewer dispatch.
- [x] Keep Columns during new navigation unless the destination has a saved view;
  history restores its own view, selection and scroll. List remains available
  without losing selected-file identity. Columns uses the existing local view
  preference serialization/inheritance machinery and adds no folder sidecars.
- [x] Full file-manager library tests: 925 passed / 6 ignored with four test
  threads. The first unrestricted run hit a playback-worker QueueFull error;
  bounded reruns pass. Added route-width/root tests and actual browser navigation,
  history, selection/scroll, view-switch and serialization checks. Strict
  library/test Clippy, application build, leaf formatting and Python compilation
  pass. Logs: `/tmp/phase7-columns-{tests,route-test,clippy,build}.log`.
- [x] Both-theme native reports pass at
  `/tmp/phase7-columns-{dark,light}-03/report.json`: folder navigation, text
  excerpts, an image verified by rendered pixel colors, Back/Forward, keyboard
  parent/child navigation and keyboard preview selection. Screenshots were
  visually reviewed. Earlier light OCR missed a filename despite correct
  navigation; the driver now includes a crop of the actual current column.

Architecture comparison against `before-phase7-miller-columns` retains rounded
aggregate duplication and maximum complexity. The existing dock dispatcher gains
one necessary view branch (CCN 18 to 19); new view-specific keyboard handling
lives with the column renderer rather than growing generic list shortcuts.
Reported clones are existing view transition/setup blocks and test fixture/assert
patterns. Directory loading, guarded viewer dispatch, selection/file operations,
preview decoding and persisted preferences retain their existing owners.

This is the first working Columns delivery. The main acceptance checkbox remains
open until narrow/wide native review, broader mixed-format/interaction checks and
whole-milestone verification. Archive member artwork/activation and image-reference
compatibility acceptance also remain open; this checkpoint does not complete
Phase 7.

### Archive artwork, member activation and read-only browsing

- [x] Reuse the existing ZIP/TAR/compressed-TAR inventory and path policy for
  inline cards and a bounded full archive inspector. Cards render image artwork
  and expose captured text on hover. Clicking a member opens its exact observed
  ordinal in the inspector; opening the archive itself starts at its virtual
  root. Infer directories from member prefixes, including archives without
  explicit directory records. Root/Up navigation stays inside that archive.
- [x] Preserve the archive origin in the existing editable file header and show
  the selected member's full archive-relative name separately. Members never
  become filesystem Entry values, host selections, mutation destinations or
  extracted paths. Archive renaming uses the existing guarded file operation,
  rebinds the inspector source and clears its old preview jobs/cache. Closing
  returns to the browser through its existing viewer lifecycle.
- [x] Bind member activation to the observed archive revision. Reject a replaced
  or edited archive before loading its member ordinal; an old card must never
  select an unrelated member in a new container. Compare the loaded inspector's
  FileSource with the opening request's captured identity. Each payload lookup
  opens through the ordinary-content policy and validates the retained source
  before and after reading, with the existing ReadCheck for guarded sessions.
- [x] Keep inline inventories at 32 cards. The inspector scans at most 4,096
  members, with existing 512 MiB encoded-source and compressed 64 MiB decoded,
  window and cooperative five-second limits. Fetch one regular member at a time,
  bounded to 8 MiB, and require a fully validated inventory before publishing its
  payload. Check cancellation between bounded reads. Later duplicate/unsafe names,
  corrupt trailers, source changes and incomplete scans discard selected bytes.
  ZIP directory mode remains metadata-only even without a trailing slash.
- [x] Reuse shared bounded image decoding and preview delivery. Cards use small
  thumbnails; the chosen inspector image uses up to 1,024 pixels per dimension.
  Text is validated UTF-8 and limited to 64 KiB at a character boundary, with a
  visible clipping notice and virtualized lines. Other binary members show
  metadata rather than being decoded as text. SVG/markup remains text here;
  this is not an extraction, embedded-resource or full media-editing facility.
- [x] A separate two-worker/four-queue executor retains actual read ownership.
  Reuse the Shelf preview cache/delivery mechanism, retaining only visible card
  requests (at most 128 identities) or the inspector's chosen member. Hidden
  cards, selection changes, navigation, studio entry and close retire requests.
  Decoder/source rules stay in the inventory owner instead of being copied into
  the inspector. Ordinary-file session identity publication is separated from
  job orchestration; ZIP header admission lives beside TAR member admission.
- [x] Full file-manager library: 932 passed / 6 ignored with four test threads.
  Added seven tests cover 80-member inventories and sorted ordinal identity,
  virtual prefixes, image resolution budgets, binary/UTF-8 behavior, rejected
  read guards, changed sources, gzip duplicate/trailer failures, oversized members,
  archive rename and stale-card opening. Existing codec tests now also retrieve
  a real selected payload through gzip, bzip2, xz and zstd. Strict library/test
  Clippy, application build, leaf formatting, Python compilation and diff checks
  pass. Logs: `/tmp/phase7-members-{all-tests,clippy,build}.log`.
- [x] Native dark/light reports pass at
  `/tmp/phase7-members-{dark,light}-02/report.json`: inline artwork and expanded
  images are verified by actual rendered pixels, members open at the clicked
  position, virtual folders navigate, text and compressed-TAR members render,
  Back returns to the listing, source hashes remain unchanged and no member paths
  appear on disk. Both-theme screenshots were visually reviewed. Compressed
  inventory/large-payload limit regressions pass at
  `/tmp/phase7-compressed-{dark,light}-04/report.json`; its driver follows the
  Columns-enabled toolbar's observed Shelf position.

Architecture comparison against `before-phase7-archive-members` reduces aggregate
percentage duplication by 0.01 points, retains maximum complexity and adds two
clone clusters. Scoped inspection finds existing cancellation drops, enum variant
lists, test setup/assertions and renderer parameter lists. The new hover fragment
shares the usual detail/image/text display vocabulary, but bounds archive text
and uses monospace versus the ordinary preview's full text/audio handling. No
second archive parser, path validator, image decoder, job delivery or mutation
mechanism was added. The existing type dispatcher gains one necessary archive
case (CCN 17 to 18). New rendering and ZIP/session admission responsibilities do
not introduce additional high-complexity functions. Reported Python cycles are
unchanged functions with reordered membership, not new imports.

The archive delivery checkbox is now checked. Whole-milestone narrow/wide,
Miller interaction/compatibility and workspace acceptance remain open; this does
not mark Phase 7 complete.

### Columns row-reveal spacing correction

- [x] Fix current-row and parent-route reveal offsets to include egui's vertical
  item spacing, matching `ScrollArea::show_rows` and the existing sidebar's
  arithmetic. A distant selected row could otherwise remain outside the painted
  virtual range; the wide native review exposed the same error in parent columns.
- [x] Add an actual rendering regression with 100 directory rows. It checks that
  the selected last row is painted in both current and ancestor columns, at
  spacing 0/6/12. Before the fix it fails at spacing 6 in the current column;
  after the fix all six cases pass. Logs:
  `/tmp/phase7-columns-spacing-{before,after}.log`.
- [x] Strict file-manager library/test Clippy and application build pass:
  `/tmp/phase7-columns-spacing-{clippy,build}.log`.

This correction is committed independently from the ongoing wide/narrow native
acceptance driver changes. It does not close the remaining Columns or integrated
Phase 7 acceptance gates.

### Columns resize selection correction

- [x] Detect changes to the Columns viewport and reveal the active row in the
  resized current column. Give current and preview panes stable widget identities
  when the number of displayed parents changes. A shorter window could otherwise
  leave the selected last row below the visible range.
- [x] Add a persistent-context rendering regression across 1600×600 and 700×320
  frames with 100 rows. It verifies the selected row in the actual current-column
  bounds and paint clip, rather than accepting its name in the adjacent preview.
  It fails at 700×320 before the correction and passes afterward. Reuse the
  existing long-list fixture for the spacing test. Logs:
  `/tmp/phase7-columns-resize-{before,after}.log`.
- [x] Strict library/test Clippy and application build pass:
  `/tmp/phase7-columns-resize-{clippy,build}.log`. Both native geometry runs at
  `/tmp/phase7-columns-geometry-{dark,light}-03/` reach and pass the distant-row
  resize observation; they later expose a separate text-preview width defect.

The viewport correction is committed separately. Narrow horizontal text preview
acceptance remains open until the subsequent width defect is corrected.

### Docked Columns text-preview width correction

- [x] Separate docked text excerpt sizing from hover-overlay sizing, sharing the
  existing syntax/run renderer. Docked panes use their available width and keep
  their short-file content independent of pointer height. Hover previews retain
  their existing screen/pointer budget. The former hover-only renderer forced a
  760-pixel width inside a 300-pixel pane, creating blank over-scroll space.
- [x] Add a rendering regression for a 300-pixel pane on a 1600-pixel screen,
  with the pointer at both top and bottom. It fails at 760 pixels before the fix;
  the pane entrypoint stays within 300 pixels and renders the excerpt at both
  positions. All 13 text-strip tests pass, along with strict library/test Clippy
  and the application build. Logs:
  `/tmp/phase7-columns-text-{before,after,clippy,build}.log`.
- [x] Native geometry reports pass in both themes at
  `/tmp/phase7-columns-geometry-{dark,light}-04/report.json`, including wide two-
  parent Columns, unsupported-file accessibility, studio return, a distant row
  through resizing, narrow horizontal access to the text preview, actual selected
  file URI copying and List/Columns switching. This validates the preceding
  spacing and resize fixes together with the docked renderer. The tracked driver
  and its shared sizing/input support are committed as the acceptance increment.

### Integrated Columns and reference-sort acceptance checkpoint

- [x] Extend the existing private native runner with optional output dimensions
  and matching pointer coordinates, retaining the 1280×800 default. Add explicit
  horizontal-wheel input to its existing pointer helper. Parse the axis alongside
  the other input modes instead of growing the event-dispatch branching. This
  affects acceptance tooling, not application input or user configuration.
- [x] Extend the Columns driver with 1560×900 and 760×800 windows on a 1760×1000
  private output. Both themes pass at
  `/tmp/phase7-columns-geometry-{dark,light}-04/report.json`: two parent columns,
  ordinary/unsupported files, text preview, opening/returning from a studio,
  selection of row 99 in a 100-file listing, resizing with that row still visible,
  narrow horizontal text access, actual selected-file clipboard URI publication,
  wide restoration and List/Columns switching. Both-theme screenshots were
  visually reviewed. Earlier failures led to the three separately committed
  spacing, resize and docked-text fixes recorded above.
- [x] Extend the existing preference scenario to exercise Columns. Both themes
  pass at `/tmp/phase7-columns-preferences-{dark,light}-01/report.json`: saved
  parent Columns, explicit inheritance, child List override, restart, reset to
  inherited Columns, a second restart and unchanged content folders. The same
  driver retains its Photos mode. Update observed toolbar positions after the
  Columns tab was added.
- [x] Existing root-image ordering scenarios pass at
  `/tmp/phase7-image-order-{dark,light}-01/report.json`. Add an expanded-folder
  mode to the same driver; both themes pass at
  `/tmp/phase7-expanded-image-order-{dark,light}-01/report.json`. Alpha/Copy are
  actual contained-file cards, while opposite/unreadable images remain at the
  root. Verify chosen-reference distances, reference retention after selection,
  header reversal, invalidation after changing the contained reference, unsigned
  dHash ordering including the maximum u64 hash, honest failed-image coverage,
  restored browsing, and absence of embedding index/models/runtime enrollment.
- [x] Refresh the existing expanded-search/container drivers' toolbar coordinates.
  The guarded expanded-name/content/fallback/copy/ancestor-replacement scenario
  passes at `/tmp/phase7-expanded-search-dark-04/report.json`. The full container
  preview/settings/collapse/history/nesting/rename/open scenario passes at
  `/tmp/phase7-containers-light-10/report.json`; the preceding run clicked the
  now-moved Shelf button instead of Folder previews and is retained as a failed
  observation, not a product failure.
- [x] `cargo test --workspace -j4 -- --test-threads=4` passes: 3,274 passed,
  zero failed, 36 ignored across 47 reported suites. Log:
  `/tmp/phase7-workspace-tests.log`. The ignored tests are not claimed as run.
  Python compilation and diff whitespace checks pass for the changed drivers.

Architecture comparison against `before-phase7-columns-integration` retains
aggregate duplication and maximum complexity, with no newly flagged function
complexity or clone regression. Reported dependency cycles are unchanged Python
functions with reordered membership; the input helper's axis parser keeps mode
recognition separate from event dispatch. Existing renderer, file-source,
selection, preference and search owners remain in place.

The Columns and image-reference delivery checkboxes are now checked. The final
whole-milestone gate remains open: full-workspace strict Clippy/formatting and the
final requirement/source/evidence audit have not yet been completed. Further
metadata/nested-layout regression review may still identify follow-up work.
The user requested a pause after this acceptance commit to review the branch;
this checkpoint does not mark the goal complete.
