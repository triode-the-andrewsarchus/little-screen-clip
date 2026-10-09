# Phase 5: Gallery, music, and video

Status: complete against the Phase 5 scope and completion gate in [plan.md](plan.md). Phase 4 remains in progress independently. The backend and resource limits below are part of this delivery.

## Photo gallery

The manual **Photos** view follows [photo_grid_spec.md](../../photo_grid_spec.md). Its half-square grid allows exactly five shapes: 2×2, 3×2, 4×2, 2×3 and 2×4. Bounded row candidates and two-row memoized lookahead minimize crop loss plus wasted space and ending penalties. Descenders start/end on even X coordinates and participate in the next row; a final descender without following images is illegal. All ordinary row boundaries have even widths. Cropping is centered presentation only. Selection insets, checkmarks, Select mode, Ctrl/Shift selection, menus, file dragging and ordinary file operations remain shared with the list.

Ordered groups target 48 entries (`gallery::CHUNK_SIZE`), rounded to a multiple of the square-column count, and are independent chunks. Filled rows are mandatory: extra cropping takes precedence over gaps. All nonfinal chunks fill their entire rectangle; only the actual folder ending may leave trailing unused columns. The bounded candidate search retains a square fallback, and descenders must leave enough images to fill their continuation regions. Metadata, layouts and thumbnails have separate caches. Dimensions and EXIF orientation come from `imagesize`/`kamadak-exif` header inspection; HEIF/AVIF primary dimensions, native crop and rotation come from libheif container metadata without decoding. RAW metadata and embedded previews use the project's existing ExifTool backend without developing RAW pixels. Compact metadata persists under the XDG cache directory with file revision keys; layouts cache their grid generation and chunk identities. Modifying a file retires only its affected chunk and thumbnail. Integer column changes invalidate packing and, if needed, repartition chunks while preserving per-file metadata and thumbnail caches. Scroll anchors use absolute item indices to survive repartitioning.

The cumulative chunk-height index initially uses the exact height of filled square placeholders and replaces it with packed results. Unknown chunks do not reserve additional blank estimated-height spacers. Corrections preserve the visible item and its viewport offset, including placeholder-to-layout changes. Jumping into a 10,000-image folder creates local square placeholders immediately; no preceding chunk solve or image decode is required. Separate bounded executors run metadata probing (four workers by default, configurable from one to eight with `LITTLE_CLIP_GALLERY_METADATA_WORKERS`), chunk packing (two workers), and thumbnail decoding (four workers). Visible chunks and neighbors precede other chunks. Rapid jumps cancel speculative jobs; layout generations and identities reject obsolete results.

Only laid-out visible images and a small overscan region request thumbnails, sorted by distance to the viewport. Only viewport and overscan identities are retained; there is no fixed ceiling that would leave visible photos permanently undecoded. Resolution is quantized to 128-pixel bands, capped at 4096 pixels, accounting for tile size and DPI. Width reductions reuse larger thumbnails; width increases replace undersized previews. Compressed generated thumbnails persist separately under the artifact cache. HEIF, RAW, JPEG and TIFF sources prefer suitable embedded previews; JPEG EXIF previews can display without decoding primary pixels. Ordinary raster input remains capped at 64 MiB, allocation at 128 MiB and dimensions at 16,384 pixels; unsupported or oversized previews retain a placeholder and external/studio opening. Original files are never cropped or rewritten. Subfolders, non-images and hidden-file settings retain their normal behavior.

Opening a photo from any folder view captures the current visible image order. Unmodified Left/Right keys navigate that collection, except while typing or adjusting the slideshow interval. The shared Image Studio provides previous/next photo controls and a slideshow with a configurable interval of 1–30 seconds. The slideshow wraps at the end of the collection and waits for each image to finish opening before starting its timer. Closing the studio ends the slideshow. Pending saves and unsaved edits block photo changes and pause the slideshow without discarding edits. Ordinary Image Studio saves retain Phase 4's source guards.

## Music

Music view now includes a configurable automatic activation threshold (75% by default), manual List/Photos/Music overrides, metadata columns, cover art or a stand-in, and bounded per-file waveform previews. Audio rows are larger than ordinary folder/file rows; the listing uses cached row offsets and renders only the viewport. Entering Music folds the Shelf without clearing collected items.

Files previews one explicitly chosen track. Clicking its waveform starts playback at that position or seeks the active track. There is no bottom player panel, current-file label, seek slider, queue, previous/next, repeat or shuffle controls; the old Play folder music action is removed. A completed track stops without advancing or repeating. Ctrl/Shift+click on any part of a music row, including its waveform, uses file multi-selection instead of playback or audio-range interaction. Plain waveform clicks play or seek; dragging selects an audio range. Range copying uses the shared guarded audio exporter and desktop clipboard/Left Hand handoff. Pasting imports clipboard content into the current folder. The private workflow has exported and pasted a selected range with duration matching the drag while preserving the original source.

## Playback ownership

Files owns one music preview in its own process, using the existing shared rodio device. Navigating to another folder by Back, Forward, Up, address entry, tree selection or opening a subfolder stops playback and cancels pending loads immediately. Space in Music starts the active audio range at its earlier endpoint and stops at its later endpoint without looping. Range edges always snap to nearby zero crossings, using the shared workbench algorithm (500 sample frames, with a quietest-sample fallback when there is no crossing). Endpoint windows decode off the UI thread; stale results are ignored, and copying or playback waits for snapping. Playback duration is bounded in the audio source itself. Without an active range, Space stops playback. Escape always stops preview playback. Text editing retains normal Space behavior while stopping playback. Manual and automatic changes to List stop playback. Opening an audio/MIDI studio or video session also stops it. Accepted audio-copy jobs survive playback stops, and closing Files retains their close guards. Nothing automatically resumes playback.

Automatic Music activation counts audio files among visible regular-file entries and excludes subfolders. Manual view choices override activation for the current location and survive refresh/history. Refreshing the same folder retains the active preview unless the resulting view changes to List.

## Video backend

Video paths open an owned FFplay window from a Files viewer session. This reuses the installed FFmpeg stack for synchronized audio/video decoding and ordinary playback controls instead of introducing a second decoder and audio clock. In the video window, Space pauses/resumes, Left/Right seek ten seconds, Up/Down seek one minute, right-click seeks proportionally, F toggles fullscreen, and Q closes playback. The Files session offers replay, stop and return to folder.

One session owns at most one child. Stop, returning to the folder, replacing playback and closing Files kill and reap it. On Linux the child also receives SIGKILL if Files dies abruptly. Music remains stopped after leaving the video session. Video is read-only; image/audio edits and exports use the existing protected studios and source guards.

Preflight uses core's bounded process runner for FFprobe off the UI thread, selects the first non-cover video stream, and checks FFplay availability with a three-second timeout and 4 KiB retained output. Source identity is checked after preparation and before every launch. Missing tools, invalid streams and failed player exits produce visible errors with the external-opening option.

Listing previews reuse the bounded music-cover extractor for one aspect-fitted video poster, with the same stream selection and pixel check as playback. They do not require FFplay.

The local stream pixel budget is 4096×2160 pixels, with either dimension at most 4096. FFplay uses two decoder threads, one filter thread, finite input buffering, frame dropping on slow machines and a 256 MiB **single-allocation** cap; this cap is not a total-process memory limit. Frames and audio stream through the player; Files retains no video-sized frame array. Larger streams use Open externally. Playback codecs depend on the installed FFmpeg build. The local inventory includes H.264, HEVC, VP9, AV1, MPEG-4, AAC, FLAC, Opus and Vorbis; rendered acceptance currently proves H.264/AAC, not every listed codec.

## Music resource and format expectations

Metadata and cover extraction use bounded FFprobe/FFmpeg requests (three seconds each, 64 KiB metadata and 256 KiB encoded cover output). Cover rasters are at most 160×160 with a 4 MiB allocation cap. Waveforms share core's streaming `WaveformColor` crossover with the workbench: warm bass, green mids and blue highs, with the shared accent for selected ranges. Each channel retains independent filter history and previews average channel colors without phase-cancelling downmixes. Listing and Shelf excerpts use the same calculation. Full-length waveforms keep 512 peaks and colors per visible track and decode Symphonia planar packets rather than retaining PCM or dispatching individual interleaved samples. Decoding has a ten-second budget and cancellation checks. At most eight music previews run concurrently (bounded by available CPUs) on a dedicated executor, leaving the shared job executor available for other work. Each music job overlaps its bounded metadata/cover helper with decoding; at most eight such helpers exist. Ordinary listing and Shelf previews retain single-job admission; gallery decoding uses its separate four-worker queue. At most 128 visible identities are retained, and retired work is cancelled. Unavailable waveforms retain the file, metadata and a visible fallback with error details on hover.

Native music playback uses the existing rodio/Symphonia feature set. WAV, FLAC and the workspace's enabled compressed formats are supported according to that decoder; classifying a file as audio does not guarantee decoding. Decoder failures are displayed and ordinary next-track/external-opening controls remain available. Clipboard slice extraction uses FFmpeg and the existing guarded export model. The automatic threshold can be configured in `[file_manager]` as `music_threshold_percent`; the view's slider changes it for the current application session.

## Completion audit

| Phase 5 requirement | Implementation and evidence |
| --- | --- |
| Adjustable photo grid and selection | Virtualized `ui/gallery.rs`, shared selection/actions, 80–280 px size control. Private gallery runs verify both decoded thumbnails and mixed-folder access. |
| Next/previous and slideshow | `viewers/gallery.rs`; rendered navigation, timed advancement and pause. Unit tests cover ordering, wrapping and unsaved edits blocking advancement. |
| Music metadata, covers and waveforms | Visible cards contain title, artist, album, duration, cover/stand-in and 512 peaks. Rendered runs verify tagged FLAC; unit tests cover missing covers and cancellation. |
| Configurable automatic Music and manual views | Visible regular-file ratio, default 75%, TOML configuration and session slider; unit tests cover configuration and ratios, rendered runs verify manual override through refresh/history. |
| Single-track playback and waveform seek | Owned rodio preview; no advancement, repeat or bottom controls. Playback stops on navigation, Escape and List; Space plays a selected range once or stops when none is active. Earlier rendered queue-control runs describe the historical implementation. |
| Direct waveform selection/copy/paste | Rendered playhead seek and range selection; Ctrl+C exports a guarded slice and Ctrl+V imports it. The pasted duration matches the selected interval and the original SHA-256 is unchanged. |
| Large folders and bounded previews | Two 10,000-entry fixtures exercise List, Photos and Music. Visible text shape counts stay below 200, the last entry is not laid out, and history restores selection/scroll. Gallery caches retain only viewport/overscan identities; other preview caches retain at most 128 visible identities; gallery admits four thumbnail jobs; List/Shelf admit one job and Music admits at most eight on a dedicated executor. |
| Navigation and close ownership | Current music policy stops on navigation and audio/video studio entry; earlier rendered music navigation results are historical. Video return/app-close tests verify child reaping. Source checks reject changed video replay. |
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

## Single-track playback follow-up

The user's later playback policy replaces the original queue workflow. The entire bottom media dock and its rendering regression test are removed, along with the queue module and repeat/shuffle logic. Playback errors and accepted audio-copy progress are shown in the Music toolbar. The existing manual-view/history test now expects stopped playback; no new tests were added or run for this request. `cargo check -p clip-workbench --all-targets --offline` and `./launch.sh build --bin clip-file-manager` pass. The historical Music acceptance scenario in `scripts/file_manager_phase5_acceptance.py` targets the removed queue controls and must be revised before running it against this policy; the earlier runtime evidence above does not validate this change.

## Adaptive photo grid spec verification

The `photo_grid_spec.md` implementation replaces the earlier square-only gallery. Verification on the local machine:

- `cargo test --workspace --offline -- --test-threads=1`: 1,913 passed, 23 ignored. After the final placeholder-space regression fix, `cargo test -p clip-workbench --lib --release file_manager::gallery --offline -- --test-threads=1 --nocapture` passes all 20 gallery tests, including that additional regression.
- Tests cover every specified geometry and common camera ratio, small galleries, mixed/tall endings, even descender boundaries, lookahead versus greedy packing, independent chunks, resizing, 10,000-entry middle jumps, visible-item anchoring, source revisions, header-only probes, orientation, suitable embedded JPEGs, duplicate-decode prevention and out-of-order/cancelled completions. Unknown chunks reserve enough height to prevent square placeholders overlapping later chunks. Wide-grid tests extend through 124 half-square units.
- Optimized synthetic packing: 9.4 ms for the visible 48-item chunk; 1.78 s to pack all 10,000 aspect records serially. These timings exclude filesystem reads and thumbnail decoding. Production packing uses two workers and never requires predecessor chunks for a direct jump.
- Private Weston/Niri gallery acceptance passes all 12 observations in both appearances: `/tmp/photo-grid-spec-gallery-dark-verified/report.json` and `/tmp/photo-grid-spec-gallery-light-verified/report.json`. The checks verify decoded source pixels, individual folder/file captions, photo navigation, slideshow, non-media opening, and return to List. Caption OCR uses rendered tile bounds because full-window OCR omits captions beside large colored tiles. The local runner uses `TESSDATA_PREFIX=/home/vega/.local/share/tessdata` for its installed English model.
- `cargo check -p clip-workbench --all-targets --offline`, `./launch.sh build --bin clip-file-manager`, Rust formatting, Python script compilation and diff whitespace checks pass. The all-target check retains six existing workbench binary warnings.
- Architecture comparison against `pre-photo-grid-spec`: duplication decreases from 7.82% to 7.74%; maximum complexity stays at 196 and dependency cycles at two. The report flags one six-line test-setup clone plus pre-existing fragments in touched core code, with no new high-complexity production function. Shared regular-file opening, raster limits and thumbnail premultiplication remove production duplication.

## Gap-free grid follow-up

Filled grid regions are a hard constraint, taking precedence over crop loss. Batch sizes target 48 entries but round to a multiple of the current square-column count (for example, 50 entries at five columns or 49 at seven). Every nonfinal chunk fills its rectangle. Row candidates with holes are rejected; descender continuations must be fillable with remaining images. The bounded search can use filled square rows as a fallback. Only the actual folder ending permits trailing unused columns. Unknown chunks use exact square-placeholder heights, eliminating estimated-height spacers. Resizing preserves per-file metadata caches and absolute item anchors when batch boundaries move.

- All 23 optimized gallery tests pass, including cell-by-cell coverage of chunk boundaries and mixed ratios across grid widths 2–124, filled unknown batches, resize repartitioning and 10,000-image direct jumps.
- Optimized production packing measures 7.5 ms for a 48-item visible chunk and 1.52 s for 10,000 synthetic aspect records serially.
- Private rendered acceptance scrolls through 150 mixed square/landscape/portrait photos, with pixel coverage checks at 16 scroll positions. It passes all 18 observations in `/tmp/gapless-gallery-rendered-dark-final/report.json`. Two-pixel gutters are intentional; empty grid regions fail the check.
- All-target Cargo checking, release build through `launch.sh`, formatting and diff whitespace checks pass. Architecture comparison against `pre-gapless-gallery` leaves clone count, maximum complexity and dependency cycles unchanged; flagged duplicate fragments were already present at the baseline.


## Videos timeline view

The shared view tabs include **Videos**, selected automatically when at least 75% of visible regular files are videos, unless a manual view was chosen. Each video has a horizontal timeline strip, filename, dimensions and duration. Folders and other files retain compact rows. Hover shows a timestamp and an aspect-preserving frame; clicking a strip opens playback at that position. Ctrl/Command/Shift-click selects files. Enter and double-click open from the beginning.

### Resolution and decoding

Columns follow the painted strip's **physical pixel width**, including desktop scale, without 256-column rounding or a 2,048-column cap. The bounded maximum is 16,384 columns. `[file_manager] video_timeline_decimation` accepts `1` (default), `2`, `4`, `8`, or `16`; invalid factors fall back to 1. Restart Files after changing it. Decimation reduces generation resolution while retaining the strip's display width.

Four bounded background jobs process visible rows. Short videos (up to ten minutes) use patched Timelens ordinary-frame streaming, selecting frames before scaling and color conversion. Longer videos use the independent FFmpeg indexed helper: each requested column chooses the nearest indexed keyframe, and each distinct selected keyframe is decoded once. Decoder contexts own disjoint column ranges; counts now follow the workload policy below. Decoder draining preserves reordered/open-GOP pictures. Large H.264 and HEVC timelines, including cropped/portrait 4K, prefer CUDA with CPU retry; other tested codecs favor CPU. CUDA decoders share one device per file. Unsupported H.264 pixel formats are rejected before device creation.

Sparse reads on Linux CIFS/SMB2 mounts use aligned, read-only direct I/O to avoid excess kernel read-ahead. Normal local files retain buffered caching. This changes no mount settings. A measured H.264 run fetched 290 MB instead of 1.21 GB, with the same selected pictures. The indexed helper writes lossless 64-pixel-high PPM strips through the existing bounded image reader. Missing helpers/indices or decoder errors use the previous Timelens/AV1 streaming fallback. Native attempts allow 30 seconds each, or 60 for 4K inputs; fallback allows 240 seconds within a 370-second task deadline. Cancellation kills/reaps read-only children.

Long-video timelines approximate changes between keyframes. There is **no fixed low sample cap**: increasing width can select additional indexed frames. The rejected 48-sample implementation is not used. Persistent strips live in the existing artifact store under `video-timelines-v3`; source identity, selected stream and width determine the cache key.

### Hover and lifecycle

Hover owns one persistent Timelens decoder on an independent bounded queue. Pointer requests coalesce immediately without a hover pause; metadata is published before timeline completion. Recent keyframes are cached, raw frame transport avoids PNG round trips, and the latest completed picture stays visible with its actual timestamp while the next decodes. Supported HEVC hover uses GPU decode/scale and preview-sized downloads. Replaced source/revision/resolution results are rejected. Navigation, studio entry and scrolling retire read-only jobs and textures. Playback remains the existing owned FFplay session, using the same probed non-cover stream and killing/reaping its child on exit.

### Measurements

Optimized builds on this machine, 3,200 columns, four admitted jobs, three files per batch. These runs bypass Files' artifact cache; storage caches were not flushed. Folders are under `/mnt/rust/Jellyfin/Shows/` on CIFS. Timings include helper completion, excluding application painting.

| Folder / encoding | Individual strips | Three-file batch | Warm hover median / p95 |
| --- | --- | --- | --- |
| AMV: short H.264 MKV | 2.47–3.77 s | 3.82 s | 6.62 / 10.34 ms |
| Lazarus: HEVC MKV | 4.62–4.82 s | 4.98 s | 10.51 / 76.06 ms |
| Umbrella Academy: H.264 MKV | 9.93–10.78 s | 10.83 s | 20.92 / 46.16 ms |
| House of the Dragon: two AV1 MKV, one HEVC MP4 | 1.77 s AV1; 5.17 s MP4 | 5.32 s | 8.32 / 9.63 ms (AV1) |

Cached reverse hover medians were 0.16–0.24 ms; cold startup was 98–636 ms. Individual uncached requests can exceed those medians. Previous 3,200-column Timelens measurements were 29.31 s for Lazarus, 48.54 s for the AV1 episode and 36.17 s for the HEVC MP4. Those earlier runs overlapped other inputs, so they establish the prior cost rather than an isolated speedup ratio. The CIFS identifier correction reduced the three H.264 production batch from 41.47 s to 10.83 s without changing samples.

The requested Music reference on `/home/vega/Documents/type_testing/Ashes Against The Grain` measured eight-track cache rounds of 691.5, 675.0 and 675.1 ms; individual waveform-only times were 105–646 ms. Video generation is substantially faster than before, but remains slower than this audio baseline. Dense H.264 indices and large 4K inputs retain higher costs. The 29.6 GB, 9,382-second Project Hail Mary HEVC movie generated 3,200 columns in 19.73 s on the final production CUDA path (an earlier run took 25.92 s), compared with the prior 136.87 s at only 2,048 columns. Its CPU run took 50.74 s while overlapping a quality check; production therefore prefers CUDA for 4K HEVC. At 16× decimation its 200-column strip took 1.67 s; warm hover median/p95 was 92.13/138.46 ms, with cached reverse requests at 0.17 ms. Movie hover is responsive to coalesced requests but still exceeds one display frame for uncached pictures.

### Build, licensing and validation

`launch.sh build` and `launch.sh files` build both helpers as separate GPL-2.0-or-later executables. Timelens is pinned to commit `988c9f4773b25916aa404801c9e0f468f8052321`; its first build needs GitHub, Rust, GStreamer/GStreamer App development packages and suitable runtime codec plugins. The indexed helper requires a C compiler, pthreads, `pkg-config` and FFmpeg development libraries (`libavformat`, `libavcodec`, `libavutil`, `libswscale`); missing development libraries retain the fallback. FFprobe supplies metadata and FFplay playback. Overrides are `LITTLE_CLIP_TIMELENS` and `LITTLE_CLIP_VIDEO_STRIP`.

`scripts/README.timelens.md` beside the build script and patch explains the GPL v2-or-later licensing boundary. The installed Timelens directory retains complete modified source, patch, README and license. `scripts/video_strip/README.md` explains the independent helper's license, corresponding source and rebuild instructions; these are copied beside its binary. Neither helper links into the application library.

Reproduce with `scripts/video_preview_benchmark.py`, which follows production routing and defaults to 3,200 columns. `scripts/video_timeline_resolution_check.py --gpu` creates synthetic H.264/HEVC/AV1 luminance ramps: indexed CPU/CUDA preserve all 128 steps at 200/400/800/1,600/3,200 columns, and ordinary-frame Timelens preserves 126 at 1,024/2,048/3,200. Direct/buffered readers also produced byte-identical real H.264 MKV, HEVC MKV, AV1 MKV and HEVC MP4 strips.

The private 4K Weston/Niri acceptance script verifies actual requested helper widths against painted strip pixels: a 3,134-pixel strip requests 3,134 columns at 1× and 196 at 16×. It also checks clean application closure. Targeted Rust tests cover bounded PPM decoding, configuration factors, and selected-stream/pixel-budget behavior. Release build, formatting, shell syntax, Python compilation and diff whitespace checks pass. Architecture comparison against `pre-indexed-video-helper` leaves maximum complexity, clone count and dependency cycles unchanged and reduces duplication slightly. Its remaining flag is an unchanged 13-line editor-environment fallback clone already present in that baseline.

### io_uring and workload dispatch follow-up

Compared the low-level [`io-uring`](https://github.com/tokio-rs/io-uring), [`tokio-uring`](https://github.com/tokio-rs/tokio-uring) and [`Monoio`](https://github.com/monoio-rs/monoio) crates. Chose `io-uring` 0.7.15: explicit queues fit the helper's existing thread-owned synchronous FFmpeg callbacks without adding an async runtime. The separate GPL helper now includes a Rust static library with bounded indexed/sequential read-ahead, aligned buffers and completion tracking. Each decoder owns 16 × 64 KiB buffers; pending buffers cannot be reused, and shutdown drains reads before freeing buffers or closing the borrowed descriptor. Setup/read errors fall back to synchronous reads. Corresponding Rust source, lockfile, vendored dependency sources/licenses and offline rebuild instructions are retained beside the installed helper.

`--io-uring` enables this experiment directly; `LITTLE_CLIP_VIDEO_IO=uring` enables it for uncached Files strips. **Default remains synchronous** because trials did not show a consistent improvement. Buffered local caching and CIFS/SMB2 direct reads retain their existing storage-specific behavior.

After FFprobe, dispatch uses codec, decoded pixel count, file bytes, duration, visible file count and available CPU parallelism. At most four file jobs are admitted, fewer on low-core machines. CPU workers divide a target budget capped at 16 cores, with a maximum of eight per heavy file or four per lighter file. Heavy means at least 512 MiB, 30 minutes or two million pixels. Large H.264 (at least two million pixels) and HEVC (at least four million pixels) prefer GPU decode. Pixel count includes cropped/portrait 4K inputs previously missed by a width-only check. Single GPU files use four contexts; concurrent GPU files use two each, bounded further by CPU availability. CPU fallback retains its independently chosen worker count. Short videos retain ordinary-frame Timelens streaming.

Local fixtures are in `/home/vega/Videos/video-preview-tests-20261008/`: 11 complete files in six workload folders, **18,808,224,779 bytes**, below the 20,000,000,000-byte limit. They cover short H.264, two H.264 episodes, two HEVC episodes, mixed AV1 MKV/HEVC MP4, cropped 2862×2158 HEVC and full 3840×2160 HEVC. All local file SHA-256 values were reread and verified against the copy manifest. Source files were not modified. `scripts/copy_video_test_fixtures.py` provides budgeted, grouped, atomic no-overwrite copies and a manifest for future experiments.

Worker sweeps and paired reader trials use 3,200 columns and compare output hashes across modes/counts. Local two-file CPU HEVC batches measured about 3.47 s with four workers per file and 2.66–2.82 s with eight. H.264 GPU batches showed little benefit above two workers per file. The full 3840×2160 single-file GPU sweep measured about 11.46 s at four workers, versus 12.61–12.96 s at two, 12.88–13.08 s at eight and 13.26–13.55 s at sixteen. This supports four workers for a lone large GPU file without treating more workers as automatically faster.

After compilation completed, paired tests measured local HEVC batches at 2.57–2.60 s synchronous versus 2.62–2.66 s with io_uring; NAS equivalents were 2.90–2.99 s versus 3.13–3.22 s. Mixed AV1/HEVC MP4 local batches were about 2.8 s in either mode; NAS batches were 3.53–3.60 s synchronous versus 4.11–4.14 s with io_uring. H.264 GPU runs showed wider variation and NAS prefetch regressions. Cold-local full-4K pairs were about 12–13.5 s with either reader. The io_uring prototype is therefore retained for experiments, rather than enabled automatically. Every paired strip had the same output hash. Earlier trials overlapping compilation are excluded from these comparisons; NAS cache state remains uncontrolled.

Reproduce with:

```sh
python3 scripts/video_workload_benchmark.py /home/vega/Videos/video-preview-tests-20261008/manifest.json --compare-io
python3 scripts/video_workload_benchmark.py /home/vega/Videos/video-preview-tests-20261008/manifest.json --folders single-wide-4k --storage local --workers 2,4,8,16
python3 scripts/video_workload_benchmark.py /home/vega/Videos/video-preview-tests-20261008/manifest.json --folders single-wide-4k --storage local --compare-io --cold-local
python3 scripts/video_timeline_resolution_check.py --gpu --io-uring
cargo test --release --manifest-path scripts/video_strip/rust_io/Cargo.toml
```

Cold-local runs evict only the selected file's clean cached pages with `posix_fadvise`; no global cache drop, NAS tuning, GPU clock changes or mount changes are performed. Synthetic Rust tests cover random seeks, eviction, direct reads, unaligned EOF, read failures and destruction with pending work. A private child with `io_uring_setup` denied by seccomp generated a byte-identical strip through synchronous fallback. Synthetic native CPU/CUDA strip checks retain all 128 luminance steps at every decimation width with prefetch enabled. Dispatch tests cover cropped 4K, size/duration classification, CPU budgets and concurrent GPU limits.

Final validation passes the release Files build, three Rust reader tests, four dispatch/image-reader tests, synthetic CPU/CUDA detail checks with io_uring enabled, denied-ring fallback, offline rebuilding from retained vendored source, formatting and diff checks. Private 4K acceptance with prefetch enabled verifies 3,134 requested/painted columns at 1× and 196 requested columns across the same painted width at 16×. The harness now waits for Niri's resize transform to settle before applying the existing width assertion. Short-video three-file production batches measured 3.86 s on both local disk and NAS. Architecture comparison introduces no new function above the complexity threshold; its remaining duplication flag is the pre-existing editor-environment fallback in `config.rs`.

### GPU full-frame area reduction

The standalone GPL helper now reduces complete NV12/P010 CUDA frames to a 1×64 column on FFmpeg's existing CUDA device and stream. Every pixel contributes, with fractional vertical overlap at row boundaries; this does not sample a few slices or reduce temporal resolution. Only 384 bytes of averaged YUV values are downloaded per frame, followed by a tiny libswscale RGB conversion. Previously the helper downloaded full frames and performed full-frame CPU area scaling. Decoder-owned modules/buffers are synchronized before frame reuse and released before decoder teardown. Unsupported formats, missing optional builds/drivers and reduction failures retain the complete-frame fallback.

Builds with `nvcc` and `ffnvcodec` headers compile/embed PTX; running them needs the CUDA driver but no compiler or CUDA runtime library. Source, generated PTX/header, GPL notices and rebuild instructions remain beside the independent executable. GPU averaging is enabled by default in capable builds; `VIDEO_STRIP_GPU_AVERAGE=0` selects the old path for comparisons. CPU and short ordinary-frame Timelens paths are unchanged.

On the RTX 3090, three alternating paired runs at **3,200 columns** measured these medians. These direct helper requests ignore the application's `config.toml` decimation setting (including 4×). Timings include helper startup, indexing, decoding, reduction and PPM output, exclude FFprobe/UI painting, and do not overlap compilation, fixture copying or synthetic encoding. Local files use the existing copies; NAS cache state and background GPU activity are uncontrolled.

| Workload | Full-frame download / CPU area | GPU area reduction | Speedup |
| --- | ---: | ---: | ---: |
| Full 3840×2160 HEVC, local, four workers | 12.38 s | 7.56 s | 1.64× |
| Cropped 2862×2158 HEVC, local, four workers | 3.78 s | 2.86 s | 1.32× |
| Two 1080p H.264 files, local, two workers each | 6.42 s | 6.42 s | 1.00× |
| Project Hail Mary, NAS, four workers | 14.27 s | 10.71 s | 1.33× |

Per-worker host-call profiling supports the source of the gain: in the full-4K case, median aggregate transfer plus CPU scaling time was 27.13 worker-seconds, replaced by 1.68 worker-seconds for reduction, small transfer and conversion. These are overlapping worker sums, **not elapsed time**. CUDA synchronization can move pending work into another host call, so these are not isolated GPU kernel timings. The remaining seek/read/decode path dominates the accelerated runs; the 1080p batch had much less transfer/scaling work and no measurable wall-time improvement.

Real-file average RGB differences from the previous area filter were 0.48–0.95 levels on the 0–255 scale; 99th-percentile differences were 2–5 levels. Different rounding/chroma reduction means output is not byte-identical, but sample count/order and complete-frame coverage are retained. Nonuniform 8-bit H.264 and 10-bit HEVC synthetic fixtures pass with one/four workers and verify that the default uses the reducer. Builds without reduction and an intentionally invalid PTX module fall back to byte-identical old output. The 128-frame luminance regression preserves all 128 steps at 200/400/800/1,600/3,200 columns with CUDA averaging.

Reproduce spatial/default/fallback checks and paired benchmarks with:

```sh
scripts/build_video_strip.sh
python3 scripts/video_gpu_average_check.py --synthetic
python3 scripts/video_gpu_average_check.py --manifest /home/vega/Videos/video-preview-tests-20261008/manifest.json
VIDEO_STRIP_GPU_AVERAGE=1 python3 scripts/video_timeline_resolution_check.py --gpu
```

`--storage source` selects NAS originals from the fixture manifest; `--folders`, `--width` and `--rounds` control the benchmark cases. `--fallback-helper PATH` adds synthetic comparisons against a reducer-unavailable helper. `VIDEO_STRIP_PROFILE=1` exposes per-worker stage durations and averaged/downloaded frame counts. No additional local fixture copies were made.

Architecture comparison against `pre-gpu-average` leaves maximum complexity and clone/cycle counts unchanged, reduces duplication by 0.02 percentage points, and introduces no function above the complexity threshold after separating CUDA submission, column color conversion and complete-frame fallback. Remaining flags are the config editor-environment clone already in that baseline and rank changes between unchanged UI acceptance helpers. A final freshly rebuilt 4K pair measured 12.87 s versus 7.54 s (1.71×), consistent with the three-pair median above.

### Sparse GPU experiments and bottleneck profiling

`VIDEO_STRIP_GPU_SLICES=5` enables a separate CUDA kernel reading single-pixel vertical columns at the left edge, quarter-width, center, three-quarter-width and right edge. It averages those five values with the existing vertical area weights. `VIDEO_STRIP_GPU_SLICES=1` reads only the center column, selecting each output row's vertical midpoint with no averaging. Both sample decoded GPU memory and retain the small download/RGB conversion. The decoder still reconstructs the complete frame. Unset/`0` retains the original complete-frame GPU averaging kernel. Each sparse mode has its own Files preview cache namespace; restoring full-frame mode restores its existing cache. Restart Files to change the inherited environment. No personal config or decimation setting is changed.

Three rotating-order comparisons at 3,200 columns showed no meaningful end-to-end gain:

| Workload | Full GPU average | Five columns | Center column |
| --- | ---: | ---: | ---: |
| Local full 4K HEVC | 7.55 s | 7.47 s | 7.51 s |
| Local cropped 4K HEVC | 2.87 s | 2.85 s | 2.86 s |
| Local two-file 1080p H.264 | 6.42 s | 6.42 s | 6.42 s |
| Project Hail Mary on NAS | 10.76 s | 10.70 s | 10.69 s |

Sparse modes reduce the measured reduction/download/conversion work, but introduce larger color differences and miss unsampled content. They remain opt-in. The pre-experiment helper/source snapshot is retained at `/tmp/video-full-average-backup/`; synthetic checks compare full-frame output against that binary byte for byte. Independent CPU references verify both sparse kernels' sample coordinates, bit depth and vertical sampling from decoded YUV pixels, on 8-bit H.264 and 10-bit HEVC with one/four workers.

The helper's `VIDEO_STRIP_PROFILE=1` diagnostics now separate actual AVIO reads, container seeking, packet extraction, decoder flush/frame release, packet submission, draining and frame retrieval. Reads nested inside seeks/packet extraction are recorded separately and subtracted before reporting exclusive seek/demux costs. Worker opening/probing has a separate timer; the following shares describe active frame processing, excluding startup/indexing. These are **aggregate overlapping worker-time shares, not elapsed-time shares**:

| Indexed workload | Decoder calls | File reading | Seek bookkeeping |
| --- | ---: | ---: | ---: |
| Local full 4K HEVC | 94.0% | 0.2% | below 0.1% |
| NAS copy of the same 4K HEVC | 62.3% | 32.0% | below 0.1% |
| Local 1080p H.264 previews | 97.1% | 0.2% | below 0.2% |
| NAS 1080p H.264 previews | 47.9% | 49.5% | below 0.2% |
| Local 1080p HEVC CPU previews | 96.6% | 0.4% | below 0.1% |
| NAS 1080p HEVC CPU previews | 81.0% | 16.7% | below 0.1% |
| Project Hail Mary on NAS | 47.0% | 48.8% | below 0.2% |

For local 4K, medians of two runs were 27.29 decoder worker-seconds, 0.058 reading and 0.011 exclusive seeking, with 7.52 s elapsed. On its NAS original, 18.28 decoder worker-seconds and 9.39 reading produced 7.61 s elapsed: much of the read latency overlaps other workers' decoding. Project Hail Mary measured about 19.00 decoder worker-seconds, 19.69 reading, 0.064 exclusive seeking and 1.62 reduction, with approximately 11 s elapsed. Its worker AVIO callbacks delivered about 840 MB; this is logical delivered data, not a network-traffic measurement. Root probing/indexing is outside these worker totals.

Decoder-call timings include CPU parsing, NVIDIA decoder scheduling/waits, frame mapping and internal GPU copies; they do not isolate pure NVDEC execution. The installed FFmpeg n9.0.2 [NVDEC implementation](https://raw.githubusercontent.com/FFmpeg/FFmpeg/n9.0.2/libavcodec/nvdec.c) maps/retrieves decoded surfaces and makes frames writable inside the codec path. An optional `VIDEO_STRIP_PROFILE_SYNC=1` diagnostic stream barrier after frame retrieval checks for pending work before averaging. On local/NAS 4K and Project Hail Mary it attributed roughly 0.52–0.56 worker-seconds to readiness, mostly shifting time from the following reduction call, while elapsed time remained consistent. It is off by default because it changes synchronization and shares a stream across workers.

The short ordinary-frame Timelens path was checked separately on the 219-second AMV fixture: optimized helper wall times were 3.20–3.34 s local and 3.07–3.08 s on NAS. A quiet `perf` CPU-clock run attributed 86.98% of samples to libavcodec. `strace -f -c` recorded combined read/lseek time below 10 ms on both local and NAS inputs. These trace/profile runs were separate from baseline timings; NAS cache state is uncontrolled. Short previews therefore also show codec work dominating in the measured case, without treating CPU sample percentages as wall-time percentages.

Reproduce with:

```sh
python3 scripts/video_gpu_average_check.py --sparse-modes
python3 scripts/video_gpu_average_check.py --sparse-modes --synthetic --previous-helper /tmp/video-full-average-backup/video-strip
VIDEO_STRIP_PROFILE=1 VIDEO_STRIP_GPU_SLICES=0 python3 scripts/video_workload_benchmark.py /home/vega/Videos/video-preview-tests-20261008/manifest.json --folders single-wide-4k h264-batch hevc-batch --storage both --rounds 2
```

Profiling confirms that decode/decoder internals dominate local previews; reading is a substantial second cost, sometimes equal to decoder time, on NAS. Container seek bookkeeping itself is small for these indexed files. Sparse sampling remains available for future experiments but the default retains full-frame preview quality.

Final verification passes the release Files build, 8-bit/10-bit sparse pixel references with one/four workers, byte-identical full-frame restoration against the saved helper, and 128-step temporal checks at every tested width in both sparse modes. The refactored three-mode benchmark executes successfully. Formatting, shell/Python syntax and whitespace checks pass. Architecture comparison against `pre-five-slices` adds no function above the complexity threshold, leaves maximum complexity/clone/cycle counts unchanged and reduces measured duplication slightly; its final remaining flag is the config clone already in that baseline.

A follow-up at 800 columns, corresponding to 4× division of a 3,200-column request, also showed negligible sparse gains: three rotating-order medians were 2.114 s full-frame, 2.093 s five-slice and 2.109 s center-only. The first invocation was slower than subsequent repeats; a single first-mode comparison can overstate a sampling improvement. Application config remained unchanged throughout.


### Progressive video timelines (2026-10-08)
Long indexed timelines now expose a full-width preview after eight distributed anchors per decoder, then refine it roughly four times per second. Each decoder remains open and skips its anchors during the dense pass, so the selected keyframe count and final pixels are unchanged. Pending positions repeat the nearest available column; the Files row displays “Refining…” until completion. Small sample sets that finish in the anchor pass use ordinary final delivery. Short Timelens timelines publish as ordinary frames arrive, retaining their existing temporal sampling. Timelens’ long fallback publishes completed worker chunks; the FFmpeg AV1 streaming fallback has no snapshot protocol.
Snapshot pixels are copied under a short mutex and published outside it by atomic rename. The existing cancellable command worker polls at 50 ms, decodes bounded PPM snapshots, and sends through a one-image channel. The UI updates its existing texture; completion remains authoritative, partial images are not cached, and dropping the visible row retires its progress channel and kills/reaps its read-only helper.
Five rotating-order repetitions per case compared the saved pre-change helpers against progressive generation on local fixtures. Measured elapsed time includes helper startup and simulated 50 ms snapshot polling/RGBA conversion, but excludes application metadata probing, queue admission and UI painting. Builds, encoding and other performance workloads were not running during timings. Every final file was byte-identical to the previous helper.
| Local fixture | Columns | Previous final | Progressive final | First preview | Elapsed overhead |
| --- | ---: | ---: | ---: | ---: | ---: |
| single-wide-4k | 800 | 2.105 s | 2.110 s | 0.352 s | +0.24% |
| single-wide-4k | 3200 | 7.566 s | 7.566 s | 0.353 s | +0.01% |
| single-4k | 800 | 1.854 s | 1.858 s | 0.352 s | +0.23% |
| single-4k | 3200 | 2.856 s | 2.874 s | 0.353 s | +0.63% |
| hevc-batch | 800 | 2.006 s | 1.961 s | 0.452 s | -2.25% |
| hevc-batch | 3200 | 1.958 s | 1.978 s | 0.455 s | +1.04% |
| h264-batch | 800 | 1.904 s | 1.907 s | 0.301 s | +0.15% |
| h264-batch | 3200 | 3.507 s | 3.482 s | 0.303 s | -0.74% |
| short-batch | 800 | 3.107 s | 3.313 s | 0.101 s | +6.63% |
| short-batch | 3200 | 3.157 s | 3.282 s | 0.102 s | +3.94% |

All measured medians stay below the requested 10% elapsed-overhead ceiling. On the full 4K fixture, helper CPU medians were 0.806/0.807 s previous/progressive at 800 columns and 1.531/1.524 s at 3,200 columns. CPU time alone excludes GPU execution/waits; elapsed completion is the acceptance metric. These measurements establish the budget on the tested workloads rather than guarantee timing on all hardware/media.

Reproduce with:

```sh
python3 scripts/video_progressive_check.py /path/to/local/video.mkv --rounds 5 --widths 800,3200
python3 scripts/video_timeline_resolution_check.py --gpu --progressive
cargo test --release -p little-clip-core process::tests --offline
cargo test --release -p clip-workbench --lib video_timeline --offline
```


Validation: release Files build passes; three core process tests and five
video-timeline tests pass, including pending texture replacement/retirement.
Synthetic ordinary/indexed CPU/CUDA strips retain all 128 luminance steps at
all tested widths, and progressive/non-progressive final bytes match. Partial
indexed strips retain endpoints and monotonic order without gaps. Private
Weston/Niri acceptance passes at 1× and 16× decimation, including a helper held
before completion to prove Files paints snapshots while its task is pending;
screenshots show the Refining label. Normal final painting also passes.
Formatting, Python/shell syntax and whitespace checks pass.

Architecture comparison with `pre-progressive-video` leaves maximum complexity
and dependency cycles unchanged and reduces duplication from 7.54% to 7.52%.
The one additional clone is the identical parameter lists of `generate` and
`generate_indexed` after adding their progress sender, confirmed with jscpd's
exact fragment; it is not duplicate implementation logic. Keep the explicit
parameters rather than introduce a context abstraction solely to hide this
metric. The other clone flag is the config clone already present in the baseline.
