# References after file operations

This C8 implementation passed native acceptance in both appearances; its
requirement audit and evidence are recorded in `phase8_closeout.md`.

Only confirmed rename results, completed move pairs and studio rename handoffs
retarget references. A partially completed batch contributes its completed pairs;
failed, skipped and unprocessed items do not become moves. Copies do not retarget
or duplicate annotations. Trash/removal and external moves retain path references
rather than guessing a new target. Project relocation discovered outside these
operations still requires explicit registration and the existing identity choice.

Each state owner keeps its own deferred metadata intents and accepted write
lifetime. At most 128 ordered mappings are applied in one transaction. Queue
admission can retry automatically; a failed metadata transaction requires an
explicit Retry or Skip pending reference updates. These actions never repeat the
completed filesystem operation. No matching references means the existing store's
bytes and identity remain unchanged; absent stores are not created for a no-op.

The shared local-state worker, cooperating-writer lock and pinned atomic install
remain the only persistence transport. Latest state is read under that lock;
unknown, malformed and oversized metadata stays intact. Stores commit separately,
so a failure in one does not roll back references already saved by another owner.
The global recovery notice identifies the failed store and remains available in
both Home and folder views. Normal application close is blocked while intents or
accepted writes remain; failures require retry or explicit skipping. A forced
termination can leave stores at different completed checkpoints. On restart each
valid saved store is loaded normally; unavailable references are retained for
manual repair rather than silently removed or inferred from unrelated files.

## Home

Pins keep their IDs when their paths move. Component-aware prefix mapping also
retargets descendants of a moved directory, while similarly named siblings remain
unchanged. Exact file targets retain their raw path bytes, including the absence
of a trailing slash.

If the destination already has a pin, the moving pin's ID wins. Both sets of
cards and artwork bindings are retained and remapped to that surviving ID. The
same remapping applies to layout undo/redo snapshots, preserving the authored
desk and preventing undo from resurrecting orphan references. Camera, titles and
unrelated layout edits are preserved. Existing dirty layout/label changes finish
through their normal save owner before relocation is applied; unapplied drawing
text and active gestures must settle first.

## Session marks and sorting sets

Current marks, cleared-mark recovery and sorting memberships follow the mappings.
A moved source wins an exact destination collision; unrelated records remain.
Relocating recovery entries advances their recovery revision, so stale restore or
forget requests cannot consume a newly retargeted backup.

Marks retain the observed full metadata record, but matching permits the ctime
change expected from a rename. Object, size, mtime and mode must still match. A
same-filesystem rename therefore preserves the review color; replacement, content
changes and cross-filesystem object changes can leave a stored mark dormant until
it is assigned again. A mark remains a personal note, not a permission or content
identity guarantee. Sorting membership is a path classification and follows the
new reference even when the object changes during a completed move.

Relocation acknowledgments do not save or discard unsaved meaning/set drafts.
Definition drafts retain their original comparison base, so a subsequent Save
still merges untouched fields and rejects conflicting edits to the same field.

## Projects

Registered folder references move without changing project UUIDs, title hints or
portable manifests. Relative README/TODO references remain relative to the moved
project folder. A duplicate registered destination is an explicit failure rather
than silently choosing another project's identity. Existing guarded manifest/task
reads still validate the actual project ID before exposing or changing tasks.
