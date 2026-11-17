# Portable-drive completion audit

The accepted plan remains the completion criterion. The previous goal turn made progress (implementation commits and test evidence), but did not prove every requirement. This audit treats unverified or incomplete items as unfinished. No item is marked complete solely because a broader test suite is green.

Audit starting revision: `7202c4ec`. Personal `.cargo/config.toml` and `config.toml` edits remain outside the task. Existing evidence is recorded in `docs/portable-drives.md`; further tests must match each requirement's scope. Host gates do not excuse independent implementation work.

| Plan item | Audit status | Current evidence / remaining work |
| --- | --- | --- |
| 1.1 | Partial | Numeric inventory/capacity and mount routing exist in discovery.rs; verify stable reconnect with changed device paths on disposable media. |
| 1.2 | Partial | UUID ambiguity refusal and fallback buckets exist; explicit fallback disclosure/enrollment needs end-to-end checks. |
| 1.3 | Partial | Reloadable DriveConfig exists; explicit backup selection now uses the existing setting-preserving writer with guarded atomic replacement, alongside operational SQLite state. Lazy selection, restart, raw paths, comments, symlinks, permissions and concurrent/replaced-file refusal pass targeted tests. The full quota/reload acceptance matrix remains. |
| 1.4 | Partial | SQLite operations/baselines/conflicts and durable backup manifests now exist; ready-publication recovery is fault-tested. RAM preflight/allocation journals and atomic session/pair enrollment now recover unpublished setup without deleting published data. Copying-stage/per-item completion and coverage accounting remain. |
| 1.5 | Partial | Bounded admission and retained operations now bind client UUIDs and serialized request fingerprints atomically. Concurrent retries, changed-parameter refusal, queue/transaction failure, completed/restarted results and orphaned-owner recovery are tested; per-item interrupted-operation reconciliation remains. Full reconciliation, RAM data population and RAM export now use four retained worker slots, coalesced per-pair events and conflicting-drive/destination deferral. A 100 GB sparse initial sync and a separate 1 GB RAM population both leave another pair responsive. RAM metadata preflight, quota admission, formatting and initial mounting now use one retained preparation worker; image preflush and unmount authorization now use a retained partition worker. Source/destination pin admission, image/catalog mount controls, retention and startup work still need the full responsiveness matrix. Large-export responsiveness and startup-recovery scheduling need further evidence. Normal daemon signal handling now closes admission under the admission lock, cooperatively cancels queued/active work and joins the owner from a blocking context. Accepted-record completion and singleton release are tested. |
| 1.6 | Partial | Pinned roots and overlap policy project bind subtrees into observed filesystem coordinates. Descriptor-free scheduling scopes retain that observation after pins retire; they cannot authorize payload access. Watch/cache retirement covers overlapping observed regions. Pure projection, real FUSE unmount and startup RAM-backing refusal pass. A final-file bind-mount payload escape is fixed; lookup now uses kernel NO_XDEV before foreign metadata/open hooks, and the retained input still must match the root's device and mount ID. Guest tests prove same-device and cross-device leaf binds cannot supply hashes or backup contents on FAT32/exFAT/NTFS. Directory/policy bind aliases, case-fold aliases, remaining managed-root/image coverage and full authorized-device lifetimes still need the complete matrix. |
| 2.1 | Complete | Guarded partition/subtree enrollment and persistent pair identities are tested. New bidirectional initial/replacement cases interrupt after the first durable file with cancellation, source disappearance, same-size/mtime source changes, destination changes or baseline-publication faults. Reopened registries retain only verified common baselines, preserve installed identities and recover without needless copies. Failed deletion-baseline commits cannot resurrect removed files. All 22 scenarios pass on host storage and real FAT32/exFAT/NTFS sources. See the common-baseline recovery checkpoint. |
| 2.2 | Partial | Native FAT32/exFAT/NTFS3 union and concurrent same-timestamp edits preserve both conflict variants; FAT32/exFAT case-fold collisions explicitly fail with both local originals retained. Remaining remount/event-loss and fault matrices still need evidence. |
| 2.3 | Partial | Watch overflow now clears the root observation epoch; coarse/unknown filesystems never reuse full-scan fingerprints. Direct filesystem and loss/reconnect matrices remain. |
| 2.4 | Partial | Persistent collision-safe complete conflict variants tested; provenance and repeat behavior need a full matrix. |
| 2.5 | Partial | Deletion and delete-versus-edit tests exist; event-loss and cancellation matrices remain. |
| 2.6 | Partial | Native file/directory name tables verify exact directory entries and bytes, or explicit refusal with originals retained. NTFS3 alternate-stream paths and FAT32/exFAT trailing dots/spaces are refused before installation. Native FAT32 oversized-file refusal passes; link/metadata omissions, alternate mount options and ntfs-3g remain. |
| 2.7 | Partial | Changed-file reconciliation, overflow invalidation and independent-drive scheduling tests pass. Coalesced path batches are shared and capped at 4096 paths/1 MiB; registration is incremental with 8192-watch/16 MiB/depth-256 budgets. Real overflow delivery matrices remain. |
| 3.1 | Partial | Capacity, MemAvailable and expanded contents are checked before admission; configurable per-session/aggregate byte, inode and session reservations now exist. Native quota and aggregate refusal tests pass; full pressure matrix remains. |
| 3.2 | Complete | Pinned filesystem and effective-user write/search/read-only checks select a usable tmpfs; RAM and backup admission share the directory check. A disposable guest covers /tmp with each native FAT32/exFAT/NTFS mount: all 36 selection cases pass, including XDG/shm fallback, read-only/unavailable candidates, explicit overrides and writable-tmpfs /tmp preference. Positive cases create 0700 private directories/0600 images on verified tmpfs, mount/write/read/discard successfully. Existing quota/restart/unmount tests pass. See the RAM-directory checkpoint. |
| 3.3 | Complete | Bounded streaming runs on retained workers; initial and explicit-resume jobs use catch-up before successful ready publication. Per-file callback cases deterministically change a same-size/same-mtime source and add an unenumerated path after the first installation, with a one-pass negative control. Cancellation, missing source and changed source preserve accepted files, incomplete/paused state and pending work; verified retry enables browsing. Both cases pass on real FAT32/exFAT/NTFS sources and bounded RAM destinations. Live coordinator cancel/resume and 1 GB independent-session responsiveness pass. See the initial-population checkpoint. |
| 3.4 | Complete | RAM sidebar shows connection state and bounded observed pending-path counts, with explicit full-check/unknown states. Menus and Jobs show the stable drive ID, RAM path and last confirmed drive sync separately from export history. Native removal/offline restart/reconnect and dark/light UI acceptance pass; acknowledgements preserve newer events and exports cannot advance drive confirmation. Counts remain advisory and never authorize eject/discard. See the RAM pending-status checkpoint. |
| 3.5 | Partial | Real bounded-media ENOSPC and controlled removal during a confirmed partial FUSE write pass. Failure/cancellation retain the original portable file, complete RAM edit, pending work and previous confirmation. Reconnect preserves both concurrent conflict variants and converges after catch-up. Physical USB/device-transport and native FAT/exFAT/NTFS interrupted-I/O matrices remain. |
| 3.6 | Partial | Eject includes paused ready RAM mirrors and refuses incomplete population/failed preflush. Native tests also queue eject and unconfirmed discard behind a held write, preserve ownership through cancellation, keep unrelated controls responsive and retain terminal results after reconnect. Broader read-only, dirty-session and eject/discard cancellation matrices remain. |
| 3.7 | Partial | Native quota, restart, directory-fsync kill and cancellation fixtures pass. Bounded guest tests prove available-memory admission refusal, underlying-tmpfs exhaustion/recovery, acknowledged external writes retained across abrupt daemon death while held open, 13 namespace/metadata edits without later data writes/fsync, and immutable-flag recovery with correctly refused truncation. The helper propagates eviction/operation errors and flushes successful mutations before acknowledgement. Interrupted unacknowledged operations and broader concurrent pressure matrices remain. |
| 4.1 | Partial | Lazy selection produces a five-minute review bound to retained partition/destination roots. Cancel/expiry, replaced roots, changed partition facts, one-shot destination after preference changes and failed preference writes are backend-tested. Pending reviews are bounded and released before unmount. Dark/light focused UI runs prove cancellation, file backup publication, and image review; alias and broader failure matrices remain. |
| 4.2 | Partial | Durable checksummed SQLite manifests and filesystem/registry current-version references survive reference-installation faults; pruning removes matching manifests and protects current across clock changes. Admission/execution check working headroom; independent images require their full size estimate before unmount, while Btrfs permits shared allocation with explicit uncertainty. Detailed file-allocation estimation remains. |
| 4.3 | Partial | Bounded retries, metadata fidelity and hard-link preservation/reuse pass native checks. Read-only source guards now reject writable backing filesystems and policy loss on real FAT32/exFAT/NTFS, retaining current; ordinary live collection is preserved. Selectable reviewed/CLI read-only collection now owns flush/unmount/verified mount/copy/reader retirement/restoration. Real UDisks guest busy refusal, cancellation and late-cancel completion pass on all three source formats. Desktop password-agent, broader fidelity/mount-option and interruption matrices remain. |
| 4.4 | Partial | Real read-only Btrfs version snapshots and extent sharing passed; capability failure/admission matrix remains. |
| 4.5 | Partial | Real 4 KiB edits allocate 256 KiB, old bytes intact; strengthen final target verification evidence. |
| 4.6 | Partial | Explicit retention and mounted-provider refusal exist; open-provider/prune interruption matrix remains. |
| 5.1 | Partial | Preflush and unmount run as retained stages. Real owned FUSE integration proves external-reader busy refusal, successful pin retirement, independent RAM writeback during a held unmount and retained completed eject after cancellation. Image cancellation after unmount prevents copy admission. Admission retains the device-node pin and checks its disk sequence across authorized image reads. Reviewed requests show exact capacity/destination/free bytes and bind the selected roots; non-Btrfs full-image space failure precedes unmount. Real guest UDisks/PolicyKit authorization, busy-source refusal and successful preflush/unmount/image/restoration now pass. Interactive password-agent and hotplug/replacement races remain. |
| 5.2 | Partial | Isolated real UDisks/PolicyKit tests now prove authorized image reads, unprivileged denial and busy-device refusal on FAT32/exFAT/NTFS partitions. The desktop authentication-agent/password interaction remains a separate UI gate. |
| 5.3 | Complete | Exact authorized-device/image hashes, partition sizes, durable publication and checksummed source manifests pass on real FAT32/exFAT/NTFS GPT partitions. Manifests retain typed partition number/start/byte offset/logical sector size/type/label/flags and immediate containing-table type/identifier; unknown fields remain unknown. Native tests assert actual GPT layout and exact manifest retention. See the partition-layout checkpoint and its guest report. |
| 5.4 | Partial | Real Btrfs image delta and exact nonzero sectors pass; real non-Btrfs partition images now pass on FAT32/exFAT/NTFS with exact source SHA-256 and retained earlier images. Mount-option and interrupted-device matrices remain. |
| 5.5 | Partial | Optional mounted-state restoration now retains actual cleanup completion after imaging, failure or cancellation, refuses changed device lifetimes and reports separate restoration failures without losing completed backups. Owned authority and real FUSE fixtures pass; Real authorized image success and mounted-state restoration now pass on all three guest source formats. Final UI and interrupted-device acceptance remain. |
| 5.6 | Partial | Real UDisks loop setup/read-only mount/read/unmount passes for each native partition-image format; writes return EROFS. Real FUSE catalog reads through mounted images now pass, with ENOMEDIUM before/after provider availability and ESTALE for lost timestamp precision. exFAT FUSE precision compatibility and concurrent-provider matrices remain. |
| 6.1 | Complete | Version-2 paginated SQLite catalogs preserve raw names, numeric sizes/types, original modification/ctime fields, captured Drive and pinned source mount/namespace, explicit known/unknown driver precision, per-directory coverage and typed stage/reason/errno. Host permission/replacement/raw-name tests, indexed multi-page reads, real FUSE details/errors and native FAT32/exFAT/NTFS precision/metadata checks pass. See the typed catalog coverage checkpoint. |
| 6.2 | Partial | Independent catalog/search rules exist and full backup/sync inclusion is preserved. Exact read-only lookup now loads old whole-drive/folder settings independently of status pagination; dark/light UI acceptance verifies unchanged saves retain both flags. CLI writes require both flags, omitted subtrees work, and successful rule jobs publish exclusions before completion. Operational-baseline accounting and rule-address alias/selector handling remain. |
| 6.3 | Partial | Search exclusion routing is reused by Shelf reference summaries, recursive photo discovery/features and audio constellation discovery/cache reads. Excluded roots are rejected before descent, with rechecks before decoding and publication. Isolated fixtures cover policy changes, cached results, symlink discovery aliases and explicit browsing/reads. Observed policy changes now retire Shelf summaries, photo similarity state and audio map discovery while retaining mutation owners. Broader search/answer/image-order caches, bind/case-fold/relocation aliases and the remaining consumer inventory still need verification. |
| 6.4 | Partial | Future scans honor subtree rules. Saved descendant records now have durable cleanup intents, mounted-reader retention, compaction, restart recovery and separate capture/redaction dates. Real FUSE and dark/light Files acceptance cover pending cleanup, busy readers and reopened Not catalogued boundaries. Broader alias/case-fold and concurrent-provider matrices remain with 6.3/6.8. |
| 6.5 | Partial | Hard bytes/entry/depth/time limits and million-file fixture pass; explicit larger-run UI and worker policy need evidence. |
| 6.6 | Complete | Indexed 512-entry pages preserve raw names, stable inodes and resumable offsets without directory-wide sorts. Real FUSE verifies correct nested dot/parent inodes, the maximum signed cookie, invalid-row EOVERFLOW without server poisoning, partial-listing EIO, completed-parent ENOENT, typed metadata, signed dates, invalid-date EOVERFLOW and multi-megabyte descriptor-backed reads. Original special-file types/link bytes/device numbers remain inspectable through bounded metadata while inert entries skip content jobs. Twenty-two valid mutation calls return EROFS; descriptor access-mode errors retain Linux EINVAL/EBADF. See the inode, type and timestamp checkpoints. Provider revision policy and the broader provider-lifetime matrix remain separately tracked in 6.7/6.8. |
| 6.7 | Partial | Provider metadata/digests validate the same retained descriptor; same-size/mtime changed bytes are rejected when a digest exists, and a stale newer backup falls back to an older match. File backups require their captured directory identity. Capture now reuses current completed file-backup hashes only with verified manifest/root/full revision and queried-change provenance, without source payload reads. Native FAT32/exFAT/NTFS correctly decline metadata-only hash promotion. Sync hash reuse, older capture-enrichment candidates, uncertain revisions and cross-driver precision handling remain. |
| 6.8 | Partial | A real separate-process FUSE fixture proves an existing handle returns ESTALE after its provider changes while a new open selects a newer valid backup. Replaced file-backup directories and catalog self-mounts are rejected. Lookup-time mount boundaries also reject a catalog file bind-mounted into a backup without recursively opening its server. Mounted images now require an atomically published local mount observation bound to boot/namespace, original image/loop, root identity and a non-reused kernel mount ID; same-root bind replacement, missing/changed provenance, held-file staleness and restoration pass on FAT32/exFAT/NTFS images. Root aliases and the broader concurrent-provider/interrupted-cleanup matrix remain. |
| 7.1 | Partial | Core guarded/source and drive tests pass; Old unbound operation records are protected, and atomic idempotent admission/restart/lost-owner fixtures pass. Completed-item cancellation and the other migration cases remain. |
| 7.2 | Partial | Daemon/reconnect/restart fixtures pass; mount-event loss and interruption state tests remain. |
| 7.3 | Partial | Commit `6b876b69` supplies a disposable KVM guest. Real FAT32/exFAT/NTFS format/write/unmount/remount probes pass without host mount privileges or attached media; failed-command and timeout probes are rejected. Eight application tests per filesystem now pass: union/conflicts, deletion policy, versioned file backups/manifests/timestamps, catalog exclusion versus full backup/sync inclusion, case/name handling, and applicable NTFS-stream/FAT32-size checks. Tests use whole-volume loop files with filesystem-UUID fallback; partitioned-device, read-only/full-media and mount-option matrices remain. |
| 7.4 | Partial | Real FUSE, Btrfs, authorized partition-image/loop-provider and temporary read-only file-collection tests pass. Provider concurrency, interrupted-device and capability-failure matrices remain. |
| 7.5 | Partial | Dark/light private acceptance now includes RAM pending counts, drive confirmation, offline export and reconnect, alongside catalog/policy/backup review flows. Resource, broader coverage and interrupted-recovery UI flows remain. |
| 7.6 | Partial | Million-file, delta, sparse-active-backup, RAM byte/inode limits, bounded guest available-memory pressure, underlying-tmpfs exhaustion and acknowledged data/namespace/metadata edits across daemon death pass. A 10 TiB sparse RAM entry is rejected before hashing. Overflow delivery, concurrent pressure/admission and unacknowledged interrupted writes remain. |
| 7.7 | Partial | Prior worktree passed workspace tests, format, clippy and documented architecture review; rerun affected and final gates after remaining changes. |

## Common-baseline recovery after interrupted synchronization

Plan item 2.1 is now Complete. Existing service fixtures cover guarded subtree
enrollment and later link/relocation refusal. The shared reconciliation fixture
now accepts the engine's existing progress callback. New tests persist a selected
subtree pair, retain its local directory identity and reopen its actual registry
after interruption; there are no new production hooks or schema changes.

Twenty cases exercise both copy directions, initial installations and replacement
of an earlier common version. After the first file and its baseline commit, each
case injects cancellation, source disappearance, source bytes changed with size
and modification time preserved, destination changes, or a SQLite baseline
publication failure on the next file. Assertions distinguish the exact failure,
retained completed bytes, uncommitted later paths and each per-file baseline.
The database fault occurs after the next file has actually been installed, so
that completed write remains visible despite the stale comparison baseline.

After reopening the registry, a fresh comparison converges verified data and
retains the identity/revision of previously installed files instead of copying
them again. Concurrent destination edits preserve both conflict variants on both
sides and retain the original baseline. Two additional cases fail the baseline
DELETE after the physical deletion, then reopen and reconcile: neither direction
resurrects the deleted file, and the obsolete baseline is removed. These are
per-file baseline proofs; copying-stage operation-ledger accounting, event loss
and the broader deletion/conflict matrices remain in their separate partial rows.

Both tests pass on owned host storage and on each real FAT32, exFAT and NTFS
source in the disposable VM (22 scenarios per filesystem). Accepted report:
`/tmp/portable-drive-vm-sync-baseline-accepted/report.json`, with success=true,
no timeout, exactly one successful guest marker and three two-test passing
groups. The complete ordinary backend suite passes: 137 tests, 64 explicit
fixtures ignored. Drive all-target clippy with warnings denied, targeted Rust
formatting and whitespace checks pass. Logs use
`/tmp/portable-drive-sync-baseline-` with `host.log`, `backend.log`,
`tests-build.log`, `vm.log` and `clippy.log`.

The original-baseline architecture result remains REGRESSION: 4300 clone groups,
144 cycles, average CCN 3.48, maximum 274 and duplication 7.07%. There is no
additional reported hotspot, clone group or dependency cycle from these tests.
Report: `.fitness/portable-drives-sync-baseline-architecture.json`.

## Initial RAM population interleavings and recovery

Plan item 3.3 is now Complete. Inspection confirms preparation publishes an
incomplete session, initial/resume requests set the initialization flag, roots
are watched before worker submission, and the retained worker performs initial
and catch-up comparisons before its result is published. The successful-result
transaction is the only path enabling the new session's ready state. Directory
inventory stays in bounded SQLite batches and file data uses fixed-size streaming
buffers, independently from the coordinator and GUI.

New tests use the actual worker's per-file progress callback, the shared existing
bounded RAM fixture and real FUSE mounts. After `a` has been installed and before
`b`, a source edit preserves both size and modification time and a new `c` is
added outside the original inventory. A one-pass control deliberately misses both
changes; initialization's catch-up converges all bytes before ready publication.
The persisted session remains non-browsable until that successful result is
published. This is live collection, not an atomic filesystem snapshot; ongoing
later edits retain their normal pending/watch semantics under 3.4.

Three deterministic failure cases cancel, remove `b`, or change its bytes while
preserving size/mtime after the initial inventory. They assert the specific
cancellation/ENOENT/digest-mismatch result, complete `a` retention, no installed
staging or `b`, initialized/live backing, paused incomplete state, retained full
check and no drive confirmation. An explicit fresh comparison then preserves
all source bytes, enables browsing, unpauses the pair and acknowledges only the
successful work. The fixture reuses TestDrives within cfg(test); no production
hooks, helper protocol, runtime behavior or persisted schema changed.

The two parent tests pass both on owned host storage and against each real
FAT32/exFAT/NTFS source in the disposable guest. The final guest report is
`/tmp/portable-drive-vm-ram-population-accepted/report.json`, with success=true,
no timeout, one successful guest marker and three two-test passing groups.
Host log: `/tmp/portable-drive-ram-population-host-accepted.log`.
The live coordinator's existing cancellation/resume test passes in
`/tmp/portable-drive-ram-population-owner-cancel.log`, retaining accepted `a`,
refusing eject while incomplete and completing/resuming the larger file before
enabling browsing. The separate 1 GB population fixture keeps another actual RAM
session promptly writable: `/tmp/portable-drive-ram-population-owner-responsive.log`.

All 135 ordinary backend tests pass (64 explicit fixtures ignored), as do the
drive all-target clippy gate with warnings denied, targeted Rust formatting and
whitespace checks. Logs use `/tmp/portable-drive-ram-population-` with
`backend.log`, `tests-accepted-build.log`, `clippy.log` and `fmt.log`.
No host mount privileges, attached media, live daemon or desktop were used.

The original-baseline architecture result remains REGRESSION: 4300 clone groups,
144 cycles, average CCN 3.48, maximum 274 and duplication 7.07%. Shared barrier,
convergence and publication assertions remove the two initial fixture clones.
There is no additional reported hotspot, clone group or dependency cycle.
Report: `.fitness/portable-drives-ram-population-architecture.json`.
Memory-pressure, event-loss and removal/device-transport matrices remain under
their existing separate rows; this does not complete the whole accepted plan.

## RAM directory fallback and private backing

Plan item 3.2 is now Complete. The isolated guest overlays `/tmp` with actual
FAT32, exFAT and NTFS mounts rather than assuming the host's tmpfs layout.
Twelve child-environment cases per source format prove default XDG/shm fallback,
unset/missing/disk-backed XDG, a symlink resolving to verified tmpfs, explicit
disk-backed/tmpfs/read-only overrides, no usable candidate, read-only default
locations and writable-tmpfs `/tmp` preference. The parent environment is unchanged.

The negative run reproduced selection of a read-only XDG directory on all three
profiles: `/tmp/portable-drive-vm-ram-fallback-negative/report.json` is expected
failure evidence. Selection now skips unusable default candidates, and an explicit
non-default override fails instead of being redirected. The existing backup
destination write/search/read-only check is shared through Root's retained
directory admission, preserving execution-time mutation checks and backup space
accounting. No configuration defaults, public formats or removable contents changed.

Each successful case verifies the chosen root and backing image are actually on
tmpfs, the directory is 0700 and image 0600, then mounts the bounded filesystem,
writes/flushes/reads bytes, stops the helper and discards the owned backing. These
prove location and private backing, not the separate memory/admission pressure
requirements in 3.1/3.7. Tmpfs can still swap; there is no physical residency claim.

Accepted guest evidence: `/tmp/portable-drive-vm-ram-fallback-accepted/report.json`, with
success=true, no timeout, one successful guest marker, three passing parent tests
and all 36 expected case markers. Each child must emit exactly one completion
marker after its checks. The 27 successful-selection cases each exercise
the real formatter and FUSE helper; nine cases correctly refuse selection.
All 135 ordinary backend tests pass (62 explicit fixtures ignored), including the
existing unprivileged unwritable-backup preview refusal. Both real RAM quota/
restart/unmount fixtures and both real Btrfs backup cases pass. All-target drive
clippy with warnings denied,
targeted formatting and whitespace checks pass. Logs use
`/tmp/portable-drive-ram-fallback-` with `backend.log`, `native-ram.log`,
`verified-tests-build.log`, `btrfs.log`, `clippy.log` and `fmt.log`.

The original-baseline architecture result remains REGRESSION:
4300 clone groups, 144 cycles, average CCN 3.48, maximum 274 and duplication
7.07%. This step adds no reported hotspot, clone group or dependency cycle.
Report: `.fitness/portable-drives-ram-fallback-architecture.json`.
The other acceptance rows retain their existing scope and unfinished status.

## Mounted-image provider lifetime

Image providers previously used only their saved mount pathname. The new local
`image-mount` entity captures the backup/image path, UDisks loop object, root
device/inode/mount identity, known namespace, boot UUID and kernel unique mount
ID after successful read-only mounting. Mount and backup records publish in one
transaction. `STATX_MNT_ID_UNIQUE` (Linux 6.8+) prevents ordinary mount-ID reuse
from authorizing a replacement; unsupported/unknown identity refuses admission.
The [statx manual](https://man7.org/linux/man-pages/man2/statx.2.html) specifies
that this ID is not reused during a running system. A checked libc statx call is
needed because this workspace's rustix masks out that newer request bit.

Catalog candidates now distinguish original, file-backup and image identities.
Images require the matching runtime observation and validate the retained root
before payload admission. Missing/malformed/legacy or mismatched observation
records do not supply bytes. A held provider keeps its existing pinned-root and
file-revision checks. This closes the demonstrated image-mount replacement case;
it does not prove immutable payload bytes when metadata alone is uncertain (6.7).

Failure cleanup shares normal image retirement using the actual in-memory owner,
including a successful mount not yet published to SQLite. It attempts to persist
that mounted owner for retry, attempts cleanup even if that journal fails, and
includes journal/cleanup failures with the original error. Final loop retirement
atomically removes the runtime observation and writes the final backup state.
No public protocol/manifest schema changed and no removable-media files were added.

The real authority/image guest passes four cases on each FAT32/exFAT/NTFS source
profile. Its image case now injects an observation-publication failure after
mounting and verifies both state and losetup show no remaining mapping. Successful
admission is reloaded through another SQLite connection. Binding the same image
root over its own pathname makes new and held catalog reads return ESTALE;
removing the bind restores readable original bytes. Missing observations return
ENOMEDIUM, and changed boot/namespace/unique-ID observations return ESTALE.
Restoring the record permits reads; successful unmount removes the observation.
Report: `/tmp/portable-drive-vm-image-mount-owner-final/report.json`, with exactly
one successful guest marker, no timeout and all three four-test groups passing.

All 135 ordinary backend tests pass (60 explicit fixtures ignored), including
unique-mount/boot observation and writable-view refusal. All 21 selected host
provider cases pass. Logs: `/tmp/portable-drive-image-mount-owner-backend.log` and
`/tmp/portable-drive-image-mount-owner-providers-final.log`. Final helper/test builds,
drive all-target clippy with warnings denied, targeted formatting and whitespace
checks pass; logs use the same prefix with `helper-build.log`, `tests-build.log`,
`clippy.log` and `fmt.log`. No host mounts, attached media or live daemon were used.

The original-baseline architecture result remains REGRESSION: 4300 clone groups,
144 cycles, average CCN 3.48, maximum 274 and duplication 7.08%. This step adds
no clone group or dependency cycle. The existing mount_image hotspot grows from
22 to 24 due to retained failure cleanup; resolve stays at 25. Observation helpers
remain below the hotspot threshold. Report:
`.fitness/portable-drives-image-mount-owner-architecture.json`.
Item 6.8 remains Partial for source-root aliases, broader concurrent-provider and
interrupted cleanup/recovery coverage. The full accepted plan remains unfinished.

## Lookup-time mount boundaries

The regression fixture first reached a foreign catalog's payload-open handler
and received ENOMEDIUM before Root rejected the mount. The pre-fix guest report
is `/tmp/portable-drive-vm-no-xdev-negative/report.json` (expected failure on all
three boot profiles). This established that a final-file mount check after
open could recurse into a provider's own single-threaded catalog server.

Root now shares one descriptor-relative O_PATH child lookup using openat2 with
BENEATH, NO_SYMLINKS and NO_XDEV. Directory traversal, final metadata and content
inputs all use it. Kernel EXDEV is preserved before foreign getattr/open hooks;
guarded content reopening uses the retained pin, with the existing final
mount/device check retained. Leaf symlink metadata remains available but content
inputs explicitly require regular files. Linux 5.6+ openat2 support is required;
unsupported calls fail without a weaker fallback. No persisted format changes.

The guest test uses actual FAT32/exFAT/NTFS source roots and a separate catalog
helper. It checks file and directory binds, partial catalog coverage retaining
EXDEV, and a catalog file bound into its own real backup provider. The recursive
read fails promptly with ENOMEDIUM while metadata stays usable, and removing the
bind reveals the untouched backup bytes. Consolidated native cases retain
same-device, cross-device and symlink-inode binds, original source bytes and
backup refusal. Their shared errno assertion is reused from the existing catalog
tests; the guest mount owner is reused without another cleanup implementation.

Validation includes 133 ordinary backend tests (60 explicit fixtures ignored),
21 selected host provider tests, both Btrfs fixtures, the source-read trace with
its positive control, 15 native/catalog cases per source format and four
authority/image cases per format. Accepted logs use `/tmp/portable-drive-no-xdev-`
with `backend-accepted.log`, `providers-accepted.log`, `btrfs-final.log`,
`capture-trace-final.log`, `clippy-final.log` and `fmt-final.log`. Guest reports:
`/tmp/portable-drive-vm-no-xdev-native-coverage/report.json` and
`/tmp/portable-drive-vm-no-xdev-authority-verified/report.json`.
An earlier broad native filter selected authority-only tests without their
services; `/tmp/portable-drive-vm-no-xdev-native-accepted` is rejected evidence.

The original-baseline architecture comparison remains REGRESSION, with unchanged
4300 clone groups, 144 cycles, average CCN 3.48, maximum 274 and duplication
7.08%. Shared assertions and consolidated bind scenarios removed the two initial
extra test clones. Report: `.fitness/portable-drives-no-xdev-architecture.json`.
Items 1.6 and 6.8 remain Partial: source-root admission/replacement aliases,
image-mount lifetimes and the remaining provider matrix are still unfinished.

## Verified backup fingerprints during catalog capture

Capture optionally imports existing SHA-256 fingerprints from the current
completed file backup. The lookup is bounded to the drive's current file version;
it does not load or scan all backup history. Drive/evidence, expected manifest
location, captured source-root device/inode, regular-file kind and the complete
per-entry revision must agree. Invalid/missing hashes are not imported.

Precise stored timestamps alone do not qualify. Both collection and capture must
support queried change timestamps: the conservative current allowlist requires
Linux 7 or later and an observed ext4/Btrfs/XFS/tmpfs driver. Collection records
this provenance in the checksummed manifest; missing/false legacy observations
remain ineligible. A zero fractional ctime is also rejected per entry to exclude
coarse/legacy inode observations. The kernel's
[multigrain timestamp documentation](https://docs.kernel.org/7.2/filesystems/multigrain-ts.html)
describes the queried-change mechanism. The v7.0
[ext4](https://raw.githubusercontent.com/torvalds/linux/v7.0/fs/ext4/super.c),
[Btrfs](https://raw.githubusercontent.com/torvalds/linux/v7.0/fs/btrfs/super.c),
[XFS](https://raw.githubusercontent.com/torvalds/linux/v7.0/fs/xfs/xfs_super.c) and
[tmpfs](https://raw.githubusercontent.com/torvalds/linux/v7.0/mm/shmem.c)
registrations opt in through FS_MGTIME. This admission does not make capture an
atomic payload snapshot or qualify FAT32, exFAT, NTFS, FUSE or network drivers.

The existing publication validator now supplies the shared manifest reader.
Checksum, SQLite queries, header validation and before/after revision checks
retain one original file descriptor. Replacing its pathname cannot retarget the
callback. Enrichment uses the remaining capture time and catalog byte budget,
bounded cursor pages and one transaction before releasing final database headroom.
Corruption, replacement, unsupported provenance, oversized input or a late failed
row rolls back imported hashes while preserving the ordinary metadata catalog.
Cancellation still aborts capture; rollback errors are not hidden.

No source payload hashing was added to inventory. Current- and late-rule selection
also share the existing paginated exclusion visitor, retaining each caller's
transaction. This removes the duplicate rule filter and leaves capture CCN 15.
The optional collection fact uses existing manifest version 1 and catalog version
2 fields; no wire/schema version changes and no removable-media marker files.

Validation against the final test executable:

- 132 ordinary backend tests pass, with 60 explicit fixtures ignored:
  `/tmp/portable-drive-catalog-hash-tracking-backend-final.log`.
- Five hash/manifest cases pass, including changed bytes preserving size/mtime,
  changed source ctime, corrupt/wrong-root/wrong-evidence/oversized/legacy input,
  transaction rollback, retained descriptor/name replacement and cancellation/time
  admission: `/tmp/portable-drive-catalog-hash-tracking-host-accepted.log`.
- An explicitly run strace fixture passes with actual imported hashes: no source
  payload reads during capture, and an intentional control read is detected.
  `/tmp/portable-drive-catalog-hash-tracking-trace-accepted.log`; raw traces:
  `/tmp/portable-drive-catalog-hash-source-trace-{false,true}.log`.
- Twenty-one real FUSE/provider cases and both real Btrfs version fixtures pass:
  `/tmp/portable-drive-catalog-hash-tracking-providers-final.log` and
  `/tmp/portable-drive-catalog-hash-tracking-btrfs-final.log`.
- The disposable native guest passes 15 cases each on FAT32/exFAT/NTFS3, including
  a saved backup hash that is correctly not promoted on these source drivers:
  `/tmp/portable-drive-vm-catalog-hash-tracking-native/report.json`.
- The authority/image guest passes four cases per filesystem, with the real
  UDisks/PolicyKit and mounted image/catalog flows:
  `/tmp/portable-drive-vm-catalog-hash-tracking-authority/report.json`.
- Both guest reports have success=true and exactly one successful guest marker;
  neither timed out. No host mounts or attached user media were used.
- Backend/helper builds, all-target drive clippy with warnings denied, targeted
  Rust formatting and diff whitespace checks pass. Logs:
  `/tmp/portable-drive-catalog-hash-tracking-helper-build.log`,
  `/tmp/portable-drive-catalog-hash-tracking-clippy-final.log` and
  `/tmp/portable-drive-catalog-hash-tracking-fmt-final.log`.

The final architecture comparison against `pre-portable-drive-services` remains
REGRESSION: 4300 versus 4289 clone groups, 144 versus 143 cycles, average CCN
3.48 versus 3.44, unchanged maximum 274 and duplication 7.08% versus 7.30%.
This step adds no reported clone group or high-complexity function; branch-wide
cleanup remains required. Compact report:
`.fitness/portable-drives-catalog-hash-reuse-architecture.json`.

Item 6.7 remains Partial. Sync hashes, older enrichment candidates, uncertain
revision policy and cross-driver precision need further implementation/evidence.
Large hash-checked opens also remain part of the responsiveness work in 1.5.
This checkpoint does not claim the rest of the accepted plan is complete.

## Catalog inode and directory-cookie correctness

Regression tests reproduced three defects before the fix: negative SQLite IDs
were accepted as virtual inodes, an out-of-range inode panicked during signed
subtraction, and actual kernel directory entries reported the root inode as the
parent of `a/b` instead of the captured inode of `a`. All three failures are in
`/tmp/portable-drive-catalog-inodes-negative.log`. The retained pre-fix helper is
`/tmp/little-clip-catalog-parent-inode-helper`, SHA-256
`12f4107cc06a3cc4ce690dcb13b6eb249a2b41f9991b57a022705fcbeb184610`.

One checked row-to-inode conversion now serves lookup and directory pages. It
keeps the existing positive-rowid-plus-two representation, reserves the root and
directory-cookie prefix, and refuses values outside Linux's signed seek range.
Unknown/reserved/out-of-range incoming inode IDs have no row and cannot wrap into
negative SQLite IDs or poison the owner mutex. Negative page offsets return
EINVAL. The metadata owner resolves directory paths to captured directory inodes;
a missing, excluded or non-directory parent returns EIO rather than inventing a
root association. The protocol adapter uses that owner for the `..` entry.
No persisted format or valid existing inode mapping changes.

The kernel boundary is verified directly, rather than inferred from a cast:
an entry with the largest valid signed cookie appears exactly once, stat and
directory enumeration agree on its inode, and enumeration resumes to EOF.
An overflowing row produces EOVERFLOW for lookup and listing while another valid
entry remains accessible. Root, one-level and deeper `.`/`..` entries match their
actual captured directory/parent inodes. Pure cases also cover all reserved IDs,
signed extremes, missing/non-directory parents and the negative-offset error.

Validation:

- 126 ordinary backend cases pass, with 58 explicit fixtures ignored:
  `/tmp/portable-drive-catalog-inodes-backend-final.log`.
- Twenty-one selected provider/catalog cases pass, including all five new inode
  cases, in `/tmp/portable-drive-catalog-inodes-provider-final.log`. These retain
  the previous signed-time, type/link, read-only, partial-listing and held-provider
  checks. The guest-only device-node case is excluded from the host selection.
- Current in-process metadata-only and legacy-reader/redaction checks pass in
  `/tmp/portable-drive-catalog-inodes-inventory-final.log` and
  `/tmp/portable-drive-catalog-inodes-redaction-final.log`. Inventory still opens
  zero file payloads.
- Backend/helper builds, all-target drive clippy with warnings denied, targeted
  formatting and diff checks pass. Logs use `/tmp/portable-drive-catalog-inodes-`
  with `accepted-build.log`, `accepted-helper.log`, `clippy-final.log` and
  `fmt-final.log`. The tested helper SHA-256 is
  `3ef5c041b026af194415e4fa99f504bad8a89d19f72b95b160a63699ed6ae77f`.

Architecture remains REGRESSION against the original full-feature baseline, with
4300 versus 4289 clone groups and 144 versus 143 cycles, unchanged from the prior
checkpoint. Duplication is 7.09% versus 7.30%, average CCN 3.48 versus 3.44, maximum
274 unchanged. Directory-parent resolution belongs to the existing metadata owner
and is tested independently; the protocol callback stays at CCN 14, its inverse
directory lookup at 5, and the checked mapping at 3. Page enumeration remains at
15. Valknut additionally flags the necessary readdir entity, without a new
excessive-CCN function or clone group. Selected results are retained in
`.fitness/portable-drives-catalog-inodes-architecture.json`.

Combined with the preceding real-mount type/time/read-only and rendered checks,
this completes the FUSE implementation requirement in item 6.6. Matching-provider
uncertainty, the broader provider replacement/concurrency matrix, other plan items
and the full goal remain unfinished.

## Catalog types and inert special-file metadata

The original mode bits identify FIFO, socket, character-device and block-device
entries independently of their names. The shared core catalog-status protocol
now exposes those typed observations, and Files retains them while disabling
payload-format classification. The mounted representation deliberately remains
an inert regular-file metadata placeholder: open returns EOPNOTSUPP rather than
producing an empty successful file or activating a kernel pipe/device. Original
file types remain available in the saved record, rather than being silently
reported as ordinary source files. A real FIFO mode would select the kernel's
pipe operations and bypass the catalog's payload callbacks; the primary kernel
sources are linked in `docs/portable-drives.md`.

Catalog scans now distinguish deliberately unread special-file payloads from
missing metadata. A fully inventoried special entry does not make a metadata-only
capture incomplete. Ordinary backup/sync scans retain their unsupported-special
warning. Optional `special_device` preserves the captured raw device number for
character/block nodes; older records have None, without inventing device zero.
Normal file records omit this field and retain their existing representation.

`system.little-clip.catalog.entry` supplies the lossless saved entry as bounded
JSON (maximum 64 KiB, E2BIG for oversized records). It works independently of
newer directory/source coverage fields. Linux permits this metadata-only system
attribute on a symlink itself, whereas it rejects user attributes on symlinks
before calling FUSE. This preserves inspection of blocked absolute, escaping
and uncaptured link targets without following them. Relative links that resolve
to the catalog root now work; cyclic links retain normal ELOOP behavior.

Files uses its shared entry-icon selection for special metadata, and both preview
admission and painting bypass content caches for these entries. The first dark
render passed its text observations but exposed generic folder icons; it is
retained under `/tmp/portable-drive-catalog-types-dark-initial/` and is not used
as final visual acceptance. The preview regression test proves that a saved
special entry named like a MIDI file does not admit a content job or allocate a
preview identity. Ordinary files carrying the same status xattr remain ordinary
because core first verifies the owned FUSE mount identity.

The expanded real-mount mutation fixture checks twenty-two namespace, data,
permission, ownership and xattr calls, including descriptor variants, FIFO/node
creation and both rename flags. Each returns EROFS and the original and backup
bytes remain unchanged. Read-only descriptor truncation, positioned writes and
allocation retain Linux EINVAL/EBADF admission errors.

Validation:

- 123 ordinary backend cases pass, with 56 explicit fixtures ignored, in
  `/tmp/portable-drive-catalog-types-backend-final.log`. Sixteen selected provider
  cases, including the real-mount metadata/type/link/mutation fixtures, pass in
  `/tmp/portable-drive-catalog-types-provider-accepted.log`. The guest-only node
  test is explicitly excluded from that host selection.
- The isolated guest device-node case passes in
  `/tmp/portable-drive-vm-catalog-types-devices-accepted/report.json`, with both
  success=true and its successful guest marker. It creates only owned temporary
  character/block nodes on guest tmpfs and never opens the originals for I/O.
  The existing filesystem harness invokes it in each of its three phases; this
  does not claim that FAT/exFAT/NTFS support Unix device nodes.
- Two core catalog protocol/ordinary-source tests pass in
  `/tmp/portable-drive-catalog-types-core-final.log`; nine Files preview cases
  pass in `/tmp/portable-drive-catalog-types-previews-accepted.log`.
- Dark and light private-desktop runs each pass four observations in
  `/tmp/portable-drive-catalog-types-{dark,light}-accepted/report.json`. Final
  screenshots were visually inspected: metadata icons and type labels remain
  readable, root-link navigation returns the same contents, and refused pipe
  directory navigation preserves the listing and visible notice actions.
- Helper/app/fixture builds, affected all-target clippy with warnings denied,
  targeted Rust formatting, Python compilation and rustdoc generation pass.
  Logs use `/tmp/portable-drive-catalog-types-` with `final-accepted-build.log`,
  `accepted-ui-build.log`, `clippy-accepted.log`, `fmt-final.log` and
  `rustdoc-final.log`. The generated special-kind/status/attribute API pages
  contain the expected rendered descriptions. The tested helper SHA-256 is
  `12f4107cc06a3cc4ce690dcb13b6eb249a2b41f9991b57a022705fcbeb184610`.

The full-feature architecture comparison remains REGRESSION against the original
baseline: clone groups 4300 versus 4289, cycles 144 versus 143, duplication 7.09%
versus 7.30%, average CCN 3.48 versus 3.44, maximum 274 unchanged. This checkpoint
adds no clone group after sharing fixture preparation and inert-open assertions.
The two flagged preview clones are existing fragments: local jscpd checks against
`e7742aa0` retain the same fragment hash `39e9795c7d7e818f` before and after.
Shared entry-icon reuse keeps preview painting at CCN 15. The bounded symlink
validation callback has CCN 13; Valknut newly flags that necessary path-validation
entity after the root-link case was added. Selected findings and local clone
report paths are retained in `.fitness/portable-drives-catalog-types-architecture.json`.

The inode checkpoint above completes item 6.6's parent/offset handling.
Provider uncertainty/concurrency, the final architecture gate and the full goal
remain open.

## FUSE timestamp projection and bounded protocol upgrade

A separate-process real-mount regression test reproduced an upstream fuser 0.14
wire conversion error: source `(-2 seconds, 250000000 nanoseconds)` was presented
as `(-1, 750000000)`. The retained failing helper is
`/tmp/little-clip-catalog-fuser014-helper`, SHA-256
`75a9e686c32a86565c82eeaed1d063872137a33344dbe8e8c0788d3f5e018b67`;
the original failure is recorded in `/tmp/portable-drive-catalog-time-negative.log`.
The final current test executable also fails against that retained helper in
`/tmp/portable-drive-catalog-fuser-old-negative-final.log`, confirming the same
assertion distinguishes the old and upgraded helper.

The drive helper now uses upstream fuser 0.18's corrected signed conversion.
The unrelated fuse-binwalk dependency retains its existing fuser 0.14 version.
The typed protocol adapter reuses the existing catalog, SQLite owner, held
providers and checked `backup::time` conversion. Its mutex satisfies the new
library's `Sync` callback contract; one configured request thread preserves the
existing ownership and admission model. Production and in-process tests share
the same mount configuration. The mount advertises a 1 MiB maximum read, matching
the existing request allocation bound, and retains the 128-handle limit.

Attribute projection now returns `EOVERFLOW` for invalid saved nanoseconds,
capture/redaction seconds outside the signed range, and redaction-revision
addition overflow. It does not fabricate a substitute date or rely on upstream
saturation. Original modification dates and virtual redaction ctime keep their
separate meanings. No catalog/database format changes are required.

Validation:

- 120 ordinary backend cases pass, with 53 explicit fixtures ignored, in
  `/tmp/portable-drive-catalog-fuser-backend-final.log`.
- Eleven provider/metadata cases pass in
  `/tmp/portable-drive-catalog-fuser-provider-final.log`. These include six actual
  helper/legacy-helper cases: exact negative and positive fractional dates,
  corrupt saved nanoseconds returning EOVERFLOW while other entries remain
  accessible, multi-megabyte sequential/offset reads, thirteen mutation calls
  returning EROFS, old-schema helper refusal, partial listings and provider
  replacement. The ordinary cases also check root/revision overflow without
  panic. The mutation test verifies both original and backup bytes after all
  attempted writes; read-only descriptor truncation/write return EINVAL/EBADF.
- Both migrated in-process metadata-only/redaction-reader fixtures pass in
  `/tmp/portable-drive-catalog-fuser-inprocess-final.log`. Metadata inventory
  still performs zero payload opens.
- Four real partition/authority cases per FAT32, exFAT and NTFS3 source pass
  (twelve total), including catalog payload reads through mounted images:
  `/tmp/portable-drive-vm-catalog-fuser-native-final/report.json`. The report
  contains success=true and the successful guest completion marker, and each
  filesystem's four-case result is present in `guest.log`.
- Helper/backend builds and all-target drive clippy with warnings denied pass;
  logs use `/tmp/portable-drive-catalog-fuser-` followed by `new-build.log`,
  `helper-build.log`, `clippy-final.log` and `fmt-final.log`. The tested current
  helper SHA-256 is
  `2611b96641ee8a3e708d21e17b7150195ca20d56b376e40ff3cde4eb44a27ddf`.

The full-feature architecture comparison still reports REGRESSION against
`pre-portable-drive-services`: 4300 clone groups versus 4289 and 144 cycles versus
143, unchanged from the preceding checkpoint. Duplication is 7.09% versus 7.30%,
average CCN 3.48 versus 3.44, maximum 274 unchanged. The new protocol adapter and
metadata tests add no reported hotspot. Selected evidence is retained in
`.fitness/portable-drives-catalog-time-architecture.json`. File-type fidelity,
remaining mutation calls, uncertain provider equivalence and full final gates
remain open; this checkpoint does not complete item 6.6 or the full goal.

## Typed catalog coverage and source precision

New version-2 catalog databases retain per-directory direct-listing and subtree
coverage, typed failure stage/reason/errno, the captured Drive, and core's existing
source mount/namespace identities. These are dated observations, never payload
access authority. Raw paths, sizes, modes/types and timestamp seconds/nanoseconds
remain unchanged. Modification-time storage resolution is a separate fact for
verified kernel FAT, exFAT and NTFS3 mounts; unrecognized and FUSE drivers remain
unknown. The descriptive precision field never loosens provider revision/hash
checks. The driver sources are linked in `docs/portable-drives.md`.

The shared scanner now owns depth-bounded frames, a 128-entry transactional batch,
progress and the final coverage report. Catalog directory completion is written
only after child rows commit and the retained directory identity is confirmed.
Unreadable roots/directories and replaced directory objects remain incomplete.
Unfinished records inherit the global entry/byte/time/database stop reason, while
an earlier direct diagnostic is retained. Configured exclusions stay separate
from depth limits; a depth-limited folder remains a partial directory. Ordinary
sync/backup scans retain their existing batch policy and persisted entry shape.

The FUSE server exposes bounded JSON in `user.little-clip.catalog.details` and
labels incomplete folders separately from Not catalogued boundaries. Exhausting
an incomplete listing returns EIO after its known entries; an unknown child of
that directory returns EIO instead of claiming absence. A completed parent's
missing child still returns ENOENT, and a captured empty directory lists normally.
Directory pages use the existing rowid inode/offset scheme with a new ordinary
SQLite `(scan,parent)` index and a rowid range predicate. A 1,028-name test proves
stable raw names/inodes/offsets across pages, and EXPLAIN confirms a rowid seek
without a temporary directory-wide sort. No payload opens occur in that test.

The version gate prevents old helpers from silently ignoring the new incomplete
listing semantics. The current helper still accepts version-1 catalogs with
unknown details. Ordinary drive state and backup manifests stay at version 1.
A retained version-1 helper refuses a new catalog before mounting; the tested
binary is `/tmp/little-clip-catalog-schema1-helper` (SHA-256
`92c27944ff0a7d825840db34abe1cf11d4d80912848348c86efd536f3f640057`).

Metadata redaction removes descendant entry diagnostics with the original rows,
clears the boundary's old coverage and updates retained ancestor/root coverage.
Capture dates and redaction revisions keep their existing separate meanings.
The source identity record contains no descendant-name inventory. New summary
messages avoid copying per-path failure names into retained ancestor summaries.

Validation:

- 119 ordinary backend cases pass, with 50 explicit native/scale fixtures ignored:
  `/tmp/portable-drive-catalog-coverage-backend-accepted.log`. Seven new host cases, together with the existing database-cap case,
  cover directory completion, labels versus observed mount type, original dates,
  raw-name failures/legacy unknowns, actual EACCES as an ordinary user, root/depth/
  entry/database coverage, multi-page reads, directory replacement, and redaction.
- Core catalog compatibility/ordinary-source protection tests pass two cases;
  Files drive logic passes fourteen. Selected all-target clippy, app/helper build,
  formatting and Python compilation pass. Logs use the
  `/tmp/portable-drive-catalog-coverage-` prefix with `core-final.log`,
  `files-accepted.log`, `clippy-accepted.log`, `ui-build-accepted.log` and
  `fmt-accepted.log`.
- Three separate-process helper cases pass in
  `/tmp/portable-drive-catalog-coverage-fuse-current.log`: old-helper refusal,
  partial-listing metadata/EIO/ENOENT including core inventory errors, and retained
  provider/stale-handle behavior. The first partial fixture passed its checks but
  failed when its second mount reused an existing directory; mount paths now
  include the catalog ID. That failed run is not counted as acceptance.
- Fifteen native cases per FAT32/exFAT/NTFS3 source (45 total) pass in
  `/tmp/portable-drive-vm-catalog-coverage-native-final/report.json`. These verify
  observed driver precision and exact source modification fields alongside prior
  backup/sync/safety coverage. The twelve image/read-only authority cases also pass:
  `/tmp/portable-drive-vm-catalog-coverage-authority-final/report.json`. Both real
  Btrfs cases remain green in `btrfs-final.log` with the checkpoint log prefix.
- The million-file fixture passes in
  `/tmp/portable-drive-catalog-coverage-scale-final.log`: excluded traversal takes
  about 0.0022 seconds, a 1,000-entry limited capture about 0.060 seconds, its
  database is 561,152 bytes, and peak process RSS is about 13 MiB. This is process
  memory, not the fixture's kernel/tmpfs allocation.

Rendered verification distinguishes Partial catalog, Not catalogued and actual
empty folders. The first status layout adjustment put controls below the visible
dock; the final layout reserves actions first, wraps text in its own left-aligned
column and skips an empty pending row. It shares the existing status presentation
with retained notices. Tests verify actual Retry/Dismiss captions before clicking,
using bounded button-interior OCR because full-window OCR omits small bordered
controls. Earlier failed visual/OCR runs remain under the preceding checkpoint
log names rather than being counted as passes.

Final dark/light coverage runs each pass five observations, including verified
Retry and Dismiss clicks, in
`/tmp/portable-drive-catalog-coverage-actions-{dark,light}-accepted/report.json`.
Dark/light exclusion-boundary runs each pass three observations in
`/tmp/portable-drive-catalog-boundary-actions-{dark,light}-accepted/report.json`.
Both final layouts were visually inspected: controls remain visible, text stays
in its own column, and the alert icon does not overpaint the message.

The full-feature architecture result remains REGRESSION against
`pre-portable-drive-services`: 4300 clone groups versus 4289, reported cycles
144 versus 143, duplication 7.10% versus 7.30%, average CCN 3.48 versus 3.44,
and maximum 274 unchanged. This checkpoint adds no clone group or new hotspot
relative to `2e7191ae`. Shared fixture preparation removes its first two new
clones. Catalog capture falls from CCN 18 to 16, and the old monolithic scan's
CCN 30 is replaced by cohesive traversal, commit and report methods at or below
15. Existing provider resolution remains unchanged. Selected fields are retained
in `.fitness/portable-drives-catalog-coverage-architecture.json`.

Item 6.1 is complete, and the inode checkpoint completes item 6.6. Items 6.5,
6.7–6.8 and the final architecture/workspace gates remain open. This coverage
checkpoint alone does not establish full catalog type/time fidelity,
cross-driver provider equivalence or the complete interrupted-device matrix.

## Selectable read-only file collection

File-backup reviews now offer **Temporarily mount read-only for collection**;
`clipctl drives backup DRIVE --read-only` and `start-backup TOKEN --read-only`
select the same policy. Live file collection remains the default. The new
`ReadOnlyBackup` / `StartReadOnlyBackup` wire commands fail on older daemons;
adding an optional field to the old command would allow older serde consumers
to ignore it. Existing live/image request bytes remain unchanged, including
request-identity fingerprints. An image review is rejected without consuming its
token. The file-tree and checksummed SQLite manifest format is unchanged.

The existing retained partition worker now owns enrolled preflush, ordinary
unmount, UDisks read-only mount and confirmation. Device lifetime, namespace and
strong root guards are reused. A descriptor-free owned-mount observation retains
only cleanup authority and scheduling scope. The bulk worker owns the admitted
read-only source pin; both worker and coordinator release content pins before
accepted cleanup can unmount it. Restoration preserves the original access mode,
retains results through cancellation, refuses a replaced collection view and
reports restoration errors separately from a completed backup. Live compensation
still runs its identity checks when a checkpoint write fails; journal-open or write failure
cannot erase completed bytes or suppress an otherwise owned restoration.

Durable phases include read-only mount admission/confirmation, file collection
and file publication. Recovery discloses the last observed mount and possible
authority effects without replaying mount operations after restart. This policy
does not preserve arbitrary original mount-option overrides. Full interruption,
physical hotplug and desktop password-agent coverage remain required.

Six host tests cover false read-only inventory, declined authority, cancellation
during accepted unmount, request binding and image-review ownership, interrupted
journals without authority replay, and retained live compensation after journal
failure. Two guest tests use actual UDisks/PolicyKit mounts: busy readers refuse
admission; normal and originally-read-only collection preserve bytes and access
mode; cancellation after an accepted read-only mount restores access without
copying; cancellation during final restoration retains the completed backup.
An unrelated pair remains responsive during the held authority reply. These run
beside the two existing image-authority cases on FAT32, exFAT and NTFS3: twelve
harness cases pass in
`/tmp/portable-drive-vm-read-only-command-authority-compensation-final/report.json`. The native
fifteen-case filesystem matrix also passes on all three formats (45 cases):
`/tmp/portable-drive-vm-read-only-command-native-final/report.json`.

The initial combined guest rerun found a fixture assumption that UDisks remounts
must retain `/work/media`. The fixture now separates captured partition facts
from verification of the initial mount, then resolves the observed current
mount. A later run failed image-provider mounting because the test executable's
inferred helper path points into `target/debug/deps`; the runner now supplies
`LITTLE_CLIP_DRIVE_MOUNT_HELPER` explicitly. Neither failure was counted as
acceptance or resolved by relaxing production checks.

Backend tests pass 112 cases with 48 explicit fixtures ignored; core protocol
and compatibility tests pass five, CLI flags one, Files drive logic fourteen,
and real Btrfs two. Selected all-target clippy passes with warnings denied.
Logs use `/tmp/portable-drive-read-only-command-` with `backend-final.log`,
`core-final.log`, `cli-final.log`, `files-final.log`, `btrfs-final.log` and
`clippy-final.log`.

The full-feature architecture gate remains REGRESSION: clone groups 4300 versus
4289 and reported cycles 144 versus 143; duplication 7.10% versus 7.30%, average
CCN 3.48 versus 3.44 and maximum 274 unchanged. This checkpoint adds no clone
group relative to the previous commit. Reusing admitted-source validation reduces
`start_background` CCN from 22 to 19; `partition_plan` remains 22. New worker and
read-only lifecycle methods stay at or below 15. Selected fields are retained in
`.fitness/portable-drives-read-only-mode-architecture.json`. Item 4.3 remains
Partial for its broader fidelity/consistency and authority/recovery matrices.

Validation at this checkpoint:

- Backend: 112 passed, 48 explicitly ignored fixtures;
  `/tmp/portable-drive-read-only-command-backend-final.log`.
- Five core protocol/configuration tests, one CLI parsing test and fourteen
  drive-UI logic tests pass (`core-final.log`, `cli-final.log`, `files-final.log`
  with the same `/tmp/portable-drive-read-only-command-` prefix).
- The private authority guest passes four cases on each of FAT32/exFAT/NTFS3
  (twelve total, including six read-only collection cases):
  `/tmp/portable-drive-vm-read-only-command-authority-compensation-final/report.json`.
  Existing imaging, authorization denial and image/catalog providers are retained.
- The native filesystem matrix passes fifteen cases per filesystem (45 total):
  `/tmp/portable-drive-vm-read-only-command-native-final/report.json`.
  Both real Btrfs delta/link-group cases pass in
  `/tmp/portable-drive-read-only-command-btrfs-final.log`.
- Dark and light private-compositor runs each pass twenty rendered observations
  in `/tmp/portable-drive-read-only-command-review-{dark,light}-final/report.json`.
  Both read-only dialogs were inspected for legibility and clipping. The flow
  selects and cancels read-only review, then starts a fresh live file backup and
  cancels image review. Host authorization is never invoked by that UI fixture;
  actual read-only collection is exercised inside the authority guest.
- Affected-package all-target clippy with warnings denied, formatting, helper/app
  build and Python compilation pass; logs share the checkpoint prefix.

The first draft exposed the old-daemon field-ignoring hazard before commit and
was replaced with distinct commands. The test initially expected fake image
authority to succeed after validating review retention; its assertion now checks
the fixture's intended authorization denial. Earlier guest fixture/helper-path
setup failures are retained under the preceding `read-only-mode` log prefix;
only successful complete reports above count as native evidence.

Architecture remains REGRESSION for the full feature: 4300 clone groups versus
baseline 4289, duplication 7.10% versus 7.30%, average CCN 3.48 versus 3.44,
maximum 274 and reported cycles 144 versus 143. This checkpoint adds no clone
group or new hotspot over the preceding committed source guard. Shared source
admission reduces `start_background` CCN from 22 to 19; `partition_plan` stays
22. New collection/worker methods remain at or below 15. The bulk Worker owns
its inputs instead of extending one large closure, and reuses the existing
partition authority, cancellable delivery, source guards, manifests and backup
publication. Selected fields are retained in
`.fitness/portable-drives-read-only-mode-architecture.json`. Full-feature
regression remediation and final workspace gates remain open. Item 4.3 stays
Partial for the wider fidelity/mount/interruption acceptance matrix.

## Read-only source guard

Core mount inventory now retains separate per-mount and backing-superblock
read-only observations. Discovery incorporates either fact into drive access
status. These are additive live observations; persisted path, storage and
namespace identities are unchanged. The guarded source constructor can require
read-only collection. It validates the retained mount identity/namespace,
descriptor filesystem flags and the matching mount's read-only superblock.
Existing root validation repeats this condition throughout inventories and
payload/metadata collection. A final source check precedes publication, including
after Btrfs sealing. The manifest records the enforced consistency policy.
Ordinary roots keep their prior live-collection behavior.

A read-only bind view over a writable superblock cannot establish this stronger
precondition. The native guest creates that exact case, verifies the view's
read-only descriptor flag and refuses the stronger guard, while separately
proving ordinary file backup still succeeds through the view. It then makes
the filesystem read-only, completes a guarded backup and verifies its manifest
policy/header checksum. A second collection deliberately restores write access
after fingerprint capture; validation fails, source bytes remain intact and
only the previous completed version/current reference remains registered.

All fifteen cases pass on each native FAT32, exFAT and NTFS3 mount (45 total):
`/tmp/portable-drive-vm-read-only-source-native-final/report.json`. The first run
exposed fixture remount dispatch to the exFAT FUSE helper and failed rather than
counting it as acceptance. The shared owned guest mount fixture now requests
internal kernel remounts, preserving its captured mount identity; no host mount
or attached device is changed. It also retains existing symlink/file-bind cleanup.

Backend tests pass 106 cases with 46 explicit fixtures ignored:
`/tmp/portable-drive-read-only-source-backend-final.log`. Two core mount-parser
cases pass, including distinct view/superblock flags and existing raw names:
`/tmp/portable-drive-read-only-source-core.log`. Both real Btrfs cases remain
green: `/tmp/portable-drive-read-only-source-btrfs-final.log`. Selected all-target
clippy and affected-crate formatting pass with the
`/tmp/portable-drive-read-only-source-` prefix (`clippy-final.log`, `fmt-final.log`).

Architecture remains REGRESSION for the full feature: 4300 clone groups versus
baseline 4289, duplication 7.11% versus 7.30%, average CCN 3.48 versus 3.44,
maximum 274 and reported cycles 144 versus 143. There is no additional clone or
new source-guard hotspot. The existing backup entrypoint adds two complexity
points (32 to 34) for the consistency policy and pre-publication source check.
Selected fields are retained in `.fitness/portable-drives-read-only-source-architecture.json`.
This is backend preparation for item 4.3, which remains Partial. Selecting the
mode and owning flush/unmount/read-only mount/collection/restoration through the
daemon, review/CLI controls and their authorization/cancellation/recovery
acceptance remain required; no new rendered UI acceptance is claimed.

## Reusing completed aliases during collection

Collection now looks up source device/inode groups in the current manifest's
existing disk-backed index before writing another alias payload. It selects
only entries whose revision and metadata records have already committed in this
collection. The shared group validator checks captured revision, hash/link target,
attributes and coverage, then verifies the retained candidate's bytes. Divergent
source revisions retain bounded failure behavior; replaced/corrupt candidates
stop publication. This reuses the relationship installer and final coverage pass,
without a second cache or another persisted source representation.

The installer supports both an existing pinned destination and an absent name.
Existing destinations must retain their captured revision through staging;
absent names use atomic no-overwrite installation. Parent and staging handles
remain held through installation. Link capability failures fall back to the
ordinary full copy, with the existing final pass reporting lost relationships.
The final pass still checks payloads, relationships and partial source coverage.

Ordinary nested-alias tests now assert one payload's bytes written in each
independent version. The attribute-only retry case also includes an alias and
proves recollection retains one write and a shared final inode. A new fault case
replaces the completed staging candidate with different bytes before another
alias: backup fails, the previous manifest/current references remain valid, and
source bytes remain intact. Backend tests pass 105 cases with 45 explicit
fixtures ignored: `/tmp/portable-drive-backup-link-reuse-backend-final.log`.

Both real Btrfs cases pass with new assertions that linked first/edited versions
write one payload, while old versions and later source splits stay independent:
`/tmp/portable-drive-backup-link-reuse-btrfs-final.log`. The native module passes
fourteen cases on each FAT32/exFAT/NTFS3 mount (42 total):
`/tmp/portable-drive-vm-backup-link-reuse-native-final/report.json`. Its destination
case asserts two full payload writes on FAT32/exFAT, versus one on NTFS; the NTFS
source case also asserts one payload write and retains its attribute-drift refusal.

Selected backend/Files/daemon all-target clippy and affected-crate formatting pass:
`/tmp/portable-drive-backup-link-reuse-clippy-final.log` and
`/tmp/portable-drive-backup-link-reuse-fmt-final.log`. There is no new rendered UI
acceptance. Shared installation policy and fixture setup remove new accidental
hotspots/duplication. Architecture remains REGRESSION for the full feature, with
4300 clone groups versus baseline 4289, duplication 7.11% versus 7.30%, average
CCN 3.48 versus 3.44, maximum 274 and reported cycles 144 versus 143. This
checkpoint adds no clone group or hotspot; selected fields are retained in
`.fitness/portable-drives-backup-link-reuse-architecture.json`. Item 4.3 remains
Partial, including selectable quiescence and broader acceptance matrices.

## Hard-link relationships in file backups

Matching captured regular-file and symlink aliases now share a backup inode on
supporting filesystems. A disk-backed `backup_links` table indexes exact source
device/inode strings and raw relative paths, without a whole-tree in-memory map
or SQLite integer coercion of large unsigned identities. Group members must agree
on captured source revision, content/link target, exposed attributes, original
creation time, attribute coverage and source link count. A revision mismatch
stops publication and retains current. Access time can change from our own
inventory reads; each group uses the first inventory insertion's observation.

The installation path links the retained descriptor into an owned staging
directory, validates the existing destination, atomically installs the alias,
and verifies the installed inode and bytes. This works for held symlink inodes
without following their user referents. Cancellation and guard/install failures
stop publication. A link syscall failure retains the complete independent copy
and records its errno; all affected group paths disclose incomplete relationships.
Missing aliases are disclosed with captured versus original link counts and
never expand the selected source scope. Completed per-path facts include an
optional `hard_link_to` using core's lossless text/raw-byte path codec. Final
fidelity summaries are derived after consolidation and directory restoration.

Host regressions verify nested regular-file aliases, independent completed
versions, raw-named linked symlinks with untouched outside referents, uncaptured
aliases and changed revisions preserving the previous current version. Backend
tests pass 104 cases with 45 explicit fixtures ignored:
`/tmp/portable-drive-backup-links-backend-final.log`.

The native guest passes fourteen tests on each FAT32, exFAT and NTFS3 mount
(42 total): `/tmp/portable-drive-vm-backup-links-native-verified/report.json`.
The destination-fidelity case now verifies both aliases: FAT32/exFAT keep
independent complete files and per-path link omissions with an actual errno;
NTFS retains the shared inode. A separate native source case preserves an NTFS
group, changes only an attribute between alias collections with preserved mtime,
and verifies failure leaves the old manifest, current reference, bytes and
attributes intact. FAT32/exFAT explicitly refuse source hard-link creation;
those branches do not claim source alias support.

The retained-descriptor guest case additionally creates a new alias after
the original name is replaced, proving it links the held regular/symlink inode
and leaves replacement/referent canaries unchanged. Both metadata guest cases
pass: `/tmp/portable-drive-vm-backup-links-descriptor-final/report.json`.
Both real Btrfs cases pass in the authorized backup-testing directory, including
new version groups, independent old bytes and later source unlink/recreation:
`/tmp/portable-drive-backup-links-btrfs-final.log`.

Selected backend/Files/daemon all-target clippy and affected-crate formatting
pass (`/tmp/portable-drive-backup-links-clippy-final.log` and
`/tmp/portable-drive-backup-links-fmt-final.log`). No rendered UI acceptance was
added. Group consolidation currently stages complete independent copies first;
temporary space/write amplification is disclosed and eliminating it remains
open, alongside selectable quiescence and broader acceptance matrices.

The collection owner now coordinates its payload, obsolete-path, relationship,
directory, fidelity and flush phases. Successful entry/retry facts retain one
transactional writer. Common named-object validation and guest backup setup
remove new accidental duplication. Backup entrypoint CCN decreases from 41 to
32 and population from 18 to 13. No new hard-link hotspot or clone group remains;
4300 clones versus baseline 4289, duplication 7.11% versus 7.30%, average CCN
3.48 versus 3.44, maximum 274 and reported cycles 144 versus 143. Selected fields
are in `.fitness/portable-drives-backup-links-architecture.json`. The feature-wide
REGRESSION gate remains open; item 4.3 stays Partial.

## Backup metadata and fidelity reports

File collection now retains source metadata objects while capturing exposed
ownership, modes, access/modification dates and raw extended attributes, including
POSIX ACLs. It checks both installed contents and actual destination metadata.
Directory/root fields are restored after children, timestamps after verification
reads, with a final filesystem flush before publication. Attribute-only changes
on coarse/uncertain sources trigger the existing bounded collection retry.
Bytes installed by an earlier attempt remain included in write accounting.
Inherited/stale destination attributes are removed only after complete source
name coverage. Existing destination hard links are broken before changing fields,
so one restored path cannot silently change another path's metadata.

Each manifest has a raw-path-keyed `backup_metadata` table containing JSON source
facts and typed per-field omissions. The `DriveBackup.fidelity` summary is optional
for compatibility with older versions and images. Counts distinguish restored
paths with omissions, unique omitted fields/attribute names and incomplete source
attribute observations. The manifest header and registry summary must agree.
Attributes use bounded buffers and a per-entry budget (1024 attributes, up to
1 MiB); unavailable/uncaptured facts remain explicit. Original ctime, creation,
inode and link facts are recorded observations, not promises to reproduce those
filesystem identities. Initial access observations are retained while the inode
is unchanged. The manifest also records whether the source mount was read-only
when metadata collection began; this does not implement selectable quiescence.

Retained symlink descriptors use controlled descriptor-backed metadata views,
without following the user's link target. Guest tests replace both regular and
symlink names while descriptors remain held, then verify raw trusted attributes,
ownership and timestamps affect only the held original. Replacement and referent
canaries remain intact. A second guest test exceeds the per-entry attribute budget
and verifies complete payload plus disclosed partial metadata. Both pass in
`/tmp/portable-drive-vm-backup-metadata-descriptor-final/report.json`.

The native module now passes thirteen cases on each real FAT32, exFAT and NTFS3
guest mount (39 total):
`/tmp/portable-drive-vm-backup-metadata-native-verified/report.json`. New cases
check destination ownership/mode/date coercion and missing attributes, attribute-only
source recollection where writable attributes exist, and rejection of a symlink
inode bound from outside the selected hierarchy. FAT32/exFAT explicitly report
unavailable writable user attributes; those branches do not claim a round trip.
The first symlink-bind run correctly refused backup but failed fixture cleanup
because unmount followed the dangling referent. The fixture now unmounts only
its captured identity with `UMOUNT_NOFOLLOW`, and verifies the original portable
file reappears unchanged. Evidence is confined to the disposable guest.

The expanded real Btrfs case verifies an attribute-only third file version writes
zero payload bytes, removes the obsolete attribute and retains earlier versions'
values. Shared file/image extents and the 4 KiB image delta still pass:
`/tmp/portable-drive-backup-metadata-btrfs-final.log`. Backend tests pass 101 cases
with 43 explicit fixtures ignored. Core drive model compatibility tests pass four
cases and Files drive-controller tests pass fourteen; these are not rendered UI
acceptance runs. Logs: `/tmp/portable-drive-backup-metadata-backend-final.log`,
`/tmp/portable-drive-backup-metadata-core.log` and
`/tmp/portable-drive-backup-metadata-files-ui.log`. Selected all-target clippy and
affected-crate formatting pass with the `backup-metadata-*-final.log` prefix.

Shared manifest verification and refusal assertions remove new test duplication.
The checkpoint introduces no metadata hotspot or additional clone group over the
preceding committed checkpoint: 4300 clones versus baseline 4289, duplication
7.12% versus 7.30%, average CCN 3.48 versus 3.44, maximum 274 unchanged and reported
cycles 144 versus 143. Selected fields are in
`.fitness/portable-drives-backup-metadata-architecture.json`. The feature-wide
REGRESSION gate remains open. Item 4.3 stays Partial: hard-link preservation and
selectable quiesced read-only collection are still required.

## Final-file mount boundaries

Reviewing live collection exposed a gap in the shared `Root::input` contract.
Parent-directory traversal rejected nested mounts, but the final regular file only
received core's ordinary-content classification. A file bind-mounted from outside
the admitted root could therefore supply payload bytes for hashing/copying. The
negative guest run confirms this on all three native source formats, returning
the owned foreign file's successful SHA-256 instead of refusing access:
`/tmp/portable-drive-vm-final-file-mount-negative/report.json` and its guest log.

`Root::input` now compares the opened descriptor's device and mount ID with its
retained root before returning it to any content consumer. The check uses the
existing pinned mount identity, without another pathname lookup or routing policy.
The guest test independently binds a file from another filesystem and from the
same source filesystem outside the root; the latter proves a device-only check
would be insufficient. Both hashing and file-backup collection must refuse, no
backup may publish, and unmounting the fixture must reveal the intact original
portable file while the foreign canary remains unchanged. A small RAII fixture
unmounts only its captured mount identity; all bind operations require the existing
disposable-guest boot identity/marker and touch owned fixture paths only.

Final native evidence is
`/tmp/portable-drive-vm-final-file-mount-accepted/report.json`: all ten cases pass
on each of FAT32, exFAT and NTFS3 (30 total), including both bind variants. Existing
RAM crash/namespace/pressure cases also pass through this shared input check:
`/tmp/portable-drive-vm-final-file-mount-ram/report.json` (five guest tests). The real
Btrfs version/delta fixture still passes with old bytes and shared extents retained
(`/tmp/portable-drive-final-file-mount-btrfs.log`). Backend tests pass 95 cases with
38 explicit fixtures ignored; selected backend/Files/daemon all-target clippy and
affected-crate formatting pass. Logs use the
`/tmp/portable-drive-final-file-mount-` prefix (`backend.log`, `clippy.log`).

Architecture remains REGRESSION with no new finding from this fix and unchanged
4300 clone groups, 7.13% duplication, average CCN 3.48, maximum 274 and reported
cycles 144 versus 143. Selected fields:
`.fitness/portable-drives-final-file-mount-architecture.json`. This closes the
specific final-file payload escape; item 1.6 and the feature-wide gate remain Partial.

## Bounded live-file backup collection

File backups now enumerate metadata without hashing the entire tree in advance,
then collect each file with a fresh fingerprint immediately before its guarded
copy. The existing keyset cursor reads bounded manifest pages while successful
entries replace their original inventory rows with the revision actually copied.
Copying still uses the shared checked streaming/staging/no-overwrite mechanisms;
retry does not bypass root or namespace validation. A changed entry gets at most
three attempts, while cancellation, source type changes and root replacement
refuse collection. A failed copy with unchanged source is not repeatedly replayed.
On coarse/uncertain filesystems, an additional hash determines whether unchanged
metadata conceals changed contents before retrying a failed attempt.

The checksummed SQLite manifest records collection start/completion observations,
the per-file attempt limit, total retried entries and individual retry paths using
the existing lossless Unix path codec. Only successful revisions are published.
Continuous changes leave the previous immutable version, database current record,
filesystem current reference and old manifest intact. Backup review and completion
messages explicitly disclose per-file verified live collection rather than an
atomic source snapshot. A later source edit is not a promise to retroactively change
an already collected file or include paths added after directory enumeration.

Four new backend cases prove a successful retry with a raw non-UTF8 filename and
preserved mtime, three-attempt exhaustion, cancellation between fingerprint capture
and copy, and refusal of a file replaced by a symlink. They validate the final
manifest fingerprint against the installed bytes and source revision, and all
failed cases check both current references plus the old manifest checksum. A shared
fault-injection fixture binds edits to the existing progress callback after capture;
there is no process-global test hook or production bypass.

The disposable native guest adds the same preserved-mtime/same-size successful
retry and continuous-change bound on actual FAT32, exFAT and NTFS3 sources. The
complete native module passes nine cases on each filesystem (27 total):
`/tmp/portable-drive-vm-backup-retry-matrix/report.json` and its guest log. The real
Btrfs fixture in the explicitly authorized backup-testing directory also passes;
4 KiB data edits still allocate 256 KiB while old versions remain unchanged.
Evidence: `/tmp/portable-drive-backup-retry-btrfs-final.log`. Backend tests pass 95 cases
with 37 explicit fixtures ignored; affected-crate formatting and selected
backend/Files/daemon all-target clippy pass. The fourteen Files drive-controller tests
also pass (`/tmp/portable-drive-backup-retry-files-ui-tests.log`); this is not a new
rendered UI acceptance run. Final logs:
`/tmp/portable-drive-backup-retry-backend-final.log` and
`/tmp/portable-drive-backup-retry-clippy-final.log`.

The first architecture pass found an additional metadata-setting clone and repeated
test setup. Shared file/directory metadata application and the common fault-injection
fixture remove that accidental duplication. The backup entrypoint's CCN decreases
from 58 to 40; collection methods introduce no new hotspot. The final comparison
reports 4300 clone groups versus baseline 4289 (one fewer than the preceding
checkpoint), 7.13% duplication, average CCN 3.48, maximum 274 and reported cycles
144 versus 143. Selected fields are retained in
`.fitness/portable-drives-backup-retry-architecture.json`. The feature-wide gate
remains REGRESSION and stays open. Item 4.3 is Partial: available metadata fidelity,
explicit omission records and a selectable quiesced read-only source still need
implementation and acceptance.

## Namespace and metadata changes across abrupt daemon death

The shared guest crash lifecycle now accepts an edit and a verification callback,
so namespace-only cases do not duplicate child ownership, image-lease retirement or
recovery setup. Before publication the child seeds an existing directory and an
xattr, then fsyncs its initial state. Each independent parent makes exactly its
selected edit, kills the owned daemon and verifies the same retained image after
recovery. Open create/truncate/mtime descriptors remain held until the helper dies.
No later data write, fsync or daemon request can accidentally publish the tested edit.

The preceding helper still lost an acknowledged empty file. Negative evidence:
`/tmp/portable-drive-vm-ram-namespace-negative/report.json`. Successful mutation
callbacks now share one flush completion function, called under the existing lock.
Rename reuses this path instead of retaining its duplicate flush. The callbacks
cover creation, directory/link removal, links, permissions/ownership, truncation,
timestamps, xattrs, supported inode ioctls and allocation. Truncating open is handled
in its existing open helper. Creation flushes before allocating the returned open
handle, avoiding a leaked FUSE handle if publication fails. Failed operations keep
their original errors. Data writes retain their separate stored-prefix handling.

Inspection also found that `op_truncate` returned its successful name-lookup result
after a permission check failed. It now returns the actual operation result. An
owned immutable file supplies direct negative evidence: the previous helper falsely
reports successful truncation in
`/tmp/portable-drive-vm-ram-truncate-negative/report.json`. The patched case returns
EPERM, then retains the immutable flag and original bytes across daemon death.

Final evidence: `/tmp/portable-drive-vm-ram-namespace-accepted/report.json` and its
guest log. All five guest tests pass: data-write crash cases, the 13 namespace-only
cases, inode flags/refused truncation, available-memory pressure and underlying-tmpfs
exhaustion. The namespace matrix covers empty creation, mkdir, unlink, rmdir,
truncate, chmod, mtime, hard links, symlinks, rename, chown and xattr addition/removal.
Deletion verification stops before synchronization, preserving the product's default
union policy rather than mistaking intentional restoration for failed crash recovery.
Backing-store exhaustion may now fail at file creation itself; either creation or
subsequent growth must report ENOSPC/EIO and retain existing acknowledged edits.

Backend tests pass 91 cases with 36 explicit fixture entrypoints ignored. Selected
backend/Files/daemon all-target clippy with warnings denied, native quotas, paused
full-media eject, both controlled removal/cancellation cases, four helper delivery
tests and affected-crate formatting pass. Logs use the
`/tmp/portable-drive-ram-namespace-` prefix (`backend.log`, `clippy.log`,
`native-quota.log`, `native-regressions.log`, `builder-tests.log`). Both normal and
test-gated companions rebuild from the updated source patch; test controls remain
absent from the normal build.

The architecture comparison remains REGRESSION with no new crash-fixture finding
or additional clone group: 4301 versus 4289 groups, 7.13% duplication, average CCN
3.48, maximum 274 and reported cycles 144 versus 143. Selected measured fields are
retained in `.fitness/portable-drives-ram-namespace-architecture.json`. The shared
crash lifecycle and mutation completion avoid duplicated recovery/flush mechanisms.
The full architecture gate and item 3.7 remain open; interruption during an
unacknowledged request and concurrent pressure/admission still need evidence.

## Acknowledged writes across abrupt daemon death

The new guest test starts an owned test process containing the daemon service,
populates and pauses a RAM session, and publishes its session record to the parent.
The parent is the external writer: it holds the file open, receives successful
writes, then kills and reaps its daemon child without closing or fsyncing that
file. It waits for the validated backing image's exclusive lease to retire before
restarting against the same state. No guessed helper PID is signalled. The child
has parent-death handling, a bounded lifetime and an RAII kill/reap guard in its
parent. The internal probe is explicitly skipped in module-wide guest selection.

The negative run, `/tmp/portable-drive-vm-ram-crash-negative/report.json`, genuinely
loses the new file with the previous helper after successful recovery. The first
diagnostic run instead rejected recycled FUSE mount identifiers; the corrected test
checks an explicit successful recovery result and the retained image device/inode,
rather than assuming Linux cannot immediately recycle mount/minor numbers.

The companion patch now flushes filesystem metadata and cached blocks into the
tmpfs image before acknowledging stored write bytes. Flush errors clear the
successful-byte result and are translated back to the caller. This includes a
partial stored prefix on other error paths. The patched crash test covers a new
file, an overwritten existing file and an 8 MiB file written in 4 KiB chunks.
Each case checks exact recovered bytes, the same image identity/path, full-check
pending state and successful portable writeback before confirmed cleanup. The
8 MiB case took about 63 ms in the final bounded guest; this is one observed run,
not a general throughput guarantee. Data remains RAM-backed and cannot survive a
host reboot or power loss.

Final guest evidence is `/tmp/portable-drive-vm-ram-crash-accepted/report.json` and
its log: all three guest tests (crash matrix and both pressure tests) pass. The
backing-full case still returns ENOSPC with zero underlying free bytes and logical
headroom. The existing controlled partial-removal/cancellation and full-media/paused
eject fixtures both pass with rebuilt normal and gated helpers. Backend tests pass
91 cases with 34 explicit fixture entrypoints ignored; selected backend/Files/daemon
all-target clippy with warnings denied and four delivery tests pass. Logs:
`/tmp/portable-drive-ram-crash-backend.log`,
`/tmp/portable-drive-ram-crash-native-regressions.log`,
`/tmp/portable-drive-ram-crash-clippy.log`, and
`/tmp/portable-drive-ram-crash-builder-tests.log`.

The architecture comparison remains REGRESSION: 4301 versus 4289 clone groups,
7.14% duplication, average CCN 3.48, maximum 274 and reported cycles 144 versus 143.
The new crash fixture adds no reported hotspot or additional clone group; the
feature-wide gate remains open. Formatting is checked for the affected crate.
An initial workspace format invocation also formatted 141 previously unformatted
vendor files; each was restored only after its contents exactly matched rustfmt
applied to HEAD, with no vendor changes retained.

Item 3.7 remains Partial. Acknowledged data writes now have direct crash evidence;
namespace-only mutations (including empty creation, mkdir and unlink), death during
an unacknowledged request and concurrent pressure/admission still require work.

## Backing-tmpfs exhaustion and memory-pressure checkpoint

`vm_ram.sh` mounts two separate 512 MiB tmpfs stores inside the existing disposable
guest. The runner still exports code/tools read-only, exposes no host block devices
or network and retains failed command status. Shell and Rust both require the
guest marker and a distinct boot identity; Rust verifies the RAM-store filesystem
and capacity before allocating a fixture. The existing native filesystem tests
reuse that same boot-identity check. No host pressure or mount operation is used.

The memory case creates a dirty RAM copy, sets a deliberately tight configured
minimum around measured `MemAvailable` and allocates a 256 MiB pressure file.
Another RAM admission is refused while the original view remains readable and
writable. Releasing the file and explicitly restoring the fixture's normal minimum
allows another copy; discarding that copy preserves the original. This is not an
assertion that kernel `MemAvailable` immediately returns to its cold-start estimate.
Production defaults are unchanged.

The backing-store case observes a sparse 128 MiB image with 106496 allocated bytes,
fills the backing tmpfs to zero free bytes and verifies over 64 MiB logical image
headroom. The previous helper incorrectly acknowledged a 64 MiB write anyway.
The e2fsprogs 1.47.4 `unix_write_blk64` cache-eviction path stored `reuse_cache`'s
error in a local variable but returned its unchanged successful outer result.
The companion patch now propagates that error without clearing an earlier
write-through failure. Its source hash changes rebuild normal and gated helpers;
the complete corresponding patch remains in each generated source bundle.

The old-helper negative run fails on that falsely successful write:
`/tmp/portable-drive-vm-ram-pressure-io-negative/report.json`. The patched run passes
both pressure cases:
`/tmp/portable-drive-vm-ram-pressure-patched/report.json`. The final guest run is
recorded separately below. An external write to the exhausted view now reports
ENOSPC/EIO. Returning backing space and restarting the service retain the image's
device/inode and RAM path, recover the acknowledged edit and full-check marker,
and permit another edit plus successful media writeback. Failed external growth
is not advertised as a complete saved file or silently redirected to disk.

Final evidence is `/tmp/portable-drive-vm-ram-pressure-accepted/report.json` and its
guest log: both cases pass, with zero backing bytes free, 131747840 logical bytes
free and a reported `No space left on device (os error 28)`. Backend tests pass
91 cases with 32 explicit fixtures ignored. Six existing native quota, full-media,
controlled-removal, population, real FUSE unmount and restart fixtures pass with the
rebuilt normal/gated companions. Backend/Files/daemon all-target clippy with warnings
denied, four companion delivery tests, Python/shell syntax, repository formatting
and whitespace checks pass. Logs:
`/tmp/portable-drive-ram-pressure-backend-final.log`,
`/tmp/portable-drive-ram-pressure-native-regressions.log`, and
`/tmp/portable-drive-ram-pressure-clippy.log`.

The architecture comparison remains REGRESSION with twelve net new clone groups,
7.14% duplication, average CCN 3.48, maximum 274 and reported cycles 144 versus
baseline 143. Reusing the configured-service factory removes the initial new
constructor clone; the pressure fixtures add no reported hotspot. The feature-wide
gate stays open. Report:
`.fitness/portable-drives-ram-pressure-accepted-architecture.json`.

The shared fixture's configured-service constructor avoids another daemon setup
implementation. Existing resource/quota, full-media/eject and controlled-removal
fixtures remain relevant checks. Item 3.7 remains Partial: the directory-fsync kill
test is not evidence for an unflushed write held open by an external client. That
remaining case must kill an owned daemon child while the separate test parent
holds the acknowledged file open, wait for its helper's image lease to retire,
then verify recovery against that same image. Broader pressure/cancellation races
also remain open.

## Controlled mid-write removal checkpoint

The separate `--test-io-gate` helper builds from the same verified e2fsprogs
archive and normal patch, with one additional test-only patch. A private inherited
Unix socket arms a write and reports its stored position after at least 256 KiB.
The filesystem lock is released before the bounded 20-second control wait, so
metadata remains responsive while this data request is held. The control protocol
has its own mutex. The normal companion does not contain the gate; the gated Rust
constructor exists only under `cfg(test)`.

`removing_media_during_a_partial_write_retains_ram_and_queued_operation_results`
passes both normal failure and cancellation. Each case edits an 8 MiB RAM file,
verifies that the helper has stored a nonzero incomplete staged prefix, and queues
eject/unconfirmed discard while the write remains active. Cancellation cannot
release that owner early. An unrelated drive's rule operation completes while
those requests wait. The fixture lazily detaches only its owned FUSE mount, removes
its inventory entry, and returns a real EIO to the in-flight write.

The failed/cancelled copy retains its terminal result, original media file, complete
RAM edit, observed pending work and deliberately old drive-confirmation timestamp.
Queued eject reports failure after removal; unconfirmed discard preserves the RAM
view. More offline edits remain possible. Recovering the source filesystem finds
the original file and no unpublished staging residue. A portable edit made before
re-enrollment conflicts with the retained RAM edit: both complete variants appear
on both sides, and the additional offline file is copied back. Resuming automatic
catch-up clears observations generated by conflict installation without erasing
the earlier failed/cancelled operation result.

Build modes have separate fingerprints/source trees/output defaults. The builder
refuses test output at production packaging paths, including symlinked targets;
legacy normal caches remain accepted, and either mode change invalidates the
opposite cache. Source extraction/patching is staged and published only with the
matching completed-patch marker, so a failed patch cannot turn an incomplete source
directory into an accepted build. Four delivery tests cover both mode transitions,
path guards and interrupted-patch retry. Actual repackaging verifies source/license/
rebuild contents and compiled gate selection for both modes. The production executable
hash and mtime remain unchanged. Native startup now separates process launch from
mount-readiness/quota checks while retaining its existing limits and child owner.

Backend tests pass 91 cases with 30 explicit fixtures ignored. Five selected native
tests pass: gated removal (both cases), full-media/paused eject, incomplete initial
population/eject refusal, real owned FUSE unmount/cancellation, and offline daemon
restart/reconnect. Backend/Files/daemon all-target clippy with warnings denied,
Python compilation, delivery tests, repository formatting and whitespace checks
pass. Evidence:
`/tmp/portable-drive-ram-removal-backend-final.log`,
`/tmp/portable-drive-ram-removal-native-accepted.log`,
`/tmp/portable-drive-test-gate-unit-final.log`,
`/tmp/portable-drive-test-gate-packaging-final.log`, and
`/tmp/portable-drive-ram-removal-clippy.log`.

The architecture gate remains REGRESSION with unchanged material totals: twelve
net new clone groups, 7.14% duplication, average CCN 3.48, maximum 274 and reported
cycles 144 versus baseline 143. Launch/readiness and source-preparation/publication
boundaries remove the initial new Lizard CCN hotspots. The builder entrypoint still
has a structural finding from its necessary cache-mode/path validation and verified
archive admission. No new clone group is introduced. Report:
`.fitness/portable-drives-ram-removal-final-architecture.json`.

This is actual kernel FUSE mount removal plus controlled I/O failure, not a claim
of physical USB unplug coverage. Items 3.5/3.6 remain Partial for their remaining
filesystem/device and cancellation matrices. Underlying-tmpfs pressure and
unflushed crash recovery remain open under 3.7.

## RAM pre-unmount and full-media checkpoint

Pre-unmount planning now includes paused RAM mirrors while preserving ordinary
folder pairs' pause policy. Incomplete RAM population is refused rather than
silently omitted or marked ready by a partial preflush; resuming synchronization
or explicitly discarding the incomplete view are the two recovery choices. Export
can preserve its current contents before discard, but does not itself unblock
pre-unmount admission. Failed reconciliation explicitly reports retained RAM
contents and leaves pending acknowledgements/drive confirmation unchanged.

The new native `full_media_retains_ram_edits_and_paused_eject_waits_for_preflush`
fixture fills only an owned 128 MiB ext4/FUSE filesystem. A separate RAM view has
an 8 MiB edit; the source has insufficient allocation for its staged replacement.
Real ENOSPC leaves the original portable file and complete RAM edit intact, with
no unpublished temporary file left behind. A deliberately old confirmation date
makes the failed-write assertion independent of same-second timestamp coincidence.
An explicit eject while paused hits the same preflush failure and never calls the
unmount authority. Releasing the owned space reservation permits flush, clears
pending work and advances confirmation. Another edit while still paused is
installed before the test's held unmount authority runs.

While that authority is held, an unconfirmed discard remains pending and an
unrelated drive's policy request completes. After release, eject completes and
the discard is refused. The RAM view remains browsable/editable offline; another
unconfirmed discard still retains its bytes. Explicit confirmed discard removes
the view, enrollment and pending ledger. The existing real normal-FUSE-unmount
fixture continues proving busy-reader refusal and pin retirement separately; the
new full-media fixture uses an owned inventory authority and does not claim a real
block-device eject or FAT/exFAT/NTFS full-media coverage.

Four explicit native tests pass: the new full-media case, cancelled initial
population/eject refusal/resume, actual owned FUSE unmount/cancellation retention,
and offline daemon restart/reconnect. Backend tests pass 91 cases with 29 explicit
fixtures ignored; selected backend/Files/daemon all-target clippy with warnings
denied and repository formatting pass. Logs:
`/tmp/portable-drive-ram-eject-backend-final.log`,
`/tmp/portable-drive-ram-eject-native-final.log`,
`/tmp/portable-drive-ram-eject-media-accepted.log` and
`/tmp/portable-drive-ram-eject-clippy-final.log`.

The shared native-source factory replaces duplicate FUSE setup. Shared confirmed
discard assertions also verify enrollment/ledger cleanup. RAM session/result
persistence now belongs to the existing RAM pending owner and remains in the
generic pair-result transaction. Separating pre-unmount pair policy lowers
`partition_plan` CCN from 26 before this checkpoint to 22; generic `persist_result`
no longer exceeds the CCN hotspot threshold. The broader architecture gate remains
open: twelve net new clone groups, 7.14% duplication, average CCN 3.48, maximum 274
and reported cycles 144 versus baseline 143. The extra clone initially introduced
by repeated test cleanup was removed. Report:
`.fitness/portable-drives-ram-eject-accepted-architecture.json`.

Items 3.5 and 3.6 remain Partial. These tests do not establish mid-write physical
removal, unflushed process death, underlying-tmpfs pressure, or all accepted-write
and eject/discard interleavings.

## RAM pending-status checkpoint

Commit `200f1174` adds a bounded SQLite pending-path ledger, retained sync
checkpoints and a filesystem flush before drive confirmation. Distinct raw path
names, including removals and renames, retain at most 4096 entries/1 MiB per
session. Unknown coverage has its own full-check marker. Updating a queued name
assigns a new monotonic sequence; full and partial acknowledgements cannot consume
newer edits, and replay after an empty ledger cannot clear newly inserted work.
Restart preserves observed names and requires a complete comparison. Recovered
offline RAM mounts receive watches even while their source is absent. Persistence
errors make counts explicitly uncertain until a successful full-check admission.

Files shows observed counts and the last confirmed drive reconciliation, with
drive identity and RAM path in its menu/Jobs description. Saving elsewhere retains
its separate export history. Routine scans still schedule complete reconciliation
without repeatedly declaring coverage lost. Unavailable watches, registration/event
loss and inventory changes retain conservative full-check behavior. The UI run
exposed the earlier routine-scan warning problem; the native fixture now samples
multiple periodic scans after reconnect and verifies that clean coverage stays clear.

The real tmpfs/FUSE lifecycle test passes with offline edits, durable export,
offline shutdown/restart, another observed RAM edit after recovery, reconnect and
graceful shutdown with an open file. Export leaves both the media baseline and
drive-confirmation timestamp unchanged. Reconciliation clears only its covered
observations and confirms the drive after the filesystem flush. Raw-name, byte
budget, count budget, partial/full acknowledgement and stale-checkpoint replay
fixtures pass. Missing legacy fields imply neither a clean queue nor a confirmed
drive flush.

Private dark/light `--ram-pending-only` runs pass, with PNGs inspected. They
exercise actual Copy to RAM controls, online writeback, offline counts/disabled
flush, durable export retaining pending work, and clean reconnect. Reports:
`/tmp/portable-drive-ram-pending-accepted-dark/report.json` and
`/tmp/portable-drive-ram-pending-accepted-light/report.json`. Test OCR uses the
existing `/tmp/little-clip-tessdata` through `TESSDATA_PREFIX`; the first run without
it failed before exercising RAM controls. The harness closes context menus with an
outside click, matching egui's context-menu behavior.

Validation: backend 91 passed/28 explicit fixtures ignored, plus the selected native
lifecycle fixture; Files 982 passed/six ignored; core 452 passed/three ignored;
CLI eight, daemon 35, selected package integrations and twelve core doctests pass.
Final targeted Files status/control tests, selected-package all-target clippy with
warnings denied, application/helper builds, rendered core API documentation,
Python compilation, repository formatting and whitespace checks pass. Logs:
`/tmp/portable-drive-ram-pending-{packages,backend-scheduling,native-scheduling-final,ui-logic-accepted,clippy-final,docs,format-final}.log`.

The feature-wide architecture gate remains REGRESSION against the original
baseline: duplication 7.30% to 7.15%, average CCN 3.44 to 3.48, maximum 274,
twelve net new clone groups and reported cycles 143 to 144. The RAM checkpoint
adds no net clone group or reported hotspot after separating one-path budget
admission from batch persistence and keeping tracking-error handling in its owner.
The reported extra self-cycle remains the previously documented admission-fixture
name collision. Broader architecture findings remain open. Raw report:
`.fitness/portable-drives-ram-pending-accepted-architecture.json`.

This completes item 3.4; it does not establish interrupted media-write safety,
dirty-discard/eject waiting or memory-pressure behavior under items 3.5–3.7.

## Catalog exclusion boundary checkpoint

The real FUSE test now verifies the fixed-size coverage attribute, an excluded
folder versus a captured empty folder, normal and pinned navigation errors,
unchanged file revisions, and zero file payload opens during directory inventory
(including a TypeScript filename). Ordinary filesystems with an identically named
attribute do not become catalog views, and existing inventory serialization and
revision semantics remain compatible. Core owns the one shared bounded lossless
mount inventory parser; the backend reuses it.

Dark and light private desktop runs passed `--catalog-boundary-only`. Their PNGs
were inspected: the full label fits the Size column, empty folders remain distinct,
and the navigation error wraps even with a long mount path. Reports are under
`/tmp/portable-drive-catalog-boundary-final-{dark,light}/`. The metadata observation
is display-only and does not authorize content access or indexing.

Validation: backend 69 passed/25 explicit fixtures ignored; core 450 passed/three
ignored; Files 977 passed/six ignored; all selected-package integration tests and
twelve core doctests passed. The current native FUSE fixture and five Files UI
logic tests passed separately. Selected-package all-target clippy, builds, API-doc
generation/inspection and package formatting passed. Workspace-wide formatting
reports 141 unchanged vendored files, with no failures outside `vendor/`; those
files were not reformatted as part of this feature.

The original architecture gate remains REGRESSION: duplication 7.30% to 7.17%,
average CCN 3.44 to 3.48, maximum 274 unchanged, twelve net new clone groups,
and reported cycles 143 to 144. The extra reported self-cycle is the admission
fixture's `cleanup` calling `self.temp.cleanup()`, not recursive invocation of its
own method. Reusing `pin_content_directory` removed the new detector hotspot;
the broader clone/complexity review stays open. Raw report:
`.fitness/portable-drives-catalog-boundary-reviewed-architecture.json`.

At this boundary checkpoint, saved-record redaction was still unfinished. The
following redaction checkpoint implements that work.

## Saved catalog redaction checkpoint

Excluding a subtree now transactionally queues saved metadata cleanup. Mounted
catalog helpers retain shared file leases; cleanup takes an exclusive lease and
also checks legacy catalog mounts before changing a database. Busy readers retain
their original snapshot. The existing bounded bulk worker applies cleanup after
unmount, preserves original capture/source times and coverage, records a separate
redaction date/revision, compacts the database and retires the durable intent.
Restart replays incomplete work idempotently, even with the source absent. Older
rules are migrated once and repeated pending requests coalesce. Historical catalog
enumeration is paginated without a first-thousand limit. Source files, backup
payloads/manifests and enrolled sync baselines are retained.

Files discloses capture/redaction dates, pending reader release and retry failures.
Its catalog context-menu identity survives transient status-row changes; a real
egui regression test fails without the persistent menu key and passes with it.
Private dark/light `--catalog-redaction-only` runs both passed, including a held
FUSE file, busy-unmount refusal, cleanup after release and remounted Not catalogued
coverage. Reports: `/tmp/portable-drive-catalog-redaction-accepted-dark/report.json`
and `/tmp/portable-drive-catalog-redaction-accepted-light/report.json`.

Final serial affected-package tests passed: backend 78 (26 ignored), Files 979
(six ignored), core 451 (three ignored), CLI six, daemon 35, selected integrations
and twelve core doctests. Log:
`/tmp/portable-drive-catalog-redaction-tests-serial.log`. One earlier concurrent
system-observation test saw source identity invalidation; isolated and full serial
runs subsequently passed. Selected-package build and all-target clippy with
warnings denied, core/backend documentation, Python compilation and repository
`cargo fmt --check` passed. The earlier `cargo fmt --all --check` result included
141 unchanged vendored dependency files; bare repository formatting passes.
Explicit native FUSE/legacy-helper tests pass. Compaction of 4096 actual file
records reduced the catalog from 1,810,432 to 61,440 bytes and retained all source
files (`/tmp/portable-drive-catalog-redaction-compaction-final.log`).

The original-baseline architecture result remains REGRESSION: duplication 7.30%
to 7.16%, average CCN 3.44 to 3.48, maximum 274 unchanged, twelve net additional
clone groups and reported cycles 143 to 144. No new UI test clone was reported;
the extra cycle is the previously documented cleanup-name false match. Reused
transactional entity visiting removed duplicate pagination. The broader gate
remains open. Raw report:
`.fitness/portable-drives-catalog-redaction-ui-architecture.json`.

## Catalog provider validation checkpoint

Catalog provider validation checkpoint: provider revision and SHA-256 validation
now operate on the retained content descriptor, using core FileSource checks
before returning it. File-tree backups require their saved StorageIdentity;
missing legacy identities and replaced directories cannot authorize new contents.
Tests cover a replaced name whose new file has the expected digest while the
retained file has different bytes, matching hashes with changed timestamps,
same-size/mtime tampering, and fallback from a stale newer backup.

Backend tests pass 82 cases (27 explicit fixtures ignored), and the separately
selected native FUSE provider-change test passes against the newly built actual
helper. It reads part of a file, modifies that provider while retaining mtime,
publishes a newer backup, verifies a new open's correct bytes and verifies ESTALE
on the original open handle. Logs:
`/tmp/portable-drive-provider-validation-backend-reviewed.log` and
`/tmp/portable-drive-provider-validation-fuse-reviewed.log`. Backend all-target
clippy with warnings denied and repository formatting pass. The native test uses
a separate helper process: the initial in-process test design deadlocked while
forking with its own FUSE client handle. Both failed disposable mounts/processes
were retired and their owned fixture directories removed.

The original architecture result remains REGRESSION, with the same 4301 clone
groups, 7.16% duplication, average CCN 3.48, maximum 274 and 144 reported cycles.
Sharing timestamp-preserving mutation setup removed both newly reported fixture
clones; provider resolution CCN fell from 29 to 25. No broader gate is closed by
these tests. Report:
`.fitness/portable-drives-provider-validation-reviewed-architecture.json`.

## Partition-layout checkpoint

Drive observations now retain optional typed partition layout, and the existing
manifest source record persists it without changing the SQLite schema or backup
payload format. Facts include the partition number, fixed-512-sector start and
checked byte offset, reported logical sector size, type/label/flags and immediate
containing table type/identifier. Missing values and overflow stay unknown;
whole-volume/crypt/optical observations do not inherit a guessed layout. Older
records deserialize with no layout. Enrollment evidence remains unchanged.

Bounded inventory compatibility is separated from mount reconciliation. Injected
query responses cover missing optional columns, missing DISK-SEQ and both missing,
with at most three attempts and no invented lifetime. Parser tests cover 4 KiB
logical sectors, zero offsets, conversion overflow, immediate nested-table ownership
and legacy JSON. The optional layout is boxed to keep the frequently passed Drive
record compact rather than inflating the retained partition completion enum.

`/tmp/portable-drive-vm-partition-layout-final/report.json` passes with three
successful filesystem markers. Real guest FAT32/exFAT/NTFS tests each assert GPT
partition number one, 2048 fixed sectors / 1 MiB offset, logical sector size,
table UUID/type and partition type; the checksummed completed image manifest
retains the exact observed layout. Existing authorized/denied access, busy-source,
source/image hashes, read-only providers and restoration checks also pass.

Final affected-package tests pass 84 backend cases (27 explicit fixtures ignored),
979 Files (six ignored), 451 core (three ignored), six CLI, 35 daemon, selected
integrations and twelve core doctests. Log:
`/tmp/portable-drive-partition-layout-packages-final.log`. All-target clippy with
warnings denied passes across those five packages. The normal helper build,
core/backend documentation and repository formatting pass. Rendered PartitionLayout
documentation was inspected for all nine fields and the partition-only contract.

The original architecture result remains REGRESSION: 4301 clone groups, 7.16%
duplication, average CCN 3.48, maximum 274 and 144 reported cycles. This change
adds no clone group; separating the command compatibility stage removes the
reported discovery complexity hotspot. Raw report:
`.fitness/portable-drives-partition-layout-final-architecture.json`.

## Complete exclusion-rule traversal checkpoint

Catalog capture and index-policy publication now use the existing transactional
keyset visitor for every saved rule. RAM-session/allocation exclusions also use
that visitor, and managed backup paths are collected within the same retained
snapshot. The old thousand-record cutoffs are removed. Publication sorts and
deduplicates paths, and compares an existing registry against the bounded new
snapshot length so an unchanged oversized policy is not rewritten repeatedly.
Index clients still reject oversized/malformed/unreadable registries entirely.

Both new regression tests failed before the fix: catalog capture entered an old
excluded subtree (three records instead of two), and a separate core policy client
missed the oldest non-UTF-8 exclusion. They now pass, including whole-drive catalog
refusal, all 1026 policy entries, child-path admission, unrelated allowed paths,
malformed/oversized fail-closed behavior and restored-policy recovery. The client
probe runs after service shutdown to prevent periodic publication from racing its
intentional registry faults. Logs:
`/tmp/portable-drive-policy-pages-before.log`,
`/tmp/portable-drive-policy-pages-after.log`, and
`/tmp/portable-drive-policy-pages-client-final.log`.

All 86 backend tests pass (28 explicit fixtures ignored), with backend all-target
clippy/warnings denied and repository formatting passing. The architecture result
remains REGRESSION, with no additional clone group (4301), 7.15% duplication,
average CCN 3.48, maximum 274 and 144 reported cycles. The capture/publication
functions retain necessary explicit snapshot and whole-root refusal branches;
there is no second pagination mechanism. Report:
`.fitness/portable-drives-policy-pages-architecture.json`.

At this checkpoint the editor still needed a direct lookup. The following
checkpoint closes that gap without removing status pagination.

## Exact policy-editor lookup checkpoint

GetRule is a read-only request with a lossless relative path. It uses the existing
stable rule key and a read-only SQLite connection, rejects mismatched stored
addresses and creates no operation. The wire envelope stays a direct Drives read.
Files reuses its bounded request queue and dialog generations to wait for the
exact reply; cancelled/stale replies are ignored, and failures or wrong-address
replies cannot open a default editor. Whole-drive settings now load saved flags
instead of unconditionally starting enabled. Default flags are displayed only
after a confirmed absent override; inherited exclusions still apply.

The CLI exposes `get-policy` and fixes omitted whole-partition subtrees in both
read/write policy and pairing commands. Policy writes require both booleans,
avoiding implicit resets when a flag is omitted. Rule execution publishes the
current index-policy file before reporting successful completion rather than
waiting for the five-second discovery cycle.

Backend tests cover exact root/raw-byte rules older than the status page, missing
rules, invalid paths, corrupt key/address pairs and zero operations from reads.
Core IPC tests roundtrip the raw path and typed reply while verifying direct-read
classification. Files tests cover deferred editing, stale/cancelled replies,
lookup failures and mismatched/unexpected responses. CLI tests cover root/folder
reads, omitted root subtrees and rejection of incomplete policy writes.

Dark and light private desktop runs pass `--policy-editor-only`. They seed two
independent flag combinations before 105 newer rules, verify both are absent
from the first status page, reopen the actual root/folder editors, save without
changing a checkbox, wait for completed UI mutations and query the resulting
stored flags. Both original combinations are retained. Screenshots were inspected
for checkbox state and complete layout. Reports:
`/tmp/portable-drive-policy-editor-dark-final/report.json` and
`/tmp/portable-drive-policy-editor-light-final/report.json`.

Affected-package tests pass 88 backend cases (28 explicit fixtures ignored), 981
Files (six ignored), 452 core (three ignored), eight CLI and 35 daemon tests, plus
selected integrations and twelve core doctests. Full package log:
`/tmp/portable-drive-rule-lookup-packages-final.log`; final backend fixture review
and CLI checks also pass in the corresponding `backend-final`/`cli-final` logs.
Selected-package all-target clippy/warnings denied, normal binaries, the owned
fixture helper, core/backend API docs and repository formatting pass. Rendered
GetRule/Rule documentation was inspected. The initial wrapped-menu OCR failure
was corrected by targeting its visible second line; product rendering was intact.

The original architecture gate remains REGRESSION, with no additional clone group
(4301), duplication 7.15%, average CCN 3.48, maximum 274 and 144 reported cycles.
Tests reuse the existing TestDrives owner and share the published-policy reader
with backup tests; both new fixture clones were removed. The product reuses the
existing IPC/SQLite/request-generation mechanisms. Raw report:
`.fitness/portable-drives-rule-lookup-verified-architecture.json`.

## Cross-cutting gates

- Preserve a headless core/backend and independent file-manager lifecycle; no workbench application dependency.
- Preserve directory/source identities, lossless paths, accepted write ownership and completed mutations through failures/cancellation.
- Verify every section's delivery gate against the corresponding item matrix, including real filesystem/authorization flows.
- Keep implementation commits reviewable and leave personal configuration edits uncommitted.
- Run the final workspace/format/clippy/architecture checks only after the remaining implementation changes, and retain measured filesystem/UI evidence.

The goal remains active while any row or cross-cutting gate lacks direct completion evidence.

RAM-export follow-up: Save RAM now uses the retained reconciliation pool and held source/output roots. Cancellation retains completed output; a retry into a new destination preserves directory modes/times. Export status and the last successful export are separate persisted fields from media writeback, with backward-compatible record defaults. The native removal/reconnect/restart fixture proves that an offline export leaves the media baseline unchanged and reports its saved location. Backend tests: 36 passed, nine explicit fixtures ignored; the selected native lifecycle fixture, two core drive-schema tests and file-manager compilation passed. Pending writeback counts, large-export responsiveness acceptance and the other remaining audit gates still need direct evidence. Architecture comparison is unchanged in its material result: REGRESSION, seventeen net new clone groups, 7.23% duplication and average CCN 3.48. Report: `.fitness/portable-drives-ram-export-architecture.json`.

## RAM follow-up verification

The standalone filesystem companion is packaged in commit `9c41da5c` with source,
license, pinned upstream checksum and local atomic-NOREPLACE patch. Ordinary backend
checks passed 30 tests with seven explicit host fixtures ignored. Explicit native runs
passed RAM byte/inode exhaustion, aggregate reservation refusal, live-helper ownership,
open-file graceful restart, forced-exit directory-fsync durability, daemon removal/reconnect/restart and concurrent-backup
writeback tests. Sparse logical growth is rejected before reading its payload. A cancellation test first reproduced stale enrollment metadata overwriting a live mount record, then passed after preserving the updated backing identity.

The architecture comparison against the original baseline still reports REGRESSION:
7.30% to 7.24% duplication, average CCN 3.44 to 3.47, max 274 unchanged, dependency
cycles 143 unchanged, thirteen net new clone groups. Shared comparison limits and
owned-mount recognition remove repeated policy. Remaining backup/publication and
process-adapter duplication/complexity stays an open review item; the result is not
represented as a passing gate. Raw report: `.fitness/portable-drives-ram-architecture.json`.


Shutdown follow-up: native test-support IPC verifies that SIGTERM exits successfully
and a still-open, un-fsynced RAM file survives restart while its sync pair stays
paused (the original source never receives the file). Launcher mocks verify that a
busy stop retains its PID/socket and never sends SIGKILL. `systemd-analyze --user
verify little-screen-clip.service` and shell syntax checks pass. The actual running
user daemon and installed systemd configuration have not been changed.

Latest shutdown architecture checkpoint: duplication 7.30% → 7.23%, average CCN
3.44 → 3.47, max 274 unchanged, cycles 143 unchanged. Fourteen net new clone groups
still make the original-baseline comparison a REGRESSION; adapter duplication and
new hotspots remain unfinished review work. Report:
`.fitness/portable-drives-shutdown-architecture.json`.

RAM pressure follow-up also needs a worst-case single-directory listing test:
upstream fuse2fs uses high-level readdir buffering. The native process is capped at
512 MiB, but successful paging/controlled failure for very large, long-name RAM
directories is not yet proven. The million-file catalog fixture does not prove
that separate RAM-server case.


Scheduling checkpoint: 35 ordinary backend tests pass with nine explicit native/scale
fixtures ignored. Six selected native RAM fixtures pass, including a 1 GB population
beside prompt independent RAM writeback, cancelled-copy resume and real normal-unmount
pin retirement. Full scans and RAM population retain task ownership to actual exit;
shutdown drains queued and running reconciliations. Pending automatic passes keep
arrival order and share bounded event batches. A caught worker panic preserves the
already-written fixture and leaves the pool able to execute its next job.

Watch registration reuses the scan's descriptor-backed frame, advances in 4096-entry /
five-millisecond slices, and discloses incomplete coverage through a forced full scan.
Event reads also yield after bounded bytes/time. This does not establish a hard deadline
for an individual filesystem metadata syscall. Remaining owner-thread setup is still
unfinished, as are the other listed plan and host gates. Save RAM subsequently moved
to the retained pool (see the export follow-up).


The scheduler architecture checkpoint remains REGRESSION against the initial baseline:
7.30% → 7.23% duplication, average CCN +0.04, max 274 unchanged, cycles unchanged,
seventeen net new clone groups. Shared two-drive fixtures and bounded operation/content
waiters removed six accidental clone groups from the first scheduler measurement.
The remaining domain guards/adapters and hotspots still need the final architecture
review. Report: `.fitness/portable-drives-scheduler-final-architecture.json`.
A queued-cancellation fixture additionally verifies that a cancelled Save RAM request
never creates its destination during admission.

Preference-persistence checkpoint: the backup picker writes only its selected path
through core's comment-preserving TOML editor and guarded atomic file installation.
The daemon captures its configuration path at startup; owned fixtures inject a
private path or disable preference-file writes. Selection/restart, failed writes,
explicit reload-to-unset and stale-SQLite refusal pass the service fixture. Core
tests cover retained comments, modes, hard links, symlinks, raw path bytes,
concurrent edits and parent replacement. Validation: 447 core tests passed (three
host fixtures ignored), all fourteen configuration tests passed again after the
final read-admission simplification, 37 backend tests passed (nine explicit fixtures
ignored), and core/backend all-target clippy passed with warnings denied.

Architecture still reports REGRESSION: seventeen net new clone groups, duplication
7.30% to 7.23%, average CCN 3.48, max 274 and cycles 143 unchanged. The new guarded
configuration save is also reported at CCN 20 and remains a review hotspot;
the architecture gate has not passed. Report:
`.fitness/portable-drives-settings-architecture.json`.


RAM-preparation checkpoint: source inventory, reservation, formatter and initial
mount now run on one retained worker. Quota reloads and further RAM admissions
wait until it exits. A held-preflight native fixture proves that another real RAM
session still flushes an edit and accepts Pause; cancellation keeps the first
operation unfinished until its worker is released, then removes its unpublished
setup before applying a deferred quota reduction. Native removal/reconnect,
large-population, cancelled-population, quota and unmount-pin fixtures still pass.

Preparation journals its metadata scan before enumeration and its private image
before formatting. Session/pair publication removes that journal transactionally;
initial and recovered views share guarded mounting and atomic ready-state updates.
Fault-state fixtures prove that restart retires only the abandoned scan/empty
image, refuses replaced allocation directories, and preserves published edits.
Cancellation cleanup stays on the worker. Startup allocation cleanup and retained
RAM filesystem recovery still run during coordinator startup and need the separate
startup responsiveness acceptance matrix.

Validation: 38 ordinary backend tests passed with eleven explicit fixtures ignored;
eight selected native RAM fixtures passed in 9.70 seconds. Both setup recovery
fixtures and the updated quota-reduction/preflight fixture passed again after test
fixture consolidation. The rebuilt test-support IPC host passed all three shutdown
tests, including SIGTERM durability of an unclosed RAM file created through a
worker-owned native server. The actual user daemon remains untouched.

Architecture remains REGRESSION against the original baseline: seventeen net new
clone groups, duplication 7.30% to 7.22%, average CCN 3.48, max 274 and cycles 143
unchanged. Sharing source validation and mount publication removed repeated policy;
consolidating restart fixtures removed the new test clone. Admission and shared
mount guards still have CCN 17 and remain review hotspots. Report:
`.fitness/portable-drives-ram-preparation-architecture.json`.

Unmount-scope checkpoint: mount coordinates are observed once with each pinned root
and reused only as scheduling evidence. The scope itself owns no file descriptor;
a real FUSE fixture proves it does not keep a retired mount busy. Unknown same-device
coordinates conservatively overlap. Four root-policy tests, ordinary backend tests
and backend all-target clippy pass. The original architecture comparison remains
REGRESSION with seventeen net new clone groups and average CCN 3.48; this checkpoint
adds no clone group. Report: `.fitness/portable-drives-unmount-scope-architecture.json`.


Partition-preparation checkpoint: Eject and partition-image requests share retained
preflush/unmount stages on one bounded worker. Pair/RAM status updates are committed
together before advancing past each flush. All worker admission now shares the same
captured scope-overlap policy. Snapshot reuse avoids rediscovering the drive twice
per enrolled folder during a single pre-unmount admission. Image requests reserve
the future bulk slot and keep their admitted destination/configuration through the
handoff; in-flight destination exclusions survive preference changes.

Authority refusal and cancellation fixtures prove that no completed image is
advertised and no partition backup folder is created. A real owned FUSE fixture
first refuses normal unmount with an external pin; after releasing it, another RAM
session flushes an edit while unmount is held. Cancellation waits for that result
and records the completed eject, and remounting the owned fixture verifies the last
local edit was flushed. Actual UDisks/PolicyKit acceptance remains host-gated.

File backups also use the common managed-destination guard. A restarted native
fixture with a backup directory pointing into private RAM backing refuses the
operation before creating any backup entry or damaging the image.

Validation: 40 ordinary backend tests passed; nine native RAM fixtures passed in
11.89 seconds, and the added private-backing refusal fixture passed separately.
The rebuilt test-support host passed all three shutdown tests. Targeted backend
clippy passes with warnings denied; daemon, file-manager and CLI compilation also
passed. No user daemon or attached media were changed.

Remaining partition work includes precise size/free-space presentation, device-node
lifetime binding before/after authorized open, explicit remount restoration and
phase-specific durable restart recovery. These remain required rather than being
inferred from the narrower unmount tests. Architecture comparison still reports
REGRESSION; admission guards and the original clone/complexity backlog remain under
review. The final report for this checkpoint is recorded below.

Final partition checkpoint comparison remains REGRESSION: seventeen net new clone
groups, duplication 7.30% to 7.22%, average CCN 3.48, max 274 and cycles 143
unchanged. Consolidating admission policy, accepted-request fixtures and shared
barriers removed four new clone groups found in the first measurement. The partition
admission function remains a CCN-20 review hotspot. Report:
`.fitness/portable-drives-partition-preparation-architecture.json`.

Selected-root safety follow-up: enrollment and every later synchronization resolve
selected subtrees through the retained partition directory walk. A replacement
symlink cannot redirect a selected subtree outside that partition. Retained roots
also check their descriptor's current resolved name; a moved root makes its scheduling
coordinates uncertain until the task retires. Runtime root checks reuse enrollment's
managed-storage guard, with only the verified RAM session's own local view exempted.
Missing RAM-session metadata cannot authorize synchronization.

Owned fixtures prove that rejected linked enrollment creates no output folder,
later subtree replacement leaves unrelated host files untouched, relocation into
the source cannot recursively mirror itself, and relocation into managed backups
cannot write there. Validation: 43 ordinary backend tests passed (thirteen explicit
fixtures ignored), all ten selected native RAM/unmount fixtures passed in 12.68
seconds, and backend all-target clippy passed with warnings denied. Shared test
enrollment/completion helpers removed the two new fixture clones initially found.
The overall architecture result remains REGRESSION: seventeen net new clone groups,
7.22% duplication, average CCN 3.48; the common managed-storage guard is a CCN-16
review hotspot. Report: `.fitness/portable-drives-sync-root-architecture.json`.

Remaining identity coverage includes real bind/case-fold aliases, live mounted-source
binding to the selected block device, device-node lifetime checks through authorized
image open, and the full concurrent relocation/reader matrix. These are still open.

Device-lifetime checkpoint: image jobs now retain an O_PATH metadata pin from
admission through preflush, unmount, authorization, both image passes and publication.
The guard reuses core's full metadata fingerprint and namespace/mount observations.
The authorized descriptor must be a matching block device and return the originally
observed kernel disk sequence. Discovery keeps this transient sequence separate from
persistent enrollment evidence; old records deserialize with an unknown sequence.
An older lsblk can still supply ordinary inventory, but cannot admit an unverified
image backup.

Owned fixtures reject a device pathname replaced while unmount is pending, reject
ordinary-file authorization descriptors and unknown sequences, and inject a lifetime
failure at image verification without replacing the previous completed image. These
fixtures do not substitute for real PolicyKit/block-device acceptance. Live mounted
source binding and the full real-media identity matrix remain open.

Validation: 45 ordinary backend tests pass (thirteen explicit fixtures ignored).
All ten selected native RAM/unmount fixtures also pass (12.25 seconds). Backend
all-target clippy passes with warnings denied, and the daemon, file manager and CLI
compile. Sharing the two-pass
read loop and held-unmount fixture removed the additional clone groups discovered
during this checkpoint. Architecture still reports REGRESSION against the original
baseline: seventeen net new clone groups, duplication 7.30% to 7.21%, average CCN
3.48, max 274 and dependency cycles 143 unchanged. Report:
`.fitness/portable-drives-device-lifetime-architecture.json`.

Bulk-root checkpoint: file backup and catalog collection receive their admitted
source roots instead of reopening mount pathnames inside the helper. File and
image backups similarly receive the admitted destination root; UUID folders are
resolved through its retained directory walk. Btrfs commands and image staging use
the selected backup parent's retained handle. Catalog collection receives its
admitted state root and creates the catalog database through that handle.

Owned source/destination/state replacement fixtures refuse collection without
touching replacement canaries or publishing a new completed artifact. These are
admitted-root checks; they do not prove the remaining mounted-block binding,
concurrent bind-alias or all operational SQLite state-path replacement cases.
File/catalog fixtures now retain their roots across invocations, sharing admission
and version setup rather than repeating pathname opens at every call.

Validation: 47 ordinary backend tests pass (thirteen explicit fixtures ignored).
The real Btrfs file/image test passes with each 4 KiB edit allocating 256 KiB
exclusively in an 8 MiB version and preserving the original bytes. The real FUSE
catalog fixture passes offline metadata, unavailable-content errors, backup reads,
excluded directories, write rejection and provider recursion refusal. The native
concurrent-backup fixture also passes independent RAM writeback and cancellation.
Daemon, file-manager and CLI compilation passes. Backend
all-target clippy passes with warnings denied. Architecture remains REGRESSION,
with twelve net new clone groups (down from seventeen at the previous checkpoint),
7.21% duplication and average CCN 3.48; max CCN 274 and cycles 143 are unchanged.
Report: `.fitness/portable-drives-bulk-roots-architecture.json`.

Partition-checkpoint follow-up: existing operation records now retain the selected
drive/destination, typed flush/unmount/image phase, whether this operation confirmed
the unmount, and a completed image's identifier. Image progress distinguishes copying
from byte-for-byte verification. Critical phase recording failures stop further
image admission/publication; a journal failure after actual publication is reported
without discarding the already-completed image result. Unmount completion is delivered
to its owner before recording the next checkpoint, preserving the known effect if
checkpointing fails.

Restart retains those facts and reports an interrupted authorization request as an
uncertain effect instead of declaring it failed. No mount action is replayed from
the journal. Eight owned interruption states and legacy records pass restart checks;
owned image tests verify phase order, verification-time source failure and checkpoint
failure before publication while retaining the previous current image. The details
window shows the selected partition size and destination from the persisted context.
The explicit restoration option and pre-admission space/size presentation remain
unfinished at this checkpoint.

Validation: 48 ordinary backend tests pass, backend all-target clippy passes with
warnings denied, and daemon/file-manager/CLI compilation passes. The real native
FUSE unmount/cancellation fixture and real Btrfs file/image delta fixture also pass.
The original-baseline architecture comparison still reports REGRESSION with twelve
net new clone groups, 7.21% duplication, average CCN 3.48, max CCN 274 and cycles 143.
Separating the existing operation-row renderer keeps the new partition context from
introducing a details-list complexity hotspot. Report:
`.fitness/portable-drives-partition-checkpoints-architecture.json`.

Mounted-state restoration follow-up: image requests expose an opt-in setting in the
drive menu and `--image --restore-mounted` in the CLI. Legacy requests default to
false; file backups reject the flag. Cleanup runs on retained workers after a
confirmed job-owned unmount, even after cancellation or an imaging failure. It
revalidates the original device lifetime and observed partition before and after
mount authority, preserves original read-only access, and does not mount an
originally unmounted or already restored partition. Scheduling claims survive the
image/cleanup handoff while independent drives continue reconciling.

Typed restoration checkpoints retain separate errors. A completed backup with a
remount failure remains published and ends with `complete_with_warning`. Owned
fixtures cover held mount authority, cancellation after actual unmount, changed
device/node refusal, declined image open, readonly restoration, legacy requests,
and a real owned image publication whose later remount fails. The extended native
FUSE fixture performs a real unmount/remount around an injected image-open refusal;
this does not prove successful authorized raw-device imaging. Pre-admission space
estimates and reviewed selection/destination binding remain unfinished.

Validation: 54 ordinary backend tests pass, with thirteen explicit fixtures ignored;
all ten selected RAM/native fixtures pass separately. All three private shutdown
transport tests also pass after rebuilding the owned fixture. Backend all-target
clippy with warnings denied and daemon/file-manager/CLI compilation passed. Architecture still
reports REGRESSION against the original baseline: eleven net new clone groups,
7.20% duplication, average CCN 3.48, max CCN 274 and cycles 143. Shared completion,
authority and fixture paths remove duplicate logic; the image menu stays below the
complexity hotspot threshold. Report:
`.fitness/portable-drives-image-restoration-architecture.json`.

Backup-review follow-up: the destination prompt now requests a read-only daemon
preview instead of queuing independent preference and backup mutations. A single-use,
five-minute token retains the selected drive observation and source/destination
handles. Execution revalidates those facts, uses the captured destination even after
preferences change, and drops source review pins before unmount. Sixteen pending
reviews bound retained resources; cancellation, expiry, overlapping unmount and
shutdown retire them. GUI generations prevent late results/errors from replacing a
newer selection, and cancelled review cleanup does not emit a status notice.

The review shows exact partition capacity, destination and free bytes. Independent
images require their full size plus 64 MiB working headroom before unmount; Btrfs
permits shared/sparse allocation with an explicit full-image estimate and warning.
File admission checks working headroom without promising a precise allocation
estimate. Destination effective-user permissions and read-only state are checked,
and execution repeats space checks. A requested preference save must succeed before
copy/unmount admission. Full source traversal estimates, destination quota/capability
faults and final authorized-device acceptance remain under their original gates.

One-shot backup roots are durably registered before collection and policy publication.
They retain no-index and sync-recursion protection after success, publication failure,
preference changes and restart. Older completed versions supply historical roots too.
Real bind/relocation aliases and every recursive consumer still need their full audit.
Private UI captures showed ordinary folder previews rendering backup descendant
metadata. The no-index consumer follow-up below addresses that entry point;
the broader alias and consumer inventory remain unfinished.

UI verification also exposed two independent defects: mount row IDs followed list
positions, and long notification labels pushed Dismiss outside the window. Commits
`cc37ec81` and `e42a1e7e` fix these with regression tests that failed beforehand and
pass afterward. The driver now observes job completion/notice dismissal before
measuring the next menu position and verifies small bordered captions separately.

Focused dark/light backup-review acceptance passed with nineteen observations each:
`/tmp/portable-drive-review-focus-dark-verified/report.json` and
`/tmp/portable-drive-review-focus-light-verified/report.json`. These runs cover lazy
selection, cancellation without preferences, file publication, and image-review
cancellation; they do not claim raw-device authorization or the full lifecycle matrix.

Validation for the reviewed-backup checkpoint: 63 ordinary backend tests pass with
fourteen explicit fixtures ignored; all ten selected RAM/native fixtures pass, as
does the new real Btrfs shared-allocation review fixture. File-manager tests pass
965 ordinary cases with six intentionally ignored; two core drive-schema tests and
three private shutdown/transport tests pass. Affected daemon/file-manager/CLI
all-target compilation and clippy with warnings denied pass, and CLI review/start
help exposes the new options. Rustfmt and diff whitespace checks pass for changed
sources. These are affected-component checks, not a new final-workspace claim.

The expanded full private UI scenario passes in both appearances with 47 observations
and no cleanup error: `/tmp/portable-drive-full-review-dark-verified/report.json`
and `/tmp/portable-drive-full-review-light-verified/report.json`. It covers the prior
RAM/sync/catalog/reconnect flow plus review cancellation, file publication and image
review, with bounded-caption OCR checks for menu and dismissal controls.

Architecture still reports REGRESSION against the original baseline: eleven net new
clone groups, 7.19% duplication, average CCN 3.48, max CCN 274 and cycles 143. This
checkpoint removes its initial extra clone groups by sharing backup admission,
settings persistence and owned review fixtures, and separates preview capture from
registration. The new preflight functions stay below the CCN hotspot threshold;
original feature-wide clone/complexity findings remain for final review. Report:
`.fitness/portable-drives-backup-review-architecture.json`.


No-index consumer follow-up: Shelf file-reference summaries now stop before enumerating
an excluded directory or reading its payload. Explicit browser navigation and guarded
individual-file reads remain available. Recursive photo discovery checks each directory
and entry before admission and does not follow child symlinks. Photo feature indexing
checks cached hits, decoder admission and completed features, so a rule introduced
during model initialization prevents fallback decoding and persistence. Audio discovery
checks before opening each directory; analysis, cached database loads and final snapshot
publication also apply the shared drive policy. A completed rescan removes excluded
sample observations from its replaced snapshot.

Four isolated policy fixtures pass without mutating the parent test environment or
using the live registry. They prove excluded/missing paths are rejected before opening,
policy changes invalidate cached observations, a valid photo is decodable when admitted
but produces no feature after exclusion during initialization, directory/file symlinks
cannot bypass recursive photo discovery, and direct navigation/reads remain possible.
The full file-manager library suite passes 969 cases (six ignored); the final scanner
suite passes five cases after adding symlink refusal, and the four policy fixtures
pass again with the valid-image positive control. Logs are
`/tmp/portable-drive-noindex-files-tests.log`,
`/tmp/portable-drive-noindex-scanner-final.log` and
`/tmp/portable-drive-noindex-tests-final.log`.

This does not yet prove every recursive consumer or every alias, nor immediate
retirement of already displayed UI caches after a policy change. Those requirements
remain open alongside the catalog redaction/coverage work.

File-manager all-target clippy with warnings denied passes for this checkpoint
(`/tmp/portable-drive-noindex-clippy.log`), as do explicit changed-source rustfmt and
diff whitespace checks. The implementation reuses core's policy registry without a
second exclusion mechanism or new dependency. Additional traversal/admission checks
increase existing scanner and decoder CCN; those branches enforce distinct pre-descent,
pre-decode and post-analysis boundaries rather than duplicate traversal algorithms.

The final no-index architecture checkpoint still reports REGRESSION against the
original baseline: eleven net new clone groups (unchanged from the preceding
backup-review checkpoint), 7.19% duplication, average CCN 3.48, maximum 274 and
143 cycles. Current touched hotspots include sample loading (23), audio discovery
(20), audio processing (16), photo discovery (22) and neural batching (17). Their
added policy/symlink boundaries are necessary; broader feature-wide clone and
complexity cleanup remains unfinished. The saved report is
`.fitness/portable-drives-noindex-architecture.json`.


Live-policy follow-up: core exposes an immutable view of its existing bounded policy
cache. Metadata observation stays on the drive-status polling worker, including when
status IPC returns an error. Core reuses the shared inventory identity (device, inode,
size, precise mtime/ctime and mode), rejects non-regular policy registries, preserves
fail-closed malformed/unreadable handling, and normalizes equivalent rule lists to
retain their snapshot identity. The first observed empty policy leaves existing views
alone; unavailable or changed policies generate a UI invalidation once per observation.

The file-manager handoff clears Shelf preview caches and retires photo similarity
jobs/features. Audio discovery cancels its active index and refreshes the current map
with a new generation, rejecting old folder completions. Explicit playback, the sample
bank, collected Shelf items and accepted save/send completions retain their owners.
This is observation-driven through the existing periodic status worker; it does not
claim synchronous notification at the instant another process replaces the policy.
Broader search/answer/image-order retirement remains under item 6.3, along with aliases.


Disposable-guest checkpoint: `6b876b69` adds a reusable QEMU/KVM runner and native
FAT32/exFAT/NTFS fixture script. This host's kernel/modules and signature-verified
temporary QEMU/BusyBox/ntfsprogs packages boot successfully. Host `/usr`, repository
code and optional tools are read-only exports; the guest has no network or host block
devices, and its writable loop images live in guest RAM. No live daemon was restarted
and no attached user media were mounted or written.

Native format/write/unmount/remount probes pass for all three drivers. An injected
failed command is rejected even though QEMU exits zero; an injected sleep produces a
retained timeout report after its first mounted filesystem. Ordinary host execution
is refused before fixture creation. See `scripts/drive_tools/VM_TESTING.md` and the
three reports under `/tmp/portable-drive-vm-harness-{smoke,refusal,timeout}`. These
prove the environment, not application sync, backup, catalog or authorization behavior;
those acceptance cases remain unfinished. The harness's five Python functions have
maximum CCN 10 and introduce no reported clone in the scripts measurement. The full
feature checkpoint remains REGRESSION: eleven net new clone groups, duplication
7.18%, average CCN 3.48, maximum 274 and cycles 143. Reports:
`.fitness/portable-drive-vm-harness-measure.json` and
`.fitness/portable-drives-vm-policy-architecture.json`.

Live-policy validation: all seventeen policy-focused file-manager tests pass against
the final core implementation, and the complete library suite passes 975 tests with
six intentionally ignored. These include immutable/equivalent/malformed/removed
registry snapshots, policy handoff despite failed status IPC, one-time UI retirement,
rejected stale map completions, retained Shelf send completion and retained sample-pack
save completion. Changed-source rustfmt and whitespace checks pass. Logs:
`/tmp/portable-drive-live-policy-tests-final.log` and
`/tmp/portable-drive-live-policy-files-tests.log`. All-target clippy and rendered API
documentation remain to be checked before the final feature gate; the current backend
native-test compilation is still in progress.


Native filesystem checkpoint: the final isolated guest run passed eight backend
cases on each of FAT32, exFAT and kernel NTFS3. File and directory names share one
scenario table; every successful copy verifies the actual literal directory entry
and payload, rather than relying on alias lookups. The tests reproduced NTFS3
staging rename silently installing a basename prefix and FAT32/exFAT dropping a
trailing dot. Pinned-mount filesystem checks now refuse those ambiguous paths;
ordinary Unix colon/trailing-dot/trailing-space names remain usable. No attached
user medium or host mount/elevation operation was used.

Evidence: `/tmp/portable-drive-vm-native-app-final/report.json` and `guest.log`.
Before combining the equivalent file/directory cases, ordinary backend tests passed
64 tests with 23 explicit fixtures ignored. Core/backend/file-manager all-target
clippy passed with warnings denied; final consolidated-test clippy is still to run.
The original-baseline architecture gate remains REGRESSION: duplication 7.30% to
7.18%, average CCN 3.44 to 3.48, max 274 and cycles 143 unchanged, twelve net new
clone groups. Common filename scenarios were consolidated; remaining short conflict
assertions retain independent native and ordinary regression coverage. Raw report:
`.fitness/portable-drives-native-names-architecture.json`.


Image-admission checkpoint: the existing FAT32 single-file limit is now shared by
ordinary copies, image review and image execution. Real FAT32 fixtures prove that
both reviewed and direct service requests refuse a 4 GiB image before unmounting,
while execution refuses before invoking source validation or copying. The exact
maximum of 4,294,967,295 bytes passes the filesystem-size check; free-space
admission remains separate. No incomplete image or backup bucket is created.
Evidence: `/tmp/portable-drive-vm-fat-image-size/report.json` (the FAT32-specific
assertions return early on exFAT/NTFS3). All 64 ordinary backend tests pass, with
25 explicit fixtures ignored. All twelve core doctests pass; rendered docs and
post-change clippy are still pending.


Authorized imaging checkpoint: `/tmp/portable-drive-vm-authorized-images-final/report.json`
passes all three GPT partition profiles using real guest UDisks and PolicyKit.
A UID-65534 child proves actual NotAuthorized denial with an explicit execution
marker. An open source file proves busy-unmount refusal. Successful images have
exact partition sizes and independently checked full-device SHA-256 values.
Read-only loop mounts supply original file bytes and reject writes with EROFS;
a second backup restores the original mount and leaves the first image unchanged.
A replacement block node with the same rdev is refused, as is an authorized
file descriptor with a mismatched kernel disk sequence.

These tests exposed and fixed the incorrect UDisks Manager object path and an
incorrect full-file timestamp comparison for block nodes. UDisks/udev update node
mtime/ctime while retaining the same inode and device. The O_PATH admission pin,
mount/object identity, block type, rdev and authorized BLKGETDISKSEQ remain checked;
ordinary-file fingerprints retain their full revision checks. Existing replacement
and namespace/disk-lifetime tests remain intact. Ordinary backend tests: 64 pass,
25 ignored; core/backend/file-manager all-target clippy passes with warnings denied.
Core doctests pass all twelve examples, and `cargo doc --no-deps` renders the
immutable policy API with its resolved Arc link and no warnings.

The original-baseline architecture comparison still reports REGRESSION: duplication
7.30% to 7.18%, average CCN 3.44 to 3.48, max 274 and cycles 143 unchanged, twelve
net new clone groups. This checkpoint adds no new reported hotspot. The remaining
branch-wide architecture review is still open. Report:
`.fitness/portable-drives-native-images-architecture.json`.


Image/catalog-provider checkpoint: the current helper build and complete guest
flow pass in `/tmp/portable-drive-vm-image-catalog-final/report.json`. Each source
profile exercises exact image/manifest checksums, the captured Drive record and
published current-image pointer, live FUSE metadata while the source is unmounted,
image-backed payload reads, EROFS, ENOMEDIUM after provider retirement, and explicit
catalog unmounting before source remount. The guest now supplies util-linux's
`/bin/umount` path because fusermount3 uses options absent from BusyBox umount.
Core/backend/file-manager all-target clippy passes again with warnings denied;
changed Rust files pass rustfmt checks, and Python/shell syntax checks pass.

**Still unfinished:** UDisks on this system selects exFAT FUSE when restoring or
mounting an image. It reports zero fractional mtime where the original kernel
exFAT mount reported 120 ms. Metadata-only catalog entries with that precision
loss are rejected with ESTALE; whole-second entries remain readable. The native
fixture explicitly exercises both outcomes rather than silently relaxing revision
validation. This does not establish full cross-driver catalog compatibility, and
that requirement remains open alongside the other rows. The source files and
images still retain their original bytes.


The final image/catalog architecture checkpoint retains the same material gate
result: REGRESSION, twelve net new clone groups, duplication 7.18%, average CCN
3.48, max 274 and cycles 143. The native image/provider test adds no new reported
hotspot or clone group. Raw report:
`.fitness/portable-drives-image-catalog-architecture.json`.


Request-admission checkpoint: retained mutations now share one durable UUID namespace
for client-chosen and legacy server-generated identities. SQLite commits the
operation and SHA-256 fingerprint of the typed request together under the existing
admission lock. Matching retries return the existing ID before consuming a backup
review; changed parameters, missing records and unbound older operations fail
closed. Known queue rejection atomically removes both new records and restores the
review. If cleanup itself fails, a terminal failed operation is retained. Replaying
an unfinished record with no live owner marks it interrupted without executing it.

Files retries one transport failure with an identical SubmitDrive envelope, while
server errors and direct status/review/cancel controls are not repeated. The CLI
accepts an explicit --request-id and reports its generated identity if a reply is
lost. The real daemon dispatch, CLI parser, lossless protocol roundtrip and the
owned IPC fixture cover this path without touching a user daemon or device.

Evidence: `/tmp/portable-drive-admission-ipc-final.log` loses the first socket reply,
repeats it through twelve concurrent connections, checks one complete backup with
its original bytes, refuses changed parameters, restarts the host and obtains the
same completed operation/version. Backend fault fixtures cover rollback and failed
cleanup, pending and terminal replay, restart and live orphan recovery. Before consolidating the two ownership-loss scenarios, full
checks passed 71 backend tests (25 explicit fixtures ignored), 977 file-manager tests
(six ignored), 448 core tests (three ignored), all nine core IPC tests, the CLI parser
and daemon dispatch tests, and twelve doctests. The final consolidated suite passes 70 backend tests (25 ignored), with both
restart and live-owner-loss branches still executed. The current all-target build
and clippy (warnings denied) pass across backend, core, file manager, CLI and daemon.
Rendered submit/constructor/classification documentation was inspected, including
its resolved Self::request link; rustfmt and Python compilation checks pass.


Final admission evidence: `/tmp/portable-drive-admission-cli-ipc.log` contains two
passing owned socket tests. The second forwards the actual CLI request to the host
but discards its acknowledgement, verifies the CLI prints the request identity,
and retries the same command successfully without another operation. The first
also replays a consumed backup review through the actual CLI after host restart.
Native FAT32/exFAT/NTFS image, restoration and catalog-provider fixtures pass again
in `/tmp/portable-drive-vm-admission-images/report.json`; the separately documented
exFAT fractional-timestamp limitation is unchanged.

The original-baseline architecture result remains REGRESSION: duplication 7.30% to
7.17%, average CCN 3.44 to 3.48, max 274 and cycles 143 unchanged, twelve net new
clone groups. The admission implementation introduces no reported hotspot or clone;
shared cold/live ownership-loss scenarios removed the extra test clone. The broader
architecture gate remains open. Report:
`.fitness/portable-drives-admission-final-architecture.json`.
