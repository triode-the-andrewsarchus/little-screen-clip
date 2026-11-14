# Phase 4 — rich previews and built-in editors

Status: complete after the 2026-11-14 scope and acceptance audit. The original implementation checkpoint is
preserved in [phase4_initial_checkpoint.md](phase4_initial_checkpoint.md).
Its old capability table, crate ownership, preview limits and toolbar descriptions
are historical; they do not describe the present application.

The milestone is a safe shared opening/preview/editing framework for supported
formats. Adding every format in `docs/file_types.md`, perfecting every editor and
adding later navigation views are ongoing work, not requirements to keep Phase 4
open indefinitely. Listing, thumbnail, full viewing, editing and export remain
separate capabilities; a rendered preview does not imply editing support.

## Current behavior and owners

`clip-file-manager` owns browser navigation, source-aware opening, collection
sessions and return to the folder. `clip-studios` owns shared desktop studios;
`clip-ui` owns shared widgets/fonts/vector rendering. Neither the standalone file
manager nor neutral studios depend on the workbench application. Unsupported or
preferred external workflows retain explicit desktop handoff.

Files owns one active viewer/loading request. Entering a studio preserves browser
history, selection and scroll; browser operations continue ticking. Cancelling an
opening rejects late completion. A source-page/line/cell/chapter/region selected
in a supported strip is retained through asynchronous loading.

List rows reuse bounded visible preview owners for images, text/Markdown and
rendered diagrams, audio waveforms/spectra, MIDI, video timelines, PDF/notebook
pages, books, spreadsheets/databases and other supported inspectors. Work is
retired when hidden, replaced or no longer relevant; identity/revision checks
reject stale content. Photo metadata and thumbnails share core gallery owners.
Individual decoders have their own documented budgets rather than one universal
encoded-byte limit. Container previews are being added separately in Phase 7.

| Family | Browser preview and built-in opening | Editing/output contract |
|---|---|---|
| Raster images/SVG and supported illustration imports | Shared photo/image preview and studio; source collections retain per-photo unsaved edits/history | Supported image tools; guarded save/export, independent Save As and batch edit export |
| Text/code/Markdown/diagrams | Text minimap or rendered strip/hover; shared text/diagram studio | Editable supported source, guarded Save/Save As; graphical tools where implemented, not universal graphical editing |
| Audio | Shared waveform/spectrum, range interaction and Audio Studio | Session edits and staged exports; source and existing siblings remain protected |
| PDF/office documents | Page strips, selected-page opening and shared paged studio | Supported PDF operations/OCR publish protected outputs; imported/read-only sources do not inherit PDF mutation controls |
| Handwritten notebooks | Native page artwork for qualified Supernote, Xournal and reMarkable sources | Read-only; compatibility limits are recorded in `HANDWRITTEN_NOTEBOOK_PLAN.md` |
| MIDI/multisamples | Piano-roll or playable mapping strips and shared inspectors | Existing region/synthesis/export capabilities; preview is not full sampler compatibility |
| Spreadsheets/databases/books and structured records | Registered content-aware strip/inspector | Only the operations actually offered by that inspector; no generic write capability inferred |
| Models/Gaussian scenes | Shared scene previews and studios | Existing studio transformations/exports, subject to backend capability |
| Video | Timelines and embedded playback | Track/speed/subtitle/loop controls; extracted stereo audio can open in Audio Studio |
| Unsupported content | Ordinary listing, file operations and external handoff | External application owns its session |

## Save and close contracts

File-backed text/image edits retain `FileSource` identity and use guarded atomic
publication. Save As uses an unused destination. Output-directory handles remain
owned through staging and installation; source/path replacement and collisions
are errors. Replacing an ordinary file changes its inode, so other hard links
retain their old contents. Do not infer ACL/xattr preservation from mode-bit
preservation.

Audio/MIDI/PDF exports retain their existing pinned input/output and publication
owners. Accepted writes survive read-style cancellation and defer application
close until their completion is accounted for. Unsaved text/image/audio sessions
prompt before discard; retained photo edits participate too. Successful exports
do not imply all other session edits were saved or discarded.

Studio file naming is shared source-aware behavior. Return-to-folder controls live
in the main menu bar; studio naming/title controls do not require an extra browser
navigation dock. File context menus retain external opening; the studios do not
need duplicate generic external-open buttons alongside application-specific ones.

Both desktop executables dispatch shared internal workers before ordinary CLI
parsing. Bundled helper resolution is independent of the launch directory.

## Requirement-by-requirement audit

| Original Phase 4 requirement/gate | Current source and direct evidence |
|---|---|
| Reuse existing viewers and type detection | `viewers::{kind,load,session}`, core format registry and `clip-studios`; current tests qualify native diagram extensions, ambiguous document hints, fonts, office and image imports, and unsupported external routing |
| Bounded visible preview jobs, cancellation and stale rejection | `previews`, `shelf/preview`, per-type strip owners and core cancellable task delivery; current preview/lifecycle tests, source replacement tests and selected-page tests pass |
| Images, audio, PDF and text previews, then incremental formats | `ui/list.rs` dispatches the shared owners; current mixed-row alignment tests and real both-theme notebook/diagram/media acceptance provide rendered artwork evidence rather than suffix-only dispatch |
| Separate format capabilities | Table above plus `docs/file_types.md` and format-specific compatibility docs; read-only notebook/office guards and unsupported routes are tested |
| Source-aware editors and source-directory outputs | Shared text/image save owners, audio/MIDI export owners and paged-viewer source/output owners; studio/file-source tests cover exact output, collisions and replaced parents |
| Modified prompts, safe replacement and external changes | `viewers/chrome.rs`, collection write/rename owners and core `FileSource`; current tests cover unsaved close, retained photo histories, existing destination preservation, changed sources and pending export snapshots |
| Return to same browser location/selection/scroll | `viewers` owns sessions separately from browser `Location`; host lifecycle tests and native child-opening/history runs verify opening and returning without constructing a second browser |
| External application opening remains available | File context actions and `external`/viewer fallback; unsupported routing test passes; native synthetic opener must receive the exact original argument |
| Completion gate: inspect/edit without context loss; bounded mixed-folder work | Covered by the owners/tests above and the native audit below; ongoing format additions do not expand this gate to every listed suffix |

## Current verification

At the initial Phase 7 checkpoint (`77bf6155`):

- File-manager library: 878 passed, six ignored, with four test threads.
- Shared studio library: 840 passed, twelve ignored, with four test threads.
- Core file-source tests: ten passed, zero failed.
- Strict file-manager library/test Clippy, leaf formatting and warning-free native
  build pass. The preceding merged checkpoint also passed workspace tests with
  four test threads, strict workspace Clippy and a warning-free workspace build.
- Current container native dark/light runs prove actual artwork, file opening,
  nested navigation/history and browser/studio ownership; all three notebook
  families have separate both-theme artwork/selected-page/read-only acceptance.

A refreshed Phase 4 private-compositor driver is
`scripts/file_manager_phase4_audit.py`. It checks current text save/refresh,
unsaved-close prompts, changed-source refusal, ordinary studio opening/return,
exact inert desktop handoff and narrow browser geometry. Both current-theme runs pass:
`/tmp/phase4-audit-dark-06/report.json` and
`/tmp/phase4-audit-light-03/report.json`. They use the initial container-preview
binary (`77bf6155`); subsequent inline-preview configuration does not change the
studio/opening/save owners audited here. The image case verifies more than
20,000 source-colored rendered pixels, not just a filename. Text save/reopen and
conflict cases verify exact disk bytes. The synthetic desktop opener verifies its
exact original path argument.

Early review runs exposed stale observer assumptions (listing-only coordinates
for editor text and full-window OCR omitting button labels). Screenshots showed
the actual controls and correct edited source. The shared native observer now
also reads focused studio-control crops. Those failing observations are retained,
not represented as application regressions or current passes. No outstanding
Phase 4 functional defect was found in this audit; the necessary changes were
current capability/ownership documentation and refreshed acceptance coverage.
