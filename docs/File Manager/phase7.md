# Phase 7 — views, container previews and meaningful sorting

Status: in progress on `feat/file-manager-phase7`, starting at `d0298ebd`.
Architecture baseline: `before-phase7-views`. Automatic view selection is
explicitly excluded by the user; existing automatic behavior is preserved.

## Required delivery and acceptance

- [ ] Audit every Phase 4 scope/gate against current owners, tests and native UI;
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
and core `FileSource`/ordinary-directory pins. Final audit will tie each original
scope item to current tests and observed UI before marking Phase 4 complete.
