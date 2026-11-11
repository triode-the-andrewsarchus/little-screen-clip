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

- Established architecture baseline `native-audio-ffmpeg` at `89ccc61`.
- Confirmed common audio already uses native decoding and DSP; migration must
  remove remaining processes without duplicating those implementations.
- Added bounded native artwork extraction in `audio/cover.rs` and connected
  music cards to it. Unsupported readers retain the old fallback temporarily.
  All three targeted core artwork tests pass: embedded ID3 artwork, front-cover
  preference/image bounds, and missing-artwork/cancelled requests.
- Preparing a separate `clip-media-worker` executable using FFmpeg 9 custom
  stream I/O. It must remain an independently linked process, rather than a
  dependency of the desktop binaries. Its initial probe implementation is not
  yet connected to production callers; filter/render/export migration remains.
- Initial architecture comparison: duplicate groups remain 1515, maximum CCN
  remains 195 and cycle count remains 16. The reported regression attributes
  existing clones to the two-line module export addition and identifies the
  unchanged `video_workload_benchmark.py` cycle with its members reversed.
  Checked the actual diff; these reports do not identify newly added logic.
- Symphonia 0.5 exposes metadata allocation options, but its metadata reader
  implementation does not consistently enforce them. Do not treat those options
  as a hard parsing-memory guarantee. Artwork decoding enforces its own byte,
  image-dimension and allocation limits; review parser budgets during migration.

## Final recommendation

Pending implementation and verification. This section must be reviewed after
the native audio and FFmpeg library migration, using actual remaining code and
measured limitations. No new GStreamer work is authorized by this provisional
assessment.
