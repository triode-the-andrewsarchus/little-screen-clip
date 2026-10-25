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

## Follow-up: compact mixer, output history and pitch

Each requested behavior has its own implementation commit. The final integration
commit adds gesture regressions, keeps the Studio button visible, separates
coincident fade flags, and updates these notes.

| Request | Resolution | Commit |
|---|---|---|
| Compact editor | Left-side vertical dB fader, detented Pan knob, and draggable A/R flags above the waveform; keep fade guides. | `d75a413` |
| Compressor history | Audio-clock level peaks and gain-reduction trace replace the moving level dot. History overlays the transfer graph, keeping all four parameter controls visible. | `8c15a16` |
| Middle-drag over samples | Reserve file drags for primary-button gestures and suppress hover audition through pan release until fresh pointer movement. | `048680a` |
| Per-pad retrigger | Square R toggle defaults on; off permits overlapping hits with a bounded oldest-voice limit. Choke groups still stop other members. | `73bd14f` |
| Mute-link painter order | Reserve a background shape before drawing any pad faces; place every connecting segment there. | `58a2b24` |
| Per-layer repitch | Detented ±24-semitone knob, P/T mode, duration-preserving worker-side phase vocoder, and matching baked exports. | `53ebf27` |

The mixer stays inside a 96-pixel strip at every editor size. The right sidebar
keeps its existing default width and limits. Gain has marked dB positions; Pan
and Pitch reset on double-click. The knobs retain raw drag accumulation through
the center detent, so slow motion can leave the detent. Fade handles clamp to the
trimmed span and use separate lanes when they meet. A pointer regression verifies
that dragging attack changes the envelope without changing trim.

P is the default for new and legacy layers. Prepared PCM belongs to the existing
bounded cache and background queue. Stale pitch/source results are rejected;
Studio replacement invalidates both source and pitch caches. No new executor,
audio-device stream, or package dependency was introduced. The existing realfft
dependency supplies the STFT. The audio callback only borrows prepared PCM. Export
uses pinned inputs and the same pitch/trim/gain/fade pipeline; rendered parameters
reset to neutral while P/T mode is retained. The basic phase vocoder can color
transients at large shifts; listening quality is not established by frequency
and duration assertions alone.

### Validation

- Focused constellation tests: 89 passed. They include pitch peaks at ±12/24 semitones, exact frame counts, stereo alignment, short hits, stale preparations, P/T export-to-preview PCM equality, retrigger/overlap, bounded meter history, middle-drag over a sample, background link ordering, fade gestures, and editor geometry in both themes at 840, 1400 and 2200 pixel widths.
- Full workspace: 2,563 passed, 23 ignored across 26 targets (`/tmp/constellation-mixer-workspace.log`). This run preceded the final localized fade-lane refinement; all 89 focused tests passed again afterward (`/tmp/constellation-mixer-final-focused.log`).
- Final file-manager binary build passed (`/tmp/constellation-mixer-build-final.log`); the two pre-existing float-inference warnings remain outside the constellation code.
- Private Weston/Niri scenarios passed in dark/default keys and light/hardware-Dvorak keys, 36 observations each (`/tmp/constellation-mixer-dark`, `/tmp/constellation-mixer-light`). Screenshots were inspected for mixer, waveform and Output placement. These runs preceded only the coincident-fade lane refinement, which is covered by the focused geometry test.
- Formatting, diff whitespace and acceptance-script Python compilation passed.
- Rustdoc generation (including private modules) and the workbench doctest passed; generated module and PitchMode documentation were checked. The existing unrelated bare-URL warning remains in image_strip/hilbert.rs.

The private runner uses ALSA null, isolated sockets and fixture directories. It
does not establish physical ALSA/MIDI behavior or perceptual compressor/pitch
quality on the user's equipment.

### Architecture review

Refreshed `origin/main` remains `999cd24c9f28814cf06441a1a7be780ac9e5c20c`.
The architecture tool reports **REGRESSION**, both against this pass's baseline
`constellation-mixer-pitch` and against current main:

| Metric | Before this pass | Current | Current main |
|---|---:|---:|---:|
| NLOC | 240,457 | 241,457 | 227,320 |
| Functions | 7,268 | 7,290 | 6,934 |
| Sum CCN | 29,426 | 29,549 | 27,909 |
| Average CCN | 4.05 | 4.05 | 4.02 |
| Maximum CCN | 196 | 196 | 196 |
| Duplication | 5.71% | 5.70% | 5.98% |
| Clone groups | 1,403 | 1,408 | 1,382 |
| Reported cycles | 6 | 6 | 5 |
| Health | 62.73 | 62.71 | 73.88 |

New or enlarged hotspots in this pass are `ui::render_starfield` (27 → 32),
`background::accept_background` (14 → 18), `output::trigger` (16 → 19),
`pack_export::render` (16 → 21), and the new phase-vocoder `pitch::shift` (24).
These implement pan/hover gesture arbitration, stale-result validation,
retrigger/choke/voice bounds, export duration safety, and the STFT respectively.
The clone report includes repeated headless egui setup in UI regression tests;
repeated test setup is retained rather than adding a production abstraction.
There is no new reported cycle count in this pass. The tool reports the unchanged
Python video benchmark cycle as both added and resolved because its member order
changed; that is not a cycle fix. PitchMode lives with pack metadata, avoiding a
model-to-DSP dependency solely to represent a persisted enum.

Reuse searches and source inspection led to the existing audio bus, cache budget,
background queue, waveform viewport, fade envelope, FFT dependency, shared tabs,
and guarded export installer. Custom work is confined to the requested mixer
widgets, single-band history ring and pitch transform. The +1,000 NLOC includes
regression tests and DSP, rather than a second playback or file-save framework.
The branch-wide regression remains, including the earlier indexing/extraction
hotspots. This pass does not claim to eliminate that broader debt.

## Follow-up: pad navigation, group priorities and folder continuity

The six requested changes are implemented as separate commits. The priority
follow-up supersedes the initial numeric control with the requested three levels.

| Request | Resolution | Commit |
|---|---|---|
| Singleton mute groups | Leaving, moving or clearing a member dissolves any one-member remainder; loading normalizes legacy singletons. Undo restores partners together. | `acd8ba0` |
| Unreadable active buttons | Reuse the map's rounded outline button for velocity override, Editor, Mute, Solo, compressor Bypass and saved-pack selection. | `8229478` |
| Reveal clicked pads | Each pad cycles editor layers independently and centers the indexed sample without changing zoom. Double-click no longer launches Studio; the editor button remains. | `b253fbc` |
| Choke priority | Explain and persist precedence. New groups assign High to the first pad, Medium to the second, Low to subsequent pads. All three remain editable. | `b0a2e81`, `eca94ce` |
| Circular samples | Share one antialiased disk texture in the batched mesh, retaining four vertices/six indices per sample and equal radii at each zoom. | `d29324d` |
| Keep Constellation across folders | Preserve the view while checking recursive audio presence on the existing worker queue. A completed empty inventory permits fallback; explicit view changes win. | `d214b9d` |

Priority governs whether a hit is accepted while a partner rings: higher blocks
lower; equal priority uses the latest hit. Finished, muted and solo-gated voices
cannot block another pad. Existing groups keep manually edited levels when joined;
new members of those groups start Low. Pack loading maps the brief numeric format
onto Low/Medium/High; older packs without a priority use equal Low levels. Priority
travels with the instrument during pad swaps and participates in history and saves.

Pad navigation changes focus and camera position only on explicit ordinary clicks.
Shift-click keeps assignment behavior. Keyboard/MIDI hits retain their existing
performance behavior. Layer clicks wrap independently for each pad, and empty
pads are safe. A layer absent from the current index reports that fact; it cannot
be shown as an acoustic point until that location is indexed. Studio still uses
the guarded copy-and-replace handoff through its editor button.

The recursive presence check extends the existing guarded index inventory with
an early exit. It does not decode files, follow symlink trees, include generated
packs, or start indexing automatically. Folder generations reject stale results;
a dedicated cancellation flag stops superseded inventories. Unknown/error results
do not establish absence. Browser history and explicit mode changes retain their
existing hooks, and navigation still stops preview audio.

### Validation and architecture

The file-manager suite passes 644 tests with one ignored. The full serial workspace
run passes 2,569 tests with 23 ignored across 26 targets
(`/tmp/constellation-navigation-workspace.log`). Coverage includes singleton cleanup
and undo, group-join priorities, manual priority preservation, legacy metadata,
active/finished/muted precedence, per-pad layer cycling, circular mesh density,
recursive ordinary audio detection and inherited-view fallback. Existing camera
and responsive-editor tests also pass.

Refreshed current main remains `999cd24`. Against the start of this pass
(`constellation-navigation-groups`, `49939a9`), the architecture tool reports
**REGRESSION**: NLOC 241,457 → 241,867, functions 7,290 → 7,301, sum CCN
29,549 → 29,608, average CCN 4.05 → 4.06, maximum CCN unchanged at 196,
duplication 5.70% → 5.69%, clone groups 1,408 → 1,409, and reported cycles
unchanged at six. The added clone is inventory-fixture setup; existing headless
UI test clones are still reported in modified files.

The changed hotspots are FileManager::tick (15 → 18), discover_audio_paths
(17 → 18), render_starfield (32 → 34), and Rack::trigger (19 → 26). Their new
branches perform view fallback after asynchronous presence checks, stop inventory
at the first audio file, reveal a requested sample, and enforce audible-voice
priority respectively. This is retained functional complexity; there is no new
executor, device stream, dependency or source module. Circle rendering removes the
custom diamond builder in favor of egui's existing textured-rectangle primitive.

For the entire branch against current main: NLOC 227,320 → 241,867, functions
6,934 → 7,301, sum CCN 27,909 → 29,608, average CCN 4.02 → 4.06, maximum 196,
duplication 5.98% → 5.69%, clone groups 1,382 → 1,409, reported cycles five → six,
and health 73.88 → 62.71. The broader branch regression remains. The unchanged
Python video benchmark cycle again appears both added and resolved due to member
ordering; no cycle fix is claimed.

The final compact group menu (`b24d215`) replaces the wrapping priority row,
keeping Studio visible when a grouped pad is selected. The responsive geometry
regression now includes a grouped High-priority pad in both appearances. The full
workspace run preceded this localized layout refinement; all 94 focused
constellation tests passed again afterward.

Final file-manager build, formatting, diff whitespace and acceptance-script Python
compilation pass. Both private scenarios pass with 45 observations each:
`/tmp/constellation-navigation-dark-completed` and
`/tmp/constellation-navigation-light-completed` (hardware-Dvorak keys). They
verify manual Medium/High changes, persisted join-order defaults, editor-button
Studio entry, unchanged original bytes, two portable saves, recursive-only audio
folders staying in Constellation, empty-folder fallback, and normal guarded exit.
Screenshots of the grouped editor, priority menu and active velocity override
were inspected. These use ALSA null; physical hardware audio/MIDI and perceptual
pitch/compressor quality remain outside this runner.

Intermediate automation attempts needed the existing English Tesseract data at
`/tmp/constellation-tessdata` and corrections to assumptions about the removed
pad double-click shortcut. OCR missed the High row and Editor label despite
visible controls; click targets now use the verified menu-title offset and button
center. An actual grouped-header overflow was fixed by the compact group menu,
and the geometry test was extended. Product behavior was not weakened to make the
scenario pass.

## Follow-up: compressor timing, mixer layout and shared vignette

| Request | Resolution | Commit |
|---|---|---|
| Remove input/output history bars | Keep the orange gain-reduction trace, steady transfer curve, handles and edge peak meters. | `a64becd` |
| Compressor Attack/Release | Add saved logarithmic timing controls, preserving 5 ms / 120 ms legacy defaults and updating live coefficients without restarting voices. | `85f071c` |
| Discoverable choke priority | Right-click and the editor group button call the same High/Medium/Low menu, including Leave mute group. | `70d9550` |
| Taller gain and grouped tuning | Increase fader height from 84 to 150 pixels; place Pan alongside it, with pitch mode and Pitch knob together below Pan. | `3794135` |
| Velocity override location | Move the outlined toggle under the grid and reserve its space when sizing pads. | `3a37568` |
| Canvas vignette | Add a shared painter with the original photo-canvas helper's name and signature; use it after map content and before hover cards. | `2bc54f3` |

The shared `ui_ext::paint_canvas_edge_fade` retains the eight-quad, 28-pixel
edge treatment from `8826c084a6e1e6d5892da2f9e036223a3b234b30`. Adjacent quads
share 16 vertices instead of the photo helper's 32. The center stays transparent;
invalid bounds draw nothing, and tiny canvases cap the fade to one quarter of each
dimension. Paint clipping does not change projection or picking. The rack guide
includes the exact call/import change for the photo branch; that branch was not
modified here.

The mixer occupies a 116-pixel strip, leaving the existing right sidebar width
unchanged. Actual Pan/Pitch knob centers are checked for vertical alignment;
text-label centers vary with their different glyph widths. The short-window Output
panel reserves a visible scrollbar gutter so Release, Makeup and the meter footer
remain discoverable through ordinary scrolling. The acceptance scenario checks
the top controls and then scrolls to the timing controls.

Timing is stored in the bank, included in history/saves, and validated within
0.1–200 ms Attack and 10–2000 ms Release. A single-pole coefficient is computed
on control updates; the stereo audio loop only loads atomic coefficients.
Non-finite live edits retain prior valid values. Tests check the measured envelope
after one time constant, changes to fast attack/recovery, legacy defaults,
round-trip persistence and slider double-click resets. The context-menu pointer
test changes the right-clicked pad rather than the currently selected pad.

### Validation and architecture

`cargo test --workspace -- --test-threads=1` passes 2,572 tests with 23 ignored
across 26 targets (`/tmp/constellation-timing-workspace.log`). The final scrollbar
affordance is a localized UI follow-up after that run; all 96 constellation tests
pass again afterward (`/tmp/constellation-timing-final-focused.log`). The final
file-manager build passes; its two pre-existing float-inference warnings remain.
Rustdoc generation succeeds with the existing unrelated bare-URL warning in
image_strip/hilbert.rs.

Against this pass's `constellation-timing-vignette` baseline (`cd9f400`), the
architecture tool reports **REGRESSION**: NLOC 241,867 → 242,253, functions
7,301 → 7,310, sum CCN 29,608 → 29,655, average CCN unchanged at 4.06, maximum
unchanged at 196, duplication unchanged at 5.69%, clone groups 1,409 → 1,412,
and cycles unchanged at six. Validation rises from CCN 14 to 16 for the two new
finite timing ranges. The clone report includes repeated pointer-test setup and
short mesh-building fragments; sharing the complete menu/painter avoids a second
implementation of their behavior. No executor, device stream, dependency or
source module was added.

Refreshed main remains `999cd24`. Entire-branch values against main are NLOC
227,320 → 242,253, functions 6,934 → 7,310, sum CCN 27,909 → 29,655, average
CCN 4.02 → 4.06, maximum 196, duplication 5.98% → 5.69%, clones 1,382 → 1,412,
cycles five → six, and health 73.88 → 62.70. The branch-wide regression remains;
these UI changes do not erase earlier indexing/extraction complexity.

The final private dark and light/hardware-Dvorak scenarios each pass all 47
observations: `/tmp/constellation-timing-dark-completed` and
`/tmp/constellation-timing-light-checked`. They verify the pad context-menu choices,
manual priority changes, Studio handoff and unchanged originals, portable versions,
saved compressor defaults, the scrolled timing controls, velocity override below
the grid, folder continuity and guarded application close. Mixer and Output
screenshots were visually inspected. The runner uses ALSA null; hardware listening
and physical MIDI behavior are not established by these checks.

The shared vignette's generated rustdoc was checked, and formatting, diff
whitespace and both acceptance scripts' Python compilation pass. Intermediate
UI runs exposed the Output overflow affordance and stale automation targets after
moving velocity override. Bordered light-theme menu labels needed a dedicated
text-column OCR crop and verified row-center click targets. The scenario now
explicitly dismisses the context menu before selecting Pad 1, and checks the
selected pad before editing; manifest assertions continue to verify both pads'
priorities. No product behavior was weakened for OCR.

## Independent layer speed and editor strip

The selected-layer editor now puts only Gain and Pan in its existing 116-pixel
left strip. A 140-pixel waveform leaves room for a compact control strip beneath:
Retrigger at the left, independent Speed and Pitch faders, and Edit in Audio Studio
at the right. Vertical separators distinguish these sections. Below 660 pixels of
waveform width, the faders occupy a second compact row. Both themes are checked at
840×650, 1400×1000 and 2200×1300; no right-sidebar widening is involved.
The unnecessary choke-priority sentence has been removed from the shared menu.

Speed uses signed rates from −2× to +2×, with negative rates reversing complete
frames while preserving channel order. Zero is stopped; double-click restores
+1×. Pitch uses single cents over ±2400 cents with its own zero detent and reset.
Fixed-width editable readouts keep neighboring controls stationary across signs,
cents and octave labels. The waveform retains fade guides and a pitch-preparation
status. Reverse playback mirrors the fade flags, drag direction and envelope;
forward and reverse gestures are tested without changing trim bounds. No source-buffer copies, new playback streams, dependencies or executors
are introduced. Existing worker-side phase-vocoder preparation and the portable
export pipeline serve both controls. Old Time-mode packs migrate to their exact
playback rates, including old +4× settings, with independent pitch reset to zero.
New edits stay within ±2×. Zero-speed export returns a clear error and publishes
nothing; reversal and pitch preparation retain the existing bounded PCM limits.

### Architecture review

The `constellation-independent-faders` baseline is `4214707`. After removing the
temporary diagnostic assertion and redundant finite check (range membership rejects
NaN), the tool still reports REGRESSION: NLOC 242,253 → 242,529, functions
7,310 → 7,318, sum CCN 29,655 → 29,695, average 4.06 and maximum 196 unchanged,
duplication 5.69% → 5.68%, clone groups unchanged at 1,412, cycles unchanged at six,
and health 62.6969 → 62.6937. The bank validator gains a legacy migration branch;
the existing export renderer gains rate validation and reverse-buffer selection.
These are required compatibility and safety behavior, rather than a parallel
rendering mechanism. Its CCN rises 21 → 24; bank validation rises 16 → 18.
The cycle identity churn in the diff report concerns unchanged Python functions
with reordered members; the aggregate cycle count is unchanged in this pass.

A fresh fetch confirms main remains `999cd24`. Entire-branch comparison is still
REGRESSION: NLOC 227,320 → 242,529, functions 6,934 → 7,318, sum CCN
27,909 → 29,695, average 4.02 → 4.06, maximum 196 unchanged, duplication
5.98% → 5.68%, clones 1,382 → 1,412, cycles five → six, and health
73.8835 → 62.6937. Earlier branch-wide structural debt remains; this pass adds
no clone groups or cycles and reuses the original source, pitch and export paths.

### Validation

The workspace suite passes 2,576 tests with 23 ignored across 26 targets
(`/tmp/constellation-faders-workspace.log`). After the final readout and reverse-envelope fixes,
all 100 constellation tests pass again (`/tmp/constellation-faders-complete-focused.log`).
These cover frame-aligned stereo reversal without buffer copies, zero-speed silence,
non-finite rate rejection, legacy defaults/migration, combined pitch/speed exports
matching live PCM, and cleanup after rejected stopped-layer exports. Rendering tests
check both themes and three window sizes, including the complete readouts. A separate
test checks stationary surrounding controls across changing signs and unit labels.

The final file-manager build, formatting check, Python compilation, public and
private rustdoc generation pass. The rendered `SlotSample` speed field documentation
was inspected. Two pre-existing Rust float-inference warnings and the unrelated
image-strip rustdoc bare-URL warning remain.

The private dark and light/hardware-Dvorak scenarios each pass all 47 observations:
`/tmp/constellation-faders-dark-complete` and
`/tmp/constellation-faders-light-verified`. These desktop runs precede the final
reverse-envelope-only follow-up, which is covered by the focused Rust tests. The mixer screenshots were inspected in
both themes. A real screenshot exposed a numeric readout clipped despite its label
being visible; the allocation now measures the rendered label width and the
regression test checks the readouts too. OCR-only failures required text-column
crops and anchoring the shorter priority menu to its heading, plus a verified
velocity-button click fallback. Assertions for priority persistence, Studio saves,
unchanged originals, portable packs and guarded close remain in place. The private
runner uses ALSA null; these checks do not establish hardware listening or MIDI.


## Resettable layer strip and uniform dot-edge fade

Gain is now the only vertical control beside the waveform. The strip below it
orders Retrigger, Pan, Speed, Pitch and Edit in Audio Studio, with separators
between sections. Each section receives bounded space and wraps as a unit when
needed; the Studio button cannot overlap the fixed-width pitch readout. Pan shares
the center-detent fader track with Speed and Pitch. Double-click resets Gain to
0 dB, Pan to center, Speed to +1× and Pitch to zero. This reuses Audio Studio's
`slider_double_clicked` helper: native egui sliders sense drags, so checking their
ordinary `Response::double_clicked` alone never reset them. The original slider
response retains its change flag. Matching the row height to egui's interaction
height also prevents invisible vertical overflow. Rendering coverage checks both
themes at 840×650, 1400×1000, 2200×1300 and 3200×1800, including readouts and
non-overlapping Studio bounds.

The isolated `1dbde80` commit replaces the canvas vignette with one opacity per
complete dot. Follow-ups remove nested canvas padding and shorten the fade to six
pixels of circle-edge clearance; the selected dot remains beneath its larger
highlight. The final top-edge correction paints the header divider directly at the
canvas boundary, eliminating separator-row padding below that line. Side and
bottom boundaries keep their approved geometry. The existing shared vignette
helper remains available to the photo branch. Dot vertices retain identical alpha,
and a rendering assertion checks that the canvas starts at the divider.

### Architecture review

The `constellation-pan-dot-fade` baseline is `0768d42`. The final tool comparison
reports REGRESSION: NLOC 242,529 → 242,737, functions 7,318 → 7,322, summed CCN
29,695 → 29,720, average 4.06 and maximum 196 unchanged, duplication 5.68%
unchanged, clones 1,412 → 1,414, cycles six unchanged, health 62.6937 → 62.6894.
The two added clone groups involve short native-widget row construction and
synthetic pointer-input test setup. Shared fader behavior and the existing Studio
reset helper avoid introducing parallel control mechanisms. The remaining new
hotspot deltas are the existing dock's mode-dependent canvas margin (CCN 16 → 17)
and the acceptance scenario's optional compact-editor path (37 → 40). The toolbar
retains its previous complexity: the indexing section owns its own divider.
There are no new dependencies, playback streams, workers or dependency cycles in
this pass.

A fresh fetch confirms current main remains `999cd24`. Entire-branch comparison
still reports REGRESSION: NLOC 227,320 → 242,737, functions 6,934 → 7,322,
summed CCN 27,909 → 29,720, average 4.02 → 4.06, maximum 196 unchanged,
duplication 5.98% → 5.68%, clones 1,382 → 1,414, cycles five → six,
health 73.8835 → 62.6894. Earlier branch-wide debt remains; this UI pass does not
claim to resolve that structural regression.

### Validation

The workspace suite passed 2,579 tests with 23 ignored across 26 targets
(`/tmp/constellation-pan-fade-workspace.log`) before the final compact-width and
canvas-boundary adjustments. All 103 constellation tests pass after those
adjustments (`/tmp/constellation-top-boundary-final-tests.log`); the extended
divider assertion also passes after moving separator ownership to the indexing
section (`/tmp/constellation-top-divider-regression.log`). Three browser UI tests
passed after the outer margin change (`/tmp/constellation-flush-browser-ui.log`).
The final file-manager build passes (`/tmp/constellation-top-divider-build.log`).
Two unrelated existing Rust float-inference warnings remain.

Private Weston/Niri compact-editor checks pass all ten observations in each theme:
`/tmp/constellation-top-final-dark` and `/tmp/constellation-top-final-light`.
The light run uses the hardware-Dvorak key configuration. Both verify pad
assignment, visible Gain/Pan/Speed/Pitch/Retrigger controls, the Studio button's
hit target, return without replacing the original, undo and clean close. Final
screenshots were inspected in both themes. The dedicated `--editor-only` scenario
allows this bounded regression check without repeating the full portable-pack
scenario. OCR fallbacks use verified text crops and a confirmed modal button
center. Formatting, Python compilation and `git diff --check` pass. The private
runner uses ALSA null; these checks do not establish physical audio or MIDI quality.
