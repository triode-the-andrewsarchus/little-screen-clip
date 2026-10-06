# Phase 5: Gallery, music, and video

Status: complete against the Phase 5 scope and completion gate in [plan.md](plan.md). Phase 4 remains in progress independently. The backend and resource limits below are part of this delivery.

## Photo gallery

The manual **Photos** view presents a virtualized grid of square, center-cropped images with adjustable thumbnail size and tight, uniform gutters. Selected images are inset and carry a contrasting checkmark. **Select** mode toggles individual tiles without modifier keys; Ctrl+click toggles files and Shift+click selects a range. Photo filenames appear on hover rather than below each tile. Selection, multi-selection, context menus, native file dragging, and ordinary file operations use the same behavior as the list. Subfolders and non-image files remain visible and accessible. Hidden-file settings apply to both views.

Only visible photo tiles request decoding. The gallery retains at most 128 visible preview identities, runs one bounded decoding job at a time, and retires obsolete work and textures on scrolling or view changes. Preview output is at most 320×320 pixels. Encoded input is capped at 64 MiB, raster allocation at 128 MiB, and dimensions at 16,384 pixels. Images that exceed these preview bounds retain an icon and can be opened through the existing studio or externally. Photo tiles center-crop the aspect-fitted preview without modifying source pixels; ordinary list previews retain aspect fitting.

Opening a photo from any folder view captures the current visible image order. Unmodified Left/Right keys navigate that collection, except while typing or adjusting the slideshow interval. The shared Image Studio provides previous/next photo controls and a slideshow with a configurable interval of 1–30 seconds. The slideshow wraps at the end of the collection and waits for each image to finish opening before starting its timer. Closing the studio ends the slideshow. Pending saves and unsaved edits block photo changes and pause the slideshow without discarding edits. Ordinary Image Studio saves retain Phase 4's source guards.

## Music

Music view now includes a configurable automatic activation threshold (75% by default), manual List/Photos/Music overrides, metadata columns, cover art or a stand-in, and bounded per-file waveform previews. Audio rows are larger than ordinary folder/file rows; the listing uses cached row offsets and renders only the viewport. Entering Music folds the Shelf without clearing collected items.

The Files-owned streaming player provides an explicit folder queue, play/pause, seek, previous/next, repeat and shuffle. Waveform clicking seeks; dragging selects a range. Range copying uses the shared guarded audio exporter and desktop clipboard/Left Hand handoff. Pasting imports clipboard content into the current folder. The private workflow has exported and pasted a selected range with duration matching the drag while preserving the original source.

## Playback ownership

Files owns its music queue and playback in its own process, using the existing shared rodio device. The queue is a snapshot of explicitly chosen files, not a live alias of the current folder. Navigating folders, toggling views, or opening a photo/text studio retains the queue. Opening an audio/MIDI studio or video session pauses queued music. Closing Files stops playback and cancels pending loads; accepted copy jobs and ordinary file operations retain their close guards. Closing a studio does not automatically resume music.

Automatic Music activation counts audio files among visible regular-file entries and excludes subfolders. Manual view choices override activation for the current location and survive refresh/history. Refresh does not replace the playback queue.

## Video backend

Video paths open an owned FFplay window from a Files viewer session. This reuses the installed FFmpeg stack for synchronized audio/video decoding and ordinary playback controls instead of introducing a second decoder and audio clock. In the video window, Space pauses/resumes, Left/Right seek ten seconds, Up/Down seek one minute, right-click seeks proportionally, F toggles fullscreen, and Q closes playback. The Files session offers replay, stop and return to folder.

One session owns at most one child. Stop, returning to the folder, replacing playback and closing Files kill and reap it. On Linux the child also receives SIGKILL if Files dies abruptly. Music remains paused until explicitly resumed after leaving the video session. Video is read-only; image/audio edits and exports use the existing protected studios and source guards.

Preflight uses core's bounded process runner for FFprobe off the UI thread, selects the first non-cover video stream, and checks FFplay availability with a three-second timeout and 4 KiB retained output. Source identity is checked after preparation and before every launch. Missing tools, invalid streams and failed player exits produce visible errors with the external-opening option.

Listing previews reuse the bounded music-cover extractor for one aspect-fitted video poster, with the same stream selection and pixel check as playback. They do not require FFplay.

The local stream pixel budget is 4096×2160 pixels, with either dimension at most 4096. FFplay uses two decoder threads, one filter thread, finite input buffering, frame dropping on slow machines and a 256 MiB **single-allocation** cap; this cap is not a total-process memory limit. Frames and audio stream through the player; Files retains no video-sized frame array. Larger streams use Open externally. Playback codecs depend on the installed FFmpeg build. The local inventory includes H.264, HEVC, VP9, AV1, MPEG-4, AAC, FLAC, Opus and Vorbis; rendered acceptance currently proves H.264/AAC, not every listed codec.

## Music resource and format expectations

Metadata and cover extraction use bounded FFprobe/FFmpeg requests (three seconds each, 64 KiB metadata and 256 KiB encoded cover output). Cover rasters are at most 160×160 with a 4 MiB allocation cap. Waveforms share core's streaming `WaveformColor` crossover with the workbench: warm bass, green mids and blue highs, with the shared accent for selected ranges. Each channel retains independent filter history and previews average channel colors without phase-cancelling downmixes. Listing and Shelf excerpts use the same calculation. Full-length waveforms keep 512 peaks and colors per visible track and decode Symphonia planar packets rather than retaining PCM or dispatching individual interleaved samples. Decoding has a ten-second budget and cancellation checks. At most eight music previews run concurrently (bounded by available CPUs) on a dedicated executor, leaving the shared job executor available for other work. Each music job overlaps its bounded metadata/cover helper with decoding; at most eight such helpers exist. Ordinary listing, gallery and Shelf previews retain their single-job admission. At most 128 visible identities are retained, and retired work is cancelled. Unavailable waveforms retain the file, metadata and a visible fallback with error details on hover.

Native music playback uses the existing rodio/Symphonia feature set. WAV, FLAC and the workspace's enabled compressed formats are supported according to that decoder; classifying a file as audio does not guarantee decoding. Decoder failures are displayed and ordinary next-track/external-opening controls remain available. Clipboard slice extraction uses FFmpeg and the existing guarded export model. The automatic threshold can be configured in `[file_manager]` as `music_threshold_percent`; the view's slider changes it for the current application session.

## Completion audit

| Phase 5 requirement | Implementation and evidence |
| --- | --- |
| Adjustable photo grid and selection | Virtualized `ui/gallery.rs`, shared selection/actions, 80–280 px size control. Private gallery runs verify both decoded thumbnails and mixed-folder access. |
| Next/previous and slideshow | `viewers/gallery.rs`; rendered navigation, timed advancement and pause. Unit tests cover ordering, wrapping and unsaved edits blocking advancement. |
| Music metadata, covers and waveforms | Visible cards contain title, artist, album, duration, cover/stand-in and 512 peaks. Rendered runs verify tagged FLAC; unit tests cover missing covers and cancellation. |
| Configurable automatic Music and manual views | Visible regular-file ratio, default 75%, TOML configuration and session slider; unit tests cover configuration and ratios, rendered runs verify manual override through refresh/history. |
| Queue, playback, seek, previous/next, repeat/shuffle | Explicit queue snapshot and owned rodio player. Rendered runs exercise every control and capture non-silent PCM from a private null sink. Queue tests cover ordering and end-of-queue behavior. |
| Direct waveform selection/copy/paste | Rendered playhead seek and range selection; Ctrl+C exports a guarded slice and Ctrl+V imports it. The pasted duration matches the selected interval and the original SHA-256 is unchanged. |
| Large folders and bounded previews | Two 10,000-entry fixtures exercise List, Photos and Music. Visible text shape counts stay below 200, the last entry is not laid out, and history restores selection/scroll. Preview caches retain at most 128 visible identities; gallery/list/Shelf admit one job and Music admits at most eight on a dedicated executor. |
| Navigation and close ownership | Rendered music navigation retains playback; Audio Studio pauses it, verified by silent captured PCM. Video return/app-close tests verify child reaping. Source checks reject changed video replay. |
| Assessed video backend and expectations | Owned FFplay child, bounded FFprobe inspection and bounded posters in shared `media.rs`. H.264/AAC moving frames and pause are verified in both appearances; codec dependencies and allocation/dimension limits are documented above. Missing-player errors retain external opening. |
| Subfolders/non-media remain accessible | Both views use the ordinary browser entries/actions. Gallery acceptance opens a text file and returns; Music retains folders and text entries. |
| Shared media edits and protected exports | Existing Image/Audio Studios remain file-backed. Photo edits block slideshow changes; waveform export validates source identity and writes a separate slice. Existing source replacement, save conflict and undo tests run in the workspace suite. Video remains read-only. |

## Verification

Before the subsequent waveform-color, square-grid and keyboard-navigation refinements, the Phase 5 release passed the following checks:

- 73 file-manager unit tests pass, including 10,000-entry virtualization, common compressed formats, source guards and playback lifecycle. The final local debug run loads those directory fixtures in 58–59 ms and renders the first frame of each view in 2 ms, with 53–81 text shapes; these are local measurements, not cross-machine timing guarantees.
- `cargo test --workspace --offline -- --test-threads=1` passes; the workbench library reports 716 passed and 8 ignored. Across the workspace, 1,893 tests pass and 22 are ignored. Shared background-executor timing tests failed under concurrent test/UI load, so the final recorded workspace run is serial with acceptance jobs finished.
- Private gallery: `/tmp/files-phase5-bounded-gallery-dark/report.json` and `/tmp/files-phase5-bounded-gallery-light/report.json` each pass 12 observations.
- Private Music: `/tmp/files-phase5-bounded-music-dark-final/report.json` and `/tmp/files-phase5-bounded-music-light/report.json` each pass 21 observations, including actual PCM, selected-slice duration, source integrity and studio playback conflicts.
- Private video: `/tmp/files-phase5-bounded-video-dark/report.json` and `/tmp/files-phase5-bounded-video-light/report.json` prove advancing frames, stable paused frames, replay, return cleanup and application-close cleanup.
- `/tmp/files-phase5-final-fallback-b/report.json` passes all three missing-player observations: visible dependency error, external-opening option, no owned player window, and return to browsing.
- The playback-panel regression test checks three frames and rejects opaque panels painting over the controls. Central content is clipped to the space left by outer docks; compact scroll areas fit their assigned height. Rendered Light/Dark screenshots confirm full button borders.
- Native decoder tests cover MP3, Ogg Vorbis and M4A/AAC decoding to fixed-size waveforms and seeking; rendered music proves WAV/FLAC playback. Other formats remain subject to the enabled decoder/backend support.
- Architecture: clone groups 1297→1296, duplication 7.98%→7.86%, maximum complexity 196 and measured cycle count 2 unchanged. Whitespace-normalized fragment comparison finds zero new clones and no new/increased functions above CCN 15. The service still labels pre-existing audio-export fragments in touched files as regressions. Shared media data/inspection now avoid the Shelf→video→listing-preview dependency cycle.
- Release build through `./launch.sh build --bin clip-file-manager`, formatting, Python script compilation and diff whitespace checks pass.

All rendered acceptance uses a private Weston/Niri compositor. Audio capture uses private PipeWire services and a null sink. No interactive-compositor input or host audio routing is used.

## Media feedback refinements

The square gallery, inset/checkmark selection, shared waveform colors and folder Left/Right navigation are subsequent changes. `cargo check -p clip-workbench --all-targets --offline`, `./launch.sh build --bin clip-file-manager`, formatting and diff whitespace checks pass. Architecture comparison against `pre-media-gallery-feedback` keeps the maximum complexity and cycle count unchanged, with no new or increased functions above CCN 15. The flagged waveform clones all match pre-existing fragments; clone count stays at 1296 and duplication decreases from 7.86% to 7.85%. At that checkpoint, no additional tests or UI acceptance runs were performed for those refinements. The subsequent waveform performance work reruns the workspace suite; measurements and current validation are recorded in [waveform-performance.md](waveform-performance.md).
