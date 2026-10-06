# Phase 5: Gallery, music, and video

Status: in progress. Phase 4 remains in progress independently. Phase 5 is not complete until all scope in [plan.md](plan.md) is implemented and verified.

## Photo gallery

The manual **Photos** view presents a virtualized grid with adjustable thumbnail size. Selection, multi-selection, context menus, native file dragging, and ordinary file operations use the same behavior as the list. Subfolders and non-image files remain visible and accessible. Hidden-file settings apply to both views.

Only visible photo tiles request decoding. The gallery retains at most 128 visible preview identities, runs one bounded decoding job at a time, and retires obsolete work and textures on scrolling or view changes. Preview output is at most 320×320 pixels. Encoded input is capped at 64 MiB, raster allocation at 128 MiB, and dimensions at 16,384 pixels. Images that exceed these preview bounds retain an icon and can be opened through the existing studio or externally. Preview fitting preserves aspect ratio.

Opening a photo from Photos captures the current visible image order. The shared Image Studio provides previous/next photo controls and a slideshow with a configurable interval of 1–30 seconds. The slideshow wraps at the end of the collection and waits for each image to finish opening before starting its timer. Closing the studio ends the slideshow. Pending saves and unsaved edits block photo changes and pause the slideshow without discarding edits. Ordinary Image Studio saves retain Phase 4's source guards.

## Music

Music view now includes a configurable automatic activation threshold (75% by default), manual List/Photos/Music overrides, metadata columns, cover art or a stand-in, and bounded per-file waveform previews. Audio rows are larger than ordinary folder/file rows; the listing uses cached row offsets and renders only the viewport. Entering Music folds the Shelf without clearing collected items.

The Files-owned streaming player provides an explicit folder queue, play/pause, seek, previous/next, repeat and shuffle. Waveform clicking seeks; dragging selects a range. Range copying uses the shared guarded audio exporter and desktop clipboard/Left Hand handoff. Pasting imports clipboard content into the current folder. The private workflow has exported and pasted a selected range with duration matching the drag while preserving the original source.

## Playback ownership

Files owns its music queue and playback in its own process, using the existing shared rodio device. The queue is a snapshot of explicitly chosen files, not a live alias of the current folder. Navigating folders, toggling views, or opening a photo/text studio retains the queue. Opening an audio/MIDI studio or video session pauses queued music. Closing Files stops playback and cancels pending loads; accepted copy jobs and ordinary file operations retain their close guards. Closing a studio does not automatically resume music.

Automatic Music activation counts audio files among visible regular-file entries and excludes subfolders. Manual view choices override activation for the current location and survive refresh/history. Refresh does not replace the playback queue.

## Video backend

Video paths open an owned FFplay window from a Files viewer session. This reuses the installed FFmpeg stack for synchronized audio/video decoding and ordinary playback controls instead of introducing a second decoder and audio clock. In the video window, Space pauses/resumes, Left/Right seek ten seconds, Up/Down seek one minute, right-click seeks proportionally, F toggles fullscreen, and Q closes playback. The Files session offers replay, stop and return to folder.

One session owns at most one child. Stop, returning to the folder, replacing playback and closing Files kill and reap it. On Linux the child also receives SIGKILL if Files dies abruptly. Music remains paused until explicitly resumed after leaving the video session. Video is read-only; image/audio edits and exports use the existing protected studios and source guards.

Preflight uses core's bounded process runner for FFprobe off the UI thread, selecting the first non-cover video stream and checks FFplay availability with a three-second timeout and 4 KiB retained output. Source identity is checked after preparation and before every launch. Missing tools, invalid streams and failed player exits produce visible errors with the external-opening option.

Listing previews reuse the bounded music-cover extractor for one aspect-fitted video poster, with the same stream selection and pixel check as playback. They do not require FFplay.

The local stream pixel budget is 4096×2160 pixels, with either dimension at most 4096. FFplay uses two decoder threads, one filter thread, finite input buffering, frame dropping on slow machines and a 256 MiB **single-allocation** cap; this cap is not a total-process memory limit. Frames and audio stream through the player; Files retains no video-sized frame array. Larger streams use Open externally. Playback codecs depend on the installed FFmpeg build. The local inventory includes H.264, HEVC, VP9, AV1, MPEG-4, AAC, FLAC, Opus and Vorbis; rendered acceptance currently proves H.264/AAC, not every listed codec.

## Music resource and format expectations

Metadata and cover extraction use bounded FFprobe/FFmpeg requests (three seconds each, 64 KiB metadata and 256 KiB encoded cover output). Cover rasters are at most 160×160 with a 4 MiB allocation cap. Full-length waveforms keep 512 peaks per visible track and stream samples through the existing rodio decoder rather than retaining PCM. Decoding has a ten-second budget and cancellation checks. One preview request runs at a time, at most 128 visible identities are retained, and retired work is cancelled. Unavailable waveforms retain the file, metadata and a visible fallback with error details on hover.

Native music playback uses the existing rodio/Symphonia feature set. WAV, FLAC and the workspace's enabled compressed formats are supported according to that decoder; classifying a file as audio does not guarantee decoding. Decoder failures are displayed and ordinary next-track/external-opening controls remain available. Clipboard slice extraction uses FFmpeg and the existing guarded export model. The automatic threshold can be configured in `[file_manager]` as `music_threshold_percent`; the view's slider changes it for the current application session.

## Remaining verification

- Repeat the complete music workflow in both appearances after the latest panel sizing and preview changes, including Audio Studio conflict handling.
- Finish the combined regression/resource review and record final acceptance reports.

## Verification so far

- `cargo check -p clip-workbench --lib --bin clip-file-manager --offline` passes.
- `cargo test -p clip-workbench --lib --offline file_manager::`: 70 passed before the latest preview-source check; a final run remains pending. Coverage includes photo ordering, non-media access, slideshow wrapping, unsaved-edit retention, music activation/manual overrides, queue repeat/shuffle and bounded cancellable waveform previews.
- Two 10,000-entry tests cover ordinary files and hard-linked WAV files across List, Photos and Music, including virtualization and history viewport restoration.
- Private gallery acceptance: `/tmp/files-phase5-gallery-dark-e/report.json` passes all 12 rendered observations. It covers List/Photos switching, opening and navigating photos, timed slideshow advancement, pause, returning to the grid, and opening a non-media file from Photos.
- The latest release build also passes all 12 private Light observations in `/tmp/files-phase5-gallery-light-b/report.json`; screenshots of both appearances were inspected.
- `./launch.sh build --bin clip-file-manager`, `cargo fmt --all --check`, and `git diff --check` pass.
- The gallery-only architecture comparison was neutral. During music implementation, excessive-complexity functions and repeated callbacks were simplified; the subsequent comparison retained maximum complexity and cycle count but still flagged existing audio-export clone fragments. Final combined review remains pending.
- The private music runner exercised playback controls, repeat/shuffle, navigation and manual-view retention through 11 observations. Its final tab-selection assertion failed because the OCR selector chose the “Play folder music” button; the selector is corrected but the complete run has not yet been repeated.
- Latest music layout changes pass the 66 targeted tests; the prior release build and rendered observations precede those changes. The script supports gallery and music scenarios, but does not establish full Phase 5 acceptance.

- Current gallery release acceptance passes all 12 observations in both `/tmp/files-phase5-gallery-current-dark/report.json` and `/tmp/files-phase5-gallery-current-light/report.json`.
- Video acceptance passes advancing-frame and pause-frame comparisons, replay, and return-to-folder child reaping in Dark and Light. `/tmp/files-phase5-video-close-light/report.json` also verifies application-close cleanup.
- Music acceptance captures non-silent PCM from an isolated PipeWire null sink. Waveform seeking, range export and paste succeeded in `/tmp/files-phase5-music-wave-fixed-dark`; a final OCR filename assertion failed after the filesystem/duration/source-integrity checks had passed. The assertion now checks the rendered item count; complete reruns remain pending.
- `cargo test --workspace --offline -- --test-threads=1` passes across the workspace (initial default parallel execution failed 33 background-job tests; serial execution passed those tests). Further source changes require a final rerun.
- Combined architecture measurement: clone groups 1297→1296, duplication 7.98%→7.88%, maximum complexity 196 and cycle count 2 unchanged. Comparing whitespace-normalized clone fragments with the baseline finds zero new fragments; the service's regression label is caused by existing fragments in touched files. Increased callers of shared acceptance helpers are expected reuse.
