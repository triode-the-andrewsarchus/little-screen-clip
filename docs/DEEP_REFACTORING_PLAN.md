# Deep refactoring plan — reviewed proposal

**Status: implementation authorized and in progress. Native mirror adoption remains separately reviewable.**

This plan follows the completed work in [REFACTORING_PLAN.md](REFACTORING_PLAN.md), rebased onto `main` at `2f75a97`. It replaces the previous proposal after checking its claims against the combined source tree. It preserves the goal of shared, useful interactive views while removing speculative interfaces, incorrect diagnoses, and unsupported performance promises.

## What the refactor should achieve

1. Keep browsing, editing and clipboard interaction responsive when a decoder, daemon, storage device or model is slow.
2. Let the workbench and file manager share studios without one application owning the other's implementation.
3. Make a new format reuse existing classification, bounded decoding, preview data and interactions where appropriate.
4. Preserve the project's distinctive views: audio constellation, photo similarity, color strips, waveforms, Timelens, and playable multisample maps. Share their foundations without flattening them into one generic widget.
5. Reduce actual maintenance cost and unnecessary compilation, measured against representative tasks rather than arbitrary file sizes or complexity quotas.

This is primarily behavior-preserving work. New views, additional format coverage, changes to the mirror's appearance and removal of existing tools are separate product changes with explicit acceptance criteria.

## Review of the previous proposal

| Previous claim or proposal | Review and revised decision |
| --- | --- |
| The workbench compiles its studio tree twice | Addressed by the current branch: `main.rs` calls `run_workbench()` in the library. Preserve this for all new formats; do not schedule it again. |
| File manager must become an independent application | It already has an independent lifecycle and now its own binary package. Its library dependency on `clip-workbench` remains a worthwhile structural issue, not evidence that it is currently a workbench tab. |
| Search uses ONNX/BERT and owns all heavy ML dependencies | Incorrect. Search uses an owned external model process and GGUF/llama.cpp helpers. ONNX supports image tools and photo similarity; tokenizers are used by SigLIP. Move dependencies with their actual owners, not by name. |
| A `clip-ui` dependency on `little-clip-core::palette` is lightweight | Cargo depends on crates, not individual modules. Depending on core still inherits its dependency graph. Begin with an honest measured boundary; consider a smaller theme/types crate only if needed. |
| SVG thumbnails still require `rsvg-convert` | Superseded by main. `ImageThumbnail::generate_from_svg` calls core's shared SVG renderer, including SVGZ handling. Some comments remain stale; do not add another renderer or move core behind a UI dependency. |
| The physical screen is 320×220 | Incorrect. The current renderer and mirror use 1920×462. Derive geometry from their shared contract, not a new hardcoded dimension. |
| Waveform/color/snapping logic must be rebuilt for every view | Overstated. Core already provides waveform summaries, `WaveformColor` and zero-crossing mechanisms. Locate remaining duplication before extracting anything. |
| `FileBatch` is an accidental untyped escape hatch | File references are intentional, including one-file batches and Shelf references. Keep reference semantics, raw path bytes and IPC payload policy. Classifying a referenced file does not authorize loading or copying its contents. |
| Every format needs a full editor and every visual tier | Keep a capability matrix, but permit useful summaries, read-only inspectors, metadata-only views and explicit unsupported states. G-code must never control a machine; multisample preview is not a full sampler. |
| Split seven studio crates plus a facade immediately | Too much simultaneous migration. Extract a minimal shared boundary, then split individual domains when dependency isolation or independent development justifies it. Avoid a facade that re-enables all heavyweight dependencies. |
| Search needs a new connection pool, model trait and scoring modules | These are unproven prescriptions. Store already has vectors, safety, session, publication and other submodules. Inspect remaining responsibilities and preserve transactions, ranking and source admission. No new pool or model backend abstraction without a concrete need. |
| Replace large `match` expressions to lower CCN | Not a useful goal by itself. Enum matches can provide exhaustive checks and readable dispatch. Simplify complex behavior; do not replace straightforward icon/extension tables merely to improve a score. |
| All clocks should use egui time | Incorrect for workers, timeouts, audio and metadata. Use frame time for visual animation, monotonic time for lifetimes/deadlines, playback clocks for sound, and wall time for dated facts. |
| Optimistic updates and automatic four-second toasts everywhere | Unsafe for authoritative mutations and inconsistent with the preferred UI. Successful operations normally need no toast. Failures, pending work and decisions need appropriate persistence and accurate completion results. |
| Native egui mirror guarantees zero latency and 144 Hz | Unsupported. Native text may improve clarity, but decoding stays asynchronous and frame rate depends on workload. Treat this as a measured prototype, retaining the raster mirror as a reference until approved. |
| Delete stale `/tmp` files by process signature | Too broad. Cleanup must use application-owned locations, verified ownership/lifetime and held directory handles. Accepted outputs and other sessions must remain untouched. |
| Every change must make aggregate metrics monotonically better | A noisy, move-sensitive tool cannot enforce that safely. Review new complexity and duplication in context; verify concrete improvements and record justified tradeoffs. |

Evidence anchors: [entrypoints/library](../crates/clip-workbench/src/lib.rs), [file-manager entrypoint](../crates/clip-file-manager/src/main.rs), [search runtime](../crates/clip-workbench/src/file_manager/search/runtime.rs), [model helper documentation](../scripts/search_model/README.md), [ONNX runtime](../crates/clip-workbench/src/onnx_runtime.rs), [SigLIP](../crates/clip-workbench/src/file_manager/gallery/similarity/siglip.rs), [SVG thumbnails](../crates/little-clip-core/src/item.rs), [screen dimensions](../crates/little-clip-render/src/lib.rs), [shared waveform mechanisms](../crates/little-clip-core/src/audio/waveform.rs).

## Baseline and evidence limits

The `refactor-rebase-main` and `refactor-rebased` architecture snapshots compare merged main with this branch. Measurements use the same fast profile, scope and tool versions; they are not a complete inventory of generated, vendored or runtime code.

| Metric | Merged main | Rebased refactor, before final test/error-message fixes |
| --- | ---: | ---: |
| Measured NLOC | 323,037 | 322,528 |
| Functions | 10,516 | 10,505 |
| Average / maximum CCN | 4.07 / 195 | 4.07 / 195 |
| Duplicate percentage | 4.55% | 4.51% |
| Clone groups | 1,527 | 1,516 |

The latter snapshot has 219 functions above CCN 15. The analyzer reports moved functions as newly complex; matching their original symbols finds no new threshold crossing. Seven unmatched clone fragments reflect changed field names, removed fields or substituted guarded readers in existing repeated blocks. These remain review candidates, not seven newly designed algorithms. Report the tool's raw result alongside this interpretation; do not manufacture an `IMPROVED` result by moving code or excluding files.

Before each approved milestone, capture its relevant baseline: warm incremental build time, affected dependency graph, peak build disk usage, time to usable window, UI frame timings, worker queue pressure, or representative query/decode latency. Use comparable fixtures and report distributions where timing varies. No universal “two-second crate” or “ten-second app” promises.

## Invariants that every phase must preserve

The detailed rules in [AGENTS.md](../AGENTS.md) remain authoritative. In particular:

- The workbench stays a fixed-size, centered, single-instance popup; the file manager stays independently resizable. Browser selection, history and scroll survive studio entry/exit.
- UI frames do not perform content reads, decoder work, helper waits or model inference. Admission, cancellation, generations and bounded caches remain owned by explicit sessions/workers.
- Read cancellation must not discard accepted write results. Save/export/rename/copy operations retain source identity, validated output directories, conflict handling and atomic no-overwrite/replacement semantics.
- Classify system interfaces before ordinary content. Protected paths, devices and pseudo-filesystems never enter general decoders. Preserve system observations, namespace/lifetime identity and restricted metadata-only policies.
- Search results keep source-root identity, unavailable state, publication generation and navigation guards. Extraction of a search crate must not replace guarded entries with bare paths.
- Keep owned helper subprocesses, resource limits, cancellation, descriptor inheritance and teardown. Both desktop binaries must dispatch every worker their enabled studios launch before normal CLI parsing.
- Keep the GPL video helper and separately licensed NKI/EXS helper outside application linkage, with corresponding source/license packaging. Keep native multisample playback free of a JVM dependency.
- Retain per-photo edits/history, unsaved previews, ratings, export-all and inline rename. Retain shared editor chrome and bottom-bar sizing.
- Retain exact velocity-region selection, focused MIDI routing, note release/sustain, and mapping/harmony behavior. Preserve waveform/MIDI range semantics and bounded Timelens sampling.
- Reuse SVG icons, shared tabs and existing appearance mechanisms. Successful operations do not gain redundant notifications.

## Proposed architecture, incrementally

The desired ownership direction is applications → shared desktop/domain code → core. There is no requirement to create every box as a Cargo crate immediately.

| Boundary | Owns | Does not own |
| --- | --- | --- |
| `little-clip-core` and existing domain modules | Shared models, classification facts, guarded file access, process/job mechanisms, IPC contracts, reusable parsing and signal math | egui state, application navigation |
| `little-clip-render` / `little-clip-scene` | Existing headless raster cards / scene import and GPU rendering | UI event routing, synchronous content probing during paint |
| Proposed `clip-ui` | Shared styles, icons, tabs, generic controls and status presentation | Browser, decoder, model or mutation lifecycles |
| Shared studio implementation, initially one neutral library if that is the smallest safe extraction | Existing studios, domain widgets, preview interactions, their job/session ownership | Workbench popup lifecycle or file-manager navigation |
| Proposed `clip-search`, after its interfaces are ready | Index/store/retrieval/model-process orchestration and guarded extraction protocol | egui contexts or browser state |
| `clip-workbench` / `clip-file-manager` | Their respective application state, navigation and command coordination | Each other's implementation |

Extract only the generic portions of `ui_ext` and appearance support. ANSI parsing is text-domain behavior; the component gallery can compose domain widgets and need not live inside the lowest-level UI crate. Do not pull core upward into `clip-ui` to share drawing code.

A smaller image/audio/text/etc. studio crate is justified when it removes a demonstrated dependency edge, gives a useful build boundary, or has independently owned lifecycle/testing. A line-count threshold alone is insufficient. Preserve all currently enabled tools in the normal build; a minimal optional build is a separate supported configuration, not a silent removal of ML or media features.

## Content and visual reuse policy

Start from [document_format.rs](../crates/clip-workbench/src/file_manager/document_format.rs), core MIME/extension helpers, decoder hooks, the existing preview caches and [file_types.md](file_types.md). Avoid inventing a second complete format registry.

Keep three distinct facts:

1. **What might it be?** A cheap extension/MIME hint, with no filesystem I/O on the UI thread.
2. **What was safely observed?** A worker-side bounded probe, tied to source identity and access context. Ambiguous `.xml`, `.exs`, `.preset`, `.iso` and similar names require positive evidence before specialized routing.
3. **What can this view do?** Capabilities such as thumbnail, strip, audition, read-only inspection, edit or export; explicit pending/unavailable/unsupported states.

A shared enum may help implement these facts, but exhaustive matching alone does not guarantee UI coverage or safe access. Keep format-specific detectors and limits; thin compatibility helpers can remain during migration. Do not globally probe every file to populate a registry.

The four presentation contexts remain useful planning axes:

| Context | Useful result | Constraints |
| --- | --- | --- |
| Hardware card / clipboard mirror | Bounded summary or representative visual | Renderer consumes prepared data; no per-frame decoder or subprocess work |
| Grid / Shelf / search tile | Recognizable thumbnail, sample coverage or useful metadata | Distinguish content bytes from references; explicit selection/focus behavior |
| Inline strip | Quick exploration appropriate to the format | Bounded row work; shared cache and cancellation; prevent row gestures from stealing text editing |
| Studio / inspector | Deeper interaction and editing where supported | Preserve session history, file safety, exports, external handoff and source checks |

Audit coverage family by family rather than requiring every cell to become an editor. Include images/RAW/layers, audio/video/MIDI, multisamples, PDF/Office, text/subtitles, fonts/collections, notebooks, EPUB/CBZ, mail/calendar, SQLite, GPX/GeoJSON, G-code, 3D/splats and archives. System interfaces stay on their separate observation policy. Record each cell as implemented, useful fallback, intentionally unsupported or proposed; prioritize practical formats and user workflows.

Reuse decoded summaries and proven signal/geometry algorithms between contexts. Keep distinct interaction layers: a tiny waveform badge needs neither a studio's channel editing state nor a row's selection routing. An isomorphic keyboard, piano mapping view, constellation, and waveform have different jobs. Do not force a universal visual or a generic event bus around them.

## Preparation for future changes

Apply these acceptance criteria where the approved refactor already touches the relevant code. They prepare concrete reuse paths without requiring new product features or a framework for every boundary.

| Preparation | Acceptance criterion | Future benefit |
| --- | --- | --- |
| Prepared content separate from painting and interaction | Existing prepared data can feed raster and native views without duplicate decoding or caches; preserve source identity, revision, access context and pending/error states. | Native mirror, richer Shelf cards and new previews. |
| Studio sessions independent of their host | Document identity, undo/history, unsaved edits, background jobs and accepted exports have explicit owners; host navigation and window lifecycle stay outside the session. | Consistent hosting in both applications and a foundation for possible tabs or multiple documents later. |
| Explicit input and audition ownership | Document how MIDI, shortcuts, pointer gestures and playback acquire and release focus, including navigation, view disappearance and non-multisample selection. Preserve text editing and the existing single-preview policy. | More interactive views without competing players or stolen input. |
| Incremental format capabilities | Reuse existing classification and record thumbnail, strip, inspection, editing and export availability; distinguish safe observed capabilities from extension hints. | New formats can join existing views without another registry or decoder path. |
| Explicit background completion contracts | Existing workers carry operation/session identity and reject stale read results; cancellation, progress and shutdown preserve authoritative completion of accepted writes. | Consistent behavior as more previews and exports are added. |
| Parsers and search independent of egui | Keep parsing, ranking and guarded data access outside painting and application state; use the smallest notification boundary that real callers need. | Isolated tests, CLI reuse and possible future batch operations. |

The central review question is who owns each document session, preview job, playback instance and accepted save, including when its view disappears. Prefer clarifying existing owners over introducing a generic event bus, plugin system or universal widget abstraction. This preparation does not add tabs, multi-document UI or batch features.

Retain a small representative fixture and workflow set: mixed-folder opening, studio switching with unsaved edits, multisample velocity/MIDI auditioning, slow search and delayed daemon responses. Reuse existing fixtures and private acceptance runners; record relevant responsiveness, correctness and bounded resource use rather than creating a large benchmark suite unrelated to the change.

## Compatibility with later file-manager phases

[File Manager/plan.md](<File Manager/plan.md>) remains the product roadmap. Its later phases are intended capabilities, with design decisions still open. Although it records accepted phase 5–6 checkpoints, ongoing media/search work remains a practical focus; this refactor must support that work and avoid baking the current single-folder presentation into shared services. It does not reopen completion claims or authorize implementation of later phases.

Prepare the following boundaries as their existing owners are refactored. The objective is to avoid foreseeable structural rework, not to guarantee that future requirements will never require adaptation.

| Roadmap phase | Prepare during this refactor | Leave to the feature design |
| --- | --- | --- |
| **7: Views and meaningful sorting** | Keep location/source identity, listing data, view composition, selection and scroll state distinct. Avoid making shared preview/search services depend on one mutable current-directory field. Preserve origin/provenance for entries so Miller columns and bounded inline expansion can reuse them. Reuse the phase 6 query/sort representation, typed missing/pending states and stable selection identity. Keep view preference resolution separate from observed content categories. | Column UI, expansion policy/budgets, dominant-type thresholds, metadata sort choices and preference inheritance rules. |
| **8: Organization and Home** | Keep filesystem facts separate from pins, session marks, sorting sets, visibility preferences and project references. Retain explicit browse/reveal targets and source identity rather than treating every displayed item as an immediate child of one directory. Isolate history/location handling so a future virtual Home need not impersonate a filesystem path. Keep persistence ownership separate from rendering and portable metadata separate from machine-local state. | Virtual Home implementation, durable annotation identity across rename/move/restart, `.fmproject`/`.hide`/`.show` schemas and precedence. No browsing-created sidecars. |
| **9: Transformations and metadata** | Preserve a UI-independent path from an explicit selection and output/naming policy through validation, execution and per-item results. Reuse existing ops/pipeline, naming and save mechanisms; retain source revisions, held output-directory handles, conflicts and partial completion. Distinguish read/probe support from safe write capability. | Batch planning UI, metadata writer policies, sidecars and assisted rename suggestions. |
| **10: Rules and contextual workflows** | Keep operation submission independent of the initiating button or studio, with explicit inputs, operation identity, cancellation and attributable results. Preserve the existing node/pipeline model. Separate read-only proposal/validation from mutation so a future dry run can report intended effects without executing actions; execution must still revalidate. Keep observed filesystem events distinct from a policy decision to run a rule. | Trigger orchestration, execution history persistence, imported recipe trust, loop prevention and dynamic Home policies. Do not prebuild a rule engine or generic event bus. |
| **11: Storage analysis and reconciliation** | Keep evidence tied to source identity/revision and coverage; distinguish exact content verification from similarity and incomplete observations from absence. Reuse bounded scans and pairwise comparison tools. Keep proposed changes inspectable before submission through the established operation mechanism; do not tie comparison results directly to mutations. | Duplicate indexing strategy, hard-link and allocated-size accounting, merge planning and cleanup policy. |
| **12: Remote performance and scheduling** | Separate operation correctness/completion from queue admission and scheduling. Keep bounded read cancellation distinct from accepted writes, preserve typed unavailable/partial outcomes, and avoid assuming that an ordinary mounted path is fast or local. Keep storage/mount observations usable without embedding SSH addresses or guessed host identity into path strings. Retain existing pinned local-file safety contracts. | Verified remote mappings, resource contention scheduling, execution backend selection, SSH terminals and jobs surviving disconnection. No universal filesystem trait or remote executor merely for future use. |
| **13: Filesystem editing and Git** | Keep proposed rename/move/create/delete actions, validation and application independent of the UI that creates them, reusing the same operation boundary needed by phases 9–11. Preserve external-change checks and per-item outcomes. Keep Git observations asynchronous and separate from directory facts and visibility policy; an ignored item is not an absent item. | Textual filesystem syntax, Git status UI and any separately approved Git mutations. Do not promise cross-filesystem transactional rollback. |

### Cross-phase acceptance criteria

- **Location and presentation:** inspect the existing location/history, search scope and entry models before changing them. Preserve lossless paths, source-root guards and navigation provenance. During extraction, document remaining single-folder assumptions and keep them in the current browser composition layer rather than moving them into shared domain APIs. Do not weaken system-interface navigation guards to accommodate future virtual results.
- **One operation path:** trace representative naming, transfer, editor save/export and pipeline paths. Share established validation/execution contracts where they actually fit; document necessary differences rather than forcing all operations into one oversized command enum. Future batch, rule, merge and textual planners should be able to submit through these owners without bypassing safety or rewriting mutation execution. A preview plan is never an authorization token or a substitute for execution-time checks.
- **Facts, annotations and policy:** prepared content and metadata remain observations with freshness/coverage; presentation preferences and temporary marks remain separate state. Rendering must not write persistence or trigger rules. Keep future persistence/schema choices open until rename/move and portability behavior is specified.
- **Scheduling:** preserve explicit job/session ownership and priorities. Make scheduler decisions separable from worker semantics so future storage-aware admission can reuse correctness, cancellation and completion handling. Do not introduce persistent queues or new resource keys without a current consumer.
- **Dependency ownership:** browser navigation, future Home/organization and rule-trigger UI belong with the file manager; reusable studios remain neutral; transformations remain in ops; search owns retrieval rather than all browser metadata. These directions are more important than one crate per roadmap phase.
- **Review gate:** each relevant extraction records which later phases it enables, which current assumptions remain, and why those assumptions can be changed locally. Verify with existing concrete consumers and focused contract tests; do not create unused adapters or implement speculative future features just to check this list.

## Execution sequence proposed for approval

### 0. Establish targeted evidence and contracts

- Record current dependency/caller maps and representative workflow baselines. Use Cargo metadata/tree/timings and existing architecture tools; do not delete build caches to benchmark without an explicit need.
- Inventory worker entrypoints, IPC call sites, preview/cache ownership and feature configurations. Mark facts versus suspected issues in the review findings. Map session, input, audition and accepted-write ownership using the preparation criteria above; establish the representative workflow baseline.
- Select one concrete slice for each milestone and specify its user-visible behavior, rollback boundary, tests and expected benefit. Map the affected owners to the later-phase compatibility checklist above, including any single-folder, persistence or scheduling assumptions that must remain local.

**Exit:** an evidence-backed worklist, with no implicit authorization to expand into unrelated features.

### 1. Fix confirmed blocking and lifecycle hazards

These have clearer user value than a large crate move and can land independently.

- **Mount startup:** `Mounts::new()` currently calls `scan_mounts_sync()`, which invokes `lsblk`. Move discovery to the existing bounded worker path. Prefer an immediate empty/loading state or a previously validated snapshot; do not replace it with a different synchronous scan. Bound subprocess output/lifetime and reject obsolete results.
- **USB I/O:** `display_worker::run` is async but invokes synchronous open/send callbacks. Isolate the hardware session on one owned thread or equivalent dedicated blocking owner. Preserve latest-frame coalescing, 400 ms keepalive, minimum send gap, reconnect backoff and driver release-before-reopen. Specify shutdown behavior and test stalls with a fake device; do not access hardware merely to validate a refactor.
- **UI IPC:** Trace remaining direct calls in shortcuts/actions/prompt execution. Reuse `BackgroundJobs` and existing queued IPC patterns first. Add a dedicated owner only if ordering or admission requires it. Maintain per-operation identity, pending state, timeout meaning and reconciliation with daemon snapshots. Do not optimistically report a mutation as completed.
- **Clipboard route:** Verify the remaining `clipctl copy` fallback/error window. If adding cancellation, use a route/request identity and compare-and-cancel semantics so one client cannot cancel a newer route. Expiry remains the crash fallback; async protocol I/O in `Drop` is not the primary mechanism.

**Exit:** delayed/failing fake IPC, mount and USB sources do not stall unrelated UI/daemon work; no operation is duplicated or loses its completion result.

### 2. Consolidate one presentation path and classification seam

- Pilot the classification/capability policy with one existing family, including ambiguous-name and protected-source tests. Preserve all existing dispatch routes while migrating consumers incrementally.
- Use audio as the first reuse candidate: inspect core waveform summaries, crossover colors, downsampling and snapping, then consolidate only genuinely repeated behavior across two consumers.
- Make prepared-content boundaries usable by both the existing raster mirror and a future native view, preserving guarded source and readiness states. Keep current rendering operational during extraction.
- Separate pure calculations, background work, interaction state and painting where that makes the code testable. Keep explicit event/ownership contracts rather than creating an all-purpose `GeometryBuilder` or controller framework.
- Extend the same method to PDF or video only after the first slice demonstrates a benefit. Preserve the specialized photo grid layout and Timelens renderer/helper boundaries.

**Exit:** two real consumers share a useful mechanism; representative frame/latency/memory results do not regress; no new parallel cache or classification system remains.

### 3. Extract application boundaries in small, buildable steps

- Extract a small shared UI boundary and the minimum studio/session interfaces needed to remove application-to-application ownership. Measure what dependencies remain through core.
- Preserve explicit session ownership of undo, unsaved edits, playback and accepted jobs independently of host navigation. Verify focus transfer and teardown against the input/audition contract; do not add multi-document UI as part of extraction.
- Move shared desktop/studio implementation to a neutral owner while retaining temporary forwarding exports where needed. Keep browser-specific system, mounts, transfers, search UI and Shelf navigation with the browser.
- Remove `clip-file-manager → clip-workbench` only when both applications can consume the neutral implementation without a dependency cycle. Do not copy modules to make the build pass.
- Move dependencies with code. ONNX remains available to image tools; SigLIP/tokenizers remain available to photo similarity; SQLite remains available to both search and its document inspector.
- Audit internal worker launchers and `/proc/self/exe` assumptions at each step. Preserve dispatch in both hosts where necessary; use separate helper binaries only with explicit discovery, packaging and lifetime contracts.
- Update launch/build/acceptance commands, documentation, helper packaging and alternate-working-directory behavior together. When the neutral library extraction lands, update the file-manager roadmap’s current `clip-workbench` sharing references to the delivered ownership without changing its product scope.

**Exit:** both binaries keep all current studios and workflows; each source is compiled once per configuration; Cargo has the intended acyclic ownership graph; measured rebuild cost or dependency isolation improves.

### 3a. Validate the shared boundary with a native mirror prototype

Schedule this immediately after the relevant shared UI and prepared-content extraction, before unrelated search or domain restructuring. Preparing and prototyping during the refactor avoids cementing a raster-only interface. Keep ownership moves and rendering changes in separate, buildable commits so regressions remain attributable.

- Build a native egui slot view against the prepared-content boundary, using existing input routing and shared presentation semantics. Use the prototype to validate the boundary with a second real consumer; avoid speculative interfaces.
- Compare appearance, text clarity, interaction, frame time, memory, input latency and slot hit testing against the current raster mirror at relevant scales. Include pending/error states and representative content types.
- Keep the current mirror working and retain the hardware renderer plus an exact display-reference view. Native rendering does not remove asynchronous preparation or guarantee a particular frame rate.

**Exit:** reviewable prototype and measured findings. Adopting it as the on-screen replacement requires separate user approval; the prototype must not block independently useful structural changes. The mirror and rolling capture remain specific to Quick Palette.

### 4. Extract search and simplify selected domain hotspots

This follows interface preparation; it is not a second simultaneous workspace rewrite.

- For search, retain existing external model ownership and worker protocol. Replace the direct egui repaint dependency with a small notification boundary. Keep UI formatting in the browser.
- Define the minimum guarded file/result descriptors needed outside the browser without dropping root identity, revisions, unavailable observations or path-byte fidelity. Reuse existing descriptors before adding another type hierarchy.
- Audit existing `store/*`, query, explanation and retrieval modules. Extract cohesive ranking/transaction responsibilities only where tests and call sites demonstrate a boundary. Preserve ranking quality, incremental scanning, index schema, query plans and bounded cache behavior. No database migrations merely for a code move.
- For waveform/image/MIDI UI, extract state transitions and computations that can be tested independently. Preserve event precedence, audio clocks and painter ordering.
- For pipeline execution, retain the enum dispatch unless actual extension needs justify a registry. Domain handlers may be ordinary functions. Preserve cancellation, progress, input/output typing, error attribution and draft/history behavior.
- Keep text, binary and image diff algorithms distinct unless a real consumer needs a common interface. Shared navigation is already consolidated.
- Consider a radio/DSP crate or feature only after measuring its dependency cost and identifying all users. Do not disable existing audio tools by default as a side effect.

**Exit:** each change removes a concrete coupling, duplication or failure-prone state transition; source-compatible behavior and representative search/media results are preserved.

### 5. Optional product-facing presentation improvements

These are separately reviewable designs, not prerequisites for the structural work.

- If the phase 3a prototype is approved, adopt the native on-screen mirror in a separate change while retaining the hardware renderer and exact display-reference view. Otherwise retain the current mirror and record the findings.
- Assess icon-grid/Shelf/search-tile reuse against the actual layouts. Reuse common pieces without forcing the photo mosaic, Shelf and similarity grid into identical cards.
- Add missing useful summaries or interactions by an approved capability priority list. Do not make complete four-tier coverage a prerequisite for shipping safe refactors.

**Exit:** user approval of the visual/interaction prototype before replacing existing presentation. The mirror and rolling capture remain specific to Quick Palette.

### 6. Targeted status, clock and artifact hygiene

These can be scheduled earlier when a specific bug is demonstrated.

- Give transient feedback, persistent errors, progress and unsaved state distinct lifetimes. Reuse `StatusToast` where appropriate; keep important errors actionable and normally omit success messages.
- Use the clock appropriate to each responsibility. Test timeout/cancellation without coupling worker lifetimes to repaint frequency.
- Inventory existing `ArtifactLease`, `TempDir`, staging and export handles before introducing new cleanup. A narrowly scoped crash-recovery sweep needs verified ownership, session lifetime/leases, directory pinning and protection for accepted/pending exports. Never sweep generic `/tmp` by basename or PID alone.

**Exit:** demonstrated leaks/stale statuses are fixed without invalidating another session, accepted write or shared artifact.

## Verification and stop rules

For each approved slice:

- Run formatting and targeted tests during development; run workspace/all-target compilation and serial workspace tests at integration boundaries. Build both desktop executables after entrypoint or crate changes.
- Run relevant Clippy checks and distinguish pre-existing diagnostics from introduced ones. Do not suppress warnings globally or rewrite unrelated code just to obtain a green global `-D warnings` result.
- Use the private Weston/Niri runner for UI acceptance. Preserve its private clipboard payloads when testing copy behavior; never inject test input through the user's compositor.
- Keep malformed/oversized input, cancellation, source replacement, protected paths, ambiguity, rename/export conflicts and helper failure coverage. Retain real format fixtures and licensing boundaries.
- Compare architecture with rename/move awareness. Prefer deletion, consolidation and simpler state over file splitting. A long but straightforward enum match is not automatically a regression.
- Record before/after evidence appropriate to the task, including build disk use. Serialize builds and sweeps; use bounded build jobs and avoid unnecessary feature/profile variants.
- Keep commits buildable and reversible. Do not combine schema changes, visual redesign and crate relocation in one commit.
- If investigation reveals a new architectural mechanism, major scope increase, lost capability or changed product behavior, update this proposal and seek approval before proceeding with that extension. Routine implementation choices within an approved slice do not require repeated permission.

## Execution status

The user authorized completing this plan after the review and future-roadmap additions. Execute the phases incrementally, retaining the specific design and adoption gates above. The initial architecture baseline is `deep-refactor-start`; the working tree began at `4fabcc3`, with 164 GiB available. Phase 1 is in progress: mount discovery, USB ownership, workbench mutations, clipboard routes and several studio handoffs have been updated. Rolling-audio copy, mutation-worker admission/isolation and final integration checks remain open. Phases 2–6 remain outstanding; native mirror adoption retains its separate review gate.


### Implementation ledger

- **Initial dependency evidence:** Cargo metadata confirms `clip-file-manager → clip-workbench`, with workbench depending on core, ops, render and scene; render depends on core/scene; ops and scene depend on core. The native NKI helper remains independent. Snapshot: `/tmp/deep-refactor-metadata.json`. Neutral studio ownership and search extraction are still outstanding.
- **Phase 1, mount discovery:** construction now performs no scan; the first tick submits to the existing bounded executor. Explicit refresh replaces the previous observation receiver and cancels its ticket. `lsblk` uses core's command runner with a five-second timeout, a 2 MiB output limit and owned process-group cleanup; truncated JSON is rejected. Existing mounted-entry fallback remains available. Seven mount tests pass, including deferred startup with an occupied worker and retired-result rejection (`/tmp/deep-mount-tests.log`). Architecture comparison against `deep-refactor-start` keeps clone groups and dependency cycles unchanged; added checks/test contribute three CCN points. Full integration and private UI verification remain due.
- **Next phase 1 slice:** USB ownership must cover the synchronous startup `TrofeoDriver::open` as well as worker open/send callbacks. Existing retry/keepalive tests provide the baseline. UI IPC and clipboard-route audit remain outstanding.

- **Phase 1, USB ownership:** startup enumeration and session callbacks now belong to a named dedicated thread with a private timer runtime. A single latest-frame watch channel replaces the bounded FIFO, avoiding producer backpressure during a stalled device call. Shutdown closes the frame channel; the owner releases the driver when an in-flight call returns. USB transfers retain existing timeouts; OS enumeration is not forcibly interruptible, and the async executor never joins the native thread. Fake-device checks cover retries, keepalive, delayed open/write, newest-frame delivery and release. All ten daemon tests pass, including the four display-worker tests, with the final run recorded in `/tmp/deep-usb-tests-final.log`. The pacing test also verifies the 80 ms minimum gap and newest-frame replacement during that wait. No physical USB access was used. Architecture comparison reports existing clones touching modified files and a reordered cycle in untouched search Python code; clone/cycle totals remain unchanged. No new shared abstraction or dependency was introduced. Full workspace integration checks remain due.
- **Phase 1, IPC audit:** synchronous UI calls remain in mirror/shortcut slot swaps, palette execution, node pipeline execution, auto-action changes and rolling-audio copy refresh. Existing startup state transport and `BackgroundJobs` provide reuse points. Slot swaps currently mutate local state even after a failed send; migration must reconcile authoritative results and preserve accepted operation completion. `clipctl copy` installs a two-second route before `wtype` and ignores its exit status; cancellation needs request identity to avoid cancelling another client's route. These fixes remain outstanding.

- **Phase 1, first UI IPC migration:** mirror/shortcut swaps and node pipeline submission now share one retained `BackgroundJobs` mutation slot. It has no post-write deadline, suppresses duplicate submissions, defers window closure while pending, and publishes errors or pipeline results after view changes. Swaps no longer mutate a potentially stale local snapshot after failed transport; the existing state poller reconciles the daemon's slots. Rolling-audio copy no longer synchronously fetches state. Twelve workbench action tests pass (`/tmp/deep-ipc-actions.log`), including delayed/error completion and pipeline publication. Palette actions, in-app/standalone prompts and auto-action toggles still need migration; full integration/UI checks remain due. Initial architecture comparison identified one added test-loop clone, consolidated into a shared completion helper; reported Python cycles are in untouched files and differ by member ordering.

- **Phase 1, palette/prompt/settings IPC:** palette operations and recipes, the in-app prompt, standalone prompt and auto-action toggles now use the retained mutation worker. Production dispatch contains no synchronous daemon calls on these UI paths. Completion closes action popups only on confirmed success; rejected/failed prompts keep input and expose errors. Auto-action values publish only on confirmation, and pending forms prevent duplicate submission. The standalone and main workbench hosts share request polling and action-response validation. Existing selection/response tests use test-only immediate dispatch adapters; separate asynchronous tests verify actual delayed completion, close behavior, setting publication and standalone prompt retry. All 96 workbench lifecycle tests pass (`/tmp/deep-ipc-all-workbench-final.log`). Architecture clone/cycle totals remain at baseline with no new high-complexity function reported.
- **Remaining phase 1 audit:** shared studio call sites include synchronous rolling-audio copy, OCR push-to-hand and image-diff copy. Assess these and existing export/appearance timeout policies before claiming the UI IPC gate complete. `clipctl copy` route cancellation remains outstanding. Later refactor phases are still outstanding.

Desktop integration for the palette/prompt/settings slice: both `clip-workbench` and `clip-file-manager` binaries build (`/tmp/deep-ipc-desktop-build.log`); the private Weston/Niri component-gallery acceptance passes with no failures (`/tmp/deep-ipc-workbench-ui/report.json`). This checks desktop integration, not the outstanding shared-studio IPC paths.

- **Phase 1, clipboard route lifetime:** identified requests now receive typed pending/claimed/cancelled/expired/superseded acknowledgements. The daemon retains one route record and atomically compares identity before cancellation, so cleanup cannot revoke another client's override or undo a watcher claim. Legacy requests without identity retain their old success response; the new CLI requires an identified acknowledgement before sending input. CLI shortcut/primary-selection helpers use bounded core command execution, and primary fallback occurs only after confirmed cancellation/expiry; a claim racing cancellation suppresses a duplicate push. Route IPC errors exit without reusing an uncertain stream, while expiry remains crash cleanup. “Claimed” means the watcher took the override, not completed content decoding. Five focused tests pass across core, daemon and CLI, including a socket-pair cancellation race (`/tmp/deep-route-tests-final.log`). Architecture clone/cycle totals and maximum complexity remain unchanged; existing touched-file warnings are not evidence of newly duplicated algorithms. Shared studio IPC cleanup and later refactor phases remain outstanding.

The route protocol slice also passes `cargo build -j4 --workspace --bins` (`/tmp/deep-route-build.log`) and formatting. No live desktop copy/paste or hardware was invoked during its tests.

- **Phase 1, OCR and image-diff copies:** retained request execution/result validation now lives in shared `desktop_ipc::PendingRequest`; workbench actions and both prompt hosts delegate to it. OCR hand delivery runs on the bounded worker, removes the redundant second OS clipboard write, suppresses duplicate submission and retains completion through editor retirement/close. Image-diff results use `Arc`, and mask encoding plus IPC run off the UI thread against the captured result; pending copies defer studio/window close. Success no longer produces an unconditional copy toast; failures are reported from actual replies. Validation passes 44 copy tests, 96 workbench lifecycle tests, 51 diff tests, both desktop builds and formatting (`/tmp/deep-studio-copy-tests.log`, `/tmp/deep-shared-ipc-workbench.log`, `/tmp/deep-shared-ipc-diff.log`, `/tmp/deep-shared-ipc-build.log`). The analyzer reports one extra clone group; normalized-fragment inspection against `deep-refactor-start` identifies an existing shell/Python test-script fixture helper whose matching context changed when a test was inserted. No new duplicate production mechanism or dependency cycle was added. Rolling-audio copy and remaining export/appearance contracts still need review before the phase 1 gate closes.

- **Phase 1, export completion and appearance reloads:** image/audio handoffs already run on workers; their two-second reply deadlines are removed so slow accepted writes do not trigger a premature fallback. MIDI export no longer cancels an earlier accepted copy on repeat submission or viewer drop, and its worker no longer applies a post-operation deadline. Pending MIDI exports block studio/window close; the daemon handoff validates `Success` rather than treating an error response as delivery. Published MIDI artifacts remain retained when a reply is lost, protecting consumers from pathname deletion. Appearance reloads now use shared retained requests and coalesce later toggles into one follow-up, preserving newer local save errors through late daemon replies. Targeted checks pass 18 appearance tests (one ignored), 30 export tests (one ignored), and 28 image-save tests. A MIDI studio clock mismatch was also corrected: export completion and footer display now use the same existing studio clock, with a visibility regression test. Logs: `/tmp/deep-appearance-tests.log`, `/tmp/deep-export-tests.log`, `/tmp/deep-image-save-tests.log`, `/tmp/deep-midi-retention-final.log`. All 24 MIDI tests and both desktop binary builds pass (`/tmp/deep-midi-retention-final.log`, `/tmp/deep-export-build.log`).
- **Admission follow-up:** retained daemon mutations must not exhaust the ordinary preview worker budget during a daemon stall. Audit the explicit caller-supplied queues and default mutation admission before closing phase 1; retain bounded admission and do not lose accepted completions. Rolling-audio slice preparation/copy remains synchronous and still requires migration.

- **Phase 1, mutation-worker isolation:** retained desktop daemon requests now use a dedicated instance of the existing bounded executor (two workers, sixteen queued operations). Default requests, image/optimized-image/diff copies, MIDI exports and audio slice handoffs use this budget; ordinary preview jobs remain on their existing pool. Admission is nonblocking and queue rejection occurs before execution; accepted work retains its existing result and lifetime contracts. A controlled test occupies both workers and all queue slots, verifies that excess work never executes, runs a preview independently, then releases the workers and verifies every admitted completion. The isolation test, 56 copy-related tests and both desktop binary builds pass (`/tmp/deep-mutation-isolation.log`, `/tmp/deep-mutation-copy.log`, `/tmp/deep-mutation-build.log`). Architecture comparison reports 1517 clone groups versus the original 1516 and unchanged 16 cycles; normalized-fragment inspection finds only earlier modified test/context and handoff fragments, with no added clone in this slice. No new executor implementation or dependency was introduced. Rolling-audio copy and final phase 1 integration remain outstanding.

- **Phase 1, rolling-audio copy:** removed synchronous slice creation and daemon handoff from the UI path. The strip uses the existing guarded audio slice plan/exporter, with pinned input/output handling and collision-safe output creation, on the isolated mutation executor. Its host polls retained completion before modal/view-specific rendering; repeated copies are rejected while pending, stopping playback leaves the copy intact, and pending work defers close. Successful copies omit the old unconditional success toast; daemon failures reach the strip feedback. Read-only snapshot IPC now has a five-second socket timeout. The controlled rolling test exercises real stereo extraction and verifies exact selected sample values, then delays the reply to check pending-close behavior, repeat suppression, failure publication and artifact retention. The rolling suite passes 14 tests (one live-daemon test ignored), recorded in `/tmp/deep-rolling-copy-final.log`. Architecture comparison with `deep-mutation-isolation` reduces clone groups by one with unchanged cycle totals; the host update gains one necessary pending-copy polling branch, and reported cycle differences reorder members in unchanged Python files. Shared image-session retirement and final integration remain to be audited before closing phase 1.

Rolling-copy host verification passes all 96 workbench lifecycle/layout tests (`/tmp/deep-rolling-host-final.log`), and both desktop binaries build (`/tmp/deep-rolling-build.log`). The earlier `workbench::tests` filter selected zero tests and is not counted as evidence. Formatting and diff checks pass; free disk remains about 162 GiB.

- **Phase 1, image retirement:** the workbench now waits for all image save/copy completions before discarding or replacing its current image session, including unedited images and encoded copies that never borrow the canvas. Snapshot restoration alone was insufficient: a pending delivery could lose its owner even though its pixels were already available. The shared photo collection retains its existing session-cache behavior. A regression test holds an encoded-copy delivery, verifies host retirement is deferred, publishes a controlled failure and then permits retirement. All 29 image-save tests pass (`/tmp/deep-image-retirement.log`). Clone/cycle totals remain unchanged from the preceding slice.
- **Remaining IPC audit finding:** waveform-image and spectrum-image copy in `audio_preview/export.rs` still render/encode and synchronously call the daemon from UI actions, then ignore handoff errors and publish success. Migrate these through retained worker completion and verify their close/navigation ownership before declaring phase 1 complete. CLI `--last` calls occur before any UI opens and require separate startup assessment, not automatic conversion into UI jobs.

Image-retirement integration also passes all 96 workbench tests and both desktop builds (`/tmp/deep-image-retirement-host.log`, `/tmp/deep-image-retirement-build.log`); formatting and diff checks pass.
