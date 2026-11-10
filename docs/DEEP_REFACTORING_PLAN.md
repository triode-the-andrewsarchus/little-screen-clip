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

Evidence anchors: [entrypoints/library](../crates/clip-workbench/src/lib.rs), [file-manager entrypoint](../crates/clip-file-manager/src/main.rs), [search runtime](../crates/clip-search/src/runtime.rs), [model helper documentation](../scripts/search_model/README.md), [ONNX runtime](../crates/clip-studios/src/onnx_runtime.rs), [SigLIP](../crates/clip-file-manager/src/file_manager/gallery/similarity/siglip.rs), [SVG thumbnails](../crates/little-clip-core/src/item.rs), [screen dimensions](../crates/little-clip-render/src/lib.rs), [shared waveform mechanisms](../crates/little-clip-core/src/audio/waveform.rs).

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

Start from [file_format.rs](../crates/little-clip-core/src/file_format.rs), core MIME/extension helpers, decoder hooks, the existing preview caches and [file_types.md](file_types.md). Avoid inventing a second complete format registry.

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

The user authorized completing this plan after the review and future-roadmap additions. Execute the phases incrementally, retaining the specific design and adoption gates above. The initial architecture baseline is `deep-refactor-start`; the working tree began at `4fabcc3`, with 164 GiB available. Phase 1 is complete against its confirmed blocking/lifecycle worklist: mount discovery, USB ownership, retained UI mutations, clipboard routes, studio handoffs, worker isolation and image retirement have been verified. Phase 2 has delivered shared peak-range queries, direct document hints, source/readiness contracts and the presentation inventory. Phase 3 has delivered shared UI, neutral studios and independent application ownership. Phase 3a has delivered the opt-in native mirror prototype and its measured findings; the raster mirror remains the default. Phase 4 has delivered the headless search backend, including source readers and coordinator. Remaining domain/state-transition audits and phase 6 status/clock/artifact work are outstanding; native mirror adoption retains its separate review gate.


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

- **Phase 1, audio-image delivery:** waveform and spectrum copies now capture shared analysis snapshots and render/encode on the isolated retained-request worker. Both share one submission/completion path, validate daemon replies and omit unconditional success feedback. The daemon already publishes accepted images to the system clipboard, so the duplicate detached clipboard threads were removed. Studio close guards cover image copies, slice copies and saves; pending image results survive playback stops and analysis replacement. Shared `PendingRequest::poll` now owns close deferral/repaint for these copies, rolling copies and workbench mutations. Tests cover both generated PNG types, captured analysis, duplicate suppression, Escape during pending delivery and delayed daemon rejection; the existing waveform palette/signal test remains. All 101 audio tests pass (`/tmp/deep-audio-image-final.log`); an initial fixture omitted its second stereo channel and was corrected before the passing run. Architecture comparison reports 1515 clone groups versus 1517 at `deep-mutation-isolation`, with unchanged 16 cycles; the remaining high-complexity delta is the preceding rolling-copy polling branch in host update, not a new audio-image branch. Full workbench integration passes 2065 tests (23 ignored), and both desktop binaries build (`/tmp/deep-phase1-workbench.log`, `/tmp/deep-audio-image-build.log`). Formatting and diff checks pass.

The private Weston/Niri reload-error run passes (`/tmp/deep-phase1-reload-ui/report.json`), including the injected daemon failure and fixed, centered first/final window geometry. It used an isolated daemon/runtime and did not access the interactive compositor or clipboard. Workspace-wide tests and Clippy remain integration gates; the complete deep refactor is still in phase 1, with phases 2–6 outstanding.


### Phase 1 integration gate

At `2162da1`, serial `cargo test -j4 --workspace -- --test-threads=1` with the configured OCR fixture passes **2604 tests, zero failures, 29 ignored**, including crate integration tests and doctests (`/tmp/deep-phase1-workspace.log`). Both desktop binaries and the private reload-error/window-geometry acceptance were already verified for this revision. Ordinary `cargo clippy -j4 --workspace --all-targets` stops on the pre-existing `clippy::approx_constant` denial at `creative_fx/cmyk_halftone.rs:144`, introduced by `ca162b6`; the diagnostic pass allowing only that lint completes (`/tmp/deep-phase1-clippy.log`, `/tmp/deep-phase1-clippy-diagnostics.log`). Existing warnings remain; this is not a claim of a warning-free workspace or a successful `-D warnings` gate. No new diagnostics appear in the new shared IPC/appearance mechanisms or the migrated audio-image path. Formatting passes; disk remains about 162 GiB free. No hardware or interactive compositor input was used.

### Phase 2 selected pilot

The `deep-presentation-start` baseline records 1515 clone groups and 16 cycles. Core `FullAudioAnalysis::query_peaks` has CCN 27/NLOC 146, including three copies of min/max/mean-color reduction; rolling audio separately selects the same 64/1024/16384-frame pyramid levels. Share only borrowed peak-block range selection and consolidate core's reduction. Preserve core's average coloring, rolling's strongest-block coloring, core's sample-level detail, existing decode/cache ownership and prepared summaries. Check level boundaries, incomplete/empty summaries, representative query timings and both consumers. This prepares a reusable allocation-free summary boundary for raster/native consumers without a new cache or UI controller. The broader classification/capability and prepared-content work remains part of phase 2.

- **Phase 2, peak-summary reuse pilot:** core and rolling audio now share `peak_blocks_for_range`, borrowing the existing 64/1024/16384-frame summaries. Core's three repeated reductions are one loop; its mean color and raw-sample detail remain, while rolling keeps strongest-block color. Empty/reversed/out-of-range queries are bounded, rounding cannot overflow, and incomplete PCM does not panic. The helper does not decode, allocate or own a cache. Core's `query_peaks` drops from CCN 27 to 14; clone groups drop from 1515 to 1509, with 16 unchanged cycles. The analyzer's overall regression label comes from reordered members of unchanged Python cycles; the intended hotspot is explicitly resolved. Boundary/readiness tests pass with the 36-test audio group (one ignored), and the earlier rolling suite passes 14 tests (one ignored). An isolated optimized harness compares exact output tuples for 50,000 representative ranges against the original source and seven alternating 5M-query timing rounds: medians 71.823 ms before and 74.278 ms after (+3.42%, about 0.49 microseconds per 1000 columns). This is a small query cost, not a claimed frame-time speedup; shared prepared data and substantial duplication/complexity reduction are the benefit. Harness, source snapshot and timings are in `/tmp/deep-peak-benchmark.py`, `/tmp/deep-waveform-before.rs`, `/tmp/deep-peak-equivalence.rs`, `/tmp/deep-peak-timings.txt`. Acceptance checks pass: 36 core audio tests (one ignored), the full 2065-test workbench suite (23 ignored), both desktop builds, eight core doctests, generated API documentation and normal core/all-target Clippy (existing warnings remain). Logs: `/tmp/deep-peak-core-final.log`, `/tmp/deep-peak-workbench.log`, `/tmp/deep-peak-build.log`, `/tmp/deep-peak-doctest.log`, `/tmp/deep-peak-doc.log`, `/tmp/deep-peak-clippy.log`. The rendered API page was inspected. Private audio loading/layout/return acceptance also passes (`/tmp/deep-peak-audio-ui/report.json`); its silent fixture is a navigation/layout check, while synthetic signal tests and the direct old/new comparison verify sample/color behavior. Classification/capability and the broader prepared-content boundary remain outstanding in phase 2.


- **Phase 2, classification and prepared-content audit:** studio dispatch now carries the existing `document_format::Format` directly, removing nine duplicate enum variants and their conversion function. Parser dispatch, precedence, opening limits and guarded loading remain explicit. Contract tests verify that ambiguous XML is admitted as a multisample only after positive parsing in both studio and Shelf workers, ordinary XML stays text, and a protected XML alias cannot supply a prepared document. A raster contract test distinguishes pending/unavailable/ready audio and verifies that a prepared waveform renders identically when its source path changes to a protected node, without opening that source. The existing `AudioWaveform`/`AudioRef` model is the prepared audio boundary; no second registry, cache or framework was added. [presentation_boundaries.md](presentation_boundaries.md) records the family-by-family capability inventory, current source/readiness ownership, and legacy `FileBatch` preparations that must remain on workers during native mirror prototyping. The audit preserves the later-phase location/provenance and read/write capability distinctions. Validation passes 20 viewer tests, five Shelf preview tests, 12 multisample tests (one ignored), six raster-card tests and both desktop builds (`/tmp/deep-classification-viewers.log`, `/tmp/deep-classification-previews.log`, `/tmp/deep-classification-multisample.log`, `/tmp/deep-classification-raster.log`, `/tmp/deep-classification-build.log`). Architecture clone/cycle totals remain 1509/16; overall CCN grows by two from contract tests, with no new high-complexity production function. Reported Python cycle changes only reorder unchanged members. End-to-end native-versus-raster performance/resource comparison remains a phase 3a gate; the small isolated query cost from the preceding pilot remains disclosed.

The classification slice's diagnostic Clippy pass also completes, allowing only the already recorded approximate-constant lint (`/tmp/deep-classification-clippy.log`). Shared UI extraction can now proceed independently while phase 3a retains the full presentation/performance comparison gate.


- **Phase 3, shared UI boundary in progress:** generic controls, tabs, SVG adapters, appearance and font setup move to `clip-ui`, with temporary workbench reexports retaining caller paths. The old application-owned widget sources are removed rather than compiled through cross-crate paths. Native-dialog discovery and shared audio-device ownership remain in desktop runtime code. Core owns shared fallback byte constants; render reexports them, so UI has no raster/scene/application crate dependency. Canonical font/SVG assets and original licenses remain under render; hashes of all 109 former workbench asset files match their canonical replacements (`/tmp/deep-ui-asset-hashes.json`). The font fixture script follows the canonical path. Metadata confirms `clip-ui → core`; `clip-file-manager → clip-workbench` still remains and is not claimed resolved (`/tmp/deep-ui-metadata.json`). Core's broad existing media/document dependencies remain a build-cost limit. Both desktop/all-target checks pass (`/tmp/deep-ui-check-final.log`). Architecture function/CCN totals are unchanged, clone groups fall from 1509 to 1508 and cycles remain 16. The reported new CCN-18 tab `show` is byte-identical to its old workbench implementation. This extraction gives shared UI its own dependency/compilation owner; it is not a claim that relocating code reduced its logic complexity. Acceptance passes: 2609 workspace tests (29 ignored), both desktop builds, both desktop/all-target checks and normal UI-only/all-target Clippy (existing warnings remain). Logs: `/tmp/deep-ui-workspace.log`, `/tmp/deep-ui-build.log`, `/tmp/deep-ui-check-final.log`, `/tmp/deep-ui-clippy.log`. API documentation builds (`/tmp/deep-ui-doc.log`). Studio/search extraction and native mirror prototyping remain outstanding.


Shared UI private acceptance passes in both appearances from an alternate working directory (`/tmp/deep-ui-dark/report.json`, `/tmp/deep-ui-light/report.json`), retaining fixed centered first/final geometry. The file-manager ordinary-preview regression passes font/binary/text/image rows, studio return with selection preserved, exact captured-content creation and collision rejection (`/tmp/deep-ui-files-verified/report.json`). Its stale total-count labels, Shelf coordinate and back-button coordinate were corrected to current UI evidence; fixture count is checked on disk and existing byte/collision assertions remain. Earlier failed runs are retained in `/tmp/deep-ui-files*` as runner-drift evidence, not counted as passes. A cached UI-library check took 2.16 seconds in this environment (`/tmp/deep-ui-warm-check.log`); this is one warm sample, not a clean-build promise. The dependency inventory is `/tmp/deep-ui-tree.txt`. Disk remains about 160 GiB free. Worker CLI dispatch, helper packaging and application entrypoints are unchanged; canonical asset includes work independently of launch directory. Shared studio/session extraction remains the next substantive application-boundary step.

### Phase 3 checkpoint: neutral studio owner

Shared studios, preview/export sessions, shell controls and barcode/conversion
workers now live once in `clip-studios`, using egui directly without an eframe or
application dependency. Workbench forwards existing paths while retaining its
popup, physical mirror, rolling strip and navigation. Core owns guarded
`FileSource`/`ReadCheck`; their source and atomic-write logic are unchanged.
Explicit `test-support` features expose narrowly scoped host fixture helpers;
normal builds omit them. Film Look compute and ONNX CUDA support remain enabled.

The browser and its search/system/transfer/navigation modules still reside in
workbench. Removing file-manager's remaining workbench dependency is outstanding;
this checkpoint does not claim that phase 3 is complete. Current-folder state
remains browser-owned, preparing later views/Home without introducing a location
framework. Accepted writes remain session-owned; host retirement tests retain
pending-copy completion even when the canvas has no edits.

Validation: serial workspace tests pass (2609 passed, 29 ignored), both desktop
binaries build, and both hosts plus studios pass all-target compilation. The new
file-manager executable test additionally exercises barcode and conversion
worker dispatch from another working directory. Logs:
`/tmp/deep-studios-workspace.log`, `/tmp/deep-studios-build.log`,
`/tmp/deep-studios-check-accepted.log`, `/tmp/deep-studios-dispatch.log`.
Studio API documentation builds. Diagnostic all-target Clippy passes with the
existing approximate-constant diagnostic allowed; existing warnings remain
(`/tmp/deep-studios-clippy.log`). Private dark file-manager preview/return/Shelf
collision acceptance and light/dark component galleries pass, including launching
the galleries from `/tmp` (`/tmp/deep-studios-files/report.json`,
`/tmp/deep-studios-gallery-{light,dark}/report.json`).

Architecture comparison against `deep-studios-start` reports REGRESSION: clone
groups 1508→1511, measured NLOC +78, functions +5, total CCN +7; average/max CCN,
duplication percentage and cycle count are unchanged. Rename-aware source review
shows no new production algorithm or duplicated implementation: the residual
changes are boundary visibility, explicit fixture bridges and executable dispatch
testing, alongside repeated Cargo dependency declarations. The added dispatch
test uses one table-driven path for both workers. Lizard comparison of 181 moved
files finds unchanged function complexity except new CCN-1 accessors/fixture
bridges, a relocated host test and fixture functions, and equivalent signature
spelling (`/tmp/deep-studios-symbol-comparison.json`). This is an ownership
improvement, not a claimed reduction of studio algorithm complexity or a clean
architecture-tool score. Disk remains about 155 GiB free; no cache sweep was
needed during this extraction.

### Phase 3 checkpoint: independent application ownership

The browser now lives in `clip-file-manager`, including navigation, selection,
Shelf, transfers, system observations and the existing search implementation.
Its 286 Rust source files are byte-identical to the previous workbench-owned
files. Both desktop applications consume `clip-studios`/`clip-ui` directly;
neither application depends on the other. Workbench direct production dependency
entries fall from 51 to 19; browser-only dependencies move with their actual
owners. This proves dependency isolation, not a proportional reduction in the
transitive graph or a clean-build speedup. Local ownership snapshot:
`/tmp/deep-browser-local-dependencies.json`.

Barcode/conversion dispatch belongs to studios and remains enabled in both
executables. File-manager dispatch owns search/extraction, system and FUSE
workers. Actual executable tests check shared worker output from another working
directory and rejection of malformed browser-worker arguments before window
startup. Existing workbench executable tests retain barcode/pixel equivalence.
Browser fixtures and their private acceptance/generation scripts moved together;
search benchmark/quality/mount tools now select the file-manager test harness.
The mount runner reuses the existing verified-harness selector rather than
risking execution of a GUI artifact. Current source links and roadmap ownership
references follow the delivered boundary.

Serial workspace tests pass (2610 passed, 29 ignored), both desktop binaries
build, and the updated executable dispatch suite passes both tests. All-target
checks pass; diagnostic all-target Clippy passes with existing warnings and only
the known approximate-constant diagnostic allowed. Logs:
`/tmp/deep-browser-workspace.log`, `/tmp/deep-browser-build.log`,
`/tmp/deep-browser-check-pruned.log`, `/tmp/deep-browser-dispatch.log`,
`/tmp/deep-browser-clippy.log`. The two Python harness-selection tests pass.
The explicit private user/mount-namespace search safety test passes
(`/tmp/deep-browser-mounts.json`).

Private UI acceptance passes for ordinary font/binary/text/image previews,
selection-preserving studio return, exact captured Shelf content and collision
handling (`/tmp/deep-browser-files/report.json`), and the workbench component
gallery launched from `/tmp` (`/tmp/deep-browser-gallery-dark/report.json`).
Search-to-model/PDF acceptance passes, including exact selected PDF, no automatic
index enrollment, deferred source revalidation and no replacement payload access
(`/tmp/deep-browser-studio-source-verified/report.json`). That runner's obsolete
Back-button coordinate was replaced with visible-text targeting; its substantive
source-access assertions remain intact.

Architecture comparison against `deep-browser-start` reports REGRESSION from
move-sensitive function attribution. Clone groups remain 1511, dependency cycles
remain 16, average/max CCN and duplication percentage are unchanged; measured
NLOC +69, functions +2 and total CCN +2 reflect worker dispatch separation and
its executable test. The browser bodies are byte-identical, so their reported
new complexity is not newly added logic. No metric improvement is claimed for
relocation. Current-folder assumptions, location/history and input routing stay
in browser composition; studio and mutation lifetimes remain with their existing
owners. Later Home/views/rules can reuse these boundaries without depending on
the popup application. Native mirror prototyping, search extraction, selected
hotspot simplification and remaining targeted hygiene are still outstanding.

The full private multisample acceptance also passes all 68 observations:
inline velocity and MIDI/sustain, mapping/harmony/pads, missing-note indication,
zoom, guarded rename, SFZ/SF2/NKI/DecentSampler/EXS/Renoise and packed/archive
sources, native compressed audio, modern NKI mapping, and ordinary XML/Elixir
fallbacks (`/tmp/deep-browser-multisamples-verified/report.json`). The first run
exposed a test setup omission: the existing separately licensed helper was
packaged beside the release executable but absent beside the debug executable.
Packaging it with the existing `build_multisample_helper.py --skip-build
--output-dir target/debug/nki-helper` retained corresponding source/licenses;
no parser code or application linkage changed. About 152 GiB remains free.

### Phase 3a checkpoint: native mirror prototype and findings

An opt-in native Quick Palette mirror now consumes prepared text runs, waveform
summaries, labels and colors. `LITTLE_CLIP_NATIVE_MIRROR=1` enables Native and
Display reference tabs on a fresh workbench process. Normal launches retain the
raster mirror. Hardware rendering and the exact reference remain operational;
no native default adoption is included. Usage and data contracts are documented
in [presentation_boundaries.md](presentation_boundaries.md#opt-in-native-mirror-prototype).

The raster worker returns its existing highlighted runs alongside its reference
pixels; there is no second file probe or syntax pass. Native painting performs
no content I/O. Structured text, images, Markdown, references and other rich
cards use that same raster result. Oversized display data also retains its
raster fallback. Shared core palette policy supplies slot accents, and mirror
geometry derives from render's screen constants. The existing widget owns all
pointer routing in both modes.

The native model is prepared on the worker and contains bounded display data,
not a retained clipboard `Arc`, raw image bytes, paths/handles or PCM. An ownership
test proves that holding the model does not force clipboard copy-on-write.
Prepared audio amplitudes/colors are copied without reanalysis or normalization;
no new decoder, player, cache or queue is introduced. Existing snapshot/selection
and appearance checks publish raster/native results together. During a mode
change, an appearance stamp retains the coherent raster view until native colors
are ready. Tests cover byte-identical reference pixels, structured fallback,
retained prepared text after source deletion, bounded text, slot clicks across
both layouts/two widths, equivalent drag swaps, and rejected stale native runs.

Final integration passes: **2616 workspace tests, 30 ignored**
(`/tmp/deep-native-integration-final.log`). The newly ignored test is the explicit
CPU comparison, which was also run successfully. Final focused workbench tests
pass (151 passed, 2 ignored), renderer tests pass (34 passed, 1 ignored), both
desktop binaries build, workspace/all-target compilation passes, and API docs
build. Logs include `/tmp/deep-native-bounded-final-tests.log`,
`/tmp/deep-native-render-accepted.log`, `/tmp/deep-native-desktops-accepted.log`,
`/tmp/deep-native-bounded-final-build.log`, `/tmp/deep-native-all-targets.log` and
`/tmp/deep-native-doc.log`. Diagnostic Clippy passes with the known approximate
constant diagnostic allowed; pre-existing warnings remain
(`/tmp/deep-native-bounded-final-clippy.log`).

Private Weston/Niri captures verify mixed content, fixed popup/single-instance
behavior, dark/light modes, HiDPI and mode switching. Current bounded-model
artifacts are `/tmp/deep-native-bounded-dark/report.json`,
`/tmp/deep-native-bounded-hidpi/report.json` and
`/tmp/deep-native-bounded-theme-switch/report.json`. The theme-switch fixture's
mock daemon deliberately has no ReloadConfig implementation; its expected error
remains visible while the new light presentation is verified. The async audio
readiness/hydration check passes (`/tmp/deep-native-audio-transition/report.json`).
The runner explicitly checks completed mirror content; an earlier capture of a
still-preparing window no longer counts as visual acceptance. HiDPI OCR tolerates
whitespace within recognized filenames without weakening the required content.

The reproducible headless comparison uses a 512-bin prepared stereo waveform,
source text, a color card and raster fallback. These are **debug-profile CPU
paint/tessellation measurements**, excluding GPU presentation and physical input
latency (`/tmp/deep-native-bounded-measurements.log`):

| Logical host width / density | Raster median / p95 | Native median / p95 |
| --- | --- | --- |
| 900 / 1× | 0.0034 / 0.0035 ms | 0.0948 / 0.1001 ms |
| 900 / 2× | 0.0035 / 0.0035 ms | 0.1625 / 0.1697 ms |
| 1800 / 1× | 0.0034 / 0.0034 ms | 0.1323 / 0.1379 ms |
| 1800 / 2× | 0.0035 / 0.0035 ms | 0.2424 / 0.2513 ms |

Matched reference preparation medians were 13.225 ms without text collection and
13.223 ms with it: this run establishes no meaningful preparation improvement.
Native tessellation uses 2860–9168 vertices versus 12 for the cached reference.
The fixture's text string capacities total 192 bytes, excluding vector/model and
font-atlas storage; the retained reference remains 3,548,160 RGBA bytes. Sampled
private-process RSS was roughly 505–507 MiB at 1× for raster/native runs and about
538 MiB for the current native HiDPI capture; these separate-run samples do not
prove a memory saving. No physical-input or GPU-latency improvement is claimed.
The shared handler and event tests establish equivalent application-side input
routing, not an end-to-end hardware latency bound.

The prototype validates the shared preparation seam and gives a reviewable
clarity/interaction comparison. It is not a drop-in visual replica: native font
metrics/wrapping and waveform painting differ, rich cards remain raster, and
raster diagnostics have not been recreated as native components. The reference
is still prepared and retained, so startup and memory savings require separate
work and measurements if native adoption is approved. Search extraction and the
remaining planned simplification/hygiene work are still outstanding.

The final bounded-model Display reference switch also passes
(`/tmp/deep-native-bounded-reference-switch/report.json`), as does final
workspace/all-target compilation (`/tmp/deep-native-all-targets-final.log`).
Architecture comparison with `deep-native-mirror-start` reports REGRESSION:
measured NLOC +989, functions +23, total CCN +91; clone groups stay 1511,
dependency cycles stay 16, and average/max CCN stay 4.06/195. The lower measured
duplication percentage (4.48→4.46%) includes denominator growth and is not evidence
of removed clone groups. The localized complexity increases are optional run
collection in `draw_text_preview` (25→28) and the native/reference worker branch
in `update_mirror_texture` (14→16). These explicit branches preserve the normal
raster path without collecting native data; they add no format parser or decoder
path. The added code implements the approved prototype, its bounded-data/input
contracts and reproducible acceptance/measurement fixtures; it is not presented
as an algorithm-complexity reduction. Slot accent policy has one canonical owner.
No new dependencies, Cargo feature configurations or external runtimes were
introduced. About 151 GiB remains free. Native default adoption, real GPU/input
latency measurements and any further visual redesign remain separately
reviewable; the next structural milestone is search extraction.

### Phase 4 checkpoint: search runtime and notification boundary

`clip-search` now owns the existing external model-process runtime and exact
size/date ranges. It has no egui or application dependency. Backend preference,
startup/exchange deadlines, cancellation identity, nonblocking input, bounded
replies, query-vector reuse, recovery and child kill/reap behavior are unchanged.
Helpers still resolve relative to the executable/packaged install, with the same
repository fallback depth after the move. Public API docs describe these worker
contracts. The existing 768-component protocol constant now has one search
library owner; persisted dimensions, schema versions, SQL and ranking are intact.

The browser service stores a plain completion callback instead of an egui
context. The browser caches its egui adapter once and registers shared callback
handles. Publication, registration of an already available result and observer
teardown all occur outside the snapshot lock. The headless contract test checks
completion-before-registration, completion-after-registration, replacement of an
observer and request-scoped progress. Host progress-painting tests remain with
the browser. No generic event bus, model backend trait or connection pool was
introduced.

This is a partial extraction. Service/store/retrieval, source policy, extraction
and browser composition still use the existing guarded entry/root descriptors
in file manager. Those guards have not been weakened or replaced with path-only
records. Their shared source boundary is the next step; this checkpoint does not
claim that the complete search backend has moved.

Validation passes: 2617 serial workspace tests, including the new range API
doctest (30 tests remain ignored), workspace/all-target compilation, both desktop builds,
search API documentation and diagnostic Clippy with the previously known
approximate-constant diagnostic allowed. Existing warnings remain. Logs:
`/tmp/deep-search-foundation-workspace-accepted.log`,
`/tmp/deep-search-foundation-check.log`, `/tmp/deep-search-foundation-build.log`,
`/tmp/deep-search-foundation-doc.log` and `/tmp/deep-search-foundation-clippy.log`.
The 13 new-crate unit tests include controlled model startup/failure/recovery,
cancellation, reaping, vector validation and exact size/DST bounds. The calendar
test's self-spawned exact selector was updated to its new test-harness path.
After normalizing documentation, visibility, the dimension import and that test
selector, both moved source bodies match their originals exactly.

Private acceptance passes for command controls, explicit scopes, exact UTC/date
bounds, invalid-draft retention and protected/private/repository/kernel-alias
exclusions without a database/model (`/tmp/deep-search-foundation-controls/report.json`).
The actual installed models also pass private literal-content/recursive search,
guarded enrollment, persisted embeddings, stale-source omission, semantic
ranking, typed session answers and return to the embedding role
(`/tmp/deep-search-foundation-real-search/report.json`). Only owned fixtures and
a private index were used; existing runtime/model files were reused, not downloaded.

Comparison with `deep-search-start` reports REGRESSION from the UI adapter's two
closure expressions: `search_ui` CCN 15→17. Overall functions stay 10593, total
CCN changes by +2, measured NLOC by +11, clone groups stay 1511 and cycles stay 16;
average/max CCN and duplication percentage are unchanged. No production parsing,
ranking or process algorithm changed. The callback closures add an actual
notification boundary and cache its host adapter; the analyzer's count is not
represented as a simplification. Core's broad existing dependency graph remains
a build-cost limit. About 150 GiB remains free.

### Phase 4 checkpoint: guarded source ownership

`clip-search` now owns the existing enrollment policy, pinned `Root`,
`SourceSession` continuity observation and serializable `SourceRoot`. Its source
boundary retains device/inode/mount identity, optional boot/namespace/unique-mount
continuity and lossless path bytes. Legacy records without mount identity remain
readable but cannot authorize content access. Validation still obtains a current
admitted root and retains its directory handle. Alias/mount crossings, root
replacement and sensitive paths retain their original behavior. Decoder requests,
result availability, row revisions and payload extraction remain in the browser;
this is not completion of backend extraction.

Core now owns the existing metadata-only `MountIdentity` and `Namespace`
mechanisms, shared by search continuity and browser system inspection. It also
owns the existing reversible address codec and lossless path serde adapters.
UTF-8 paths remain JSON strings and other Unix paths remain byte arrays; optional
and list forms retain the same representation. Browser navigation still owns
relative/home resolution. Existing browser module aliases keep serde attributes
and system consumers on these canonical implementations without copying sources.
No new identity model, schema migration, protocol, decoder or process was added.

The moved implementation bodies match the originals after normalizing ownership
imports, visibility, documentation and formatting (`/tmp/deep-source-body-comparison.json`).
New boundary tests verify exact text/byte/list/optional wire representations and
source descriptor round trips, missing legacy mount refusal, changed mount refusal
and root replacement. Existing source-session, protected-path, bind-boundary,
recovery and browser system tests continue to exercise the same implementation.

Validation passes: 2620 serial workspace tests (30 ignored), workspace/all-target
compilation (`/tmp/deep-source-foundation-check-final.log`), both desktop builds,
shared core/search API docs and the private user/mount-namespace bind-boundary
check. Logs are `/tmp/deep-source-foundation-workspace.log`,
`/tmp/deep-source-foundation-build.log`, `/tmp/deep-source-foundation-doc.log`
and `/tmp/deep-source-foundation-mounts.json`. The three additional passing tests
are two source/wire-boundary regressions and one runnable address API doctest.
Existing browser source-reader and system identity tests exercise the moved
implementations through the host's compatibility imports.

The final comparison with `deep-source-foundation-start` is NEUTRAL:
clone groups remain 1511, cycles 16, duplication 4.46%, average CCN 4.06 and maximum
CCN 195. Measured NLOC grows by 91, functions by two and total CCN by three from
the new tests; no production algorithm changed. An earlier comparison labeled
reordered members of unchanged Python cycles as new/resolved cycles; the final
comparison reports none (`/tmp/deep-source-foundation-architecture.json`). The
actual benefit is removing source policy/continuity's browser-system ownership
and keeping a canonical guarded source descriptor outside the application, not
an algorithm-speedup or reduced-build-cost claim. About 149 GiB remains free.
Diagnostic Clippy also passes with the pre-existing approximate-constant diagnostic
allowed; existing warnings remain (`/tmp/deep-source-foundation-clippy.log`).


### Phase 4 checkpoint: shared inventory, storage and retrieval

`clip-search` now owns the existing query grammar, literal/progressive filename
ranking, transactional SQLite store (including recovery/import/publication),
bounded vector and lexical caches, rank fusion, explanations and streaming
literal matching. The cluster moved together so its transaction/cache/cancellation
contracts remain intact. Its sole local crate dependency is core; it has no egui
or application dependency. Browser coordination, source payload extraction and
internal-reader process dispatch remain application-owned. This checkpoint does
not claim the entire search coordinator/extraction subsystem has moved.

Core now owns the existing `Entry`, `Directory` and `Identity` snapshots and
worker-side enumeration, with their exact metadata/revision/availability and
lossless path representation. Existing formatted inventory fields remain for
persisted compatibility. Source admission still precedes content access; an
inventory snapshot or filename hint never substitutes for a pinned root/file.
The existing cheap semantic-document registry and text/TypeScript probe have one
core owner. Scene/MIDI path-hint exports and browser compatibility imports use
those canonical implementations. Eight obsolete `known` wrappers were removed;
module classification assertions use the shared registry. Decoder/player owners,
preview caches and ambiguous-format admission are unchanged.

Browser-specific sort conversion now lives with browser sorting; input focus and
progressive-key event precedence remain in the browser. Restore input, space,
source-identity checks and private staging remain owned by the store, while a
function parameter supplies the application's existing disposable-reader
launcher. The same inherited handles, byte/vector/page limits, cancellation,
protocol and process lifecycle still reach `prepare_in_helper`. The new actual
executable integration test exports a populated backup, changes indexed text,
prepares/restores through the application reader, verifies original text/vector
restoration and checks the backup bytes remain unchanged. The no-overwrite
rename syscall used by recovery and browser mutations has one core file-source
owner; callers retain their existing safety admission and directory pins.

The explicit search `test-support` feature owns shared fixtures, separate from
unit-test modules. Backend tests moved with their implementation; browser
orchestration tests consume only that fixture API. Benchmark/quality/scale
harness selectors and source hashes follow the moved implementation, while
private mount checks continue to select the browser reader harness. Both runner
selection tests pass. No database migration, schema/dimension/extraction-version
change, SQL/ranking change or new connection/model abstraction was introduced.

Validation:

- The serial workspace suite passes 2620 tests (30 ignored) in
  `/tmp/deep-search-storage-workspace.log`. The actual executable worker suite
  then passes all three tests, including the new restore handoff, in
  `/tmp/deep-search-storage-worker-dispatch.log`.
- Workspace/all-target checks, both desktop builds and core/search API docs pass:
  `/tmp/deep-search-storage-check-final.log`, `/tmp/deep-search-storage-build.log`
  and `/tmp/deep-search-storage-doc.log`. Rendered inventory/query/store/opaque
  restore API pages were inspected. Diagnostic Clippy retains the previously
  known approximate-constant allowance and existing warnings; it is not a
  warning-free workspace claim. Logs: `/tmp/deep-search-storage-clippy-final.log`
  and `/tmp/deep-search-storage-clippy-accepted.log`.
- Private mount boundary checks pass (`/tmp/deep-search-storage-mounts.json`).
  The private real-model application run passes literal/recursive content,
  guarded enrollment, persisted vectors, semantic ranking, stale-source omission,
  typed answers and return to the embedding role
  (`/tmp/deep-search-storage-real-search/report.json`). It reused installed
  models with owned fixtures/private index; no downloads or host input.
- The relocated retrieval runner passes its 1000-file/1500-vector debug component
  check (`/tmp/deep-search-storage-retrieval-fixed.json`), retaining its query-plan
  and cancellation/cache diagnostics. Five-sample p95s range from 1.32 to 39.30 ms;
  this is a functional component smoke measurement, not optimized scale or
  end-to-end performance acceptance. The first runner attempt identified stale
  source-hash paths; those paths were corrected before the accepted run.

A normalized source audit matches all 377 unchanged functions in the moved query,
store, retrieval, explanation and literal-matching modules. The remaining two
functions were reviewed explicitly: the same sort conversion moved to browser
sorting, and restore receives the existing helper launcher through a parameter
(`/tmp/deep-search-storage-body-audit.json`). In particular, the six high-CCN
functions reported as "new" by the move-sensitive analyzer already exist in the
baseline with the same bodies and complexity. The final raw comparison reports
REGRESSION for those moves and modified-file clone findings. Overall clone groups
decrease 1511→1508, functions decrease by five, total CCN increases by two from the
new executable test/launcher and fixture Default after deleting eight wrappers;
average/max CCN and cycle totals remain unchanged. NLOC increases by 123,
duplication changes 4.46%→4.45%; no speedup is claimed from these counts.
`/tmp/deep-search-storage-architecture.json` retains the raw findings. The
ownership/deletion benefit is separate from its coarse classification. Core's
broad transitive dependency graph remains; about 147 GiB was free at the check.

### Phase 4 checkpoint: headless coordinator and source readers

`clip-search` now owns the background service, reconciliation monitor, literal
walk, bounded source readers and typed-answer/data-only extraction cluster. It
also owns the source-aware `Results` contract and preference validation. The file
manager keeps navigation, rendering, command editing and session columns, and
explicitly dispatches the same two internal worker flags to the shared library.
Those entrypoints retain their existing limits/sandbox and must run only in
disposable processes. No protocol, request budget, root/revision check, store
schema, model lifecycle or admission/accepted-maintenance behavior changed.
The backend's sole local crate dependency is core, with no egui or application
imports. This completes the search ownership extraction, rather than adding an
adapter back to browser internals.

Core now owns the existing cancellable `jobs::task` wrapper, shared by browser
previews and search readers. Response deadlines still do not prove worker/child
exit; accepted write owners remain retained until completion. Core also owns
photo metadata/thumbnail/hash preprocessing, keeping the original separate
caches, source lock, revision/identity checks, orientation and embedded-RAW
policy. Browser rows, Shelf, similarity preprocessing and search consume that
single implementation. Source-relative media timing and non-cover video
selection likewise have one core owner. Dependencies follow code with unchanged
versions/features; the broad existing core graph remains a build-cost limit.
Generated audio fixtures moved once to core's explicit test-support API, with
studio forwarding exports. No parallel cache, decoder or worker protocol was
introduced.

The two browser-specific service tests remain in file manager: pending progress
paints only for the current request, and accepted maintenance blocks host close.
Their queued-service fixture is an explicit search test-support API, independent
from unit-test modules. Backend tests moved with their implementation. The
sandbox child's exact test selector and private mount runner now select the
`clip-search` harness. Native scale provenance includes the relocated core
preprocessing/task sources.

Validation:

- 2621 serial workspace tests pass, with 30 ignored
  (`/tmp/deep-search-coordinator-workspace.log`). The executable worker suite
  subsequently passes all four tests, including a new valid YAML inspect/execute
  extraction-plan round trip through the application entrypoint, as well as
  inherited-handle restore and early shared-worker dispatch
  (`/tmp/deep-search-coordinator-worker-dispatch.log`).
- Both desktop binaries, workspace/all-target checks and core/search API docs
  pass (`/tmp/deep-search-coordinator-build.log`,
  `/tmp/deep-search-coordinator-check-final.log`,
  `/tmp/deep-search-coordinator-doc.log`). Rendered service, worker, task and
  photo-hash API docs were inspected. Diagnostic Clippy passes with the existing
  approximate-constant allowance and existing warnings
  (`/tmp/deep-search-coordinator-clippy.log`,
  `/tmp/deep-search-coordinator-worker-clippy.log`); no warning-free claim.
- The private mount test passes (`/tmp/deep-search-coordinator-mounts.json`).
  Private real-model search passes content/recursive search, persisted vectors,
  semantic ranking, stale-source omission, typed answers and return to embedding
  (`/tmp/deep-search-coordinator-real-search/report.json`). SVG visual preparation
  and retrieval also pass (`/tmp/deep-search-coordinator-svg/report.json`).
- Private embedding reuse after move/copy/metadata-only changes, changed-input
  inference, orphan retirement and replaced-root refusal pass
  (`/tmp/deep-search-coordinator-previews/report.json`). Separately, ordinary
  font/binary/text/image rows, studio return and collision-safe captured-content
  creation pass in light appearance
  (`/tmp/deep-search-coordinator-ordinary-previews/report.json`). Runs reuse
  installed models with owned fixtures and private application/compositor state.
  No model downloads, host input or physical USB access were used.

The source audit matches 425 moved functions after ownership/import, visibility,
formatting and exact-selector normalization. It additionally matches the full
original photo-hash block and the moved stream-selection/preference-validation
bodies (`/tmp/deep-search-coordinator-body-audit.json`). The analyzer incorrectly
recorded old `parse_dhash` as spanning lines 116–370, swallowing the remaining
module. Correctly discovering those unchanged functions after the move accounts
for exactly +7 functions/+28 CCN in the raw metrics; the new executable extraction
test adds one function/CCN. Raw comparison therefore reports REGRESSION despite
unchanged production algorithms. Final counts: functions +8, total CCN +29,
NLOC +37, clone groups 1508→1509, with average/max CCN, duplication percentage and
cycle totals unchanged. The unmatched clone fragments all describe pre-existing
WAV fixtures whose bodies were retained; moving their owner changes reported
clone pairing/grouping (`/tmp/deep-search-coordinator-clone-review.json`). Keep
the raw findings in `/tmp/deep-search-coordinator-architecture-final.json`; do not
represent them as algorithm simplification or performance improvement. About
142 GiB remained free. Remaining domain/state-transition and status/clock/artifact
requirements still require their own audit before completing the overall plan.
