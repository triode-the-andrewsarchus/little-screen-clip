# Presentation boundaries and format capabilities

This is an implementation inventory for the deep refactor, not a format registry
or authorization to add missing editors. The product roadmap remains
[File Manager/plan.md](<File Manager/plan.md>).

## Hints, observed content and actions

[file_format.rs](../crates/little-clip-core/src/file_format.rs)
provides cheap path hints for semantic documents. Entry admission, strip sources
and studio dispatch reuse that enum. A hint does not establish readability,
content identity or write support. Studio dispatch carries the shared format
rather than translating its variants into a second classification enum.

Multisamples are the classification pilot. Distinct extensions such as `.sfz`
and `.sf2` select a bounded parser, whose failure remains a failure rather than
proof of a playable instrument. Ambiguous `.xml` and `.exs` names keep their
ordinary handling unless the existing worker-side
[`read_if_identified`](../crates/clip-file-manager/src/file_manager/multisample/mod.rs)
returns a parsed document with mapped regions. Generic `.preset` and `.iso`
files are not admitted by resemblance. List/Shelf and studio workers use this
same observation path. The prepared `Document` enables the existing mapping and
audition controls; a filename alone does not. Parsing does not guarantee that
all referenced samples, effects or scripting can be played.

Ordinary-source guards precede probing. Search preview sources also retain
`PreviewContext`; row results are accepted only for their current identity and
revision. Hidden/obsolete work is retired through the existing preview cache.
No new registry, probe queue or cache is needed for this pilot. Protected aliases
must not acquire a document simply because the alias ends in `.xml`.

## Prepared audio as a shared presentation input

Core already supplies two useful levels of prepared audio:

- [`AudioWaveform`](../crates/little-clip-core/src/audio/types.rs) is a bounded
  summary for clipboard cards. `AudioRef.waveform == None` means pending;
  an empty prepared waveform means unavailable; a populated waveform supplies
  peaks, optional colors and snippet position. Raster audio cards consume this
  data without opening the payload's source path. Shelf cards also consume the
  captured summary rather than re-decoding the referenced audio.
- `FullAudioAnalysis` supplies shared PCM and peak pyramids for studio and inline
  exploration. `peak_blocks_for_range` borrows existing summaries for both core
  and rolling-strip queries. Core averages block colors; the rolling strip uses
  the strongest block's color. Interaction and selection stay in their callers.

The owning clipboard item/session supplies identity and lifetime. Source checks
and decoding stay with its existing worker; a prepared summary is not permission
to reopen its path. Workbench mirror jobs retain a clipboard/selection snapshot
and appearance generation, rejecting obsolete completions. Slot-state summaries
include preview readiness so an asynchronously completed summary invalidates the
visible mirror. No prepared-content cache independent of these owners is added.

A native mirror can consume titles, metadata, prepared audio and image/animation
thumbnails from the same snapshot. It must not call the legacy raster renderer
on the UI thread: `FileBatch` cards still contain worker-side spreadsheet/MIDI,
PDF/Office, code and scene preparation. Those paths need an existing worker
result or bounded raster fallback until individually extracted. The hardware
renderer and exact display-reference view remain available. This is a concrete
migration boundary, not a claim that every raster card is already a pure painter.

## Current presentation coverage

“Fallback” means an ordinary reference/listing, generic metadata or external
opening; it does not promise a specialized visual. Dedicated media grids and
Shelf cards have different layouts. This inventory records capabilities, not
one compulsory visual for every context.

| Family | Hardware/raster card | Grid/Shelf | Inline listing | Studio/inspector |
| --- | --- | --- | --- | --- |
| Images, RAW, layered images | Prepared image thumbnail; RAW/layer support follows import preparation | Photo layout, thumbnails, bounded analysis | Image strip/hover | Shared Image Studio, with supported edits and source-aware exports; layered imports are not a general layer editor |
| Audio | Prepared waveform, pending/unavailable states | Music summaries/cover/waveforms; captured Shelf waveform | Shared waveform interaction | Audio Studio, supported processing and protected exports |
| Video/animation | Prepared representative frame | Video timelines and hover decoder; Shelf representative preview | Bounded timelines | Video preview/playback; no general video editor |
| MIDI | Worker-parsed score card for a single reference | Prepared score/summary | Piano-roll range preview | Shared MIDI playback, range export and synthesis |
| Multisamples | Reference fallback | Prepared mapping summaries where available | Interactive note/velocity strip | Shared mappings, harmony and audition; partial sampler semantics |
| PDF/Office | Worker-prepared page/document visual | Bounded page/summary previews | Page strip | PDF tooling or converted Office preview; no general Office authoring |
| Text/subtitles | Text/code/Markdown or reference fallback | Prepared text/metadata | Text minimap or subtitle cues | Source-aware text editing; subtitle navigation shares the text host |
| Fonts/collections | Reference fallback | Prepared specimen/summary | Specimen strip | Font specimen/collection inspection |
| Notebooks | Reference fallback | Prepared cell summary | Notebook cells | Notebook inspection; no notebook execution |
| EPUB/CBZ | Reference fallback | Prepared book summary | Chapter/page strip | Book inspection |
| Mail/calendar | Reference fallback | Prepared message/event summary | Attachment/event strip | Message/calendar inspection; no mail delivery or calendar synchronization |
| SQLite | Reference fallback | Prepared schema/summary | Table/schema strip | Read-only guarded inspection; no database mutation UI |
| GPX/GeoJSON | Reference fallback | Prepared geometry summary | Geography strip | Geography inspection |
| G-code | Reference fallback | Prepared toolpath summary | Toolpath strip | Toolpath inspection; no device control |
| 3D/splats/materials | Existing worker-prepared scene/model visual or fallback | Scene/material previews | Domain-specific strips | Existing model/scene/material inspection tools |
| Archives | Reference fallback | Generic reference/binary fallback | Generic/binary fallback | Existing fallback/external opening; no new archive editor promised |
| System interfaces | Separate policy; not ordinary content | Typed bounded metadata | System summaries | Dedicated system inspector; no generic payload decoding |

The implementing dispatch is in
[viewers.rs](../crates/clip-file-manager/src/file_manager/viewers.rs),
[previews.rs](../crates/clip-file-manager/src/file_manager/previews.rs),
[Shelf preview sources](../crates/clip-file-manager/src/file_manager/shelf/preview.rs)
and [raster cards](../crates/little-clip-render/src/cards/mod.rs). Detailed format
support remains in [file_types.md](file_types.md). These are current source-level
capabilities, not a claim of end-to-end acceptance for every format combination.


## Shared UI ownership

`clip-ui` owns generic controls, navigation tabs, SVG icon adapters, appearance
and font setup. Its local crate dependency is core, not an application or the
raster/scene renderer. Core still has broad media/document dependencies; this
first extraction does not make an isolated UI build independent of those.
Core's `font_assets` owns fallback byte constants, reexported by render. Fonts
and icons retain one canonical asset location and their original licenses.

The workbench forwards its old UI module paths during migration. Shared
audio-device access and native-dialog availability now live in studio runtime
code; those services are not generic widgets. Neither
application's lifecycle or internal worker dispatch moves into `clip-ui`.

## Shared studio ownership

`clip-studios` owns the existing desktop studios, their preview/export sessions,
shared shell controls and barcode/conversion workers. It depends on `egui`
directly rather than a window host. It has no browser or popup lifecycle owner.
`clip-ui` supplies generic widgets; core owns guarded `FileSource`/`ReadCheck`.
The physical screen mirror remains workbench-owned.

Both executables retain internal worker dispatch before ordinary argument parsing.
Host tests opt into explicit fixture helpers through `test-support`; normal
builds do not enable that feature. Subtitle fixtures are shared from core.

The browser resides in `clip-file-manager`, which has no workbench dependency.
Both applications depend directly on the neutral studio/UI owners.
Navigation, current-folder assumptions, selection and history stay in the browser;
these have not been promoted into shared studio APIs. Studio sessions retain
accepted write completion and document edits while hosts control navigation.


## Opt-in native mirror prototype

`LITTLE_CLIP_NATIVE_MIRROR=1 ./launch.sh ui` enables the experimental **Native**
and **Display reference** tabs in Quick Palette. The normal launch still uses
the raster mirror. Set the flag when starting a fresh workbench process; a second
invocation only focuses the existing popup. Neither the hardware renderer nor
its exact display reference has been replaced. These controls are specific to Quick Palette.

`ScreenRenderer::render_pixmap_with_previews` returns the same reference pixels
plus bounded visible text runs from the existing classification/highlighting
pass. The normal raster entry point does not collect these runs. Native text
uses their existing contrast-adjusted colors; it does not repeat file detection,
syntax analysis or content reads. Structured text, Markdown, images, file
references and other specialized cards retain their worker-produced raster
presentation in the prototype. Audio uses the already prepared peak/color
summary; no decoder or PCM buffer is introduced. Native text admission allows at
most 32 prepared lines, 128 spans per line and 2048 UTF-8 bytes per line; larger
prepared text retains its complete raster fallback. The prototype also paints
color swatches and empty cards natively. Raster and native slot accents share core's
palette policy.

The mirror worker prepares bounded display metadata and text runs alongside
its reference image. Native painting holds bounded labels, text and peak
summaries rather than a clipboard snapshot, source handles, full image payloads
or PCM, so it cannot force extra clipboard copy-on-write. Prepared waveform
summaries up to 8192 entries per channel/color series are copied on the
worker without reanalysis or renormalization; larger summaries retain the raster
fallback. Both results publish under the existing state/selection and appearance
checks. The native model's appearance stamp falls back to the coherent raster
reference during a color-mode change, until new text colors are ready. This adds
no independent decoder, thumbnail cache, job queue, player or mutation path. Slot geometry and click/inspection/context/drag handling
remain in `ScreenMirrorWidget`; changing the presentation does not change input
routing. The reference image remains retained for comparison and fallback, so
this prototype is not a memory-saving replacement.

Reproduce the mixed-card private capture with:

```sh
LITTLE_CLIP_NATIVE_MIRROR=1 TESSDATA_PREFIX=/tmp/little-clip-tessdata \
  python3 scripts/native_mirror_acceptance.py --binary target/debug/clip-workbench \
  --appearance dark --settle-ms 12000 --move 1100 760 --output-dir /tmp/native-review-new
```

Omit the environment flag for the raster baseline; use `--appearance light` or
`--scale 2` for the other presentation checks. The runner uses a private daemon,
Weston/Niri instance and owned fixtures. Its content assertion prevents a window
that is still preparing the preview from passing as a completed visual check.
The wait is test setup, not a claimed startup latency.

The ignored `shell::native_mirror::tests::compare_native_and_reference_frames`
test compares warmed headless CPU painting/tessellation at two widths and two
pixel densities. It reports the actual build profile, preparation timings,
vertices, text string capacities and reference pixel bytes. It excludes GPU
presentation, physical input latency and complete process/GPU memory accounting.
Private captures additionally sample the owned workbench child's RSS/high-water
RSS. These measurements must not be presented as a guaranteed frame rate or a
performance win. Native adoption remains a separate user decision.

Shared photo preprocessing now lives in core's
[`gallery_image`](../crates/little-clip-core/src/gallery_image.rs): header metadata,
resolution-specific thumbnails and appearance hashes retain the same separate
caches and guarded source/revision checks. Browser photo rows, Shelf, similarity
preparation and search readers use those implementations. Cache/layout/job
ownership in the browser remains distinct from prepared image data; this move
does not introduce another cache or make decoding a UI-frame operation.
