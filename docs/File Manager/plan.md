# File Manager Implementation Plan

Status: Phase 1 is complete as a standalone application. See [phase1.md](phase1.md) for delivered behavior and validation. Phase 2 is complete and verified; [phase2.md](phase2.md) records delivered behavior, operation contracts, and acceptance results. Phase 3 is accepted by the user as an initial implementation; [phase3.md](phase3.md) records its behavior and remaining follow-up work. Phase 4 is in progress; [phase4.md](phase4.md) records current capabilities, save contracts and verification evidence.

The product goals are recorded in [core.md](core.md). This plan turns those goals into a sequence of usable milestones. The later phases describe intended capabilities, not settled specifications: each needs a short design pass before implementation. Phase 1 establishes the browser foundation; subsequent phases build on it.

## Product direction

The file manager should eventually become the main surface for working with files: browsing, inspecting, playing, editing, transforming, organizing, and moving them. Photo galleries, music players, document browsers, and small editors should become different ways of working within this surface.

The first milestone should look and behave like a familiar file manager. The unusual features should build on a reliable browser, understandable selection, and predictable file operations. Users should be able to use ordinary folders without configuring rules, projects, indexes, or media libraries.

### Working principles

- Keep navigation fast and available while directory reads, previews, searches, or operations run in the background.
- Preserve the user's place when entering and leaving a viewer, changing views, or returning through history.
- Treat mixed folders as normal. A music-oriented view still needs to account for cover images, text files, and subfolders.
- Keep filesystem facts separate from presentation preferences and temporary task markings.
- Make destructive effects and overwrite decisions visible before execution. Report partial success accurately.
- Reuse the existing clipboard, viewers, editors, operation pipelines, colors, icons, and job infrastructure where their contracts fit.
- Add capabilities in independently usable increments. Do not require the final automation system to ship the first browser.

## Roadmap at a glance

| Phase | Milestone | Main outcome |
| --- | --- | --- |
| 1 | Basic folder browser | Back/forward, address bar, folder tree, and directory listing |
| 2 | Ordinary file operations | Selection, create, rename, copy, move, paste, and trash |
| 3 | Shelf and clipboard integration | Hold files and create files from clipboard contents |
| 4 | Rich previews and built-in editors | Inspect supported types and save edits into their source folders |
| 5 | Media browsing and playback | Usable photo gallery, music player, and video browsing workflows |
| 6 | Search | Progressive name matching, recursive discovery, and metadata/content queries |
| 7 | Views and meaningful sorting | Miller columns, smart expansion, mixed-folder views, and metadata sorting |
| 8 | Organization and a real Home | Pins, session states, A/B sorting, visibility rules, and project boxes |
| 9 | File transformations and metadata | Batch pipelines, metadata edits, and content-aware renaming |
| 10 | Rules and contextual workflows | Folder automation, visual rules, and dynamic Home sections |
| 11 | Storage analysis and reconciliation | Duplicates, usage, folder differences, and merge planning |
| 12 | Remote performance and transfer scheduling | Remote-aware execution, terminals, and storage-aware queues |
| 13 | Filesystem editing and Git workflows | Edit directory layouts as text and inspect repository state |

The phase numbers give a default order, not a requirement to finish every feature in one phase before doing useful work elsewhere. Each phase should be split into reviewable slices when implementation begins. No calendar estimates are assigned before the relevant design and repository assessment.

## Phase 1 — Basic folder browser

### Goal

Open the file manager, see the current directory, and move through the folder tree using familiar controls. This milestone is a browsing surface; file mutations begin in Phase 2.

### Application boundary and layout

The file manager is its own application, launched as `clip-file-manager` or `./launch.sh files [DIRECTORY]`. It reuses workbench code and core services and remains tightly coupled to them. Its navigation and application lifecycle are independent. Use an ordinary resizable desktop window; the workbench retains its separate fixed-size, centered, single-instance popup behavior.

```text
+-------------------------------------------------------------------+
| Little Screen Clip — Files                                         |
+-------------------------------------------------------------------+
| Back | Forward | Up | Address: /home/user/Pictures        | Refresh |
+----------------------+--------------------------------------------+
| Folders              | Name                  Size       Modified  |
| > /                  | > Holidays/                                |
| v Home               | > Screenshots/                             |
|   > Documents        |   example.png         ...        ...       |
|   v Pictures         |   notes.txt           ...        ...       |
|     > Holidays       |                                            |
+----------------------+--------------------------------------------+
| Current location / item count / loading or error status            |
+-------------------------------------------------------------------+
```

The wireframe specifies structure, not exact styling. Use the shared Light/Dark palette, fonts, controls, and SVG icons. The tree panel can resize independently of the application window.

### Included behavior

**Directory listing**

- Show immediate children of the current directory in a simple details list.
- Show name and type icon, with size and modification time where available. Do not recursively calculate folder sizes.
- Put folders first, then sort names consistently. Advanced sorting comes later.
- Single click selects a row. Double click or Enter on a folder navigates into it.
- Show an explicit empty-folder state, distinct from loading and failure.
- Provide a hidden-file toggle. Start with dotfiles hidden; hidden ancestors of an explicitly opened path must still be representable in the tree.
- Treat unsupported files as ordinary entries with a generic icon. File viewers and file launching are deferred to Phase 4.

**Folder tree**

- Provide access to the filesystem root and the user's home directory.
- Expand folders on demand and load only the requested children. Do not scan the whole filesystem at startup.
- Keep expansion separate from navigation: the disclosure control expands/collapses; activating a folder opens it.
- Highlight the current location and reveal its ancestor chain when navigation happens through the address bar or history.
- Show permission and unavailable-location errors on the affected branch without breaking the rest of the tree.
- Follow directory symlinks when explicitly opened. Avoid automatic recursive traversal through symlink cycles.

**Address bar**

- Display the current directory as an editable path.
- Enter navigates to an existing directory; Escape restores the displayed current path.
- Accept absolute paths, `~` and `~/...`, and paths relative to the current directory.
- Treat input as a filesystem path, never a shell command. Handle spaces without requiring shell quoting.
- Report an invalid, inaccessible, or non-directory destination without losing the current listing.
- Preserve filesystem paths losslessly internally, including names that are not valid UTF-8. Define a safe display representation before implementation.
- Defer completion, breadcrumbs, search syntax, and remote URI entry.

**History and parent navigation**

- Back and Forward traverse successful directory navigations.
- Navigating somewhere new after going Back removes the abandoned forward branch.
- Refresh and reselecting the current location do not add history entries.
- Failed navigation does not add a history entry or silently change the current location.
- Up opens the parent directory and is disabled at the filesystem root.
- Restore the previous scroll position and selection when returning to a history location, where those entries still exist.
- Keep unavailable history destinations recognizable and provide an error/recovery action if a folder has been removed or unmounted.

**Keyboard and focus**

- `Alt+Left` / `Alt+Right`: Back / Forward.
- `Alt+Up`: parent directory.
- `Ctrl+L`: focus and select the address bar.
- `Enter`: open the selected folder; submit a path when the address bar has focus.
- `F5`: refresh the listing and relevant tree branch.
- `Ctrl+H`: toggle hidden entries.
- Arrow keys move selection in the focused list or tree. Text input receives its own editing keys.
- Controls expose tooltips and clear disabled states.

**Loading and recovery**

- Read directories and metadata through bounded background work. Keep the application interactive on slow mounts and large folders.
- Reject stale results after navigation, refresh, or session changes.
- Keep the existing listing visible while a destination loads; clearly identify the pending destination. Commit the new location and history together after a successful load.
- Distinguish an unreadable directory from an unreadable child metadata field. A missing field should not hide an otherwise visible entry.
- Refresh explicitly after external changes. Live filesystem watching is deferred to Phase 2.
- Use a virtualized list so rendering cost follows visible rows. A full directory listing may initially live in memory; document its measured limits before adding a more elaborate model.

### Explicitly deferred

Copy/move/delete, folder creation, multi-selection, drag and drop, Shelf, previews, playback, search, automatic expansion, Miller columns, rules, indexing, remote connection setup, project metadata, and multiple browsing tabs or split panes.

This scope keeps the first milestone small while establishing behavior that later features can reuse.

### Implementation slices

1. Decide the entry point and define navigation state, location identity, listing results, and error states.
2. Add asynchronous directory listing and the plain file list.
3. Add address entry, Back/Forward/Up, refresh, and history restoration.
4. Add lazy folder-tree expansion and synchronization with the current location.
5. Complete keyboard behavior, hidden entries, and loading/error presentation.
6. Validate the milestone in the private UI runner and record practical limits.

These are planning slices, not an instruction to begin coding.

### Acceptance criteria

- Starting the File Manager shows a useful default directory, proposed as the user's home.
- The user can reach a nested folder via the tree, the list, and a typed path.
- Back, Forward, and Up behave as specified, including forward-history truncation.
- Tree expansion and current-location highlighting remain coherent across all navigation methods.
- Empty directories, permission failures, deleted destinations, symlinks, hidden files, and unusual filenames have defined behavior.
- A late directory result cannot replace a more recently requested location.
- A large folder and a slow directory read do not freeze navigation controls or the application.
- The interface looks like a conventional file manager, shares existing palette, font, icon, and control conventions, and runs independently of the workbench and daemon.

## Phase 2 — Ordinary file operations

### Goal

Make the browser usable for everyday file management.

### Scope

- Multi-selection, range selection, select all, and context actions.
- Create folders and empty files; rename one item; copy, cut, and paste file selections.
- Move to trash, restore where supported, and explicitly distinguish permanent deletion.
- Drag and drop within the browser, then desktop interoperability as a separate slice.
- Conflict handling: replace, skip, or choose another name, with understandable batch choices.
- Background operation progress, cancellation, and an accurate result summary for partial failures.
- Detect external changes with bounded watching or refresh fallback. Update affected listings without discarding unrelated selection.
- Define symlink handling, recursive-copy rules, cross-filesystem moves, and destination-inside-source rejection.

Introduce a simple operation queue here. Phase 12 improves its scheduling; reliable copy/move behavior must not wait for remote optimization. Record which operations can actually be undone and avoid implying that every filesystem action is reversible.

### Completion gate

The user can organize a directory without leaving the file manager. Interrupted jobs and conflicts do not silently lose files, and listings converge to the actual filesystem state.

## Phase 3 — Shelf and clipboard integration

### Goal

Connect browsing to the existing hand/inventory model.

### Scope

- Define Shelf behavior for held file references, collected groups, and clipboard-derived content.
- Copy/cut files using interoperable clipboard file references, preserving the distinction between moving and copying.
- Paste text as a text file, images as image files, and other supported payloads through explicit encoders.
- Choose useful default filenames and extensions, with conflict handling and a quick naming affordance.
- Keep file references distinct from their decoded content: pasting a copied file and creating a file from copied image pixels are different operations.
- Explain unavailable held paths and clarify whether Shelf entries are references or retained snapshots.
- Avoid loading every held file's bytes into clipboard state.

### Completion gate

Copying text or an image elsewhere and pasting into a folder creates a usable file. Held file groups can be carried between locations with clear copy/move intent.

## Phase 4 — Rich previews and built-in editors

### Goal

Inspect supported files and make small edits without opening another application.

### Scope

- Establish a file-opening route that reuses existing viewers and type detection.
- Introduce visible-item thumbnail/preview jobs with bounded caches, cancellation, and stale-result rejection.
- Add image thumbnails, audio waveforms, PDF page counts, and document/text previews first; extend to other supported types incrementally.
- Keep a format capability matrix: listing, thumbnail, preview, editing, and export support are separate capabilities.
- Let the existing small editors open files with a source path and default Save/Save As destinations in that directory.
- Define modified-state prompts, safe replacement, and external-change detection before saving over a source.
- Return from a viewer to the same folder, selection, and scroll position.
- Keep external application opening available for unsupported or preferred workflows.

Preserve the existing single active `ViewerSession` ownership and ordered opening behavior. Thumbnail jobs must not create hidden competing viewer sessions. PDF and scene previews must retain their existing temporary-file and bounded-decoding constraints.

### Completion gate

Supported files can be inspected and edited from the browser without losing context. Preview memory and background work remain bounded while scrolling through mixed folders.

## Phase 5 — Media browsing and playback

### Goal

Turn previews into complete everyday gallery and playback workflows.

### Scope

- Photo grid with adjustable thumbnail size, selection, next/previous viewing, and slideshow controls.
- Music view with metadata columns, cover art, queue, play/pause, seek, next/previous, repeat, and shuffle.
- Explicitly define playback lifetime when navigating or closing the file manager. Decide process ownership before building persistent playback.
- Video browsing and playback through an assessed reusable backend; establish codec and resource expectations.
- Keep subfolders and non-media files accessible in media views.
- Provide manual view choice before automatic type-based selection is introduced in Phase 7.
- Integrate supported media edits and exports through the Phase 4 save model.

### Completion gate

The user can browse a photo collection and listen through a folder of music with ordinary controls. Media state survives navigation according to an explicit lifecycle contract.

## Phase 6 — Search

### Goal

Find files through progressive name matching, then metadata and contents.

### Scope

- Type-to-search when focus is in the browser, without stealing text from editors or the address bar.
- Search the current directory and any folders expanded inline by the later smart-expansion feature.
- Match exact names and ordinary substrings first, then ordered subsequences/name fragments, then the requested relaxed extension-boundary behavior, then typo-tolerant fuzzy matches.
- Specify examples such as `a_foo_b.md` matching `a_` plus `foo`, and `foo.md` finding `foo_2.md` when no exact match exists.
- If the initial scope has no match, search recursively from the current directory and configured always-search locations.
- Show scope expansion and matching mode clearly. Make ongoing recursive work cancellable and prevent symlink loops and duplicate results.
- Add globs, file-type shorthands, size comparisons, and ranges such as `10kb...2MB`.
- Add document content search comparable to `rg-a`, with snippets, capability reporting, and cached extraction where useful.
- Specify metadata query syntax, ranking, units, and whether fallback means no exact results or no results after all local matching stages.

Choose a minimal query language before implementing it. Use on-demand searching first; add durable indexes only when measured workloads justify their maintenance cost.

### Completion gate

Search follows a documented order and users can understand why a file matched and where it was found. Results from an old query cannot replace the current query.

## Phase 7 — Views and meaningful sorting

### Goal

Make folder structure and useful file attributes easier to see.

### Scope

- Add Miller columns with an adjacent file preview, retaining the conventional tree/list view.
- Add configurable inline expansion for small folders, with a clear background treatment and visible provenance.
- Bound automatic expansion depth, concurrent reads, and total visible items; never recursively expand without limits.
- Extend search scope to the inline-expanded contents through the same location model.
- Select media-oriented views using a configurable dominant-type threshold, based on content categories rather than extensions.
- Add per-folder view overrides and explicit inheritance from parent folders.
- Sort audio by artist/album/genre and other types by meaningful metadata, with clear placement of missing values and unrelated files.
- Explore perceptual image sorting such as warm/cool only after defining a reproducible signal and handling low-confidence results.

### Completion gate

Mixed folders remain fully navigable. Automatic views and expansion have understandable overrides, and metadata loading does not continually reshuffle a user's active selection.

## Phase 8 — Organization and a real Home

### Goal

Offer a personal starting surface and lightweight ways to organize work.

### Scope

- Build Home as an explicit virtual location containing chosen pins and sections. Keep it distinct from the filesystem home directory.
- Add pinned files/folders first. Dynamic rule-driven sections arrive in Phase 10.
- Add Session Mode with Red/Yellow/Green states, rendered as lines under items and with user-defined meanings.
- Add A/B/... sorting as configurable colored sets, supporting filtering and repeated subdivision into new groups.
- Keep session states, sorting sets, and visibility markings distinct in their data model and visual treatment.
- Define `.hide` and `.show` syntax, matching, precedence, inheritance, and recovery controls. `.show` can reveal selected dotfiles without revealing all dotfiles.
- Use the specified solid background treatment for visibility markings, with readable labels so color is not the only signal.
- Add project-box creation, `.fmproject`, `README.md`, `TODO.md`, and a global project list. Existing files must not be overwritten during conversion.
- Design the `.fmproject` schema: identity, descriptive fields, references, portability, and optional local preferences are candidate concerns, not settled fields.

### Completion gate

Users can build a useful Home, track a sorting session, and create or register projects. Restart, rename, and move behavior for marks and project identities is explicitly defined.

## Phase 9 — File transformations and metadata

### Goal

Apply the project's existing tools to files and groups of files.

### Scope

- Run existing transformations and pipelines against selections with an explicit output directory and naming policy.
- Preview planned outputs and conflicts before replacing files.
- Edit supported image, audio, document, and other metadata; distinguish read support from safe write support.
- Add ordinary batch rename first, then OCR/model-assisted suggestions as optional capabilities.
- Show a before/after rename table, unresolved collisions, and confidence where appropriate.
- Define sidecar behavior, preservation of unrelated metadata, and limits of format-specific writers.

### Completion gate

Users can transform a batch, edit supported metadata, and review a batch rename without silently overwriting unrelated files. File jobs use shared operation mechanisms rather than a parallel transformation framework.

## Phase 10 — Rules and contextual workflows

### Goal

Make repeated folder behavior configurable through nodes.

### Scope

- Design triggers, conditions, actions, and outputs around the existing node/pipeline model.
- Support manually invoked rules before enabling filesystem-triggered automation.
- Add dry runs, execution history, cancellation, per-folder disable controls, and loop prevention.
- Introduce archive extraction and conversion recipes, including resize and metadata chains.
- Treat deleting a source archive after extraction as a separate step that requires successful completion and the configured retention policy.
- Support filename policies such as timestamped duplicates instead of `foo (1)`.
- Add visual rules, including related archive/folder highlighting and changed-content indicators.
- Add dynamic Home sections such as recent downloads or changed Syncthing files. Define the data source for origin and change information; filesystem timestamps alone do not establish it.
- Define trust and execution policy for imported recipes and scripts.

### Completion gate

Users can understand what a rule will do, inspect what it did, and disable it. Outputs cannot create unbounded self-triggering chains.

## Phase 11 — Storage analysis and reconciliation

### Goal

Help users understand storage and reconcile collections.

### Scope

- Find exact duplicates using cheap candidate filtering followed by content hashing.
- Detect duplicate content among held/selected files and warn without blocking intentional duplication.
- Add folder usage analysis with clear treatment of symlinks, hard links, inaccessible paths, and apparent versus allocated size.
- Compare folders and present an explicit merge plan, with content verification when requested.
- Reuse existing comparison tools for supported file pairs.
- Add safe cleanup workflows that select candidates before any deletion.

### Completion gate

Results distinguish exact evidence from heuristic similarity. A folder merge shows additions, replacements, conflicts, and deletions before execution.

## Phase 12 — Remote performance and transfer scheduling

### Goal

Respect storage topology and avoid unnecessary data movement.

### Scope

- Browse locations mounted by the operating system with sensible latency limits, cancellation, and bounded metadata work.
- Add an explicit way to associate a mounted location with a remote host/path. Do not infer executable SSH destinations from arbitrary path names.
- Where verified capabilities permit, execute operations between locations on the same remote host there rather than round-tripping bytes through the local machine.
- Offer a terminal in the local directory or an SSH terminal at the mapped remote directory, with paths passed safely.
- Extend the queue from Phase 2 to schedule independent storage pairs concurrently, while limiting contention on shared devices or hosts.
- Show reconnect, mount disappearance, and partial completion states. Define which remote jobs can survive local disconnection.
- Preserve local fallback when an optimized remote operation is unavailable.

### Completion gate

Local behavior remains reliable; remote optimization has equivalent conflict and failure semantics. Independent transfers can progress together without flooding the same storage resource.

## Phase 13 — Filesystem editing and Git workflows

### Goal

Expose directory structure as an editable plan and add repository-aware browsing.

### Scope

- Investigate the intended VS Code-extension-style filesystem-as-file workflow before selecting syntax.
- Prototype a textual directory representation where editing names and structure produces a proposed rename/move/create/delete plan.
- Provide validation and a preview diff before applying that plan through the established operation engine.
- Handle ambiguity, collisions, external changes, and partial failures. Do not promise transactional rollback across arbitrary filesystems.
- Add Git status and `.gitignore` awareness without making ignored files indistinguishable from absent files.
- Start with status display and existing diff integration. Broader Git mutation workflows require separate scope decisions.

### Completion gate

Editing a directory representation produces an understandable operation plan. Repository state is loaded asynchronously and does not impede ordinary browsing outside repositories.

## Architecture and integration guardrails

These are constraints for future implementation, not a mandate to introduce all of these components during Phase 1.

### Responsibility boundaries

- The `clip-file-manager` executable owns its application lifecycle. Its browser owns navigation, selection, and view composition. Shared desktop components are exposed through the `clip-workbench` library rather than copied into another implementation.
- `little-clip-core` owns shared models and domain-neutral services where actual reuse exists, including established file classification, clipboard payloads, configuration, and IPC contracts.
- `little-clip-ops` owns transformations, pipeline execution, and external-command policy.
- Existing render and scene crates retain their rendering responsibilities; hardware-specific code stays in the driver.
- Decide whether filesystem operations warrant a dedicated library when Phase 2's requirements are concrete. Do not expand core or create a new crate merely to match the roadmap's headings.
- Do not put long-running browse or preview work on the UI thread, or require a healthy USB display to browse files.

### State and background work

Keep actual directory entries, navigation history, selection, and view preferences distinct. Use lossless paths and define identity across refresh, rename, and move before introducing persistent item annotations. Preview cache identity must include enough source information to invalidate changed files.

Reuse bounded executors, cancellation, and completion generation checks. Prioritize navigation and visible previews over speculative scanning. Assess whether long copy jobs or indexing need separate bounded queues so they cannot starve ordinary previews.

Preserve existing resource contracts: shared image revisions use `Arc`, PDF page rasters stay in viewer-owned temporary directories, and scene jobs retain their coalescing and bake cleanup behavior.

### Presentation and persistence

Use shared `ui_tabs::TabBar` wherever navigation tabs are introduced, shared SVG assets for controls, and the fixed Light/Dark palette. Color used for file classifications and session marks is data; keep base surfaces neutral and provide another way to identify each state.

Begin with runtime navigation state. Introduce persistence only for a defined user need: pins, preferences, projects, saved rules, and optionally resumed sessions. Decide which information travels with a folder and which stays in local configuration. Browsing alone must not create sidecars or project files.

### Architecture review

Before significant implementation, inspect reuse candidates and establish the required architecture baseline. Compare after each substantial milestone and address new complexity, duplication, or dependency regressions. The roadmap is not a reason to prebuild a plugin system, universal index, or generic filesystem abstraction.

## Validation strategy for implementation phases

Acceptance checks for implementation phases:

- Test navigation transitions and asynchronous result rejection independently from rendering.
- Exercise real temporary directory fixtures for file operations, conflicts, partial failures, symlinks, and names requiring careful path handling.
- Use the private Weston/Niri runner for UI acceptance. Pointer injection must stay on that runner's sockets.
- Check existing workbench startup, clipboard behavior, viewer ownership, and virtual mode after integration changes.
- Measure navigation latency, first visible results, peak memory, cache growth, and cancellation responsiveness on large local folders and slow mounts.
- Set performance budgets from a recorded baseline on documented hardware rather than inventing universal timing targets.
- Follow repository formatting, affected-crate tests, broader workspace checks where appropriate, and architecture comparison requirements.

Each phase's implementation proposal should name its acceptance cases and known limitations before work starts.

## Decisions to resolve near the relevant phase

| Decision | Needed by | Proposed starting position |
| --- | --- | --- |
| Application boundary | Phase 1 | Settled: standalone file manager, shared workbench library, independent resizable window |
| Default start location and restoration | Phase 1 | Home directory initially; no automatic cross-restart history |
| File-list and tree activation semantics | Phase 1 | Single selection, double-click/Enter navigation, separate expansion control |
| Trash, undo, and operation journaling | Phase 2 | Explicit recoverability per operation |
| Shelf references versus snapshots | Phase 3 | Prefer references for files; retain clipboard content under existing ownership rules |
| Save/replacement semantics and external edits | Phase 4 | Explicit source tracking and conflict checks |
| Playback lifetime and process ownership | Phase 5 | Decide before adding playback that outlives the active view |
| Search fallback ordering and query grammar | Phase 6 | Document examples and scope transitions first |
| Folder preference inheritance and storage | Phases 7–8 | Explicit overrides; distinguish portable settings from local state |
| Session/set persistence and item identity | Phase 8 | Specify rename/move/restart behavior before durable marks |
| `.fmproject`, `.hide`, and `.show` formats | Phase 8 | Versioned, readable formats with defined precedence |
| Automatic rule trust and trigger semantics | Phase 10 | Manual runs and dry runs before unattended triggers |
| Remote host mappings and execution capability | Phase 12 | Explicit verified mappings and local fallback |
| Filesystem-as-file syntax | Phase 13 | Prototype and review plans before enabling mutations |

Unresolved later-phase decisions should not block the basic browser.

## Coverage of the original goals

| Goal from core.md | Planned phase(s) |
| --- | --- |
| Ordinary browsing and file management | 1–2 |
| Modify filesystem as a file | 13 |
| Shelf / hand / inventory integration | 3 |
| Make files from clipboard contents | 3 |
| Broad previews, waveforms, PDF page counts | 4 |
| Gallery and music-player roles | 5 |
| Progressive name, subsequence, and fuzzy search | 6 |
| Recursive and always-search locations | 6 |
| Document content, size, glob, and type search | 6 |
| Node rules for extraction, conversion, naming, and highlighting | 9–10 |
| Personal Home and contextual sections | 8, 10 |
| Mixed-type thresholds, inherited views, and meaningful sorting | 7 |
| Miller columns with file preview | 7 |
| Smart inline folder expansion | 7 |
| Session Red/Yellow/Green states | 8 |
| A/B/... sorting and repeated subdivision | 8 |
| Duplicate finding and highlighting | 11 |
| Folder diffing and merging | 11 |
| Storage usage analysis | 11 |
| In-hand duplicate detection | 3 establishes references; 11 adds verified detection |
| Project boxes and global project list | 8 |
| `.hide` and `.show` | 8 |
| Remote-aware work and SSH terminals | 12 |
| Git status and `.gitignore` | 13 |
| Batch and OCR/model-assisted rename | 9 |
| Metadata editing | 9 |
| Transfer queue and storage-aware concurrency | 2, 12 |
| Built-in editors with source-directory saves | 4 |

## Next implementation milestone

Phase 3 is accepted and committed as `278091e`. The current Phase 4 checkpoint implements shared studios, visible listing previews, guarded text/image saves and protected audio/PDF outputs; current behavior and verification are recorded in [phase4.md](phase4.md). Continue Phase 4 before moving to Phase 5 media browsing and playback. Shelf polish remains follow-up work.
