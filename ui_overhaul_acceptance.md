# Workbench UI acceptance observations

Working evidence sheet for E1.4 and final acceptance. Record each run with date, commit, display mode, compositor/UI scale, action, result, and screenshot or log path. A blank result means **not tested**, not passed. Keep hardware USB tests separate from virtual/display-render checks.

## Starting evidence, 2026-09-23

- Branch baseline: `4edf785`, architecture baseline `easy1-start`: 81,632 LOC, 1,863 functions, average CCN 5.01, 5.28% duplication, 467 clone groups, health 79.94. The local `config.toml` has an unrelated uncommitted edit; it is excluded from UI commits.
- Read-only `niri msg --json focused-output` reported DP-1 at 3840×2160 physical, scale 1.5, **2560×1440 logical**. `niri msg --json outputs` reported three outputs at 2560×1440 logical, each scale 1.5. This proves only the queried environment, not popup placement or multi-output correctness.
- `popup::size_from_output` applies `floor(logical × 0.92)` and its tests cover logical scale handling, null/invalid output, and fixed min/max/resizable hints. These are source/unit-test claims; native mapping and focus are still unverified.
- No `little-clip-workbench` window was present in Niri's window list during the initial read-only baseline. A later focused-output popup smoke run is recorded below; no screenshot, clipboard, or hardware scenario has yet been run for this sheet.
- Built current debug workbench with `cargo build -p clip-workbench`, then launched it directly. Niri reported one focused, floating `little-clip-workbench` window (ID 295), client 2355×1324, position 102×75.3 in the 2560×1440 logical workspace. After `./launch.sh ui` while it was open, Niri still reported the same single focused ID/PID, size, and position. The test window was closed and Niri then reported zero workbench windows. This confirms settled geometry and repeat-launch singleton/focus in this configuration; it does **not** capture the first visible frame or prove there is no transient jump.
- A second run with `niri msg --json event-stream` **did capture the startup regression**. For window 296, Niri emitted `WindowOpenedOrChanged` with 4710×2648 at position 0×34, then `WindowLayoutsChanged` to 2355×1324 while still at 0×34, then another layout change to 2355×1324 at 102×75.3. The initial dimensions are exactly 2× the intended logical size. The event order proves an initial oversize/top-left map followed by visible size/position correction, contrary to the fixed-at-mapping requirement. Keep P1 open; H1.1 must fix the pre-map DPI conversion and remove reliance on a visible correction.
- Likely conversion boundary to investigate: egui-winit 0.27.2 `create_winit_window_builder` multiplies `ViewportBuilder` point sizes by the primary/first monitor scale before creation; `apply_viewport_builder_to_window` recomputes with the actual window scale afterward. `popup::window_builder` supplies logical output dimensions as viewport points, and `center_when_mapped` sends a separate Niri center action. This source path is consistent with the observed two-step geometry but needs instrumentation before assigning the exact 2× factor.
- After the Easy 1 code changes, `cargo fmt --check` and `cargo test --workspace --quiet` passed. `cargo clippy --workspace --all-targets -- -D warnings` did **not** pass: it stopped in untouched `little-clip-core` with 146 lint errors in the library and 151 in its test target (for example `collapsible_if` in `animation.rs:109`). This is a baseline repository lint failure, not evidence that the new UI code is lint-clean; compare it with a clean baseline before treating it as a branch regression.
- `architecture_compare` against `easy1-start` reported global duplication 5.28%→5.27%, average CCN 5.01→4.99, unchanged 467 clone groups, and health 79.94→79.95. Its `REGRESSION` label lists existing clone patterns merely touching edited `image_preview.rs`/`ui_ext.rs`; no net clone group or dependency cycle was added. Re-run after the final Easy 1 diff before assigning causality to per-file clone matches.

## Screen and scale matrix

The sizes below are *expected client sizes from current policy*, not observed windows. Test physical modes with the compositor's resulting logical size; do not divide logical dimensions by scale again. Do not change the user's output configuration merely to populate this table.

| Logical output | Expected fixed client | Dark | Light | 100/125/150/200% scale | Niri centering / repeat-launch focus |
| --- | --- | --- | --- | --- | --- |
| 1280×720 | 1177×662 | pending | pending | pending | pending |
| 1366×768 | 1256×706 | pending | pending | pending | pending |
| 1920×1080 | 1766×993 | pending | pending | pending | pending |
| 1920×1200 | 1766×1104 | pending | pending | pending | pending |
| 2560×1600 | 2355×1472 | pending | pending | pending | pending |
| 3440×1440 | 3164×1324 | pending | pending | pending | pending |
| 3840×2160 | 3532×1987 | pending | pending | pending | pending |
| Current DP-1: 2560×1440 logical at 150% | 2355×1324 | pending | pending | initial 4710×2648 then settled 2355×1324 | **fail:** first map top-left/oversize; singleton and settled focus pass |

For each row, capture the first visible frame and the settled frame. Check fixed, non-maximizable, floating hints; no top-left jump; no visible resize; content margins; all tool rows and status bars; no clipped close/save/copy action; internal split dragging without outer resize. Repeat after changing focused output, disconnect/failure fallback, and direct binary launch. If a window manager or display is unavailable, mark the row blocked by environment and retain the headless geometry result separately.

## Reproduction scenarios and fixture requirements

Keep fixture data under a local scratch directory unless a small deterministic generator is added to `scripts/`. Never place personal clipboard data or machine config in the repo.

Generate the available source-controlled fixture set with `python3 scripts/ui_acceptance_fixtures.py --output-dir /tmp/clip-ui-fixtures-<run-id>`. It creates Unicode/ANSI/Markdown text, 1,000-column and 20,000-row CSVs, binary insertion/deletion pairs, transparent PNG, stereo/silence/transient WAVs, and a simple STL in the chosen scratch directory. Use `--large-audio-seconds 300` for a roughly 53 MB WAV that can expose delayed preview completion; the default is 30 seconds to keep routine runs fast. The generator refuses to overwrite an existing fixture. Multi-page PDF, multi-track MIDI, SVG/EXIF and malformed-file fixtures are still needed; record their provenance when added.

| ID | Fixture / entry | Steps and observations to record | Current result |
| --- | --- | --- | --- |
| P1 | `./launch.sh ui`, then repeat the same command while open | Count windows; first/settled size and placement; second invocation focuses same window; dismiss and relaunch on another focused output. | **fail at first map:** 4710×2648 top-left → 2355×1324 top-left → centered on DP-1. Repeated `./launch.sh ui` focuses the same window; different-output relaunch pending. |
| P2 | Workbench with daemon virtual mode and Left/Right/Undo sample payloads | Mirror visible on entry, actual display theme retained across workbench Light/Dark toggle, slot hit targets/copy/selection correct, narrow/ultrawide mirror bounded. | pending |
| P3 | Large audio file copied while workbench remains open | Observe mirror and workbench during `Analyzing Audio`, then after async waveform completion; both must repaint without reopen. Test stale replacement and silent/error state. | pending |
| I1 | Raster + transparent image, SVG, long Unicode filename, EXIF/GPS metadata | Enter Image Studio; all tool buttons one click away; Save/Copy/Close and external apps remain reachable at compact/wide sizes; inspect undo/revert and save-dialog Escape/focus. | pending |
| A1 | Stereo 44.1 kHz and ultrasonic audio, multiple regions/stems | Compare transport/mode/toggle heights and baselines; inspect selection, playback, ratios, filters, tooltip and keyboard focus; adjust internal splits during playback. | pending |
| T1 | Long Unicode/ANSI/Markdown text and 1+ MiB file | Search, regex/case, wrap, gutter/minimap, line hit testing, selection/copy; compare Light/Dark and UI scale. | pending |
| D1 | Two text/images/binaries with insertions, unequal sizes and long names | Every comparison mode, swap, navigation, copy, image divider, binary widths, minimap and focus/Tab behavior. | pending |
| F1 | Multi-page PDF; wide/tall spreadsheet; multi-track MIDI; small/large 3D model | Load and interact; verify bounded rendering/progress/error, page/row/track controls, external handoff, and output/close semantics. | pending |
| N1 | Node graph recipe with typed ports, parameters, preview and save/load | Build, run, undo/redo and round-trip; inspect contrast, ports, focus, unsaved close, and output. | pending |
| S1 | Settings, auto-actions, prompts, absent external executable | Persist/reload preferences, keep display theme separate from workbench appearance, verify error and retry remain visible, no leaked shortcut behind modal. | pending |

For every scenario, capture the keyboard path and mouse path where both exist, source and destination of copied/saved data, and whether the prior document/session remains available. The detailed feature checklist lives in `ui_overhaul_parity_ledger.md`; a scenario passes only when its relevant ledger actions pass, not when the view merely opens.

## Performance observations to add

On named reference hardware, capture UI-frame time during idle, scroll, zoom, playback, and large-file load, plus task duration separately. Record item count/size, cache/job bounds and whether CPU decode, IPC, encode, or process wait occurred on the UI thread. The target from `ui_overhaul_plan.md` is smooth 60 Hz manipulation (16.7 ms frame budget) with next-frame visible response; this target is **not yet measured**.
