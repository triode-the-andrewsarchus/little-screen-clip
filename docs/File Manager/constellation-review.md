# Sample constellation branch review

Reviewed the six feature commits after `main` (`999cd24`), in their original order.
Fixes are appended to the branch; the feature history is preserved.

## Commit-by-commit findings

| Feature commit | Findings and changes |
| --- | --- |
| `d6aa9b6` — initial map/indexer | Fixed subtree queries that matched similarly named siblings and interpreted `%`/`_` as wildcards. Prevented an index completion for another folder from replacing the visible map. Guarded directory/audio inputs through core's ordinary-content policy, retained complete source duration when available, persisted empty re-indexes, and reported worker disconnection/spawn failures. Removed a test that indexed a personal directory into the real database. Folder database loads now run on the bounded executor. |
| `4daf65e` — history trail | Reviewed the adaptive count and duplicate suppression; existing tests pass. Refreshes reset obsolete trail/selection state, and the newest-point glow uses the correct alpha representation. |
| `be74200` — dimmed stars/File menu | Fixed an overflowing `u8` density counter in dense active clusters. Dimmed search/category results no longer capture audition targets. Pointer interaction respects the canvas response, including overlapping windows and dragging. The File menu delegates indexing controls to a focused menu method. |
| `9f587a7` — hover waveforms/specifications | Removed synchronous fallback probes from the HUD. Hover requests/results have bounded queues, cooperative cancellation, generation checks, guarded inputs, and a 15-second waveform limit. Consolidated WAV/AIFF chunk scanning, validated FLAC STREAMINFO length and AIFF sample rate, and removed unsafe UTF-8 slicing from paths/names. |
| `b86426b` — pack shelf | Replaced destructive in-place saves with staged atomic no-overwrite installation; repeated saves create numbered directories. Rejected `.`/`..` names, propagated link/write failures, validated bank sizes/slots/layers/trim values, and guarded output/source handles. Pack listing/loading/saving/deleting run on workers. Browser metadata is retained instead of reloaded every frame. Accepted mutations retain completion and defer application close. |
| `82e72a0` — inline trims/direct playback | Reverted the forced shared 512/1024-frame output buffers and restored the browser's background audition loader. Connected slot voices to navigation/view/studio/Escape stop hooks. Playback references shared, frame-aligned PCM instead of copying trim buffers. PCM preparation is bounded and off the UI thread, stale bank/list/audio results are rejected, Unicode labels are safe, and Escape clears constellation search after egui drops focus. |

The fixed output buffers are the leading explanation for the reported ALSA
`alsa::poll()`/`POLLERR` regression: that commit changed the application-wide sink.
The private acceptance run uses an ALSA null device and cannot establish that a
particular physical audio backend no longer produces the error. Restart Files to
replace an already-open stream with the restored backend configuration.

## Follow-up audit

`881895d` retains job inputs through rejected executor admission and retries only
`QueueFull`, which proves the operation has not run. Completed or disconnected
mutations are never retried. Superseded reads are canceled and removed before new
admission, stale errors use the same validity rules as successful results, and a
memory-budget refusal no longer permanently marks a source as undecodable. Browser
close releases its PCM and invalidates preview requests. Pending work uses the
existing repaint scheduler so progress and admission retries do not wait for input.
A saturated one-worker executor test verifies that a pending save eventually runs
exactly once. The slot header keeps its clickable numbered pad, fits the title to
available width, and shows distinct saved-version directory names in the browser.
Coarse rendering now caps foreground and background stars independently; a dense
1,000-sample test checks bounded mesh size and preserved category/search matches.

`d04798f` separates directory discovery, parallel analysis, and cache publication.
Entry/open failures and joined worker panics cannot commit a partial replacement;
unsupported/silent content remains a deliberate classification omission. Directory
identity tracking stops bind-alias cycles, the pool reserves one reported CPU when
possible and uses at most eight workers, and progress counts completed analyses.
Indexing and PCM preparation share rate/channel limits before allocation sizes are
derived from headers. SQLite schema version six preserves Unix path bytes, and all
legacy migrations commit their data changes and version markers together. Tests
cover invalid UTF-8 paths distinct from their display-equivalent names, legacy TEXT
migration, forced migration rollback, excluded discovery trees, inaccessible audio,
and a decoder-accepted over-budget sample-rate header.

The small remaining increases in discovery/repaint complexity implement explicit
I/O failure and liveness checks. The review keeps those checks rather than hiding
them behind generic wrappers to change a metric. The index coordinator itself fell
from CCN 29 to 4; new migration code has named, version-specific responsibilities.

## Pad assignment and retrigger follow-up (`2eb9a41`)

This follow-up made clicking a map sample followed by Shift+number assign it even
when a canvas control has keyboard focus. Text editors still keep their keys.
The vendored input adapter preserves physical keys for shifted characters such
as `!`, which egui cannot represent as logical keys. Pad handling prefers physical
digit positions and falls back to logical digits for virtual keyboards; it uses
each event's modifiers rather than the frame's final modifier state.

Each pad now owns its own layered voice group. Retriggering stops that pad's
previous voices before starting ready layers; other pads can keep ringing.
Navigation and Escape retire all groups, and the repaint scheduler keeps polling
until their voices finish. A device-free rodio mixer test verifies that both layers
of one pad stop while another pad remains audible, then verifies global stopping.
The private UI scenario clicks the actual star, assigns two pads, repeats triggers
with a two-second fixture, and verifies both assignments in saved metadata. The
null device cannot establish perceptual audio quality or physical ALSA behavior.

Against `constellation-pad-fixes-start`, this follow-up adds 152 measured lines,
two regression tests, and 19 total CCN; duplication remains 5.81%, maximum CCN
remains 196, and cycle count remains six. The extra clone group is the literal
number-key enumeration also present in Audio Studio; the existing list there has
a different ordering and is private. Other touched-file clone findings include
existing worker code and egui test setup. No new audio backend, decoding pipeline,
or generic voice abstraction was introduced.

## Integrated rack and UI review

The map now shares the file manager canvas, with category colors confined to
sample identity. Default fitting includes organic coordinates beyond the unit
square, fixing a sample clipped out of the narrower map. Indexing reuses the depth
view's themed progress-bar treatment. Rack and Packs use the shared tab bar in
one side panel; the floating pack browser is gone. The selected pad's resizable
layer editor sits below the map, and MIDI/velocity/output controls share a footer.

The rack has sixteen pads. Default rows are ZXCV / ASDF / QWER / 1234, bottom to
top. Validated `file_manager.constellation_pad_keys` rows override them directly;
logical, unshifted input also preserves hardware-remapped punctuation absent from
egui's fixed key enum. Pad labels follow this local configuration independently of
saved packs. Older ten-slot banks keep their samples and gain six empty pads.

An opt-in midir worker owns controller connections, with bounded event delivery
and stale-epoch rejection across preview stops. Notes 36–51 retain velocity unless
overridden to 127. Pads can share choke groups. A linked stereo pad bus adds 2 dB
headroom, gentle dynamics, a peak ceiling, reduction metering, and bypass without
changing device buffer policy. Configuration and playback contracts are detailed
in [constellation-drum-rack.md](constellation-drum-rack.md).

Against `constellation-pad-ui-start`, the necessary MIDI/input/dynamics mechanisms
and regression coverage add 674 measured lines and 119 total CCN. Average CCN drops
from 4.06 to 4.05, duplication drops from 5.81% to 5.79%, maximum CCN stays 196,
clone count stays 1,394, and cycle count stays six. Shelf rendering drops from CCN
50 to 7; the inline browser is CCN 16 versus the old 25. The tool still labels the
comparison REGRESSION, including clone findings in existing egui test scaffolding;
this is not being presented as a clean architecture acceptance. The new two modules
have separate responsibilities: controller lifecycle and a shared stereo output bus.

The workspace suite passes 2,516 tests with 23 existing ignores. All 47 focused
constellation tests pass; a separate adapter test verifies punctuation retention
and expiry, and core configuration tests cover exact hardware-remap rows plus
invalid/duplicate bindings. Private dark/default and light/hardware runs pass actual
star selection, repeated pad hits, saved output settings, version preservation,
inline Packs navigation, and exit through an ALSA null sink. Physical controllers
and perceptual audio quality remain outside that automated evidence. Clippy runs
successfully with existing repository warnings; it is not warning-clean.

## Architecture comparison against current main

The baseline was measured from a clean detached worktree at `999cd24`, not from
the feature branch. The same fast profile was used for every measurement.

| Metric | Main | Original feature branch (`82e72a0`) | Reviewed branch |
| --- | ---: | ---: | ---: |
| Total cyclomatic complexity | 27,909 | 30,006 | 28,888 |
| Average cyclomatic complexity | 4.02 | 4.26 | 4.05 |
| Maximum cyclomatic complexity | 196 | 196 | 196 |
| Lines of code | 227,320 | 238,554 | 236,542 |
| Duplication | 5.98% | 5.76% | 5.79% |
| Clone groups | 1,382 | 1,390 | 1,394 |
| Reported cycles | 5 | 5 | 6 |
| Valknut health | 73.88 | 62.65 | 62.71 |

The architecture tool still classifies the whole feature branch as **REGRESSION**
against main. This is not a clean architecture acceptance result.

The largest simplification replaces twenty nested generated tree evaluators
(CCN 52–84 each) with static model nodes and one shared traversal. Original and
converted predictions matched bit for bit for 5,452 reachable-leaf and adjacent
split-boundary inputs. A committed golden fingerprint and topology checks guard
that conversion; `scripts/compact_constellation_model.py` reproduces it from the
original export. Classification weights, thresholds, category order, and floating
point addition order are preserved. Naming priors now use an ordered table, with
word boundaries for short abbreviations (`subdivision` and `what` are regression
fixtures). Its CCN fell from 82 to 9. PCM loading fell from 17 to 3; shared chunk
parsing removes duplicated container traversal.

Remaining hotspots include feature extraction (CCN 56), map rendering (62), shelf
rendering (50), pack browser rendering (25), and SQL record decoding (20). The
record decoder now reports invalid rows rather than silently discarding them.
These are disclosed maintenance costs; this review does not claim they disappeared.

The extra reported cycle is the converter's local recursive `block` parser for a
bounded-depth tree, not a Rust module dependency cycle. Other added/resolved cycle
entries name unchanged scripts under different absolute worktree prefixes; they
must not be interpreted as new cross-module dependencies. Clone findings include
static model data and existing clones in touched files, as well as test fixtures.

## Validation and evidence

- `cargo test --workspace -- --test-threads=2`: 2,516 passed, zero failed,
  23 ignored, including workspace doctests.
- All 47 focused constellation regression tests pass and cover database boundaries, stale results,
  protected inputs, malformed audio/banks, save preservation, shared trim storage,
  Unicode labels, Escape handling, model equivalence, and mutation completion.
- `cargo build -p clip-workbench --bin clip-file-manager` passes.
- `cargo fmt --all --check` and `git diff --check` pass.
- `cargo doc -p clip-workbench --lib --no-deps --document-private-items` passes.
  Rendered module, save, and slot documentation was inspected. Rustdoc reports one
  pre-existing bare-URL warning in `file_manager/image_strip/hilbert.rs`.
- The private Weston/Niri constellation acceptance workflow passes indexing,
  Ctrl+F/search dismissal, slot assignment, repeated saves preserving owned files,
  pack browsing, navigation, and clean exit. It uses isolated HOME/XDG directories
  and an ALSA null device; it never injects input into the user's compositor.

Private-run screenshots and its machine-readable report are retained at
`/tmp/constellation-rack-dark-final-pass/` and
`/tmp/constellation-rack-verified-light/`. The full test log is
`/tmp/constellation-rack-workspace-final.log`. Run the UI scenario with an installed
Tesseract English model, or point `TESSDATA_PREFIX` at one:

```bash
python3 scripts/constellation_acceptance.py \
  --output-dir /tmp/constellation-acceptance-new --appearance dark
```

Pack persistence remains reference-based: symlinks refer to complete source files,
while trimming and volume live in `bank.json`. It is not a trimmed-WAV exporter.
PCM caches retain at most 15 seconds/two million interleaved values per source and
use an aggregate admission budget. Model training provenance and perceptual
classification quality cannot be established from the checked-in export alone.
