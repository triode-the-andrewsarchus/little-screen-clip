# Media library worker

This executable hosts `ffmpeg-next` processing and GStreamer playback independently from the desktop binaries.
It does not spawn the FFmpeg CLI. The other workspace crates must communicate
with it through owned child processes rather than depend on this package.

Build with `cargo build -p clip-media-worker`. Development requires FFmpeg
development libraries, GStreamer 1.24+ App/Video development libraries, pkg-config
and the Clang tooling needed by the bindings.
The initial dependency line is `ffmpeg-next` 9.x. Runtime deployment must include
compatible FFmpeg/GStreamer shared libraries and GStreamer codec/playback plugins,
or declare the appropriate system packages.

The default workspace build packages `media-helper` beside the release binaries.
A standalone package can be made with `python3 scripts/build_media_helper.py`;
`--profile debug` selects a development build, and `--skip-build` packages an
already successful build. The bundle carries the executable, this notice,
`COPYING`, both Rust crates, shared fixtures, the lockfile and vendored Rust
dependencies with their license files. It contains no application library code.

Rebuild a delivered bundle without Cargo network access:

```sh
cd media-helper/source
cargo build --release --locked --offline -p clip-media-worker
```

FFmpeg and GStreamer App/Video development headers/libraries, pkg-config and Clang are still required
for that rebuild; compatible shared libraries are required to run the result.
The bundle does not contain native FFmpeg/GStreamer libraries or codec plugins. Distributors supplying
those libraries must also preserve their selected build's source/license
requirements. Set `LITTLE_CLIP_MEDIA_WORKER` to an independently rebuilt helper
to retain the override across normal application builds.

The current commands are:

```text
clip-media-worker probe FORMAT_WHITELIST /proc/self/fd/N
clip-media-worker decode-audio FORMAT_WHITELIST /proc/self/fd/N
clip-media-worker render-audio SETTINGS_JSON [--output=/proc/self/fd/M] /proc/self/fd/N ...
clip-media-worker video-frame SETTINGS_JSON /proc/self/fd/N
clip-media-worker video-preview SETTINGS_JSON /proc/self/fd/N
clip-media-worker video-timeline SETTINGS_JSON /proc/self/fd/N
clip-media-worker playback SETTINGS_JSON /proc/self/fd/N
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

## Streaming playback

`playback` uses GStreamer `playbin3` with an application video sink, never a native
window sink. The worker owns the playback clock/audio output; hover disables
audio decoding and device acquisition. Source preflight shares the existing
format allowlist and pixel budget. Small H.264 uses software decoding; other
inputs allow GStreamer's ranked hardware decoders and retry software if initial
preroll fails before any frame is delivered. Decoder max-thread properties are
bounded where available. Supported HEVC/large H.264 uses CUDA conversion/scaling
before downloading preview-sized RGBA when the plugins/device are available;
unknown/non-4:2:0 source formats select software before CUDA element creation.

Stdin carries newline-delimited `playback::Control` JSON, at most 1 KiB per
command. Stdout carries length-prefixed bounded `playback::Update` JSON and
exactly its declared tightly packed RGBA bytes. Frames are bounded to full-HD
pixel count, sinks retain one newest frame, and seek epochs reject stale frames
in the parent. Pause, EOF, duration and failures also produce state updates.
The parent must pump control/output off its UI thread, retain source guards and
kill/reap on cancellation; closing stdin also retires the worker.

Session options select embedded audio/subtitle tracks, enable or suppress text,
set a positive playback rate and loop only the current file. The track catalogue
is capped at 64 tracks of each type with bounded labels; state headers are capped
at 64 KiB independently of the raw frame budget. Subtitle rendering requires the
GStreamer text/subtitle overlay plugins and fonts (including `pango` for text
tracks). The optional `scaletempo` audio filter preserves pitch at changed rates
when the audiofx plugin is installed. Options never write application settings.
