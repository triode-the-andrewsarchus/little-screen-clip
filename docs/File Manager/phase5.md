# Phase 5: Gallery, music, and video

Status: in progress. Phase 4 remains in progress independently. Phase 5 is not complete until all scope in [plan.md](plan.md) is implemented and verified.

## Photo gallery

The manual **Photos** view presents a virtualized grid with adjustable thumbnail size. Selection, multi-selection, context menus, native file dragging, and ordinary file operations use the same behavior as the list. Subfolders and non-image files remain visible and accessible. Hidden-file settings apply to both views.

Only visible photo tiles request decoding. The gallery retains at most 128 visible preview identities, runs one bounded decoding job at a time, and retires obsolete work and textures on scrolling or view changes. Preview output is at most 320×320 pixels. Encoded input is capped at 64 MiB, raster allocation at 128 MiB, and dimensions at 16,384 pixels. Images that exceed these preview bounds retain an icon and can be opened through the existing studio or externally. Preview fitting preserves aspect ratio.

Opening a photo from Photos captures the current visible image order. The shared Image Studio provides previous/next photo controls and a slideshow with a configurable interval of 1–30 seconds. The slideshow wraps at the end of the collection and waits for each image to finish opening before starting its timer. Closing the studio ends the slideshow. Pending saves and unsaved edits block photo changes and pause the slideshow without discarding edits. Ordinary Image Studio saves retain Phase 4's source guards.

## Music checkpoint

Music view now includes a configurable automatic activation threshold (75% by default), manual List/Photos/Music overrides, metadata columns, cover art or a stand-in, and bounded per-file waveform previews. Audio rows are larger than ordinary folder/file rows; the listing uses cached row offsets and renders only the viewport. Entering Music folds the Shelf without clearing collected items.

The Files-owned streaming player provides an explicit folder queue, play/pause, seek, previous/next, repeat and shuffle. Waveform clicking seeks; dragging selects a range. Range copying uses the shared guarded audio exporter and desktop clipboard/Left Hand handoff. Pasting imports clipboard content into the current folder. These direct waveform workflows still need rendered acceptance.

## Playback ownership

Files owns its music queue and playback in its own process, using the existing shared rodio device. The queue is a snapshot of explicitly chosen files, not a live alias of the current folder. Navigating folders, toggling views, or opening a photo/text studio retains the queue. Opening an audio/MIDI studio pauses queued music. Closing Files stops playback and cancels pending loads; accepted copy jobs and ordinary file operations retain their close guards. Video integration must extend this contract before completion.

Automatic Music activation counts audio files among visible regular-file entries and excludes subfolders. Manual view choices override activation for the current location and survive refresh/history. Refresh does not replace the playback queue.

## Remaining scope

- Complete rendered acceptance for waveform seeking, selection, copying and pasting, plus actual audio-output capture.
- Verify the latest compact-row layout, Shelf folding and player-panel sizing in both appearances.
- Exercise very large audio directories and playback/studio save interactions.
- Integrate an assessed video backend, browsing/playback, codec capabilities and resource limits, including process cleanup and music conflict handling.
- Complete regression, resource and architecture review of the combined media implementation.

## Video backend assessment so far

Core already classifies common video paths and extracts FFmpeg previews, but there is no shared video playback studio. This development machine has `ffplay`, `ffmpeg`, and `ffprobe`. Its installed decoder inventory includes H.264, HEVC, VP9, AV1, MPEG-4, AAC, FLAC, Opus, and Vorbis. This establishes available local tools, not working Files playback or universal codec support. Playback backend integration, process cleanup, audio/video synchronization, supported controls, missing-tool behavior, and bounded resource use remain unverified and required.

## Verification so far

- `cargo check -p clip-workbench --lib --bin clip-file-manager --offline` passes.
- `cargo test -p clip-workbench --lib --offline file_manager::`: 66 passed on the current checkpoint. Coverage includes photo ordering, non-media access, slideshow wrapping, unsaved-edit retention, music activation/manual overrides, queue repeat/shuffle and bounded cancellable waveform previews.
- The 10,000-entry test exercises both List and Photos, including virtualization and history viewport restoration.
- Private gallery acceptance: `/tmp/files-phase5-gallery-dark-e/report.json` passes all 12 rendered observations. It covers List/Photos switching, opening and navigating photos, timed slideshow advancement, pause, returning to the grid, and opening a non-media file from Photos.
- The latest release build also passes all 12 private Light observations in `/tmp/files-phase5-gallery-light-b/report.json`; screenshots of both appearances were inspected.
- `./launch.sh build --bin clip-file-manager`, `cargo fmt --all --check`, and `git diff --check` pass.
- The gallery-only architecture comparison was neutral. During music implementation, excessive-complexity functions and repeated callbacks were simplified; the subsequent comparison retained maximum complexity and cycle count but still flagged existing audio-export clone fragments. Final combined review remains pending.
- The private music runner exercised playback controls, repeat/shuffle, navigation and manual-view retention through 11 observations. Its final tab-selection assertion failed because the OCR selector chose the “Play folder music” button; the selector is corrected but the complete run has not yet been repeated.
- Latest music layout changes pass the 66 targeted tests; the prior release build and rendered observations precede those changes. The script supports gallery and music scenarios, but does not establish full Phase 5 acceptance.
