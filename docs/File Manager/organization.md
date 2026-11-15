# Local organization

Session status, sorting sets and visibility rules are separate concepts. Session
marks describe progress through a task. Sorting sets classify references.
Visibility rules decide which directory entries are shown. None of these moves,
rewrites or deletes the content being organized.

## Session marks

The Session menu shares the existing view toolbar. Enable marks, select files or
folders, then assign Red, Yellow or Green. A row's context menu exposes the same
assignment actions. Selected rows are handled as one batch; missing metadata
fails the batch instead of silently marking only part of it. Temporary archive
contents and unavailable offline search results cannot be assigned marks.

Marks appear as solid underlines with R/Y/G labels, without increasing row
height. List, active Columns rows, ancestor columns and photo rows use the same
presentation state. Full meanings appear in the menu and row hover text. Names
and meanings are editable; the default meanings are simply Red, Yellow and Green
so the application does not prescribe what a color means.

Definition edits can be saved or discarded explicitly. Valid drafts are also
submitted with the next session action or application-close request. Accepted
writes block close until their completion is delivered. Invalid drafts remain
editable and prevent silently losing the edit. An open studio resolves its own
close decision first, so a subsequent browser-owned definition error can be seen
and corrected in the browser.

Disabling Session hides the annotations and retains them. Clear all session marks
clears assignments across folders, retaining one recoverable set. Restore swaps
the current and retained assignments, preserving work done after clearing.
Forget cleared-mark backup explicitly releases that recovery set. These actions
retain the session name and meanings. Recovery requests carry the observed
backup revision, so a stale window cannot accidentally clear newly restored
marks or forget another window's newer backup.

## References and revisions

A session mark includes a lossless absolute path and the inventory entry's full
metadata fingerprint. The underline is shown only for that observed revision.
Editing/replacing a file, changing permissions or changing a folder's contents
can make its old mark dormant. The reference is retained, not silently deleted;
remarking updates it to the new observation. Counts are explicitly stored-mark
counts, including dormant or unavailable references. An annotation is a personal
note, not proof of target availability, content permission or device state.

External moves do not guess new targets. Browser-managed rename/move retargeting
is part of the remaining Phase 8 history/identity work. Refresh/F5 reloads local
organization state when no definition draft would be lost, including changes
saved by another window. Error recovery offers an explicit reload that discards
definition drafts. Directory navigation and selection do not change annotations.

## Persistence contract

State lives at `$XDG_STATE_HOME/little-screen-clip/organization.json`, falling back
to `~/.local/state/little-screen-clip/organization.json`. Version 1 contains a
`session` object: `enabled`, `definitions` (`name` and three ordered `meanings`),
`marks`, `previous` and an internal `recovery_revision`. Marks contain a `target`
(`path` and the shared inventory `Identity` fingerprint) and a lowercase `color`.
Missing recovery revisions default to zero for earlier version-1 data.

The file allows 8 MiB, 16,384 current marks and 16,384 recovery marks. Paths are
absolute, at most 4,096 bytes, with no parent traversal or NULs. Names/meanings
must be nonempty and at most 128 UTF-8 bytes. Non-UTF-8 Unix path names use core's
existing lossless path codec. Duplicate references, malformed/newer schemas and
oversized input fail explicitly; the source state file remains unchanged.

The shared guarded local-state transport pins the output directory and merges
semantic changes under a cooperating-writer lock before atomic replacement.
Different definition fields merge independently; conflicting edits to the same
field fail rather than replacing another window's newer label. Read preparation
and index building run on the existing bounded state worker. Accepted mutation
delivery is unbounded; only queue admission is retried automatically. Commands
and published indexes use shared immutable ownership so ordinary frames do not
clone the full mark list. File content is never read or written by assignment.

Sorting sets use the separate model described below. Visibility rules remain a
subsequent Phase 8 delivery; neither is inferred from session colors.

## Sorting sets

Sets classify path references independently from session review marks. A/B are
the initial root groups. Each group has a stable UUID, name, RGB border color and
optional parent. Choose a group filter, then Subdivide this set to add children;
repeat to refine a classification. Assigning an item to a child replaces its
previous membership, and every ancestor filter includes that child. Direct parent
assignments remain in the parent until deliberately reassigned. Groups can be
renamed and recolored. Deletion is allowed only after removing direct assignments
and child groups, so concurrent assignments cannot silently disappear.

All files, Unassigned files and any group are available as filters. Filters apply
to file rows and ordinary folder-preview children. Folders remain available for
navigation, including folders outside the chosen set; no recursive filesystem
scan is implied by filtering. Archive member previews describe the archive payload,
not independently classifiable ordinary paths. Filtering does not move, rename or
copy files. Selection is reconciled with the filtered listing. Filter choice is
local to this viewing session and starts at All files after restart; definitions
and assignments persist across folders and restarts.

A small bordered name badge distinguishes set membership from session underlines.
Its full hierarchy is available on hover. Context menus and the Sets toolbar menu
both support assignment/removal of the current selection. An inline editor owns
name/color drafts; Save set and Discard set edits are explicit. Closing with an
unapplied draft returns to this editor rather than silently discarding it.

Organization schema version 2 adds `sets`, containing `groups` (`id`, `parent`,
`name`, `color`) and `members` (`path`, `group`). Version 1 migrates by supplying
initial A/B groups without changing existing session data. Future versions remain
errors and are preserved. Limits are 256 groups, 16 hierarchy levels and 16,384
memberships within the existing 8 MiB state budget. Cycles, missing parents/groups,
nil/duplicate IDs, duplicate sibling names and duplicate paths are invalid.

Set membership follows a path reference through ordinary content edits; it does
not assert that a replacement at that path is the previously observed object.
Session marks continue to require their full observed metadata revision. External
moves are not guessed. Browser-managed moves/renames remain part of C8. Concurrent
assignments merge against the latest state. A stale group edit fails rather than
replacing a newer definition; empty-set deletion checks the latest memberships
and hierarchy inside the guarded transaction.
