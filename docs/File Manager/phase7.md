# Phase 7 — views, container previews and meaningful sorting

Status: in progress on `feat/file-manager-phase7`, starting at `d0298ebd`.
Architecture baseline: `before-phase7-views`. Automatic view selection is
explicitly excluded by the user; existing automatic behavior is preserved.

## Required delivery and acceptance

- [x] Audit every Phase 4 scope/gate against current owners, tests and native UI;
  fix actual gaps and replace its stale capability/architecture descriptions.
- [x] Preview small folders directly in List, with distinct containment styling,
  names, useful existing file previews and obvious child activation. Disclosure
  and opening a folder remain separate actions.
- [ ] Preview archive contents in the same interaction model. Retain archive
  provenance; never treat member names as host paths or extract implicitly.
- [x] Configurable automatic expansion of small folders, explicit expand/collapse,
  bounded depth/reads/entries/cache, cancellation and replacement invalidation.
- [x] Search `here` includes explicitly expanded ordinary descendants through the
  existing scope model, with accurate provenance, coverage and file actions.
- [ ] Miller columns with an adjacent preview, familiar navigation/history and
  the conventional List retained. Mixed/unsupported files remain accessible.
- [x] Persist per-folder view overrides with explicit parent inheritance and a
  way to clear overrides. Browsing creates no sidecars in user folders.
- [ ] Sort/group by broad content family (all images together). Each group gets
  appropriate metadata headers and sortable fields. Audio artist/album/genre,
  useful image/video/document metadata, deterministic missing values and stable
  selected-file identity while background metadata arrives.
- [ ] Retain Phase 6 image hash/reference sorting. Warm/cool sorting is optional
  exploration in the roadmap, not a prerequisite for the requested delivery.
- [ ] Native review in both appearances and narrow/wide windows; targeted safety,
  lifecycle, navigation and ordering tests; full workspace tests with bounded
  test concurrency, strict Clippy, formatting and architecture review.

## Implementation boundaries

Extend browser location/selection and existing bounded preview owners instead of
creating per-child studios. Visible expansion requests are bounded and retired
when hidden. Ordinary folders use pinned directory observations, skip automatic
symlink traversal and preserve incomplete reads. Archive inventories are read-only
and do not authorize extraction, external resource resolution or mutations.
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
