# Phase 4: Rich previews and built-in editors

Status: in progress after Phase 3 commit `278091e`. This checkpoint records the current implementation and its verification; Phase 4 is not complete.

## Opening and returning

Double-click a file, press Enter with one file selected, or choose **Open in Files** from its context menu. Directories keep their existing navigation behavior. Files owns one active studio or cancellable opening request at a time. Closing the studio returns to the existing folder, selection, history, and scroll position; a background refresh discovers saved outputs. Accepted browser operations continue to receive completions while a studio is open.

The workbench's image, audio, text, PDF, MIDI, spreadsheet, model and Gaussian scene modules now belong to its shared library. Both executables use those implementations. The workbench keeps its existing viewer ownership and startup queue. File loading and parsing use the bounded executor. PDF raster lifetime/decoding, image Arc revisions, audio replacement analysis, and scene coalescing/bake cleanup retain their established owners.

Unsupported files use the desktop's default application. **Open externally** is also available in file context menus and in studios. Paths are passed as process arguments, not shell commands. External applications own their own lifetimes; opening one does not discard the current studio.

## Listing previews

Visible rows request the same bounded image/text/file/audio previews used by the Shelf. Images replace their type icon with a thumbnail. Hover the preview icon for a larger thumbnail, text excerpt, two-second audio waveform excerpt, or PDF page count/title. PDF page rendering stays in the full PDF studio; listing previews do not create competing viewer sessions.

Each browser cache retains at most 128 visible file identities. Leaving the visible range retires cached data and cancels obsolete work. Identity includes the path and directory-entry revision; a source change invalidates that preview. One preview job runs at a time per browser cache, independently from the Shelf's existing preview job. Completions for retired identities are rejected. Initial requests schedule repaints immediately.

Existing preview bounds remain: 128×80 thumbnails, 8 MiB image reads, 2 KiB text excerpts, 128 MiB raster allocation, and 16,384-pixel raster dimensions. PDF metadata uses the existing bounded subprocess runner. These limits do not restrict ordinary file copy/move operations.

## Capability matrix

All types retain ordinary listing and file operations. Actual support depends on the installed decoder and existing studio backends.

| Type | Listing preview | Built-in inspection | Editing | Output |
| --- | --- | --- | --- | --- |
| Raster images / SVG | Thumbnail within preview limits | Existing Image Studio | Existing tools and overlays | Save/Save As; guarded atomic source replacement |
| UTF-8 text / code / Markdown | Excerpt | Existing syntax/Markdown preview | Small plain-text editor, up to 2 MiB | Save source or Save As beside source |
| Other text / large text | Excerpt where decodable | Existing bounded text preview | External editor | External workflow |
| Audio | Waveform excerpt | Existing Audio Studio | Existing channel/region tools | Staged exports beside source, no replacement of existing files |
| PDF | Page count/title and type icon | Existing pages, ToC, find and OCR studio | Existing page and OCR operations | New protected sibling PDF; source remains intact |
| Spreadsheet / CSV | Text excerpt or type icon | Existing spreadsheet viewer | Existing viewer capabilities | Existing viewer/export workflow |
| MIDI | Generic file icon | Existing MIDI viewer | Existing region selection/tools | Existing MIDI export workflow |
| 3D models / Gaussian scenes | Generic file icon | Existing model/scene studios | Existing studio capabilities | Existing studio workflow |
| Unsupported / video | Generic icon | External application | External application | External application |

Listing thumbnails do not imply editor/export support for that format. Rich video playback, gallery navigation, persistent music queues and broader media browsing remain Phase 5.

## Save and close contracts

### Text and image

File-backed sessions record the source's resolved path, device/inode, length, mtime and ctime, including nanoseconds, plus permissions. Text/image writes prepare a temporary file in the destination directory, flush it, and install the completed result atomically. Replacing the source requires that its recorded identity/timestamps still match. A changed, removed or substituted source is rejected; save to a new filename or reopen it instead. Save As destinations must be unused, including when an image dialog offers its ordinary overwrite confirmation.

Text opens in Preview mode; **Edit text** exposes the small editor. Save and Ctrl+S replace the tracked source after the check. Save As requests a single unused filename in the current source directory. Successful saves update the tracked source and regenerate the inspection preview in the background. Preview shows the last saved version while the editor retains an unsaved draft. Non-UTF-8 files and files larger than 2 MiB remain inspection-only.

Image dialogs retain the shared source-directory defaults and `_edit`/`_converted` naming. Successful saves update source tracking and the saved edit revision. A full image opening reads at most 64 MiB of encoded bytes; raster decoding retains the image library's default 512 MiB allocation limit. Spreadsheet/MIDI files have a 32 MiB opening limit. Oversized files can be opened externally.

Atomic replacement preserves permission bits but creates a new inode: other hard links keep the old bytes. Extended attributes and ACLs are not copied. The final comparison and rename are separate filesystem operations; this is external-change detection, not a lock or transaction against arbitrary concurrent writers. Files opened through a symlink can be inspected; saving directly through the alias is refused. Save As remains available. Cancellation cannot promise rollback after publication.

### Audio and PDF

Audio exports use the existing source-directory save dialog and naming controls. In Files, ffmpeg writes staged outputs and publishes them without replacing existing destinations. Source identity is checked before export and before each publication. Partial success is reported by the existing studio. Temporary channel/region edit settings remain session-owned; closing asks before discarding them, even after exporting selected regions. Source audio is never replaced by this path.

PDF page edits and OCR retain their existing sibling names, page-count validation, and viewer-owned staging directories. Files publishes by creating a new hard link to a completed staged PDF in the same filesystem; an existing sibling is refused. Successful output becomes the studio's current document. Page edits check their source before/after the worker operation. PDF transformations already publish their results, so a successful PDF edit does not represent an unsaved draft.

Back to folder and window close ask before discarding text/image or temporary audio edits. Accepted text/image saves, audio exports, PDF page edits and OCR must finish before closing. Keep editing returns to the studio; Discard edits returns to the browser. This prompt does not claim to undo outputs already created. Studio Escape handling retains its own modal/selection behavior.

## Worker entry points

Barcode scanning and isolated image conversion/decode relaunch the current executable. Both binaries now call the shared internal-worker dispatcher before ordinary CLI parsing. Worker paths use OS strings so unusual filenames do not require UTF-8. Existing worker exit/error-file conventions are preserved.

OCR and PDF document helpers resolve their bundled script path independently of the launch directory, retaining the relative-path fallback when the build's source tree is unavailable. They use the existing installed Python environment and Tesseract/PyMuPDF dependencies.

## Acceptance cases

Acceptance cases were checked using the private Weston/Niri runner and the shared studios' tests:

1. Open an image, edit/save a new sibling, return and retain the original folder/selection/scroll. Check source replacement and save revision tracking.
2. Change an opened image/text externally, then attempt Save; preserve external bytes and allow a new filename.
3. Edit UTF-8 text, cancel a close, save, inspect the refreshed preview, and close. Cover unusual filenames, Save As collisions, non-UTF-8 text and the editor limit.
4. Browse/scroll a mixed folder with images, audio and PDFs; check thumbnail/waveform/page-count hover previews, bounded retention and external-change invalidation.
5. Export audio beside its source; reject an existing output, report a partial batch, and defer close during export.
6. Edit/OCR a PDF; protect source and existing sibling outputs, retain raster cleanup and reject close during writes.
7. Open spreadsheet, MIDI, model and scene files through their shared studios. Check playback/render cleanup after close.
8. Run barcode and conversion tools from Files; check that helpers do not launch another Files window.
9. Open unsupported files externally and inspect Light/Dark layouts at narrow and wide window sizes. Check the independent workbench's popup/ordered viewer behavior after sharing modules.

| Case | Evidence |
| --- | --- |
| 1–3: image/text inspection and writes | Full Light/Dark UI runs reproduce edited text and flipped image pixels, refreshed previews, Keep/Discard prompts and a refused external text change. Source/image/text tests cover source replacement, saved revisions, external image changes, collisions, symlinks, permissions, unusual filenames and inspection-only limits. |
| 4: listing previews and bounds | Private hover captures show the image, waveform and three-page PDF count. Preview tests cover raster/text limits, pending retirement, changed-entry revisions and releasing cached data; the virtualized listing caps requested visible identities at 128. |
| 5: audio exports | Real ffmpeg tests verify output creation, existing-file preservation, partial success and changed-source rejection. Existing audio save/lifecycle tests and the Files close guard cover accepted writes and stopping playback. |
| 6: PDF editing/OCR | A private rotation creates a three-page sibling with 90-degree page rotation while preserving original bytes. Private OCR creates a searchable three-page sibling, retains native text, verifies its embedded source SHA-256 and returns to the browser. Protected-publication and existing PDF edit/OCR/lifetime tests cover collisions, deferred close and raster cleanup. |
| 7: other studios | Full UI runs open spreadsheet, MIDI, STL and SPLAT fixtures and return to the browser; SPLAT renders a Gaussian. Shared lifecycle/worker tests cover playback stop, cancellation, mesh fallback and viewer-owned resources. |
| 8: internal helpers | Both Files worker flags succeed without a display, with decoded pixels/dimensions and barcode JSON validated, including Unicode input paths. |
| 9: external opening and application independence | Full Light/Dark runs validate the exact path received by a synthetic desktop opener and compact browser layouts. A compact image-studio capture retains Back/Open externally and source-path hover access. The independent workbench runner verifies fixed geometry, singleton focus and viewer return; ordered-opening tests pass. |

Passing full UI reports: `/tmp/files-phase4-dark-q/report.json` and `/tmp/files-phase4-light-d/report.json`. Supplemental artifacts: `/tmp/files-phase4-hover-final`, `/tmp/files-phase4-narrow-viewer`, `/tmp/files-phase4-pdf-edits/03-rotated.png`, and `/tmp/files-phase4-pdf-ocr-final/report.json`. The rotation's supplemental collision-toast OCR check did not pass; filesystem publication safety is proven by the targeted protected-output test, and the rotation artifact itself was validated with `pdfinfo` and an exact source-byte comparison.

## Recorded checks

`cargo check -p clip-workbench --bins --offline`, `./launch.sh build --bin clip-file-manager`, `cargo fmt --all --check`, and `git diff --check` pass.

`cargo test --workspace --offline -- --test-threads=1` passes: 1,877 tests, zero failures, 22 existing ignored tests (GPU/manual integration checks and benchmarks). The shared library passes 701 tests and the workbench binary passes 809. Serial execution avoids contention between existing tests using the global bounded executor. Added coverage exercises source revisions, permissions/hard links, external changes, destination collisions, partial audio publication, protected PDF outputs, text limits/unusual names, image saved-state tracking, preview bounds/retirement/revisions, cancelled viewer openings and helper-script resolution. The workbench test target compiles the same studio source modules locally to retain access to its existing private test fixtures; production binaries share the library modules.

The private Weston/Niri workbench run at `/tmp/files-phase4-workbench-return/report.json` passes fixed geometry, single-instance focus, shared image opening and return to the same popup. Existing viewer lifecycle tests cover ordered asynchronous/startup openings. Both Files helper entry points also completed without a display: encoded PNG decoding reproduced the expected dimensions/pixels and barcode scanning produced valid JSON. Unicode worker paths were included.

Against `pre-file-manager-phase4` at `278091e`, duplication falls from 8.06% to 7.98%, and clone groups fall from 1,298 to 1,297. There are no new complexity threshold violations or dependency cycles. Loading, image-byte publication and audio-region export were separated into their existing responsibilities to address initial complexity findings. The automated comparison still labels existing clones in touched audio/studio files as regressions; normalized fragment comparison against the baseline finds zero new fragments. The shared worker dispatcher also removes repeated executable argument handling. A completion-order race uncovered by verification in the existing Audio Studio Digital Decode tool was fixed by observing job completion before draining its event channel.
