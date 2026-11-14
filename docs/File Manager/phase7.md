# Phase 7 — views, container previews and meaningful sorting

Status: in progress on `feat/file-manager-phase7`, starting at `d0298ebd`.
Architecture baseline: `before-phase7-views`. Automatic view selection is
explicitly excluded by the user; existing automatic behavior is preserved.

## Required delivery and acceptance

- [x] Audit every Phase 4 scope/gate against current owners, tests and native UI;
  fix actual gaps and replace its stale capability/architecture descriptions.
- [ ] Preview small folders directly in List, with distinct containment styling,
  names, useful existing file previews and obvious child activation. Disclosure
  and opening a folder remain separate actions.
- [ ] Preview archive contents in the same interaction model. Retain archive
  provenance; never treat member names as host paths or extract implicitly.
- [ ] Configurable automatic expansion of small folders, explicit expand/collapse,
  bounded depth/reads/entries/cache, cancellation and replacement invalidation.
- [ ] Search `here` includes explicitly expanded ordinary descendants through the
  existing scope model, with accurate provenance, coverage and file actions.
- [ ] Miller columns with an adjacent preview, familiar navigation/history and
  the conventional List retained. Mixed/unsupported files remain accessible.
- [ ] Persist per-folder view overrides with explicit parent inheritance and a
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

## Phase 4 audit in progress

The old `phase4.md` describes the initial checkpoint, not current behavior:
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
