# Native audio and FFmpeg library migration

## Objective and completion gates

Use native Rust audio processing where it is practical, and migrate remaining
FFmpeg processing to `ffmpeg-next`. Do not introduce new GStreamer integration
during this work. Preserve supported formats, source identity checks, bounded
preview work, cancellation, and accepted export completion.

- [ ] Inventory and migrate production FFmpeg/FFprobe call sites.
- [ ] Reuse native decoding and DSP rather than adding parallel implementations.
- [ ] Verify FFmpeg 9 compatibility with real decoding/filtering/encoding tests.
- [ ] Preserve pinned input and seekable staging output handles.
- [ ] Check cancellation, malformed inputs, stream selection and time ranges.
- [ ] Run affected tests, final workspace checks and architecture comparison.
- [ ] Review the evidence below and write a final GStreamer recommendation.

## Starting architecture

`little-clip-core/src/audio/decode.rs` already uses Symphonia for common-format
metadata, waveform and spectral decoding. `audio/analysis.rs`, `audio/types.rs`
and `audio/waveform.rs` provide native analysis and edit processing. Extend these
owners instead of introducing another general audio subsystem.

Remaining process boundaries include:

| Source (under `crates/`) | Current responsibility | Migration considerations |
| --- | --- | --- |
| `little-clip-core/src/audio/source.rs` | FFprobe and FFmpeg preparation for ASF/XWMA; native NCW handling | Keep bounded prepared PCM, revision-aware cache and guarded handles. |
| `clip-file-manager/src/file_manager/music/preview.rs` | FFprobe tags/technical metadata; cover through `media_frame` | Prefer native metadata/art extraction for ordinary music. Preserve technical details and uncommon-format support. |
| `clip-studios/src/audio_preview/export.rs` | Audio range exports and session filter graphs | Preserve edit order, sample format, seekable staging, atomic installation and accepted-write lifetime. |
| `little-clip-ops/src/pipeline/content.rs` | Media conversion operations | Inspect operation-specific codecs, options and output guarantees before replacement. |
| `clip-file-manager/src/file_manager/media.rs` | Thumbnail/frame extraction | Preserve non-cover video selection and bounded image decoding. |
| `clip-file-manager/src/file_manager/video_timeline/backend.rs` | Persistent scrubbing and timeline helper control | Existing lifecycle, sampling policies and cancellation remain requirements. |
| `clip-search/src/reader/video.rs` | Isolated bounded video extraction | Retain disposable-helper resource limits and sandbox boundary. |
| `clip-search/src/reader/media.rs` | Shared media probe and transcription audio windows | Keep format allowlists, selected-stream offsets and bounded mono extraction. |
| `clip-file-manager/src/file_manager/viewers/video.rs` | One owned FFplay session | Playback replacement needs audio/video synchronization and rendering, not just codec bindings. |
| `little-clip-core/src/animation.rs` | Animation conversion | Compare native format support and timing fidelity before selecting a backend. |

The C helper in `scripts/video_strip/main.c` already links directly to FFmpeg.
A Rust wrapper alone does not improve its GPU reduction or indexed sampling.
Timelens is already a separate GStreamer-based GPL helper, documented in
`scripts/README.timelens.md`; this migration must not accidentally link that
helper's implementation into the application.

## GStreamer evaluation tied to code

### Video playback and scrubbing

The video studio and `file_manager/video_timeline/backend.rs` need persistent
decoding, seeking, reliable shutdown and consistent stream-relative timing.
GStreamer's synchronized playback and seek machinery could reduce bespoke
player coordination here. Evaluate whether it actually simplifies embedded
audio/video presentation and frame delivery before adopting it. Decoder reuse
is also possible with FFmpeg; a framework change is not evidence of a speedup.

### Dense timeline rendering

`scripts/video_strip/main.c` has specialized indexed sampling, CUDA reduction
and bounded progressive output. Replacing it with a general pipeline risks
losing those optimizations. Require equivalent local/NAS timings, sampling
accuracy, cancellation and memory bounds before considering GStreamer here.

### Ordinary audio

`audio/decode.rs` and native region DSP already supply application-owned samples
to analysis and playback. A second decoding pipeline would need to justify its
buffer transfers, device ownership and format differences. There is currently
no evidence that GStreamer would simplify these paths.

### Filtering and exports

`clip-studios/src/audio_preview/export.rs` uses FFmpeg filter graphs. GStreamer
would require validated equivalents for the actual effects and export codecs.
Keeping FFmpeg's filters through library APIs is a more direct migration;
native DSP should use the existing edit implementation where equivalent.

### Distribution and isolation

Both frameworks have native dependencies; Rust bindings alone do not bundle
them. GStreamer adds plugin selection and deployment. Preserve existing helper
licensing boundaries, and choose the FFmpeg build configuration deliberately.
Keeping persistent decoding in a helper can retain forced cancellation and
crash isolation, but starting a fresh helper per operation retains startup cost.

## Work log

- [x] Native music artwork extraction and its targeted core tests.
- [x] Separate FFmpeg 9 library worker, initial probing and lossy-depth tests.
- [x] Rewind inherited inputs before probing, with an EOF-position regression test.
- [x] Keep video dimensions separate from attached cover artwork; regression fixture passes.
- [x] Migrate and verify production metadata callers (music, video, format bridge,
  search and animation metadata).
- [x] Native streaming WAV ranges and simple gain, with owned staging and retry cleanup.
- [x] Studio exports/session graphs and pipeline audio filters through library jobs.
- [x] Search audio windows through bounded library PCM streaming.
- [x] Browser thumbnail/cover fallback and search still-frame library jobs.
- [ ] Animated/video preview batches and timeline fallback migration.

- Established architecture baseline `native-audio-ffmpeg` at `89ccc61`.
- Confirmed common audio already uses native decoding and DSP; migration must
  remove remaining processes without duplicating those implementations.
- Added bounded native artwork extraction in `audio/cover.rs` and connected
  music cards to it. Unsupported readers retain the old fallback temporarily.
  All three targeted core artwork tests pass: embedded ID3 artwork, front-cover
  preference/image bounds, and missing-artwork/cancelled requests.
- Added a separate `clip-media-worker` executable using FFmpeg 9 custom
  stream I/O. It must remain an independently linked process, rather than a
  dependency of the desktop binaries. Its initial probe implementation is
  not yet fully connected to production callers; filter/render/export migration
  remains. Four worker tests pass against the installed FFmpeg 9.0.2 libraries,
  including ordinary-handle probing and compressed bit-depth regression coverage.
- Compared worker metadata with FFprobe on generated WAV, FLAC, MP3, AAC/M4A,
  Ogg/Vorbis and WMA: codec, rate, channels, sample format, layout, bit depths
  and profile match. Verified MP4 dimensions and rejection of secondary file
  opens through a concat input. CLI tools were used only to generate/reference
  the smoke fixtures; the worker itself calls FFmpeg libraries directly.
- Production metadata migration is being verified in the file-manager host.
  Search and ASF/XWMA metadata callers retain their existing demuxer allowlists,
  process deadlines and cancellation while moving to the worker protocol.
- Host music checks pass: three tests passed, one manual benchmark ignored.
  Five worker tests pass, including probing from an inherited handle at EOF and
  direct compressed-audio decoding into interleaved float PCM.
- Switched ASF/XWMA preparation to library decoding. Six core source/cache tests
  pass (one external-reference test remains explicitly ignored). All four bundled
  mono/stereo WMA/XWMA fixtures produce sample-for-sample identical float PCM to
  the former CLI path. The search audio-window contract test also passes.
- Added executable-level descriptor and secondary-open tests; all seven worker
  unit/integration checks pass. Workspace tests now build the actual helper
  binary as part of the integration-test artifact graph.
- Migrated animation metadata to the worker and shared non-cover stream selector.
  A generated MP4 containing a 160 × 90 video and 32 × 32 cover verifies the
  original last-stream overwrite bug is fixed. Its core regression test passes.
- Initial architecture comparison: duplicate groups remain 1515, maximum CCN
  remains 195 and cycle count remains 16. The reported regression attributes
  existing clones to the two-line module export addition and identifies the
  unchanged `video_workload_benchmark.py` cycle with its members reversed.
  Checked the actual diff; these reports do not identify newly added logic.
- Symphonia 0.5 exposes metadata allocation options, but its metadata reader
  implementation does not consistently enforce them. Do not treat those options
  as a hard parsing-memory guarantee. Artwork decoding enforces its own byte,
  image-dimension and allocation limits; review parser budgets during migration.

- Added `audio/render.rs` using the existing guarded native reader and sample
  conversion buffer. Ordinary mono/stereo WAV export and pipeline gain stream
  packet-sized PCM into owned staging; large RIFF outputs and incompatible
  layouts/timing defer to the codec backend. Failed attempts clear partial WAV
  output before retry. Source and directory identities remain checked before
  publication; the same admitted inputs/staging are retained through fallback.
- Native selections use source packet clocks, decoder preroll and Opus pre-skip,
  rather than accumulating decoded frame counts. Explicit generated-fixture
  checks cover FLAC, MP3, raw AAC, Vorbis and Opus. MP4/M4A native export is
  intentionally deferred: Symphonia 0.5 parses but does not apply edit-list timing.
  Recognition uses content, including misleading filename extensions.

- Native rendering verification: six core renderer checks (including the explicit
  compressed reference test), 106 audio-studio checks, five pipeline content
  checks and seven file-source checks pass. All seven worker checks also pass
  after separating converter setup and tail draining from packet delivery.
- Architecture follow-up removed the new decoder/encoder control-flow hotspots
  and repeated captured-handle validation. Maximum CCN and dependency cycles are
  unchanged. The staging orchestration's CCN increases from 15 to 19 because
  native dispatch and its fallible handle/retry operations are additional paths;
  retaining those explicit errors is necessary. Remaining clone reports largely
  touch unchanged waveform/animation/test blocks, plus ordinary renderer test
  setup. No second decoding cache or application FFmpeg linkage was introduced.

- Added `clip-media-protocol` for shared, data-only render settings. Core owns
  process requests; `clip-media-worker/src/audio_render.rs` owns FFmpeg decoder,
  filter and encoder contexts. This keeps the separately licensed worker from
  depending on core, and prevents request-schema duplication or native library
  linkage into the applications.
- Migrated `clip-studios/src/audio_preview/export.rs` and pipeline content audio
  operations from CLI option lists to structured library requests. Studio jobs
  keep their accepted-write cancellation/reaping lifetime, seekable staging and
  pinned inputs. Pipeline jobs keep their bounded `CommandRequest` supervision,
  process-group handling and source checks before publication.
- Graphs retain gain, pan, clipping, low/high-pass, crossfade/mix, deletion/concat,
  loudness and silence-removal behavior. `audio_filter.rs` inspects parsed filter
  names before initialization, rejecting secondary file-opening/source filters;
  demuxer protocol restrictions alone do not cover graph sources.
- Executable checks cover exact stereo selection and mono pan/gain, concatenated
  inputs, graph/encoder draining, regular staging requirements, rejected graph
  sources, and re-opened FLAC/MP3/M4A/Vorbis/Opus/WMA/AIFF exports. A manual
  compressed-fixture check compares source-relative selections against full-decode
  CLI trims for FLAC, MP3, M4A, Vorbis and Opus. Near MP3 priming, CLI input seeks
  can emit warm-up silence; a full-decode trim is the timing reference instead.
- The broad host container/export check caught a missing MP2 output allowlist
  entry, which is restored. It now passes across all its existing format/codec
  combinations; five pipeline content checks also pass through the library worker.

- Library export verification after final negotiation changes: all 106 audio-studio
  tests and five pipeline content tests pass. All 13 worker unit/integration
  checks pass with the manual compressed-reference test explicitly included.
  Worker/protocol all-target Clippy passes with warnings denied; workspace
  doctests and formatting also pass. Disk free space remains about 158 GiB.
- Architecture follow-up separates command parsing and muxer setup from decode
  delivery. No new worker function exceeds the CCN threshold; maximum workspace
  CCN and cycle counts stay unchanged. The report still counts two additional
  clone groups, including fixture setup and typed export request construction;
  shared wire defaults now have one protocol constructor. Existing studio
  staging orchestration retains the previously documented necessary error paths.

- Removed the now-unused exported `decoder_input_args` and embedded CLI option
  arrays from `audio/source.rs`. Header recognition now supplies the actual
  `asf`/`xwma` worker demuxer name directly, while extension hints still select
  deferred preparation. Six source/cache checks pass; the external-reference
  fixture check remains explicitly ignored until its optional path is supplied.

- [x] Preserve gaps at a selected range's beginning and bound resampler tails.
  The library renderer subtracts the requested source start rather than the
  first decoded sample's timestamp. It trims the negotiated output duration
  after processing so resampler flush samples cannot overrun the selection.
  Strengthened the search packet-gap fixture to include a gap across the second
  window's start. All seven search video/source-clock tests and all 13 worker
  checks (including the explicit compressed reference) pass with this fix.

- Migrated `clip-search/src/reader/media.rs::audio_window` to the existing
  library render worker. It retains the selected global stream index, container
  origin, 30-second windows, five-second overlap, two-hour horizon, mono 16 kHz
  PCM/WAV contract, 2 MiB capture limit and 60-second supervised process deadline.
  It creates no staging path inside the disposable reader.
- Raw PCM pipe output is restricted to explicit short ranges and bounded
  rate/channel settings, with a separate 32 MiB writer cap. Ordinary container
  exports still require owned regular staging handles. The pipe test verifies
  exact frame count and channel/gain selection after resampling; a writer-budget
  test verifies rejection without partial overflow writes. Seven worker unit and
  seven ordinary integration checks pass; the compressed reference check also
  passed explicitly before adding the writer-budget test. Search's quiet/tail/
  horizon check and all seven video/source-clock checks pass. Worker all-target
  Clippy is clean. Architecture comparison reports no new complexity hotspot;
  its additional clone is test request setup rather than a parallel media path.

- Migrated browser `file_manager/media.rs::media_frame` and search
  `reader/video.rs::frame` to `clip-media-worker/src/video.rs`. The worker selects
  the same explicit stream, checks source dimensions before opening its codec
  and after decoding, applies display orientation and scales to the requested
  bounds. Parent preview/search capture limits and deadlines remain in place.
- Still-frame replies carry a bounded typed header with the actual decoded
  source-relative timestamp and PNG dimensions/length. The data-only protocol
  parser checks incomplete/trailing payloads, dimensions and missing timestamps.
  Search no longer scrapes `showinfo` logs; its sparse-frame retry still returns
  the preceding available frame with that frame's observed timestamp.
- Verification: all seven worker unit and ten executable checks pass, including
  both explicitly enabled manual-reference checks. Rotation snapshots at 0/90/
  180/270 degrees exactly match independently decoded CLI reference RGBA pixels.
  All six current search video checks pass (the old log-parser-only check is
  replaced by the typed-header check). Three browser media checks and search's
  quiet-window check pass, including actual red-video/blue-cover thumbnails.
  Worker/protocol all-target Clippy passes with warnings denied.
- Updated `file_manager_video_search_acceptance.py` to inject controlled failures
  into the library worker rather than the retired CLI path. Python compilation
  and an isolated invocation verify both late-frame rejection and successful
  descriptor-preserving handoff. This does not claim the real-model GUI
  acceptance scenario was run.
- Architecture comparison keeps maximum CCN/cycle counts unchanged. The new
  source setup, frame delivery and wire parser score 16 due to their explicit
  stream, bounds and failure checks; those checks remain together rather than
  being scattered solely to reduce a metric. New clone reports include fixture
  request setup; shared response/request owners avoid a parallel preview stack.

## Final recommendation

Pending implementation and verification. This section must be reviewed after
the native audio and FFmpeg library migration, using actual remaining code and
measured limitations. No new GStreamer work is authorized by this provisional
assessment.
