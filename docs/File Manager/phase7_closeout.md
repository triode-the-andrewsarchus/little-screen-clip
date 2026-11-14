# Phase 7 closeout

Approved objective: finish the six closeout tasks, record the review checkpoint,
merge `feat/file-manager-phase7` into `main`, then begin Phase 8.
Personal `.cargo/config.toml` and `config.toml` changes stay outside commits.

## Delivery ledger

- [x] 1. Prioritize filenames over optional media summaries; verify narrow/wide headers.
- [x] 2. Make Filename order an idempotent ascending natural-name reset; preserve viewport/selection.
- [x] 3. Verify keyboard/focus, sticky-header and preview continuity interactions.
- [x] 4. Recheck folder/archive/search and saved-view workflows against the current UI.
- [x] 5. Update selected acceptance drivers and reconcile historical/current documentation.
- [x] 6. Pass workspace tests, strict all-target Clippy, owned-source formatting and canonical build.
- [x] Record requirement-by-requirement review checkpoint and remaining intentional limits.
- [x] Merge Phase 7 into main, preserving personal configuration changes.
- [x] Start Phase 8 with its roadmap and a first usable implementation slice.

## Evidence

Task 3 audit found a sidebar handoff bug: bookmark/mount clicks did not claim
keyboard focus, and a release frame could reactivate default list focus. Both
rows now request focus and mouse gestures retain ownership through release.
`cargo test -p clip-file-manager --lib bookmark_and_mount_rows_keep -j4` passes
in both views using mounted-folder fixtures, without invoking mount operations.
Each rendered frame refreshes the fixture scan timestamp, so a slow test never
falls through to the host mount inventory. Native keyboard evidence is recorded
below.

Task 1: `cargo test -p clip-file-manager --lib media_summaries_yield -j4` passes.
The shared header reserves measured filename width before optional summaries at
95/130/260px; this keeps calendar and other custom row names legible.

Task 2: `cargo test -p clip-file-manager --lib filename_reset_is_idempotent -j4`
passes. Repeated resets retain ascending natural names, selected file and viewport
in both views; mixed folders retain type grouping and clear only the family sort.

Record commands, results, source revisions and native reports as each task finishes.
Tests intentionally ignored by their existing harnesses are reported separately.

## Final build and test gate

The production closeout fixes are `d5e12827` (filename space), `659d7ea5`
(idempotent name reset), and `18eec117` (sidebar focus). `bd35b315` keeps
rendered mount fixtures isolated even during slow test runs.

- `cargo test --workspace -j4 -- --test-threads=1`: 47 suites,
  **3,303 passed, zero failed, 36 ignored**, zero compiler warnings.
  Log: `/tmp/phase7-closeout-workspace-tests.log`.
- `cargo clippy --workspace --all-targets -j4 -- -D warnings`: passed.
  Log: `/tmp/phase7-closeout-workspace-clippy.log`.
- `cargo fmt --check`: passed. `cargo fmt --all --check` reports existing
  formatting in 141 vendored files and no owned files; upstream formatting was
  preserved. Logs: `/tmp/phase7-closeout-owned-format.log` and
  `/tmp/phase7-closeout-format.log`.
- `CARGO_BUILD_JOBS=4 ./launch.sh build`: passed, including bundled media-helper
  corresponding source/licenses and complete-graph release-cache pruning.
  Log: `/tmp/phase7-closeout-canonical-build.log`.
- Python acceptance drivers compile; `git diff --check` passes.

Cargo invocations and cleanup were serialized. Only stale generated debug
executables were removed; current binaries, live processes and dependency
libraries were retained. Personal configuration changes are excluded.

## Current native workflow evidence

Drivers use isolated Weston/Niri sockets, generated fixtures, private clipboard
and audio infrastructure. These reports include screenshots and observations;
they do not claim every filesystem, machine or media format was exercised.
Each directory below is under `/tmp/` and contains `report.json`.
Run drivers with `TESSDATA_PREFIX=/home/vega/.local/share/tessdata python3
scripts/<driver>.py --output-dir <new-directory> --appearance light|dark`.

| Requirement | Driver | Passing report directory |
| --- | --- | --- |
| Sticky mixed-family headers/common columns | `file_manager_metadata_acceptance` | `phase7-closeout-metadata-dark-01` |
| Single-family header, repeat Filename order, narrow/wide List/Columns | `file_manager_metadata_acceptance` | `phase7-closeout-metadata-reset-light-01` |
| Square entry targets, bounded inline folders, archive warning | `file_manager_phase7_acceptance` | `phase7-closeout-containers-light-01` |
| Retained Miller chain, divider drag/reset, narrow/wide navigation/history | `file_manager_columns_acceptance` | `phase7-closeout-columns-light-01` |
| Name/size/modified/type ordering and reversal | `file_manager_sorting_acceptance` | `phase7-closeout-sorting-dark-01` |
| Saved Columns inheritance/override across restarts | `file_manager_view_preferences_acceptance --columns` | `phase7-closeout-preferences-dark-01` |
| Saved Photos choice/inheritance across restarts | `file_manager_view_preferences_acceptance` | `phase7-closeout-preferences-photos-light-01` |
| Expanded here names/content/fallback, URI copy, ancestor replacement rejection | `file_manager_expanded_search_acceptance` | `phase7-closeout-expanded-search-light-01` |
| ZIP/TAR member reveal, shared image/text studio, source preservation | `file_manager_archive_members_acceptance` | `phase7-closeout-archive-members-light-02` |
| gzip/bzip2/xz/zstd browsing, oversized refusal, unchanged sources | `file_manager_compressed_archive_acceptance` | `phase7-closeout-compressed-dark-01` |
| Reference/dHash order/reversal/revision invalidation | `file_manager_image_order_acceptance --expanded` | `phase7-closeout-image-order-dark-01` |
| List/Columns image Enter/Escape/arrows, calendar/toolpath/notebook return | `file_manager_escape_acceptance` | `phase7-closeout-escape-dark-01` |
| Audible waveform/range playback, Escape stop, embedded video | `file_manager_list_media_acceptance` | `phase7-closeout-media-light-02` |

Earlier failed driver runs remain available alongside the final reports. Old
archive-pane/disclosure/Shelf assumptions were replaced with the current shared
browser/studio workflows. The first media run inspected only the silent trailing
200 ms of a buffered audio capture; recorded PCM confirmed playback. The updated
driver checks newly recorded samples across the bounded capture window.

## Architecture review

Compared with `before-phase7-closeout`: average/max complexity unchanged,
duplication percentage unchanged at 7.3%, dependency-cycle count unchanged.
The aggregate tool reports REGRESSION for five additional clone groups. Inspection
locates the new repetitions in rendered test setup/input events/assertions and
short acceptance source-preservation assertions. No duplicated production
mechanism, cache, decoder or high-complexity function was added. Existing focused
header, sorting and browser-focus owners were extended. Retaining explicit
independent test expectations is preferable to a new generic UI-test abstraction
for this closeout. The reordered Python cycle report is the same unchanged
`video_gpu_average_check.py` pair, not a new dependency.

The workspace gate also includes rendered regression tests
`arrow_selection_keeps_visible_previews_and_scroll_stable_in_list_and_columns`
(real retained preview textures and overlapping rows),
`list_and_columns_default_keyboard_focus_and_restore_it_before_the_first_returned_key`,
`bookmark_and_mount_rows_keep_arrows_out_of_the_file_selection`,
`family_headers_stick_switch_and_remain_clickable_without_sorting_type_in_single_family`,
and `family_header_sort_preserves_rendered_scroll_with_an_offscreen_selection`.
Together these cover focus routing, preview continuity, scroll anchoring and
sticky sorting rather than relying only on screenshot text matches.

Bookmark native acceptance also passes:
`phase7-closeout-bookmarks-dark-07/report.json` (dark, 1280×1100 private output).
It verifies single/batch GTK additions, navigation/history, menu gating, native
Copy-only folder-reference drops (including an unreadable descendant), shortcut
removal without payload deletion, external atomic bookmark replacement, empty
section removal and first-drop restoration. The driver waits for resize animation
before resolving targets and accounts for Select All including visible descendants.

## Review checkpoint — 2026-11-14

All six approved closeout tasks are complete. Review of the current source and
native reports confirms the Phase 7 requirements:

1. Columns retains an actual horizontally scrollable folder chain and shared
   final rich List; divider resize/reset and ancestor/history navigation pass.
2. Folder/archive cards use square entry targets and direct-child-first bounded
   nesting (maximum three layers); arrows stay on current-folder rows.
3. Expanded here search shares browser actions/provenance and rejects replaced
   source ancestors; metadata loading and sort completion preserve file identity.
4. Per-folder view choices and explicit inheritance persist across restarts
   without writing sidecars into browsed folders.
5. Metadata sorts, missing-value placement, sticky family headers, common
   Size/Modified fields and idempotent Filename order meet the current scope.
6. Reference/dHash ordering remains explicit and shares the Phase 6 mechanism;
   no model/index enrollment or automatic media-view selection was added.
7. List/Columns focus, editor return, sidebar ownership and stable preview
   textures/scroll are covered by native workflows and rendered regression tests.
8. ZIP/TAR/compressed-TAR browsing uses bounded managed snapshots and visible
   archive status; tests verify studio handoffs and unchanged source archives.

The closeout source changes extend existing owners; no unresolved correctness
finding remains in this review. Screenshots inspected include deep Columns,
pinned mixed-family metadata headers and bookmark selection/drop states.
The full original implementation ledger is preserved verbatim after its title
in `phase7_history.md`; current requirements are in `phase7.md`.

Intentional limits: archive snapshots reject unsafe/oversized/incomplete input,
archive edits do not update their source container, extra image warm/cool ordering
is optional, new automatic views and audio similarity are excluded. Home,
organization marks/sets, visibility and project schemas remain Phase 8 work.
This is the technical review checkpoint for the authorized merge, not a claim
that future user review cannot find further issues.

## Merge

Merged with `--no-ff` as `bf6bf73e` on main. The complete Phase 7 branch is an
ancestor of main; the merge has no additional source changes beyond the tested
branch. Both personal configuration files were restored with identical SHA-256
hashes. Phase 8 starts on `feat/file-manager-phase8` from that merge.

Phase 8 now has a working static Home pin surface and persistence, with native
restart/open/unpin/folder-return verification in both appearances. Its initial
scope, evidence and remaining roadmap are recorded in `phase8.md`.
