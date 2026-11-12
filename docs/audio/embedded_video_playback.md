# Embedded and hover video playback

Goal: replace the external FFplay window with an embedded synchronized player,
and make thumbnail/timeline hovers play a small muted video from the start or
hovered position. Timeline strip generation and media exports retain their
existing specialized FFmpeg paths.

## Implementation and verification checklist

- [x] Establish architecture baseline `embedded-gstreamer-playback`.
- [x] Add GStreamer playback to the isolated media worker, retaining inherited
  ordinary-file descriptors, synchronized sinks and bounded RGBA transport.
- [x] Share one file-manager session/transport between full and hover playback.
- [x] Replace the external video window with an embedded picture and controls.
- [x] Start muted playback from thumbnail start or the hovered timeline position.
- [x] Verify playback progression, pause, seeks, EOF, source/stream/orientation,
  cancellation and stale-seek rejection using actual decoded frames.
- [x] Verify UI controls, hover behavior and cleanup on navigation/studio entry.
- [x] Update distribution dependencies and the worker's source bundle.
- [x] Complete workspace tests/build, architecture review and runtime inspection.

The parent owns its latest frame and latest desired control only. The worker
uses a clock-synchronized bounded appsink; hover disables audio decoding/device
acquisition. GStreamer remains linked only into the helper. Dropping a parent
session requests cancellation; its bounded worker retains admission until the
child has been killed and reaped. Source and search guards remain checked off
the UI thread. New seeks carry epochs so earlier buffered frames cannot replace
the currently requested position.

Backend validation: all 24 protocol/worker/executable tests pass, with manual
reference tests explicitly enabled. A generated three-second color/audio fixture
verifies real-time progression, initial-position pixels, pause stability, seeking
in both directions with epochs, EOF and control-pipe shutdown. A cover-bearing
MP4 produces red video rather than its blue artwork. Low-frame-rate media uses
a frame-rate range, avoiding an impossible upsample when drop-only is enabled.

- [x] Normalize frame times against the initial GStreamer stream origin and
  retain the probed source duration. A real MPEG program stream exposed a
  540 ms timestamp origin and an incorrect GStreamer duration estimate. Its
  requested 500 ms frame now reports its actual 480 ms source time and the
  correct two-second duration. All 24 worker/protocol tests pass afterward.
- The expanded host format matrix exposed an MXF preroll failure, fixed by
  the descriptor-backed random-access source described below.

- [x] Restore MXF random-access preroll using a descriptor-backed `filesrc`
  view. The retained `/proc/self/fd` inode remains pinned; the push-only fd
  source could not load MXF indexes. The full eleven-format playback, poster
  and timeline matrix now passes, including actual MXF frames at 500 ms.

- [x] Correct embedded control sizing and reserve the footer height. Runtime
  screenshots exposed a seek widget retaining its default 100-point slider
  despite a wider allocation, plus a clipped information line. Explicit scoped
  slider widths now fill the control row and the complete footer remains visible
  at the private runner’s 1280×800 output.

Private runtime verification passes in dark appearance using
`file_manager_video_playback_acceptance.py`: stationary thumbnail and timeline
hover frames advance, thumbnail requests start at zero, timeline requests start
later, motion reuses the same decoder, hover audio stays silent, and leaving
hover reaps the child. Embedded playback produces changing frames and actual
private-sink audio; Space pauses both, seeking changes paused pixels, resuming
works, Back to folder reaps the player, and application close reaps a reopened
player. Niri reports one Files window and no separate video window. The runner
uses private compositor/audio sockets and synthetic files only.

Architecture follow-up separates command dispatch, source admission, video-sink
construction, initial preroll and bus handling. The embedded UI likewise separates
keyboard shortcuts and transport controls. These phases share the same player. The existing worker/reference suite passes after
the backend extraction; the host check remains warning-free.

- [x] Preserve preview-sized GPU transfer for supported HEVC/large H.264.
  The GStreamer CUDA graph reduces/converts before download; a real HEVC
  fixture negotiated NV12 1280×720 to RGBA 320×180 on CUDA and delivered the
  expected source frame. Unknown/non-4:2:0 source formats select software
  before CUDA elements are built. Hiding CUDA devices exposed a factory-load
  failure before preroll; initialization now retries software as well as
  preroll failures, and the same fixture still delivers a valid frame.

- [x] Correct rotation reference fixtures for FFmpeg 9. The old `rotate` tag
  command produced no display matrix. `-display_rotation` now generates real
  0/90/180/270-degree fixtures, with explicit geometry checks and independent
  RGBA comparisons for both library snapshots and GStreamer playback. The
  snapshot comparison bounds mean channel error below two levels, accounting
  for chroma conversion ordering; the actual direction is also checked. All
  24 worker/protocol tests pass, including the expanded reference test.

- [x] Keep new pixel assertions clean under the current strict Clippy checks
  by using fixed-array chunks. Worker/protocol all-target Clippy passes with
  warnings denied.

- [x] Verify fullscreen on the light private runtime: F grows the embedded
  window to the 1280×800 output; Escape restores its original size without
  restarting or closing playback. The full light-theme hover/audio/cleanup
  acceptance scenario passes as well as the earlier dark run.

- [x] Review all-target workspace lints against the pre-feature revision. The
  only new warning was a redundant optional timeline branch; using `?` keeps
  the same hit-testing behavior and removes it. The complete command-local
  warning inventory now reports no diagnostics on changed repository primary
  spans or their macro expansion sites. Existing workspace lint debt remains.

## Final verification and delivery

| Check | Result |
| --- | --- |
| Full workspace tests, one test thread | 2,829 passed; zero failed; 35 ignored across 43 suites, including doctests. |
| Worker/protocol tests with manual references enabled | All 24 passed; genuine quarter-turn rotation geometry/RGBA, compressed-audio timing and AV1 timeline references included. |
| Eleven-format host matrix | Playback frames, posters and timeline strips pass, including MXF and MPEG timestamp origins. |
| Debug and optimized workspace builds | Passed with zero compiler warnings. The optimized build also packaged both helpers and safely pruned 393 obsolete release-cache entries. |
| Private Wayland/audio acceptance | Dark and light pass. The optimized executable also passes the complete dark hover/playback/fullscreen/cleanup scenario. |
| Delivered-source offline rebuild | All 24 tests pass from the vendored minimal worker workspace with `--offline --locked`, including manual references. |
| Packaging tests / formatting | Three packaging tests, Python compilation and Rust formatting pass. |
| Lints | Worker/protocol all-target strict Clippy passes. The full workspace inventory reports no diagnostics on changed lines or macro expansion sites; existing workspace lint debt prevents a blanket strict-Clippy claim. |
| Linkage | Desktop binaries link neither FFmpeg nor GStreamer. The worker links the expected FFmpeg 9 and GStreamer App/Video libraries. |

The default launcher’s optimized binaries and `media-helper` companion are
rebuilt. Deployment requires matching FFmpeg shared libraries and GStreamer
1.24+ with playback/codec plugins; the companion includes current source,
vendored Rust dependencies, fixtures, notices and both migration documents.

Architecture review removed the new oversized UI/command/pump functions and
shared GPU/CPU output-cap construction. The CLI dispatcher falls from CCN 17 to
3. One source-admission function remains at CCN 20 because it keeps ordinary-file,
container/stream, dimensions, finite timing and decoder eligibility checks
together. Maximum repository CCN remains 195, duplication percentage decreases
slightly, and one additional clone group remains (including standard pipe setup
and fixture patterns); modified-file reports also include unchanged row code.
This is a reviewed feature cost, with one shared player and no application-level
GStreamer linkage.

## Player refinements

- [x] Fill the fullscreen viewport and move title/navigation and transport into
  translucent overlays that hide after 2.5 seconds without interaction.
- [x] Use the shared fixed-size play/pause icon and remove stop/volume controls.
- [ ] Add temporary subtitle/audio track, speed and loop choices.
- [ ] Add a manual-config-only subtitle default, initially disabled.
- [ ] Open a stereo audio copy from the video-file context menu.
- [ ] Verify controls, actual subtitles/tracks, audio extraction and fullscreen.
