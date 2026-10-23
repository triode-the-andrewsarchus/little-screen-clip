# Constellation editor review

Review date: 2026-10-23. This pass starts at `f9cce81` on
`feat/sample-constellation-view`. Current main was fetched and verified at
`999cd24c9f28814cf06441a1a7be780ac9e5c20c`.

## Changes

- `625b4b5`: project the map against the full canvas before docks obscure it.
  Relaxation includes obscured points too, preventing nearby exposed points from
  shifting. Painting and pointer interaction remain clipped to the exposed area.
- `9e26221`: Space retriggers the map selection through the existing guarded
  streaming loader. Shift+Space and Neighbor skip samples already auditioned since
  the last map click. Exploration respects the shared filters and stops on exhaustion.
- `530bd1e`: enlarge and frequency-color the waveform using core's `WaveformColor`,
  inset the trim handles, add double-click reset, and edit one layer at a time.
- `71f0676`: expose live threshold and ratio controls, persist them with the pack,
  and show gain reduction. Existing packs keep the previous subtle defaults.
- `ba73233`: compact the footer and editor controls so all sixteen pads remain
  visible at the acceptance window size. Overflow uses a reserved scrollbar gutter,
  keeping the end handle accessible. Share the rendering test harness and check
  actual visible star coordinates at both coarse and relaxed zoom.
- `4f28f92`: retain the last valid compressor controls during non-finite numeric
  edits, preventing an invalid envelope from propagating into audio output.
- `c9898ab`: exercise the inset end handle with actual egui pointer events.
- `38d5f8c`: preserve stereo transients in editor waveform peaks and colors.

The subsequent waveform review also corrects opposing-phase stereo samples
appearing silent: peaks span all channels and each channel has its own shared
crossover filter. Mono snapping data remains separate.

## Architecture

The implementation extends existing state, playback, persistence and UI mechanisms.
It adds no production module or dependency. The exploration set is necessary because
the fading playback trail intentionally discards old entries; the trail cannot
provide a complete visited set. Output controls use shared atomics without opening
another hardware stream. Waveform preparation stays on the bounded PCM worker.

Both comparisons use Architecture Fitness's fast, diff-aware profile:

| Metric | Current main | Before this pass | After this pass |
| --- | ---: | ---: | ---: |
| NLOC | 227,320 | 238,233 | 238,628 |
| Functions | 6,934 | 7,200 | 7,210 |
| Total CCN | 27,909 | 29,165 | 29,209 |
| Average CCN | 4.02 | 4.05 | 4.05 |
| Maximum CCN | 196 | 196 | 196 |
| Duplication | 5.98% | 5.75% | 5.74% |
| Clone groups | 1,382 | 1,394 | 1,394 |
| Reported cycles | 5 | 6 | 6 |
| Valknut health | 73.883 | 62.735 | 62.729 |

The complete branch remains a **REGRESSION** against main. Its existing classifier,
map renderer and database loader remain hotspots; the full comparison calls out
`extract_features` (CCN 56), `load_samples` (21), `classify_sample` (17), and
`discover_audio_paths` (17), along with owner-side dispatch growth. Lower duplication
percentage alone does not clear that debt. The reported extra cycle belongs to
the Python model converter's bounded recursion, rather than a Rust module cycle.
Different absolute worktree prefixes also make unchanged Python cycles appear
added/resolved; those are not independent architectural changes.

This pass adds 395 NLOC and ten functions, including tests. Average/max CCN, clone
groups and cycle count are unchanged. Its comparison still says **REGRESSION**:
total CCN rises by 44, and the acceptance `exercise` function grows from CCN 44 to
46 with extra OCR coordinate fallbacks. These are not a clean architecture acceptance.
The added control validation, traversal state and channel-safe peaks implement
requested behavior rather than parallel mechanisms. Two intermediate extra clone
groups were removed by sharing the rendering harness. The final tool also lists
existing clone spans touching the modified UI file; the final clone count is flat.

Raw comparison results are in `/tmp/constellation-editor-architecture-main.json`
and `/tmp/constellation-editor-architecture-delta.json`.

## Verification

Private Weston/Niri acceptance runs cover dark and light appearance, including
the hardware-remapped Dvorak configuration. Both passed 26 observations:

- `/tmp/constellation-editor-dark-final/`
- `/tmp/constellation-editor-light-first/`

They verify indexing and incremental reuse, configured pad assignment/retrigger,
real Copy-only native drops and layering, source preservation, output toggle
persistence, versioned saves, filters, portable WAV export, undo/redo, close
protection, inline packs and navigation. OCR expectations follow the new layout;
saved metadata and exported files provide the behavioral assertions.

Validation commands and results:

- `cargo test --workspace -- --test-threads=2`: 2,535 passed, zero failed,
  23 ignored across 26 targets, including doctests. The final stereo waveform
  correction was subsequently checked with the focused suite below.
- `cargo test -p clip-workbench --lib constellation -- --test-threads=2`:
  65 passed on the final code, including the stereo regression and handle test.
- `cargo build -p clip-workbench --bin clip-file-manager`: passed.
- `cargo fmt --all --check`, `git diff --check`, and Python acceptance-script
  compilation: passed.
- `cargo doc -p clip-workbench --no-deps --document-private-items`: passed.
  Inspected the generated bank controls, audition contracts and live dynamics
  documentation. The unrelated bare-URL warning in `image_strip/hilbert.rs` remains.
- Diagnostic `cargo clippy -p clip-workbench --lib -- -A clippy::approx_constant`:
  passed with the existing 583-warning workbench backlog. This is not a clean
  strict-Clippy result; the allowance is command-local for the existing
  `cmyk_halftone.rs` approximate-constant blocker, with no source suppressions.

Rendering tests compare visible mesh coordinates while toggling both docks at
coarse and relaxed zoom. Other focused regressions cover repeated Space selection,
neighbor exhaustion/reset/filtering, tiny waveform buckets, stereo phase
cancellation, compressor response/stereo linking/bypass, legacy defaults and
settings round trips. Pointer input verifies that dragging the inset OUT handle
changes only the end time.

Physical ALSA/MIDI devices and subjective compressor quality remain unverified.
Acceptance uses an isolated null audio sink and does not prove a physical device
is free of `POLLERR`. Backend buffer sizing remains untouched.
