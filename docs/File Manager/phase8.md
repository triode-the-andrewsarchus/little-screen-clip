# Phase 8 — organization and a real Home

Status: started on `feat/file-manager-phase8`, from the Phase 7 merge `bf6bf73e`.
Architecture baseline: `before-phase8-home`.

## First delivery: static Home pins

Implement an explicit virtual Home surface, separate from the filesystem home
folder and from the clipboard Shelf. Users deliberately pin files and folders,
open them through ordinary guarded browser/studio navigation, and remove a pin
without deleting its target. Pins persist in local application state, never in
the browsed content folder. Missing/unavailable targets keep their references;
opening reports a failure rather than dropping them or browsing another path.

Reuse existing pinned-directory/FileSource atomic state writes, cancellable
background delivery and ordinary guarded navigation. No payload scans, new media
cache, search index or plugin framework belong in Home persistence. Accepted
state writes must finish before application close. Malformed/newer state is
reported and preserved, never silently reset and overwritten.

Define the persistence contract before durable pins: versioned state, lossless
Unix paths, stable pin identity independent from display names, bounded records,
explicit duplicate policy, and cooperating-window updates. A pin is a path
reference, not proof that an unavailable mount or replaced file is the original
object. External renames/moves do not magically retarget pins. Browser-managed
retargeting and full navigation-history integration must be tested before being
claimed. Home must never be implemented as a fabricated filesystem path.

- [x] Define and test the bounded Home state and atomic update contract.
- [x] Add the Home surface and deliberate pin/open/remove actions.
- [x] Verify restart, missing paths, filenames with non-UTF-8 bytes, failed writes,
      concurrent updates and source-folder preservation.
- [x] Native review in both appearances; document the delivered initial limits.

## Remaining Phase 8 roadmap

- Named user-arranged Home sections; dynamic rule-driven sections remain Phase 10.
- Session Mode: red/yellow/green underlines with user-defined meanings and labels.
- Configurable A/B/... sorting sets, filtering and repeated subdivision.
- Keep session state, sorting-set membership and visibility rules distinct.
- Specify `.hide`/`.show` matching, precedence, inheritance and recovery, including
  selectively revealing dotfiles and readable solid-background markings.
- Specify versioned `.fmproject` identity/references/portability/local preferences.
- Create/register projects, README/TODO defaults and a global project list,
  preserving every pre-existing file during conversion.
- Define and verify restart/rename/move behavior across marks and projects.

The current objective requires starting Phase 8 after the Phase 7 closeout;
it does not claim completion of this full roadmap.

## Delivered initial slice

The toolbar Home button opens a separate virtual surface, retaining the last
folder selection/view/scroll without inventing a directory path. Pin current
folder and Pin selection deliberately collect references from the ordinary
browser; temporary archive snapshots cannot be pinned. File pins read their
parent directory before opening a shared studio, so image sibling ordering and
return-to-folder behavior use the real listing. Folder pins use ordinary guarded
navigation. Escape/Back to folder restores browsing. Hidden folder shortcuts,
drop targets and host file-action menus cannot mutate the retained folder while
Home is visible. Accepted browser operations continue ticking.

State lives in `$XDG_STATE_HOME/little-screen-clip/home.json` (fallback
`~/.local/state/...`), version 1: ordered `pins` records containing a UUID `id`,
a lossless `path`, and a `folder` presentation hint. There are at most 512 pins,
4,096 bytes per absolute path and 1 MiB per state file. Nil/duplicate IDs,
duplicate paths, NULs, parent traversal and unsupported schemas are rejected.
Adding an existing path keeps its original ID/order/hint. A change reads and
merges the latest state under a nonblocking cooperating-writer lock; lock
contention or failed writes are explicit errors, preserving the previous state.
Unknown/malformed/newer files are left unchanged. Unpin only updates this local
state, without reading or deleting the referenced payload. Source identity is
revalidated by ordinary navigation/opening, not inferred from a stored pin.

The shared `local_state` transport and worker were extracted from folder view
preferences in `5af645ef`; their existing persistence/locking tests still pass.
Home keeps its own schema and domain state. Accepted writes retain unbounded job
ownership, and the application close gate includes pending Home writes.

Validation:

- `cargo test -p clip-file-manager --lib -j4 -- --test-threads=1`:
  **965 passed, zero failed, 6 ignored**; includes five new persistence/rendered
  Home tests and the existing preference/clipboard/navigation/studio regressions.
  Log: `/tmp/phase8-home-all-tests.log`.
- `cargo clippy -p clip-file-manager --all-targets -j4 -- -D warnings`,
  `cargo fmt --check`, `cargo build -p clip-file-manager -j4`: passed.
  Logs: `/tmp/phase8-home-{clippy,format,build}.log`.
- `scripts/file_manager_home_acceptance.py`: passed in both appearances against
  the final debug build. Reports: `/tmp/phase8-home-final-dark/report.json` and
  `/tmp/phase8-home-final-light/report.json`. These verify persisted file/folder
  pins, stable IDs across restart, text-studio handoff, non-destructive unpin,
  reopening the folder, Escape and no source-directory sidecars. Native
  screenshots in both appearances were visually inspected. Button coordinates
  are paired with captured private-runner geometry; file/state effects and studio
  contents are independently asserted when OCR omits bordered labels.

Architecture comparison against `before-phase8-home` retains average/max
complexity and 7.3% duplication. It reports REGRESSION for four additional small
clone groups, chiefly the explicit queue-admission/result-publication scaffolding
in the two domain controllers. The shared disk transport, lock and worker are
factored once; a generic domain-controller framework was not introduced solely
to hide the remaining short Task/constructor patterns. Home rendering was split
into its action row and pin component, eliminating a new excessive-complexity
function. Existing host functions gain the necessary Home branches: menu gating
(+1), Home counts (+1), and suspending startup/transfer-refresh/watch navigation
(+3). There is no new high-complexity function or media cache. No new dependency
cycle was introduced; aggregate Python cycle ordering varies between runs.

Initial limits: a simple static pin list, not the finished Home visual design.
Named sections/reordering, rich pin previews, Home history integration, startup
Home selection and browser-managed rename/move retargeting remain follow-up
Phase 8 work. External moves leave path references unchanged and missing targets
report an error. Session marks, sorting sets, visibility rules and projects remain
on the roadmap above. The full Phase 8 completion gate is not claimed.
