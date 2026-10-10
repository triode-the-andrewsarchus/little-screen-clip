# Repository Guidelines

## Project Structure & Module Organization

This is a Rust 2024 Cargo workspace. Shared clipboard models, configuration, IPC, colors, and media helpers live in `crates/little-clip-core`. Keep hardware and USB-display code in `little-clip-driver`, card/text rendering in `little-clip-render`, native Gaussian scene import and GPU rendering in `little-clip-scene`, and transformations and pipeline operations in `little-clip-ops`. The `little-clipd`, `clipctl`, and `clip-workbench` crates provide the daemon, CLI, and egui workbench binaries.

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

Every `launch.sh` Cargo build performs an automatic target sweep toward a 20 GB cache budget (`LITTLE_CLIP_TARGET_MAXSIZE` overrides it), retaining two incremental sessions per crate/configuration. Without `cargo-sweep`, cleanup clears debug artifacts with `cargo clean --profile dev`. Serialize launch builds and sweeps with other Cargo commands so they cannot prune a compiler’s active incremental session.

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

The file manager is a separate application (`clip-file-manager`, launched with `launch.sh files`), tightly coupled to the workbench through shared library code and core services. Do not embed it as a workbench tab. Share UI chrome, fonts, viewers, and operation mechanisms as they become needed; keep application lifecycle and navigation independent. The workbench's fixed popup constraints do not apply to the file manager's ordinary resizable window. File mutations must preserve their completion results: read-style cancellation or post-operation timeouts can hide effects that already happened. Retry queue admission only. Batch cancellation is cooperative and must still report completed items. Use atomic no-overwrite creation/rename; replacements must preserve the old destination until installation succeeds. Native Wayland file drags borrow eframe's display with their own event queue. Finish received moves only after complete success; outgoing source removal verifies identities and quarantines originals. Enable egui-winit's clipboard shortcut key forwarding only for focused file lists or audio waveforms; register the waveform focus target every frame so range shortcuts retain focus. Text fields retain normal text clipboard handling.

The file manager opens shared studio modules from the workbench library and owns one file viewer/loading request at a time. Preserve browser selection/history/scroll across studios, keep accepted browser operations ticking, and disable browser drop targets while a studio is visible. File-backed text/image saves use `file_source::FileSource` for identity checks and atomic replacement; Save As uses unused destinations. Files audio/PDF outputs must not overwrite existing siblings. Defer close during accepted writes and prompt before discarding session edits. Both desktop binaries must dispatch shared internal barcode/conversion workers before ordinary CLI parsing. Resolve bundled helper scripts independently of the launch directory.

List view mixes text minimaps, music waveforms, video timelines, and PDF page strips using the same row renderers and bounded preview caches as the dedicated media views. Retire hidden media previews without losing text row classifications. PDF strips render only the displayed first/last page groups with both thumbnail dimensions bounded; opening a clicked page preserves the requested selection through asynchronous PDF metadata loading.

Files music playback previews one explicitly chosen track, without a queue, automatic advancement, repeat, or a bottom player dock. Stop playback and cancel pending loads on folder changes through any navigation path, Escape or Space without an active Music range before focused widgets consume them, and manual or automatic switches to List. Space in Music plays the active audio range once from its lower edge to its upper edge; limit the decoded source rather than polling the UI clock. Finalize range edges with core’s zero-crossing search using bounded endpoint decode windows off the UI thread; copying and playback must also wait for snapping. Audio/MIDI/video studios also stop the preview; accepted audio-copy jobs survive playback stops. Waveform frequency colors share core's streaming `WaveformColor` crossover with the workbench. Music cards use bounded visible-entry cover/metadata/peak previews, not per-row studios or retained PCM. Decode waveform summaries as planar packets; admit at most eight visible music jobs on a separate bounded executor, with metadata/cover work overlapping decoding. Retire preview work on scrolling and view changes. Manual media views belong to history locations. Keep central content painting and hit testing inside the space left by outer docks; egui 0.27 allows overflowing frames to grow across those boundaries. Compact dock scroll areas must set a zero minimum scrolled height. Video sessions own one FFplay child and kill/reap it on stop, external handoff, return to the folder or application close; Linux parent-death signalling prevents detached playback after a crash. Probe and play the same non-cover video stream so the pixel budget applies to the decoded stream. Videos uses bounded, cancellable worker-side processes for visible timeline rows; hover scrubbing owns one persistent Timelens decoder on an independent bounded queue. Timeline sampling must follow the requested output width: never stretch a small fixed sample count into thick slices. Use ordinary-frame sampling for short timelines and nearest indexed keyframes for long timelines; decode each selected keyframe once across its contiguous columns. Publish progressive snapshots atomically from persistent decoders, reuse coarse samples in the dense pass, and cache only completed strips. Bound worker-side progress delivery and retire it with the row’s revision/resolution request. Derive column count from the painted strip’s physical pixel width and configured decimation (1/2/4/8/16, default 1). The standalone FFmpeg helper selects worker counts from codec, pixel count, size, duration and visible file count; divide a bounded CPU budget across visible jobs and share one CUDA device per file. GPU timeline reduction stays in that standalone GPL helper, defaults to averaging all frame pixels before downloading a bounded column, and falls back to complete-frame transfer for unsupported formats or reduction failures. Experimental sampling modes use separate preview cache namespaces and retain the original full-frame path/cache for restoration. Report overlapping worker profile totals separately from elapsed time, and subtract nested reads before attributing seek/demux costs. Synchronize submitted CUDA work before frame reuse and release reducer allocations before decoder teardown. Optional Rust io_uring readers own bounded aligned buffers and drain pending reads before closing borrowed descriptors. Enable prefetch only when controlled local/NAS timings justify it. Sparse input on Linux CIFS/SMB2 uses aligned read-only direct I/O to avoid network read-ahead; local files retain buffered caching. Keep hover keyframe sampling independent. Publish metadata ahead of the strip and coalesce pointer positions without a hover pause. Retain the latest completed keyframe while the newest request decodes, label its actual timestamp, and reject replaced source/revision/resolution results. Retire and cancel read-only preview children on navigation, scrolling and studio entry. Use GPU decode/scale for supported HEVC hover previews and CPU decoding for small H.264 previews; large H.264 and HEVC timelines may use CUDA with CPU retry; use pixel count to include cropped/portrait 4K. Reject unsupported CUDA pixel formats before device creation. Keep the GPL helper separate from the application library and retain its corresponding source/license with build artifacts.

Files Photos follows `photo_grid_spec.md`: five shapes on a half-square grid, constrained row candidates with bounded lookahead, even descender boundaries and clean chunk endings. Keep ordered chunks independent, targeting 48 entries but aligning their counts to full square rows at the current grid width. Complete rows and chunk boundaries must have no holes: accept extra cropping instead. Only the folder’s final row may have trailing unused columns. Unknown chunks use filled square placeholders without estimated-height spacers. Keep header/orientation metadata separate from layouts and thumbnails, and all probing/packing/decoding on separate bounded executors. Preserve the visible item anchor when correcting estimated chunk heights. Prioritize visible chunks and thumbnails, cancel obsolete speculative jobs, and reject stale generations. Width changes preserve metadata; source revisions invalidate only affected chunks/previews. RAW previews extract embedded images without developing RAW pixels. Share raster limits, guarded regular-file opening and premultiplied thumbnail conversion.

The file-manager Shelf is a full-width bottom dock with two compact rows of mirror-style preview cards: daemon-owned shared-hand items above, deliberately collected groups below. Do not add a separate local Held card or collect ordinary copy/cut/paste history. Keep the collection actions visible after groups are added, and poll shared summaries independently from collection/send requests. It separates path references from captured content bytes. Keep clipboard MIME import, slot hydration, and reference availability checks off the UI thread; retain full payload content only when collecting the chosen shared slot; visible shared-card previews retain only bounded display data. `PushFileReferences` preserves references even for one image/media file; existing `PushFiles` retains content classification. Shelf content uses shared `Arc` bytes and staged atomic no-overwrite creation. Socket-path resolution belongs in core IPC; desktop clients share the workbench library transport.

Shared display/workbench SVG assets live in `little-clip-render/assets/icons`. Use embedded SVG icons for application chrome, controls, and notifications; do not use emoji glyphs as icons. Keep notification icons separate from their text. Copied summaries and terminal output use plain text; preserve emoji in user content and Unicode rendering fixtures.

Use the shared `ui_tabs::TabBar` for workspace, tool, viewer-mode, and sheet navigation. Keep parameter choices and on/off controls as buttons. Re-selecting an active tab is a no-op; selection changes must preserve existing entry/exit hooks.

UI acceptance automation must use the private Weston/Niri runner. Its pointer injection is scoped to that run's private Wayland and Niri sockets; do not send test input through host `/dev/uinput` or the user's interactive compositor. Niri screenshot actions overwrite the private clipboard; preserve the payload under test or assert it before capturing.

The little screen mirror and rolling audio strip belong only to Quick Palette. Auto Actions and Big Node Machine use their own canvas space. Script-node selection opens the built-in side editor; keep script undo history separate from graph undo, and save drafts before pipeline execution or recipe saving.

Image Studio tool controls belong in the shared left panel, with result panels on the right where needed. Creative FX retains FX selection on the left and effect controls on the right. Shared zoom controls belong in the center of the footer. View shows all metadata fields directly in that panel, and reserves it only when it has fields to display; metadata-free images use the full canvas.

Film Look enables vendored filmr's `compute-gpu` feature; `gpu` alone does not enable compute rendering. Preserve the full-spectrum exposure and dye output model; the upstream GPU development shader uses a different model. GPU grain uses filmr's native density-space shader; CPU fallback retains output-space grain. Apply only one grain model per render. Keep GPU pipelines bound to one shared device, cancellation checks between GPU stages, and CPU fallback when no adapter is available. Validate acceleration with optimized end-to-end timings as well as GPU dispatch tests.

Async preview completion updates an existing clipboard item in place. Keep `SlotStateSummary` sensitive to preview readiness so open workbenches rehydrate the affected slot and repaint the mirror; item ID and payload kind alone do not reveal these updates. Workbench startup loads clipboard state off the UI thread; mirror rendering and operation discovery use the bounded job executor. Share workbench slot snapshots with `Arc<SlotManager>` and use `Arc::make_mut` for local swaps. Reject stale mirror/action completions after state or selection changes, and stale mirror completions after appearance changes; cache discovery separately from search filtering.

Raster import uses core's `item::image_reader` to initialize HEIF/HEIC/AVIF decoder hooks once. Pass MIME hints through dimensions, thumbnails, and comparisons for formats without reliable signatures (TGA); reuse core image extension and MIME classification. HEIF EXIF extraction uses the existing `kamadak-exif` container reader.

Audio Studio channel edits prepare replacement analysis on the bounded job executor. Retain displayed waveforms and spectrum textures while replacements compute; reject stale completions and never replace an edited track with a loading canvas. Spectrum rendering also runs off the UI thread.

Image Studio committed pixels and original/undo/redo revisions use `Arc<RgbaImage>`. Wrap replacement results in a new `Arc`, and use `Arc::make_mut` for in-place edits so background readers and history keep their original pixels.

PDF page rasters belong in a viewer-owned temporary directory. Keep only raster paths in page state and read one compressed page at a time in the bounded decode worker, preserving re-selection after texture eviction without document-sized RAM retention.

PDF navigation uses docked Pages/ToC tabs on the left and Find/OCR tabs on the right. OCR must save a positioned invisible text layer in the PDF while preserving native page content. Opening a source PDF may reuse its `.ocr.edited.pdf` sibling only when the source’s SHA-256 matches the original-document hash embedded in the OCR copy’s PDF Info metadata (`LittleClipOCRSourceSHA256`). Compute and check hashes in the background; missing or invalid hashes fall back to the source. Completed OCR disables the OCR action and rejects repeated job starts, including when an OCR copy is opened directly. Page text selection uses UTF-8 source offsets and normalized visible-page coordinates (including crop and rotation); right-click image copying outlines the chosen image in red and extracts its object without overlaid page text.

Gaussian scene imports and workbench renders run on the bounded job executor. Coalesce navigation into one outstanding render and present ordered camera completions during dragging; reject completions for a replaced session or viewport. Little-screen scene playback uses 240 frames per 12-second orbit in renderer-owned temporary directories under `/tmp`, retaining only one decoded frame per loop. Scene bakes use a separate bounded queue so they cannot starve ordinary preview workers. The mirror acquires daemon-completed orbits over IPC using independently owned hard links in `/tmp`; match the full render identity and fall back asynchronously if frames are unavailable. Retire previews outside the visible slot layout, cancel superseded bakes, and release GPU allocations after baking; renderer shutdown and failed bakes must remove their frame files.

`WorkbenchApp` owns one active `ViewerSession` at a time. CLI previews queued at startup open in their established order; asynchronous completions must wait for the active session to close rather than replace or appear behind it.

Runtime Hundred Rabbits themes and per-color overrides are retired, including on the USB display. Define fixed Light/Dark base colors in core's `palette::BaseColors` and access them through `AppPalette.colors`; switch workbench and display together. Keep base surfaces and text neutral gray; use color for highlights and data. Keep egui-specific semantic roles in `clip-workbench/src/ui_appearance.rs`. Do not reintroduce raw `Theme` fields or rewrite a user's personal `config.toml` while migrating it.

The comparison text viewport uses one dark `console_surface` in both appearances so terminal ANSI colors remain readable. Keep this surface in the central palette; ANSI escape colors represent data, not workbench chrome.

Bundled CJK and monochrome emoji font assets live in `little-clip-render` and are shared with the workbench. Screen text measurement and drawing must use the same glyph fallback selection; prefer the requested system font wherever it has the glyph.

Raw text preview selection spans virtual 4 KiB source fragments. Store its endpoints as absolute UTF-8 source byte offsets and copy from the original content; egui's cross-Label selection inserts layout-dependent separators and drops endpoints that scroll out of view.

File-list text strips share the text viewer's Syntect definitions and palette mapping. Stream whole-file summaries and generate dimension-specific rasters on bounded preview workers; resizing reuses the summary. Map source lines left to right, preserve indentation and empty-line density, and aggregate before rasterization. Hover excerpts are bounded, precolored source samples; previews retain ordinary row selection/opening and never consume wheel input. Keep lightweight text classification while retiring offscreen summaries so variable-height rows remain stable. Resolve `.ts` source-versus-video classification on workers and use it consistently for rows, automatic views, and viewer opening.

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
