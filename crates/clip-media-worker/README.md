# FFmpeg library worker

This executable hosts `ffmpeg-next` independently from the desktop binaries.
It does not spawn the FFmpeg CLI. The other workspace crates must communicate
with it through owned child processes rather than depend on this package.

Build with `cargo build -p clip-media-worker`. Development requires FFmpeg
development libraries, pkg-config and the Clang tooling needed by the bindings.
The initial dependency line is `ffmpeg-next` 9.x. Runtime deployment must include
compatible FFmpeg shared libraries or declare the appropriate system packages.

The current commands are:

```text
clip-media-worker probe FORMAT_WHITELIST /proc/self/fd/N
clip-media-worker decode-audio FORMAT_WHITELIST /proc/self/fd/N
clip-media-worker render-audio SETTINGS_JSON [--output=/proc/self/fd/M] /proc/self/fd/N ...
clip-media-worker video-frame SETTINGS_JSON /proc/self/fd/N
clip-media-worker video-preview SETTINGS_JSON /proc/self/fd/N
clip-media-worker video-timeline SETTINGS_JSON /proc/self/fd/N
```

The parent must inherit a pinned ordinary input descriptor and retain child
ownership through exit. The worker duplicates the descriptor, probes through
seekable Rust stream I/O, denies FFmpeg URL protocols for secondary opens and
emits at most 64 KiB of JSON metadata. Audio decoding emits interleaved
little-endian float PCM at the source rate and channel count, capped at 256 MiB.
Its decoder and converter drain before successful completion. Parent-side cancellation and deadlines
remain mandatory; the library interrupt callback is additional protection.

Audio rendering uses the data-only `clip-media-protocol::AudioRender` settings.
It reads up to eight inherited inputs and writes one owned regular staging
handle, using stdout when the explicit output argument is absent. It never
resolves an output pathname. The sole pipe-output exception is raw `s16le` or
`f32le` PCM with an explicit range of at most 60 seconds, sample rate at most
48 kHz and at most two channels; an independent 32 MiB writer cap rejects
unexpected expansion. Search retains its smaller 2 MiB capture budget and
60-second deadline. Settings are capped at 64 KiB. Filter descriptions
are parsed without initialization and restricted to the application's audio
processing filters before any graph is instantiated; movie, command and other
external-source filters are rejected. Demand-driven input feeding avoids
reading an entire concatenation's later inputs into graph queues in advance.

Render jobs have no internal preview deadline: accepted studio writes retain
parent-side cancellation, kill/reap ownership and completion delivery. Bounded
pipeline jobs retain their existing `CommandRequest` supervision. Containers
and encoders are finalized only after decoder, filter and encoder draining.
The protocol crate has no native media or desktop dependencies; sharing request
data does not link FFmpeg into application libraries.

Still-frame requests use `clip-media-protocol::VideoFrame`. Stream selection
can use a global index, video ordinal (including artwork), or first non-cover
video. Output is a little-endian u32 JSON-header length, `VideoFrameInfo` JSON,
then the exact PNG byte count declared in that header. Empty success means no
frame after the requested instant. The parent can explicitly retry at the
preceding seek point and receives that frame's actual source-relative time.

Source pixel budgets are checked before codec opening and after decoding.
Output bounds are at most 4096 × 4096, PNG data at most 32 MiB, and header data
at most 1 KiB. Display matrices are applied before scaling; callers retain their
smaller dimension, capture and process-time budgets. Parsing the response does
not decode untrusted PNG pixels; host image readers retain their own limits.

`VideoPreview` streams a bounded MJPEG sequence using one source decoder,
one fixed-rate filter graph and one JPEG encoder. It accepts at most 512 frames,
1–60 fps and an optional source interval of at most 60 seconds. The output cap
is 32 MiB. EOF closure uses the observed source end, preserving a final held
image without extending a short movie to the requested interval. Snapshot and
batch jobs share source admission, packet draining, clocks and orientation.

`VideoTimeline` preserves the long-AV1 sequential fallback: decode only
keyframes, reduce each once to a column, hold columns across their source time,
and tile exactly the requested 1–16384 columns. The output is one JPEG, with
a 4 MiB parent capture budget and 240-second parent/library deadline. It shares
the existing decoder/filter/image encoder; it does not replace the primary
indexed C/GPU helper. Source frame rate is passed into the buffer filter so
final-column padding follows the same clock as the former CLI path.

`cargo test -p clip-media-worker` includes real executable/descriptor tests.
Those integration tests also make the helper executable available before a
full workspace test run. For a fresh targeted host/core test run, build this
package first so the sibling worker is available.

This package is GPL-2.0-or-later. The complete GPL v2 text is retained at
[`scripts/timelens-GPL-2.0.txt`](../../scripts/timelens-GPL-2.0.txt). Preserve this
package's corresponding source, build instructions and notices when distributing
its executable, along with the obligations of the selected FFmpeg build. The
workspace's default MIT/Apache license is not this helper's license.

Migration status and the eventual playback-framework recommendation live in
[`media_backend_migration.md`](../../docs/audio/media_backend_migration.md).
