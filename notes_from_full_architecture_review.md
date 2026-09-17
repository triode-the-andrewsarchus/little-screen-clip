# Full Architecture Review Notes

## Resolution status (updated 2026-09-17)

| Review point | Status |
| --- | --- |
| Highest-value work 1: owned temporary artifacts | **DONE/FIXED** |
| Highest-value work 2: pipeline compile/validation | **DONE/FIXED** |
| Highest-value work 3: shared bounded jobs/processes | **DONE/FIXED** |
| Highest-value work 4: crate layering | **DONE/FIXED** |
| Large mixed-responsibility workbench modules | **DONE/FIXED** |
| Renderer and core-audio module decomposition | **DONE/FIXED** |
| Duplicate staging and clipboard mechanisms | **DONE/FIXED** |
| Remaining external-command consolidation | **DONE/FIXED** |
| Hex parser API split | **DONE/FIXED** |
| Shared operation descriptor/catalog | **DONE/FIXED** |
| Curated core/ops public exports | **DONE/FIXED** |
| Pipeline catalog enforcement | **DONE/FIXED** |
| Lightweight IPC state projections | **DONE/FIXED** |
| Pinned Git dependencies | **DONE/FIXED** |
| Required-side-effect error reporting | **DONE/FIXED** |
| Configuration error reporting | **DONE/FIXED** |
| Local state/retention documentation | **DONE/FIXED** |
| IPC log redaction and request bounds | **DONE/FIXED** |
| Public API contracts | **DONE/FIXED** |
| Rustdoc warnings and stable doctests | **DONE/FIXED** |
| Architecture/IPC/privacy README documentation | **DONE/FIXED** |
| Hermetic fixtures and failure-path coverage | **DONE/FIXED** |

Reviewed 2026-09-17. This review covers all 90 Rust source files (69,541 LOC) in the seven-crate workspace, treating the current tree as authoritative and not relying on earlier review reports. The companion `bugs_from_full_architecture_review.md` contains behavior defects; this file records organization, reuse, maintainability, test, and documentation findings.

## Review method and health signals

- Read the workspace/manifests and the crate boundaries; inspected every source module's public surface and all high-risk execution, persistence, IPC, rendering, external-process, and background-work paths.
- `cargo test --workspace` passed (all unit tests).
- `cargo test --doc --workspace` passed, but ran **zero doctests** in every library crate.
- `cargo doc --workspace --no-deps` completed with nine broken intra-doc-link warnings: `pipeline/recipes.rs:329`, `render/markdown.rs:14`, `render/lib.rs:340`, `image_preview/tools/deskew.rs:6` (four), and `ui_ext.rs:418`.
- `cargo clippy --workspace --all-targets -- -D warnings` fails before leaving `little-clip-core`, with 106 findings there alone. Most are mechanical, but the volume means the advertised strict quality gate is not usable as a regression check.

## Highest-value architectural work

1. Establish one owned temporary-artifact service. `StagingManager`, direct `std::env::temp_dir()` trees, fixed runtime snapshot paths, and pipeline staging all coexist. They have different cleanup behavior, error handling, and lifetimes. A single scoped artifact API should create, atomically write, retain/lease, and clean files; it should serve clipboard images, pipeline intermediates, audio sessions, PDFs, and stem workspaces.
2. Make pipeline validation a distinct compile step before execution. The current executor mixes graph traversal, loose input lookup, process launching, filesystem output, prompting, and result collection in `pipeline/executor.rs`. Preserve the straightforward node model, but validate node IDs, unique input wiring, pin existence, port compatibility, required inputs, destinations, and execution policy once before running a graph.
3. Introduce a bounded background-job/process layer shared by daemon and workbench. At present each feature creates its own thread/channel/process convention (image tools, audio analysis, model loading, PDF previews, network ping, pipeline commands, daemon previews). It needs cancellation/revision IDs, bounded concurrency/output, timeouts, user-visible errors, and cleanup semantics—not another generic UI abstraction.
4. Restore crate layering. `little-clip-render` depends on `little-clip-ops` and performs payload classification/rendering for spreadsheets, models, syntax, and other operation-domain behavior; `little-clip-driver` depends on the renderer. This makes a nominal renderer a broad application/service layer rather than a pure drawing crate. Move only the presentation-ready view models or domain-neutral classifiers into an appropriate lower layer, then keep rendering dependent on those models.

## Organization and reuse findings

### Large mixed-responsibility modules

- `clip-workbench/src/audio_preview.rs` (3,332 LOC), `image_preview.rs` (3,172), `main.rs` (2,646), `image_preview/tools/inspect.rs` (2,254), `image_preview/tools/film_look.rs` (1,914), `canvas.rs` (1,806), and `binary_diff_view.rs` (1,799) combine state, event routing, widget layout, persistence, process invocation, background-job ownership, and pure calculations. The issue is not their line counts alone: it is that the same state changes are observable from multiple concerns and difficult to test without egui. Extract cohesive, testable state transitions and process/job adapters around existing tool boundaries; do not split files just to reduce a metric.
- `little-clip-render/src/lib.rs` is a 3,600+ line rendering catch-all. It contains color conversion helpers, font/text layout, payload/domain branching, syntax highlighting, spreadsheet/model handling, animation logic, and pixel drawing. Its public API deserves a small rendering facade with internal modules organized by rendering primitive/card type.
- `little-clip-core/src/audio.rs` is a 3,300+ line mixture of decoding, waveform pyramids, spectral analysis, pitch/chord/transient detection, filter application, and test data generation. Split by algorithm/data ownership only where it reduces duplicated decoding and makes cancellation/progress contracts explicit.

### Duplicate or divergent mechanisms

- Staging is duplicated between `core/staging.rs`, `pipeline/types.rs` (`/tmp/little_clip_pipeline_staging`), audio-session/stem/PDF paths, and ad-hoc workbench output paths. The prior stated project direction is a staging cache, but there is no single owner or lifecycle.
- Clipboard-to-system-copy is implemented independently in the daemon, image preview, palette tool, and CLI. Process creation, MIME handling, error reporting, and self-copy loop suppression differ. Consolidate the Wayland/X clipboard adapter and document supported MIME types.
- Command execution is replicated in the pipeline executor, image/background-removal tools, OCR, audio/stem tooling, PDF helpers, raw-image helpers, animation, and daemon. It lacks common timeout, cancellation, stderr truncation, executable discovery, and capability reporting behavior.
- Hex parsing is centrally implemented in `color.rs`, but is rewrapped in `config.rs` and `item.rs`, and callers select permissive white-on-error versus validated `Option` behavior. Expose two deliberately named APIs (strict parse and fallback parse) rather than wrappers that hide the difference.
- `get_applicable_ops`/the operation dispatcher and the pipeline catalog independently describe transformations. They will drift as operation coverage grows. A shared operation descriptor/adapter would let the UI, direct IPC operations, and pipeline nodes advertise the same applicability and documentation.

### API and dependency boundaries

- `little-clip-core` and especially `little-clip-ops` broadly re-export entire modules with glob re-exports. That turns internal helpers into a de facto public API and makes ownership unclear. Export curated, stable domain surfaces instead.
- `PipelineExecutor` accepts arbitrary serialized `NodeKind`, pins, and properties from IPC YAML rather than reconciling them with the catalog. The catalog is therefore display metadata rather than the source of truth for executable graphs.
- `IpcRequest`/`IpcResponse` transport complete `SlotManager` and `ClipItem` graphs, including image/audio payloads. This is simple but couples every client to internal models, multiplies copies, and makes large state polling expensive. Define lightweight card/state projections and fetch media/details on demand if the protocol remains long-lived.
- `IpcResponse::State(SlotManager)` is also flagged by Clippy as a very large enum variant. Boxing alone fixes only enum footprint; protocol projection is the architectural fix.
- The root declares shared dependency versions, while many crate manifests still spell their own versions. More importantly, `filmr` and `ntsc-rs` are unpinned Git dependencies. `Cargo.lock` stabilizes an existing checkout, but a lock refresh can change behavior without a manifest review. Pin revisions or release versions and document the update policy.

## Error handling, lifecycle, and observability

- Many production paths intentionally discard I/O, channel, process, and cleanup errors (`let _ = ...`). Some are best-effort, but that intent is undocumented and indistinguishable from accidental loss. Define which side effects are required, trace failures with target/path/context, and expose actionable errors to the caller.
- Configuration loading silently falls through to defaults when a present local/user file cannot be parsed. A typo can change runtime behavior with only an indirect warning. Return/report the rejected path and parse error while retaining a deliberate fallback mode.
- `last_action.json`, runtime snapshots, staging, recipe scripts, and temporary external-tool files are mutable user-state contracts without an ownership/retention document. Document locations, permissions, cleanup, recovery after restart, and whether recipes/scripts are a trusted local-code boundary.
- The daemon logs the complete JSON IPC request at `info` level. Clipboard text, paths, image bytes, and graph scripts can enter ordinary logs. Logging needs redaction/length limits and a documented diagnostic opt-in.

## Documentation findings

- Public API prose is uneven. Most data fields have useful comments, but many public functions/types—especially pipeline types/execution, rendering helpers, and workbench state/tool entry points—lack a concise contract. Add `# Errors`, `# Panics`, and `# Safety` sections where appropriate, not empty templates. Critical missing contracts include: mutation/undo behavior, ownership/lifetime of staged files, external-command requirements, blocking/thread behavior, allowed path encoding, and side effects on the OS clipboard/filesystem.
- Fix the nine rustdoc warnings by escaping UI labels and Markdown examples that are not links. Treat `rustdoc::broken_intra_doc_links` as a warning denied in CI after remediation.
- The four libraries have no executable documentation examples. Add a few small doctests for stable, pure APIs (`parse_hex_color`, slot transitions, URL cleaning, port compatibility, text transforms, duration/units), rather than trying to doctest hardware/UI/process APIs.
- README and crate/module overviews do not provide an authoritative architecture map, process dependency table, IPC versioning policy, external-binary requirements, or data-retention/privacy model. These are required to make the codebase approachable to a new contributor.

## Test strategy concerns

- Tests pass but several are environment-dependent and silently skip when developer-local files/tools are absent. Examples include absolute `/home/vega/...` audio/model paths and tests that write render outputs into a personal `.gemini/antigravity-ide/.../scratch` path. They provide no CI coverage and introduce side effects outside a test sandbox.
- Failure paths are under-tested relative to happy paths: unavailable/full staging directory, non-UTF-8 filenames, short USB writes, malformed/oversized IPC lines, concurrent audio snapshots, malformed pipeline connections, process timeout/cancellation, failed batch copies, config reload behavior, and unavailable external tools.
- Image, audio, PDF, and pipeline test fixtures should be checked-in compact assets or generated in `tempfile::TempDir`; external commands should be behind adapters or feature-gated integration tests.

## Per-area coverage notes

- **Core (14 files):** configuration, item classification, slots, IPC, staging, media/PDF/RAW/animation/audio, theme/color/syntax, and rolling-audio paths reviewed. Main concerns are artifact lifecycle, permissive classification, external tool contracts, and large algorithm concentration.
- **Operations (22 files):** operation registry/dispatch, text/image/binary diffs, metadata/barcode/network/timestamp/unit/spreadsheet/MIDI/model/STL/perspective/effects, and all pipeline files reviewed. Main concerns are duplicated capability descriptions, pipeline validation/execution policy, subprocess lifecycle, and unchecked resource use.
- **Renderer/driver/daemon (8 files):** all render, markdown, USB protocol, capture/lock monitor, and daemon code reviewed. Main concerns are layer inversion, USB framing validation, IPC/privacy/resource policy, and sync work in async handlers.
- **Workbench/CLI (46 files):** all UI modules, tool submodules, shell, preview states, and IPC client code reviewed. Main concerns are feature state/process logic cohabiting with widgets, duplicated external-process/clipboard adapters, and cancellation/staleness/resource control.
