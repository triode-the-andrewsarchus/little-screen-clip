# Workbench UI overhaul: remaining task queue

Status: open. Last source audit: 2026-09-23 on `feat/workbench-ui-overhaul` at `1ae0aec`. This is a working queue, not a claim that the older `ui_overhaul_plan.md` is fully implemented. Re-check code and manual behavior before marking an item done; split, merge, or add tasks as discoveries warrant. The original plan remains the feature-parity and design reference.

## Already landed; verify in final acceptance

- Fixed-size, screen-adaptive popup sizing; single-instance activation and centering fixes.
- Readable shared control metrics, aligned HF controls, wrapping one-click Image Studio tools, compact header actions, shared studio panel geometry and image split handles.
- Separate workbench Light/Dark appearance, shared bounded display mirror, and async-preview mirror invalidation.
- Palette focus fix, image save-dialog shortcut precedence, background rolling-audio refresh and spreadsheet loading, and virtual spreadsheet rows/columns.

These are code-level observations from the branch, **not** a completed Niri, mixed-DPI, visual, or feature-parity test. In particular, the user's centering, mirror, and async-audio reports deserve explicit live regression scenarios.

## Working agreement for the two-model flow

- **Easy** means a bounded change with known behavior and localized ownership. Assign implementation to `ui_luna_worker` (`gpt-6-luna`); the primary model reviews its diff, checks feature parity, fixes any issue, and commits a coherent increment. Give the worker one task ID or a small non-overlapping set, explicit acceptance checks, and ownership of files. Let it write code.
- **Hard** means changing cross-view ownership, async lifecycle, color contracts, coordinate systems, or complex viewer semantics. The primary model designs and implements these, or delegates a clearly isolated piece with an explicit review boundary.
- Keep a usable app at each commit. Run affected tests during each task; run workspace fmt/test/clippy and architecture comparison at each phase gate. Inspect real UI at representative host sizes when layout changes. Do not alter personal `config.toml` or Niri settings.
- Before each phase, update this list from the current tree. Promote an “easy” item to hard when its behavior or dependency is less contained than expected. The labels assign **implementation risk**, not importance.

The ordering below makes five model switches: Easy 1 → Hard 1 → Easy 2 → Hard 2 → Easy 3 → Hard 3. The primary model reviews and repairs the whole easy batch before advancing. Hard 1 establishes contracts that Easy 2 can apply mechanically; Hard 2 handles the cross-cutting cases those contracts expose.

## Easy 1 — Inventory and contained repairs (Luna implementation; primary review)

- [x] **E1.1a Seed the action-parity ledger.** `ui_overhaul_parity_ledger.md` now covers every surface in plan section 7 with source anchors and unrun scenarios. Reviewer caught and corrected initial shortcut errors; this is an inventory scaffold, not parity sign-off.
- [ ] **E1.1b Complete action-level coverage.** For each surface, verify every current button, shortcut, parameter range, output and close/cancel behavior against source; split broad rows into checkable actions and mark inventory gaps. Primary-model review must confirm no feature was omitted before accepting the ledger.
- [x] **E1.2 Fix Image Studio external-app descriptors.** Descriptors now pair configured executable, displayed label, availability-aware tooltip, and selected launch target. Focused tests pass; live external-app/fallback behavior remains in the acceptance sheet.
- [x] **E1.3 Remove only proven-dead UI paths.** Re-checked references and removed the unused `ui_ext::render_view_header` and `ShortcutDispatcher::handle_escape`; the active application Escape path remains. `cargo test -p clip-workbench` passed (185 passed, 1 ignored).
- [x] **E1.4a Establish reproducible inputs and starting evidence.** `scripts/ui_acceptance_fixtures.py` creates deterministic scratch data; `ui_overhaul_acceptance.md` records the architecture baseline, screen matrix and observed centered singleton popup on the current 150%-scale Niri output.
- [ ] **E1.4b Complete baseline observations.** Exercise the 1280×720 through 3840×2160 and Light/Dark cases, first-visible-frame positioning, long names, many tools, loading/errors, and performance. Add PDF/MIDI/EXIF fixture provenance. Distinguish pre-existing defects from migration regressions; hardware access remains separate.

## Hard 1 — Shared contracts and platform decisions (primary model)

- [x] **H1.1a1 Correct startup DPI conversion on the available output.** Niri event-stream originally observed a 4710×2648 top-left first map for a 2355×1324 request. A focused local egui-winit 0.27.2 patch now supplies logical startup dimensions; its first recorded open event is 2355×1324 and centered on the current 150%-scale output. Direct launch and repeated `./launch.sh ui` preserve the same fixed singleton. Workbench tests pass.
- [ ] **H1.1a2 Prove other output/scale cases.** Exercise the first Niri event on different logical resolutions, mixed output scales and failure fallback, with fixed/non-maximizable hints; do not infer those results from the current 150% test.
- [ ] **H1.1b Settle the layout backend.** An isolated coherent egui 0.36.2 upgrade confirms its startup window builder uses logical sizes but produces 427 workbench compile errors. Evaluate a bounded `egui_taffy` trial against native layout; choose one based on image header, HF toolbar, studio panels, focus/side effects and frame cost. Recheck popup hints and behavior after any future upgrade, then remove the local egui-winit patch when safe.
- [ ] **H1.2 Define semantic workbench colors and component tokens.** Replace workbench dependence on display-oriented `AppPalette` through an incremental adapter; keep display themes and mirror rendering independent. Establish typography, control/input sizes, spacing, focus and contrast states in one place, with a component gallery or equivalent test harness. Gate: representative components in both appearances meet the plan's contrast targets on their actual backgrounds.
- [ ] **H1.3 Establish session, action, focus, and task boundaries.** Replace scattered optional viewer dispatch with explicit route/return/session lifecycle incrementally; define host-scoped preview identity, modal and shortcut precedence, typed actions, status/progress, and revision-aware task completion using `BackgroundJobs`. Do not build a general plugin/docking system. Gate: a representative image, audio, and text path prove navigation, focus, cancellation/error handling, and no repeated side effects.
- [ ] **H1.4 Complete shared host controls.** Make headers, tool groups, fields, transport, split panels, overlay/status, and escape hatches consume the contracts above. Preserve direct access to every image tool; toolbar wrapping must not strand labels or hide common actions. Gate: the user's Image Studio button/row and audio mode/transport examples align at compact, standard, and wide host sizes.

## Easy 2 — View migration on established contracts (Luna implementation; primary review)

- [ ] **E2.1 Migrate image tools in cohesive slices.** View/geometry; annotate/protect; color/looks; extract/analyze; output. Apply shared headers, fields, panels, readable labels, contrast roles, and one-click tool navigation without changing transformations. Test each slice's parameters, undo/revert, save/copy, and external handoffs.
- [ ] **E2.2 Migrate ordinary audio control rows.** Apply shared transport, field, segmented-mode, progress and status components to inspect, HF, stems, decode, and rolling strip where behavior is already defined. Preserve seek, selections, filters, gain, export, playback, and keyboard semantics. Escalate any audio-engine or async-lifecycle change to Hard 2.
- [ ] **E2.3 Migrate straightforward chrome in remaining viewers.** Update PDF, spreadsheet, MIDI, 3D, text/diffs, palette, settings/prompts, and node-machine headers/forms/status to semantic roles and measured controls. Preserve specialized canvases and existing workflows. Gate: no ordinary chrome relies on `.small()` merely to fit; the remaining reduced text has a named use.
- [ ] **E2.4 Fill focused interaction and contrast tests.** Equal sibling controls, stable Play/Pause widths, field focus, tooltip/disabled reasons, modal Escape, label contrast, and async-preview readiness. Gate: tests cover behavior rather than restating widget construction.

## Hard 2 — Cross-view behavior, scale, and embedding (primary model)

- [ ] **H2.1 Finish asynchronous file and export paths.** Audit image/PDF/text/MIDI/3D and other opens, save/encode/export, external processes, and preview work for UI-thread blocking. Move expensive operations through bounded jobs with progress/errors, revision checks, cancellation where real, and repaint on completion. Preserve output fidelity and destinations.
- [ ] **H2.2 Make dense content scale-correct.** Measure wrapped text rows/search/minimap geometry; make binary byte cells and MIDI piano-roll labels/font hit areas scale from fonts; validate spreadsheet two-axis scrolling, sticky headers, full-cell inspection, and large-data bounds. Gate: scroll/search/copy targets remain correct across UI/DPI scales and large fixtures.
- [ ] **H2.3 Complete complex workspace semantics.** Migrate node editor, diff/comparison modes, PDF page/edit flow, 3D interaction, audio gesture-only controls, and Quick Palette/automation flows to the session/action/overlay contracts. Gate: the parity ledger's keyboard, output, and return-destination scenarios pass.
- [ ] **H2.4 Prove reusable preview hosting.** Separate decoded source, preview view state, and editor state; render multiple independent inline previews and full studios using the same data where safe. Add revision-aware, bounded cache/job/texture lifetimes suitable for future file thumbnails, including invalidation and eviction. Gate: embedding harness demonstrates independence, no repeated side effects, and bounded loading. File-manager operations themselves are out of scope.

## Easy 3 — Consolidation and evidence (Luna implementation; primary review)

- [ ] **E3.1 Remove superseded adapters and duplicate chrome.** Delete only paths made obsolete by Hard 1–2 and confirm architecture metrics improve for real reasons. Keep specialized domain behavior intact.
- [ ] **E3.2 Update docs/config migration notes and parity ledger.** Replace the obsolete `ui_architecture_design.md` with actual implemented boundaries; update `AGENTS.md` only for durable new conventions. Preserve display theme choices and personal config. Mark each ledger item with test/manual evidence, not mere existence of a control.
- [ ] **E3.3 Build repeatable visual/interaction captures.** Collect Light/Dark screenshots at representative logical sizes and scales, plus Niri launch/return, long filenames, many presets/tracks/pages, loading/error, mirror freshness, and screen-reader/focus observations where feasible.

## Hard 3 — Final integration and acceptance (primary model)

- [ ] **H3.1 Review and fix the Easy 3 diff, then close all parity gaps.** Compare the full ledger with running behavior; reclassify any failing scenario into a concrete task and assign it to the appropriate model before final sign-off.
- [ ] **H3.2 Verify reliability, performance, and boundaries.** Exercise the resolution/DPI matrix, fixed non-resizable centered singleton popup, virtual/no-hardware daemon behavior, idle repaint, 60 Hz interaction target, bounded caches/jobs, and no UI-thread decode/IPC/subprocess waits. Run `cargo fmt --check`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and comparable architecture measurements. Manual hardware access requires deliberate separate testing.
- [ ] **H3.3 Finish reviewable delivery.** Ensure every original feature is still reachable, both modes are readable, inline preview reuse works, docs match code, and no temporary second design system remains. Provide final before/after evidence and any explicit limitations.

## Dependency notes

`E1` can start now. `H1` must settle shared contracts before `E2`. `H2` follows the representative H1 slices and E2 findings. `E3` follows migration, and `H3` is the acceptance gate. Within a batch, split task IDs into bounded commits; do not run overlapping agents against the shared worktree. Revisit the queue after each review, since an uncovered interaction can change both task size and difficulty.
