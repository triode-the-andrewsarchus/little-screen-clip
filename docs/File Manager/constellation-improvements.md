# Constellation improvements

The requested scope is eight independently committed improvements. Completion
requires behavior and regression evidence for each item, followed by workspace,
architecture, and private compositor validation.

- [x] Undo/redo, dirty indication, and protection when loading or closing an unsaved pack.
  Metadata-only 32-step history, gesture coalescing, inline save/discard/keep-editing
  admission, and snapshot-specific asynchronous save completion.
- [ ] Map/file-list sample dragging; normal drop replaces, Shift+drop layers, with pad feedback.
- [ ] MIDI playback independent of UI painting and navigation load.
- [ ] Shared output for map, layer, library, and pad auditions with consistent loudness/protection.
- [ ] Portable trimmed-WAV kit export, with optional collected original files.
- [ ] Pad loading/missing/playing/choke indicators without permanent visual clutter.
- [ ] Selected-pad editor with visible essential controls, numeric trim, mute/solo, pan, fades.
- [ ] Incremental indexing and duration/channel filters, with indexed/new status.

Baseline: `constellation-eight-improvements` at `c5524ae`.
