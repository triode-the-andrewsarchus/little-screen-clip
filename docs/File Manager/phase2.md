# Phase 2: ordinary file operations

Status: complete and verified after the Phase 1 commit (`579325c`). Phase 2 passed its completion gate.

## Implemented behavior

| Area | Behavior |
| --- | --- |
| Selection | Click replaces; Ctrl-click toggles; Shift-click selects a range; Ctrl+Shift-click adds a range; Shift+Up/Down extends a range |
| Selection lifetime | Visible selection survives refresh/history; hiding items removes them from selection |
| Create | New folder and New file create a folder or empty regular file in a captured destination |
| Rename | Exactly one item, including a symlink itself; names and destination collisions are validated |
| Copy/cut/paste | Captured file references, recursive copy, same-filesystem rename for moves, and staged copy for cross-filesystem moves |
| Context actions | Create, rename, copy, cut, paste, trash, and clearly separate permanent deletion |
| Conflicts | Replace, Skip, or another name; Replace/Skip can apply to remaining conflicts in the current batch |
| Queue | One running batch, up to eight waiting batches, byte/item progress, cancellation, and recent result summaries |
| Trash | Platform trash storage, asynchronous Trash browser, selection, and restore to the original location without overwrite |
| Permanent deletion | Separate confirmation, explicit absence of Undo, and partial-failure reporting |
| Refresh | Background directory polling every three seconds while idle enough to read; F5 remains available; unchanged snapshots do not rebuild the UI |
| Drag-and-drop | Drag selected list rows into a listed folder, tree branch, or current list background, including other Wayland clients; default Move, hold Ctrl at drag start for Copy |
| Desktop interoperability | Copy exports `text/uri-list`; Cut exports `x-special/gnome-copied-files`; Paste reads supported local file references through `wl-paste` |

Directory rename and move update history and pending reads beneath the old path. Opening a naming dialog captures its destination; later navigation does not redirect the operation. Create selects its result only if the user's location and selection have stayed unchanged. Tree/list refresh retains unrelated selected entries.

### Keyboard

Existing Phase 1 navigation shortcuts remain available. With list focus:

- Ctrl+A selects visible entries; Escape clears selection.
- Ctrl+Z restores the last multi-selection collapsed by an ordinary single-file selection, in List, Photos and Music. The saved primary item and range anchor are restored too; missing/hidden entries are excluded. This is one selection snapshot per folder, not filesystem-operation undo, and text fields retain their own undo.
- Ctrl+C / Ctrl+X / Ctrl+V copy, cut, and paste file references.
- F2 renames one selected item.
- Delete opens Move to Trash confirmation; Shift+Delete opens permanent deletion confirmation.
- Shift+Up/Down extends selection; ordinary arrows move the primary row.

Ctrl+Shift+N opens New folder outside the address field. Enter applies a naming or removal confirmation; Escape cancels it. Closing a modal restores file-list focus. Names use the same reversible byte escapes as the address bar, preserve literal spaces, and cannot contain `/` or be `.`/`..`.

## Operation contracts

All filesystem work runs through core's bounded `BackgroundJobs`. Naming operations and file batches are serialized. Only rejected queue admission is retried automatically. Accepted mutations have no post-operation timeout and retain their completion results after cooperative cancellation. Closing while work is pending is deferred; close again after completion. Waiting batches can be cancelled separately.

Copies preserve symlinks without following them. Ordinary files copy in 256 KiB chunks; Unix permissions and ordinary file/directory times are preserved. Ownership, ACLs, extended attributes, hard-link relationships, and sparse allocation are not preserved. Special files such as sockets, FIFOs, and devices are rejected for copy. Repeated directory identities stop filesystem alias traversal. A destination within the source directory is rejected, including a destination reached through a directory symlink.

A copy is prepared in a private staging directory on the destination filesystem. Source metadata identities are checked before installation. Same-filesystem moves use atomic no-overwrite rename when there is no replacement. Other moves finish the destination first; source removal occurs afterward and uses a private quarantine name to avoid deleting a newly recreated entry at the original path. Source changes, cancellation, and cleanup failures retain the source or remaining source and report the recovery location.

Create and rename never overwrite. File creation uses exclusive creation; rename and final installation use Linux `renameat2(RENAME_NOREPLACE)`. Replacement requires an explicit choice. Replace operates on the entire destination entry, including an existing folder's contents; it is not a folder merge. The previous destination is moved aside before installation and restored on installation failure where possible. If recovery or cleanup fails, the preserved path is included in the result. There is no generic Undo for copy, move, replacement, or permanent deletion.

The [trash-rs library](https://docs.rs/trash/5.2.9/trash/) supplies platform trash placement and discovery. Restore uses an atomic no-overwrite move from the Linux trash store rather than an existence check followed by ordinary rename. It can recreate original parent folders. A collision or unsupported cross-filesystem restore is reported and leaves the item in Trash.

Trash and permanent deletion compare the entry's current metadata identity with the browser snapshot captured at confirmation. Changed entries are refused and require refresh/reselection. Interrupted or failed recursive permanent deletion may have removed part of a folder; it cannot be rolled back. Cancellation checks run between batch entries and copy chunks. They cannot interrupt an operating-system call or a platform trash/removal operation already running.

Reports distinguish completed, skipped, failed, cancelled, and unprocessed entries. Failures after a destination has been installed explicitly describe the effect and retained source. Recent reports are bounded to ten batches. Up to fifty individual errors/skips are displayed per expanded report; summary counts cover the whole batch. Transfer artifacts left after an interrupted process or cleanup failure produce a directory notice when no operation is running. Enable hidden entries to inspect `.little-clip-copy-*`, `.little-clip-replaced-*`, and `.little-clip-moving-*` paths. There is no persistent transaction journal or automatic crash replay.

## Clipboard and platform limits

The existing core clipboard writers are reused. Local references remain available if adapters are unavailable, with a visible error. Clipboard import accepts only local file URIs, rejects unsupported payloads, and rejects output above 1 MiB rather than silently importing a truncated selection. Completed cut entries are removed from held references; desktop cut references are updated only after checking that the clipboard still contains the consumed selection.

Native desktop drag-and-drop uses `text/uri-list` on eframe's existing Wayland display through an independent event queue; it does not alter the workbench window or its lifecycle. An egui payload supplies selected paths and remains the internal fallback when no native bridge is available. Incoming drops capture a folder target before asynchronous URI decoding, then enter the same copy/move queue and conflict handling as paste.

A receiver finishes a native move only when every item succeeds. Skip, partial failure, cancellation, invalid input, and queue rejection do not authorize the sender to remove originals. Outgoing moves remove originals only after a complete URI write and the peer's move acknowledgment. Cleanup checks captured source identities, including descendants, and reuses the verified quarantine/removal mechanism; changed entries are retained. Sources already moved by the peer count as completed.

Native pipe transfers are nonblocking workers with a 10-second deadline and a 1 MiB URI limit. Source identity scans are bounded to 100,000 entries and the same deadline; larger selections can use the ordinary operation queue through copy/paste. Remote URLs and unsupported actions are refused. UI polling dispatches only the bridge's pending event queue and never performs a socket read or waits for a peer on the UI thread.

The vendored egui-winit integration exposes an opt-in clipboard shortcut flag for non-text surfaces. Files enables it only for list focus outside a modal, so Ctrl+C/X/V reaches the file-reference handlers without a synchronous text clipboard read. Address and name fields retain ordinary text clipboard behavior.

Filesystem polling has one outstanding read for the current directory; it does not scan the whole tree or add a watcher per expanded branch. Polls run off the UI thread and retain selection/scroll. Slow mounts can delay a poll. Operations that change a directory invalidate its tree branch and request a listing refresh where relevant.

## Requirement audit

| Phase 2 requirement | Implementation and verification |
| --- | --- |
| Multi-selection and context actions | Range/toggle/all selection unit checks; rendered selection, keyboard, and shared context-action controls |
| Create, rename, copy, cut, paste | Atomic naming tests, URI parsing checks, and private desktop UI checks with filesystem assertions |
| Trash, restore, permanent deletion | No-overwrite restore and deletion identity tests; temporary-file UI confirmations and restore |
| Internal and desktop drag-and-drop | Two separate Wayland clients exercise copy, both directions of move, recursive copy, cancelled drop, skipped move, list-folder drop, and tree-folder drop |
| Replace, Skip, alternate names, batch policy | Transfer conflict tests cover all three choices; private UI checks cover repeated conflicts and summaries |
| Background queue, progress, cancellation, partial failures | Bounded queue tests, staged-copy cancellation, post-installation cancellation, partial batch results, and native receipt safeguards |
| External changes and preserved selection | Quiet polling tests preserve selected paths and an address draft; rendered external-file insertion |
| Symlinks, recursion, cross-filesystem move, containment | Broken links and link targets are preserved correctly; recursive copy, metadata, actual `/tmp` to `/dev/shm` move, and symlink-alias containment tests |

## Verification results

- `cargo build -p clip-workbench --bins --offline`: passed for the standalone file manager and workbench.
- `cargo test --workspace --offline -- --test-threads=2`: passed, 1,237 tests passed, 14 ignored, zero failures across 26 suites. The shared workbench library includes 70 tests, 50 covering the file manager.
- `cargo clippy -p clip-workbench --lib --bins --no-deps --offline`: passed with existing repository warnings; this does not establish a warning-free workspace. No warnings referenced the new file-manager modules or changed egui-winit integration.
- `cargo fmt --all --check` and `git diff --check`: passed. The excluded vendored egui-winit file was formatted separately with Rust 2021 formatting.
- Architecture comparison against `pre-file-manager-phase2`: **NEUTRAL**, no new cyclomatic-complexity threshold violations or dependency cycles. Structural analysis flags the main UI coordinator and acceptance helpers as coupling/complexity hotspots; some ranking changes also appear in untouched scripts. Duplication fell from 8.35% to 8.14%, and clone groups from 1,301 to 1,300.

Private Weston/Niri acceptance used temporary file fixtures and scoped pointer injection:

| Run | Result | Report |
| --- | --- | --- |
| Full browser and file operations, Dark | 51 observations passed, plus filesystem and clipboard assertions | `/tmp/fm-dark-complete/report.json` |
| Full browser and file operations, Light | 51 observations passed, plus filesystem and clipboard assertions | `/tmp/fm-light-complete/report.json` |
| Native desktop drag-and-drop, Dark | 11 observations passed, plus filesystem assertions | `/tmp/fm-dnd-dark-final/report.json` |
| Native desktop drag-and-drop, Light | 11 observations passed, plus filesystem assertions | `/tmp/fm-dnd-light-b/report.json` |
| Workbench regression | Fixed centered size, single-instance focus, dismissal, and compositor responsiveness passed | `/tmp/fm-workbench-regression/report.json` |

The full UI runs cover navigation and tree behavior, naming, range selection, keyboard file clipboard operations, repeated conflict skipping, trash and restore, cancelled and completed permanent deletion, external change detection, selection clearing, and an actual context-menu Copy action. Native runs exercise two separate application clients, copied and moved files, recursive folder copy, cancelled drops, conflict skipping, and list/tree folder targets. These checks establish the tested Wayland behavior; they do not establish interoperability with every third-party desktop application.

Reports and screenshots are local temporary artifacts; the acceptance scripts are retained in `scripts/file_manager_acceptance.py` and `scripts/file_manager_desktop_acceptance.py`. The overall roadmap remains in [plan.md](plan.md).

Rebuild before trying this version: `launch.sh files` launches an existing binary when present.

```bash
./launch.sh build --bin clip-file-manager
./launch.sh files /path/to/folder
```
