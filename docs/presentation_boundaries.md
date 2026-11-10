# Presentation boundaries and format capabilities

This is an implementation inventory for the deep refactor, not a format registry
or authorization to add missing editors. The product roadmap remains
[File Manager/plan.md](<File Manager/plan.md>).

## Hints, observed content and actions

[document_format.rs](../crates/clip-workbench/src/file_manager/document_format.rs)
provides cheap path hints for semantic documents. Entry admission, strip sources
and studio dispatch reuse that enum. A hint does not establish readability,
content identity or write support. Studio dispatch carries the shared format
rather than translating its variants into a second classification enum.

Multisamples are the classification pilot. Distinct extensions such as `.sfz`
and `.sf2` select a bounded parser, whose failure remains a failure rather than
proof of a playable instrument. Ambiguous `.xml` and `.exs` names keep their
ordinary handling unless the existing worker-side
[`read_if_identified`](../crates/clip-workbench/src/file_manager/multisample/mod.rs)
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
[viewers.rs](../crates/clip-workbench/src/file_manager/viewers.rs),
[previews.rs](../crates/clip-workbench/src/file_manager/previews.rs),
[Shelf preview sources](../crates/clip-workbench/src/file_manager/shelf/preview.rs)
and [raster cards](../crates/little-clip-render/src/cards/mod.rs). Detailed format
support remains in [file_types.md](file_types.md). These are current source-level
capabilities, not a claim of end-to-end acceptance for every format combination.
