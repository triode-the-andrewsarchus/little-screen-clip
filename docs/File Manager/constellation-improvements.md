# Constellation improvements

The requested scope is eight independently committed improvements. Completion
requires behavior and regression evidence for each item, followed by workspace,
architecture, and private compositor validation.

- [x] Undo/redo, dirty indication, and protection when loading or closing an unsaved pack.
  Metadata-only 32-step history, gesture coalescing, inline save/discard/keep-editing
  admission, and snapshot-specific asynchronous save completion.
- [x] Map/file-list sample dragging; normal drop replaces, Shift+drop layers, with pad feedback.
  Copy-only native targets, guarded worker decoding, atomic assignment, stale-pad
  rejection, eight-layer/PCM limits, and undo support. Focused suite: 52 passed.
  Architecture review found necessary reference-drop routing growth; map drag and
  target resolution now have cohesive handlers instead of growing event loops.
- [x] MIDI playback independent of UI painting and navigation load.
  Shared prepared rack dispatches directly from the input callback; UI telemetry
  is best effort. Stop and callback epoch checks share the playback lock. No PCM
  decoding occurs in callbacks. Device-free callback/audio regression: 53 tests pass.
- [x] Shared output for map, layer, library, and pad auditions with consistent loudness/protection.
  Music retains its guarded streaming loader and delegates only playback to the
  rack bus. Layer/library auditions share pad headroom, Glue and bypass. Audio
  comparison regression plus file-manager suite: 602 passed, one ignored.
- [x] Portable trimmed-WAV kit export, with optional collected original files.
  Float WAV layers bake trim/gain, relative manifests survive relocation, and
  optional originals are regular copies. Shares pinned atomic installer; accepted
  exports retain completion. Relocation/failure/version tests: 55 focused pass.
- [x] Pad loading/missing/playing/choke indicators without permanent visual clutter.
  Compact loading spinner, unavailable/playing dots, and nonzero choke labels.
  Failure tooltips and selected-pad Retry audio action use existing worker state,
  with no filesystem probing in paint. Focused suite: 55 passed.
- [x] Selected-pad editor with visible essential controls, numeric trim, mute/solo, pan, fades.
  Essential controls precede the waveform. Shared stereo balance/envelope rendering
  feeds both live layers and export; mute/solo gates active voices immediately.
  Legacy serde defaults preserve sound. DSP, export, validation and live-gate
  regressions pass (58 focused tests). Pad painting/header responsibilities were
  separated after measurement flagged growth in the interaction handlers.
- [x] Incremental indexing and duration/channel filters, with indexed/new status.
  Schema 7 persists FileSource identity/size/nanosecond timestamps. Complete scans
  reuse matching classified observations, analyze new/changed files, and atomically
  remove disappeared rows. Legacy/unclassified files are analyzed on the next scan.
  Progress/completion report reused vs newly analyzed counts. Duration and channel
  filters share the map, hit-test, random and neighbor predicate. Same-size/inode
  replacement, restored-mtime rewrite, migration and cancellation tests pass
  (59 focused tests).

Baseline: `constellation-eight-improvements` at `c5524ae`.

## Commits

| Improvement | Commit |
| --- | --- |
| Undo and unsaved-pack protection | `65126c5` |
| Copy-only sample drops | `e522f71` |
| MIDI triggering outside UI painting | `3e5f0b8` |
| Shared protected audition output | `2e537f6` |
| Portable WAV kits | `13ab698` |
| Pad readiness and playback cues | `5c5febf` |
| Precise pad editing and envelopes | `da0a3aa` |
| Incremental indexing and audio filters | `d86b0d2` |

Private compositor verification additionally caught clipped bottom pads and hidden
fade controls. The follow-up fix compacts the parameter chrome, adapts pad heights,
and keeps export options in the pack's Export menu. A frame-based duration fix
preserves sub-millisecond hits through preparation and WAV export. Internal rack
stops now retire the browser-owned map loader too, preventing a pending audition
from restarting after undo, bank replacement, or switching to Packs.

## Whole-branch architecture review

Freshly fetched `origin/main` is `999cd24`; the comparison includes the entire
feature branch, not just these eight commits. Measurements use the architecture
server's `fast` profile and its diff-aware comparison.

| Dimension | Current main | Before these eight changes | Final working tree |
| --- | ---: | ---: | ---: |
| Lines of code | 227,320 | 236,542 | 238,233 |
| Functions | 6,934 | 7,135 | 7,200 |
| Total cyclomatic complexity | 27,909 | 28,888 | 29,165 |
| Average cyclomatic complexity | 4.02 | 4.05 | 4.05 |
| Maximum cyclomatic complexity | 196 | 196 | 196 |
| Duplication | 5.98% | 5.79% | 5.75% |
| Clone groups | 1,382 | 1,394 | 1,394 |
| Reported cycles | 5 | 6 | 6 |
| Valknut health | 73.88 | 62.73 | 62.73 |

**The tool still classifies the full branch and the eight-change delta as
REGRESSION. This is not a clean architecture acceptance result.** The new feature
surface adds necessary document history, reference-drop admission, shared layer
rendering, and export behavior. Mean/max complexity and clone count stayed stable
across this pass; duplication declined. No new Rust module cycle was identified.
The extra reported cycle is the existing converter's bounded recursive `block`
parser. Added/resolved entries for unchanged scripts use different absolute
worktree prefixes and do not establish new dependencies.

Remaining product hotspots include `ui.rs:404::render_starfield` (CCN 62),
`classifier.rs:31::extract_features` (56), `ui.rs:809::render_sample_hud` (23),
`ui.rs:167::render_toolbar` (21), and `db.rs:260::load_samples` (21). The last grows
by one checked SQLite field. Input admission, native drop routing, pad interaction,
background completion, and rack/export dispatch retain smaller CCN 16–19 hotspots.
The acceptance scenario itself has CCN 44 from its assertions and fixture checks.
These costs remain disclosed rather than being hidden behind arbitrary wrappers.

Reuse searches preceded new mechanisms. This pass extends existing `FileDrag`,
native Copy-only targets, `Music` preparation, shared audio sink, pinned `FileSource`
identity, bounded jobs, and the atomic pack installer. Hound replaces a provisional
handwritten WAV serializer. Live layers and export share one balance/fade source.
Pad status painting and selected-pad headers were separated from interactions
after the tool flagged concentrated growth; the pack browser dropped below the
high-complexity threshold (CCN 16 → 14). Remaining clone findings include model
data and test/setup fragments; the clone-group count has no net increase in this pass.

Raw comparisons are retained at
`/tmp/constellation-eight-architecture-reviewed-main.json` and
`/tmp/constellation-eight-architecture-reviewed-delta.json`.

## Final verification

- `cargo test --workspace -- --test-threads=2`: **2,530 passed, 0 failed, 23 ignored**,
  including doctests. Log: `/tmp/constellation-workspace-reviewed.log`.
- File-manager regression suite: **609 passed, 1 ignored**; the constellation
  subset has **60 passing tests**. This includes actual mixer samples, callback
  dispatch without UI polling, same-pad/choke isolation, stale input epochs,
  cache revision invalidation, portable relocation, short-hit export, and retiring
  a pending map load without cancelling a newly chosen Music preview.
- Private Weston/Niri acceptance: **24 observations passed in each appearance**.
  The light run uses `[';qjk', 'aoeu', "',.p", '1234']` and checks the apostrophe
  pad's persisted assignment. Both runs verify real native Copy-only replace and
  Shift-layer drops, preserved sources, versioned saves, incremental reuse,
  channel filters, WAV export, undo/redo, close protection, Packs, and navigation.
  Reports, screenshots, saved banks and exported WAVs remain under
  `/tmp/constellation-eight-dark-reviewed/` and
  `/tmp/constellation-eight-light-reviewed/`. Their ALSA null devices report no
  `POLLERR`; physical ALSA/MIDI interoperability and listening remain unverified.
- `cargo build -p clip-workbench --bin clip-file-manager`, `cargo fmt --all --check`,
  `git diff --check`, and Python compilation of the acceptance script pass.
- `cargo doc -p clip-workbench --lib --no-deps --document-private-items` passes.
  Rendered export, layer-source and FileSource pages were inspected. One existing
  bare-URL warning remains in `file_manager/image_strip/hilbert.rs`.
- Standard Clippy stops at the existing `approx_constant` error in
  `little-clip-ops/src/creative_fx/cmyk_halftone.rs:144`. The diagnostic run with
  `-A clippy::approx_constant` completes with the existing 583 workbench warnings;
  no lint suppression was added to source. Callback gate ownership and read
  cancellation were simplified to remove the new actionable warnings.

The implementation details and usage contracts are documented in
[`constellation-drum-rack.md`](constellation-drum-rack.md). The earlier original-
branch audit remains in [`constellation-review.md`](constellation-review.md).
