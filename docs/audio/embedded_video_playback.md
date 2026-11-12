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
  translucent overlays that hide after 500 milliseconds without interaction.
- [x] Use the shared fixed-size play/pause icon and remove stop/volume controls.
- [x] Add temporary subtitle/audio track, speed and loop choices.
- [x] Add a manual-config-only subtitle default, initially disabled.
- [x] Open a stereo audio copy from the video-file context menu.
- [x] Verify controls, actual subtitles/tracks, audio extraction and fullscreen.

The player context menu offers embedded subtitle and audio tracks, rates from
0.1× through 10×, and looping of the current file. Choices reset on close.
`[file_manager].video_subtitles_enabled = false` initializes each new session
(including muted hover playback); menu changes never write this setting.
A generated two-audio/two-subtitle MKV verifies actual subtitle pixels, switching
and disabling text, selected audio ordinal, 4× timing and looping.

Video-file menus expose **Edit Audio Track**. The guarded background loader
extracts the default audio stream into a seekable float stereo WAV in an owned
temporary workspace, opens it in the existing Audio Studio and suggests the
original folder for exports. Source identity comes from held input/output
descriptors; Audio Studio and deferred readers retain workspace ownership.
Tests verify stereo metadata and non-silent decoded samples, backing-file
ownership, unchanged original bytes, cancellation and a missing-audio failure.

Private optimized-runtime verification passes in dark appearance: fullscreen
video occupies the former menu/header space, overlays/cursor disappear after
inactivity and return on pointer movement, Escape preserves the player, and
right-click exposes playback choices. The file menu opens the extracted stereo
audio in Audio Studio; returning to the folder and closing a reopened video reap
the owned player. An initial OCR multiword locator was corrected after its
screenshot showed the requested action already present.

The follow-up architecture review separates picture painting, overlay activity,
fullscreen controls and Escape handling, and moves conditional menu chrome out
of application lifecycle updates. File-menu handoff reuses document positions
instead of a second boolean route. Average CCN remains 4.04, maximum 195,
clone groups remain 1,521, and duplication falls from 4.26% to 4.25%. The tool
still labels existing clone patterns in modified files as regressions; the
introduced overlay/test duplication and oversized video UI method were revised.
After the final popup-priority correction, viewer UI orchestration is CCN 17
in 91 lines (baseline CCN 13); this is the reviewed cost of the new viewer states.
The backend selection phases remain below the hotspot threshold.

- [x] Fix the live audio-track switch stall exposed by captured output. The
  legacy selector could deadlock when immediately followed by a flushing seek.
  Playback now uses playbin3 stream collections and acknowledged stream IDs;
  temporary muted preroll lets paused track changes complete before restoring
  the requested frame. Software fallback excludes hardware decoder factories
  in this disposable process, because decodebin3 ignores the legacy software flag.
  Worker/reference tests and the eleven-format host matrix pass afterward.

- [x] Give Escape priority to an open video context menu before fullscreen or
  viewer dismissal. Egui 0.27 context menus have their own state, so retain the
  response's open state before the menu processes Escape. Overlay areas stay
  below menus, and an open menu keeps the overlay visible. The regression test
  verifies menu, fullscreen and viewer dismissal in that order.

Final refinement verification: 2,832 workspace tests pass with 35 ignored after
the stream-collection correction; the additional Escape regression test also
passes. All 25 worker/protocol tests pass with manual references enabled, as do
the eleven-format host matrix, strict worker/protocol Clippy, six packaging/build
script tests, formatting and Python compilation. The current optimized workspace
build completes without compiler warnings and updates the bundled helper.

The final optimized light-theme acceptance scenario passes with two audio and
two subtitle tracks: selecting French changes actual private-sink output from
440 Hz to 880 Hz, captions paint measurable glyph pixels, and menu Escape keeps
the player alive. Pixel checks verify overlay appearance/disappearance and the
full viewport; the stereo Audio Studio handoff, folder return and process cleanup
also pass. Caption fixtures use ordinary two-second cues; direct glyph-pixel
checks avoid OCR errors on upscaled low-resolution subtitles.

## Subsequent runtime fixes

- [x] Publish Audio Studio single/multi-segment copies to the system clipboard
  even when the daemon accepts its hand push. Daemon success does not acknowledge
  clipboard publication; a clipboard failure must surface instead of leaving
  stale text behind while reporting success. Artifacts remain retained before
  any handoff. Fourteen focused copy tests pass, including successful-daemon
  clipboard publication, failure reporting, ordered batches and actual PCM.
  The private desktop regression reproduces stale text with the previous build
  using a daemon that acknowledges without publishing the selection.

- [x] Keep rate-only changes out of track selection/internal muted playback.
  Send the flushing seek before waiting for preroll so the wait observes the
  new segment rather than old-rate buffered/clock state. The regression exercises
  32 fast-rate transitions with pause/resume and actual advancing decoded frames.
  Native private-audio stress runs pass 80 transitions on 4K HEVC/GPU, 160 with
  subtitles, and 100 with paused rate changes/resume. The user's exact intermittent
  stall was not reproduced with these synthetic sources; an affected source is
  needed to verify that specific case if it persists.

Final follow-up verification: 2,835 workspace tests pass, zero failures and
35 ignored; all 26 worker/protocol/reference tests and strict worker/protocol
Clippy pass. The optimized workspace rebuild has zero compiler warnings. The
private desktop clipboard regression now passes with an acknowledging daemon
that deliberately leaves the OS selection alone: both the button and Ctrl+C
replace seeded text with `text/uri-list`, retained stereo WAV data, and duration
matching the displayed selection. Waiting for completed waveform analysis avoids
racing range setup against initial analysis; expected duration comes from the
editor readout rather than assumed pointer-drag coordinates.

## Remaining media clipboard audit

- [x] MIDI Studio and file-manager MIDI strips publish file URIs after successful daemon handoff, preserving owned files on partial failure. All 28 MIDI tests pass, including successful handoff with a failed desktop writer.
- [ ] Rolling-audio selection copies use the shared guarded audio clipboard delivery.
- [ ] Audio waveform/FFT PNG copies publish images directly and report clipboard errors.
- [x] Image Studio normal and optimized copies report desktop failure as failure even when the hand push succeeds. Shared image delivery now enforces this policy centrally; all 29 image-save tests pass.
