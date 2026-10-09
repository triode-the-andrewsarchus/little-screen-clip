# Indexed video-strip helper

`main.c` is an independent implementation using FFmpeg's public demuxing and
codec APIs. It does not contain Timelens source. The helper is licensed under
**GPL version 2 or later**, and runs as a separate executable to preserve the
application's licensing boundary. In particular, installed FFmpeg libraries may
be built with GPL components. This process boundary was chosen for license
compliance; modifications and distribution of this helper retain GPL obligations.
The existing Timelens build, patch, source archive, and GPL notices remain separate.

Build with `scripts/build_video_strip.sh`. Dependencies are a C compiler,
`pkg-config`, pthreads, and development libraries for `libavformat`, `libavcodec`,
`libavutil`, and `libswscale`. The script puts the executable, this README, complete
source, build script, and GPL text together in `target/tools/video-strip/`.
To rebuild directly from this directory, run:

```sh
cc -O3 -std=c11 -Wall -Wextra -pthread main.c -o video-strip $(pkg-config --cflags --libs libavformat libavcodec libavutil libswscale)
```

Distribute corresponding source and build instructions with the binary. Its
runtime FFmpeg library versions must match the installed shared libraries.

The helper loads container keyframe indices, maps each requested output pixel to
the nearest indexed keyframe, and decodes each distinct selected keyframe once.
A bounded worker count owns independent decoder/demuxer contexts and shares one
CUDA device when acceleration is selected; output column ranges
are disjoint. Decoder draining preserves reordered and open-GOP keyframes.
Unsupported/missing indices, corrupt frames, and failures return a nonzero status
so Files can use its existing decoder fallback. Memory is bounded by requested
width (maximum 16,384 columns) and at most 16 decoder contexts (Files normally
requests 1–8). Output is a lossless
64-pixel-high PPM, consumed through the application's existing bounded image reader.

Usage: `video-strip INPUT OUTPUT WIDTH VIDEO_STREAM DURATION_MS [--gpu] [--workers 1..16] [--io-uring]`.
Video stream is the ordinal among all video streams, including cover streams;
Files probes and supplies the intended non-cover stream. `--gpu` uses CUDA decode.
The optional GPU area-reduction build averages complete keyframes on the device
and downloads only a column; other builds download complete keyframes before
CPU averaging. Files prefers CUDA for large
H.264 and large HEVC frames, including cropped/portrait 4K, with CPU retry;
other codecs use the measured CPU path. Unsupported
H.264 pixel formats are rejected before creating the CUDA device.

This indexed path approximates intermediate frames with nearest keyframes. Files
keeps ordinary-frame sampling for short videos so brief changes are retained.
Column count still follows physical display resolution and the user's decimation
factor, rather than a fixed low sample cap. Dense indices are sampled according
to the requested pixels; increasing resolution can select more distinct frames.

On Linux CIFS/SMB2 mounts, an aligned, read-only `O_DIRECT` input reduces excess
kernel read-ahead during sparse keyframe seeks. Both filesystem identifiers must
be recognized. Each worker owns its file descriptor and aligned scratch buffer;
local files retain buffered reads and the normal filesystem cache. Failure to
enable direct I/O uses buffered input. This changes no mount or desktop settings.
The reader uses FFmpeg's public AVIO callbacks, supports byte seeks and unaligned
EOF, and bounds each direct read to 64 KiB.

## Rust io_uring experiment

Linux builds also compile `rust_io/` using the low-level `io-uring` crate
(current lockfile: 0.7.15). It requires no async runtime. Compared alternatives
were [`tokio-uring`](https://github.com/tokio-rs/tokio-uring), which owns a Tokio
runtime, and [`Monoio`](https://github.com/monoio-rs/monoio), a thread-per-core
runtime. [`io-uring`](https://github.com/tokio-rs/io-uring) fits the existing
thread-owned synchronous FFmpeg AVIO interface and allows explicit bounded
batching. It is linked only into this separate GPL helper. Dependencies' MIT
notices remain in the vendored sources; use their MIT license option.

`--io-uring` enables indexed prefetch of upcoming keyframe blocks plus sequential
read-ahead. Each decoder owns 16 aligned 64-KiB buffers (1 MiB), its ring, and
its pending completion state. Buffers cannot be evicted while a kernel read is
pending; the current demand buffer is protected from speculative eviction.
Destruction drains pending reads before closing the borrowed descriptor and
freeing buffers. Ring setup/read failure uses the synchronous reader. Local
files use their normal page cache; direct I/O remains limited to CIFS/SMB2.

The experiment is **off by default**: paired trials showed no consistent gain
and sometimes increased NAS traffic/latency. Use `LITTLE_CLIP_VIDEO_IO=uring`
when launching Files to try it on uncached strips, or the helper CLI flag for controlled benchmarks.
This is a read scheduling experiment, not a reduced-detail rendering mode.

Builds retain Rust source, `Cargo.toml`, `Cargo.lock`, vendored dependencies and
`.cargo/config.toml` in `target/tools/video-strip/rust_io/`. To rebuild the full
Linux helper from that directory without network access:

```sh
cd rust_io
cargo build --release --locked --offline
cd ..
cc -O3 -std=c11 -Wall -Wextra -pthread -DVIDEO_URING main.c \
  rust_io/target/release/libvideo_strip_io.a -ldl -lm -o video-strip \
  $(pkg-config --cflags --libs libavformat libavcodec libavutil libswscale)
```

The original direct C rebuild above remains a synchronous-only fallback.
`scripts/video_workload_benchmark.py` compares reader modes, worker counts,
local/NAS inputs and output hashes. `--cold-local` evicts only the chosen local
file's clean cached pages; NAS cache state is not controlled. Synthetic Rust
tests exercise buffer eviction, arbitrary offsets, unaligned EOF, direct reads,
failed requests and shutdown with pending read-ahead.

## GPU area reduction

When `nvcc` and the `ffnvcodec` development headers are available at build time,
the build compiles `average.cu` to PTX and embeds it in the independent helper.
It dynamically loads the CUDA driver; no CUDA runtime or compiler is needed
when running the built executable. `gpu_average.h`, kernel source, generated
PTX/header and build instructions remain beside the installed binary.

The kernel averages **every** pixel in the decoded NV12/P010 frame, including
fractional vertical overlap at the 64 output-row boundaries. It averages luma
and both chroma planes, then transfers just 384 bytes and uses libswscale for
the tiny column's RGB conversion. Each decoder owns its module and bounded
column buffer on FFmpeg's existing CUDA device/context and stream. Submitted
work is synchronized before frame/buffer reuse and freed before decoder teardown.
Unsupported formats, unavailable builds/drivers and reduction errors use the
existing complete-frame download path. CPU decoding and timeline sample counts
are unaffected. The different rounding/chroma reduction can change a few RGB
levels; this is not a byte-identical replacement for libswscale area filtering.

GPU averaging is enabled by default in capable builds.
`VIDEO_STRIP_GPU_AVERAGE=0` selects full-frame download for paired comparisons;
`=1` explicitly enables reduction. `VIDEO_STRIP_PROFILE=1` prints per-worker
host-call stage durations. These sums overlap between workers, and CUDA work
may synchronize in a subsequent host call: they are not isolated kernel timings.

Rebuild the GPU-capable binary from the retained directory with:

```sh
cc -O3 -std=c11 -Wall -Wextra -pthread -DVIDEO_GPU_AVERAGE -I. main.c \
  -ldl -o video-strip $(pkg-config --cflags --libs libavformat libavcodec libavutil libswscale)
```

The generated `average_ptx.h` is retained; regenerating it requires `nvcc` and
the steps in `build_video_strip.sh`. Add the Rust static library and
`-DVIDEO_URING` flags from the previous rebuild command to enable both features.
`scripts/video_gpu_average_check.py --synthetic` checks spatially nonuniform
8-bit H.264 and 10-bit HEVC with one/four workers. Without that flag it runs
alternating paired real-file comparisons and reports output-error statistics.

## Sparse GPU sampling experiments

The complete-frame `area_column` kernel remains the default. Set
`VIDEO_STRIP_GPU_SLICES=5` to read five single-pixel vertical columns at 0%,
25%, 50%, 75% and 100% of each plane's width, averaging those five values and
using vertical area weights. Set `VIDEO_STRIP_GPU_SLICES=1` to read only the
center column, with nearest pixels at output-row vertical midpoints and **no
averaging**. Both are GPU kernels on the decoded CUDA frame. They keep the
existing tiny download/RGB conversion and requested timeline column count.
The decoder still reconstructs the complete compressed frame. Unsupported
formats or reducer failures retain the existing complete-frame CPU fallback.

To try or restore a mode, restart Files with:

```sh
VIDEO_STRIP_GPU_SLICES=5 ./launch.sh files
VIDEO_STRIP_GPU_SLICES=1 ./launch.sh files
VIDEO_STRIP_GPU_SLICES=0 ./launch.sh files
```

`0`, an unset variable, or an unrecognized value restores complete-frame GPU
averaging. `VIDEO_STRIP_GPU_AVERAGE=0` independently disables all GPU reducers.
Files keeps five-slice and center-column previews in separate cache directories;
restoring full-frame mode reuses the original cache, without sparse previews
replacing its contents. Application decimation remains a separate setting.

Compare all three GPU modes at an explicit resolution with:

```sh
python3 scripts/video_gpu_average_check.py --sparse-modes --width 3200
python3 scripts/video_gpu_average_check.py --sparse-modes --synthetic
```

`--five-slices` compares only the first two GPU modes. `--previous-helper PATH`
adds a byte-identical full-frame rollback check against a saved helper during
synthetic checks. Sparse synthetic checks use an independent CPU reference
from decoded YUV planes to verify sample positions, vertical area/nearest
sampling, bit depth and color conversion. Real benchmarks rotate mode order,
report medians and color differences; sparse modes are not expected to match
full-frame colors.

## Stage profiling

`VIDEO_STRIP_PROFILE=1` additionally prints `Pipeline:` counters per worker:
actual AVIO read time/bytes/calls, opening/probing, seek time and reads nested
inside it, packet extraction and its nested reads, decoder reset/frame release,
packet submission, drain and receive. Subtract `seek_read_us` from `seek_us`
and `packet_read_us` from `packet_us` before combining those stages with read
costs. All reads include worker opening/probing; the nested read sums isolate
active frame reads. Worker sums overlap and are not elapsed time.

Decoder timings include CPU parsing, decoder waits and internal GPU copies.
`VIDEO_STRIP_PROFILE_SYNC=1` optionally waits on the decoded frame's CUDA stream
before reduction to check pending work. This diagnostic is off by default and
can change synchronization between workers sharing that stream. The historical
`averaged` counter counts GPU-column completions in sparse modes too; the
`GPU reduction:` line identifies the actual selected kernel.


## Progressive timeline snapshots

Add `--progressive PATH` to receive a full-width PPM preview before the final
output is ready. Each persistent decoder starts with eight evenly spaced
selected samples, then fills its remaining samples while skipping those
anchors. Every selected keyframe is still decoded exactly once. The final
output uses the same columns and pixel reduction as a non-progressive render.
Tiny sample sets that would finish during the anchor pass emit only the final.

Snapshots repeat the nearest completed column at pending positions, then refine
about four times per second. A mutex protects column completion and copying;
encoding/writing occurs outside the mutex. Publication uses a temporary file
and atomic rename, so readers never see a partially written image. Files polls
on its existing cancellable command worker, passes bounded images to the UI,
and updates the existing texture. Partial images never enter the disk cache.
Preview write errors leave ordinary final generation available.

Measure both time to first preview and final render overhead with:

```sh
python3 scripts/video_progressive_check.py /path/to/local/video.mkv --rounds 5
python3 scripts/video_timeline_resolution_check.py --gpu --progressive
```

The benchmark rotates progressive/ordinary invocation order, checks identical
final bytes, simulates the application's 50 ms progress poll/pixel conversion,
and rejects median elapsed overhead above 10%. Optional `--previous-indexed`
and `--previous-timelens` compare against saved pre-change binaries. Run timings
without concurrent builds, fixture encodes or other performance workloads.
