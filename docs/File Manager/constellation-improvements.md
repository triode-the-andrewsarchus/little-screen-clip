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
- [ ] Pad loading/missing/playing/choke indicators without permanent visual clutter.
- [ ] Selected-pad editor with visible essential controls, numeric trim, mute/solo, pan, fades.
- [ ] Incremental indexing and duration/channel filters, with indexed/new status.

Baseline: `constellation-eight-improvements` at `c5524ae`.
