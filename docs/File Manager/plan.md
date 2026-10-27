# File Manager Implementation Plan

Status: Phase 1 is complete as a standalone application. See [phase1.md](phase1.md) for delivered behavior and validation. Phase 2 is complete and verified; [phase2.md](phase2.md) records delivered behavior, operation contracts, and acceptance results. Phase 3 is accepted by the user as an initial implementation; [phase3.md](phase3.md) records its behavior and remaining follow-up work. Phase 4 is in progress; [phase4.md](phase4.md) records current capabilities, save contracts and verification evidence. Phase 5 is complete; [phase5.md](phase5.md) records the media workflow audit and verification.

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

Status: complete. [phase5.md](phase5.md) records the implementation, requirement audit, verification, and backend/resource limits.

### Goal

Turn previews into complete everyday gallery and playback workflows.

### Scope

- Photo grid with adjustable thumbnail size, selection, next/previous viewing, and slideshow controls. Inspired by iPhone's gallery.
- Music view with metadata columns, cover art, selectable waveforms and explicitly started single-track playback. No bottom player dock, queue, automatic next track, repeat or shuffle.
  - Activates when in a folder that's 75% (threshold configurable) audio files
  - Each preview is the album art (or missing stand in) and waveform, where the waveform has a navigable playhead and can be selceted in and copy-pasted from directly.
  - This will take careful considering for rendering, to handle huge folders without slow down
- Stop music on folder navigation through any entry point, Escape, switching to List, and closing Files. Space plays an active Music range once between zero-crossing-snapped endpoints; otherwise it stops playback. Completed tracks stop rather than advancing or repeating.
- Video browsing and playback through an assessed reusable backend; establish codec and resource expectations.
- Keep subfolders and non-media files accessible in media views.
- Provide manual view choice before automatic type-based selection is introduced in Phase 7.
- Integrate supported media edits and exports through the Phase 4 save model.

### Completion gate

The user can browse a photo collection and preview individual songs through their waveforms. Playback follows the explicit stop policy; waveform selection and copy/paste remain available.

## Phase 6 — Search

### Goal and current checkpoint

Find files through progressive name matching, explicit metadata/content queries, and semantic retrieval, while keeping ordinary type-to-navigate fast. Support temporary, typed answer columns for questions about the files in a folder.

Local type-to-navigate is already implemented and accepted; [phase6.md](phase6.md) records that checkpoint. Typing `foo` selects/reveals a matching entry without filtering the listing. Matching currently ignores case and prefers exact names, then prefixes, then substrings; ties retain display order. Keep its existing focus isolation, timeout, Backspace, Escape, and activation behavior. Implementation checkpoints for command search, hybrid semantic retrieval, image hash sorting, initial answer columns, progressive/recursive navigation, index monitoring, tokenizer-aware passages and persistent search preferences are tracked in [the work ledger](../../differences_from_phase_6_plan.md); the complete Phase 6 scope below remains unfinished.

[semantic_file_search_plan.md](../../semantic_file_search_plan.md) specifies the semantic index, local SQLite schema, segment representation, inference/helper reuse, freshness, scheduling, and resource limits. That document and this section are complementary: **this section owns command syntax and interaction; the semantic plan owns indexing and embedding execution.** Use one parsed query representation across the UI and backend so filters and scope cannot acquire different meanings in each path.

### Input modes and command entry

- Reserve **a leading `:` at the start of a new browser query** for command entry. Intercept it before ordinary filename matching; open/focus the search bar and collect the complete command. The `1` in `:10kb..2MB foo` belongs to its size constraint and must never initiate navigation to a filename beginning with `1`.
- Recognize command entry only with file-list focus or no focused control. Editors, address entry, naming dialogs, folder-tree controls, and media-strip shortcuts retain their input handling. A colon inside an ordinary query is literal, not a mode switch.
- Commands are explicit submissions: Enter validates/applies the command; it does not also open the previously selected file. After submission, arrows can focus results and a subsequent Enter activates the focused result. Command drafts have no two-second type-navigation expiry. Editing a draft does not run expensive embedding or answer jobs on each keystroke.
- Incomplete/invalid commands stay editable with an inline error and leave the last applied view intact. Never reinterpret an invalid command as filename text, silently discard an unknown filter, or execute it as a shell command. Escape cancels a draft; leaving an applied search restores the prior browser selection/scroll through the existing navigation mechanism.
- `::foo` escapes the command prefix and searches for the literal name text `:foo`. A leading doubled colon is an escape, not a second operator. Also offer a literal-name mode in the search UI, so colon-prefixed filenames remain accessible.
- A leading colon is the primary command entry; Ctrl+F remains an optional discoverable entry to the same search bar from the file list, no focused control, or the existing search field. It only focuses the draft and never submits it. Other editors/controls and Constellation's own find handling retain their shortcuts. Controls for scope, filters, and semantic mode edit the same structured query used by commands; they do not create hidden overriding parameters. Show a compact syntax hint and completion for known selectors/operators.

### Initial command language

Use `:` as the command introducer, commas for ANDed metadata selectors, a space before an optional literal filename query, and semicolons for an operator or named options. Examples define the initial accepted forms:

| Input | Meaning |
|---|---|
| `foo` | Existing type-to-navigate; exact name, then prefix, then substring |
| `:10kb..2MB foo` | Files in the inclusive size range with `foo` in their basename; exact/prefix matches rank before substring matches |
| `:10kb...2MB foo` or `:10kb-2MB foo` | Aliases for the same inclusive size range |
| `:10kb..2MB,*.jpg foo` | The same, restricted to the JPEG extension glob |
| `:10kb..2MB,image foo` | The same, restricted to the shared image content-type family |
| `:image` | Show image files in the current scope without a name constraint |
| `:image,!*.svg` | Images excluding SVG basenames |
| `:modified:2026-01-01..2026-03-31` | Entries modified during the inclusive calendar-date range |
| `:created:2026-01-01...2026-03-31` | Entries with a known filesystem creation date in that range |
| `:image,created:>=2026-01-01;sort:created:desc` | Images created since that date, newest first |
| `:content:"invoice number"` | Literal phrase search in supported extracted text |
| `:*.pdf;content:"invoice number"` | The same, restricted to PDFs |
| `:image;sort:size:desc` | Images ordered largest first |
| `:image;sort:dhash:asc` | Images ordered by their 64-bit dHash value; this is hash order, not a similarity guarantee |
| `:image;sort:similarity;ref:selected` | Images ordered by dHash distance from the one selected reference image, closest first |
| `:>=10MiB,video` | Video files of at least 10 MiB |
| `:regex:foo.*` | Basenames matching the regular expression from the beginning of the name; no substring/fuzzy fallback |
| `:regex:"(?i)foo.*\.jpg$"` | Case-insensitive regex, explicitly ending with the JPEG extension |
| `:emb:"2019 tax document"` | Semantic retrieval of that description in the current eligible indexed scope |
| `:10kb..2MB,*.pdf;emb:"2019 tax document"` | The semantic query with size and PDF constraints applied first |
| `:image foo;scope:recursive` | Explicit recursive filename/metadata search below the current directory |
| `:emb:"2019 tax document";scope:indexed` | Search all configured indexed folders |
| `:q:"End balance on bank statement";type:number` | Add a temporary numeric answer column to the current folder view |
| `:*.pdf;q:"End balance on bank statement";type:number` | Populate that column for PDF rows in the current folder view |

Rules:

- Selector forms are a size range/comparison, a `modified:`/`created:` date constraint, a basename glob, or a known content family (`image`, `audio`, `video`, `document`, `pdf`, `folder`, or `file`). Reuse core classification; define `document` as supported text/office/PDF content. A glob such as `*.jpg` matches the full basename and is case-insensitive by default. Multiple selectors are ANDed. Prefix an individual selector with `!` to exclude matches, as in `!*.svg` or `!audio`; OR/grouping syntax is deferred. Unknown metadata stays unknown under negation and does not pass either a positive or negative constraint on that field.
- Size uses logical file length, not allocated/compressed space. `B`, `KB`/`kB`/`kb`, `MB`, and `GB` are decimal multiples; `KiB`, `MiB`, and `GiB` are binary multiples. Unit spellings are case-insensitive. Bare numbers mean bytes. Parse decimal quantities exactly; accept `..`, `...`, and `-` as equivalent inclusive range delimiters, and serialize/display generated ranges with canonical `..`. Recognize these delimiters only between two complete nonnegative size quantities inside a size selector; never globally split or rewrite hyphens in filenames, quoted payloads, regexes, or date values. Match the longest dot delimiter first and preserve decimal endpoints such as `10.5KB-2MB`. Support `<`, `<=`, `>`, `>=`, and `=`; reject negative, overflowing, reversed, missing, or extra bounds. Integer byte counts must satisfy the stated comparison without rounding a fractional boundary outward. Directory sizes are unknown for these filters; do not recursively total folders or treat them as zero.
- `modified:` uses filesystem modification time; `created:` uses filesystem birth time, never inode-change time (`ctime`), first-indexed time, or EXIF capture time. Read creation time through the existing guarded metadata path, preserving whether the filesystem/interface actually supplies it (on Linux, check `statx`'s `STATX_BTIME` result flag). Missing birth time is unknown; a file copy may have a newer birth time than its source. Support `=`, `<`, `<=`, `>`, `>=` and inclusive ranges for both fields. Bare `YYYY-MM-DD` means that whole calendar day. Date ranges accept `..`, `...`, or a spaced hyphen (`2026-01-01 - 2026-03-31`); reject unspaced date-range hyphens rather than guessing. The parser consumes the complete date selector before interpreting trailing filename text. Calendar dates use the displayed local timezone, captured at submission; implement an inclusive upper day as the next local midnight exclusive, including daylight-saving transitions. For a date-only boundary, `>=`/`<` use its start and `>`/`<=` use the next day’s start, so comparisons include or exclude the whole named day. Explicit RFC 3339 timestamps require an offset and compare instants; do not mix date-only and instant endpoints. Display resolved boundaries/timezone and serialize ranges with `..`.
- Without an operator, the optional trailing text is a literal basename query, not a regex/glob or embedding prompt. Apply selectors before name ranking. Exact, then prefix, then substring is the initial command-name behavior; preserve the existing display-order tie break. An explicit command's hard selectors never loosen during fallback.
- Operators are `regex:`, `content:`, `emb:`, and `q:`. Allow one operator per command; reject conflicting/repeated operators. An operator can follow the leading colon directly, or follow metadata selectors after a semicolon. `regex`, `content`, and `emb` are query modes; `q` creates a column rather than selecting files by answer similarity.
- `content`, `emb`, and `q` require a double-quoted payload. Regex accepts a quoted payload, or an unquoted pattern with no whitespace/semicolon. Quote a literal filename payload if it contains reserved separators. Inside quotes, `\"` represents a quote and `\\` a backslash; preserve a backslash before any other character, including regex escapes such as `\d`. Commas, colons, and semicolons inside quotes are literal. Reject unterminated quotes and unsupported trailing options.
- Regex is case-sensitive by default, with explicit flags such as `(?i)`. Anchor matching at the start of the basename, not necessarily its end: `foo.*` means starting with `foo`, and `$` can require the end. A caller wanting an arbitrary-position match can write `.*foo.*`. Use a bounded non-backtracking regex engine; reject unsupported constructs and cap pattern length at 4 KiB. Do not run regex against extracted document content in this initial syntax.
- `;scope:here` means the current directory plus explicitly inline-expanded entries when that feature exists; `;scope:recursive` means its subtree; `;scope:indexed` means configured indexed roots. Default command scope is `here`, with no implicit recursive escalation. Embedding queries intersect the selected scope with indexed coverage; they never index or read arbitrary additional folders implicitly. Root exclusions, visibility policy, and protected-content routing continue to apply.
- `;sort:<field>[:asc|desc]` accepts `name`, `type`, `size`, `modified`, `created`, `dhash`, and `similarity`. Default direction is ascending (oldest first for dates, closest first for similarity). Reuse existing natural-name ordering, content-family/extension grouping, and directory placement for ordinary column sorts. Missing values remain last in both directions; ties use natural basename then lossless path. Without an explicit sort, retain the query mode's relevance order. Explicit sorting replaces relevance as the primary order without changing which files match. Apply it before display pagination for complete metadata/name/content result sets; for semantic retrieval it orders the bounded retrieved candidate window, visibly labeled with its cap, rather than claiming a global order of every potentially relevant file. Header clicks update the same structured sort option. `q` may preserve/change the ordinary listing sort, while sorting its new answer column remains a header action.
- `dhash` and `similarity` follow the image-specific rules below. `;ref:selected` belongs only to `sort:similarity`, which requires it; capture exactly one selected image's identity/revision on submission and show its name as the reference. Changing selection while browsing results must not change the reference. Reject missing, multiple, non-image, or unavailable references rather than silently switching metrics.
- `;type:` belongs only to `q` and accepts `text` (default), `number`, `date`, or `boolean`. Unknown/duplicate options are errors. `q` is limited to the current folder view; reject recursive/indexed scopes for it in this version. Future operators must extend this grammar through named forms rather than changing ordinary filename text.

### Progressive and content search

- Preserve ordinary typing as local name navigation. Progressive local stages now add ordered subsequences/name fragments, relaxed extension boundaries, and conservative one-edit typo matches after exact/prefix/substring matching. The transient hint identifies the fallback stage. Explicit command filename text remains literal. Retain examples such as `a_foo_b.md` matching `a_` plus `foo`, and `foo.md` finding `foo_2.md` when no stronger match exists.
- Automatic recursive fallback applies only after all enabled local name stages return no results, not merely because there is no exact match. After a brief debounce, clearly show escalation to the current subtree and configured always-search locations in an editable search field. Edits cancel obsolete work; a locally matching edit returns to current-folder scope. Prevent symlink loops and duplicate results. `file_manager.search.always_search` is an empty-default list of ordinary folder addresses, independent of semantic enrollment. Explicit command scopes never widen this way. See the work ledger for implementation and acceptance evidence.
- Metadata/name commands use on-demand directory metadata and do not require a semantic index. Explicit command scope never widens automatically. The `content` operator searches for its literal phrase in supported extracted text, case-insensitively with documented Unicode/whitespace normalization, and returns evidence snippets. It is not regex, raw FTS syntax, or semantic expansion. Reuse current indexed text when suitable and guarded, bounded on-demand extraction otherwise; no embedding model is required. An FTS prefilter must not exclude legitimate substring/phrase matches: verify against extracted text and use a compatible scan when the index cannot express the query. Report partial/truncated/unsupported coverage, cancel obsolete jobs, and do not claim a complete no-match result from incomplete extraction. Full content search comparable to `rg-a` remains part of Phase 6; richer content syntax can follow separately.
- `emb` uses the retrieval/ranking and current-segment rules in [the semantic plan](../../semantic_file_search_plan.md#search-and-ui-behavior). Its quoted description is not also a hard filename constraint. Metadata filters narrow candidates before retrieval, calibration, fusion, and the result cap, so a narrow query cannot lose valid files behind an unfiltered top-N cutoff.
- Unindexed or partially indexed scope is visible. An unavailable model or missing coverage is not the same as “no matching files.” Provide the semantic plan's setup/indexing controls without turning a search command into automatic scope enrollment or a download.

### Image hash sorting and audio similarity

- Reuse the image preview's existing perceptual analysis. Its current encoded value contains a version nibble, twelve coarse mean-color bits, and a 64-bit horizontal dHash. Parse those fields explicitly; `sort:dhash` compares the unsigned 64-bit dHash only, not the encoded string, color prefix, or hash-art pixels. Equal hashes are not proof of identical files.
- `sort:similarity;ref:selected` compares image dHashes by Hamming distance: the population count of XOR against the captured reference, from 0 to 64. This ranks resemblance to one image, not all-pairs clustering or subject-level similarity. Display the distance, not a fabricated similarity percentage. Raw hash order is available independently because numeric adjacency does not preserve Hamming distance. Keep coarse color separate from this initial metric; a later combined metric needs its own version and evaluation.
- Initially support these image sorts in `scope:here`. Reuse cached hashes for matching source/analysis revisions; schedule missing hashes on bounded cancellable workers using the existing guarded thumbnail/preprocessing path, without invoking unrelated full-file SHA-256 or metadata extraction solely for sorting. Hashes are currently produced for previewed images, so do not assume the whole folder is already analyzed. Prioritize the reference and visible entries, then the remaining eligible images; show progress and keep pending/unsupported/failed entries last. Preserve selected identity as rows settle. No semantic database or model is required. Reference replacement/edit invalidates the comparison and prompts reapplication; stale completions cannot silently choose a new reference.
- Audio similarity targets **sample discovery by sound**, with an eventual XO/Atlas-style map: nearby kicks, snares, hats, percussion, tonal samples, and effects should offer useful sonic alternatives. Song identification and recording fingerprints are outside this feature. Start with short one-shot samples; loops and long recordings need separate temporal representations and validation.
- Evaluate a compact per-sample feature vector describing spectral shape/timbre, transient attack, decay/envelope, duration, noisiness, and pitch where reliable. Compare this inexpensive DSP baseline with audio-model embeddings and a calibrated combination; do not assume semantic EmbeddingGemma retrieval preserves the fine differences between two snares. Preserve attack/body/tail information instead of averaging the whole sample into one spectrum. Test gain/leading-silence robustness without erasing meaningful pitch, decay, or dynamics. Existing waveform/spectrum analysis may supply inputs, but preview textures are not a sufficient similarity representation.
- Deliver selected-sample nearest-neighbor ranking first, then a separate 2D sample-map view with audition and file drag-out. The map is a projection for browsing; rank neighbors in the original feature space, not by screen distance. Version/cache analysis by source revision, reuse unchanged samples, and preserve map positions across ordinary opens/additions; a map rebuild must not force audio reanalysis. Keep this a staged follow-up to the initial image-only `sort:similarity`, with an explicit audio metric when added. See [the semantic plan](../../semantic_file_search_plan.md#sample-similarity-and-a-future-audio-map) for evaluation, storage, and map boundaries.

### Temporary answer columns

The `q` operator creates a column whose heading is the question and whose cells are answers about individual files. This is a separate capability from semantic retrieval: **EmbeddingGemma 2 produces vectors, not answer text or numeric values.** Reuse guarded text/page extraction and, where helpful, embedding-based passage retrieval, but answering requires a separately validated local extraction/question-answering worker. Its model/provider selection and accuracy validation are a prerequisite for the answer-column milestone, not a capability supplied by the embedding index.

Current implementation checkpoints include typed text/still-image answers and bounded short/scanned/mixed PDF answers, with source-page evidence and cross-page ambiguity preserved. See [the work ledger](../../differences_from_phase_6_plan.md) and [answer-model decision](../../semantic_file_search_plan.md#answer-model-and-applicability-decision) for tested behavior and limits; synthetic checks do not establish general extraction accuracy or complete Phase 6.

- On submitting `q`, retain the current folder's listing and add the column. Its metadata selectors limit which rows receive work; they do not replace the listing with semantically similar files. Respect the existing view/filter, and show non-applicable rows distinctly. Repeating the same question/type/selectors focuses the existing column; a different question adds another, up to four temporary columns per folder.
- Store column definitions and answers only in memory for that window and folder identity. Returning to the folder restores its columns during the same session; closing the window discards them. Header actions can rename/remove a column, retry failed cells, or refresh it. Do not write questions/answers into source files, xattrs, or the persistent semantic database, and do not log their contents.
- Schedule visible rows first, then remaining applicable rows, on a bounded queue. Work is explicit on submission, not each partial keystroke. Navigation away cancels outstanding answer work and retains only completed session results; returning may resume missing cells. Removing a column or closing the window cancels its jobs. Existing file-backed mutation jobs keep their separate completion rules.
- Every cell has a typed state: pending, value, not applicable, not found, unknown/partial, ambiguous, unsupported, unavailable, or failed. Applicability is independent of the requested value: internally preserve applicable, not applicable, and undetermined, rather than forcing uncertainty into a boolean. Show `-` for not applicable, with its reason on inspection. A boolean column therefore has valid true/false answers plus non-applicable rows; unknown and failed answers remain separately identifiable. Keep missing/error states separate from valid zero, false, and empty values. A returned value must have source evidence, such as a page/passage or structured field, available on inspection. Retrieval failure or incomplete evidence yields unknown/partial coverage, not a fabricated answer.
- A bank-statement question can return not applicable for established unrelated content, but an applicable statement with a missing or unreadable balance must return not found or unknown/partial. Low embedding similarity, an empty retrieval result, a filename, or an unsupported file extension alone cannot establish non-applicability. Images can contain statements; unsupported audio without a transcript is not automatically proven irrelevant to every question. Explicit selector exclusions may skip work, with a distinct excluded reason. Applicability and extraction should use the same answering model; a separate judge model is not required by this design. See the semantic plan's [answer-model decision](../../semantic_file_search_plan.md#answer-model-and-applicability-decision).
- Extract answers from verified current content. The worker treats document text as data, with no tool execution, network calls, or file mutations authorized by embedded instructions. Validate structured output and the requested type before accepting it. Conflicting values require an explicit source disambiguation or `ambiguous`, not an arbitrary first number. A model's asserted confidence is not evidence of correctness.
- Keep literal extraction and interpretive answering distinct. A parsed value or matching layout does not prove that it answers the question. Review a new tool capture against actual evidence, with an unanchored answer-and-compare check before authorizing reuse. Only a reviewed literal mapping may be reused; meaning/plausibility-dependent questions retain per-file assessment, even if the first example looks sensible. A disagreement or need for more context escalates once to guarded document answering, with unknown/ambiguous states when evidence remains inadequate. Neither review nor escalation may grant shell, network or mutation access. See the semantic plan for the current bounded implementation and its quality limits.
- For `type:number`, retain an exact decimal value, the original displayed text, and any currency/unit. Interpret separators/negative accounting notation only when supported by source context; ambiguous locale formatting stays ambiguous. Sort numeric values numerically, not lexicographically. Never silently compare currencies or different dimensions as equivalent: group by explicit compatible currency/unit, then sort within the group; unit-unknown values remain a separate group.
- `date` sorts validated calendar dates/instants with their precision/timezone scope preserved; do not invent missing timezone information. `boolean` sorts verified false/true values. `text` uses the existing natural text ordering. Header clicks toggle ascending/descending within comparable values. Pending/missing/error cells stay after valid values in either direction; ties preserve the prior stable row order.
- Sorting a partially populated column is allowed, with a completion count and pending rows clearly identified. Preserve selected file identity as completed values move into sorted position. Results are keyed by folder/file identity, source revision, question, answer type, and worker/prompt version; reject late results after edits, replacement, navigation, or column changes. Changed content invalidates the cell rather than presenting an old answer as current.
- The first answer-column milestone targets readable text and document/PDF evidence. Scanned documents lacking usable text require an explicit OCR/vision-capable answer path; do not claim the existing visual embedding answers their fields. Unsupported media/formats show that state until an appropriate answer worker exists. Per-file budgets and coverage limits must be exposed. Use unknown/partial when truncation or retrieval leaves relevant content unexamined; reserve “not found” for completed examination under the declared extraction policy, without implying that an unsupported modality was examined.

### Delivery and completion gate

Deliver this as separate reviewable increments: command-entry/parser and metadata/name/date filters, exclusions, and ordinary sort options; regex, literal content search, and explicit scopes; image hash/reference sorting; embedding-query integration with the semantic plan; then the independently validated answer-column worker and typed sorting. Progressive fuzzy/recursive filename fallback and richer content-search syntax may proceed as their own Phase 6 increments without blocking the first command syntax.

Acceptance includes the exact examples above, prefix interception before the `1` in a size command, unchanged ordinary `foo` navigation, colon-prefixed filename escaping, invalid/incomplete/quoted input, unit boundaries, equivalent `..`/`...`/`-` ranges including decimal endpoints, literal hyphens outside typed ranges, ISO dates and spaced date-range hyphens, creation-time absence without ctime substitution, local day/DST boundaries, negated unknown metadata, literal content verification/coverage, explicit sort versus relevance and pagination, image hash field parsing and Hamming-distance order, reference lifetime and pending-row ordering, anchored regex, and hard filters applied before semantic result limits. Exercise focus isolation and Escape/Enter behavior in the private compositor runner.

Search must show why a file matched and where it was found; results from an obsolete query or source cannot replace current results. Answer-column acceptance additionally requires evidence-backed typed values, null/error ordering in both sort directions, currency/unit handling, stable selection during completion, invalidation after edits, and disappearance of columns after the window closes. Pure regex/metadata/name queries must work without a model or database. Do not mark Phase 6 complete solely because type-to-navigate or semantic indexing works.

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
- Implement image dHash/reference sorting through the shared Phase 6 query/sort representation; additional audio similarity metrics require the evaluation described there.
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

Phase 3 is accepted and committed as `278091e`. The current Phase 4 checkpoint implements shared studios, visible listing previews, guarded text/image saves and protected audio/PDF outputs; current behavior and verification are recorded in [phase4.md](phase4.md). Phase 4 remains in progress. Phase 5 media browsing and playback is complete against its planned scope and gate; [phase5.md](phase5.md) records the requirement audit and verification. Phase 6 is under active implementation: [phase6.md](phase6.md) records the accepted initial type-to-navigate slice, and [the work ledger](../../differences_from_phase_6_plan.md) records subsequent search, indexing, answer-column and settings checkpoints with their remaining gaps. Full Phase 6 acceptance remains outstanding. Shelf polish remains follow-up work.
