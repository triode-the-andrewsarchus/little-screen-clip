# Bugs From Full Architecture Review

Reviewed 2026-09-17. Findings below are concrete current-tree behavior problems, ordered approximately by impact. No code changes were made.

## Critical / high

### B1 — ReloadConfig reports success but leaves daemon behavior on the old configuration

- **Location:** `crates/little-clipd/src/main.rs:831-840`
- **Behavior:** `ReloadConfig` loads a new `AppConfig` and passes it only to `ScreenRenderer::set_config`. `DaemonContext.config` is never updated, while clipboard capture and `GetAutoActions` read that lock (`:597`, `:918`). The rolling-capture thread also retains the startup audio config.
- **Result:** a changed `auto_actions.trim_link_tracking` setting is displayed/reported from stale state and is not applied after reload; audio capture settings cannot be reloaded despite the IPC success message.
- **Needed coverage:** change a config value, issue `ReloadConfig`, then verify both rendering and a behavior consumer observe it.

### B2 — The binary-diff input extractor compares the wrong bytes for image and audio items

- **Location:** `crates/little-clip-ops/src/binary_diff.rs:144-226`
- **Behavior:** `ClipPayload::Image`, `AudioRef`, and `Animation` try `Path::new(&item.title)` rather than their payload path. Image fallback bytes are the premultiplied thumbnail, not original encoded image bytes; audio fallback is `subtitle` text.
- **Result:** binary diff can silently compare preview pixels, metadata text, or empty data while labeling it as a file comparison.
- **Needed coverage:** compare two file-backed image/audio items with titles that are not paths; assert the data exactly matches the source files.

### B3 — USB frame packetization emits an invalid zero-length chunk for exact payload multiples and accepts short writes

- **Location:** `crates/little-clip-driver/src/device.rs:245-299`
- **Behavior:** `num_chunks = total_size / CHUNK_DATA_SIZE + 1`; for a 496-byte multiple this adds a final chunk whose `data_len` is zero. `write_bulk` return values are not checked against the requested burst length.
- **Result:** exact-multiple JPEGs use a malformed/extra packet, and a partial USB transfer is treated as a complete frame, causing corruption or protocol desynchronization.
- **Needed coverage:** packet-builder tests for empty, one byte, exactly 496 bytes, 497 bytes, and short-write behavior (through a transport abstraction).

### B4 — Pipeline graph validation is bypassable through IPC YAML

- **Location:** `crates/little-clip-ops/src/pipeline/executor.rs:92-166`, `crates/little-clipd/src/main.rs:801-817`
- **Behavior:** topology only counts connections whose node IDs happen to exist. Execution chooses the first connection to an input pin, never verifies pin names/types, permits multiple sources for one input, and executes arbitrary deserialized node/pin/property definitions rather than validating against `NODE_CATALOG`.
- **Result:** malformed graphs can silently ignore dangling connections, route arbitrary first-match data, and run with incompatible values instead of returning a precise validation error. The canvas compatibility check is UI-only.
- **Needed coverage:** unknown node IDs, nonexistent pins, incompatible ports, duplicate wires to one input, dangling sources/sinks, and catalog/input-schema mismatch.

### B5 — Pipeline execution can block the async daemon indefinitely and reports failed file-batch saves as success

- **Location:** `crates/little-clipd/src/main.rs:801-814`; `crates/little-clip-ops/src/pipeline/executor.rs:951-985`, `:1080-1130`
- **Behavior:** `PipelineExecutor::execute()` invokes synchronous filesystem and external commands directly in an async IPC handler, with no timeout or cancellation. `SaveToDisk` ignores every `FileBatch` copy error then records only the destination directory as a successful save. `run_command_with_stdin` also discards write errors.
- **Result:** a hung `ffmpeg`, `bash`, `python`, `magick`, etc. can consume a Tokio worker indefinitely; users receive a successful pipeline response even when batch exports fail or input was only partially passed to a process.
- **Needed coverage:** stalled child process, stdin write failure, unwritable destination, and a batch with one/all failed copies.

### B6 — Image staging failures create apparently valid items that point to files that do not exist

- **Location:** `crates/little-clip-core/src/item.rs:326-376`, `:701-725`, `:823-855`; `crates/little-clip-core/src/staging.rs:71-89`
- **Behavior:** `from_image_bytes` and SVG constructors ignore directory/write failures, then set `path: Some(staging_path)`. `get_staging_dir` also ignores directory creation errors. Pipeline `PortValue::from_clip_item` repeats this pattern in another temp directory.
- **Result:** the item is accepted and shown as image content, but external editing, path-based exports, later reloads, and consumers needing a file fail unpredictably.
- **Needed coverage:** unavailable/not-writable cache directory and failed staging write must result in a surfaced error or an explicitly memory-only item with no path.

### B7 — Staging cleanup over-reports deletion and hides directory/I/O errors

- **Location:** `crates/little-clip-core/src/staging.rs:48-67`
- **Behavior:** `cleanup_old_items` increments its deletion count even when `remove_file` fails; it silently ignores `read_dir`, metadata, modification-time, and removal errors.
- **Result:** callers can believe stale files were reclaimed while disk use remains unchanged, and a stale directory or permission error is invisible.
- **Needed coverage:** undeletable file/directory and unreadable staging directory cases.

### B8 — Rolling-audio snapshots race and overwrite each other

- **Location:** `crates/little-clipd/src/main.rs:865-893`, `:1595-1605`
- **Behavior:** every `GetRollingAudio` request writes `current_snapshot.wav` under the same runtime path while holding a synchronous `std::sync::RwLock` in an async handler.
- **Result:** concurrent clients can receive a path whose contents were overwritten by another request; readers can observe a partially rewritten WAV. The blocking lock and file write also stall an async worker.
- **Needed coverage:** two simultaneous requests with read/verify of both returned snapshots.

## Medium

### B9 — Unknown automatic-action keys are acknowledged as changed

- **Location:** `crates/little-clipd/src/main.rs:926-934`
- **Behavior:** any key other than `trim_link_tracking` does nothing but returns `IpcResponse::Success`.
- **Result:** clients cannot distinguish unsupported configuration from a successfully applied action.
- **Needed coverage:** an unknown key must return an error; known key must be observable through `GetAutoActions`.

### B10 — IPC accepts/logs unbounded private request lines

- **Location:** `crates/little-clipd/src/main.rs:581-589`
- **Behavior:** `BufReader::lines()` has no application-level byte limit and the daemon logs the entire line at `info`. Requests may contain arbitrary clipboard text, image bytes, paths, and executable graph scripts.
- **Result:** a local client can force unbounded allocation/log volume; normal logs disclose clipboard contents and possibly base64 media/scripts.
- **Needed coverage:** oversized line rejection and assertions that logs contain only request kind/size, not payload.

### B11 — The network preview leaks cache entries and can spawn unlimited ping processes

- **Location:** `crates/little-clip-ops/src/network_preview.rs:74-125`
- **Behavior:** every cache miss spawns a thread/process; expired entries are never removed and concurrent calls for the same uncached IP do not coalesce.
- **Result:** repeated clipboard content with distinct IPs grows `PING_CACHE` forever and can create a large number of `ping` processes/threads.
- **Needed coverage:** bounded cache eviction and coalescing/concurrency-limit tests.

### B12 — PDF thumbnail generation turns page failures into apparently successful partial documents

- **Location:** `crates/little-clip-core/src/pdf.rs:126-139`
- **Behavior:** `render_all_pdf_page_thumbnails` drops all individual page-render errors and returns `Ok` with whichever pages happened to render.
- **Result:** callers cannot tell missing/failed pages from a genuinely shorter PDF; UI state can be incomplete without an error.
- **Needed coverage:** force one page render failure and assert either an error or an explicit per-page failure result.

### B13 — PDF preview leaks its temporary output when expected naming does not match

- **Location:** `crates/little-clip-core/src/pdf.rs:75-115`
- **Behavior:** when `pdftoppm` succeeds but the guessed `prefix-page.png` is not present (including zero-padded output), the function returns without removing generated files. The comment acknowledges the alternate naming but no glob/cleanup is implemented.
- **Result:** repeated preview failures accumulate files in the system temp directory.
- **Needed coverage:** alternate `pdftoppm` output naming and cleanup on every error branch.

### B14 — Invalid image bytes are accepted as image clipboard items

- **Location:** `crates/little-clip-core/src/item.rs:326-376`; daemon `PushImage` at `crates/little-clipd/src/main.rs:623-644`
- **Behavior:** `ClipItem::from_image_bytes` does only optional header dimension detection and always returns `Some`, even for arbitrary bytes or an unsupported MIME type.
- **Result:** `PushImage` reports success for non-images; later thumbnail, render, diff, and external-editor behavior fails elsewhere with less useful errors.
- **Needed coverage:** invalid PNG/JPEG/WebP/SVG bytes and MIME/content mismatch should be rejected or represented explicitly as opaque binary data.

### B15 — External-command path conversions can panic on valid Unix filenames

- **Location:** `crates/little-clip-ops/src/pipeline/executor.rs` (e.g. `:626`, `:644`, `:710`, `:735`, `:825`, `:850`, `:885`)
- **Behavior:** multiple command arguments call `Path::to_str().unwrap()`.
- **Result:** a valid non-UTF-8 filesystem path from clipboard/file manager panics the daemon process instead of yielding a recoverable error.
- **Needed coverage:** construct a non-UTF-8 Unix `PathBuf` and verify pipeline nodes return `PipelineError`.

### B16 — Several `ClipItem` file paths are not used consistently as the source of truth

- **Location:** `item.rs` file-backed payloads; `binary_diff.rs:144-226`; `diff.rs:360-430`; `pipeline/types.rs:119-141`
- **Behavior:** some consumers use payload paths, others reconstruct a path from display title/details or write bytes to new ad-hoc temp paths.
- **Result:** a renamed display label, missing staging file, or moved original can lead different features to operate on different data for the same slot.
- **Needed coverage:** create a file-backed item, mutate only its label, and confirm all consumers still use the payload path/bytes consistently.

## Low / correctness debt

### B17 — Operation implementation contains a dead duplicate match arm

- **Location:** `crates/little-clip-ops/src/lib.rs:797-814`
- **Behavior:** `get_item_text` matches `ClipPayload::Markdown(m)` twice.
- **Result:** one arm is unreachable, which is a sign the manual payload dispatch table can drift and complicates future changes.
- **Needed coverage:** compile under the restored strict Clippy gate; use a single shared textual-payload conversion.

### B18 — Non-hermetic tests can write to developer-local locations and silently skip validation

- **Location:** examples include `little-clip-core/src/audio.rs:2804+`, `little-clip-render/src/lib.rs:3553+`, and workbench model/PDF/audio tests.
- **Behavior:** tests probe hard-coded personal files/paths, return early when absent, and some render tests write outside a temporary test directory.
- **Result:** CI pass status overstates coverage, and running tests on another machine can create unwanted files or exercise unrelated personal data.
- **Needed coverage:** replace with hermetic fixtures/`TempDir`; fail explicitly only in separately marked manual integration tests.
