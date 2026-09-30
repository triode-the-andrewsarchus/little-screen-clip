# Workbench UI architecture

This document describes the architecture present in the source tree. It is not a specification that every view must use identical chrome or a record of unverified runtime behavior. The actionable implementation queue and acceptance evidence live in [ui_overhaul_remaining_tasks.md](ui_overhaul_remaining_tasks.md) and [ui_overhaul_acceptance.md](ui_overhaul_acceptance.md).

## Shared contracts

`clip-workbench` owns desktop presentation and per-view editing state. Shared data, configuration, IPC, and the fixed Light/Dark palette live in `little-clip-core`; the hardware display is rendered by `little-clip-render`. Workbench and display resolve the same `WorkbenchAppearance`, while desktop widget roles are mapped in [ui_appearance.rs](crates/clip-workbench/src/ui_appearance.rs).

The reusable UI primitives are in [shell/](crates/clip-workbench/src/shell/):

- `StudioShell` renders a viewer header (title, icon, badges, actions, escape hatches, and close) and a footer (help hints, optional readout, and status toast). Views choose their own toolbars, workspace, panels, and domain-specific interactions; the shell does not impose a complete five-zone layout.
- `EscapeHatch` describes an external-app action. `EscapeHatchLauncher` starts the configured command and falls back to `xdg-open`, reporting the outcome through the view's toast. It does not own or close the viewer.
- `StudioPanelLayout` computes optional left, center, right, and bottom rectangles. It shrinks horizontal rails and moves them into compact rows when available space is limited. `show_side`, `show_region`, and `show_bottom` provide clipped content surfaces. `PanelState` stores resize and collapse preferences with its owning view state; these values are not persisted to disk. Only views that wire a particular control expose that interaction.
- `SplitCompareView` shares the interactive before/after divider used by image comparison tools. `studio_controls` contains reusable track sliders and slider helpers.
- `ScreenMirrorWidget` derives hit-test rectangles from `little-clip-render` slot layout and reports click, context, and drag-swap actions to the host.

These are composable primitives, not a mandate to move domain behavior into the shell. Tool actions, playback, selection, editing, and data-specific status remain with their owning view.

## View and job ownership

`WorkbenchApp` in [main.rs](crates/clip-workbench/src/main.rs) owns at most one foreground `ViewerSession`. The session enum contains the concrete view state, including an image-loading state while an image is prepared. CLI-startup previews are queued in FIFO order; asynchronous viewer completions wait while another viewer or startup preview is active. Closing a viewer advances that queue.

Expensive loads and exports use `BackgroundJobs` or a view-owned worker and deliver results through channels. The state that starts a job owns its ticket, receiver, and publication decision. Replacing or dismissing a viewer cancels its ticket and prevents stale publication; a running operation may finish when its worker cannot be interrupted. Views poll completed work and decide whether the result is still current before publishing it. There is no universal job type for all viewer operations, and a few lightweight or legacy entry paths remain synchronous; see the task queue for audited work still open.

The host's slot state is polled separately from egui rendering in [state_sync.rs](crates/clip-workbench/src/state_sync.rs). Slot summaries detect changes, and changed or still-progressing previews are hydrated from the daemon. `preview_phase` matters even when item identity and payload kind stay the same, so the mirror can repaint when a preview becomes ready.

## Preview and reuse boundaries

The daemon's `ClipItem` payload and slot summary are the shared source for slot cards and the workbench mirror. The mirror texture comes from `ScreenRenderer`; the widget reuses the renderer's slot geometry for interaction. It does not reuse a viewer's egui textures or editor state.

Opening a viewer creates view-specific state, but immutable data-sharing support differs by format:

- Image loading decodes into a cloneable `DecodedImageData` containing encoded bytes, original pixels, dimensions, format flags, and extracted metadata. `ImagePreviewState` owns path/title context, editable pixels, navigation, tools, jobs, and egui textures; image bytes and pixels can be shared across states, with pixel edits detaching through `Arc` copy-on-write. Workers return decoded data plus path/title context, and the editor state is constructed when the result is published. Save As and copy workers consume captured composition snapshots and return results to that editor state.
- `TextPreviewData` shares normalized source text and its detected language/Markdown mode and compact code-fragment layout; each `TextPreviewState` clones those `Arc`s while keeping search, selection, scroll, minimap, jobs, and egui state independent. The current route constructs one data object per load; future inline hosts still need a shared source owner. `MidiViewerState` stores parsed notes as `Arc<MidiData>`, with transport and selection viewer-owned. `SpreadsheetViewerState` can share parsed workbook data through `Arc<SpreadsheetData>`, with sheet selection, filtering, measurements, cell selection, and scroll kept per viewer. `Model3dViewerState` stores immutable mesh data as `Arc<Model3d>` and supports independent viewers from a shared source. The workbench file loader stays uncached. The USB card renderer's Model3D path cache is limited to 128 MiB of model-owned allocations and 16 entries, with LRU recency updated by rendered-card access; local external glTF buffer/image metadata participates in identity, ambiguous URI forms bypass reuse, and `.glb` remains uncached until its JSON dependencies are scanned. Byte-backed meshes do not enter the path cache.
- `AudioPreviewState` keeps path identity, progressive analysis/readiness, playback/edit state, jobs, and textures session-owned. After the analysis job emits `Complete`, it publishes an immutable `Arc<FullAudioAnalysis>` source snapshot; clones share the analysis object and its large buffers. Progressive updates remain confined to the session, and edited waveform/spectral results use a separate Arc-owned analysis. `InlineAudioWaveform` renders from that source by querying only the visible clipped pixel columns, capped at 16,384 columns; `AudioPreviewState::from_completed_source` creates an independent editor view without another source-analysis job. Workbench host wiring and simultaneous multi-host behavior remain under H2.4g. PDF page raster files, dimensions, and page numbers live in immutable Arc-shareable `PdfPageRasterResource` values that retain the temporary-directory owner. `PdfEditorState` retains its bounded selected-first decode queue and texture LRU, while `InlinePdfPagePreview` owns one independent decode slot and texture; both use the same page decode helper and may hold the same raster resource. PDF inline host wiring remains separate from this view-level sharing API.

There is not yet a cross-view preview cache or general-purpose preview widget cache. Do not force these formats through one generic preview trait: their source identity, readiness, cancellation, and backing-resource rules differ. The task tree tracks the format-specific sharing, embedding, and bounded-lifetime work.

## Appearance and platform split

`WorkbenchAppearance` is the shared `Dark`/`Light` setting in core configuration. `AppPalette::for_appearance` supplies fixed palette values to both desktop and display rendering. [ui_appearance.rs](crates/clip-workbench/src/ui_appearance.rs) maps that palette to egui semantic roles and widget visuals. Appearance reload runs asynchronously through `AppearanceSync`, which keeps daemon failure visible in the workbench. Legacy per-color settings are migration input and do not define runtime theme selection.

The workbench remains an egui application. Screen-adaptive startup sizing, singleton activation, compositor placement, and USB hardware behavior depend on their respective platform/runtime paths and require the scenarios recorded in the acceptance sheet; source-level geometry tests alone do not establish those runtime results.

## Known limits

Shared primitives are adopted view by view. Domain-specific headers, tool strips, dialogs, and status regions remain where their behavior requires them, so not every surface has the same footer or panel profile. `PanelState` has collapse fields, but collapse controls are only present where a view wires them; shared panel geometry does not automatically make every rail collapsible. Background scheduling and cancellation are also not uniform across every operation. Consult the remaining task tree for the current source gaps and runtime checks rather than treating the original broad design proposal as implemented.
