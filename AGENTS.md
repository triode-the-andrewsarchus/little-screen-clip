# Repository Guidelines

## Project Structure & Module Organization

This is a Rust 2024 Cargo workspace. Shared clipboard models, configuration, IPC, colors, and media helpers live in `crates/little-clip-core`. Keep hardware and USB-display code in `little-clip-driver`, rendering in `little-clip-render`, and transformations and pipeline operations in `little-clip-ops`. The `little-clipd`, `clipctl`, and `clip-workbench` crates provide the daemon, CLI, and egui workbench binaries.

Root `scripts/` contains supporting Python tools, `config.toml` holds runtime defaults, and `launch.sh` manages local builds and services.

## Build, Test, and Development Commands

Run commands from the repository root:

```bash
cargo build                              # Build the workspace in debug mode
cargo build --release                    # Build optimized binaries
cargo test --workspace                   # Run all unit tests
cargo test -p little-clip-ops            # Test one crate
cargo fmt --check                        # Verify Rust formatting
cargo clippy --workspace --all-targets -- -D warnings
./launch.sh build                        # Release build through the helper
./launch.sh sweep                        # Clean stale target and incremental artifacts
./launch.sh start                        # Start the local daemon
./launch.sh ui                           # Open the workbench
./launch.sh test                         # Exercise the USB display directly
```

`./launch.sh test` can stop a running daemon and access attached hardware; use it only when that is intended.

## Coding Style & Naming Conventions

Use standard `rustfmt` output (four-space indentation). Name modules, functions, and variables in `snake_case`; use `PascalCase` for types and traits and `SCREAMING_SNAKE_CASE` for constants. Keep modules focused on one domain. For fallible operations, return explicit `Result` values and use the project’s error types rather than panicking in normal runtime paths.

## Testing Guidelines

Keep unit tests beside the code in `#[cfg(test)]` modules. Use descriptive `snake_case` test names that state the expected behavior, such as `fixed_palette_text_is_readable`. Add targeted tests for new transformations, parsing or configuration behavior, rendering, and testable UI logic. Run the affected crate’s tests during development and `cargo test --workspace` before submitting broader changes.

## Commit & Pull Request Guidelines

Follow the existing Conventional Commit pattern with a concise scope: `feat(image): add film simulation controls`, `refactor(ui): extract shared toolbar`, or `feat(ui): improve contrast`. Use imperative summaries. Pull requests should explain the behavior change, list validation commands and results, link relevant issues when available, and include screenshots or GIFs for workbench changes.

## Runtime Configuration & Hardware

Do not commit personal display, compositor, or desktop settings. Keep `config.toml` changes reloadable, and preserve virtual-mode behavior when no supported USB display is attached.

The workbench must remain a non-resizable, centered, single-instance popup for Niri's fixed-size floating behavior. Screen-adaptive sizing must be resolved before the window opens, with fixed-size hints in place at mapping; do not enable window resizing/maximizing or resize a visible window to fit. Internal panels may resize independently. Repeated `launch.sh ui` calls should focus the existing popup without requiring a Niri keybinding change.

Shared display/workbench SVG assets live in `little-clip-render/assets/icons`. Use embedded SVG icons for application chrome, controls, and notifications; do not use emoji glyphs as icons. Keep notification icons separate from their text. Copied summaries and terminal output use plain text; preserve emoji in user content and Unicode rendering fixtures.

Use the shared `ui_tabs::TabBar` for workspace, tool, viewer-mode, and sheet navigation. Keep parameter choices and on/off controls as buttons. Re-selecting an active tab is a no-op; selection changes must preserve existing entry/exit hooks.

UI acceptance automation must use the private Weston/Niri runner. Its pointer injection is scoped to that run's private Wayland and Niri sockets; do not send test input through host `/dev/uinput` or the user's interactive compositor.

The little screen mirror and rolling audio strip belong only to Quick Palette. Auto Actions and Big Node Machine use their own canvas space. Script-node selection opens the built-in side editor; keep script undo history separate from graph undo, and save drafts before pipeline execution or recipe saving.

Image Studio tool controls belong in the shared left panel, with result panels on the right where needed. Creative FX retains FX selection on the left and effect controls on the right. Shared zoom controls belong in the center of the footer. View shows all metadata fields directly in that panel, and reserves it only when it has fields to display; metadata-free images use the full canvas.

Film Look enables vendored filmr's `compute-gpu` feature; `gpu` alone does not enable compute rendering. Preserve the full-spectrum exposure and dye output model; the upstream GPU development shader uses a different model. GPU grain uses filmr's native density-space shader; CPU fallback retains output-space grain. Apply only one grain model per render. Keep GPU pipelines bound to one shared device, cancellation checks between GPU stages, and CPU fallback when no adapter is available. Validate acceleration with optimized end-to-end timings as well as GPU dispatch tests.

Async preview completion updates an existing clipboard item in place. Keep `SlotStateSummary` sensitive to preview readiness so open workbenches rehydrate the affected slot and repaint the mirror; item ID and payload kind alone do not reveal these updates. Workbench startup loads clipboard state off the UI thread; mirror rendering and operation discovery use the bounded job executor. Share workbench slot snapshots with `Arc<SlotManager>` and use `Arc::make_mut` for local swaps. Reject stale mirror/action completions after state or selection changes, and stale mirror completions after appearance changes; cache discovery separately from search filtering.

Raster import uses core's `item::image_reader` to initialize HEIF/HEIC/AVIF decoder hooks once. Pass MIME hints through dimensions, thumbnails, and comparisons for formats without reliable signatures (TGA); reuse core image extension and MIME classification. HEIF EXIF extraction uses the existing `kamadak-exif` container reader.

Image Studio committed pixels and original/undo/redo revisions use `Arc<RgbaImage>`. Wrap replacement results in a new `Arc`, and use `Arc::make_mut` for in-place edits so background readers and history keep their original pixels.

PDF page rasters belong in a viewer-owned temporary directory. Keep only raster paths in page state and read one compressed page at a time in the bounded decode worker, preserving re-selection after texture eviction without document-sized RAM retention.

`WorkbenchApp` owns one active `ViewerSession` at a time. CLI previews queued at startup open in their established order; asynchronous completions must wait for the active session to close rather than replace or appear behind it.

Runtime Hundred Rabbits themes and per-color overrides are retired, including on the USB display. Define fixed Light/Dark base colors in core's `palette::BaseColors` and access them through `AppPalette.colors`; switch workbench and display together. Keep base surfaces and text neutral gray; use color for highlights and data. Keep egui-specific semantic roles in `clip-workbench/src/ui_appearance.rs`. Do not reintroduce raw `Theme` fields or rewrite a user's personal `config.toml` while migrating it.

The comparison text viewport uses one dark `console_surface` in both appearances so terminal ANSI colors remain readable. Keep this surface in the central palette; ANSI escape colors represent data, not workbench chrome.

Bundled CJK and monochrome emoji font assets live in `little-clip-render` and are shared with the workbench. Screen text measurement and drawing must use the same glyph fallback selection; prefer the requested system font wherever it has the glyph.

Raw text preview selection spans virtual 4 KiB source fragments. Store its endpoints as absolute UTF-8 source byte offsets and copy from the original content; egui's cross-Label selection inserts layout-dependent separators and drops endpoints that scroll out of view.

## Agent Maintenance & Self-Correction
During final cleanup, consider whether the work revealed that this file is incomplete, incorrect, or obsolete.

Update it only when work:
- establishes or changes a durable architectural pattern or convention;
- changes an interface, invariant, workflow, or constraint documented here;
- reveals an important non-obvious constraint future agents need to know; or
- invalidates existing guidance.

Do not update it for ordinary bug fixes, routine features, localized refactors, implementation details, or information readily discoverable from the code.

Prefer correcting or consolidating existing guidance over appending new material. Keep this file concise and current rather than historical.

If the user corrects an assumption or identifies a recurring mistake, consider whether that correction represents a durable project rule.
If so, update the appropriate guidance so future agents do not repeat it.
Do not record one-off preferences or task-specific corrections.
