# Phase 3: Shelf and clipboard integration

Status: implemented after Phase 2 commit `9c0e028`. Compilation and source review are complete; runtime acceptance is pending. Phase 3 changes remain uncommitted.

## Behavior

The Shelf is a full-width bottom dock, visible by default, with two compact rows of preview cards styled after the screen mirror. The top row shows shared hands/inventory, with Left and Right Hand first; the second row contains deliberately collected groups. Both rows scroll horizontally when needed. Selecting a card exposes its placement, intent, naming, and send/remove actions in the Shelf header. The **Shelf** button toggles the dock. Groups survive navigation within this Files session; closing Files clears them.

| Source | Shelf representation | Placement |
| --- | --- | --- |
| Selected files, Copy/Cut, or desktop file URIs | Paths with Copy/Move intent | Existing Phase 2 transfer queue and conflicts |
| Desktop text | Captured UTF-8 bytes | Named text file |
| Desktop encoded image | Captured image bytes and MIME | Named image file in its offered format |
| Supported document/data MIME | Captured bytes with an explicit extension | Named document/data file |
| Shared file/media/directory slots | References | Copy by default; choose Move explicitly |
| Shared text, URL, color, or Markdown slots | Captured text | `.txt` or `.md` |
| Shared image slot | Captured image content, read only for the collected slot | Named image file |

**Add selected files** gathers a file group without changing the desktop clipboard. **Capture clipboard** retains the currently offered payload without creating a file immediately. Both actions remain visible at the start of the Collected row, with hints explaining their behavior and tooltips explaining repeated use. Identical collected file groups with the same intent are deduplicated. Ordinary copy/cut/paste does not add collected groups. There is no separate local Held card; the hand row reflects daemon-owned slots.

File groups have explicit Copy/Move buttons and a **Copy here** or **Move here** action. Copies retain the group. Successful moves consume completed references from groups marked Move; failed/unprocessed paths remain. References use the current files on disk, and changes to those files are visible to later operations. **Check references** reports unavailable paths off the UI thread; a successful check is an observation, not a guarantee for a later placement. Transfer reports retain actual filesystem failures.

Collected content retains its bytes even after another application changes the clipboard. Content is represented separately from file references. A copied `text/uri-list` image remains a file copy; an `image/png` payload creates a new image file. Plain text that happens to look like a path stays text. Invalid file-reference offers are refused rather than reinterpreted as text. A browser can offer a web URI alongside encoded image/PDF bytes; those offered bytes can be captured without fetching the URI. GNOME file-operation payloads still require valid local references.

## Creating files from clipboard content

Ctrl+V with file-list focus, the Paste button, and the context Paste action share one asynchronous importer. File references keep Phase 2 behavior. Content opens **Create from clipboard**, showing its captured MIME, byte count, destination, and editable default filename such as `Clipboard.txt` or `Clipboard.png`.

The destination is captured when Paste begins. Later navigation does not redirect it. Enter creates the file; Escape cancels creation. Content created from a collected card remains in that collection after cancelling or failing creation. To retain desktop clipboard content beyond a cancelled ordinary paste, capture it explicitly in the Collected row. A full collected Shelf rejects collection visibly; ordinary Paste can still create a file.

Existing filenames are preserved. A collision returns to the naming dialog with the same captured content so the user can choose another name. Content creation stages a complete file on the destination filesystem, syncs it, and installs it through the existing atomic no-overwrite rename. It shares naming/transfer serialization and preserves accepted completion results. No partially written final file is exposed. Cancelling the dialog has no filesystem effect; an accepted write finishes before Files closes.

The default extension describes the stored bytes. Changing the filename does not convert the encoding. Image bytes are preserved in their offered format, without decoding and re-encoding pixels. Raster headers are checked through core's shared image reader. Text is required to be UTF-8.

### Explicit encoders

| MIME | Default extension |
| --- | --- |
| UTF-8 plain text / `UTF8_STRING` | `txt` |
| `text/markdown` | `md` |
| `text/csv` | `csv` |
| `text/html` | `html` |
| `application/json` | `json` |
| XML text | `xml` |
| `application/pdf` | `pdf` |
| PNG, JPEG, WebP, GIF, SVG | Core's MIME-to-extension mapping |

Valid local file-reference offers have priority. Otherwise, encoded images are preferred, followed by PDF, plain UTF-8 text, and the remaining explicit text/data formats. For example, a browser offering HTML and plain text produces plain text. Remote URI-only and unsupported MIME offers are rejected. This phase does not fetch remote URLs, infer arbitrary binary encodings, or guarantee that externally supplied document bytes are semantically valid.

## Shared hand and inventory integration

Visible shared cards refresh lightweight slot summaries every half-second while visible through the existing daemon IPC; More → Refresh hands and previews provides a manual refresh. Double-clicking a hand card, or choosing Collect hand item, retrieves the selected slot, checks its item identity against the displayed summary, and converts its actual payload to a reference group or captured content. Changed/empty slots require another refresh. An image slot captures its image bytes, including reading its staged path when IPC omits in-memory bytes.

**Send to Left** and **Send to Right** use existing text/image requests and the new `PushFileReferences` request. The latter always builds a reference batch, including for one image or media file. Existing `PushFiles` callers retain their prior content classification behavior. File sends use Copy intent in the shared hand/system clipboard; Shelf Move intent is local to an explicit placement. Sending content does not remove the Shelf group. Unsupported shared payload types, such as PDF bytes, can be placed as files first.

The synchronous IPC transport is shared by the file manager and workbench; socket-path resolution is shared with the daemon and CLI through core. All Shelf requests run on the bounded executor. The daemon is optional for the local Shelf and desktop clipboard. An unavailable daemon produces a visible connection error. A lost send response requires checking the shared hand before retrying; sends are never retried automatically.

Rebuild/restart the daemon to support `PushFileReferences`. An older daemon cannot accept that new request. Inventory refresh and the existing text/image requests retain their established protocol forms.

## Bounds and lifecycle

- At most 32 collected groups, 10,000 collected file references, and 128 MiB of collected content.
- At most 64 MiB per captured content payload; file URI imports retain Phase 2's 1 MiB limit.
- `wl-paste` calls retain five-second command deadlines and reject truncated output.
- One collection/send request and one independent inventory summary request at a time. Reads have a 15-second executor deadline; IPC reads/writes have two-second socket timeouts.
- Shared sends enforce the daemon's 1 MiB serialized request limit before connecting. Larger content can still be created as a local file. Slot response import is capped at 96 MiB. Paths that the existing JSON IPC cannot serialize remain usable in local file groups but cannot be sent to shared hands.
- Captured bytes use `Arc` across Shelf entries, naming dialogs, and writes. File groups retain paths without reading every file's contents.
- Read-only Shelf work does not prevent closing. Pending sends and accepted filesystem writes defer closing so their completion is accounted for.
- There is no persistent Shelf database or cross-process Shelf synchronization. Shared hand/inventory slots remain daemon-owned and are refreshed through lightweight summaries while the Shelf is visible.

## Card previews

Cards use the shared neutral surfaces and mirror-style borders, with compact headers, previews, titles, and metadata. Ordinary image references and captured images show thumbnails; text shows a short excerpt; folders/groups show entry/file names; audio references show a two-second waveform excerpt where supported. Shared audio uses its existing waveform, and shared animation uses its existing first thumbnail frame. Unsupported types retain a type icon and metadata. Rich PDF page/scene previews remain in the later viewer phase.

Only visible cards request previews. One cancellable preview job runs at a time on the bounded executor. Replaced/removed card completions are rejected; refreshed slot readiness invalidates the affected preview. Cached thumbnails are at most 128×80 pixels. Image file reads are capped at 8 MiB, text reads at 2 KiB, raster allocation at 128 MiB, and raster dimensions at 16,384 pixels. Shared preview responses are capped at 16 MiB and are reduced to card data rather than retained slot content. These preview limits do not restrict ordinary file placement.

The shared `ImageThumbnail::generate_with_limits` mechanism reuses core's decoder hooks, MIME hints, thumbnail sizing, and premultiplied output. Refresh hands and previews, or Check file references, refreshes cached previews of paths changed externally.

## Acceptance cases

Runtime checks remain to be performed with the private Weston/Niri runner:

1. Paste UTF-8 text into a folder, edit the filename, and check exact bytes; keep text containing paths as text.
2. Paste an encoded image and check dimensions/bytes and extension; copy the same image through a file URI and retain reference behavior.
3. Capture content explicitly, cancel naming from that collected card, then create from the retained Shelf content after replacing the desktop clipboard.
4. Encounter an existing file and choose another name without changing the original.
5. Collect separate groups across directories, copy one, move another, and check consumption and partial-failure retention.
6. Check the bottom two-row layout, card selection, image/text/waveform previews, that only shared hand cards appear above collections, and that both collection actions remain visible and reusable after adding groups. Check that status reports stay within their panel and row labels never wrap. Remove a referenced file externally and display its unavailable status while keeping other paths usable.
7. Refresh shared inventory, collect each supported payload category, and send references/text/images to both hands without converting single-image references to content.
8. Exercise an absent/older daemon, invalid MIME/URI payloads, content/Shelf limits, and rejected queue admission.
9. Check list versus text-field clipboard shortcuts, modal focus, Light/Dark layout, and workbench singleton behavior after sharing the IPC transport.

## Checks recorded

The file manager, workbench, daemon, and CLI compile with `cargo build -p clip-workbench -p little-clipd -p clipctl --bins --offline`. `cargo fmt --all --check` and `git diff --check` also pass. No Phase 3 tests or UI acceptance runs have been added or run.

Architecture measurements use baseline `pre-file-manager-phase3` at `9c0e028`. Initial coordinator complexity findings were addressed by separating docked layout, collection controls, entry display, and inventory display. The final comparison has no new cyclomatic-complexity threshold violations or cycles, and duplication falls from 8.14% to 8.10% while clone groups fall from 1,300 to 1,298. Its automated REGRESSION classification flags existing clone fragments in touched IPC/daemon/CLI files; fragment comparison against the baseline found zero new clone fragments. The shared socket helper removes previously repeated transport path logic.

The bottom-Shelf redesign also compiles with `cargo build -p clip-workbench --bins --offline` and `./launch.sh build --bin clip-file-manager`. Formatting and diff checks pass. Against `pre-shelf-bottom-redesign`, duplication falls from 8.10% to 8.05%, clone groups remain at 1,298, and there are no new complexity threshold violations or dependency cycles. The automated comparison flags existing fragments in touched files; normalized fragment comparison against that baseline finds zero new fragments. Preview rendering and the two-row layout still need runtime acceptance.

The user accepted this implementation as the initial Phase 3 on 2026-10-05 and authorized proceeding to Phase 4. Automated runtime acceptance remains pending, and further Shelf polish is follow-up work. The overall roadmap is in [plan.md](plan.md).

## Shelf feedback corrections

The status report scroll area is bounded by its actual panel height, with clipping at the panel boundary; completed reports receive enough height for the folder line and report. Shelf header actions scroll horizontally rather than wrapping into the cards. Row labels reserve 90 points and disable wrapping. Left and Right Hand take preview priority over prior inventory slots. Visible summaries poll every 500 ms on an independent request, and pending card previews request repaints every 50 ms immediately after admission. There is no additional locally held Shelf payload. Collection actions remain visible after groups are added.

The feedback corrections compile through `./launch.sh build --bin clip-file-manager`; formatting and diff checks pass. Architecture comparison against `pre-shelf-feedback` introduces no new complexity threshold violations or dependency cycles. Its flagged clones are pre-existing fragments in touched core/IPC/CLI files. Runtime visual acceptance and observed refresh timings remain pending.
