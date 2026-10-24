# Constellation controls review

This pass starts at `738da4c`. Each numbered request has a separate implementation
commit; follow-up commits cover integration fixes and acceptance evidence. The
later request for uniform marker sizes at every zoom supersedes the original
final-four-steps-only requirement.

## Request audit

| # | Change | Commit | Evidence |
|---|---|---|---|
| 1 | Persistent quit approval; portable pack saves without symlinks | `449bfa6` | Repeated close-approval test, real CIFS save test, private quit flow |
| 2 | Rack width changes only through its divider | `508e7b5` | 120-frame layout test; persisted manual width survives collapse |
| 3 | Double-click resets compressor sliders | `f8a870e` | Pointer-driven test resets all four parameters |
| 4 | Existing trim/gain/pan controls precede waveform | `a23ba99` | Private UI screenshots; no new automation controls |
| 5 | Remove Fit controls and wheel hints | `b82f7ed` | Editor rendering and screenshots |
| 6 | Space auditions selected editor layer; no Play button | `2cb3e15` | Selected-layer PCM test and collapsed-rack fallback |
| 7 | Selected layer has adjacent X | `d29a85f` | Private layer remove/undo flow |
| 8 | Warn before duplicate layer assignment | `e785e7a` | Cancel/approve/stale-target tests and private popup flow |
| 9 | Pad lists layer prefixes and overflow ellipsis | `5683698` | Compact/tall pad layout tests and screenshots |
| 10 | Direct Audio Studio action replaces layer menu | `dd9001d` | Private studio return with sibling save and unchanged original |
| 11 | No footer; connect MIDI inputs automatically; velocity toggle in Rack | `a2435d1` | MIDI callback/lifetime tests; private UI |
| 12 | Latest MIDI/keyboard input chooses pad labels | `dbd5c1e` | MIDI callback and keyboard tests; default/configured labels |
| 13 | Save rendered edits and originals automatically | `67de780` | Portable manifest/PCM/versioning tests and real CIFS save |
| 14 | Uniform marker sizes at every zoom; bounded extreme-zoom spacing | `a23e5fd`, superseded by `ed209ab` and `c00f69c` | 68,055-point mesh, uniform-radius and coincident-point tests |
| 15 | Outlined category buttons | `2c53a14` | Private dark/light screenshots |
| 16 | Outlined Hover audition and Bypass | `80634f8` | Private dark/light screenshots |
| 17 | Shared soft knee and makeup DSP/graph | `01af6f6` | Continuity, monotonicity, stereo, bypass and safety-ceiling tests |
| 18 | Unbound N advances to next unplayed | `ee42a8d` | Bound/unbound/text-focus and visited-history tests |
| 19 | Collapse hides editor, preserves open state | `687a932` | Camera invariance test and private closed/open editor restoration |
| 20 | Sidebar X and full-height clickable reopen rail | `631e0fc` | Private collapse/reopen flows |
| 21 | Remove Map menu; retain File re-index | `a5f5865` | Private re-index flow |
| 22 | Active/total count in status bar | `b2e8dd3` | Shared predicate count test and private screenshots |
| 23 | Two-handle duration range | `1e38221` | Pointer-driven drag, non-crossing and double-click-reset test |
| 24 | Inline duration and Mono toggle | `7d94889` | Private filter flow and shared filter tests |

## Map regression correction

Local-density radii made sparse outliers larger than neighboring dense regions.
The replacement calculates one radius from the library size, full camera viewport,
and zoom. Filters and docks do not influence it. Every visible sample is retained;
selection and hover still use explicit focus overlays.

The previous iterative spacing pass began at 1.8x zoom. It could move points
repeatedly, hollowing out clusters and rebuilding a spatial table several times per
frame. Ordinary zoom levels now use the original projection directly. Spacing
starts only after `50 / 1.15^4` (about 28.6x), ramping toward the 50x limit. At most
48 candidate cells are checked per colliding point, movement is capped at 64
logical pixels, and earlier points are never pushed again. Saturated neighborhoods
retain overlaps. This bounds worst-case work even for identical coordinates.
Drawing, filtering and hit testing share one path at every zoom.

## Architecture comparison

The architecture service was available. Reuse searches preceded new mechanisms.
`origin/main` was refreshed and remains
`999cd24c9f28814cf06441a1a7be780ac9e5c20c`. Measurements use the service's fast
profile, including tests and scripts, at production-code commit `c00f69c`.

| Metric | Current main | Start of this pass | Reviewed branch |
|---|---:|---:|---:|
| NLOC | 227,320 | 239,914 | 240,358 |
| Functions | 6,934 | 7,254 | 7,262 |
| Sum CCN | 27,909 | 29,392 | 29,396 |
| Average CCN | 4.02 | 4.05 | 4.05 |
| Maximum CCN | 196 | 196 | 196 |
| Duplication | 5.98% | 5.69% | 5.71% |
| Clone groups | 1,382 | 1,393 | 1,401 |
| Reported cycles | 5 | 6 | 6 |
| Health | 73.88 | 62.73 | 62.73 |

Both comparisons report **REGRESSION**, not a clean architectural result. The
whole branch adds indexing, feature extraction, rack playback, MIDI, portable
packs and editor integration. Newly introduced branch hotspots include feature
extraction (CCN 56) and loading (21); the maximum repository CCN is unchanged.
Remaining control-pass hotspots are `render_compressor` (15 to 26) and
`render_pad_grid` (18 to 26), reflecting added parameter interaction, note labels,
and multi-layer labels. They remain local to their UI domains; they do not add
parallel processing or persistence mechanisms. New clone matches in this pass
mostly concern egui input harnesses and PCM/test fixtures. They were inspected;
extracting a generic test framework solely to silence those short matches would
not simplify production behavior.

Concrete simplifications include removing the old pack-reference staging policy,
using the portable installer for all saves, removing obsolete export/clear/menu
paths, removing local density indexing and iterative repulsion, and sharing star
drawing/hit testing. `accept_background` drops below the hotspot threshold (16 to
14); `stage_references` is removed; `render_starfield` is no longer reported as a
high-complexity entity. The unchanged Python cycle can appear as both added and
resolved because its member ordering/path differs; it is not a newly fixed or
introduced Rust dependency cycle. The health score is not evidence that all
branch complexity has been eliminated.

## Validation

The focused constellation suite contains 80 passing tests. The save-policy test
also passes on `/mnt/rust` (CIFS), using a unique temporary directory and ordinary
copies rather than symlinks. It checks unedited bytes, rendered PCM, retained
originals, neutral rendered metadata, relative paths and no-overwrite versions.

Private UI automation uses isolated Weston/Niri sockets, HOME/config/data,
fixture files and an ALSA null sink. It does not inject input into the interactive
compositor. Dark/default keys and light/hardware-Dvorak mappings exercise the same
scenario. Screenshots and file assertions cover the controls and studio/save
flows. Physical MIDI hardware and the user's ALSA backend are not tested by the
null-sink runner; MIDI event behavior is covered by device-free callback tests.

Final commands and results:

- `cargo test -p clip-workbench --lib file_manager::constellation -- --test-threads=1`: 80 passed.
- `cargo test --workspace -- --test-threads=1`: 2,553 passed, 23 ignored across 26 targets.
- `LITTLE_CLIP_PACK_TEST_ROOT=/mnt/rust cargo test -p clip-workbench --lib automatic_pack_save_copies -- --test-threads=1`: passed on CIFS.
- `cargo fmt --all --check`, `git diff --check`, Python compilation of the acceptance script: passed.

The initial default-parallel workspace run failed in unrelated asynchronous and
process-environment tests. The first serial attempt exposed an existing OCR test
race: it waits for PID-file existence but can parse the still-empty file. The
final complete serial run passes. Those failures are recorded rather than
attributed to the constellation renderer.

- `cargo build -p clip-workbench --bin clip-file-manager`: passed; the two existing float-inference warnings remain in unrelated UI modules.
- Private dark/default-key scenario: passed, 36 observations (`/tmp/constellation-controls-dark-completed`).
- Private light/hardware-Dvorak scenario: passed, 36 observations (`/tmp/constellation-controls-light-accepted`).

Both private scenarios verify normal process exit after Discard, unchanged source
bytes after Studio return, two separate portable save versions, layer removal and
undo, and no `POLLERR` in their null-sink playback logs. OCR misses and border-bound
coordinates in intermediate attempts were corrected with known button centers
and stable state assertions; no product behavior was weakened to make the runner
pass. The dense-map regression tests cover 68,055 rendered points at six zooms,
including coincident inputs. They establish equal marker radii and bounded work;
they are not a GPU frame-rate benchmark on the user's full library.

## Follow-up: six UI corrections

The next review identified six concrete issues. Each has a separate implementation
commit, followed by shared-widget accessibility, regression coverage and documentation.

| Request | Resolution | Commit |
|---|---|---|
| Per-layer close controls and useful tab names | Filename prefixes replace numbered labels; every tab has its own X. Closing an inactive tab preserves the active sample. | `401fba5` |
| Missing sound controls in large windows | Remove the separate wide-editor layout. Sound stays above the waveform at every width. | `a7b7181` |
| Symmetric collapse/expand controls | Use full-height minus/plus rails at the same right edge; the expanded rail fits inside the existing sidebar width. | `a1ff9b1` |
| Redundant trim controls and text | Remove numeric start/end controls, selected-duration text and the marker hint; keep the waveform's draggable IN/OUT markers. | `0083c4f` |
| Filtered points too bright | Reduce excluded marker alpha from 25/28 to 12 while retaining all markers. | `e4a352f` |
| Square, low-contrast active filters | Keep neutral fills and rounded corners. Active category borders use the category color; All, Hover audition and Mono use the theme accent. | `1c4eafd` |

The new layout test checks that filename tabs and Gain, Pan, Attack and Release
text are fully inside the clipping rectangle at widths 840, 1400 and 2200, in
both appearances. A pointer test closes an inactive shared tab and verifies a
close action without a selection change. Existing manual rack-width and camera
invariance tests still pass. The workbench library suite passes 1,300 tests with
9 ignored; all five shared-tab tests pass after isolating the close widget.

The rebuilt binary passes both private compositor scenarios, each with 36
observations: `/tmp/constellation-six-dark-accepted` and
`/tmp/constellation-six-light-final` (hardware-Dvorak keys). The runner now uses
filename tabs and the new rails. Grid gesture spacing follows the rendered pad
stripe instead of assuming the old pad width. Geometry assertions cover sound
control visibility independently of OCR, which can miss the Pan label in dark
screenshots even when it is visibly present.

Architecture measurements were repeated against `constellation-followup-six`
(starting at `59156da`) and refreshed main, still `999cd24`. The follow-up adds
99 NLOC and six functions, including two regression tests; duplication remains
5.71%, average CCN 4.05, maximum CCN 196, and reported cycles six. Two additional
clone groups are test-harness matches. The tool still reports **REGRESSION**.
Shared tab painting rises from CCN 12 to 18 because of optional per-tab close
geometry and action handling; the close target itself has a separate painter,
accessible label, tooltip and focus feedback. Extending the shared tab bar avoids
a constellation-only navigation fork. The acceptance scenario grows from CCN 33
to 36 for its new targeting checks.

For the entire branch against current main: NLOC 227,320 → 240,457, functions
6,934 → 7,268, sum CCN 27,909 → 29,426, duplication 5.98% → 5.71%, clone groups
1,382 → 1,403, reported cycles five → six, and health 73.88 → 62.73. This preserves
the earlier branch-wide regression finding; the six UI fixes do not erase the
indexing and feature-extraction hotspots documented above.

Final follow-up validation: `cargo test --workspace -- --test-threads=1` passes 2555 tests with 23 ignored across 26 targets. The file-manager binary builds, and formatting, diff whitespace and acceptance-script compilation checks pass.
