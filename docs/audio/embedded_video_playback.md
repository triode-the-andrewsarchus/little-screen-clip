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
- [ ] Verify playback progression, pause, seeks, EOF, source/stream/orientation,
  cancellation and stale-seek rejection using actual decoded frames.
- [x] Verify UI controls, hover behavior and cleanup on navigation/studio entry.
- [ ] Update distribution dependencies and the worker's source bundle.
- [ ] Complete workspace tests/build, architecture review and runtime inspection.

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
- The expanded host format matrix currently exposes an MXF preroll failure;
  this is still being fixed rather than dropping that format from coverage.

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
